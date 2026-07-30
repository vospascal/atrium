//! GPU pass timing via wgpu timestamp queries — the permanent perf readout
//! for every render stage (the frame counter alone cannot tell which pass
//! regressed). Pure wgpu, no winit/egui knowledge (plan architecture rule).
//!
//! Two measured spans per frame:
//!
//! - [`SPAN_DDA`] — the DDA compute pass (begin + end written by the pass
//!   itself via `timestamp_writes`).
//! - [`SPAN_POST`] — blit begin through overlay end (the span opens on the
//!   blit render pass and closes on the egui overlay pass).
//!
//! Requires [`wgpu::Features::TIMESTAMP_QUERY`]; [`GpuFrameTimers::new`]
//! returns `None` when the device lacks it and every call site degrades to
//! "no readout" (the overlay prints that instead of numbers).
//!
//! Readback is asynchronous and never stalls the frame: each frame resolves
//! the query set into a small ring of MAP_READ buffers and maps one; a
//! non-blocking `device.poll` pumps the map callbacks and finished results
//! surface a few frames later, which is fine for a perf readout. When every
//! ring slot is still in flight the frame simply skips readback and the
//! displayed value goes one frame staler.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

/// Span index: the DDA compute pass.
pub const SPAN_DDA: usize = 0;
/// Span index: blit + egui overlay (everything after the compute pass).
pub const SPAN_POST: usize = 1;
/// Number of measured spans (two timestamps each).
pub const SPAN_COUNT: usize = 2;

const TIMESTAMP_COUNT: u32 = (SPAN_COUNT * 2) as u32;
/// In-flight readback ring size: enough for a couple of frames of latency
/// plus slack; overflow just skips a frame of readback.
const READBACK_SLOT_COUNT: usize = 4;

const SLOT_FREE: u8 = 0;
const SLOT_IN_FLIGHT: u8 = 1;
const SLOT_MAPPED: u8 = 2;

struct ReadbackSlot {
    buffer: wgpu::Buffer,
    /// SLOT_FREE -> SLOT_IN_FLIGHT (copy submitted, map requested) ->
    /// SLOT_MAPPED (map callback fired) -> SLOT_FREE (values collected).
    state: Arc<AtomicU8>,
}

/// Most recent completed GPU timings, milliseconds per span. `None` until
/// the first readback lands (or forever, when timestamps are unsupported).
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameTimings {
    pub span_milliseconds: [Option<f32>; SPAN_COUNT],
}

impl FrameTimings {
    pub fn dda_milliseconds(&self) -> Option<f32> {
        self.span_milliseconds[SPAN_DDA]
    }

    pub fn post_milliseconds(&self) -> Option<f32> {
        self.span_milliseconds[SPAN_POST]
    }
}

pub struct GpuFrameTimers {
    query_set: wgpu::QuerySet,
    resolve_buffer: wgpu::Buffer,
    readback_slots: Vec<ReadbackSlot>,
    timestamp_period_nanoseconds: f32,
    latest: FrameTimings,
}

impl GpuFrameTimers {
    /// `None` when the device was created without
    /// [`wgpu::Features::TIMESTAMP_QUERY`] (e.g. the adapter cannot do it).
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Self> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }

        let query_set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("frame timing query set"),
            ty: wgpu::QueryType::Timestamp,
            count: TIMESTAMP_COUNT,
        });
        // One resolve target reused every frame; QUERY_RESOLVE_BUFFER_ALIGNMENT
        // comfortably covers the 4 x 8-byte timestamps.
        let resolve_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame timing resolve buffer"),
            size: wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback_slots = (0..READBACK_SLOT_COUNT)
            .map(|slot_index| ReadbackSlot {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(&format!("frame timing readback buffer {slot_index}")),
                    size: wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                state: Arc::new(AtomicU8::new(SLOT_FREE)),
            })
            .collect();

        Some(Self {
            query_set,
            resolve_buffer,
            readback_slots,
            timestamp_period_nanoseconds: queue.get_timestamp_period(),
            latest: FrameTimings::default(),
        })
    }

    /// Timestamp writes bracketing a whole single-pass span (compute flavor).
    pub fn compute_span_writes(&self, span: usize) -> wgpu::ComputePassTimestampWrites<'_> {
        wgpu::ComputePassTimestampWrites {
            query_set: &self.query_set,
            beginning_of_pass_write_index: Some((span * 2) as u32),
            end_of_pass_write_index: Some((span * 2 + 1) as u32),
        }
    }

    /// Begin-only timestamp write: opens a span on the first of several
    /// render passes (the end is written by [`Self::render_span_end_writes`]
    /// on the last one).
    pub fn render_span_begin_writes(&self, span: usize) -> wgpu::RenderPassTimestampWrites<'_> {
        wgpu::RenderPassTimestampWrites {
            query_set: &self.query_set,
            beginning_of_pass_write_index: Some((span * 2) as u32),
            end_of_pass_write_index: None,
        }
    }

    /// End-only timestamp write: closes a span opened by
    /// [`Self::render_span_begin_writes`].
    pub fn render_span_end_writes(&self, span: usize) -> wgpu::RenderPassTimestampWrites<'_> {
        wgpu::RenderPassTimestampWrites {
            query_set: &self.query_set,
            beginning_of_pass_write_index: None,
            end_of_pass_write_index: Some((span * 2 + 1) as u32),
        }
    }

    /// Record this frame's query resolve + copy into a free readback slot.
    /// Call after all timestamped passes are encoded, before submit. Returns
    /// the slot to map in [`Self::after_submit`] (`None` = ring full, skip).
    pub fn encode_resolve(&self, encoder: &mut wgpu::CommandEncoder) -> Option<usize> {
        let slot_index = self
            .readback_slots
            .iter()
            .position(|slot| slot.state.load(Ordering::Acquire) == SLOT_FREE)?;
        encoder.resolve_query_set(&self.query_set, 0..TIMESTAMP_COUNT, &self.resolve_buffer, 0);
        encoder.copy_buffer_to_buffer(
            &self.resolve_buffer,
            0,
            &self.readback_slots[slot_index].buffer,
            0,
            u64::from(TIMESTAMP_COUNT) * wgpu::QUERY_SIZE as u64,
        );
        self.readback_slots[slot_index]
            .state
            .store(SLOT_IN_FLIGHT, Ordering::Release);
        Some(slot_index)
    }

    /// Kick off the asynchronous map of the slot filled by
    /// [`Self::encode_resolve`]. Call right after `queue.submit`.
    pub fn after_submit(&self, slot_index: usize) {
        let slot = &self.readback_slots[slot_index];
        let state = Arc::clone(&slot.state);
        slot.buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |map_result| {
                if map_result.is_ok() {
                    state.store(SLOT_MAPPED, Ordering::Release);
                } else {
                    // Device loss etc. — release the slot so the ring keeps
                    // cycling; the readout just stays stale.
                    state.store(SLOT_FREE, Ordering::Release);
                }
            });
    }

    /// Pump map callbacks (non-blocking) and fold any finished readbacks
    /// into the latest timings. Call once per frame; returns the freshest
    /// values (2-3 frames old, which is fine for a perf readout).
    pub fn collect(&mut self, device: &wgpu::Device) -> FrameTimings {
        let _ = device.poll(wgpu::PollType::Poll);

        for slot in &self.readback_slots {
            if slot.state.load(Ordering::Acquire) != SLOT_MAPPED {
                continue;
            }
            {
                let mapped = slot.buffer.slice(..).get_mapped_range();
                // Byte-wise decode instead of a slice cast: mapped memory has
                // no guaranteed 8-byte alignment for a u64 reinterpret.
                let mut timestamps = [0_u64; TIMESTAMP_COUNT as usize];
                for (index, chunk) in mapped[..TIMESTAMP_COUNT as usize * 8]
                    .chunks_exact(8)
                    .enumerate()
                {
                    timestamps[index] =
                        u64::from_le_bytes(chunk.try_into().expect("chunks_exact(8)"));
                }
                for span in 0..SPAN_COUNT {
                    let begin = timestamps[span * 2];
                    let end = timestamps[span * 2 + 1];
                    // Guard against unwritten (zero) or out-of-order values.
                    if end > begin && begin != 0 {
                        let elapsed_nanoseconds =
                            (end - begin) as f32 * self.timestamp_period_nanoseconds;
                        self.latest.span_milliseconds[span] =
                            Some(elapsed_nanoseconds / 1_000_000.0);
                    }
                }
            }
            slot.buffer.unmap();
            slot.state.store(SLOT_FREE, Ordering::Release);
        }
        self.latest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Graceful degradation: a device created WITHOUT the timestamp feature
    /// must yield `None`, never panic. Skips when no GPU adapter exists.
    #[test]
    fn timers_degrade_without_timestamp_feature() {
        let instance = wgpu::Instance::default();
        let adapter = match pollster::block_on(
            instance.request_adapter(&wgpu::RequestAdapterOptions::default()),
        ) {
            Ok(adapter) => adapter,
            Err(error) => {
                eprintln!(
                    "skipping timers_degrade_without_timestamp_feature: no adapter ({error})"
                );
                return;
            }
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("frame timing test device (no features)"),
            ..Default::default()
        }))
        .expect("adapter exists but device creation failed");

        assert!(
            GpuFrameTimers::new(&device, &queue).is_none(),
            "timers must be None on a device without TIMESTAMP_QUERY"
        );
    }

    /// On hardware that supports timestamps, construction with the feature
    /// enabled must succeed. Skips when the adapter lacks the feature.
    #[test]
    fn timers_construct_with_timestamp_feature() {
        let instance = wgpu::Instance::default();
        let adapter = match pollster::block_on(
            instance.request_adapter(&wgpu::RequestAdapterOptions::default()),
        ) {
            Ok(adapter) => adapter,
            Err(error) => {
                eprintln!("skipping timers_construct_with_timestamp_feature: no adapter ({error})");
                return;
            }
        };
        if !adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            eprintln!("skipping timers_construct_with_timestamp_feature: unsupported");
            return;
        }
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("frame timing test device (timestamps)"),
            required_features: wgpu::Features::TIMESTAMP_QUERY,
            ..Default::default()
        }))
        .expect("adapter reports TIMESTAMP_QUERY but device creation failed");

        let timers = GpuFrameTimers::new(&device, &queue);
        assert!(timers.is_some());
    }
}
