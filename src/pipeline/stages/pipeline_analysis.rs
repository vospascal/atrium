//! Per-stage pipeline analysis for diagnosing audio quality issues.
//!
//! Captures buffer snapshots before and after each MixStage, computing
//! diagnostic metrics at each step: RMS energy, peak amplitude, crest factor,
//! and per-band spectral energy using Goertzel's algorithm at octave frequencies.
//!
//! Run with `cargo test pipeline_analysis -- --nocapture` to see full diagnostic output.

#[cfg(test)]
mod tests {
    use crate::audio::atmosphere::AtmosphericParams;
    use crate::audio::distance::DistanceModel;
    use crate::audio::propagation::GroundProperties;
    use crate::pipeline::mix_stage::{MixContext, MixStage};
    use crate::pipeline::path::WallMaterial;
    use crate::pipeline::stages::fdn_reverb::FdnReverbStage;
    use crate::pipeline::{
        build_ambisonics, build_dbap, build_hrtf, build_vbap, render_pipeline, PipelineParams,
        RenderParams, RenderPipeline,
    };
    use atrium_core::listener::Listener;
    use atrium_core::source::SoundSource;
    use atrium_core::speaker::SpeakerLayout;
    use atrium_core::types::Vec3;

    // ── Goertzel spectral analysis ──────────────────────────────────────────

    /// Octave band center frequencies for analysis.
    const OCTAVE_BANDS: [f32; 7] = [125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0];

    /// Goertzel's algorithm: compute magnitude at a single frequency.
    /// More efficient than FFT when we only need a few frequency bins.
    fn goertzel_magnitude(samples: &[f32], target_freq: f32, sample_rate: f32) -> f32 {
        let n = samples.len();
        if n == 0 {
            return 0.0;
        }
        let k = (target_freq * n as f32 / sample_rate).round();
        let w = 2.0 * std::f32::consts::PI * k / n as f32;
        let coeff = 2.0 * w.cos();

        let mut s1 = 0.0f32;
        let mut s2 = 0.0f32;

        for &sample in samples {
            let s0 = sample + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }

        let power = s1 * s1 + s2 * s2 - coeff * s1 * s2;
        (power.max(0.0) / (n as f32)).sqrt()
    }

    // ── Snapshot and metrics ────────────────────────────────────────────────

    /// Diagnostic metrics for a buffer snapshot.
    #[derive(Clone)]
    struct StageMetrics {
        name: String,
        per_channel_rms: Vec<f32>,
        per_channel_peak: Vec<f32>,
        per_channel_crest_factor: Vec<f32>,
        /// Spectral energy at each octave band (averaged across active channels).
        spectral_db: Vec<f32>,
        /// Total sample count that is exactly zero.
        zero_sample_count: usize,
        total_samples: usize,
    }

    impl std::fmt::Display for StageMetrics {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            writeln!(f, "  Stage: {}", self.name)?;
            for (ch, ((rms, peak), crest)) in self
                .per_channel_rms
                .iter()
                .zip(&self.per_channel_peak)
                .zip(&self.per_channel_crest_factor)
                .enumerate()
            {
                let rms_db = if *rms > 0.0 {
                    20.0 * rms.log10()
                } else {
                    -120.0
                };
                let peak_db = if *peak > 0.0 {
                    20.0 * peak.log10()
                } else {
                    -120.0
                };
                writeln!(
                    f,
                    "    ch{ch}: RMS={rms:.6} ({rms_db:+.1}dB)  peak={peak:.6} ({peak_db:+.1}dB)  crest={crest:.1}dB"
                )?;
            }
            write!(f, "    spectrum:")?;
            for (i, &db) in self.spectral_db.iter().enumerate() {
                write!(f, " {}Hz={:+.1}dB", OCTAVE_BANDS[i] as u32, db)?;
            }
            writeln!(f)?;
            let zero_pct = self.zero_sample_count as f32 / self.total_samples.max(1) as f32 * 100.0;
            writeln!(
                f,
                "    zeros: {}/{} ({:.1}%)",
                self.zero_sample_count, self.total_samples, zero_pct
            )?;
            Ok(())
        }
    }

    /// Compute metrics for a buffer snapshot.
    fn compute_metrics(
        name: &str,
        buffer: &[f32],
        channels: usize,
        sample_rate: f32,
    ) -> StageMetrics {
        let num_frames = buffer.len() / channels;
        let mut per_channel_rms = vec![0.0f32; channels];
        let mut per_channel_peak = vec![0.0f32; channels];
        let mut zero_count = 0usize;

        for frame in 0..num_frames {
            for ch in 0..channels {
                let sample = buffer[frame * channels + ch];
                per_channel_rms[ch] += sample * sample;
                per_channel_peak[ch] = per_channel_peak[ch].max(sample.abs());
                if sample == 0.0 {
                    zero_count += 1;
                }
            }
        }

        for rms in per_channel_rms.iter_mut() {
            *rms = (*rms / num_frames as f32).sqrt();
        }

        let per_channel_crest_factor: Vec<f32> = per_channel_rms
            .iter()
            .zip(&per_channel_peak)
            .map(|(rms, peak)| {
                if *rms > 0.0 {
                    20.0 * (peak / rms).log10()
                } else {
                    0.0
                }
            })
            .collect();

        // Spectral analysis: extract mono signal for Goertzel (average active channels).
        let mut mono = vec![0.0f32; num_frames];
        let mut active_channels = 0u32;
        for ch in 0..channels {
            if per_channel_peak[ch] > 0.0 {
                active_channels += 1;
                for frame in 0..num_frames {
                    mono[frame] += buffer[frame * channels + ch];
                }
            }
        }
        if active_channels > 0 {
            let scale = 1.0 / active_channels as f32;
            for sample in &mut mono {
                *sample *= scale;
            }
        }

        let spectral_db: Vec<f32> = OCTAVE_BANDS
            .iter()
            .map(|&freq| {
                let mag = goertzel_magnitude(&mono, freq, sample_rate);
                if mag > 0.0 {
                    20.0 * mag.log10()
                } else {
                    -120.0
                }
            })
            .collect();

        StageMetrics {
            name: name.to_string(),
            per_channel_rms,
            per_channel_peak,
            per_channel_crest_factor,
            spectral_db,
            zero_sample_count: zero_count,
            total_samples: buffer.len(),
        }
    }

    /// Compute per-stage delta: how much each stage changed the signal.
    fn compute_delta(before: &StageMetrics, after: &StageMetrics) -> String {
        let mut result = String::new();
        for ch in 0..before
            .per_channel_rms
            .len()
            .min(after.per_channel_rms.len())
        {
            let rms_before = before.per_channel_rms[ch];
            let rms_after = after.per_channel_rms[ch];
            let delta_db = if rms_before > 0.0 && rms_after > 0.0 {
                20.0 * (rms_after / rms_before).log10()
            } else if rms_after > 0.0 {
                f32::INFINITY
            } else if rms_before > 0.0 {
                f32::NEG_INFINITY
            } else {
                0.0
            };
            result.push_str(&format!("    ch{ch}: Δ={delta_db:+.2}dB\n"));
        }
        // Spectral deltas
        result.push_str("    spectrum Δ:");
        for (i, &freq) in OCTAVE_BANDS.iter().enumerate() {
            let delta = after.spectral_db[i] - before.spectral_db[i];
            result.push_str(&format!(" {}Hz={:+.1}dB", freq as u32, delta));
        }
        result.push('\n');
        result
    }

    // ── Test infrastructure ─────────────────────────────────────────────────

    /// Constant-value test source.
    struct ConstSource {
        pos: Vec3,
        val: f32,
    }

    impl SoundSource for ConstSource {
        fn next_sample(&mut self, _sample_rate: f32) -> f32 {
            self.val
        }
        fn position(&self) -> Vec3 {
            self.pos
        }
        fn tick(&mut self, _dt: f32) {}
        fn is_active(&self) -> bool {
            true
        }
    }

    /// Impulse source: emits one sample then silence.
    struct ImpulseSource {
        pos: Vec3,
        fired: bool,
    }

    impl SoundSource for ImpulseSource {
        fn next_sample(&mut self, _sample_rate: f32) -> f32 {
            if self.fired {
                0.0
            } else {
                self.fired = true;
                1.0
            }
        }
        fn position(&self) -> Vec3 {
            self.pos
        }
        fn tick(&mut self, _dt: f32) {}
        fn is_active(&self) -> bool {
            true
        }
    }

    fn default_atmosphere() -> AtmosphericParams {
        AtmosphericParams {
            temperature_c: 20.0,
            humidity_pct: 50.0,
            pressure_kpa: 101.325,
        }
    }

    fn default_ground() -> GroundProperties {
        GroundProperties::default()
    }

    fn default_distance_model() -> DistanceModel {
        DistanceModel::default()
    }

    fn default_wall_materials() -> [WallMaterial; 6] {
        [WallMaterial::HARD_WALL; 6]
    }

    fn layout_5_1() -> SpeakerLayout {
        SpeakerLayout::surround_5_1(
            Vec3::new(0.0, 4.0, 0.0),
            Vec3::new(6.0, 4.0, 0.0),
            Vec3::new(3.0, 4.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(6.0, 0.0, 0.0),
        )
    }

    fn layout_stereo() -> SpeakerLayout {
        SpeakerLayout::stereo(Vec3::new(-1.0, 0.0, 1.0), Vec3::new(1.0, 0.0, 1.0))
    }

    // ── Core analysis function ──────────────────────────────────────────────

    /// Run the pipeline and capture per-stage metrics.
    ///
    /// Strategy: Run render_pipeline once to get the post-render buffer (before
    /// mix stages). Then run each mix stage individually, snapshotting between.
    /// Also captures the reverb send buffer and FDN internal state.
    fn analyze_pipeline(
        mode_name: &str,
        pipeline: &mut RenderPipeline,
        sources: &mut [Box<dyn SoundSource>],
        render_params: &RenderParams,
        init_ctx: &MixContext,
    ) -> Vec<StageMetrics> {
        let channels = render_params.channels;
        let frames = 4096;
        let sample_rate = render_params.sample_rate;

        // Init stages
        pipeline.init(init_ctx);
        pipeline.ensure_topology(sources.len(), render_params.layout, sample_rate);

        // Run two passes of the full pipeline to let state settle (gain ramps, filters).
        let mut warmup_buffer = vec![0.0f32; frames * channels];
        render_pipeline(pipeline, sources, render_params, &mut warmup_buffer);
        let mut warmup_buffer2 = vec![0.0f32; frames * channels];
        render_pipeline(pipeline, sources, render_params, &mut warmup_buffer2);

        // Now capture a diagnostic pass. We need to intercept between mix stages.
        // Temporarily swap out mix_stages, render without them, then run each manually.
        let mut mix_stages: Vec<Box<dyn MixStage>> = std::mem::take(&mut pipeline.mix_stages);

        // Render source + renderer portion only (no mix stages).
        // This populates pipeline.reverb_send_buffer via the renderer.
        let mut buffer = vec![0.0f32; frames * channels];
        render_pipeline(pipeline, sources, render_params, &mut buffer);

        // Capture the reverb send buffer that render_pipeline populated.
        let reverb_send_snapshot = pipeline.reverb_send_buffer.clone();

        let mut snapshots = Vec::new();

        // Snapshot: post-render, pre-mix
        snapshots.push(compute_metrics(
            "post_render",
            &buffer,
            channels,
            sample_rate,
        ));

        // Build MixContext for manual stage processing.
        // CRITICAL: pass the actual reverb_send_buffer as reverb_input,
        // just like the real pipeline does. Without this, FDN falls back to
        // mono-summing the dry buffer, giving misleadingly high reverb levels.
        let effective_render_channels = if pipeline.render_channels > 0 {
            pipeline.render_channels
        } else {
            render_params.layout.total_channels()
        };
        let mix_ctx = MixContext {
            listener: render_params.listener,
            layout: render_params.layout,
            sample_rate,
            channels,
            environment_min: render_params.environment_min,
            environment_max: render_params.environment_max,
            master_gain: render_params.master_gain,
            render_channels: effective_render_channels,
            reverb_input: Some(&reverb_send_snapshot),
            wall_reflectivity: init_ctx.wall_reflectivity,
            wall_materials: init_ctx.wall_materials,
            atmosphere: init_ctx.atmosphere,
            measurement_mode: render_params.measurement_mode,
        };

        // Snapshot buffer before each stage for LFE reconstruction check
        let mut pre_lfe_buffer: Option<Vec<f32>> = None;

        // Run each mix stage individually, snapshotting after each
        for stage in mix_stages.iter_mut() {
            let stage_name = stage.name().to_string();

            // Save pre-LFE buffer for reconstruction analysis
            if stage_name == "lfe_bass_management" {
                pre_lfe_buffer = Some(buffer.clone());
            }

            stage.process(&mut buffer, &mix_ctx);
            snapshots.push(compute_metrics(&stage_name, &buffer, channels, sample_rate));
        }

        // Restore stages
        pipeline.mix_stages = mix_stages;

        // ── Print report ────────────────────────────────────────────────────

        println!("\n{}", "=".repeat(60));
        println!("Pipeline Analysis: {mode_name}");
        println!("  channels={channels}  frames={frames}  sample_rate={sample_rate}");
        println!("{}", "=".repeat(60));

        // Reverb send buffer analysis
        let reverb_send_metrics =
            compute_metrics("reverb_send", &reverb_send_snapshot, channels, sample_rate);
        println!("  REVERB SEND BUFFER (FDN input):");
        println!("{reverb_send_metrics}");

        // FDN internal state: create a fresh FDN with the same context
        // to read its computed parameters (avoids unsafe downcasting).
        {
            let mut probe_fdn = FdnReverbStage::new();
            probe_fdn.init(init_ctx);
            let diag = probe_fdn.diagnostics();
            println!("  FDN INTERNAL STATE:");
            println!("    output_normalization: {:.6}", diag.output_normalization);
            println!("    pre_delay_samples: {}", diag.pre_delay_samples);
            println!("    buffer_size: {}", diag.buffer_size);
            println!("    delays: {:?}", diag.delays);
            print!("    loop_gains:");
            for (line_idx, g) in diag.loop_gains.iter().enumerate() {
                print!(" L{line_idx}={g:.4}");
            }
            println!();
            let avg_gain: f32 = diag.loop_gains.iter().sum::<f32>() / diag.loop_gains.len() as f32;
            println!("    avg_loop_gain: {avg_gain:.4}");
            let norm_db = if diag.output_normalization > 0.0 {
                20.0 * diag.output_normalization.log10()
            } else {
                -120.0
            };
            println!("    output_normalization_db: {norm_db:+.1}dB");
            println!();
        }

        // Per-stage snapshots with deltas
        for (i, snapshot) in snapshots.iter().enumerate() {
            println!("{snapshot}");
            if i > 0 {
                let delta = compute_delta(&snapshots[i - 1], snapshot);
                println!("  Delta from previous stage:");
                print!("{delta}");
            }
        }

        // LFE reconstruction check
        if let Some(pre_lfe) = &pre_lfe_buffer {
            println!("  LFE RECONSTRUCTION CHECK:");
            let lfe_ch = render_params.layout.lfe_channel().unwrap_or(channels + 1);

            // Check total energy conservation: Σ(all channels post-LFE) ≈ Σ(all channels pre-LFE).
            // Find the post-LFE snapshot (the one named "lfe_bass_management")
            let post_lfe_snapshot = snapshots.iter().find(|s| s.name == "lfe_bass_management");

            if let Some(post_lfe) = post_lfe_snapshot {
                // Also compute post-LFE total energy from the buffer at that point.
                // We don't have the exact buffer, but we can check per-channel RMS.
                let post_total_rms_sq: f32 = post_lfe.per_channel_rms.iter().map(|r| r * r).sum();
                let pre_total_rms_sq: f32 = {
                    let pre_metrics = compute_metrics("pre_lfe", pre_lfe, channels, sample_rate);
                    pre_metrics.per_channel_rms.iter().map(|r| r * r).sum()
                };

                let energy_ratio_db = if pre_total_rms_sq > 0.0 {
                    10.0 * (post_total_rms_sq / pre_total_rms_sq).log10()
                } else {
                    0.0
                };
                println!("    Total energy change across LFE crossover: {energy_ratio_db:+.2}dB");
                // N main channels' bass coherently summed to 1 LFE channel gives
                // +10*log10(N) dB energy gain. This is correct bass management behavior.
                let main_ch_count = channels.saturating_sub(1).max(1); // all channels minus LFE
                let expected_coherent_db = 10.0 * (main_ch_count as f32).log10();
                let deviation = (energy_ratio_db - expected_coherent_db).abs();
                if deviation > 3.0 {
                    println!(
                        "    ⚠ WARNING: Energy change ({energy_ratio_db:+.1}dB) deviates from expected coherent sum ({expected_coherent_db:+.1}dB for {main_ch_count} channels) by {deviation:.1}dB"
                    );
                } else {
                    println!(
                        "    ✓ Energy change ({energy_ratio_db:+.1}dB) consistent with {main_ch_count}-channel coherent sum ({expected_coherent_db:+.1}dB ± 3dB)"
                    );
                }

                // Per-channel detail
                let pre_metrics = compute_metrics("pre_lfe", pre_lfe, channels, sample_rate);
                for ch in 0..channels {
                    let pre_rms = pre_metrics.per_channel_rms[ch];
                    let post_rms = post_lfe.per_channel_rms[ch];
                    let ch_delta = if pre_rms > 0.0 && post_rms > 0.0 {
                        20.0 * (post_rms / pre_rms).log10()
                    } else if post_rms > 0.0 {
                        f32::INFINITY
                    } else {
                        f32::NEG_INFINITY
                    };
                    let label = if ch == lfe_ch { " (LFE)" } else { "" };
                    println!("    ch{ch}{label}: {pre_rms:.6} → {post_rms:.6} ({ch_delta:+.1}dB)");
                }
            }
            println!();
        }

        // Wet/dry ratio analysis
        println!("  WET/DRY RATIO:");
        if snapshots.len() >= 2 {
            let dry = &snapshots[0]; // post_render = dry signal
                                     // Find FDN stage
            let fdn_idx = snapshots.iter().position(|s| s.name == "fdn_reverb");
            if let Some(idx) = fdn_idx {
                let post_fdn = &snapshots[idx];
                for ch in 0..channels {
                    let dry_rms = dry.per_channel_rms[ch];
                    let total_rms = post_fdn.per_channel_rms[ch];
                    // wet² = total² - dry² (energy addition for uncorrelated signals)
                    let wet_rms_sq = (total_rms * total_rms - dry_rms * dry_rms).max(0.0);
                    let wet_rms = wet_rms_sq.sqrt();
                    let ratio_db = if dry_rms > 1e-10 && wet_rms > 1e-10 {
                        20.0 * (wet_rms / dry_rms).log10()
                    } else if wet_rms > 1e-10 {
                        f32::INFINITY
                    } else {
                        f32::NEG_INFINITY
                    };
                    println!(
                        "    ch{ch}: dry={dry_rms:.6}  wet≈{wet_rms:.6}  wet/dry={ratio_db:+.1}dB"
                    );
                }
            }
        }
        println!();

        snapshots
    }

    // ── Per-mode analysis tests ─────────────────────────────────────────────

    fn standard_room() -> (Vec3, Vec3) {
        // 6×4×3m room (small to medium listening room)
        (Vec3::new(0.0, 0.0, 0.0), Vec3::new(6.0, 4.0, 3.0))
    }

    #[test]
    fn analyze_vbap_pipeline() {
        let layout = layout_5_1();
        let atm = default_atmosphere();
        let ground = default_ground();
        let distance_model = default_distance_model();
        let wall_materials = default_wall_materials();
        let (environment_min, environment_max) = standard_room();
        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), 0.0);

        let params = PipelineParams {
            er_wall_reflectivity: 0.9,
            wall_materials: wall_materials.clone(),
            environment_min,
            environment_max,
            ..PipelineParams::default()
        };
        let mut pipeline = build_vbap(&params);

        let channels = layout.total_channels();
        let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
            pos: Vec3::new(5.0, 3.0, 0.0),
            val: 0.5,
        })];

        let render_params = RenderParams {
            listener: &listener,
            channels,
            sample_rate: 48000.0,
            master_gain: 1.0,
            distance_model: &distance_model,
            layout: &layout,
            atmosphere: &atm,
            ground: &ground,
            environment_min,
            environment_max,
            barriers: &[],
            wall_materials: &wall_materials,
            measurement_mode: true, // linear signal for accurate analysis
        };

        let init_ctx = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels,
            environment_min,
            environment_max,
            master_gain: 1.0,
            render_channels: channels,
            reverb_input: None,
            wall_reflectivity: 0.9,
            wall_materials: &wall_materials,
            atmosphere: &atm,
            measurement_mode: true,
        };

        let snapshots = analyze_pipeline(
            "VBAP 5.1",
            &mut pipeline,
            &mut sources,
            &render_params,
            &init_ctx,
        );

        // Verify FDN doesn't amplify more than 20dB over input
        let post_render_rms = snapshots[0]
            .per_channel_rms
            .iter()
            .cloned()
            .fold(0.0f32, f32::max);
        let post_fdn_rms = snapshots[1]
            .per_channel_rms
            .iter()
            .cloned()
            .fold(0.0f32, f32::max);
        if post_render_rms > 1e-10 {
            let fdn_gain_db = 20.0 * (post_fdn_rms / post_render_rms).log10();
            assert!(
                fdn_gain_db < 20.0,
                "FDN should not amplify by more than 20dB, got {fdn_gain_db:.1}dB"
            );
        }
    }

    #[test]
    fn analyze_hrtf_pipeline() {
        let layout = layout_stereo();
        let atm = default_atmosphere();
        let ground = default_ground();
        let distance_model = default_distance_model();
        let wall_materials = default_wall_materials();
        let (environment_min, environment_max) = standard_room();
        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), 0.0);

        let params = PipelineParams {
            er_wall_reflectivity: 0.9,
            wall_materials: wall_materials.clone(),
            environment_min,
            environment_max,
            ..PipelineParams::default()
        };
        let mut pipeline = build_hrtf(&params);

        let channels = layout.total_channels();
        let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
            pos: Vec3::new(5.0, 3.0, 0.0),
            val: 0.5,
        })];

        let render_params = RenderParams {
            listener: &listener,
            channels,
            sample_rate: 48000.0,
            master_gain: 1.0,
            distance_model: &distance_model,
            layout: &layout,
            atmosphere: &atm,
            ground: &ground,
            environment_min,
            environment_max,
            barriers: &[],
            wall_materials: &wall_materials,
            measurement_mode: true,
        };

        let init_ctx = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels,
            environment_min,
            environment_max,
            master_gain: 1.0,
            render_channels: 2,
            reverb_input: None,
            wall_reflectivity: 0.9,
            wall_materials: &wall_materials,
            atmosphere: &atm,
            measurement_mode: true,
        };

        analyze_pipeline(
            "HRTF stereo",
            &mut pipeline,
            &mut sources,
            &render_params,
            &init_ctx,
        );
    }

    #[test]
    fn analyze_dbap_pipeline() {
        let layout = layout_5_1();
        let atm = default_atmosphere();
        let ground = default_ground();
        let distance_model = default_distance_model();
        let wall_materials = default_wall_materials();
        let (environment_min, environment_max) = standard_room();
        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), 0.0);

        let params = PipelineParams {
            er_wall_reflectivity: 0.9,
            wall_materials: wall_materials.clone(),
            environment_min,
            environment_max,
            ..PipelineParams::default()
        };
        let mut pipeline = build_dbap(&params);

        let channels = layout.total_channels();
        let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
            pos: Vec3::new(5.0, 3.0, 0.0),
            val: 0.5,
        })];

        let render_params = RenderParams {
            listener: &listener,
            channels,
            sample_rate: 48000.0,
            master_gain: 1.0,
            distance_model: &distance_model,
            layout: &layout,
            atmosphere: &atm,
            ground: &ground,
            environment_min,
            environment_max,
            barriers: &[],
            wall_materials: &wall_materials,
            measurement_mode: true,
        };

        let init_ctx = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels,
            environment_min,
            environment_max,
            master_gain: 1.0,
            render_channels: channels,
            reverb_input: None,
            wall_reflectivity: 0.9,
            wall_materials: &wall_materials,
            atmosphere: &atm,
            measurement_mode: true,
        };

        analyze_pipeline(
            "DBAP 5.1",
            &mut pipeline,
            &mut sources,
            &render_params,
            &init_ctx,
        );
    }

    #[test]
    fn analyze_ambisonics_pipeline() {
        let layout = layout_5_1();
        let atm = default_atmosphere();
        let ground = default_ground();
        let distance_model = default_distance_model();
        let wall_materials = default_wall_materials();
        let (environment_min, environment_max) = standard_room();
        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), 0.0);

        let params = PipelineParams {
            er_wall_reflectivity: 0.9,
            wall_materials: wall_materials.clone(),
            environment_min,
            environment_max,
            ..PipelineParams::default()
        };
        let mut pipeline = build_ambisonics(&params);

        let channels = layout.total_channels();
        let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
            pos: Vec3::new(5.0, 3.0, 0.0),
            val: 0.5,
        })];

        let render_params = RenderParams {
            listener: &listener,
            channels,
            sample_rate: 48000.0,
            master_gain: 1.0,
            distance_model: &distance_model,
            layout: &layout,
            atmosphere: &atm,
            ground: &ground,
            environment_min,
            environment_max,
            barriers: &[],
            wall_materials: &wall_materials,
            measurement_mode: true,
        };

        let init_ctx = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels,
            environment_min,
            environment_max,
            master_gain: 1.0,
            render_channels: channels,
            reverb_input: None,
            wall_reflectivity: 0.9,
            wall_materials: &wall_materials,
            atmosphere: &atm,
            measurement_mode: true,
        };

        analyze_pipeline(
            "Ambisonics 5.1",
            &mut pipeline,
            &mut sources,
            &render_params,
            &init_ctx,
        );
    }

    // ── Impulse response analysis ───────────────────────────────────────────

    /// Analyze the reverb tail decay by feeding an impulse and measuring
    /// energy over time windows. This directly reveals if the FDN tail
    /// decays properly or rings/sustains too long.
    #[test]
    fn analyze_vbap_impulse_response() {
        let layout = layout_5_1();
        let atm = default_atmosphere();
        let ground = default_ground();
        let distance_model = default_distance_model();
        let wall_materials = default_wall_materials();
        let (environment_min, environment_max) = standard_room();
        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), 0.0);

        let params = PipelineParams {
            er_wall_reflectivity: 0.9,
            wall_materials: wall_materials.clone(),
            environment_min,
            environment_max,
            ..PipelineParams::default()
        };
        let mut pipeline = build_vbap(&params);

        let channels = layout.total_channels();

        let init_ctx = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels,
            environment_min,
            environment_max,
            master_gain: 1.0,
            render_channels: channels,
            reverb_input: None,
            wall_reflectivity: 0.9,
            wall_materials: &wall_materials,
            atmosphere: &atm,
            measurement_mode: true,
        };
        pipeline.init(&init_ctx);
        pipeline.ensure_topology(1, &layout, 48000.0);

        let render_params = RenderParams {
            listener: &listener,
            channels,
            sample_rate: 48000.0,
            master_gain: 1.0,
            distance_model: &distance_model,
            layout: &layout,
            atmosphere: &atm,
            ground: &ground,
            environment_min,
            environment_max,
            barriers: &[],
            wall_materials: &wall_materials,
            measurement_mode: true,
        };

        // Render impulse (first buffer)
        let frames_per_buffer = 4096;
        let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ImpulseSource {
            pos: Vec3::new(5.0, 3.0, 0.0),
            fired: false,
        })];
        let mut buffer = vec![0.0f32; frames_per_buffer * channels];
        render_pipeline(&mut pipeline, &mut sources, &render_params, &mut buffer);

        // Render several more buffers of silence to capture the tail
        let num_tail_buffers = 10;
        let mut tail_buffers = Vec::new();
        for _ in 0..num_tail_buffers {
            let mut tail = vec![0.0f32; frames_per_buffer * channels];
            render_pipeline(&mut pipeline, &mut sources, &render_params, &mut tail);
            tail_buffers.push(tail);
        }

        println!("\n{}", "=".repeat(60));
        println!("Impulse Response Analysis: VBAP 5.1");
        println!("  room: 6×4×3m, hard walls, wall_reflectivity=0.9");
        println!("{}", "=".repeat(60));

        // Measure RMS per time window
        let measure_rms = |buf: &[f32]| -> f32 {
            let mut sum_sq = 0.0f32;
            let mut count = 0;
            for frame in 0..frames_per_buffer {
                for ch in 0..channels {
                    let s = buf[frame * channels + ch];
                    sum_sq += s * s;
                    count += 1;
                }
            }
            (sum_sq / count as f32).sqrt()
        };

        let impulse_rms = measure_rms(&buffer);
        let impulse_rms_db = if impulse_rms > 0.0 {
            20.0 * impulse_rms.log10()
        } else {
            -120.0
        };
        println!("  Buffer 0 (impulse): RMS={impulse_rms:.6} ({impulse_rms_db:+.1}dB)");

        let mut prev_rms_db = impulse_rms_db;
        for (i, tail) in tail_buffers.iter().enumerate() {
            let rms = measure_rms(tail);
            let rms_db = if rms > 0.0 {
                20.0 * rms.log10()
            } else {
                -120.0
            };
            let decay_rate = rms_db - prev_rms_db;
            let time_ms = ((i + 1) * frames_per_buffer) as f32 / 48000.0 * 1000.0;
            println!(
                "  Buffer {} ({:.0}ms): RMS={:.6} ({:+.1}dB)  decay={:+.1}dB/buffer",
                i + 1,
                time_ms,
                rms,
                rms_db,
                decay_rate
            );
            prev_rms_db = rms_db;

            // Spectral analysis of tail
            let mut mono = vec![0.0f32; frames_per_buffer];
            for frame in 0..frames_per_buffer {
                let mut sum = 0.0f32;
                for ch in 0..channels.min(5) {
                    sum += tail[frame * channels + ch];
                }
                mono[frame] = sum / channels.min(5) as f32;
            }
            print!("    spectrum:");
            for &freq in &OCTAVE_BANDS {
                let mag = goertzel_magnitude(&mono, freq, 48000.0);
                let db = if mag > 0.0 {
                    20.0 * mag.log10()
                } else {
                    -120.0
                };
                print!(" {}Hz={:+.1}dB", freq as u32, db);
            }
            println!();
        }

        // The tail should decay — if buffer 5 (427ms) is still within 6dB of
        // the impulse, something is wrong (expected RT60 for this room is ~0.3-0.5s).
        let tail5_rms = measure_rms(&tail_buffers[4]);
        if impulse_rms > 1e-10 {
            let decay_at_427ms = 20.0 * (tail5_rms / impulse_rms).log10();
            println!("\n  Decay at ~427ms: {decay_at_427ms:+.1}dB from impulse");
            // In a 6×4×3m room with hard walls (RT60 ~0.3-0.5s), we expect
            // at least 20dB decay by 427ms. Flag if less.
            if decay_at_427ms > -10.0 {
                println!("  ⚠ WARNING: Reverb tail is decaying slowly — possible metallic ringing");
            }
        }
    }
}
