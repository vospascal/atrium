// Procedural ocean wave synthesis.
//
// Port of WaveNoiseProcessor.ts. Algorithm:
//   • Brown (integrated white) noise  → deep low-frequency roar
//   • Pink (1/f) noise               → broad-band hiss / wash
//   • Sine-LFO swell envelope        → slow rhythmic surging
//   • Stochastic crash component     → Gaussian-gated, lowpassed white
//                                       noise when breaker events fire

use std::f32::consts::TAU;

use atrium_core::commands::SynthParam;
use atrium_core::layers::LayerMix;
use atrium_core::source::SoundSource;
use atrium_core::types::Vec3;

use super::noise::{white_noise_density_scale, BrownNoise, OnePoleLP, PinkNoise, Rng};

/// Corner of the breaker-wash lowpass on the crash white noise.
const CRASH_LOWPASS_HZ: f32 = 1_400.0;

/// Procedural ocean wave sound source.
///
/// The swell follows a slow sine LFO whose period matches real ocean wave
/// timings (3–10 s typical). Stochastic "crash" events layer white-noise
/// bursts with an early-peaked Gaussian window for a breaking-wave feel.
pub struct WaveSource {
    pink: PinkNoise,
    brown: BrownNoise,

    /// Running sample counter for swell LFO.
    t: u64,

    // Crash burst (computed inline — no allocation)
    crash_pos: usize,
    crash_len: usize,
    // Lowpass on the crash white-noise: raw white read as harsh "sissing"
    // (strong 8-16 kHz air band). ~1.4 kHz makes it a soft breaker wash.
    crash_lp: OnePoleLP,
    /// Keeps the crash white noise at its 48 kHz density per Hz.
    white_noise_scale: f32,
    sample_rate_cached: f32,

    // Tuning knobs
    /// Seconds between swell peaks (3–10 typical for ocean).
    pub period: f32,
    /// Per-second probability of a breaker crash event.
    pub crash_prob: f32,
    /// Brown noise (deep roar) gain.
    pub roar_level: f32,
    /// Pink noise (wash / hiss) gain.
    pub hiss_level: f32,
    /// Crash burst gain.
    pub crash_gain: f32,
    /// Output trim.
    pub master_gain: f32,

    /// World-space position.
    pub position: Vec3,

    rng: Rng,
}

impl WaveSource {
    /// Create a new wave source.
    ///
    /// - `position`: where the waves originate (e.g. a far wall)
    /// - `period`: seconds between swell peaks (6 is a good default)
    /// - `crash_prob`: breaker probability per second (0.25 = occasional crashing)
    /// - `seed`: PRNG seed
    pub fn new(position: Vec3, period: f32, crash_prob: f32, seed: u64) -> Self {
        Self {
            pink: PinkNoise::new(seed.wrapping_add(1)),
            brown: BrownNoise::new(seed.wrapping_add(2)),
            t: 0,
            crash_pos: 0,
            crash_len: 0,
            crash_lp: OnePoleLP::new(CRASH_LOWPASS_HZ, 48_000.0),
            white_noise_scale: 1.0,
            sample_rate_cached: 0.0,
            period: period.max(1.0),
            crash_prob: crash_prob.clamp(0.0, 1.0),
            roar_level: 0.8,
            // Lower hiss + softer crashes: the pink-noise "sissing" the user
            // heard sat on top of the swell. Roar (brown) still carries the body.
            hiss_level: 0.15,
            crash_gain: 0.45,
            master_gain: 1.0,
            position,
            rng: Rng::new(seed),
        }
    }

    fn retune(&mut self, sample_rate: f32) {
        self.crash_lp.set_cutoff(CRASH_LOWPASS_HZ, sample_rate);
        self.white_noise_scale = white_noise_density_scale(sample_rate);
    }

    /// Inline Gaussian window for crash burst.
    /// Peak at 1/4 of duration for a "crashing" feel (asymmetric — fast attack, slow decay).
    fn crash_envelope(&self) -> f32 {
        if self.crash_len == 0 {
            return 0.0;
        }
        let mid = self.crash_len as f32 / 4.0; // peak earlier
        let sigma = self.crash_len as f32 / 6.0;
        let x = (self.crash_pos as f32 - mid) / sigma;
        (-0.5 * x * x).exp()
    }
}

impl SoundSource for WaveSource {
    /// What changes while it plays; any other setting is not one it has.
    fn set_synth_param(&mut self, param: SynthParam, value: f32) {
        // `f32::clamp` passes NaN through, and one NaN would turn every later
        // sample into NaN: keep the previous value instead.
        if !value.is_finite() {
            return;
        }
        match param {
            SynthParam::WavePeriod => self.period = value.max(1.0),
            SynthParam::CrashChance => self.crash_prob = value.clamp(0.0, 1.0),
            SynthParam::MasterGain => self.master_gain = value.clamp(0.0, 2.0),
            _ => {}
        }
    }

    fn next_sample(&mut self, sample_rate: f32) -> f32 {
        self.next_mixed(sample_rate, &mut LayerMix::untouched())
    }

    /// Three layers, in this order: roar, hiss and crash.
    fn layer_count(&self) -> usize {
        3
    }

    #[inline]
    fn next_mixed(&mut self, sample_rate: f32, mix: &mut LayerMix) -> f32 {
        if self.sample_rate_cached != sample_rate {
            self.sample_rate_cached = sample_rate;
            self.retune(sample_rate);
        }
        let omega = TAU / (self.period * sample_rate);

        // 1. Swell envelope: slow sine oscillation mapped to [0, 1]
        let swell = 0.5 * (1.0 + (self.t as f32 * omega).sin());

        // 2. Trigger crash burst stochastically
        if self.crash_len == 0 && self.rng.next_f32() < self.crash_prob / sample_rate {
            self.crash_len = (0.4 * sample_rate) as usize; // 400 ms burst
            self.crash_pos = 0;
        }

        let mut crash = 0.0;
        if self.crash_len > 0 {
            let env = self.crash_envelope();
            crash = self
                .crash_lp
                .process(self.rng.next_bipolar() * self.white_noise_scale)
                * env;
            self.crash_pos += 1;
            if self.crash_pos >= self.crash_len {
                self.crash_len = 0;
            }
        }

        // 3. Noise sources
        let roar = self.brown.next_sample(sample_rate);
        let hiss = self.pink.next_sample(sample_rate);

        // 4. Mix: noise bed modulated by swell + crash layer
        let sample = mix.sum(
            0,
            &[
                self.roar_level * roar * swell,
                self.hiss_level * hiss * swell,
                self.crash_gain * crash,
            ],
        ) * self.master_gain;

        self.t = self.t.wrapping_add(1);
        sample
    }

    fn position(&self) -> Vec3 {
        self.position
    }

    fn tick(&mut self, _dt: f32) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wave_produces_nonzero_output() {
        let mut wave = WaveSource::new(Vec3::ZERO, 6.0, 0.25, 42);
        let sum: f32 = (0..48000).map(|_| wave.next_sample(48000.0).abs()).sum();
        assert!(sum > 0.0, "wave source is silent");
    }

    #[test]
    fn wave_has_cyclic_energy() {
        let mut wave = WaveSource::new(Vec3::ZERO, 1.0, 0.0, 42);

        // With period=1s and crash_prob=0, energy should oscillate.
        // Sample first half-period and second half-period.
        let first_half: f32 = (0..24000).map(|_| wave.next_sample(48000.0).powi(2)).sum();
        let second_half: f32 = (0..24000).map(|_| wave.next_sample(48000.0).powi(2)).sum();

        // The two halves should have noticeably different energy
        // (one includes the swell peak, the other the trough)
        let ratio = first_half.max(second_half) / first_half.min(second_half);
        assert!(
            ratio > 1.2,
            "expected cyclic energy variation, got ratio {ratio}"
        );
    }

    use crate::synth::test_support::{
        averaged_power_spectrum, band_density_db, mean, peak, render, root_mean_square,
    };

    const SAMPLE_RATES: [f32; 3] = [44_100.0, 48_000.0, 96_000.0];

    /// Mean square of `samples` split into `windows` equal consecutive parts.
    fn windowed_power(samples: &[f32], windows: usize) -> Vec<f64> {
        let length = samples.len() / windows;
        (0..windows)
            .map(|window| {
                let slice = &samples[window * length..(window + 1) * length];
                slice
                    .iter()
                    .map(|&value| (value as f64).powi(2))
                    .sum::<f64>()
                    / length as f64
            })
            .collect()
    }

    /// The swell is 0.5 (1 + sin(2 pi t / period)): its power envelope peaks a
    /// quarter-period in and bottoms out at three quarters, whatever the
    /// sample rate. Only the pink hiss is left on so the noise has no
    /// slow drift of its own.
    #[test]
    fn swell_peaks_at_a_quarter_period_and_troughs_at_three_quarters() {
        for sample_rate in SAMPLE_RATES {
            let mut wave = WaveSource::new(Vec3::ZERO, 2.0, 0.0, 42);
            wave.roar_level = 0.0;
            let samples = render(2.0, sample_rate, |rate| wave.next_sample(rate));
            let power = windowed_power(&samples, 8);
            let loudest = (0..8)
                .max_by(|&a, &b| power[a].partial_cmp(&power[b]).unwrap())
                .unwrap();
            let quietest = (0..8)
                .min_by(|&a, &b| power[a].partial_cmp(&power[b]).unwrap())
                .unwrap();
            // Windows 1 and 2 straddle t = T/4; windows 5 and 6 straddle 3T/4.
            assert!(
                (1..=2).contains(&loudest),
                "{sample_rate}: loudest window {loudest}"
            );
            assert!(
                (5..=6).contains(&quietest),
                "{sample_rate}: quietest window {quietest}"
            );
            // Integrated over a window the swell^2 ratio is large (>20).
            assert!(power[loudest] / power[quietest] > 20.0);
        }
    }

    /// Mean power of swell^2 over a full period is 0.375 (3/8): measured
    /// against a free-running reference with the swell removed.
    #[test]
    fn swell_scales_mean_power_by_three_eighths() {
        let mut modulated = WaveSource::new(Vec3::ZERO, 2.0, 0.0, 42);
        modulated.roar_level = 0.0;
        // Same pink stream (seed + 1) without the swell, at the same 0.15 gain.
        let swelled = render(8.0, 48_000.0, |rate| modulated.next_sample(rate));
        let mut pink = PinkNoise::new(42u64.wrapping_add(1));
        let raw = render(8.0, 48_000.0, |rate| 0.15 * pink.next_sample(rate));
        let ratio = root_mean_square(&swelled).powi(2) / root_mean_square(&raw).powi(2);
        assert!((ratio - 0.375).abs() < 0.03, "power ratio {ratio}");
    }

    /// Gaussian window centred at len/4 with sigma len/6: unity at the peak,
    /// and the 400 ms burst ends essentially silent.
    #[test]
    fn crash_envelope_peaks_at_a_quarter_of_the_burst() {
        let mut wave = WaveSource::new(Vec3::ZERO, 6.0, 0.0, 1);
        wave.crash_len = 4_000;
        let mut best_position = 0;
        let mut best_value = 0.0_f32;
        let mut values = Vec::new();
        for position in 0..wave.crash_len {
            wave.crash_pos = position;
            let value = wave.crash_envelope();
            values.push(value);
            if value > best_value {
                best_value = value;
                best_position = position;
            }
        }
        assert_eq!(best_position, 1_000);
        assert!((best_value - 1.0).abs() < 1e-6);
        // Onset is already 0.32 of peak (exp(-1.125)): a step, not a fade-in.
        assert!((values[0] - (-1.125_f32).exp()).abs() < 1e-5);
        assert!(*values.last().unwrap() < 1e-3);
        assert_eq!(
            WaveSource::new(Vec3::ZERO, 6.0, 0.0, 1).crash_envelope(),
            0.0
        );
    }

    /// Number of crash starts observed over `seconds`.
    fn count_crashes(crash_probability: f32, seconds: f32, sample_rate: f32) -> usize {
        let mut wave = WaveSource::new(Vec3::ZERO, 6.0, crash_probability, 11);
        let mut crashes = 0;
        let mut was_crashing = false;
        for _ in 0..(seconds * sample_rate) as usize {
            wave.next_sample(sample_rate);
            let crashing = wave.crash_len > 0;
            if crashing && !was_crashing {
                crashes += 1;
                assert_eq!(wave.crash_len, (0.4 * sample_rate) as usize);
            }
            was_crashing = crashing;
        }
        crashes
    }

    /// `crash_prob` is used as a Poisson rate (events per second) but a crash
    /// cannot retrigger during its own 400 ms, so the observed rate is
    /// p / (1 + 0.4 p): 0.714 per second for p = 1, 0.227 for p = 0.25.
    #[test]
    fn crash_rate_follows_the_dead_time_corrected_poisson_rate() {
        // 8 kHz keeps the test cheap; the rate maths does not depend on it.
        let measured = count_crashes(1.0, 200.0, 8_000.0);
        let expected = 200.0 / (1.0 + 0.4); // 142.9
        assert!(
            (measured as f32 - expected).abs() < 5.0 * expected.sqrt(),
            "{measured} crashes in 200 s, expected {expected:.0}"
        );
        let slow = count_crashes(0.25, 400.0, 8_000.0);
        let expected_slow = 400.0 * 0.25 / (1.0 + 0.4 * 0.25);
        assert!(
            (slow as f32 - expected_slow).abs() < 5.0 * expected_slow.sqrt(),
            "{slow} crashes in 400 s, expected {expected_slow:.0}"
        );
    }

    #[test]
    fn crash_rate_is_the_same_at_every_sample_rate() {
        let at_44k = count_crashes(1.0, 60.0, 44_100.0) as f32;
        let at_96k = count_crashes(1.0, 60.0, 96_000.0) as f32;
        // Expected 42.9 each; the difference of two Poisson counts has
        // standard deviation sqrt(2 * 42.9) = 9.3, so allow three of them.
        assert!((at_44k - at_96k).abs() < 3.0 * 9.3, "{at_44k} vs {at_96k}");
    }

    #[test]
    fn constructor_clamps_period_and_crash_probability() {
        let wave = WaveSource::new(Vec3::ZERO, 0.1, 5.0, 1);
        assert_eq!(wave.period, 1.0);
        assert_eq!(wave.crash_prob, 1.0);
        assert_eq!(WaveSource::new(Vec3::ZERO, 6.0, -1.0, 1).crash_prob, 0.0);
    }

    #[test]
    fn output_is_finite_zero_mean_and_inside_unity() {
        let mut wave = WaveSource::new(Vec3::ZERO, 6.0, 1.0, 5);
        let samples = render(20.0, 48_000.0, |rate| wave.next_sample(rate));
        assert!(samples.iter().all(|value| value.is_finite()));
        assert!(peak(&samples) < 1.0, "peak {}", peak(&samples));
        assert!(mean(&samples).abs() < 0.02, "DC {}", mean(&samples));
    }

    #[test]
    fn rms_is_stable_across_sample_rates() {
        let rms = |sample_rate: f32| {
            let mut wave = WaveSource::new(Vec3::ZERO, 6.0, 0.25, 42);
            let samples = render(12.0, sample_rate, |rate| wave.next_sample(rate));
            root_mean_square(&samples)
        };
        let ratio = rms(96_000.0) / rms(48_000.0);
        assert!((0.85..1.17).contains(&ratio), "RMS ratio {ratio}");
    }

    /// The breaker wash is a soft ~1.4 kHz lowpass at every host rate. Built
    /// once for 48 kHz and never retuned, its corner would move in proportion
    /// (2.8 kHz at 96 kHz) and the crash would get 6 dB brighter above 4 kHz.
    #[test]
    fn crash_wash_brightness_is_sample_rate_independent() {
        let brightness_db = |sample_rate: f32| {
            let mut wave = WaveSource::new(Vec3::ZERO, 6.0, 1.0, 42);
            wave.roar_level = 0.0;
            wave.hiss_level = 0.0;
            let samples = render(8.0, sample_rate, |rate| wave.next_sample(rate));
            let spectrum = averaged_power_spectrum(&samples);
            band_density_db(&spectrum, sample_rate, 4_000.0, 5_000.0)
                - band_density_db(&spectrum, sample_rate, 500.0, 1_000.0)
        };
        let difference = (brightness_db(96_000.0) - brightness_db(48_000.0)).abs();
        assert!(
            difference < 1.0,
            "4-5 kHz vs 0.5-1 kHz moved {difference:.1} dB"
        );
    }

    /// At 48 kHz the same measurement does show the documented one-pole
    /// rolloff: -6 dB/oct above 1.4 kHz means 4.5 kHz sits about 12 dB below
    /// the 0.5-1 kHz plateau (white noise is flat, so only the filter shapes it).
    #[test]
    fn crash_wash_is_a_soft_one_pole_lowpass_at_48_khz() {
        let mut wave = WaveSource::new(Vec3::ZERO, 6.0, 1.0, 42);
        wave.roar_level = 0.0;
        wave.hiss_level = 0.0;
        let samples = render(8.0, 48_000.0, |rate| wave.next_sample(rate));
        let spectrum = averaged_power_spectrum(&samples);
        let drop = band_density_db(&spectrum, 48_000.0, 500.0, 1_000.0)
            - band_density_db(&spectrum, 48_000.0, 4_000.0, 5_000.0);
        // One pole at 1.4 kHz: |H|^2(750) / |H|^2(4500) = (1+(4500/1400)^2)/(1+(750/1400)^2) ≈ 8.8 = 9.4 dB.
        assert!(
            (drop - 9.4).abs() < 1.5,
            "plateau-to-4.5 kHz drop {drop:.1} dB"
        );
    }

    /// What changes while it plays reaches the sound: the period and the
    /// chance of a crash are kept in range, and master gain 0 silences it.
    #[test]
    fn its_period_crashes_and_gain_change_while_it_plays() {
        use atrium_core::commands::SynthParam;
        use atrium_core::source::SoundSource;
        let mut sea = WaveSource::new(Vec3::ZERO, 8.0, 0.3, 4);
        sea.set_synth_param(SynthParam::WavePeriod, 0.2);
        sea.set_synth_param(SynthParam::CrashChance, 2.0);
        assert_eq!((sea.period, sea.crash_prob), (1.0, 1.0));
        sea.set_synth_param(SynthParam::MasterGain, 0.0);
        let energy: f32 = (0..96_000).map(|_| sea.next_sample(48_000.0).powi(2)).sum();
        assert_eq!(energy, 0.0);
    }
}
