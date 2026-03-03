use crate::audio::mixer::{mix_sources, DistanceModel};
use crate::engine::commands::Command;
use crate::processors::AudioProcessor;
use crate::spatial::listener::Listener;
use crate::spatial::source::SoundSource;
use crate::world::room::Room;

/// The complete audio state owned by the audio thread.
/// Updated by draining commands from the ring buffer.
/// Never shared with the control thread directly.
pub struct AudioScene {
    pub listener: Listener,
    pub sources: Vec<Box<dyn SoundSource>>,
    pub room: Box<dyn Room>,
    pub master_gain: f32,
    pub sample_rate: f32,
    pub distance_model: DistanceModel,
    pub processors: Vec<Box<dyn AudioProcessor>>,
}

impl AudioScene {
    /// Drain all pending commands from the consumer.
    /// Called once at the start of each audio callback invocation.
    pub fn process_commands(&mut self, consumer: &mut rtrb::Consumer<Command>) {
        while let Ok(cmd) = consumer.pop() {
            match cmd {
                Command::SetListenerPose { position, yaw } => {
                    self.listener.position = position;
                    self.listener.yaw = yaw;
                }
                Command::SetMasterGain { gain } => {
                    self.master_gain = gain;
                }
            }
        }
    }

    /// Initialize all processors with room geometry and sample rate.
    /// Must be called after sample_rate is set and before the audio callback starts.
    pub fn init_processors(&mut self) {
        let (room_min, room_max) = self.room.bounds();
        for processor in &mut self.processors {
            processor.init(room_min, room_max, &self.listener, self.sample_rate);
        }
    }

    /// Render one buffer of audio.
    /// `output` is an interleaved sample buffer (e.g. [L, R, L, R, ...] for stereo).
    pub fn render(&mut self, output: &mut [f32], channels: usize) {
        let num_frames = output.len() / channels;
        let dt = num_frames as f32 / self.sample_rate;

        // Advance time-varying state on all sources
        for source in &mut self.sources {
            source.tick(dt);
        }

        // Mix all sources to output
        mix_sources(
            &mut self.sources,
            &self.listener,
            output,
            channels,
            self.sample_rate,
            self.master_gain,
            &self.distance_model,
        );

        // Run processor chain (early reflections, reverb, etc.)
        for processor in &mut self.processors {
            processor.process(output, channels, self.sample_rate);
        }
    }
}
