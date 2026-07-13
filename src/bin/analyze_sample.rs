// Analyze a real audio recording's spectral balance and temporal dynamics,
// so we can match a synth generator to it. Prints per-segment band energy,
// spectral centroid, crest factor, and short-time RMS statistics (gust depth
// + timing). Compares against the current field-wind synth for reference.
//
//   cargo run --bin analyze_sample -- "<path>" [seg_start_s seg_end_s]...

use std::f32::consts::TAU;
use std::path::Path;

use atrium::audio::decode::decode_file;
use atrium::synth::field_wind::FieldWindSource;
use atrium::synth::soft_wind::SoftWindSource;
use atrium::world::types::Vec3;
use atrium_core::source::SoundSource;
use realfft::RealFftPlanner;

const N_BANDS: usize = 8;
const FFT_SIZE: usize = 4096;

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
    let (mut s0, mut s1) = (0.0_f32, 0.0_f32);
    for &x in samples {
        let s2 = s1;
        s1 = s0;
        s0 = x + coeff * s1 - s2;
    }
    (s0 * s0 + s1 * s1 - coeff * s0 * s1) / (n as f32 * n as f32)
}

fn band_energy(samples: &[f32], sample_rate: f32, f_low: f32, f_high: f32) -> f32 {
    let probes = 8;
    (0..probes)
        .map(|i| {
            let f = f_low + (f_high - f_low) * (i as f32 + 0.5) / probes as f32;
            goertzel(samples, sample_rate, f)
        })
        .sum::<f32>()
        / probes as f32
}

fn analyze(label: &str, samples: &[f32], sample_rate: f32) {
    let n = samples.len();
    if n == 0 {
        println!("\n=== {label} === (empty)");
        return;
    }
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / n as f32).sqrt();
    let peak = samples.iter().fold(0.0_f32, |m, &s| m.max(s.abs()));
    let crest = if rms > 0.0 {
        20.0 * (peak / rms).log10()
    } else {
        0.0
    };

    println!(
        "\n=== {label} ===  ({:.1}s @ {:.0} Hz)",
        n as f32 / sample_rate,
        sample_rate
    );
    println!(
        "  RMS {rms:.4} ({:+.1} dBFS)   peak {peak:.3}   crest {crest:.1} dB",
        20.0 * rms.log10()
    );

    let energies: Vec<_> = BANDS
        .iter()
        .map(|&(lo, hi, name)| (name, lo, hi, band_energy(samples, sample_rate, lo, hi)))
        .collect();
    let max_e = energies.iter().map(|e| e.3).fold(0.0_f32, f32::max);
    for &(name, lo, hi, e) in &energies {
        let rel = if max_e > 0.0 && e > 0.0 {
            10.0 * (e / max_e).log10()
        } else {
            -120.0
        };
        let bar = "█".repeat(((rel + 40.0) / 40.0 * 30.0).clamp(0.0, 30.0) as usize);
        println!("  {name} {lo:>5.0}-{hi:<5.0}Hz  {rel:>+6.1} dB  {bar}");
    }
    let (w, t) = energies.iter().fold((0.0, 0.0), |(w, t), &(_, lo, hi, e)| {
        (w + (lo + hi) / 2.0 * e, t + e)
    });
    let centroid = if t > 0.0 { w / t } else { 0.0 };
    let bright: f32 = energies.iter().filter(|e| e.1 >= 2000.0).map(|e| e.3).sum();
    println!(
        "  → centroid {centroid:.0} Hz    energy >2 kHz: {:.1}%",
        100.0 * bright / t.max(1e-12)
    );

    // Temporal dynamics: short-time RMS in 0.25 s windows.
    let win = (0.25 * sample_rate) as usize;
    if win > 0 && n >= win * 4 {
        let mut env: Vec<f32> = Vec::new();
        let mut i = 0;
        while i + win <= n {
            let e = (samples[i..i + win].iter().map(|s| s * s).sum::<f32>() / win as f32).sqrt();
            env.push(e);
            i += win;
        }
        let emax = env.iter().cloned().fold(0.0_f32, f32::max);
        let emin = env.iter().cloned().fold(f32::MAX, f32::min);
        let emean = env.iter().sum::<f32>() / env.len() as f32;
        // Depth: how far the quietest 0.25s dips below the loudest (dB).
        let depth_db = if emin > 0.0 {
            20.0 * (emax / emin).log10()
        } else {
            99.0
        };
        // Ratio of the quiet floor to the mean — how "calm" the lulls get.
        println!(
            "  gust dynamics: RMS window min {emin:.4} / mean {emean:.4} / max {emax:.4}  → swing {depth_db:.1} dB, floor/mean {:.2}",
            emin / emean.max(1e-9)
        );
    }
}

/// Fine 24-band (≈⅓-octave) spectral profile over the first 10 s, relative dB.
fn fine_profile(label: &str, samples: &[f32], sample_rate: f32) {
    let n = 24;
    let (fmin, fmax) = (40.0_f32, 16_000.0_f32);
    let seg = &samples[..samples.len().min((10.0 * sample_rate) as usize)];
    let bands: Vec<(f32, f32)> = (0..n)
        .map(|i| {
            let f = fmin * (fmax / fmin).powf(i as f32 / (n - 1) as f32);
            (f, goertzel(seg, sample_rate, f))
        })
        .collect();
    let max_e = bands.iter().map(|b| b.1).fold(0.0_f32, f32::max).max(1e-12);
    println!("\n-- fine spectral profile: {label} --");
    for (f, e) in &bands {
        let db = 10.0 * (e / max_e).max(1e-6).log10();
        let bar = "█".repeat(((db + 50.0) / 50.0 * 40.0).clamp(0.0, 40.0) as usize);
        println!("  {f:>6.0} Hz {db:>+6.1} {bar}");
    }
}

/// How the spectral centroid tracks loudness: frames binned by RMS into
/// quiet/mid/loud thirds, average centroid each. Loud > quiet ⇒ gusts brighten.
fn brightness_vs_level(label: &str, samples: &[f32], sample_rate: f32) {
    let win = (0.1 * sample_rate) as usize; // 100 ms frames
    if win == 0 {
        return;
    }
    let bands = [
        (20.0, 100.0),
        (100.0, 250.0),
        (250.0, 500.0),
        (500.0, 1000.0),
        (1000.0, 2000.0),
        (2000.0, 4000.0),
        (4000.0, 8000.0),
        (8000.0, 16000.0),
    ];
    let mut frames: Vec<(f32, f32)> = Vec::new();
    let mut i = 0;
    while i + win <= samples.len() {
        let f = &samples[i..i + win];
        let rms = (f.iter().map(|s| s * s).sum::<f32>() / win as f32).sqrt();
        let (mut w, mut t) = (0.0_f32, 0.0_f32);
        for (lo, hi) in bands {
            let e = band_energy(f, sample_rate, lo, hi);
            w += (lo + hi) / 2.0 * e;
            t += e;
        }
        let centroid = if t > 0.0 { w / t } else { 0.0 };
        frames.push((rms, centroid));
        i += win;
    }
    frames.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let third = (frames.len() / 3).max(1);
    let avg = |s: &[(f32, f32)]| s.iter().map(|x| x.1).sum::<f32>() / s.len().max(1) as f32;
    let quiet = avg(&frames[..third]);
    let loud = avg(&frames[frames.len().saturating_sub(third)..]);
    println!(
        "  brightness vs level [{label}]: quiet-third centroid {quiet:.0} Hz → loud-third {loud:.0} Hz  (gust brightening +{:.0} Hz)",
        loud - quiet
    );
}

fn percentile(values: &[f32], quantile: f32) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let index = quantile.clamp(0.0, 1.0) * (sorted.len() - 1) as f32;
    let lower = index.floor() as usize;
    let upper = index.ceil() as usize;
    sorted[lower] + (sorted[upper] - sorted[lower]) * index.fract()
}

fn correlation(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    if n < 2 {
        return 0.0;
    }
    let mean_a = a[..n].iter().sum::<f32>() / n as f32;
    let mean_b = b[..n].iter().sum::<f32>() / n as f32;
    let (mut covariance, mut variance_a, mut variance_b) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let da = a[i] - mean_a;
        let db = b[i] - mean_b;
        covariance += da * db;
        variance_a += da * da;
        variance_b += db * db;
    }
    covariance / (variance_a.sqrt() * variance_b.sqrt()).max(1e-12)
}

fn regression_residual_std(x: &[f32], y: &[f32]) -> f32 {
    let n = x.len().min(y.len());
    if n < 2 {
        return 0.0;
    }
    let mean_x = x[..n].iter().sum::<f32>() / n as f32;
    let mean_y = y[..n].iter().sum::<f32>() / n as f32;
    let (mut covariance, mut variance_x) = (0.0, 0.0);
    for i in 0..n {
        covariance += (x[i] - mean_x) * (y[i] - mean_y);
        variance_x += (x[i] - mean_x).powi(2);
    }
    let slope = covariance / variance_x.max(1e-12);
    let variance = (0..n)
        .map(|i| {
            let prediction = mean_y + slope * (x[i] - mean_x);
            (y[i] - prediction).powi(2)
        })
        .sum::<f32>()
        / n as f32;
    variance.sqrt()
}

fn modulation_summary(label: &str, values: &[f32], frame_rate: f32) {
    if values.len() < 8 {
        return;
    }
    let mean = values.iter().sum::<f32>() / values.len() as f32;
    let mut slow = 0.0_f64;
    let mut gust = 0.0_f64;
    let mut texture = 0.0_f64;
    let mut total = 0.0_f64;
    let mut dominant = (0.0_f32, 0.0_f64);

    // The control-rate sequence is short, so a direct DFT keeps this diagnostic
    // self-contained. Frequencies below 0.03 Hz are effectively DC/trend for a
    // roughly 30-second reference and are excluded.
    for k in 1..=values.len() / 2 {
        let frequency = k as f32 * frame_rate / values.len() as f32;
        if !(0.03..=5.0).contains(&frequency) {
            continue;
        }
        let mut re = 0.0_f64;
        let mut im = 0.0_f64;
        for (i, value) in values.iter().enumerate() {
            let phase = TAU as f64 * k as f64 * i as f64 / values.len() as f64;
            let centered = (*value - mean) as f64;
            re += centered * phase.cos();
            im -= centered * phase.sin();
        }
        let power = re * re + im * im;
        total += power;
        if frequency < 0.20 {
            slow += power;
        } else if frequency < 1.0 {
            gust += power;
        } else {
            texture += power;
        }
        if power > dominant.1 {
            dominant = (frequency, power);
        }
    }

    let pct = |power: f64| 100.0 * power / total.max(1e-20);
    println!(
        "  {label:<15} modulation: slow 0.03-0.20 Hz {:>5.1}% | gust 0.20-1 Hz {:>5.1}% | texture 1-5 Hz {:>5.1}% | strongest {:.2} Hz",
        pct(slow),
        pct(gust),
        pct(texture),
        dominant.0
    );
}

/// Proper short-time Fourier analysis of spectral *motion*, not merely the
/// long-term average spectrum. The residual column answers the architecture
/// question: after broadband loudness is accounted for, how much independent
/// movement remains in each frequency band?
fn spectral_motion(label: &str, samples: &[f32], sample_rate: f32) {
    let hop = (sample_rate * 0.05).round().max(1.0) as usize;
    if samples.len() < FFT_SIZE {
        return;
    }

    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);
    let mut input = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();
    let window: Vec<f32> = (0..FFT_SIZE)
        .map(|i| {
            let phase = TAU * i as f32 / (FFT_SIZE - 1) as f32;
            0.5 - 0.5 * phase.cos()
        })
        .collect();

    let mut total_db = Vec::new();
    let mut centroids = Vec::new();
    let mut bright_fraction = Vec::new();
    let mut body_air_tilt = Vec::new();
    let mut flatness = Vec::new();
    let mut band_db: [Vec<f32>; N_BANDS] = std::array::from_fn(|_| Vec::new());
    let mut band_share: [Vec<f32>; N_BANDS] = std::array::from_fn(|_| Vec::new());

    let mut position = 0;
    while position + FFT_SIZE <= samples.len() {
        for i in 0..FFT_SIZE {
            input[i] = samples[position + i] * window[i];
        }
        fft.process(&mut input, &mut spectrum).unwrap();

        let rms = (samples[position..position + FFT_SIZE]
            .iter()
            .map(|sample| sample * sample)
            .sum::<f32>()
            / FFT_SIZE as f32)
            .sqrt();
        total_db.push(20.0 * rms.max(1e-12).log10());

        let mut bands = [0.0_f32; N_BANDS];
        let mut total_power = 0.0_f32;
        let mut weighted_power = 0.0_f32;
        let mut bright_power = 0.0_f32;
        let mut log_power_sum = 0.0_f32;
        let mut flatness_power = 0.0_f32;
        let mut flatness_bins = 0;
        for (bin, value) in spectrum.iter().enumerate() {
            let frequency = bin as f32 * sample_rate / FFT_SIZE as f32;
            if !(20.0..16_000.0).contains(&frequency) {
                continue;
            }
            let power = value.norm_sqr().max(1e-20);
            total_power += power;
            weighted_power += frequency * power;
            if frequency >= 2_000.0 {
                bright_power += power;
            }
            if frequency >= 100.0 {
                log_power_sum += power.ln();
                flatness_power += power;
                flatness_bins += 1;
            }
            if let Some(index) = BANDS
                .iter()
                .position(|&(low, high, _)| frequency >= low && frequency < high)
            {
                bands[index] += power;
            }
        }

        for i in 0..N_BANDS {
            band_db[i].push(10.0 * bands[i].max(1e-20).log10());
            band_share[i].push(bands[i] / total_power.max(1e-20));
        }
        centroids.push(weighted_power / total_power.max(1e-20));
        bright_fraction.push(bright_power / total_power.max(1e-20));
        let body = bands[2] + bands[3];
        let air = bands[5] + bands[6] + bands[7];
        body_air_tilt.push(10.0 * (body / air.max(1e-20)).max(1e-20).log10());
        let arithmetic_mean = flatness_power / flatness_bins.max(1) as f32;
        let geometric_mean = (log_power_sum / flatness_bins.max(1) as f32).exp();
        flatness.push(geometric_mean / arithmetic_mean.max(1e-20));

        position += hop;
    }

    println!("\n-- STFT spectral motion: {label} --");
    println!(
        "  centroid p10/p50/p90: {:.0} / {:.0} / {:.0} Hz",
        percentile(&centroids, 0.10),
        percentile(&centroids, 0.50),
        percentile(&centroids, 0.90)
    );
    println!(
        "  >2 kHz energy p10/p50/p90: {:.1} / {:.1} / {:.1}%",
        100.0 * percentile(&bright_fraction, 0.10),
        100.0 * percentile(&bright_fraction, 0.50),
        100.0 * percentile(&bright_fraction, 0.90)
    );
    println!(
        "  body/air tilt p10/p50/p90: {:+.1} / {:+.1} / {:+.1} dB | median spectral flatness {:.3}",
        percentile(&body_air_tilt, 0.10),
        percentile(&body_air_tilt, 0.50),
        percentile(&body_air_tilt, 0.90),
        percentile(&flatness, 0.50)
    );
    println!("  band             median energy   p90-p10   corr(level)   residual after level");
    for i in 0..N_BANDS {
        let spread = percentile(&band_db[i], 0.90) - percentile(&band_db[i], 0.10);
        println!(
            "  {} {:>6.1}%      {:>6.2} dB     {:+.2}          {:>5.2} dB",
            BANDS[i].2,
            100.0 * percentile(&band_share[i], 0.50),
            spread,
            correlation(&total_db, &band_db[i]),
            regression_residual_std(&total_db, &band_db[i])
        );
    }
    println!(
        "  level↔centroid correlation {:+.2}; level↔brightness correlation {:+.2}",
        correlation(&total_db, &centroids),
        correlation(&total_db, &bright_fraction)
    );
    modulation_summary("broadband level", &total_db, sample_rate / hop as f32);
    modulation_summary("spectral tilt", &body_air_tilt, sample_rate / hop as f32);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "assets/wind_field.mp3".into());

    println!("Analyzing: {path}");
    let buffer = decode_file(Path::new(&path)).expect("decode failed");
    let sr = buffer.sample_rate as f32;
    let total_s = buffer.samples.len() as f32 / sr;
    println!(
        "Decoded {:.1}s, {} Hz, {} samples",
        total_s,
        buffer.sample_rate,
        buffer.samples.len()
    );

    // Segments: the user flagged 0-28 s and 28-54 s as insightful.
    let segments: Vec<(f32, f32)> = if args.len() >= 4 {
        args[2..]
            .chunks(2)
            .filter(|c| c.len() == 2)
            .map(|c| (c[0].parse().unwrap_or(0.0), c[1].parse().unwrap_or(total_s)))
            .collect()
    } else {
        vec![(0.0, 28.0), (28.0, 54.0)]
    };

    for (start, end) in segments {
        let a = (start * sr) as usize;
        let b = ((end * sr) as usize).min(buffer.samples.len());
        if a < b {
            analyze(
                &format!("SAMPLE {start:.0}-{end:.0}s"),
                &buffer.samples[a..b],
                sr,
            );
        }
    }

    // Generative field-wind architecture: compare a full 60-second window
    // because its weather/gust/eddy controls intentionally span multiple scales.
    let mut field_wind = FieldWindSource::new(Vec3::ZERO, 1.0, 8.0, 42);
    field_wind.set_change_time_range(20.0, 50.0);
    field_wind.set_gust_duration_range(3.0, 10.0);
    field_wind.gust_strength = 0.35;
    field_wind.rise_bias = 0.25;
    field_wind.turbulence_depth = 0.20;
    let n = (sr * 60.0) as usize;
    let out: Vec<f32> = (0..n).map(|_| field_wind.next_sample(sr)).collect();
    analyze("SYNTH field_wind 1-8 m/s", &out, sr);

    let comparison_len = buffer.samples.len().min(out.len());
    let mut shared_driver_field = FieldWindSource::new(Vec3::ZERO, 1.0, 5.0, 46);
    shared_driver_field.set_change_time_range(18.0, 45.0);
    shared_driver_field.set_gust_duration_range(3.0, 12.0);
    shared_driver_field.gust_strength = 0.18;
    shared_driver_field.rise_bias = 0.15;
    shared_driver_field.turbulence_depth = 0.12;
    let field_comparison: Vec<f32> = (0..comparison_len)
        .map(|_| shared_driver_field.next_sample(sr))
        .collect();
    let mut soft_wind = SoftWindSource::new(Vec3::ZERO, 1.0, 5.0, 46);
    let soft_comparison: Vec<f32> = (0..comparison_len)
        .map(|_| soft_wind.next_sample(sr))
        .collect();

    analyze("SYNTH soft_wind 1-5 m/s", &soft_comparison, sr);
    spectral_motion("RECORDING soft wind", &buffer.samples[..comparison_len], sr);
    spectral_motion(
        "field_wind using soft 1-5 m/s driver",
        &field_comparison,
        sr,
    );
    spectral_motion("SYNTH soft_wind", &soft_comparison, sr);

    // ── Detailed probing: fine spectrum + gust-brightness, recording vs synth ──
    let recording = &buffer.samples[..buffer.samples.len().min((40.0 * sr) as usize)];
    fine_profile("RECORDING", recording, sr);
    fine_profile("SYNTH field_wind", &out, sr);
    println!();
    brightness_vs_level("RECORDING", recording, sr);
    brightness_vs_level("SYNTH field_wind", &out, sr);

    // Per-band dynamics: do the low body and high air breathe together, or
    // independently? (Useful when tuning the synth's independently moving bands.)
    let (body_env, air_env) = atrium::synth::extract_band_envelopes(recording, sr);
    let swing = |e: &[f32]| {
        let mx = e.iter().cloned().fold(0.0_f32, f32::max);
        let mn = e.iter().cloned().fold(f32::MAX, f32::min).max(1e-6);
        20.0 * (mx / mn).log10()
    };
    let n = body_env.len().min(air_env.len());
    let (mb, ma) = (
        body_env[..n].iter().sum::<f32>() / n as f32,
        air_env[..n].iter().sum::<f32>() / n as f32,
    );
    let (mut cov, mut vb, mut va) = (0.0_f32, 0.0_f32, 0.0_f32);
    for i in 0..n {
        let (db, da) = (body_env[i] - mb, air_env[i] - ma);
        cov += db * da;
        vb += db * db;
        va += da * da;
    }
    let corr = cov / (vb.sqrt() * va.sqrt()).max(1e-9);
    println!("\n-- per-band dynamics (RECORDING) --");
    println!(
        "  body (<{:.0} Hz) swing {:.1} dB",
        atrium::synth::BAND_SPLIT_LOW_HZ,
        swing(&body_env)
    );
    println!(
        "  air  (>{:.0} Hz) swing {:.1} dB",
        atrium::synth::BAND_SPLIT_HIGH_HZ,
        swing(&air_env)
    );
    println!("  body↔air envelope correlation: {corr:.2}  (1=move together, 0=independent)");

    // Diagnostic: raw full pink slope (should be ≈-3 dB/oct if truly full-range).
    let mut pink = atrium::synth::noise::PinkNoiseFull::new(1);
    let pink_samples: Vec<f32> = (0..(10.0 * sr) as usize)
        .map(|_| pink.next_sample())
        .collect();
    fine_profile("RAW PinkNoiseFull", &pink_samples, sr);
}
