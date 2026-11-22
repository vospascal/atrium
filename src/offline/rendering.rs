//! A scene prepared as the output device prepares it, then played block by
//! block into frames, the plan driven in step.

use atrium_core::commands::Command;
use atrium_core::speaker::{channel_name, RenderMode};
use atrium_core::types::Vec3;

use super::settings::{Output, RenderSettings};
use super::statistics::ChannelStatistics;
use crate::config::SceneConfig;
use crate::engine::scene::AudioScene;
use crate::plan::SteppedPlanner;

/// The session seed of a scene that names none, rendered without `--seed`.
/// A playing scene without one takes the time of day; a render must not.
const DEFAULT_SEED: u64 = 1;

/// Commands waiting for the next block: at most a head turn and a probe.
const COMMAND_CAPACITY: usize = 16;

/// A scene built and prepared, ready to be rendered.
pub struct Rendering {
    scene: AudioScene,
    planner: SteppedPlanner,
    commands: rtrb::Producer<Command>,
    commands_out: rtrb::Consumer<Command>,
    settings: RenderSettings,
    channels: usize,
    seed: u64,
    layout: String,
    render_mode: RenderMode,
    /// Where the scene puts the listener and which way it faces; a head
    /// turn turns from here.
    listener_position: Vec3,
    listener_yaw: f32,
}

impl Rendering {
    /// Build `scene` for `settings` and prepare it as the output device
    /// does when it opens: its rate, its channels, its pipelines.
    pub fn prepare(
        mut scene: SceneConfig,
        settings: &RenderSettings,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        settings.check()?;
        match settings.output {
            Output::Scene => {}
            Output::Headphones => scene = scene.for_headphones(),
            Output::Mode(mode) => scene.speakers.render_mode = mode.as_str().to_owned(),
        }
        let seed = settings.seed.or(scene.seed).unwrap_or(DEFAULT_SEED);
        scene.seed = Some(seed);
        let layout = scene.speakers.layout.clone();

        let built = scene.build_unstarted()?;
        let mut audio = built.scene;
        let channels = audio.speaker_layout.total_channels();
        if let Some(only) = settings.probe.only_channel {
            if only as usize >= channels {
                return Err(format!(
                    "the probe's channel {only} is not one of this output's {channels}"
                )
                .into());
            }
        }
        // As `CpalOutput::start` does, on a device with the layout's channels.
        audio.sample_rate = settings.sample_rate as f32;
        audio.fit_to_output_device(channels);
        audio.init_pipelines();

        let planner = SteppedPlanner::new(
            built.plan,
            built.plan_out,
            settings.start,
            settings.weather_speed,
        );
        let (commands, commands_out) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
        Ok(Self {
            render_mode: audio.active_pipeline,
            listener_position: audio.listener.position,
            listener_yaw: audio.listener.yaw,
            scene: audio,
            planner,
            commands,
            commands_out,
            settings: settings.clone(),
            channels,
            seed,
            layout,
        })
    }

    /// The session seed the render follows.
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// The render mode it is rendered in.
    pub fn render_mode(&self) -> RenderMode {
        self.render_mode
    }

    /// Render every block, as the audio callback would: what is due handed
    /// over first, then the commands drained, then the block rendered.
    pub fn run(mut self) -> Rendered {
        let RenderSettings {
            seconds,
            start,
            sample_rate,
            block_frames,
            probe,
            turn_degrees_per_second,
            ..
        } = self.settings;
        let channels = self.channels;
        let total_frames = (seconds * sample_rate as f64).round() as usize;
        let mut frames = Vec::with_capacity(total_frames * channels);
        let mut block = vec![0.0f32; block_frames * channels];

        // Before the first block, so it holds from the first sample.
        if !probe.is_whole() {
            self.command(Command::SetProbe { probe });
        }
        let turn = turn_degrees_per_second.to_radians();
        let mut rendered = 0;
        while rendered < total_frames {
            let frames_now = block_frames.min(total_frames - rendered);
            if turn != 0.0 {
                // Turned by scene time, so a render from `start` faces where
                // a render from 0 faces at that moment.
                let scene_seconds = start + rendered as f64 / sample_rate as f64;
                let yaw = (self.listener_yaw as f64 + turn * scene_seconds)
                    .rem_euclid(std::f64::consts::TAU) as f32;
                self.command(Command::SetListenerPose {
                    position: self.listener_position,
                    yaw,
                });
            }
            self.planner.hand_over(
                (rendered + frames_now) as f64 / sample_rate as f64,
                sample_rate as f32,
            );
            let buffer = &mut block[..frames_now * channels];
            self.scene.process_commands(&mut self.commands_out);
            self.scene.render(buffer, channels);
            frames.extend_from_slice(buffer);
            rendered += frames_now;
        }

        Rendered {
            statistics: ChannelStatistics::of(&frames, channels),
            frames,
            channels,
            seed: self.seed,
            layout: self.layout,
            render_mode: self.render_mode,
            settings: self.settings,
            gain_decibels: 0.0,
        }
    }

    fn command(&mut self, command: Command) {
        // Drained every block, and at most two are pushed per block.
        self.commands
            .push(command)
            .expect("the command ring is drained every block");
    }
}

/// A rendered scene: its frames and what they are.
#[derive(Clone, Debug)]
pub struct Rendered {
    frames: Vec<f32>,
    channels: usize,
    seed: u64,
    layout: String,
    render_mode: RenderMode,
    settings: RenderSettings,
    statistics: Vec<ChannelStatistics>,
    /// What [`Rendered::normalise`] raised the frames by (dB); 0 as the
    /// engine rendered them.
    gain_decibels: f64,
}

impl Rendered {
    /// Raise (or lower) every channel by one gain so the loudest sample of
    /// the whole render peaks at `peak_decibels` dBFS. One gain for all
    /// channels keeps the balance between them, so where a sound sits is
    /// unchanged. A silent render is left as it is.
    ///
    /// The engine renders at its calibration, where a bird at 60 dB SPL peaks
    /// far below full scale; this makes a file that can be played as it is.
    /// The gain is recorded, so the engine's own level is the file's level
    /// minus [`Rendered::gain_decibels`].
    pub fn normalise(&mut self, peak_decibels: f64) {
        let Some(gain) = gain_to_peak(&self.statistics, peak_decibels) else {
            return;
        };
        for sample in &mut self.frames {
            *sample = (*sample as f64 * gain) as f32;
        }
        self.statistics = ChannelStatistics::of(&self.frames, self.channels);
        self.gain_decibels += 20.0 * gain.log10();
    }

    /// The gain [`Rendered::normalise`] applied (dB); 0 if it was not.
    pub fn gain_decibels(&self) -> f64 {
        self.gain_decibels
    }

    /// Interleaved frames, every channel of the layout.
    pub fn frames(&self) -> &[f32] {
        &self.frames
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Each channel's name in order: L, R, C, LFE, Ls, Rs, or L, R.
    pub fn channel_names(&self) -> Vec<&'static str> {
        (0..self.channels).map(channel_name).collect()
    }

    pub fn sample_rate(&self) -> u32 {
        self.settings.sample_rate
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// The speaker layout as the scene names it: `stereo`, `quad` or `5.1`.
    pub fn layout(&self) -> &str {
        &self.layout
    }

    pub fn render_mode(&self) -> RenderMode {
        self.render_mode
    }

    pub fn settings(&self) -> &RenderSettings {
        &self.settings
    }

    /// Peak and RMS of each channel, in channel order.
    pub fn statistics(&self) -> &[ChannelStatistics] {
        &self.statistics
    }
}

/// The gain that brings the loudest of `channels` to `peak_decibels` dBFS,
/// or `None` when every channel is silent.
fn gain_to_peak(channels: &[ChannelStatistics], peak_decibels: f64) -> Option<f64> {
    let peak = channels
        .iter()
        .fold(0f32, |peak, channel| peak.max(channel.peak));
    (peak > 0.0).then(|| 10f64.powf(peak_decibels / 20.0) / peak as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(peak: f32) -> ChannelStatistics {
        ChannelStatistics { peak, rms: 0.0 }
    }

    #[test]
    fn the_loudest_channel_sets_the_gain() {
        // −1 dBFS is 0.891 25; the louder channel, 0.01, sets it (0.01 as
        // an f32 is 2e-10 short, hence the tolerance).
        let gain = gain_to_peak(&[channel(0.002), channel(0.01)], -1.0).expect("not silent");
        assert!((gain - 89.125_094).abs() < 1e-4, "{gain}");
        assert_eq!(gain_to_peak(&[channel(0.0), channel(0.0)], -1.0), None);
        let unchanged = gain_to_peak(&[channel(0.5)], 20.0 * 0.5f64.log10()).expect("not silent");
        assert!((unchanged - 1.0).abs() < 1e-12, "{unchanged}");
    }
}
