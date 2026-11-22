//! What to render: how long, from which moment of the scene, at which rate
//! and block size, for which output, and what the listener does meanwhile.

use atrium_core::probe::Probe;
use atrium_core::speaker::RenderMode;

/// Which speakers or ears a scene is rendered for. The file has as many
/// channels as the layout: 2 on headphones and stereo, 6 for quad and 5.1
/// (quad leaves C and LFE silent).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Output {
    /// As the scene file says: its speaker layout and its render mode.
    Scene,
    /// Binaurally on two channels, whatever speakers the scene names, as the
    /// engine's `--headphones` (`SceneConfig::for_headphones`).
    Headphones,
    /// The scene's speaker layout, rendered in this mode instead of its own.
    Mode(RenderMode),
}

/// Everything about a render that is not the scene itself.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderSettings {
    /// How long to render (s).
    pub seconds: f64,
    /// Where in the scene to start (s), as the editor's timeline seeks:
    /// every voice and the weather as they would be there.
    pub start: f64,
    /// Output samples per second.
    pub sample_rate: u32,
    /// Frames per call to the engine, as the audio callback's buffer.
    pub block_frames: usize,
    pub output: Output,
    /// The session seed every choice follows from. `None` for the scene's
    /// own `seed:`, and for a scene without one, a fixed seed.
    pub seed: Option<u64>,
    /// Which parts of the render reach the file. Set before the first
    /// block, it holds from the first sample.
    pub probe: Probe,
    /// The listener's head turning at a steady rate (degrees per second),
    /// positive to the left (counter-clockwise seen from above, the way
    /// `Listener::yaw` grows), as head tracking would turn it.
    pub turn_degrees_per_second: f64,
    /// How many times as fast as the session the weather runs, from the
    /// scene's beginning, as the editor's time-lapse.
    pub weather_speed: f64,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            seconds: 30.0,
            start: 0.0,
            sample_rate: 48_000,
            block_frames: 512,
            output: Output::Scene,
            seed: None,
            probe: Probe::WHOLE,
            turn_degrees_per_second: 0.0,
            weather_speed: 1.0,
        }
    }
}

/// The largest block the engine is prepared for: its buffers are reserved
/// for callbacks up to this size before audio runs.
const LARGEST_BLOCK_FRAMES: usize = 8192;

impl RenderSettings {
    /// Why these settings cannot be rendered, if they cannot.
    pub(super) fn check(&self) -> Result<(), String> {
        if !(self.seconds.is_finite() && self.seconds > 0.0) {
            return Err(format!("seconds must be more than 0, not {}", self.seconds));
        }
        if !(self.start.is_finite() && self.start >= 0.0) {
            return Err(format!("start must be 0 or more, not {}", self.start));
        }
        if !(8_000..=384_000).contains(&self.sample_rate) {
            return Err(format!(
                "sample rate must be 8000 to 384000 Hz, not {}",
                self.sample_rate
            ));
        }
        if !(1..=LARGEST_BLOCK_FRAMES).contains(&self.block_frames) {
            return Err(format!(
                "block must be 1 to {LARGEST_BLOCK_FRAMES} frames, not {}",
                self.block_frames
            ));
        }
        if !self.turn_degrees_per_second.is_finite() {
            return Err("turn must be a number of degrees per second".into());
        }
        if !(self.weather_speed.is_finite() && self.weather_speed > 0.0) {
            return Err(format!(
                "weather speed must be more than 0, not {}",
                self.weather_speed
            ));
        }
        Ok(())
    }
}
