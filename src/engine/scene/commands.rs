//! Commands from the control thread: the one `match` over `Command`, so the
//! compiler refuses a variant nobody handles.

use atrium_core::commands::Command;

use super::AudioScene;
use crate::profile_span;

impl AudioScene {
    /// Drain all pending commands from the consumer.
    /// Called once at the start of each audio callback invocation.
    pub fn process_commands(&mut self, consumer: &mut rtrb::Consumer<Command>) {
        let _s = profile_span!("process_commands").entered();
        while let Ok(cmd) = consumer.pop() {
            match cmd {
                Command::SetListenerPose { position, yaw } => {
                    self.listener.position = position;
                    self.listener.yaw = yaw;
                }
                Command::SetMasterGain { gain } => {
                    self.master_gain = gain;
                }
                Command::SetProbe { probe } => {
                    self.probe = probe;
                    for pipeline in &mut self.pipelines {
                        pipeline.probe = probe;
                    }
                }
                Command::SetPaused { paused } => {
                    self.paused = paused;
                }
                Command::SetSourceMuted { index, muted } => {
                    if let Some(source) = self.sources.get_mut(index as usize) {
                        source.set_muted(muted);
                    }
                }
                Command::SetSourcePosition { index, position } => {
                    if let Some(source) = self.sources.get_mut(index as usize) {
                        source.set_position(position);
                    }
                }
                Command::SetRenderMode { mode } => {
                    let new_pipeline = mode;
                    if new_pipeline != self.active_pipeline {
                        self.pipelines[new_pipeline.index()].reset();
                        self.active_pipeline = new_pipeline;
                    }
                }
                Command::SetSpeakerPosition { channel, position } => {
                    if let Some(speaker) =
                        self.speaker_layout.speaker_by_channel_mut(channel as usize)
                    {
                        speaker.position = position;
                    }
                }
                Command::SetSourceSpread { index, spread } => {
                    if let Some(source) = self.sources.get_mut(index as usize) {
                        source.set_spread(spread);
                    }
                }
                Command::SetSourceSpl { index, spl } => {
                    if let Some(source) = self.sources.get_mut(index as usize) {
                        let amplitude = source.set_reference_spl(spl);
                        // Keep the perceptual layer's base amplitude in sync.
                        if let Some(slot) = self.source_amplitudes.get_mut(index as usize) {
                            *slot = amplitude;
                        }
                    }
                }
                Command::SetSourceDirectivity { index, pattern } => {
                    if let Some(source) = self.sources.get_mut(index as usize) {
                        source.set_directivity(pattern);
                    }
                }
                Command::SetSourceFacing { index, yaw } => {
                    if let Some(source) = self.sources.get_mut(index as usize) {
                        source.set_facing(yaw);
                    }
                }
                Command::SetSourceOrbitSpeed { index, speed } => {
                    if let Some(source) = self.sources.get_mut(index as usize) {
                        source.set_orbit_speed(speed);
                    }
                }
                Command::SetSourceOrbitRadius { index, radius } => {
                    if let Some(source) = self.sources.get_mut(index as usize) {
                        source.set_orbit_radius(radius);
                    }
                }
                Command::SetSourceOrbitAngle { index, angle } => {
                    if let Some(source) = self.sources.get_mut(index as usize) {
                        source.set_orbit_angle(angle);
                    }
                }
                Command::SetChannelMode { mode } => self.set_channel_mode(mode),
                Command::SetAtmosphere {
                    temperature_c,
                    humidity_pct,
                } => {
                    self.environment.atmosphere.temperature_c = temperature_c;
                    self.environment.atmosphere.humidity_pct = humidity_pct;
                }
                Command::SetSynthParam {
                    index,
                    param,
                    value,
                } => {
                    if let Some(source) = self.sources.get_mut(index as usize) {
                        source.set_synth_param(param, value);
                    }
                }
                Command::ResetScene => self.reset_to_initial_state(),
            }
        }
    }
}
