//! `PlayInStep`: loops brought onto one playhead.
//!
//! A loop starts at its beginning when it is built, so one added later, or
//! copied, plays out of step with the loops already playing. Only the audio
//! thread knows where a loop is to the sample, so it is moved here.

use atrium_core::telemetry::MAX_SOURCES;

use super::AudioScene;

impl AudioScene {
    /// Move every voice in `slots` (bit `n` is voice `n`) that has a playhead
    /// to the `anchor`'s, in this one block, so they are in step to the
    /// sample. The anchor is never moved; nothing moves if it has no
    /// playhead.
    pub(super) fn play_in_step(&mut self, anchor: u16, slots: u16) {
        let anchor = anchor as usize;
        let Some(seconds) = self
            .sources
            .get(anchor)
            .and_then(|source| source.playhead_seconds())
        else {
            return;
        };
        for (slot, source) in self.sources.iter_mut().enumerate().take(MAX_SOURCES) {
            if slot != anchor && slots & (1 << slot) != 0 {
                source.set_playhead_seconds(seconds);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use atrium_core::commands::Command;
    use atrium_core::source::SoundSource;
    use atrium_core::types::Vec3;

    use crate::audio::decode::AudioBuffer;
    use crate::audio::spectral_profile::SpectralProfile;
    use crate::audio::test_node::TestNode;
    use crate::config::SceneConfig;
    use crate::engine::scene::AudioScene;

    const SAMPLE_RATE: f32 = 48_000.0;
    const BLOCK: usize = 512;
    /// 5 ms at 48 kHz: how long a joining loop dips out, and back in.
    const FADE: usize = 240;
    /// What `TestNode::new` plays a recording at.
    const AMPLITUDE: f32 = 0.5;

    /// An empty room: every voice a silent placeholder, so only the loops a
    /// test puts in are heard.
    const EMPTY_SCENE: &str = "\
environment: environments/small_atrium.yaml
atmosphere: atmospheres/default.yaml
listener:
  position: [0.0, 0.0, 1.2]
  yaw_degrees: 90
speakers:
  layout: \"stereo\"
  render_mode: dbap
sounds: {}
";

    /// A loop that writes down every sample it plays, so a test can read
    /// what the scene drew from it.
    struct Heard {
        node: TestNode,
        samples: Arc<Mutex<Vec<f32>>>,
    }

    impl SoundSource for Heard {
        fn next_sample(&mut self, sample_rate: f32) -> f32 {
            let sample = self.node.next_sample(sample_rate);
            self.samples.lock().expect("unpoisoned").push(sample);
            sample
        }
        fn position(&self) -> Vec3 {
            self.node.position()
        }
        fn tick(&mut self, dt: f32) {
            self.node.tick(dt);
        }
        fn playhead_seconds(&self) -> Option<f64> {
            self.node.playhead_seconds()
        }
        fn set_playhead_seconds(&mut self, seconds: f64) {
            self.node.set_playhead_seconds(seconds);
        }
    }

    /// A recording of `length` samples at 48 kHz with no two samples alike
    /// nearby: a different playhead plays a different signal.
    fn recording(length: usize, seed: u32) -> Arc<AudioBuffer> {
        let mut state = seed;
        let samples = (0..length)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 8) as f32 / (1u32 << 24) as f32 - 0.5
            })
            .collect();
        Arc::new(AudioBuffer {
            samples,
            sample_rate: SAMPLE_RATE as u32,
            rms: 0.0,
            spectral_profile: SpectralProfile::default(),
        })
    }

    fn empty_scene() -> AudioScene {
        let config: SceneConfig = serde_yaml::from_str(EMPTY_SCENE).expect("parses");
        let mut scene = config.build().expect("builds").scene;
        scene.sample_rate = SAMPLE_RATE;
        scene.init_pipelines();
        scene
    }

    /// Put a loop of `buffer` in `slot`, a metre ahead of the listener, and
    /// return what it will play.
    fn add_loop(
        scene: &mut AudioScene,
        slot: usize,
        buffer: &Arc<AudioBuffer>,
    ) -> Arc<Mutex<Vec<f32>>> {
        let samples = Arc::new(Mutex::new(Vec::new()));
        let ahead = scene.listener.position + Vec3::new(0.0, 1.0, 0.0);
        scene.sources[slot] = Box::new(Heard {
            node: TestNode::new(buffer.clone(), ahead, 0.0, 0.0),
            samples: samples.clone(),
        });
        samples
    }

    fn render_blocks(scene: &mut AudioScene, blocks: usize) {
        let channels = scene.speaker_layout.total_channels();
        let mut output = vec![0.0; BLOCK * channels];
        for _ in 0..blocks {
            scene.render(&mut output, channels);
        }
    }

    fn play_in_step(scene: &mut AudioScene, anchor: u16, slots: u16) {
        let (mut producer, mut consumer) = rtrb::RingBuffer::new(1);
        producer
            .push(Command::PlayInStep { anchor, slots })
            .expect("room");
        scene.process_commands(&mut consumer);
    }

    fn heard(samples: &Arc<Mutex<Vec<f32>>>) -> Vec<f32> {
        samples.lock().expect("unpoisoned").clone()
    }

    /// Two copies of one recording, the second added 7 blocks (3584
    /// samples) later, play different samples until they are brought into
    /// step. Then the second dips out over 240 samples, jumps and dips back
    /// in over 240 more, and from there on the two play the very same
    /// samples. The anchor plays on untouched throughout.
    #[test]
    fn a_copy_added_later_lands_on_the_anchors_playhead() {
        let mut scene = empty_scene();
        let song = recording(48_000, 1);
        let anchor = add_loop(&mut scene, 0, &song);
        render_blocks(&mut scene, 7);
        let late = add_loop(&mut scene, 1, &song);
        render_blocks(&mut scene, 4);
        let offset = 7 * BLOCK;
        let before = 4 * BLOCK;
        {
            let (anchor, late) = (heard(&anchor), heard(&late));
            let differing = (0..before)
                .filter(|&index| anchor[offset + index] != late[index])
                .count();
            assert_eq!(differing, before, "out of step, every sample differs");
        }

        play_in_step(&mut scene, 0, 0b10);
        render_blocks(&mut scene, 6);
        let (anchor, late) = (heard(&anchor), heard(&late));
        // The anchor never jumped or dipped: it is the recording, round and round.
        for (index, &sample) in anchor.iter().enumerate() {
            assert_eq!(
                sample,
                song.samples[index % 48_000] * AMPLITUDE,
                "anchor at {index}"
            );
        }
        // After the dip out and back in, the copy is the anchor, sample for sample.
        let in_step_from = before + 2 * FADE;
        let compared = late.len() - in_step_from;
        assert_eq!(compared, 6 * BLOCK - 2 * FADE);
        for index in in_step_from..late.len() {
            assert_eq!(late[index], anchor[offset + index], "copy at {index}");
        }
        // The dip: down in 240 linear steps where it was, silent at the
        // jump, and up in 240 steps where the anchor is.
        let dipped = |sample: f32, steps_down: usize| {
            sample * AMPLITUDE * (1.0 - steps_down as f32 / FADE as f32)
        };
        for step in 0..FADE {
            let index = before + step;
            assert_eq!(
                late[index],
                dipped(song.samples[index], step),
                "out at {index}"
            );
            let index = before + FADE + step;
            let anchor_sample = song.samples[(offset + index) % 48_000];
            assert_eq!(
                late[index],
                dipped(anchor_sample, FADE - step),
                "in at {index}"
            );
        }
        assert_eq!(late[before + FADE], 0.0, "the jump falls on silence");
    }

    /// A loop of 30 000 samples (0.625 s) joining a song of 48 000 (1 s)
    /// lands at the song's time wrapped round its own length: from the end
    /// of the dip, at song sample `k` it plays its own sample `k mod 30 000`.
    /// Slot 3 is in the mask but empty, and slot 2 is not in it: neither
    /// changes anything.
    #[test]
    fn a_shorter_loop_lands_at_the_anchors_time_round_its_own_length() {
        let mut scene = empty_scene();
        let song = recording(48_000, 1);
        let short = recording(30_000, 2);
        let anchor = add_loop(&mut scene, 0, &song);
        render_blocks(&mut scene, 75);
        let joining = add_loop(&mut scene, 1, &short);
        let left_alone = add_loop(&mut scene, 2, &short);
        render_blocks(&mut scene, 2);
        play_in_step(&mut scene, 0, 0b1010);
        render_blocks(&mut scene, 4);

        let offset = 75 * BLOCK;
        let in_step_from = 2 * BLOCK + 2 * FADE;
        let joining = heard(&joining);
        for (index, &sample) in joining.iter().enumerate().skip(in_step_from) {
            let song_sample = offset + index;
            assert_eq!(
                sample,
                short.samples[(song_sample % 48_000) % 30_000] * AMPLITUDE,
                "at song sample {song_sample}"
            );
        }
        // Song sample 38 400 + 2 · 512 + 480 = 39 904 is past the short
        // loop's length, so it lands wrapped, at its sample 9 904.
        assert_eq!((offset + in_step_from) % 30_000, 9_904);
        let left_alone = heard(&left_alone);
        for (index, &sample) in left_alone.iter().enumerate() {
            assert_eq!(sample, short.samples[index % 30_000] * AMPLITUDE);
        }
        assert_eq!(heard(&anchor).len(), offset + 6 * BLOCK);
    }

    /// An anchor with no playhead (here an empty voice) moves nothing.
    #[test]
    fn an_anchor_without_a_playhead_moves_nothing() {
        let mut scene = empty_scene();
        let song = recording(48_000, 1);
        let playing = add_loop(&mut scene, 1, &song);
        render_blocks(&mut scene, 2);
        play_in_step(&mut scene, 0, 0b10);
        play_in_step(&mut scene, 99, 0b10);
        render_blocks(&mut scene, 2);
        for (index, &sample) in heard(&playing).iter().enumerate() {
            assert_eq!(sample, song.samples[index % 48_000] * AMPLITUDE);
        }
    }
}
