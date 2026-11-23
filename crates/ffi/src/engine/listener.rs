//! The listener and the air around it: where it stands, which way it faces,
//! what steers it, and the air sound travels through.

use atrium::config::ListenerConfig;
use atrium::head_tracking::Pose;
use atrium_core::commands::Command;
use atrium_core::types::Vec3;
use atrium_world::Place;

use super::{finite, lock, rounded, Engine, Live};
use crate::api::{EngineError, Position};
use crate::head_tracking::{head_trackers, HeadTracker};

/// How far ahead of the listener an auditioned or newly added sound stands (m).
const AHEAD_OF_LISTENER: f32 = 2.0;
/// The coldest and warmest air a scene can be given (°C).
const COLDEST: f32 = -20.0;
const WARMEST: f32 = 50.0;

#[uniffi::export]
impl Engine {
    /// Move the listener to `position`, facing the way it faced. Every sound
    /// is heard from there at once; head tracking turns from there; the
    /// scene file keeps it. A position that is not a number changes nothing.
    pub fn set_listener_position(&self, position: Position) -> Result<(), EngineError> {
        if !finite(position) {
            return Ok(());
        }
        let mut scene = lock(&self.scene);
        let pose = Pose {
            position: position.into(),
            ..self.placement.get()
        };
        self.play_listener(&mut scene, pose)?;
        scene.document.listener = self.listener_in_file(pose);
        Ok(())
    }

    /// Turn the listener to face along `facing`, a direction over the ground,
    /// as `set_source_facing` turns a sound. Head tracking turns from there.
    pub fn set_listener_facing(&self, facing: Position) -> Result<(), EngineError> {
        let length = facing.x.hypot(facing.y);
        if !length.is_finite() || length < 1e-6 {
            return Ok(());
        }
        let mut scene = lock(&self.scene);
        let pose = Pose {
            yaw: facing.y.atan2(facing.x),
            ..self.placement.get()
        };
        self.play_listener(&mut scene, pose)?;
        scene.document.listener = self.listener_in_file(pose);
        Ok(())
    }

    /// Let `tracker` steer the listener, or stop with `None`. Stopping turns
    /// the listener back to the way the scene faces it. Any tracker already
    /// running is stopped first, and has let go of its port when this returns.
    pub fn set_head_tracker(&self, tracker: Option<HeadTracker>) -> Result<(), EngineError> {
        let mut head_tracking = lock(&self.head_tracking);
        // Stop before starting: both listen on the same port.
        *head_tracking = None;
        let Some(tracker) = tracker else {
            return Ok(());
        };
        if !head_trackers().contains(&tracker) {
            return Err(EngineError::HeadTracking(
                "that tracker does not run on this platform".into(),
            ));
        }
        let started = atrium::head_tracking::start(
            tracker.into(),
            self.commands.clone(),
            self.placement.clone(),
        )
        .map_err(|error| EngineError::HeadTracking(error.to_string()))?;
        *head_tracking = Some(started);
        Ok(())
    }

    /// Set the air sound travels through: `temperature` in °C, from −20 to
    /// 50, and relative `humidity` in %. They decide how fast sound travels
    /// and how much treble the air takes from a distant sound. `scene()`
    /// reports them from then on, and the scene file keeps them as its own.
    /// A listening session keeps standard air and refuses it.
    pub fn set_atmosphere(&self, temperature: f32, humidity: f32) -> Result<(), EngineError> {
        if !(temperature.is_finite() && humidity.is_finite()) {
            return Ok(());
        }
        let temperature = rounded(temperature.clamp(COLDEST, WARMEST), 2.0);
        let humidity = rounded(humidity.clamp(0.0, 100.0), 1.0);
        let mut scene = lock(&self.scene);
        if scene.document.kind == atrium::config::SceneKind::Listening {
            return Err(EngineError::Scene(
                "a listening session keeps standard air".to_string(),
            ));
        }
        scene
            .document
            .set_air(temperature, humidity)
            .map_err(|error| EngineError::Scene(error.to_string()))?;
        self.play_air(&mut scene, temperature, humidity)
    }
}

impl Engine {
    /// A short way ahead of the listener, at ear height, in the scene file's
    /// coordinates.
    pub(super) fn place_ahead(&self) -> Place {
        let pose = self.placement.get();
        let ahead = Vec3::new(pose.yaw.cos(), pose.yaw.sin(), 0.0);
        self.sounds()
            .place_at(pose.position + ahead * AHEAD_OF_LISTENER)
    }

    /// Put the listener at `pose`: heard from there at once, head tracking
    /// turns from it, and `scene()` reports it.
    pub(super) fn play_listener(&self, scene: &mut Live, pose: Pose) -> Result<(), EngineError> {
        self.command(Command::SetListenerPose {
            position: pose.position,
            yaw: pose.yaw,
        })?;
        self.placement.set(pose);
        scene.description.listener = pose.position.into();
        Ok(())
    }

    /// The listener as the scene file writes it, for `pose`.
    fn listener_in_file(&self, pose: Pose) -> ListenerConfig {
        let place = self.place_rounded(pose.position.into());
        ListenerConfig {
            position: [place.x, place.y, place.z],
            yaw_degrees: rounded(pose.yaw.to_degrees().rem_euclid(360.0), 1.0),
        }
    }

    /// Give the place air at `temperature` °C and `humidity` %.
    pub(super) fn play_air(
        &self,
        scene: &mut Live,
        temperature: f32,
        humidity: f32,
    ) -> Result<(), EngineError> {
        self.command(Command::SetAtmosphere {
            temperature_c: temperature,
            humidity_pct: humidity,
        })?;
        scene.description.temperature_c = temperature;
        scene.description.humidity_pct = humidity;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::telemetry_when;
    use super::*;
    use crate::head_tracking::HeadTrackingStatus;

    /// Opens the default output device at zero volume and binds the tracker
    /// port, so nothing else may be listening on it:
    /// `cargo test -p atrium-ffi -- --ignored`.
    #[test]
    #[ignore = "opens the default output device and the head tracking port"]
    fn head_tracking_holds_its_port_only_while_it_is_on() {
        std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .expect("repository root");
        // OpenTrack's default port, which the engine listens on.
        let port_is_free = || std::net::UdpSocket::bind(("127.0.0.1", 4242)).is_ok();
        assert!(port_is_free(), "stop the running head tracker first");

        let engine = Engine::new("scenes/frog.yaml".into()).expect("scene should play");
        engine.set_volume(0.0).expect("volume");

        engine
            .set_head_tracker(Some(HeadTracker::External))
            .expect("the listener starts");
        assert!(!port_is_free(), "the tracker listens while it is on");

        // Asking again restarts it on the same port instead of failing.
        engine
            .set_head_tracker(Some(HeadTracker::External))
            .expect("a running tracker can be chosen again");

        engine.set_head_tracker(None).expect("the listener stops");
        assert!(port_is_free(), "switching off frees the port at once");

        // A scene change closes the engine with tracking still on. The next
        // scene's engine binds the same port straight away.
        engine
            .set_head_tracker(Some(HeadTracker::External))
            .expect("the listener starts again");
        drop(engine);
        assert!(port_is_free(), "closing the engine stops head tracking");
    }

    /// What the app's map and Settings read: whether a tracker is chosen,
    /// whether it is sending, and where the head points. A stand-in tracker
    /// sends one OpenTrack packet over loopback. Opens the default output
    /// device at zero volume: `cargo test -p atrium-ffi -- --ignored`.
    #[test]
    #[ignore = "opens the default output device and the head tracking port"]
    fn telemetry_shows_whether_a_tracker_is_sending_and_where_the_head_points() {
        std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .expect("repository root");
        // The frog scene faces the listener north: +Y.
        let engine = Engine::new("scenes/frog.yaml".into()).expect("scene should play");
        engine.set_volume(0.0).expect("volume");

        let at_rest = telemetry_when(&engine, |_| true);
        assert_eq!(at_rest.head_tracking, HeadTrackingStatus::Off);
        assert!(at_rest.listener_facing.y > 0.99, "the scene faces north");

        engine
            .set_head_tracker(Some(HeadTracker::External))
            .expect("the listener starts");
        let listening = telemetry_when(&engine, |telemetry| {
            telemetry.head_tracking != HeadTrackingStatus::Off
        });
        assert_eq!(
            listening.head_tracking,
            HeadTrackingStatus::Waiting,
            "chosen, but nothing has been sent"
        );

        // A 90° left turn, which a tracker reports as −90°. North turns west.
        let left_turn: Vec<u8> = [0.0f64, 0.0, 0.0, -90.0, 0.0, 0.0]
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        std::net::UdpSocket::bind(("127.0.0.1", 0))
            .and_then(|tracker| tracker.send_to(&left_turn, ("127.0.0.1", 4242)))
            .expect("the stand-in tracker sends");
        let turned = telemetry_when(&engine, |telemetry| {
            telemetry.head_tracking == HeadTrackingStatus::Receiving
                && telemetry.listener_facing.x < -0.99
        });
        assert!(turned.listener_facing.y.abs() < 0.1, "the head points west");

        engine.set_head_tracker(None).expect("the listener stops");
        telemetry_when(&engine, |telemetry| {
            telemetry.head_tracking == HeadTrackingStatus::Off && telemetry.listener_facing.y > 0.99
        });
    }
}
