// Frequency-balance analysis for every procedural synth generator.
//
// Renders each generator dry (no pipeline) and prints its octave-ish band
// energies + spectral centroid. Use it to verify "warmer" objectively — a
// lower centroid and less brilliance/air energy means darker/warmer.
//
//   cargo run --bin analyze_synth

use std::f32::consts::TAU;

use atrium::synth::canopy_wind::CanopyWindSource;
use atrium::synth::field_wind::FieldWindSource;
use atrium::synth::rain::RainSource;
use atrium::synth::rain_v2::RainSourceV2;
use atrium::synth::soft_wind::SoftWindSource;
use atrium::synth::storm_wind::StormWindSource;
use atrium::synth::wave::WaveSource;
use atrium::world::types::Vec3;
use atrium_core::source::SoundSource;

const BANDS: &[(f32, f32, &str)] = &[
    (20.0, 100.0, "sub-bass     "),
    (100.0, 250.0, "bass         "),
    (250.0, 500.0, "low-mid      "),
    (500.0, 1000.0, "mid          "),
    (1000.0, 2000.0, "upper-mid    "),
    (2000.0, 4000.0, "presence     "),
    (4000.0, 8000.0, "brilliance   "),
    (8000.0, 16000.0, "air          "),
];

fn goertzel(samples: &[f32], sample_rate: f32, freq: f32) -> f32 {
    let n = samples.len();
    let k = (freq * n as f32 / sample_rate).round();
    let w = TAU * k / n as f32;
    let coeff = 2.0 * w.cos();

    let mut s0 = 0.0_f32;
    let mut s1 = 0.0_f32;
    let mut s2;

    for &x in samples {
        s2 = s1;
        s1 = s0;
        s0 = x + coeff * s1 - s2;
    }

    let mag_sq = s0 * s0 + s1 * s1 - coeff * s0 * s1;
    mag_sq / (n as f32 * n as f32)
}

fn band_energy(samples: &[f32], sample_rate: f32, f_low: f32, f_high: f32) -> f32 {
    let num_probes = 8;
    let mut total_energy = 0.0;
    for i in 0..num_probes {
        let freq = f_low + (f_high - f_low) * (i as f32 + 0.5) / num_probes as f32;
        total_energy += goertzel(samples, sample_rate, freq);
    }
    total_energy / num_probes as f32
}

/// Render a generator to a mono buffer at 44.1 kHz for 10 s.
fn render(mut generator: Box<dyn SoundSource>) -> (Vec<f32>, f32) {
    let sample_rate = 44_100.0;
    let num_samples = (sample_rate * 10.0) as usize;
    let samples: Vec<f32> = (0..num_samples)
        .map(|_| generator.next_sample(sample_rate))
        .collect();
    (samples, sample_rate)
}

fn analyze(label: &str, generator: Box<dyn SoundSource>) {
    let (samples, sample_rate) = render(generator);

    let rms: f32 = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
    let peak = samples.iter().fold(0.0_f32, |m, &s| m.max(s.abs()));
    let crest_db = if rms > 0.0 {
        20.0 * (peak / rms).log10()
    } else {
        0.0
    };

    println!("\n=== {label} ===");
    println!(
        "  RMS {:.4} ({:+.1} dBFS)   peak {:.3}   crest {:.1} dB",
        rms,
        20.0 * rms.log10(),
        peak,
        crest_db
    );
    println!("  Band             Hz range       Relative");
    println!("  ─────────────────────────────────────────────────────");

    let energies: Vec<_> = BANDS
        .iter()
        .map(|&(f_low, f_high, name)| {
            (
                name,
                f_low,
                f_high,
                band_energy(&samples, sample_rate, f_low, f_high),
            )
        })
        .collect();
    let max_energy = energies.iter().map(|e| e.3).fold(0.0_f32, f32::max);

    for &(name, f_low, f_high, energy) in &energies {
        let relative_db = if max_energy > 0.0 && energy > 0.0 {
            10.0 * (energy / max_energy).log10()
        } else {
            -120.0
        };
        let bar_len = ((relative_db + 40.0) / 40.0 * 30.0).clamp(0.0, 30.0) as usize;
        let bar: String = "█".repeat(bar_len);
        println!(
            "  {name} {:>5.0}-{:<5.0}Hz  {:>+6.1} dB  {bar}",
            f_low, f_high, relative_db
        );
    }

    let (weighted, total): (f32, f32) =
        energies.iter().fold((0.0, 0.0), |(w, t), &(_, lo, hi, e)| {
            ((w + (lo + hi) / 2.0 * e), t + e)
        });
    let centroid = if total > 0.0 { weighted / total } else { 0.0 };
    // Fraction of energy above 2 kHz — the "brightness / sizzle" measure.
    let bright: f32 = energies.iter().filter(|e| e.1 >= 2000.0).map(|e| e.3).sum();
    println!(
        "  → centroid {centroid:.0} Hz    energy >2 kHz: {:.1}%",
        100.0 * bright / total.max(1e-12)
    );
}

fn main() {
    println!("Procedural synth frequency balance (dry, 10 s @ 44.1 kHz)");
    println!("Lower centroid + less >2 kHz energy = warmer.");

    let mut field_wind = FieldWindSource::new(Vec3::ZERO, 1.0, 8.0, 42);
    field_wind.set_change_time_range(20.0, 50.0);
    field_wind.set_gust_duration_range(3.0, 10.0);
    field_wind.gust_strength = 0.35;
    field_wind.rise_bias = 0.25;
    field_wind.turbulence_depth = 0.20;
    analyze("Field wind (1-8 m/s)", Box::new(field_wind));
    analyze(
        "Soft wind (1-5 m/s)",
        Box::new(SoftWindSource::new(Vec3::ZERO, 1.0, 5.0, 46)),
    );
    let mut canopy_wind = CanopyWindSource::new(Vec3::ZERO, 1.5, 8.0, 43);
    canopy_wind.set_change_time_range(15.0, 45.0);
    canopy_wind.set_gust_duration_range(2.0, 8.0);
    analyze("Canopy wind (1.5-8 m/s)", Box::new(canopy_wind));
    analyze(
        "Storm wind (8-18 m/s)",
        Box::new(StormWindSource::new(Vec3::ZERO, 8.0, 18.0, 44)),
    );
    analyze(
        "Waves (period 6, crash 0.25)",
        Box::new(WaveSource::new(Vec3::ZERO, 6.0, 0.25, 42)),
    );
    analyze(
        "Rain v1 (intensity 0.5)",
        Box::new(RainSource::new(Vec3::ZERO, 0.5, 42)),
    );
    analyze(
        "Rain v2 (intensity 0.5)",
        Box::new(RainSourceV2::new(Vec3::ZERO, 0.5, 42)),
    );
}
