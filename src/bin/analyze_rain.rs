//! Short-time spectrum and texture analysis for rain recordings/synths.
//!
//! Unlike the retired long-window Goertzel probe, this reports the statistics
//! that distinguish rain textures: spectral motion, 5 ms impulsiveness,
//! 250 ms steadiness, modulation bands, and cross-band balance.
//!
//!   cargo run --bin analyze_rain -- "research papers/soft_rain.mp3"

use std::f32::consts::TAU;
use std::fs::File;
use std::path::Path;

use atrium::audio::decode::decode_file;
use atrium::synth::noise::{OnePoleHP, OnePoleLP};
use atrium::synth::rain::RainSource;
use atrium::synth::rain_v2::RainSourceV2;
use atrium::synth::river::RiverSource;
use atrium::world::types::Vec3;
use atrium_core::source::SoundSource;
use realfft::RealFftPlanner;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

const FFT_SIZE: usize = 4096;
const BANDS: &[(f32, f32, &str)] = &[
    (20.0, 100.0, "sub-bass  "),
    (100.0, 250.0, "bass      "),
    (250.0, 500.0, "low-mid   "),
    (500.0, 1000.0, "mid       "),
    (1000.0, 2000.0, "upper-mid "),
    (2000.0, 4000.0, "presence  "),
    (4000.0, 8000.0, "brilliance"),
    (8000.0, 16000.0, "air       "),
];

fn percentile(values: &[f32], quantile: f32) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let index = quantile.clamp(0.0, 1.0) * (sorted.len() - 1) as f32;
    let low = index.floor() as usize;
    let high = index.ceil() as usize;
    sorted[low] + (sorted[high] - sorted[low]) * index.fract()
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

fn modulation_summary(values: &[f32], frame_rate: f32) -> (f32, f32, f32, f32) {
    if values.len() < 8 {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let mean = values.iter().sum::<f32>() / values.len() as f32;
    let (mut slow, mut cluster, mut texture, mut total) = (0.0_f64, 0.0, 0.0, 0.0);
    let mut strongest = (0.0_f32, 0.0_f64);
    for k in 1..=values.len() / 2 {
        let frequency = k as f32 * frame_rate / values.len() as f32;
        if !(0.03..=8.0).contains(&frequency) {
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
        if frequency < 0.2 {
            slow += power;
        } else if frequency < 1.0 {
            cluster += power;
        } else {
            texture += power;
        }
        if power > strongest.1 {
            strongest = (frequency, power);
        }
    }
    let pct = |power: f64| (100.0 * power / total.max(1e-20)) as f32;
    (pct(slow), pct(cluster), pct(texture), strongest.0)
}

fn texture_dynamics(samples: &[f32], sample_rate: f32) {
    let window_250 = (sample_rate * 0.25).round() as usize;
    let rms_250: Vec<f32> = samples
        .chunks_exact(window_250.max(1))
        .map(|frame| (frame.iter().map(|x| x * x).sum::<f32>() / frame.len() as f32).sqrt())
        .collect();
    let swing_250 =
        20.0 * (percentile(&rms_250, 0.95) / percentile(&rms_250, 0.05).max(1e-12)).log10();

    let window_5 = (sample_rate * 0.005).round() as usize;
    let micro_crest: Vec<f32> = samples
        .chunks_exact(window_5.max(1))
        .map(|frame| {
            let rms = (frame.iter().map(|x| x * x).sum::<f32>() / frame.len() as f32).sqrt();
            let peak = frame.iter().fold(0.0_f32, |m, x| m.max(x.abs()));
            20.0 * (peak / rms.max(1e-12)).log10()
        })
        .collect();

    println!(
        "  250 ms steadiness: p05-p95 swing {swing_250:.2} dB (p05/median {:.2})",
        percentile(&rms_250, 0.05) / percentile(&rms_250, 0.50).max(1e-12)
    );
    println!(
        "  5 ms micro-crest: p50/p90/p99 {:.1}/{:.1}/{:.1} dB",
        percentile(&micro_crest, 0.50),
        percentile(&micro_crest, 0.90),
        percentile(&micro_crest, 0.99)
    );
}

fn analyze(label: &str, samples: &[f32], sample_rate: f32) {
    let rms = (samples.iter().map(|x| x * x).sum::<f32>() / samples.len().max(1) as f32).sqrt();
    let peak = samples.iter().fold(0.0_f32, |m, x| m.max(x.abs()));
    let crest = 20.0 * (peak / rms.max(1e-12)).log10();
    println!(
        "\n=== {label} ({:.1}s) ===",
        samples.len() as f32 / sample_rate
    );
    println!(
        "  RMS {:+.1} dBFS | peak {:+.1} dBFS | whole-clip crest {:.1} dB",
        20.0 * rms.max(1e-12).log10(),
        20.0 * peak.max(1e-12).log10(),
        crest
    );
    texture_dynamics(samples, sample_rate);

    if samples.len() < FFT_SIZE {
        return;
    }
    let hop = (sample_rate * 0.05).round().max(1.0) as usize;
    let window: Vec<f32> = (0..FFT_SIZE)
        .map(|i| 0.5 - 0.5 * (TAU * i as f32 / (FFT_SIZE - 1) as f32).cos())
        .collect();
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(FFT_SIZE);
    let mut input = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();

    let mut level_db = Vec::new();
    let mut centroids = Vec::new();
    let mut bright_fraction = Vec::new();
    let mut flatness = Vec::new();
    let mut flux = Vec::new();
    let mut band_share: [Vec<f32>; 8] = std::array::from_fn(|_| Vec::new());
    let mut previous_normalized = vec![0.0_f32; spectrum.len()];
    let mut position = 0;
    while position + FFT_SIZE <= samples.len() {
        for i in 0..FFT_SIZE {
            input[i] = samples[position + i] * window[i];
        }
        fft.process(&mut input, &mut spectrum).unwrap();

        let frame_rms = (samples[position..position + FFT_SIZE]
            .iter()
            .map(|x| x * x)
            .sum::<f32>()
            / FFT_SIZE as f32)
            .sqrt();
        level_db.push(20.0 * frame_rms.max(1e-12).log10());

        let mut bands = [0.0_f32; 8];
        let (mut total, mut weighted, mut bright) = (0.0_f32, 0.0_f32, 0.0_f32);
        let (mut log_sum, mut arithmetic, mut bins) = (0.0_f32, 0.0_f32, 0usize);
        for (bin, value) in spectrum.iter().enumerate() {
            let frequency = bin as f32 * sample_rate / FFT_SIZE as f32;
            if !(20.0..16_000.0).contains(&frequency) {
                continue;
            }
            let power = value.norm_sqr().max(1e-20);
            total += power;
            weighted += frequency * power;
            if frequency >= 2_000.0 {
                bright += power;
            }
            if frequency >= 100.0 {
                log_sum += power.ln();
                arithmetic += power;
                bins += 1;
            }
            if let Some(index) = BANDS
                .iter()
                .position(|(low, high, _)| frequency >= *low && frequency < *high)
            {
                bands[index] += power;
            }
        }
        centroids.push(weighted / total.max(1e-20));
        bright_fraction.push(bright / total.max(1e-20));
        let arithmetic_mean = arithmetic / bins.max(1) as f32;
        flatness.push((log_sum / bins.max(1) as f32).exp() / arithmetic_mean.max(1e-20));
        for index in 0..8 {
            band_share[index].push(bands[index] / total.max(1e-20));
        }

        let mut frame_flux = 0.0_f32;
        for (bin, value) in spectrum.iter().enumerate() {
            let normalized = value.norm_sqr() / total.max(1e-20);
            frame_flux += (normalized - previous_normalized[bin]).max(0.0);
            previous_normalized[bin] = normalized;
        }
        if !flux.is_empty() || position > 0 {
            flux.push(frame_flux);
        }
        position += hop;
    }

    println!(
        "  STFT centroid p10/p50/p90: {:.0}/{:.0}/{:.0} Hz",
        percentile(&centroids, 0.10),
        percentile(&centroids, 0.50),
        percentile(&centroids, 0.90)
    );
    println!(
        "  >2 kHz p10/p50/p90: {:.1}/{:.1}/{:.1}% | flatness p50 {:.3} | flux p50/p90 {:.3}/{:.3}",
        100.0 * percentile(&bright_fraction, 0.10),
        100.0 * percentile(&bright_fraction, 0.50),
        100.0 * percentile(&bright_fraction, 0.90),
        percentile(&flatness, 0.50),
        percentile(&flux, 0.50),
        percentile(&flux, 0.90)
    );
    print!("  median band shares:");
    for (index, (_, _, name)) in BANDS.iter().enumerate() {
        print!(
            " {name}={:.1}%",
            100.0 * percentile(&band_share[index], 0.50)
        );
    }
    println!();
    let (slow, cluster, texture, strongest) =
        modulation_summary(&level_db, sample_rate / hop as f32);
    println!(
        "  level modulation: 0.03-.2 Hz {slow:.1}% | .2-1 Hz {cluster:.1}% | 1-8 Hz {texture:.1}% | strongest {strongest:.2} Hz"
    );
    println!(
        "  level↔centroid correlation {:+.2} | level↔brightness {:+.2}",
        correlation(&level_db, &centroids),
        correlation(&level_db, &bright_fraction)
    );
}

fn render(source: &mut dyn SoundSource, sample_rate: f32, sample_count: usize) -> Vec<f32> {
    (0..sample_count)
        .map(|_| source.next_sample(sample_rate))
        .collect()
}

/// Long-term fractional-octave PSD. Each value is mean power/bin inside a
/// 1/6-octave band, so widening bands do not receive an artificial HF boost.
fn fine_spectral_envelope(samples: &[f32], sample_rate: f32) -> Vec<(f32, f32)> {
    const N: usize = 8192;
    const HOP: usize = 2048;
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(N);
    let mut input = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();
    let window: Vec<f32> = (0..N)
        .map(|i| 0.5 - 0.5 * (TAU * i as f32 / (N - 1) as f32).cos())
        .collect();
    let mut psd = vec![0.0_f64; spectrum.len()];
    let mut frames = 0usize;
    let mut position = 0usize;
    while position + N <= samples.len() {
        for i in 0..N {
            input[i] = samples[position + i] * window[i];
        }
        fft.process(&mut input, &mut spectrum).unwrap();
        for (bin, value) in spectrum.iter().enumerate() {
            psd[bin] += value.norm_sqr() as f64;
        }
        frames += 1;
        position += HOP;
    }
    for power in &mut psd {
        *power /= frames.max(1) as f64;
    }

    let mut bands = Vec::new();
    let mut center = 125.0_f32;
    let half_band = 2.0_f32.powf(1.0 / 12.0);
    while center <= 16_000.0 {
        let low = center / half_band;
        let high = center * half_band;
        let first = ((low / sample_rate) * N as f32).ceil() as usize;
        let last = ((high / sample_rate) * N as f32).floor() as usize;
        let slice = &psd[first.min(psd.len() - 1)..=last.min(psd.len() - 1)];
        let mean = slice.iter().sum::<f64>() / slice.len().max(1) as f64;
        bands.push((center, 10.0 * mean.max(1e-30).log10() as f32));
        center *= 2.0_f32.powf(1.0 / 6.0);
    }
    let max_db = bands
        .iter()
        .map(|(_, db)| *db)
        .fold(f32::NEG_INFINITY, f32::max);
    for (_, db) in &mut bands {
        *db -= max_db;
    }
    bands
}

fn print_fine_spectrum_comparison(reference: &[f32], synth: &[f32], sample_rate: f32) {
    let reference = fine_spectral_envelope(reference, sample_rate);
    let synth = fine_spectral_envelope(synth, sample_rate);
    println!("\n=== HIGH-RESOLUTION 1/6-OCTAVE PSD (relative dB/bin) ===");
    println!("  center       reference       river   synth-ref");
    for ((frequency, reference_db), (_, synth_db)) in reference.iter().zip(&synth) {
        println!(
            "  {:>6.0} Hz    {:>+7.2} dB  {:>+7.2} dB   {:>+7.2} dB",
            frequency,
            reference_db,
            synth_db,
            synth_db - reference_db
        );
    }

    let contrast = |bands: &[(f32, f32)]| {
        let values: Vec<f32> = bands.iter().map(|(_, db)| *db).collect();
        percentile(&values, 0.90) - percentile(&values, 0.10)
    };
    println!(
        "  envelope contrast p90-p10: reference {:.2} dB | river {:.2} dB",
        contrast(&reference),
        contrast(&synth)
    );
}

#[derive(Clone, Copy)]
struct OnsetStats {
    rate_hz: f32,
    novelty_p90: f32,
    novelty_p99: f32,
    decay_ms: f32,
}

/// Detect millisecond-scale contacts relative to a 100 ms adaptive floor.
/// This does not claim to count physical drops; it quantifies how many audible
/// edges survive the aggregate texture and how quickly they return to it.
fn onset_stats(samples: &[f32], sample_rate: f32) -> OnsetStats {
    let frame = (sample_rate * 0.001).round().max(1.0) as usize;
    let frame_rate = sample_rate / frame as f32;
    let floor_alpha = (-1.0 / (0.100 * frame_rate)).exp();
    let mut floor = 0.0_f32;
    let mut novelty = Vec::new();
    for chunk in samples.chunks_exact(frame) {
        let energy = chunk.iter().map(|sample| sample * sample).sum::<f32>() / frame as f32;
        if floor == 0.0 {
            floor = energy.max(1e-12);
        }
        // Measure against the preceding floor; only then let this frame move
        // the baseline. Updating first erases the very onsets being measured.
        novelty.push(10.0 * (energy / floor.max(1e-12)).max(1e-12).log10());
        floor = floor_alpha * floor + (1.0 - floor_alpha) * energy;
    }

    let refractory = (0.004 * frame_rate).round() as usize;
    let mut last = None;
    let mut peaks = Vec::new();
    let mut decays = Vec::new();
    for index in 1..novelty.len().saturating_sub(1) {
        if novelty[index] >= 0.25
            && novelty[index] > novelty[index - 1]
            && novelty[index] >= novelty[index + 1]
            && last.is_none_or(|last| index.saturating_sub(last) >= refractory)
        {
            peaks.push(novelty[index]);
            last = Some(index);
            let end = (index + (0.050 * frame_rate) as usize).min(novelty.len());
            let decay_frames = novelty[index..end]
                .iter()
                .position(|value| *value <= 1.0)
                .unwrap_or(end - index);
            decays.push(1000.0 * decay_frames as f32 / frame_rate);
        }
    }
    OnsetStats {
        rate_hz: peaks.len() as f32 / (samples.len() as f32 / sample_rate),
        novelty_p90: percentile(&novelty, 0.90),
        novelty_p99: percentile(&novelty, 0.99),
        decay_ms: percentile(&decays, 0.50),
    }
}

fn print_onset_comparison(reference: &[f32], synth: &[f32], sample_rate: f32) {
    let broadband_reference = onset_stats(reference, sample_rate);
    let broadband_synth = onset_stats(synth, sample_rate);

    let highpassed = |samples: &[f32]| {
        let mut highpass = OnePoleHP::new(2_000.0, sample_rate);
        samples
            .iter()
            .map(|sample| highpass.process(*sample))
            .collect::<Vec<_>>()
    };
    let high_reference = highpassed(reference);
    let high_synth = highpassed(synth);
    let high_reference = onset_stats(&high_reference, sample_rate);
    let high_synth = onset_stats(&high_synth, sample_rate);

    let row = |label: &str, reference: OnsetStats, synth: OnsetStats| {
        println!(
            "  {label:<10} events/s {:>5.1}/{:>5.1} | novelty p90 {:>4.1}/{:>4.1} dB p99 {:>4.1}/{:>4.1} dB | median decay {:>4.1}/{:>4.1} ms",
            reference.rate_hz,
            synth.rate_hz,
            reference.novelty_p90,
            synth.novelty_p90,
            reference.novelty_p99,
            synth.novelty_p99,
            reference.decay_ms,
            synth.decay_ms,
        );
    };
    println!("\n=== MILLISSECOND CONTACT CLARITY (reference/river) ===");
    row("broadband", broadband_reference, broadband_synth);
    row(">2 kHz", high_reference, high_synth);
}

fn print_contact_ablation(bed_only: &[f32], complete: &[f32], sample_rate: f32) {
    let bed_stats = onset_stats(bed_only, sample_rate);
    let complete_stats = onset_stats(complete, sample_rate);
    let rms_db = |samples: &[f32]| {
        let rms = (samples.iter().map(|sample| sample * sample).sum::<f32>()
            / samples.len().max(1) as f32)
            .sqrt();
        20.0 * rms.max(1e-12).log10()
    };
    println!("\n=== RIVER EVENT ABLATION (continuous/full) ===");
    println!(
        "  broadband novelty p90 {:>4.1}/{:>4.1} dB | p99 {:>4.1}/{:>4.1} dB | local peaks/s {:>5.1}/{:>5.1}",
        bed_stats.novelty_p90,
        complete_stats.novelty_p90,
        bed_stats.novelty_p99,
        complete_stats.novelty_p99,
        bed_stats.rate_hz,
        complete_stats.rate_hz,
    );
    println!(
        "  total RMS bed/full {:+.1}/{:+.1} dBFS",
        rms_db(bed_only),
        rms_db(complete),
    );
}

struct BandTexture {
    envelope: Vec<f32>,
    cv: f32,
    skewness: f32,
    kurtosis: f32,
    p99_over_median_db: f32,
}

/// Broad cochlear-like channel followed by 2 ms RMS envelope sampling. The
/// marginal moments distinguish sparse contact fields from Gaussian noise even
/// when their long-term spectra are identical.
fn band_texture(samples: &[f32], sample_rate: f32, low: f32, high: f32) -> BandTexture {
    let mut hp = OnePoleHP::new(low, sample_rate);
    let mut hp2 = OnePoleHP::new(low, sample_rate);
    let mut lp = OnePoleLP::new(high, sample_rate);
    let mut lp2 = OnePoleLP::new(high, sample_rate);
    let frame = (sample_rate * 0.002).round().max(1.0) as usize;
    let mut envelope = Vec::with_capacity(samples.len() / frame);
    let (mut energy, mut count) = (0.0_f32, 0usize);
    for sample in samples {
        let filtered = lp2.process(lp.process(hp2.process(hp.process(*sample))));
        energy += filtered * filtered;
        count += 1;
        if count == frame {
            envelope.push((energy / frame as f32).sqrt());
            energy = 0.0;
            count = 0;
        }
    }
    let mean = envelope.iter().sum::<f32>() / envelope.len().max(1) as f32;
    let variance = envelope
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f32>()
        / envelope.len().max(1) as f32;
    let deviation = variance.sqrt().max(1e-12);
    let skewness = envelope
        .iter()
        .map(|value| ((value - mean) / deviation).powi(3))
        .sum::<f32>()
        / envelope.len().max(1) as f32;
    let kurtosis = envelope
        .iter()
        .map(|value| ((value - mean) / deviation).powi(4))
        .sum::<f32>()
        / envelope.len().max(1) as f32;
    let p99_over_median_db =
        20.0 * (percentile(&envelope, 0.99) / percentile(&envelope, 0.50).max(1e-12)).log10();
    BandTexture {
        envelope,
        cv: deviation / mean.max(1e-12),
        skewness,
        kurtosis,
        p99_over_median_db,
    }
}

fn print_cochlear_texture(reference: &[f32], synth: &[f32], sample_rate: f32) {
    const CHANNELS: &[(f32, f32, &str)] = &[
        (350.0, 1_400.0, "body"),
        (1_400.0, 4_000.0, "presence"),
        (4_000.0, 8_000.0, "brilliance"),
        (8_000.0, 16_000.0, "air"),
    ];
    let reference_bands: Vec<_> = CHANNELS
        .iter()
        .map(|(low, high, _)| band_texture(reference, sample_rate, *low, *high))
        .collect();
    let synth_bands: Vec<_> = CHANNELS
        .iter()
        .map(|(low, high, _)| band_texture(synth, sample_rate, *low, *high))
        .collect();
    println!("\n=== 2 MS COCHLEAR-ENVELOPE TEXTURE (reference/river) ===");
    for (index, (_, _, label)) in CHANNELS.iter().enumerate() {
        let reference = &reference_bands[index];
        let synth = &synth_bands[index];
        println!(
            "  {label:<10} CV {:.2}/{:.2} | skew {:.2}/{:.2} | kurtosis {:.1}/{:.1} | p99/median {:.1}/{:.1} dB",
            reference.cv,
            synth.cv,
            reference.skewness,
            synth.skewness,
            reference.kurtosis,
            synth.kurtosis,
            reference.p99_over_median_db,
            synth.p99_over_median_db,
        );
    }
    println!("  zero-lag log-envelope correlations:");
    for left in 0..CHANNELS.len() {
        for right in left + 1..CHANNELS.len() {
            let log_envelope = |band: &BandTexture| {
                band.envelope
                    .iter()
                    .map(|value| value.max(1e-12).ln())
                    .collect::<Vec<_>>()
            };
            let reference_left = log_envelope(&reference_bands[left]);
            let reference_right = log_envelope(&reference_bands[right]);
            let synth_left = log_envelope(&synth_bands[left]);
            let synth_right = log_envelope(&synth_bands[right]);
            println!(
                "    {:<10}↔{:<10} {:+.2}/{:+.2}",
                CHANNELS[left].2,
                CHANNELS[right].2,
                correlation(&reference_left, &reference_right),
                correlation(&synth_left, &synth_right),
            );
        }
    }
}

fn decode_channels(path: &Path) -> Result<(Vec<Vec<f32>>, f32), Box<dyn std::error::Error>> {
    let file = File::open(path)?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
        hint.with_extension(extension);
    }
    let probed = symphonia::default::get_probe().format(
        &hint,
        stream,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|track| track.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or("no audio track")?;
    let track_id = track.id;
    let channel_count = track.codec_params.channels.map(|c| c.count()).unwrap_or(1);
    let sample_rate = track.codec_params.sample_rate.unwrap_or(48_000) as f32;
    let mut decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;
    let mut channels = vec![Vec::new(); channel_count];
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(symphonia::core::errors::Error::IoError(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(error) => return Err(Box::new(error)),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = decoder.decode(&packet)?;
        let spec = *decoded.spec();
        let mut samples = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        samples.copy_interleaved_ref(decoded);
        for frame in samples.samples().chunks(channel_count) {
            for (channel, sample) in frame.iter().enumerate() {
                channels[channel].push(*sample);
            }
        }
    }
    Ok((channels, sample_rate))
}

fn print_stereo_analysis(path: &Path) {
    let Ok((channels, sample_rate)) = decode_channels(path) else {
        return;
    };
    if channels.len() < 2 {
        println!("\n=== STEREO STRUCTURE === mono recording");
        return;
    }
    let sample_count = channels[0].len().min(channels[1].len());
    let left = &channels[0][..sample_count];
    let right = &channels[1][..sample_count];
    let mut mid_energy = 0.0_f64;
    let mut side_energy = 0.0_f64;
    for index in 0..sample_count {
        let mid = 0.5 * (left[index] + right[index]);
        let side = 0.5 * (left[index] - right[index]);
        mid_energy += (mid * mid) as f64;
        side_energy += (side * side) as f64;
    }
    println!("\n=== STEREO STRUCTURE OF REFERENCE ===");
    println!(
        "  L↔R waveform correlation {:+.3} | side/mid power {:+.1} dB | {:.0} Hz",
        correlation(left, right),
        10.0 * (side_energy / mid_energy.max(1e-30)).log10(),
        sample_rate
    );

    const N: usize = 4096;
    const HOP: usize = 1024;
    let window: Vec<f32> = (0..N)
        .map(|i| 0.5 - 0.5 * (TAU * i as f32 / (N - 1) as f32).cos())
        .collect();
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(N);
    let mut left_input = fft.make_input_vec();
    let mut right_input = fft.make_input_vec();
    let mut left_spectrum = fft.make_output_vec();
    let mut right_spectrum = fft.make_output_vec();
    let mut left_power = [0.0_f64; BANDS.len()];
    let mut right_power = [0.0_f64; BANDS.len()];
    let mut mid_power = [0.0_f64; BANDS.len()];
    let mut side_power = [0.0_f64; BANDS.len()];
    let mut cross_power = [0.0_f64; BANDS.len()];
    let mut position = 0;
    while position + N <= sample_count {
        for i in 0..N {
            left_input[i] = left[position + i] * window[i];
            right_input[i] = right[position + i] * window[i];
        }
        fft.process(&mut left_input, &mut left_spectrum).unwrap();
        fft.process(&mut right_input, &mut right_spectrum).unwrap();
        for bin in 1..left_spectrum.len() {
            let frequency = bin as f32 * sample_rate / N as f32;
            let Some(band) = BANDS
                .iter()
                .position(|(low, high, _)| (*low..*high).contains(&frequency))
            else {
                continue;
            };
            let l = left_spectrum[bin];
            let r = right_spectrum[bin];
            let mid = (l + r) * 0.5;
            let side = (l - r) * 0.5;
            left_power[band] += l.norm_sqr() as f64;
            right_power[band] += r.norm_sqr() as f64;
            mid_power[band] += mid.norm_sqr() as f64;
            side_power[band] += side.norm_sqr() as f64;
            cross_power[band] += (l * r.conj()).re as f64;
        }
        position += HOP;
    }
    println!("  frequency-dependent width:");
    for (index, (_, _, label)) in BANDS.iter().enumerate() {
        let correlation =
            cross_power[index] / (left_power[index].sqrt() * right_power[index].sqrt()).max(1e-30);
        let side_mid_db = 10.0
            * (side_power[index] / mid_power[index].max(1e-30))
                .max(1e-30)
                .log10();
        println!("    {label} correlation {correlation:+.3} | side/mid {side_mid_db:+.1} dB");
    }
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "research papers/soft_rain.mp3".into());
    let path = Path::new(&path);
    let recording = decode_file(path).expect("rain reference should decode");
    let sample_rate = recording.sample_rate as f32;
    let sample_count = recording.samples.len();

    analyze("REFERENCE recording", &recording.samples, sample_rate);
    print_stereo_analysis(path);

    let mut v1 = RainSource::new(Vec3::ZERO, 0.5, 42);
    let mut v2 = RainSourceV2::new(Vec3::ZERO, 0.5, 42);
    let mut river = RiverSource::new(Vec3::ZERO, 0.3, 1.2, 42);
    let mut river_bed = RiverSource::new(Vec3::ZERO, 0.3, 1.2, 42);
    river_bed.bubble_activity = 0.0;
    river_bed.splash_rate = 0.0;
    let v1 = render(&mut v1, sample_rate, sample_count);
    let v2 = render(&mut v2, sample_rate, sample_count);
    let river = render(&mut river, sample_rate, sample_count);
    let river_bed = render(&mut river_bed, sample_rate, sample_count);
    analyze("CURRENT rain v1 (intensity .5)", &v1, sample_rate);
    analyze("CURRENT rain v2 (intensity .5)", &v2, sample_rate);
    analyze("RIVER (0.3-1.2 m/s)", &river, sample_rate);
    print_fine_spectrum_comparison(&recording.samples, &river, sample_rate);
    print_onset_comparison(&recording.samples, &river, sample_rate);
    print_contact_ablation(&river_bed, &river, sample_rate);
    print_cochlear_texture(&recording.samples, &river, sample_rate);
}
