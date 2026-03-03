// Analyze frequency spectrum of our RainSourceV2 synth output
// at different intensities, to compare against real rain recordings.

use std::f32::consts::TAU;

use atrium::spatial::source::SoundSource;
use atrium::synth::rain_v2::RainSourceV2;
use atrium::world::types::Vec3;

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

fn analyze_synth(label: &str, intensity: f32) {
    let sr = 44100.0;
    let num_samples = (sr * 10.0) as usize; // 10 seconds

    let mut rain = RainSourceV2::new(Vec3::ZERO, intensity, 0xDEAD_BEEF);
    let samples: Vec<f32> = (0..num_samples).map(|_| rain.next_sample(sr)).collect();

    let rms: f32 = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();

    println!("\n=== Synth: {} (intensity={}) ===", label, intensity);
    println!("  RMS level: {:.4} ({:.1} dBFS)", rms, 20.0 * rms.log10());

    println!();
    println!("  Band             Hz range      Energy (dB)   Relative");
    println!("  ─────────────────────────────────────────────────────");

    let mut energies = Vec::new();
    for &(f_low, f_high, name) in BANDS {
        let energy = band_energy(&samples, sr, f_low, f_high);
        energies.push((name, f_low, f_high, energy));
    }

    let max_energy = energies.iter().map(|e| e.3).fold(0.0_f32, f32::max);

    for &(name, f_low, f_high, energy) in &energies {
        let db = if energy > 0.0 { 10.0 * energy.log10() } else { -120.0 };
        let relative_db = if max_energy > 0.0 && energy > 0.0 {
            10.0 * (energy / max_energy).log10()
        } else {
            -120.0
        };
        let bar_len = ((relative_db + 40.0) / 40.0 * 30.0).clamp(0.0, 30.0) as usize;
        let bar: String = "█".repeat(bar_len);
        println!("  {name} {:>5.0}-{:<5.0}Hz  {:>7.1} dB    {:>+5.1} dB  {bar}",
            f_low, f_high, db, relative_db);
    }

    let mut weighted_sum = 0.0_f32;
    let mut total_weight = 0.0_f32;
    for &(_, f_low, f_high, energy) in &energies {
        let center = (f_low + f_high) / 2.0;
        weighted_sum += center * energy;
        total_weight += energy;
    }
    let centroid = if total_weight > 0.0 { weighted_sum / total_weight } else { 0.0 };
    println!();
    println!("  Spectral centroid: {:.0} Hz", centroid);
}

fn main() {
    println!("╔═══════════════════════════════════════════════╗");
    println!("║  RainSourceV2 Synth Frequency Analysis        ║");
    println!("╚═══════════════════════════════════════════════╝");

    analyze_synth("Light rain", 0.2);
    analyze_synth("Medium rain", 0.5);
    analyze_synth("Heavy rain", 0.9);
}
