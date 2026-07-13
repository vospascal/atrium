//! Control-rate behavior layer on Bevy ECS.
//!
//! Runs identically inside the full Bevy editor App and in a headless App
//! built with `MinimalPlugins` — no window, no renderer, no winit. Systems
//! here decide *what, when, and where* at control rate (~30–60 Hz) and emit
//! small `Copy` commands to the audio thread over the rtrb ring buffer; all
//! per-sample DSP (attenuation, air absorption, directivity, envelopes) stays
//! in the engine. See `docs/behavior-ecs-headless.md`: ECS decides, engine
//! renders.

use std::f32::consts::TAU;

use atrium_core::commands::Command;
use atrium_core::types::Vec3;
use bevy::prelude::*;
use rtrb::Producer;

// ── Command bridge (ECS → audio thread) ─────────────────────────────────────

/// Wrapper around rtrb::Producer that is Send + Sync.
///
/// rtrb::Producer is safe to send between threads (it uses atomic operations),
/// but contains a raw pointer so Rust doesn't auto-impl Send.
struct SendProducer(Producer<Command>);

// SAFETY: rtrb::Producer uses atomic operations for synchronization and is
// designed for single-producer access, which Bevy guarantees since this
// resource is only reachable via ResMut (exclusive access).
unsafe impl Send for SendProducer {}
unsafe impl Sync for SendProducer {}

/// Resource wrapping the rtrb producer for sending commands to the audio thread.
#[derive(Resource)]
pub struct CommandSender {
    producer: SendProducer,
}

impl CommandSender {
    pub fn new(producer: Producer<Command>) -> Self {
        Self {
            producer: SendProducer(producer),
        }
    }

    /// Try to push a command into the ring buffer.
    /// Silently drops if the buffer is full (non-blocking).
    pub fn send(&mut self, command: Command) {
        let _ = self.producer.0.push(command);
    }
}

// ── Components ──────────────────────────────────────────────────────────────

/// Maps a behavior entity to the audio engine's source slot index.
/// Ephemeral — rebuilt whenever a scene loads, never serialized.
#[derive(Component, Clone, Copy, Debug)]
pub struct EngineSlot(pub u16);

/// Slow loudness "breathing": sinusoidal SPL modulation around a base value.
/// The audio thread recomputes source amplitude on every `SetSourceSpl`, so
/// this stays a pure control-rate concern.
#[derive(Component, Clone, Debug)]
pub struct IntensityLfo {
    /// Center SPL (dB at 1 m) the modulation breathes around.
    pub base_spl: f32,
    /// Peak deviation from the base (dB).
    pub depth_db: f32,
    /// Seconds for one full breathing cycle.
    pub period_seconds: f32,
    /// Current phase in radians.
    pub phase: f32,
    /// Last SPL actually sent — NaN until the first send.
    last_sent_spl: f32,
}

impl IntensityLfo {
    pub fn new(base_spl: f32, depth_db: f32, period_seconds: f32) -> Self {
        Self {
            base_spl,
            depth_db,
            period_seconds,
            phase: 0.0,
            last_sent_spl: f32::NAN,
        }
    }
}

/// Circular motion in the horizontal plane around a fixed center.
/// Behavior-side replacement for the engine-side orbit commands
/// (`SetSourceOrbitSpeed`/`Radius`/`Angle`), which this layer will obsolete.
#[derive(Component, Clone, Debug)]
pub struct OrbitMotion {
    /// Orbit center in world coordinates (meters).
    pub center: Vec3,
    /// Orbit radius (meters).
    pub radius: f32,
    /// Seconds per full revolution.
    pub seconds_per_revolution: f32,
    /// Current angle in radians (0 = +X, counter-clockwise).
    pub angle: f32,
}

// ── Systems ─────────────────────────────────────────────────────────────────

/// Advance every intensity LFO and send the new SPL when it moved audibly.
/// The 0.01 dB gate keeps the command ring quiet between meaningful changes.
pub fn breathe_intensity(
    time: Res<Time>,
    mut command_sender: ResMut<CommandSender>,
    mut query: Query<(&EngineSlot, &mut IntensityLfo)>,
) {
    let dt = time.delta_secs();
    for (slot, mut lfo) in &mut query {
        if lfo.period_seconds <= 0.0 {
            continue;
        }
        lfo.phase = (lfo.phase + TAU * dt / lfo.period_seconds) % TAU;
        let spl = lfo.base_spl + lfo.depth_db * lfo.phase.sin();
        let changed = lfo.last_sent_spl.is_nan() || (spl - lfo.last_sent_spl).abs() > 0.01;
        if changed {
            command_sender.send(Command::SetSourceSpl { index: slot.0, spl });
            lfo.last_sent_spl = spl;
        }
    }
}

/// Advance every orbit and reposition its source in the engine.
pub fn move_orbits(
    time: Res<Time>,
    mut command_sender: ResMut<CommandSender>,
    mut query: Query<(&EngineSlot, &mut OrbitMotion)>,
) {
    let dt = time.delta_secs();
    for (slot, mut orbit) in &mut query {
        if orbit.seconds_per_revolution <= 0.0 || orbit.radius <= 0.0 {
            continue;
        }
        orbit.angle = (orbit.angle + TAU * dt / orbit.seconds_per_revolution) % TAU;
        let position = Vec3 {
            x: orbit.center.x + orbit.radius * orbit.angle.cos(),
            y: orbit.center.y + orbit.radius * orbit.angle.sin(),
            z: orbit.center.z,
        };
        command_sender.send(Command::SetSourcePosition {
            index: slot.0,
            position,
        });
    }
}

// ── Plugin ──────────────────────────────────────────────────────────────────

/// Adds the control-rate behavior systems. Host App must insert a
/// [`CommandSender`] resource and provide `Time` (any App with `TimePlugin`,
/// including `MinimalPlugins`, does).
pub struct BehaviorPlugin;

impl Plugin for BehaviorPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (breathe_intensity, move_orbits));
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Build a bare headless App with a manually-driven clock and a command
    /// ring we can inspect. No plugins at all — proves bevy_ecs needs no
    /// window, renderer, or schedule runner.
    fn bare_app() -> (App, rtrb::Consumer<Command>) {
        let (producer, consumer) = rtrb::RingBuffer::<Command>::new(64);
        let mut app = App::new();
        app.add_plugins(BehaviorPlugin);
        app.insert_resource(Time::<()>::default());
        app.insert_resource(CommandSender::new(producer));
        (app, consumer)
    }

    fn advance(app: &mut App, seconds: f32) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(Duration::from_secs_f32(seconds));
        app.update();
    }

    fn drain(consumer: &mut rtrb::Consumer<Command>) -> Vec<Command> {
        let mut commands = Vec::new();
        while let Ok(command) = consumer.pop() {
            commands.push(command);
        }
        commands
    }

    #[test]
    fn lfo_breathes_at_the_configured_period() {
        let (mut app, mut consumer) = bare_app();
        app.world_mut()
            .spawn((EngineSlot(3), IntensityLfo::new(60.0, 6.0, 4.0)));

        // First tick (dt = 0): phase 0 → sin 0 → base SPL, sent once (NaN gate).
        app.update();
        // Quarter period: phase π/2 → sin 1 → base + depth.
        advance(&mut app, 1.0);
        // Half period from start: phase π → sin 0 → back to base.
        advance(&mut app, 1.0);

        let spls: Vec<f32> = drain(&mut consumer)
            .into_iter()
            .map(|command| match command {
                Command::SetSourceSpl { index: 3, spl } => spl,
                other => panic!("unexpected command: {other:?}"),
            })
            .collect();
        assert_eq!(spls.len(), 3);
        assert!((spls[0] - 60.0).abs() < 1e-4, "t=0 should be base SPL");
        assert!(
            (spls[1] - 66.0).abs() < 1e-3,
            "quarter period should be base + depth, got {}",
            spls[1]
        );
        assert!(
            (spls[2] - 60.0).abs() < 1e-3,
            "half period should be back at base, got {}",
            spls[2]
        );
    }

    #[test]
    fn orbit_quarter_revolution_rotates_ninety_degrees() {
        let (mut app, mut consumer) = bare_app();
        let center = Vec3 {
            x: 2.0,
            y: -1.0,
            z: 1.5,
        };
        app.world_mut().spawn((
            EngineSlot(7),
            OrbitMotion {
                center,
                radius: 4.0,
                seconds_per_revolution: 8.0,
                angle: 0.0,
            },
        ));

        app.update(); // dt = 0: still at angle 0 → (center.x + r, center.y)
        advance(&mut app, 2.0); // quarter revolution → angle π/2 → (center.x, center.y + r)

        let positions: Vec<Vec3> = drain(&mut consumer)
            .into_iter()
            .map(|command| match command {
                Command::SetSourcePosition { index: 7, position } => position,
                other => panic!("unexpected command: {other:?}"),
            })
            .collect();
        assert_eq!(positions.len(), 2);
        assert!((positions[0].x - 6.0).abs() < 1e-4);
        assert!((positions[0].y - -1.0).abs() < 1e-4);
        assert!((positions[1].x - 2.0).abs() < 1e-3);
        assert!((positions[1].y - 3.0).abs() < 1e-3);
        // Height untouched by horizontal orbit.
        assert!((positions[1].z - 1.5).abs() < 1e-6);
    }

    #[test]
    fn runs_under_minimal_plugins_without_window() {
        // The real headless configuration: MinimalPlugins provides the task
        // pool, frame counting, and TimePlugin — still no window or renderer.
        let (producer, mut consumer) = rtrb::RingBuffer::<Command>::new(64);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(BehaviorPlugin);
        app.insert_resource(CommandSender::new(producer));
        app.world_mut().spawn((
            EngineSlot(0),
            OrbitMotion {
                center: Vec3::ZERO,
                radius: 1.0,
                seconds_per_revolution: 4.0,
                angle: 0.0,
            },
        ));

        app.update();
        app.update();

        let commands = drain(&mut consumer);
        assert!(
            commands
                .iter()
                .all(|command| matches!(command, Command::SetSourcePosition { index: 0, .. })),
            "unexpected command kind in {commands:?}"
        );
        assert!(
            commands.len() >= 2,
            "expected a position update per frame, got {}",
            commands.len()
        );
    }
}
