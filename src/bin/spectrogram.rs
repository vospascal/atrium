// Render spectrogram images (time × log-frequency) of the real wind recording
// and the wind synths, so their spectral *motion* can be compared by eye.
//
// Writes PPM files (no image-crate dep); convert to PNG with `sips`.
//   cargo run --bin spectrogram -- "<recording>" <out_dir>

use std::path::Path;

use atrium::audio::decode::decode_file;
use atrium::synth::canopy_wind::CanopyWindSource;
use atrium::synth::field_wind::FieldWindSource;
use atrium::synth::soft_wind::SoftWindSource;
use atrium::synth::storm_wind::StormWindSource;
use atrium::world::types::Vec3;
use atrium_core::source::SoundSource;
use realfft::RealFftPlanner;

const FFT_SIZE: usize = 2048;
const HOP: usize = 512;
const HEIGHT: usize = 480;
const F_MIN: f32 = 40.0;
const F_MAX: f32 = 16_000.0;

/// "hot" colormap: black → red → yellow → white for t in [0, 1].
fn hot(t: f32) -> (u8, u8, u8) {
    let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u8;
    (c(3.0 * t), c(3.0 * t - 1.0), c(3.0 * t - 2.0))
}

fn write_spectrogram(label: &str, samples: &[f32], sample_rate: f32, out_path: &str) {
    let mut planner = RealFftPlanner::<f32>::new();
    let r2c = planner.plan_fft_forward(FFT_SIZE);
    let mut inbuf = r2c.make_input_vec();
    let mut spectrum = r2c.make_output_vec();
    let n_bins = spectrum.len();

    // Hann window.
    let window: Vec<f32> = (0..FFT_SIZE)
        .map(|i| {
            let s = (std::f32::consts::PI * i as f32 / (FFT_SIZE - 1) as f32).sin();
            s * s
        })
        .collect();

    // One magnitude column per hop.
    let mut cols: Vec<Vec<f32>> = Vec::new();
    let mut pos = 0;
    while pos + FFT_SIZE <= samples.len() {
        for i in 0..FFT_SIZE {
            inbuf[i] = samples[pos + i] * window[i];
        }
        r2c.process(&mut inbuf, &mut spectrum).unwrap();
        cols.push(spectrum.iter().map(|c| c.norm()).collect());
        pos += HOP;
    }
    if cols.is_empty() {
        println!("{label}: too short");
        return;
    }
    let width = cols.len();

    let max_mag = cols
        .iter()
        .flatten()
        .cloned()
        .fold(0.0_f32, f32::max)
        .max(1e-9);

    let mut img = vec![0u8; width * HEIGHT * 3];
    for x in 0..width {
        for y in 0..HEIGHT {
            // Top row = high frequency (log scale).
            let frac = 1.0 - y as f32 / (HEIGHT - 1) as f32;
            let freq = F_MIN * (F_MAX / F_MIN).powf(frac);
            let bin = ((freq / sample_rate) * FFT_SIZE as f32).round() as usize;
            let mag = if bin < n_bins { cols[x][bin] } else { 0.0 };
            let db = 20.0 * (mag / max_mag).max(1e-6).log10(); // -120..0
            let t = ((db + 80.0) / 80.0).clamp(0.0, 1.0); // -80 dB floor
            let (r, g, b) = hot(t);
            let idx = (y * width + x) * 3;
            img[idx] = r;
            img[idx + 1] = g;
            img[idx + 2] = b;
        }
    }

    let mut out = format!("P6\n{width} {HEIGHT}\n255\n").into_bytes();
    out.extend_from_slice(&img);
    std::fs::write(out_path, out).unwrap();
    let secs = width as f32 * HOP as f32 / sample_rate;
    println!(
        "{label}: wrote {out_path}  ({width}×{HEIGHT}, {secs:.0}s, {F_MIN:.0}–{F_MAX:.0} Hz log)"
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "assets/wind_field.mp3".into());
    let out_dir = args.get(2).cloned().unwrap_or_else(|| ".".into());

    let buffer = decode_file(Path::new(&path)).expect("decode failed");
    let sr = buffer.sample_rate as f32;
    // Analyze the first 40 s (covers the two flagged passages).
    let n = ((40.0 * sr) as usize).min(buffer.samples.len());

    write_spectrogram(
        "recording",
        &buffer.samples[..n],
        sr,
        &format!("{out_dir}/spec_recording.ppm"),
    );

    let mut field = FieldWindSource::new(Vec3::ZERO, 1.0, 8.0, 42);
    let samples: Vec<f32> = (0..n).map(|_| field.next_sample(sr)).collect();
    write_spectrogram(
        "field_wind",
        &samples,
        sr,
        &format!("{out_dir}/spec_field_wind.ppm"),
    );

    let mut soft = SoftWindSource::new(Vec3::ZERO, 1.0, 5.0, 46);
    let samples: Vec<f32> = (0..n).map(|_| soft.next_sample(sr)).collect();
    write_spectrogram(
        "soft_wind",
        &samples,
        sr,
        &format!("{out_dir}/spec_soft_wind.ppm"),
    );

    let mut canopy = CanopyWindSource::new(Vec3::ZERO, 1.5, 8.0, 43);
    let samples: Vec<f32> = (0..n).map(|_| canopy.next_sample(sr)).collect();
    write_spectrogram(
        "canopy_wind",
        &samples,
        sr,
        &format!("{out_dir}/spec_canopy_wind.ppm"),
    );

    let mut storm = StormWindSource::new(Vec3::ZERO, 8.0, 18.0, 44);
    let samples: Vec<f32> = (0..n).map(|_| storm.next_sample(sr)).collect();
    write_spectrogram(
        "storm_wind",
        &samples,
        sr,
        &format!("{out_dir}/spec_storm_wind.ppm"),
    );
}
