//! E2 — the world authority: who owns the CPU voxel world, on which thread, and
//! how the render thread learns what changed.
//!
//! ONE type serves both measured variants, so the A/B is a lever and not two code
//! paths:
//!
//! - **variant A, `world_thread = false`** — [`WorldHost::request_edit`] applies
//!   the edit inline on the calling (frame) thread and queues the delta.
//! - **variant B, `world_thread = true`** — the edit crosses a channel to a
//!   worker thread that owns the write side of the brickmap; the frame thread
//!   only drains finished deltas. The frame never blocks on an edit, a
//!   clearance-field rebuild, or a CAGI attribute rebuild.
//!
//! WHY A SHARED BRICKMAP AND NOT THE PLAN'S `Arc<Brickmap>` SNAPSHOT SWAP: the
//! plan's threading sketch proposed publishing immutable snapshots. Measured, that
//! design costs a **full deep copy of the brickmap per published edit** (the
//! bench's E2 section prints the number — tens of milliseconds for ~45 MB of
//! arrays), because a snapshot cannot be mutated in place while a reader holds it.
//! The delta is 576 bytes; the snapshot is 45 MB. So the authority keeps ONE
//! brickmap behind an `RwLock` and publishes the delta instead:
//!
//! - the **render thread never locks** — its uploads are owned
//!   [`WorldDelta`]s drained from a channel;
//! - **readers** (voxel picking today, atrium's `VoxelDdaResolver` at E8 — it runs
//!   on a background thread, not in the audio callback) take a read lock, which is
//!   uncontended except for the microseconds a write holds it;
//! - the CPU mirror is therefore *always* fresh, with no readback and no epoch
//!   skew, which is the property the audio side needs.

use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, RwLock, RwLockReadGuard};
use std::thread::JoinHandle;

use crate::brickmap::Brickmap;
use crate::cagi::{CagiGrid, GpuEventResponse, MaterialAttributes, EVENT_RESPONSE_SLOTS};
use crate::world_edit::{apply, VoxelEdit, WorldDelta, WorldEditSettings};

/// Something the render thread must apply, produced by the authority.
#[derive(Clone, Debug, PartialEq)]
pub enum WorldUpdate {
    /// An applied edit's GPU delta.
    Delta(WorldDelta),
    /// A rebuilt CAGI attribute buffer (the E4 resolution switch, off-frame).
    LightAttributes {
        grid: CagiGrid,
        attributes: Vec<u32>,
        emissions: Vec<[f32; 4]>,
        /// S3b — the response rows the slot indices inside `attributes` point at.
        ///
        /// Carried WITH the words rather than read from the live table on
        /// arrival: the material set may have changed again while this built,
        /// and installing new words against a newer table would point cells at
        /// the wrong response.
        ///
        /// Boxed because this variant travels by value down a channel and the
        /// table is 384 bytes against three `Vec` handles — every OTHER update,
        /// including the per-edit `Delta`, would have paid for it. One
        /// allocation per attribute rebuild, next to a build that just walked
        /// 37 M voxels.
        responses: Box<[GpuEventResponse; EVENT_RESPONSE_SLOTS]>,
        build_micros: f32,
    },
}

/// Running totals for the overlay readout and the bench.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WorldEditStats {
    pub edits_applied: u64,
    pub edits_ignored: u64,
    /// One-metre world voxels changed.
    pub voxels_written: u64,
    pub last_apply_micros: f32,
    pub last_upload_bytes: usize,
    pub total_upload_bytes: u64,
}

enum WorkerRequest {
    Edit(VoxelEdit, WorldEditSettings),
    LightAttributes(CagiGrid, MaterialAttributes),
    Stop,
}

struct Worker {
    requests: Sender<WorkerRequest>,
    results: Receiver<WorldUpdate>,
    handle: JoinHandle<()>,
    /// Requests sent minus updates drained — what the overlay shows as "pending"
    /// and what proves the frame is not waiting for them.
    in_flight: usize,
    /// Only one expensive full light-attribute sweep is useful at a time. Live
    /// material sliders can produce many newer states while it runs; retain
    /// only the newest one instead of replaying stale lighting for seconds.
    light_attributes_in_flight: bool,
    pending_light_attributes: Option<(CagiGrid, MaterialAttributes)>,
}

pub struct WorldHost {
    brickmap: Arc<RwLock<Brickmap>>,
    worker: Option<Worker>,
    /// Updates produced inline (variant A) or drained from a stopping worker.
    queued: Vec<WorldUpdate>,
    stats: WorldEditStats,
}

impl WorldHost {
    /// Take ownership of the world. Starts in variant A (inline); the caller
    /// applies the lever with [`Self::set_world_thread`].
    pub fn new(brickmap: Brickmap) -> WorldHost {
        WorldHost {
            brickmap: Arc::new(RwLock::new(brickmap)),
            worker: None,
            queued: Vec::new(),
            stats: WorldEditStats::default(),
        }
    }

    /// Read access to the authoritative world — voxel picking today, atrium's
    /// audio resolver at E8. Blocks only while an edit is being applied.
    pub fn read(&self) -> RwLockReadGuard<'_, Brickmap> {
        self.brickmap
            .read()
            .expect("the world lock is never poisoned: no panics inside it")
    }

    /// The shared handle, for a consumer that outlives one frame (E8's resolver
    /// thread takes one of these and never touches the renderer).
    pub fn shared(&self) -> Arc<RwLock<Brickmap>> {
        Arc::clone(&self.brickmap)
    }

    pub fn stats(&self) -> WorldEditStats {
        self.stats
    }

    /// Whether the authority currently runs on its own thread (variant B).
    pub fn is_threaded(&self) -> bool {
        self.worker.is_some()
    }

    /// Requests handed to the worker that have not produced an update yet.
    pub fn in_flight(&self) -> usize {
        self.worker.as_ref().map_or(0, |worker| worker.in_flight)
    }

    /// Apply the `world_thread` lever: spawn the worker, or stop it and drain what
    /// it had already finished.
    pub fn set_world_thread(&mut self, enabled: bool) {
        if enabled == self.is_threaded() {
            return;
        }
        if enabled {
            let (request_sender, request_receiver) = std::sync::mpsc::channel();
            let (result_sender, result_receiver) = std::sync::mpsc::channel();
            let brickmap = Arc::clone(&self.brickmap);
            let handle = std::thread::Builder::new()
                .name("voxel-rt world".to_string())
                .spawn(move || world_thread_main(brickmap, request_receiver, result_sender))
                .expect("failed to spawn the world thread");
            self.worker = Some(Worker {
                requests: request_sender,
                results: result_receiver,
                handle,
                in_flight: 0,
                light_attributes_in_flight: false,
                pending_light_attributes: None,
            });
        } else if let Some(worker) = self.worker.take() {
            let _ = worker.requests.send(WorkerRequest::Stop);
            let _ = worker.handle.join();
            // Everything it finished before stopping still has to reach the GPU.
            while let Ok(update) = worker.results.try_recv() {
                self.queued.push(update);
            }
        }
    }

    /// Queue one voxel edit. Never blocks the caller for more than a channel send
    /// in variant B; in variant A it *is* the edit.
    pub fn request_edit(&mut self, edit: VoxelEdit, settings: &WorldEditSettings) {
        match &mut self.worker {
            Some(worker) => {
                worker.in_flight += 1;
                worker
                    .requests
                    .send(WorkerRequest::Edit(edit, *settings))
                    .expect("the world thread outlives its sender");
            }
            None => {
                let mut brickmap = self
                    .brickmap
                    .write()
                    .expect("the world lock is never poisoned");
                match apply(&mut brickmap, &edit, settings) {
                    Some(delta) => self.queued.push(WorldUpdate::Delta(delta)),
                    None => self.stats.edits_ignored += 1,
                }
            }
        }
    }

    /// Rebuild the CAGI volume's static attributes for `grid` (E4's ~0.5 s sweep,
    /// which a GI resolution switch triggers). In variant B this happens on the
    /// world thread and arrives through [`Self::drain`]; in variant A it is applied
    /// inline and the frame pays for it — which is exactly the hitch E2 set out to
    /// remove.
    /// Rebuild the light volume's cell attributes.
    ///
    /// Takes the material attribute table because the rebuild reads it, and since S2
    /// those materials can be **live-edited**: the builders used to read the COMPILED
    /// table, which made this whole call a no-op for a material edit — it recomputed the
    /// same attributes it already had. Passing the live table in is what turns the
    /// panel's "re-pack GI attributes" button from a lie into the thing it says it is.
    pub fn request_light_attributes(
        &mut self,
        grid: CagiGrid,
        attribute_table: MaterialAttributes,
    ) {
        match &mut self.worker {
            Some(worker) => {
                if worker.light_attributes_in_flight {
                    worker.pending_light_attributes = Some((grid, attribute_table));
                    return;
                }
                worker.in_flight += 1;
                worker.light_attributes_in_flight = true;
                worker
                    .requests
                    .send(WorkerRequest::LightAttributes(grid, attribute_table))
                    .expect("the world thread outlives its sender");
            }
            None => {
                let brickmap = self.read();
                let started = std::time::Instant::now();
                let (attributes, emissions) = crate::cagi::build_cell_attributes_with_emission(
                    &brickmap,
                    &grid,
                    &attribute_table,
                );
                let build_micros = started.elapsed().as_secs_f32() * 1e6;
                drop(brickmap);
                self.queued.push(WorldUpdate::LightAttributes {
                    grid,
                    attributes,
                    emissions,
                    responses: Box::new(*attribute_table.responses()),
                    build_micros,
                });
            }
        }
    }

    /// Everything finished since the last call — what the render thread uploads.
    /// Non-blocking in both variants.
    pub fn drain(&mut self) -> Vec<WorldUpdate> {
        let mut updates = std::mem::take(&mut self.queued);
        if let Some(worker) = &mut self.worker {
            while let Ok(update) = worker.results.try_recv() {
                worker.in_flight = worker.in_flight.saturating_sub(1);
                if matches!(update, WorldUpdate::LightAttributes { .. }) {
                    worker.light_attributes_in_flight = false;
                    // A newer authored state exists. Never flash this obsolete
                    // result into the visible light volume.
                    if worker.pending_light_attributes.is_some() {
                        continue;
                    }
                }
                updates.push(update);
            }
            if !worker.light_attributes_in_flight {
                if let Some((grid, attributes)) = worker.pending_light_attributes.take() {
                    worker.in_flight += 1;
                    worker.light_attributes_in_flight = true;
                    worker
                        .requests
                        .send(WorkerRequest::LightAttributes(grid, attributes))
                        .expect("the world thread outlives its sender");
                }
            }
        }
        for update in &updates {
            if let WorldUpdate::Delta(delta) = update {
                self.stats.edits_applied += 1;
                self.stats.voxels_written += delta.voxels_written as u64;
                self.stats.last_apply_micros = delta.apply_micros;
                self.stats.last_upload_bytes = delta.upload_bytes();
                self.stats.total_upload_bytes += delta.upload_bytes() as u64;
            }
        }
        updates
    }
}

impl Drop for WorldHost {
    fn drop(&mut self) {
        self.set_world_thread(false);
    }
}

/// The world thread: apply requests in order, publish updates. It is the ONLY
/// writer of the brickmap, so the lock is never contended for more than one
/// edit's duration.
///
/// A dropped request channel (the host went away without stopping us) ends the
/// loop, so a panicking main thread cannot leave this thread alive.
fn world_thread_main(
    brickmap: Arc<RwLock<Brickmap>>,
    requests: Receiver<WorkerRequest>,
    results: Sender<WorldUpdate>,
) {
    while let Ok(request) = requests.recv() {
        match request {
            WorkerRequest::Stop => break,
            WorkerRequest::Edit(edit, settings) => {
                let delta = {
                    let mut brickmap = brickmap.write().expect("the world lock is never poisoned");
                    apply(&mut brickmap, &edit, &settings)
                };
                if let Some(delta) = delta {
                    if results.send(WorldUpdate::Delta(delta)).is_err() {
                        break;
                    }
                }
            }
            WorkerRequest::LightAttributes(grid, attribute_table) => {
                let started = std::time::Instant::now();
                let (attributes, emissions) = {
                    let brickmap = brickmap.read().expect("the world lock is never poisoned");
                    crate::cagi::build_cell_attributes_with_emission(
                        &brickmap,
                        &grid,
                        &attribute_table,
                    )
                };
                let update = WorldUpdate::LightAttributes {
                    grid,
                    attributes,
                    emissions,
                    responses: Box::new(*attribute_table.responses()),
                    build_micros: started.elapsed().as_secs_f32() * 1e6,
                };
                if results.send(update).is_err() {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::material_id;
    use voxel_core::world::{
        Voxel, VoxelWorld, WorldVoxelCoord, DETAIL_CELLS_PER_WORLD_VOXEL, WORLD_SIZE_Y,
    };

    fn island_host() -> WorldHost {
        WorldHost::new(Brickmap::build(&VoxelWorld::generate(1234, 0.0)))
    }

    fn surface_y(host: &WorldHost, x: i32, z: i32) -> i32 {
        let brickmap = host.read();
        (0..WORLD_SIZE_Y as i32)
            .rev()
            .find(|y| brickmap.is_occupied(x, *y, z))
            .expect("occupied column")
    }

    /// The two variants must produce the SAME world and the same deltas — they are
    /// one code path on two threads, and the A/B is only meaningful if that holds.
    /// NOTE: generates the full world — run with `--release`.
    #[test]
    fn inline_and_threaded_edits_agree() {
        let settings = WorldEditSettings::default();
        let mut inline_host = island_host();
        let mut threaded_host = island_host();
        threaded_host.set_world_thread(true);
        assert!(threaded_host.is_threaded() && !inline_host.is_threaded());

        let base_y = surface_y(&inline_host, 500, 500) / DETAIL_CELLS_PER_WORLD_VOXEL as i32;
        let edits: Vec<VoxelEdit> = (0..16)
            .map(|index| VoxelEdit {
                voxel: [60 + index % 4, base_y - index / 4, 62],
                material: Voxel::Air,
                light_grid: None,
                material_attributes: MaterialAttributes::compiled(),
            })
            .chain((0..8).map(|index| VoxelEdit {
                voxel: [25 + index, 31, 25],
                material: Voxel::Stone,
                light_grid: None,
                material_attributes: MaterialAttributes::compiled(),
            }))
            .collect();
        for edit in &edits {
            inline_host.request_edit(*edit, &settings);
            threaded_host.request_edit(*edit, &settings);
        }
        let inline_updates = inline_host.drain();
        // The frame thread never waits on the worker; the test does, because it
        // wants determinism rather than a frame budget.
        let mut threaded_updates = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while threaded_updates.len() < inline_updates.len() {
            threaded_updates.extend(threaded_host.drain());
            assert!(
                std::time::Instant::now() < deadline,
                "the world thread produced {} of {} updates",
                threaded_updates.len(),
                inline_updates.len()
            );
            std::thread::yield_now();
        }
        assert_eq!(inline_updates.len(), edits.len());
        assert_eq!(threaded_host.in_flight(), 0);

        for (inline, threaded) in inline_updates.iter().zip(&threaded_updates) {
            let (WorldUpdate::Delta(inline), WorldUpdate::Delta(threaded)) = (inline, threaded)
            else {
                panic!("only edit deltas were requested");
            };
            assert_eq!(inline.voxel, threaded.voxel);
            assert_eq!(inline.writes, threaded.writes);
            assert_eq!(inline.light_cells, threaded.light_cells);
            assert_eq!(inline.metadata, threaded.metadata);
        }
        let inline_world = inline_host.read();
        let threaded_world = threaded_host.read();
        assert_eq!(
            inline_world.metadata(),
            threaded_world.metadata(),
            "the two variants disagree about the world"
        );
        assert_eq!(
            inline_world.get(200, 248, 200),
            material_id(Voxel::Stone),
            "the placed voxel is missing"
        );
        assert_eq!(threaded_world.get(200, 248, 200), material_id(Voxel::Stone));
    }

    /// Switching the lever mid-session must not lose edits: whatever the worker
    /// already applied has to reach the GPU, and the world must be intact.
    /// NOTE: generates the full world — run with `--release`.
    #[test]
    fn stopping_the_world_thread_keeps_every_delta() {
        let settings = WorldEditSettings::default();
        let mut host = island_host();
        host.set_world_thread(true);
        let base_y = surface_y(&host, 496, 496) / DETAIL_CELLS_PER_WORLD_VOXEL as i32;
        for index in 0..24 {
            host.request_edit(
                VoxelEdit {
                    voxel: [58 + index % 6, base_y - index / 6, 62],
                    material: Voxel::Air,
                    light_grid: None,
                    material_attributes: MaterialAttributes::compiled(),
                },
                &settings,
            );
        }
        host.set_world_thread(false);
        assert!(!host.is_threaded());
        let updates = host.drain();
        assert_eq!(
            updates.len(),
            24,
            "stopping the thread dropped {} of 24 updates",
            24 - updates.len()
        );
        let brickmap = host.read();
        for index in 0..24 {
            let coordinate = WorldVoxelCoord::new(58 + index % 6, base_y - index / 6, 62);
            let detail = coordinate.detail_origin();
            assert!(!brickmap.is_occupied(detail[0], detail[1], detail[2]));
        }
        assert_eq!(host.stats().edits_applied, 24);
        assert!(host.stats().total_upload_bytes > 0);
    }

    /// The E4 resolution-switch rebuild, off-frame: the attributes must arrive
    /// through the drain and equal what an inline build produces.
    /// NOTE: generates the full world — run with `--release`.
    #[test]
    fn light_attributes_rebuild_off_frame() {
        let mut host = island_host();
        let grid = CagiGrid::for_world(8, host.read().metadata().max_occupied_brick_y);
        host.request_light_attributes(grid, MaterialAttributes::compiled());
        let inline = host.drain();
        let WorldUpdate::LightAttributes {
            attributes: inline_attributes,
            ..
        } = &inline[0]
        else {
            panic!("expected attributes");
        };

        host.set_world_thread(true);
        host.request_light_attributes(grid, MaterialAttributes::compiled());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let updates = host.drain();
            if let Some(WorldUpdate::LightAttributes {
                grid: arrived_grid,
                attributes,
                ..
            }) = updates.into_iter().next()
            {
                assert_eq!(arrived_grid, grid);
                assert_eq!(&attributes, inline_attributes);
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the world thread never delivered the attributes"
            );
            std::thread::yield_now();
        }
    }

    /// A repeated edit on the same voxel (hold-to-repeat) must be counted as
    /// ignored, not applied — the pipeline must stay idle when nothing changes.
    /// NOTE: generates the full world — run with `--release`.
    #[test]
    fn repeated_no_op_edits_are_ignored_inline() {
        let settings = WorldEditSettings::default();
        let mut host = island_host();
        let edit = VoxelEdit {
            voxel: [200, 250, 200],
            material: Voxel::Air,
            light_grid: None,
            material_attributes: MaterialAttributes::compiled(),
        };
        for _ in 0..5 {
            host.request_edit(edit, &settings);
        }
        assert!(host.drain().is_empty());
        assert_eq!(host.stats().edits_ignored, 5);
        assert_eq!(host.stats().edits_applied, 0);
    }
}
