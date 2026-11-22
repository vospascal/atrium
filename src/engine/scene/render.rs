//! One buffer of audio: hand on what the plan made due, tick the sources,
//! score what masks what, render the active pipeline, fade for pause, copy
//! the result to the output tap and push telemetry.

use crate::audio::spectral_profile::BARK_BANDS;
use crate::audio::turbulence::AirMotion;
#[cfg(feature = "memprof")]
use crate::engine::memprof::MemStage;
use crate::engine::telemetry::compute_telemetry;
use crate::pipeline::perceptual::SourcePerceptualState;
use crate::pipeline::{render_pipeline, RenderParams};
use crate::profile_span;
use atrium_core::directivity::directivity_gain;
use atrium_core::panner::distance_gain_at_model;
use atrium_core::source::EmitterKind;

use super::{AudioScene, PAUSE_FADE_SECONDS};

impl AudioScene {
    /// Fade `output` toward silence while pausing and back while playing,
    /// a frame at a time.
    fn fade_for_pause(&mut self, output: &mut [f32], channels: usize) {
        let target = if self.paused { 0.0 } else { 1.0 };
        if self.pause_gain == target {
            return;
        }
        let step = 1.0 / (PAUSE_FADE_SECONDS * self.sample_rate.max(1.0));
        for frame in output.chunks_mut(channels.max(1)) {
            self.pause_gain = if target > self.pause_gain {
                (self.pause_gain + step).min(target)
            } else {
                (self.pause_gain - step).max(target)
            };
            for sample in frame {
                *sample *= self.pause_gain;
            }
        }
    }

    /// Render one buffer of audio.
    /// `output` is an interleaved sample buffer (e.g. [L, R, L, R, ...] for stereo).
    pub fn render(&mut self, output: &mut [f32], channels: usize) {
        let _total =
            profile_span!("render", sources = self.sources.len(), channels = channels).entered();

        #[cfg(feature = "memprof")]
        self.memprof.begin_callback();

        // Apply any live source-pool edits before rendering this buffer.
        self.process_scene_edits();

        let num_frames = output.len() / channels;
        let dt = num_frames as f32 / self.sample_rate;

        // Paused and faded out: nothing moves, not even the clock.
        if self.paused && self.pause_gain <= 0.0 {
            output.fill(0.0);
            return;
        }

        // Hand every voice what the plan made due, and the time it is now.
        let now = self.clock.samples();
        if let Some(plan) = &mut self.plan_in {
            while let Ok(scheduled) = plan.pop() {
                match scheduled {
                    crate::plan::Scheduled::Utter { slot, utterance } => {
                        if let Some(source) = self.sources.get_mut(slot as usize) {
                            source.schedule(utterance);
                        }
                    }
                    crate::plan::Scheduled::Cancel { slot } => {
                        if let Some(source) = self.sources.get_mut(slot as usize) {
                            source.cancel();
                        }
                    }
                    crate::plan::Scheduled::Set { slot, param, value } => {
                        if let Some(source) = self.sources.get_mut(slot as usize) {
                            source.set_synth_param(param, value);
                        }
                    }
                    crate::plan::Scheduled::Air {
                        wind,
                        daylight,
                        wind_from,
                    } => {
                        self.environment.air = AirMotion::from_weather(wind, daylight, wind_from);
                    }
                }
            }
        }
        for source in &mut self.sources {
            source.set_clock(now);
        }

        // Advance time-varying state on all sources
        {
            let _s = profile_span!("source_tick").entered();
            for source in &mut self.sources {
                source.tick(dt);
            }
        }
        #[cfg(feature = "memprof")]
        self.memprof.record_stage(MemStage::SourceTick);

        // Perceptual masking analysis (feed-forward, before rendering).
        {
            let _s = profile_span!("perceptual").entered();
            self.perceptual_states.clear();
            for (i, source) in self.sources.iter().enumerate() {
                let pos = source.position();
                let active = source.is_active() && !source.is_muted();
                let amp = if active && i < self.source_amplitudes.len() {
                    if source.emitter_kind() == EmitterKind::Field {
                        self.source_amplitudes[i]
                    } else {
                        let dist_gain = distance_gain_at_model(
                            self.listener.position,
                            pos,
                            source.ref_distance(),
                            self.distance_model.max_distance,
                            self.distance_model.rolloff,
                            self.distance_model.model,
                        );
                        let emit_gain = directivity_gain(
                            pos,
                            source.orientation(),
                            self.listener.position,
                            &source.directivity(),
                        );
                        self.source_amplitudes[i] * dist_gain * emit_gain
                    }
                } else {
                    0.0
                };
                let bands = if i < self.spectral_profiles.len() {
                    self.spectral_profiles[i]
                } else {
                    [0.0; BARK_BANDS]
                };
                self.perceptual_states.push(SourcePerceptualState {
                    received_amplitude: amp,
                    spectral_bands: bands,
                    active,
                });
            }
            self.perceptual_layer.update(&self.perceptual_states);
        }

        // Render through the composable pipeline
        {
            let pipeline = &mut self.pipelines[self.active_pipeline.index()];
            let _s = profile_span!("pipeline", mode = ?self.active_pipeline).entered();
            let params = RenderParams {
                listener: &self.listener,
                channels,
                sample_rate: self.sample_rate,
                master_gain: self.master_gain,
                distance_model: &self.distance_model,
                layout: &self.speaker_layout,
                environment: &self.environment,
                measurement_mode: self.measurement_mode,
            };
            render_pipeline(pipeline, &mut self.sources, &params, output);
        }
        // One loudspeaker or one ear alone: the others are silenced after
        // everything else, so what is heard from it is what it plays.
        if let Some(only) = self.probe.only_channel {
            for frame in output.chunks_exact_mut(channels) {
                for (channel, sample) in frame.iter_mut().enumerate() {
                    if channel != only as usize {
                        *sample = 0.0;
                    }
                }
            }
        }
        self.clock.advance(num_frames as u64, self.sample_rate);
        self.fade_for_pause(output, channels);
        // What is heard, as it is heard: after the probe and the fade, stamped
        // with the clock at the block's start.
        if let Some(tap) = &mut self.output_tap {
            tap.push(output, channels, now);
        }
        #[cfg(feature = "memprof")]
        self.memprof.record_stage(MemStage::Mix);

        // Push telemetry at ~15 Hz (every N callbacks)
        {
            let _s = profile_span!("telemetry").entered();
            self.telemetry_counter += 1;
            if self.telemetry_counter >= self.telemetry_interval {
                self.telemetry_counter = 0;
                if let Some(ref mut producer) = self.telemetry_out {
                    let mut frame =
                        compute_telemetry(&self.sources, &self.listener, &self.distance_model);
                    frame.render_mode = self.active_pipeline;
                    frame.channel_mode = self.active_channel_mode;
                    frame.temperature_c = self.environment.atmosphere.temperature_c;
                    frame.humidity_pct = self.environment.atmosphere.humidity_pct;
                    frame.channel_peaks =
                        crate::engine::telemetry::compute_channel_peaks(output, channels);
                    frame.channel_count = channels as u8;
                    frame.master_gain = self.master_gain;
                    // Stamp perceptual scores from the latest analysis.
                    let scores = self.perceptual_layer.scores();
                    for i in 0..frame.source_count as usize {
                        if i < scores.len() {
                            frame.sources[i].perceptual_score = scores[i];
                        }
                    }
                    let _ = producer.push(frame); // silent drop if full
                }
            }
        }
        #[cfg(feature = "memprof")]
        self.memprof.record_stage(MemStage::Telemetry);

        #[cfg(feature = "memprof")]
        self.memprof.finish_callback();
    }
}
