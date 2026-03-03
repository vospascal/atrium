use crate::spatial::listener::Listener;
use crate::spatial::panner::stereo_pan;
use crate::spatial::source::SoundSource;

/// Mix all active sources into an interleaved stereo output buffer.
///
/// For each sample frame:
///   1. Each source produces a mono sample
///   2. Panned to stereo based on source position vs listener
///   3. Summed and scaled by master_gain
///   4. Clamped to [-1.0, 1.0]
pub fn mix_sources(
    sources: &mut [Box<dyn SoundSource>],
    listener: &Listener,
    output: &mut [f32],
    channels: usize,
    sample_rate: f32,
    master_gain: f32,
) {
    let num_frames = output.len() / channels;

    for frame_idx in 0..num_frames {
        let mut left_acc: f32 = 0.0;
        let mut right_acc: f32 = 0.0;

        for source in sources.iter_mut() {
            if !source.is_active() {
                continue;
            }

            let mono = source.next_sample(sample_rate);
            let gains = stereo_pan(listener, source.position());

            left_acc += mono * gains.left;
            right_acc += mono * gains.right;
        }

        let base = frame_idx * channels;
        output[base] = (left_acc * master_gain).clamp(-1.0, 1.0);
        if channels > 1 {
            output[base + 1] = (right_acc * master_gain).clamp(-1.0, 1.0);
        }
    }
}
