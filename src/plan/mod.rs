//! The session: what every voice will do, decided ahead of time.
//!
//! The audio thread does not decide when a voice sounds or what it says. A
//! [`Plan`] decides that from the scene's seed and each entity's id, ahead of
//! the clock, the way a video player buffers. A [`Planner`] runs it on its own
//! thread and hands the audio thread only what is due in the next fraction of
//! a second, so a change is heard almost at once and nothing planned further
//! ahead has to be taken back.
//!
//! The plan holds decisions, not sound. Every device that plays the same plan
//! hears the same take at the same moment, each from its own position.
//!
//! What is decided is a pure function of the rhythms, the seed and the time
//! window, so it is tested without a sound device.

mod clock;
mod conditions;
mod due;
mod rhythm;
mod runner;
mod session;
mod story;
mod voice;

pub use clock::Clock;
pub use conditions::{Condition, Conditions, Follow, Target};
pub use due::{Due, Scheduled};
pub use rhythm::{Rests, Rhythm, TakeRhythm};
pub use runner::Planner;
pub use session::Plan;
pub use story::{Story, Weather, PREVAILING_WIND_FROM};
pub use voice::Planned;
