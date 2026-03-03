use crate::spatial::directivity::directivity_gain;
use crate::spatial::listener::Listener;
use crate::spatial::panner::{distance_gain, stereo_pan};
use crate::spatial::source::SoundSource;

/// Distance model parameters for attenuation.
pub struct DistanceModel {
    pub ref_distance: f32,
    pub max_distance: f32,
    pub rolloff: f32,
}

impl Default for DistanceModel {
    fn default() -> Self {
        Self {
            ref_distance: 1.0,
            max_distance: 10.0,
            rolloff: 1.0,
        }
    }
}

/// Mix all active sources into an interleaved stereo output buffer.
///
/// For each sample frame:
///   1. Each source produces a mono sample
///   2. Attenuated by distance from listener (inverse distance model)
///   3. Panned to stereo based on source position vs listener
///   4. Summed and scaled by master_gain
///   5. Clamped to [-1.0, 1.0]
pub fn mix_sources(
    sources: &mut [Box<dyn SoundSource>],
    listener: &Listener,
    output: &mut [f32],
    channels: usize,
    sample_rate: f32,
    master_gain: f32,
    distance_model: &DistanceModel,
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
            let pos = source.position();
            let pan = stereo_pan(listener, pos);
            let dist = distance_gain(
                listener,
                pos,
                distance_model.ref_distance,
                distance_model.max_distance,
                distance_model.rolloff,
            );

            // Source directivity: how much energy this source emits toward the listener
            let src_dir = directivity_gain(
                pos,
                source.orientation(),
                listener.position,
                &source.directivity(),
            );

            // Listener hearing cone: how well the listener receives from this direction
            let hear = listener.hearing_gain(pos);

            left_acc += mono * pan.left * dist * src_dir * hear;
            right_acc += mono * pan.right * dist * src_dir * hear;
        }

        let base = frame_idx * channels;
        output[base] = (left_acc * master_gain).clamp(-1.0, 1.0);
        if channels > 1 {
            output[base + 1] = (right_acc * master_gain).clamp(-1.0, 1.0);
        }
    }
}
