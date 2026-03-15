//! Telemetry bridge: rtrb ring buffer → Bevy ECS messages.
//!
//! Each frame, `poll_telemetry` drains the ring buffer and writes the latest
//! `TelemetryFrame` as a Bevy message that other systems can read.
//!
//! The `TelemetryFrame` type comes from `atrium_core::telemetry` — the same
//! concrete type the audio engine pushes into the ring buffer.

use atrium_core::commands::Command;
use atrium_core::telemetry::TelemetryFrame;
use bevy::prelude::*;
use rtrb::{Consumer, Producer};

/// Bevy message written each frame with the latest telemetry.
#[derive(Message, Clone, Debug)]
pub struct TelemetryMessage {
    pub frame: TelemetryFrame,
}

/// Wrapper around rtrb::Consumer that is Send + Sync.
///
/// rtrb::Consumer is safe to send between threads (it uses atomic operations),
/// but contains a raw pointer so Rust doesn't auto-impl Send.
/// See: https://docs.rs/rtrb/latest/rtrb/struct.Consumer.html
struct SendConsumer(Consumer<TelemetryFrame>);

// SAFETY: rtrb::Consumer uses atomic operations for synchronization
// and is designed to be used from a single consumer thread (which Bevy guarantees
// since this resource is accessed via ResMut, preventing concurrent access).
unsafe impl Send for SendConsumer {}
unsafe impl Sync for SendConsumer {}

/// Resource wrapping the rtrb consumer. Inserted before `App::run()`.
#[derive(Resource)]
pub struct TelemetryReceiver {
    consumer: SendConsumer,
}

impl TelemetryReceiver {
    pub fn new(consumer: Consumer<TelemetryFrame>) -> Self {
        Self {
            consumer: SendConsumer(consumer),
        }
    }
}

// ── Command sender (Bevy → audio thread) ────────────────────────────────────

struct SendProducer(Producer<Command>);

// SAFETY: same reasoning as SendConsumer — rtrb uses atomics, single-producer access.
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

/// Cached latest telemetry frame. Always available (defaults to `TelemetryFrame::default()`).
/// Button handlers read from this so they work even on frames with no new message.
#[derive(Resource, Default)]
pub struct LatestTelemetry {
    pub frame: TelemetryFrame,
}

/// System: drain the ring buffer each Bevy frame, write the latest as a message.
pub fn poll_telemetry(
    mut receiver: ResMut<TelemetryReceiver>,
    mut writer: MessageWriter<TelemetryMessage>,
    mut cached: ResMut<LatestTelemetry>,
) {
    let mut latest: Option<TelemetryFrame> = None;
    while let Ok(frame) = receiver.consumer.0.pop() {
        latest = Some(frame);
    }
    if let Some(frame) = latest {
        cached.frame = frame;
        writer.write(TelemetryMessage { frame });
    }
}
