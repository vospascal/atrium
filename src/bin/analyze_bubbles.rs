// Compare synth output with and without bubbles to see their contribution.

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
    let mut total = 0.0;
    for i in 0..num_probes {
        let freq = f_low + (f_high - f_low) * (i as f32 + 0.5) / num_probes as f32;
        total += goertzel(samples, sample_rate, freq);
    }
    total / num_probes as f32
}

fn analyze(label: &str, samples: &[f32], sr: f32) {
    println!("\n  {label}:");
    let rms: f32 = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
    println!("    RMS: {:.4} ({:.1} dBFS)", rms, 20.0 * rms.log10());

    let mut energies = Vec::new();
    for &(f_low, f_high, name) in BANDS {
        energies.push((name, f_low, f_high, band_energy(samples, sr, f_low, f_high)));
    }
    let max_e = energies.iter().map(|e| e.3).fold(0.0_f32, f32::max);

    for &(name, f_low, f_high, energy) in &energies {
        let rel = if max_e > 0.0 && energy > 0.0 {
            10.0 * (energy / max_e).log10()
        } else { -120.0 };
        let bar_len = ((rel + 40.0) / 40.0 * 25.0).clamp(0.0, 25.0) as usize;
        let bar: String = "█".repeat(bar_len);
        println!("    {name} {:>5.0}-{:<5.0}Hz {:>+5.1} dB  {bar}", f_low, f_high, rel);
    }
}

fn main() {
    let sr = 44100.0;
    let n = (sr * 10.0) as usize;

    for &(label, intensity) in &[("Light (0.2)", 0.2_f32), ("Medium (0.5)", 0.5), ("Heavy (0.9)", 0.9)] {
        println!("\n==================================================");
        println!("=== {label} ===");

        // With bubbles (normal)
        let mut rain = RainSourceV2::new(Vec3::ZERO, intensity, 0xDEAD_BEEF);
        let full: Vec<f32> = (0..n).map(|_| rain.next_sample(sr)).collect();

        // Without bubbles
        let mut rain_no_bub = RainSourceV2::new(Vec3::ZERO, intensity, 0xDEAD_BEEF);
        rain_no_bub.bubble_gain = 0.0;
        let no_bub: Vec<f32> = (0..n).map(|_| rain_no_bub.next_sample(sr)).collect();

        analyze("WITH bubbles", &full, sr);
        analyze("WITHOUT bubbles (impacts + bed only)", &no_bub, sr);

        // Difference
        let rms_full: f32 = (full.iter().map(|s| s * s).sum::<f32>() / n as f32).sqrt();
        let rms_no: f32 = (no_bub.iter().map(|s| s * s).sum::<f32>() / n as f32).sqrt();
        let bubble_contrib = if rms_full > 0.0 {
            ((rms_full - rms_no) / rms_full * 100.0).abs()
        } else { 0.0 };
        println!("\n  Bubble contribution to RMS: ~{bubble_contrib:.0}%");
    }
}
