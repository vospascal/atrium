// Analyze frequency spectrum of rain audio recordings.
//
// Decodes each MP3, computes energy in frequency bands via DFT,
// and prints a profile we can use to tune RainSourceV2.

use std::f32::consts::TAU;
use std::path::Path;

use atrium::audio::decode::decode_file;

/// Frequency bands to analyze (Hz)
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

/// Compute energy in a frequency band using Goertzel's algorithm.
/// Much faster than full FFT when we only need specific bands.
fn band_energy(samples: &[f32], sample_rate: f32, f_low: f32, f_high: f32) -> f32 {
    // Sample several frequencies within the band
    let num_probes = 8;
    let mut total_energy = 0.0;

    for i in 0..num_probes {
        let freq = f_low + (f_high - f_low) * (i as f32 + 0.5) / num_probes as f32;
        let energy = goertzel(samples, sample_rate, freq);
        total_energy += energy;
    }

    total_energy / num_probes as f32
}

/// Goertzel algorithm — compute magnitude² at a single frequency.
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

    // Magnitude squared, normalized by N²
    let mag_sq = s0 * s0 + s1 * s1 - coeff * s0 * s1;
    mag_sq / (n as f32 * n as f32)
}

fn analyze_file(path: &str) {
    let buf = match decode_file(Path::new(path)) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Failed to decode {path}: {e}");
            return;
        }
    };

    let sr = buf.sample_rate as f32;
    // Use first 20 seconds for analysis
    let start = 0;
    let end = ((sr * 20.0) as usize).min(buf.samples.len());
    let chunk = &buf.samples[start..end];

    println!("\n=== {} ===", path.rsplit('/').next().unwrap_or(path));
    println!(
        "  Duration: {:.1}s | Sample rate: {}Hz | Samples: {}",
        buf.samples.len() as f32 / sr,
        buf.sample_rate,
        buf.samples.len()
    );

    // RMS level
    let rms: f32 = (chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len() as f32).sqrt();
    println!("  RMS level: {:.4} ({:.1} dBFS)", rms, 20.0 * rms.log10());

    // Peak
    let peak = chunk.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
    println!("  Peak:      {:.4} ({:.1} dBFS)", peak, 20.0 * peak.log10());

    // Band analysis
    println!();
    println!("  Band             Hz range      Energy (dB)   Relative");
    println!("  ─────────────────────────────────────────────────────");

    let mut energies = Vec::new();
    for &(f_low, f_high, name) in BANDS {
        let energy = band_energy(chunk, sr, f_low, f_high);
        energies.push((name, f_low, f_high, energy));
    }

    let max_energy = energies.iter().map(|e| e.3).fold(0.0_f32, f32::max);

    for &(name, f_low, f_high, energy) in &energies {
        let db = if energy > 0.0 {
            10.0 * energy.log10()
        } else {
            -120.0
        };
        let relative_db = if max_energy > 0.0 && energy > 0.0 {
            10.0 * (energy / max_energy).log10()
        } else {
            -120.0
        };
        let bar_len = ((relative_db + 40.0) / 40.0 * 30.0).clamp(0.0, 30.0) as usize;
        let bar: String = "█".repeat(bar_len);
        println!(
            "  {name} {:>5.0}-{:<5.0}Hz  {:>7.1} dB    {:>+5.1} dB  {bar}",
            f_low, f_high, db, relative_db
        );
    }

    // Spectral centroid (brightness measure)
    let mut weighted_sum = 0.0_f32;
    let mut total_weight = 0.0_f32;
    for &(_, f_low, f_high, energy) in &energies {
        let center = (f_low + f_high) / 2.0;
        weighted_sum += center * energy;
        total_weight += energy;
    }
    let centroid = if total_weight > 0.0 {
        weighted_sum / total_weight
    } else {
        0.0
    };
    println!();
    println!(
        "  Spectral centroid: {:.0} Hz (lower = darker/muddier)",
        centroid
    );
}

fn main() {
    let base = "research papers";
    let files = [format!("{base}/light rain youtube.wav")];

    println!("╔═══════════════════════════════════════════════╗");
    println!("║  Rain Audio Frequency Analysis                ║");
    println!("╚═══════════════════════════════════════════════╝");

    for f in &files {
        analyze_file(f);
    }

    println!("\n\n=== COMPARISON SUMMARY ===\n");
    println!("Key observations for tuning RainSourceV2:");
    println!("  • Compare spectral centroids: lower = muddier");
    println!("  • Check sub-bass/bass vs brilliance/air ratio");
    println!("  • Heavy rain should have more low-frequency energy");
    println!("  • Individual drops appear as presence/brilliance spikes");
}
