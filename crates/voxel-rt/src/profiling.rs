//! The renderer's span registry: the single place that names what gets measured.
//!
//! [`atrium_profile`] owns the measuring machinery and knows nothing about this
//! renderer; this module supplies the names and the indices. Both lists follow
//! the same rule as the lever registry in [`crate::variants`] — **declarations
//! stay compiled**. The label arrays and the index constants are consts checked
//! against each other by the tests at the bottom, so a span cannot be added to
//! one and forgotten in the other.
//!
//! Two families, because they answer different questions:
//!
//! - [`CPU_SPANS`] — wall-clock cost of the frame's CPU phases, measured on the
//!   thread doing the work. Their sum is what the frame loop spends *not*
//!   waiting on the GPU.
//! - [`GPU_SPANS`] — device-side pass cost via timestamp queries. A CPU span
//!   around a submit measures handing work over, never the work itself.
//!
//! The gap between the two is the diagnosis. If the CPU spans sum to 6 ms, the
//! GPU spans sum to 5 ms, and frames land 16.7 ms apart, the missing time is in
//! [`CPU_ACQUIRE`] or [`CPU_PRESENT`] — swapchain backpressure — and those are
//! measured precisely so that question is answerable rather than guessable.

use atrium_profile::gpu::{GpuSpanTimers, GpuTimings};

/// CPU phases of a frame, in the order [`crate::main`]'s redraw performs them.
/// Order matters only for the readout; the indices below are the contract.
pub const CPU_SPANS: &[&str] = &[
    "input + move",
    "edits + upload",
    "uniforms",
    "acquire",
    "encode",
    "overlay (egui)",
    "submit",
    "present",
];

/// Camera input drain plus whichever movement model is active (the walk-mode
/// body sweep reads the world lock, so this covers that contention too).
pub const CPU_INPUT: usize = 0;
/// E2 — held edits applied and their deltas uploaded. A prime suspect for
/// periodic hitching: uploads burst.
pub const CPU_EDITS: usize = 1;
/// Per-frame uniform construction: camera, lighting, animation, and the display
/// headroom probe (which talks to the window system every frame on purpose).
pub const CPU_UNIFORMS: usize = 2;
/// `surface.get_current_texture`. **The swapchain backpressure span.** Under
/// FIFO this is where the CPU blocks when the presentation queue is full, so a
/// wave that lives here is vsync pacing rather than renderer cost.
pub const CPU_ACQUIRE: usize = 3;
/// Command encoding for the light volume and the frame — CPU-side recording
/// only, not the GPU work it describes.
pub const CPU_ENCODE: usize = 4;
/// Building the egui overlay. Separated because UI layout cost is easy to grow
/// accidentally and looks exactly like renderer cost from the frame time alone.
pub const CPU_OVERLAY: usize = 5;
/// `queue.submit` for both command buffers.
pub const CPU_SUBMIT: usize = 6;
/// `pre_present_notify` + `present`. **The other backpressure span**: a driver
/// may block here instead of in acquire, and which one it picks is a platform
/// detail we should observe rather than assume.
pub const CPU_PRESENT: usize = 7;

/// GPU passes measured by timestamp queries.
pub const GPU_SPANS: &[&str] = &["DDA", "CAGI", "blit", "overlay"];

/// The DDA compute pass.
pub(crate) const GPU_DDA: usize = 0;
/// The E4 CAGI compute pass — all of this frame's CA iterations, which share one
/// pass. That pass is submitted in its OWN command buffer because Metal zeroes
/// pass-boundary counters once a command buffer holds more than one compute
/// pass. Always opened, even at zero iterations, so the readout shows ~0.00 ms
/// rather than going stale when the lever is off.
pub(crate) const GPU_CAGI: usize = 1;
/// The blit render pass alone.
///
/// **Split out from a single blit-through-overlay span, because that span was
/// lying.** It opened on the blit pass and closed on the egui pass, so the GPU's
/// wait BETWEEN the two — including waiting for the swapchain texture to become
/// writable — landed inside it. It read 3.7-6.4 ms while egui's entire CPU side
/// cost 0.43 ms, and made [`GpuTimings::total_milliseconds`] report 12.45 ms of
/// "GPU work" inside a 7.23 ms frame. Two self-contained spans exclude the
/// inter-pass gap from both, which is what makes the total trustworthy again.
pub(crate) const GPU_BLIT: usize = 2;
/// The egui overlay render pass alone. See [`GPU_BLIT`] for why these are two
/// spans and not one.
pub const GPU_OVERLAY: usize = 3;

/// Memory gauges: bytes each subsystem holds. Split CPU/GPU because they are
/// different budgets — the GPU rows are what a Quest's tighter memory has to fit,
/// while the CPU brickmap is what the world thread and the audio rays read.
pub const MEMORY_CATEGORIES: &[&str] = &[
    "world (CPU brickmap)",
    "world (GPU buffers)",
    "light volume (GPU)",
    "storage texture (GPU)",
];

/// The authoritative CPU-side brickmap: pointer grid, occupancy and material
/// words, clearance field, directional bounds.
pub const MEMORY_WORLD_CPU: usize = 0;
/// The world's GPU buffers. Larger than the CPU figure by design — it includes the
/// brick headroom that lets an edit avoid reallocating the whole world.
pub const MEMORY_WORLD_GPU: usize = 1;
/// CAGI's two ping-pong volume buffers plus the cell attribute data. The single
/// largest allocation in the renderer at typical settings.
pub const MEMORY_LIGHT_VOLUME: usize = 2;
/// The ray-traced storage texture. Scales with render scale AND output depth, so
/// it is the row that responds to the quality levers.
pub const MEMORY_STORAGE_TEXTURE: usize = 3;

/// The renderer's GPU timer set, sized to [`GPU_SPANS`].
pub type FrameTimers = GpuSpanTimers<{ GPU_SPANS.len() }>;
/// One frame's completed GPU pass timings.
pub type FrameTimings = GpuTimings<{ GPU_SPANS.len() }>;

#[cfg(test)]
mod tests {
    use super::*;

    /// Completeness: every index constant must name a real slot, and the two
    /// label lists must be exactly as long as the constants claim. This is the
    /// test that makes the registry load-bearing rather than decorative.
    #[test]
    fn every_span_index_is_in_range_and_distinct() {
        let cpu_indices = [
            CPU_INPUT,
            CPU_EDITS,
            CPU_UNIFORMS,
            CPU_ACQUIRE,
            CPU_ENCODE,
            CPU_OVERLAY,
            CPU_SUBMIT,
            CPU_PRESENT,
        ];
        assert_eq!(
            cpu_indices.len(),
            CPU_SPANS.len(),
            "a CPU span was added to one of the list/constants and not the other"
        );
        for (expected, index) in cpu_indices.iter().enumerate() {
            assert_eq!(
                expected, *index,
                "CPU span constants must be dense and in order"
            );
        }

        let gpu_indices = [GPU_DDA, GPU_CAGI, GPU_BLIT, GPU_OVERLAY];
        assert_eq!(
            gpu_indices.len(),
            GPU_SPANS.len(),
            "a GPU span was added to one of the list/constants and not the other"
        );
        for (expected, index) in gpu_indices.iter().enumerate() {
            assert_eq!(expected, *index);
        }
    }

    /// Completeness for the memory gauges, same contract as the spans.
    #[test]
    fn every_memory_category_index_is_dense_and_in_order() {
        let indices = [
            MEMORY_WORLD_CPU,
            MEMORY_WORLD_GPU,
            MEMORY_LIGHT_VOLUME,
            MEMORY_STORAGE_TEXTURE,
        ];
        assert_eq!(
            indices.len(),
            MEMORY_CATEGORIES.len(),
            "a memory category was added to one of the list/constants and not the other"
        );
        for (expected, index) in indices.iter().enumerate() {
            assert_eq!(expected, *index);
        }
    }

    #[test]
    fn every_span_label_is_unique_and_non_empty() {
        for labels in [CPU_SPANS, GPU_SPANS, MEMORY_CATEGORIES] {
            for (position, label) in labels.iter().enumerate() {
                assert!(!label.is_empty(), "span {position} has no label");
                assert_eq!(
                    labels.iter().filter(|other| *other == label).count(),
                    1,
                    "duplicate span label {label:?} — the readout would be ambiguous"
                );
            }
        }
    }

    /// The timer type must be sized from the label list, so adding a GPU span
    /// cannot leave the query set one pass short.
    #[test]
    fn the_timer_type_is_sized_from_the_label_list() {
        let timings = FrameTimings::default();
        assert_eq!(timings.span_milliseconds.len(), GPU_SPANS.len());
    }
}
