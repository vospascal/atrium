//! GPU pass timing via wgpu timestamp queries — what the CPU clock cannot see.
//!
//! A CPU span around a `queue.submit` measures how long it took to *hand work
//! over*, not how long the GPU took to do it. Only the device can answer that,
//! and it answers by writing timestamps into a query set at pass boundaries.
//!
//! `N` is the number of measured spans, fixed at the type level so
//! [`GpuTimings`] stays `Copy` and can be handed to a readout without a clone.
//! The consumer supplies both `N` and a matching label list; this crate never
//! learns what the passes are.
//!
//! Requires [`wgpu::Features::TIMESTAMP_QUERY`]. [`GpuSpanTimers::new`] returns
//! `None` when the device lacks it, and every call site is expected to degrade to
//! "no readout" rather than refuse to render.
//!
//! **Readback never stalls the frame.** Each frame resolves the query set into a
//! small ring of `MAP_READ` buffers and maps one; a non-blocking `device.poll`
//! pumps the callbacks and finished results surface a few frames later, which is
//! fine for a perf readout. When every ring slot is still in flight the frame
//! skips readback and the displayed value goes one frame staler.
//!
//! One portability trap worth knowing before adding spans: **Metal zeroes
//! pass-boundary counters when a single command buffer holds more than one
//! compute pass.** Spans measuring separate compute passes need those passes in
//! separate command buffers, or the second one reads zero.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

/// In-flight readback ring size: enough for a couple of frames of latency plus
/// slack; overflow just skips a frame of readback.
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

/// Most recent completed GPU timings, milliseconds per span. Entries are `None`
/// until that span's first readback lands (or forever, when timestamps are
/// unsupported).
#[derive(Clone, Copy, Debug)]
pub struct GpuTimings<const N: usize> {
    pub span_milliseconds: [Option<f32>; N],
    /// Increments for every completed readback, even when two frames take the
    /// same number of milliseconds. A readout uses this to tell "the value did
    /// not change" from "no new value arrived".
    pub sample_sequence: u64,
}

impl<const N: usize> Default for GpuTimings<N> {
    fn default() -> Self {
        Self {
            span_milliseconds: [None; N],
            sample_sequence: 0,
        }
    }
}

impl<const N: usize> GpuTimings<N> {
    pub fn span_milliseconds(&self, span: usize) -> Option<f32> {
        self.span_milliseconds.get(span).copied().flatten()
    }

    /// Total GPU time across every measured span.
    ///
    /// `None` unless *every* span has landed: a partial sum is a smaller number
    /// than the truth, and a perf readout that silently under-reports is worse
    /// than one that admits it has no answer yet.
    ///
    /// This deliberately excludes swapchain acquisition and presentation, so it
    /// stays meaningful when a window system paces a no-vsync surface to the
    /// display refresh. Spans are expected to cover the submitted workload
    /// without overlapping — overlapping spans double-count here.
    pub fn total_milliseconds(&self) -> Option<f32> {
        let mut total = 0.0;
        for span in self.span_milliseconds {
            total += span?;
        }
        Some(total)
    }

    /// The GPU-only frame-rate ceiling implied by [`Self::total_milliseconds`].
    /// This is not a present rate: a 120 Hz display still presents at most 120
    /// complete frames a second, however fast the GPU finishes.
    pub fn frames_per_second(&self) -> Option<f32> {
        self.total_milliseconds()
            .filter(|milliseconds| *milliseconds > 0.0)
            .map(|milliseconds| 1_000.0 / milliseconds)
    }
}

pub struct GpuSpanTimers<const N: usize> {
    labels: &'static [&'static str],
    query_set: wgpu::QuerySet,
    resolve_buffer: wgpu::Buffer,
    readback_slots: Vec<ReadbackSlot>,
    timestamp_period_nanoseconds: f32,
    latest: GpuTimings<N>,
}

impl<const N: usize> GpuSpanTimers<N> {
    /// Two timestamps per span.
    const TIMESTAMP_COUNT: u32 = (N * 2) as u32;

    /// `None` when the device was created without
    /// [`wgpu::Features::TIMESTAMP_QUERY`] (e.g. the adapter cannot do it).
    ///
    /// `labels` must have `N` entries; it is only used for reporting.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        labels: &'static [&'static str],
    ) -> Option<Self> {
        debug_assert_eq!(
            labels.len(),
            N,
            "GpuSpanTimers<{N}> needs exactly {N} labels"
        );
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            return None;
        }

        let query_set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("profile: frame timing query set"),
            ty: wgpu::QueryType::Timestamp,
            count: Self::TIMESTAMP_COUNT,
        });
        // Sized up to the resolve alignment: `resolve_query_set` writes on that
        // granularity, so a buffer merely big enough for N*8 bytes can still be
        // too small to be a legal resolve target.
        let buffer_size = Self::resolve_buffer_size();
        let resolve_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("profile: frame timing resolve buffer"),
            size: buffer_size,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback_slots = (0..READBACK_SLOT_COUNT)
            .map(|slot_index| ReadbackSlot {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(&format!("profile: frame timing readback {slot_index}")),
                    size: buffer_size,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                state: Arc::new(AtomicU8::new(SLOT_FREE)),
            })
            .collect();

        Some(Self {
            labels,
            query_set,
            resolve_buffer,
            readback_slots,
            timestamp_period_nanoseconds: queue.get_timestamp_period(),
            latest: GpuTimings::default(),
        })
    }

    /// Bytes needed for the resolve and readback buffers: the timestamp payload
    /// rounded up to the query-resolve alignment.
    fn resolve_buffer_size() -> u64 {
        let payload = u64::from(Self::TIMESTAMP_COUNT) * wgpu::QUERY_SIZE as u64;
        let alignment = wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT;
        payload.max(1).div_ceil(alignment) * alignment
    }

    pub fn labels(&self) -> &'static [&'static str] {
        self.labels
    }

    pub fn label(&self, span: usize) -> &'static str {
        self.labels.get(span).copied().unwrap_or("<unknown span>")
    }

    /// Timestamp writes bracketing a whole single-pass span (compute flavour).
    pub fn compute_span_writes(&self, span: usize) -> wgpu::ComputePassTimestampWrites<'_> {
        wgpu::ComputePassTimestampWrites {
            query_set: &self.query_set,
            beginning_of_pass_write_index: Some(Self::begin_index(span)),
            end_of_pass_write_index: Some(Self::end_index(span)),
        }
    }

    /// Timestamp writes bracketing a whole single-pass span (render flavour).
    pub fn render_span_writes(&self, span: usize) -> wgpu::RenderPassTimestampWrites<'_> {
        wgpu::RenderPassTimestampWrites {
            query_set: &self.query_set,
            beginning_of_pass_write_index: Some(Self::begin_index(span)),
            end_of_pass_write_index: Some(Self::end_index(span)),
        }
    }

    /// Begin-only timestamp write: opens a span on the first of several render
    /// passes (the end is written by [`Self::render_span_end_writes`] on the
    /// last one).
    pub fn render_span_begin_writes(&self, span: usize) -> wgpu::RenderPassTimestampWrites<'_> {
        wgpu::RenderPassTimestampWrites {
            query_set: &self.query_set,
            beginning_of_pass_write_index: Some(Self::begin_index(span)),
            end_of_pass_write_index: None,
        }
    }

    /// End-only timestamp write: closes a span opened by
    /// [`Self::render_span_begin_writes`].
    pub fn render_span_end_writes(&self, span: usize) -> wgpu::RenderPassTimestampWrites<'_> {
        wgpu::RenderPassTimestampWrites {
            query_set: &self.query_set,
            beginning_of_pass_write_index: None,
            end_of_pass_write_index: Some(Self::end_index(span)),
        }
    }

    fn begin_index(span: usize) -> u32 {
        (span * 2) as u32
    }

    fn end_index(span: usize) -> u32 {
        (span * 2 + 1) as u32
    }

    /// Record this frame's query resolve + copy into a free readback slot. Call
    /// after all timestamped passes are encoded, before submit. Returns the slot
    /// to map in [`Self::after_submit`] (`None` = ring full, skip this frame).
    pub fn encode_resolve(&self, encoder: &mut wgpu::CommandEncoder) -> Option<usize> {
        let slot_index = self
            .readback_slots
            .iter()
            .position(|slot| slot.state.load(Ordering::Acquire) == SLOT_FREE)?;
        encoder.resolve_query_set(
            &self.query_set,
            0..Self::TIMESTAMP_COUNT,
            &self.resolve_buffer,
            0,
        );
        encoder.copy_buffer_to_buffer(
            &self.resolve_buffer,
            0,
            &self.readback_slots[slot_index].buffer,
            0,
            u64::from(Self::TIMESTAMP_COUNT) * wgpu::QUERY_SIZE as u64,
        );
        self.readback_slots[slot_index]
            .state
            .store(SLOT_IN_FLIGHT, Ordering::Release);
        Some(slot_index)
    }

    /// Kick off the asynchronous map of the slot filled by
    /// [`Self::encode_resolve`]. Call right after `queue.submit`.
    pub fn after_submit(&self, slot_index: usize) {
        let Some(slot) = self.readback_slots.get(slot_index) else {
            debug_assert!(false, "slot index {slot_index} out of range");
            return;
        };
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

    /// Pump map callbacks (non-blocking) and fold any finished readbacks into
    /// the latest timings. Call once per frame; returns the freshest values
    /// (2-3 frames old, which is fine for a perf readout).
    pub fn collect(&mut self, device: &wgpu::Device) -> GpuTimings<N> {
        let _ = device.poll(wgpu::PollType::Poll);

        for slot in &self.readback_slots {
            if slot.state.load(Ordering::Acquire) != SLOT_MAPPED {
                continue;
            }
            {
                let mapped = slot.buffer.slice(..).get_mapped_range();
                let payload = Self::TIMESTAMP_COUNT as usize * 8;
                // Byte-wise decode instead of a slice cast: mapped memory has no
                // guaranteed 8-byte alignment for a u64 reinterpret.
                for span in 0..N {
                    let begin = read_timestamp(&mapped[..payload], span * 2);
                    let end = read_timestamp(&mapped[..payload], span * 2 + 1);
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
            self.latest.sample_sequence = self.latest.sample_sequence.wrapping_add(1);
        }
        self.latest
    }

    /// The most recent timings without pumping readback.
    pub fn latest(&self) -> GpuTimings<N> {
        self.latest
    }
}

fn read_timestamp(payload: &[u8], index: usize) -> u64 {
    let start = index * 8;
    let bytes: [u8; 8] = payload[start..start + 8]
        .try_into()
        .expect("payload is a whole number of 8-byte timestamps");
    u64::from_le_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPANS: &[&str] = &["dda", "cagi", "post"];

    #[test]
    fn a_total_needs_every_span_and_never_reports_a_partial_frame() {
        let complete = GpuTimings::<3> {
            span_milliseconds: [Some(2.0), Some(1.0), Some(2.0)],
            sample_sequence: 1,
        };
        assert_eq!(complete.total_milliseconds(), Some(5.0));
        assert_eq!(complete.frames_per_second(), Some(200.0));

        let incomplete = GpuTimings::<3> {
            span_milliseconds: [Some(2.0), None, Some(2.0)],
            sample_sequence: 1,
        };
        assert_eq!(
            incomplete.total_milliseconds(),
            None,
            "a partial sum under-reports; None is the honest answer"
        );
        assert_eq!(incomplete.frames_per_second(), None);
    }

    #[test]
    fn a_fresh_timings_value_has_no_measurements() {
        let timings = GpuTimings::<3>::default();
        assert_eq!(timings.span_milliseconds(0), None);
        assert_eq!(timings.span_milliseconds(99), None);
        assert_eq!(timings.total_milliseconds(), None);
    }

    /// The resolve buffer must satisfy the alignment even for a span count whose
    /// payload is far smaller than it, and must grow past it for large counts.
    #[test]
    fn the_resolve_buffer_is_always_alignment_sized() {
        let alignment = wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT;
        assert_eq!(GpuSpanTimers::<1>::resolve_buffer_size(), alignment);
        assert_eq!(GpuSpanTimers::<3>::resolve_buffer_size(), alignment);

        // 32 spans = 64 timestamps = 512 bytes, past a 256-byte alignment.
        let large = GpuSpanTimers::<32>::resolve_buffer_size();
        assert!(large >= 512, "must hold the payload, got {large}");
        assert_eq!(large % alignment, 0, "must stay aligned");
    }

    #[test]
    fn span_indices_do_not_overlap_between_neighbouring_spans() {
        assert_eq!(GpuSpanTimers::<3>::begin_index(0), 0);
        assert_eq!(GpuSpanTimers::<3>::end_index(0), 1);
        assert_eq!(GpuSpanTimers::<3>::begin_index(1), 2);
        assert_eq!(GpuSpanTimers::<3>::end_index(2), 5);
        assert_eq!(GpuSpanTimers::<3>::TIMESTAMP_COUNT, 6);
    }

    #[test]
    fn timestamps_decode_little_endian_at_the_right_offset() {
        let mut payload = vec![0_u8; 16];
        payload[8..16].copy_from_slice(&1_234_567_u64.to_le_bytes());
        assert_eq!(read_timestamp(&payload, 0), 0);
        assert_eq!(read_timestamp(&payload, 1), 1_234_567);
    }

    /// Graceful degradation: a device created WITHOUT the timestamp feature must
    /// yield `None`, never panic. Skips when no GPU adapter exists.
    #[test]
    fn timers_degrade_without_the_timestamp_feature() {
        let instance = wgpu::Instance::default();
        let adapter = match pollster::block_on(
            instance.request_adapter(&wgpu::RequestAdapterOptions::default()),
        ) {
            Ok(adapter) => adapter,
            Err(error) => {
                eprintln!(
                    "skipping timers_degrade_without_the_timestamp_feature: no adapter ({error})"
                );
                return;
            }
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("profile test device (no features)"),
            ..Default::default()
        }))
        .expect("adapter exists but device creation failed");

        assert!(
            GpuSpanTimers::<3>::new(&device, &queue, SPANS).is_none(),
            "timers must be None on a device without TIMESTAMP_QUERY"
        );
    }

    /// On hardware that supports timestamps, construction with the feature
    /// enabled must succeed. Skips when the adapter lacks the feature.
    #[test]
    fn timers_construct_with_the_timestamp_feature() {
        let instance = wgpu::Instance::default();
        let adapter = match pollster::block_on(
            instance.request_adapter(&wgpu::RequestAdapterOptions::default()),
        ) {
            Ok(adapter) => adapter,
            Err(error) => {
                eprintln!(
                    "skipping timers_construct_with_the_timestamp_feature: no adapter ({error})"
                );
                return;
            }
        };
        if !adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            eprintln!("skipping timers_construct_with_the_timestamp_feature: unsupported");
            return;
        }
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("profile test device (timestamps)"),
            required_features: wgpu::Features::TIMESTAMP_QUERY,
            ..Default::default()
        }))
        .expect("adapter reports TIMESTAMP_QUERY but device creation failed");

        let timers = GpuSpanTimers::<3>::new(&device, &queue, SPANS);
        assert!(timers.is_some());
        let timers = timers.expect("checked");
        assert_eq!(timers.label(0), "dda");
        assert_eq!(timers.label(99), "<unknown span>");
    }
}
