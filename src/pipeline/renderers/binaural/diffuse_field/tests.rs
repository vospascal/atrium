use super::ear_filters::{
    design_ear_filters, design_grid_size, echo_delay_seconds, sound_and_echo_for_coherence,
};
use super::inverse::{
    design_equaliser, held_smoothed_power, regularisation, HELD_ABOVE_HZ, HELD_BELOW_HZ,
    LARGEST_BOOST_DB,
};
use super::statistics::DiffuseStatistics;
use super::*;
use realfft::RealFftPlanner;

// ── Hand-derived values ───────────────────────────────────────────────

/// a² + b² = 1 keeps each ear's power; a² − b² is the ears' coherence. So
/// identical ears have no echo, unrelated ears an echo as strong as the
/// sound, and opposite ears nothing but the echo.
#[test]
fn sound_and_echo_shares_give_the_coherence_at_unchanged_power() {
    assert_eq!(sound_and_echo_for_coherence(1.0), (1.0, 0.0));
    assert_eq!(sound_and_echo_for_coherence(-1.0), (0.0, 1.0));
    let (sound, echo) = sound_and_echo_for_coherence(0.0);
    assert!((sound - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
    assert!((echo - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
    for coherence in [-0.4, -0.2, 0.05, 0.3, 0.5, 0.67, 0.85, 0.94] {
        let (sound, echo) = sound_and_echo_for_coherence(coherence);
        assert!((sound * sound + echo * echo - 1.0).abs() < 1e-12);
        assert!((sound * sound - echo * echo - coherence).abs() < 1e-12);
    }
}

/// The delay is 1.2 ms plus two periods of the third octave's width,
/// 2 / (0.231563 f), and never more than 30 ms.
#[test]
fn echo_delay_is_a_margin_plus_two_turns_of_a_third_octave() {
    for (frequency, expected_ms) in [
        (1000.0, 1.2 + 2000.0 / 231.563),
        (4000.0, 1.2 + 2000.0 / 926.252),
        (8000.0, 1.2 + 2000.0 / 1852.504),
        (100.0, 30.0),
        (20.0, 30.0),
    ] {
        let delay_ms = echo_delay_seconds(frequency) * 1000.0;
        assert!(
            (delay_ms - expected_ms).abs() < 1e-3,
            "{frequency} Hz: {delay_ms} ms, expected {expected_ms}"
        );
    }
}

// ── The design on a head whose answer is known ───────────────────────

/// A head that hears every frequency at unit power in both ears, with the
/// ears' coherence `coherence` at every frequency.
fn flat_head(coherence: f64, sample_rate: f32) -> DiffuseStatistics {
    let bins = design_grid_size(sample_rate) / 2 + 1;
    DiffuseStatistics {
        left_power: vec![1.0; bins],
        right_power: vec![1.0; bins],
        cross_power: vec![coherence; bins],
    }
}

/// Per third octave from 400 Hz to 12.7 kHz: the coherence (largest over
/// ±1 ms of shift, as the ear compares) and each ear's level in dB.
fn third_octave_measures(
    left: &[f32],
    right: &[f32],
    sample_rate: f32,
) -> Vec<(f64, f64, f64, f64)> {
    let size = 1 << 16;
    let mut planner = RealFftPlanner::<f64>::new();
    let forward = planner.plan_fft_forward(size);
    let spectrum_of = |response: &[f32]| {
        let mut time = forward.make_input_vec();
        for (slot, &sample) in time.iter_mut().zip(response) {
            *slot = sample as f64;
        }
        let mut spectrum = forward.make_output_vec();
        forward.process(&mut time, &mut spectrum).expect("forward");
        spectrum
    };
    let left = spectrum_of(left);
    let right = spectrum_of(right);
    let bin_hz = sample_rate as f64 / size as f64;
    let largest_lag = (0.001 * sample_rate as f64).round() as i64;
    (-4..=11)
        .map(|band| {
            let centre = 1000.0 * 2f64.powf(band as f64 / 3.0);
            let lowest = (centre * 2f64.powf(-1.0 / 6.0) / bin_hz).ceil() as usize;
            let highest = (centre * 2f64.powf(1.0 / 6.0) / bin_hz).floor() as usize;
            let left_power: f64 = left[lowest..=highest]
                .iter()
                .map(|bin| bin.norm_sqr())
                .sum();
            let right_power: f64 = right[lowest..=highest]
                .iter()
                .map(|bin| bin.norm_sqr())
                .sum();
            let coherence = (-largest_lag..=largest_lag)
                .map(|lag| {
                    (lowest..=highest)
                        .map(|bin| {
                            let shift = realfft::num_complex::Complex::from_polar(
                                1.0,
                                std::f64::consts::TAU * bin as f64 * lag as f64 / size as f64,
                            );
                            left[bin] * right[bin].conj() * shift
                        })
                        .sum::<realfft::num_complex::Complex<f64>>()
                        .norm()
                })
                .fold(0f64, f64::max)
                / (left_power * right_power).sqrt();
            let count = (highest - lowest + 1) as f64;
            (
                centre,
                coherence,
                10.0 * (left_power / count).log10(),
                10.0 * (right_power / count).log10(),
            )
        })
        .collect()
}

/// On a flat head the filters must keep both ears flat in every third
/// octave and give the coherence asked for: from 400 Hz up the echo delay
/// turns over at least twice across each.
#[test]
fn a_flat_head_keeps_its_colour_and_gets_the_coherence_asked_for() {
    let sample_rate = 48_000.0;
    for wanted in [0.0, 0.5, -0.2] {
        let filters = design_ear_filters(&flat_head(wanted, sample_rate), sample_rate).as_measured;
        for (centre, coherence, left_db, right_db) in
            third_octave_measures(&filters.left, &filters.right, sample_rate)
        {
            assert!(
                left_db.abs() < 1.0 && right_db.abs() < 1.0,
                "coherence {wanted}, {centre:.0} Hz: left {left_db:+.2} dB, right {right_db:+.2} dB"
            );
            assert!(
                (coherence - wanted.abs()).abs() < 0.15,
                "coherence {wanted}, {centre:.0} Hz: measured {coherence:.3}"
            );
        }
    }
}

/// With the ears asked to be identical there are no echoes: both filters
/// are the same, a millisecond's lead-in and nothing else.
#[test]
fn identical_ears_get_identical_filters() {
    let sample_rate = 48_000.0;
    let filters = design_ear_filters(&flat_head(1.0, sample_rate), sample_rate).as_measured;
    assert_eq!(filters.left.len(), 2880, "60 ms at 48 kHz");
    let strongest = filters
        .left
        .iter()
        .enumerate()
        .fold((0, 0f32), |best, (index, &value)| {
            if value.abs() > best.1.abs() {
                (index, value)
            } else {
                best
            }
        });
    assert_eq!(strongest.0, 48, "the lead-in is 1 ms");
    assert!(
        (strongest.1 - 1.0).abs() < 1e-3,
        "unit gain: {}",
        strongest.1
    );
    for (left, right) in filters.left.iter().zip(&filters.right) {
        assert!((left - right).abs() < 1e-6);
    }
}

// ── At work ──────────────────────────────────────────────────────────

fn shipped_head(sample_rate: f32) -> DiffuseField {
    let path = crate::pipeline::PipelineParams::default().hrtf_path;
    let sofa = sofar::reader::OpenOptions::new()
        .sample_rate(sample_rate)
        .open(&path)
        .expect("the shipped HRIRs");
    DiffuseField::new(&path, &sofa, sample_rate)
}

/// When nothing ambient plays, nothing is computed and nothing is added;
/// after the last ambient callback the filters ring out over their 60 ms
/// and then stop.
#[test]
fn silence_costs_nothing_and_a_sound_rings_out_then_stops() {
    let sample_rate = 48_000.0;
    let mut field = shipped_head(sample_rate);
    let frames = 512;
    let mut buffer = vec![0.25f32; frames * 2];
    let silent_bus = vec![0.0f32; frames];
    let render = |field: &mut DiffuseField, bus: &[f32], fed: bool, buffer: &mut [f32]| {
        let mut out = OutputBuffer {
            buffer,
            reverb_send: None,
            channels: 2,
            num_frames: frames,
            sample_rate,
        };
        field.render(bus, fed, EqualisedShare::All, &mut out);
    };
    render(&mut field, &silent_bus, false, &mut buffer);
    assert!(buffer.iter().all(|&sample| sample == 0.25), "nothing added");
    assert_eq!(field.ringing_frames, 0);

    let mut click_bus = vec![0.0f32; frames];
    click_bus[0] = 1.0;
    buffer.fill(0.0);
    render(&mut field, &click_bus, true, &mut buffer);
    assert!(buffer.iter().any(|&sample| sample != 0.0));
    let mut callbacks_ringing = 0;
    while field.ringing_frames > 0 {
        buffer.fill(0.0);
        render(&mut field, &silent_bus, false, &mut buffer);
        callbacks_ringing += 1;
    }
    // 2880 frames of filter after the first callback's 512: six more.
    assert_eq!(callbacks_ringing, 6);
    buffer.fill(0.25);
    render(&mut field, &silent_bus, false, &mut buffer);
    assert!(buffer.iter().all(|&sample| sample == 0.25), "rung out");
}

/// Leaving the colour out is all that tells the two pairs apart: on a head
/// that has none, they are the same filters.
#[test]
fn on_a_flat_head_the_equalised_ears_are_the_measured_ones() {
    let sample_rate = 48_000.0;
    let filters = design_ear_filters(&flat_head(0.3, sample_rate), sample_rate);
    for (measured, equalised) in [
        (&filters.as_measured.left, &filters.equalised.left),
        (&filters.as_measured.right, &filters.equalised.right),
    ] {
        let worst = measured
            .iter()
            .zip(equalised.iter())
            .fold(0f32, |worst, (a, b)| worst.max((a - b).abs()));
        assert!(worst < 1e-6, "the pairs differ by {worst}");
    }
}

// ── The equaliser ────────────────────────────────────────────────────

/// An equaliser's power response on a fine grid, and the grid's spacing.
fn fine_power(response: &[f32], sample_rate: f32) -> (Vec<f64>, f64) {
    let size = 1 << 16;
    let mut planner = RealFftPlanner::<f64>::new();
    let forward = planner.plan_fft_forward(size);
    let mut time = forward.make_input_vec();
    for (slot, &sample) in time.iter_mut().zip(response) {
        *slot = sample as f64;
    }
    let mut spectrum = forward.make_output_vec();
    forward.process(&mut time, &mut spectrum).expect("forward");
    (
        spectrum.iter().map(|bin| bin.norm_sqr()).collect(),
        sample_rate as f64 / size as f64,
    )
}

/// An equaliser's mean gain in dB from `lowest` to `highest` Hz.
fn band_gain_db(response: &[f32], sample_rate: f32, lowest: f64, highest: f64) -> f64 {
    let (power, bin_hz) = fine_power(response, sample_rate);
    let first = (lowest / bin_hz).ceil() as usize;
    let last = ((highest / bin_hz).floor() as usize).min(power.len() - 1);
    10.0 * (power[first..=last].iter().sum::<f64>() / (last - first + 1) as f64).log10()
}

/// An equaliser's gain in dB at each of `frequencies`, averaged over the
/// third octave around each, as the design smooths what it inverts.
fn third_octave_gain_db(response: &[f32], sample_rate: f32, frequencies: &[f64]) -> Vec<f64> {
    frequencies
        .iter()
        .map(|&centre| {
            band_gain_db(
                response,
                sample_rate,
                centre * 2f64.powf(-1.0 / 6.0),
                centre * 2f64.powf(1.0 / 6.0),
            )
        })
        .collect()
}

/// Third-octave centres from `lowest` to `highest`: 1000 · 2^(k/3) Hz.
fn third_octaves(lowest: i32, highest: i32) -> Vec<f64> {
    (lowest..=highest)
        .map(|band| 1000.0 * 2f64.powf(band as f64 / 3.0))
        .collect()
}

/// A head that hears every frequency at power p is equalised by 1/√p,
/// less what the regularisation takes. Its typical power is p, so
/// β = p / (4 · 10²) and the gain is √p / (p + β) = (1/√p) / 1.0025:
/// −0.0217 dB from the exact inverse, at every frequency.
#[test]
fn a_flat_head_is_equalised_by_its_inverse_gain() {
    let sample_rate = 48_000.0;
    let power = 0.04;
    let bins = design_grid_size(sample_rate) / 2 + 1;
    let head = DiffuseStatistics {
        left_power: vec![power; bins],
        right_power: vec![power; bins],
        cross_power: vec![0.0; bins],
    };
    let equaliser = design_equaliser(&head, sample_rate, design_grid_size(sample_rate));
    let expected = -10.0 * power.log10() - 20.0 * 1.0025f64.log10();
    let frequencies = third_octaves(-14, 13);
    for response in [&equaliser.left, &equaliser.right] {
        for (frequency, gain) in
            frequencies
                .iter()
                .zip(third_octave_gain_db(response, sample_rate, &frequencies))
        {
            assert!(
                (gain - expected).abs() < 0.05,
                "{frequency:.0} Hz: {gain:.3} dB, expected {expected:.3} dB"
            );
        }
    }
}

/// Below 100 Hz and above 16 kHz the correction is held at its value
/// there: a head whose power keeps falling below the one and above the
/// other is equalised exactly as if it had stopped.
#[test]
fn the_correction_is_held_outside_the_trusted_range() {
    let sample_rate = 48_000.0;
    let grid_size = design_grid_size(sample_rate);
    let bin_hz = sample_rate as f64 / grid_size as f64;
    // Falling 12 dB an octave below 200 Hz and above 8 kHz, flat between.
    let power: Vec<f64> = (0..grid_size / 2 + 1)
        .map(|bin| {
            let frequency = (bin as f64 * bin_hz).max(1.0);
            let below = (frequency / 200.0).min(1.0).powi(4);
            let above = (8000.0 / frequency).min(1.0).powi(4);
            below * above
        })
        .collect();
    let held = held_smoothed_power(&power, bin_hz);
    let at = |frequency: f64| held[(frequency / bin_hz).round() as usize];
    for frequency in [20.0, 50.0, 80.0] {
        assert_eq!(at(frequency), at(HELD_BELOW_HZ), "{frequency} Hz");
    }
    for frequency in [17_000.0, 20_000.0, 23_000.0] {
        assert_eq!(at(frequency), at(HELD_ABOVE_HZ), "{frequency} Hz");
    }
    let head = DiffuseStatistics {
        left_power: power.clone(),
        right_power: power,
        cross_power: vec![0.0; grid_size / 2 + 1],
    };
    let equaliser = design_equaliser(&head, sample_rate, grid_size);
    // Third octaves wholly below 100 Hz and wholly above 16 kHz.
    let below = third_octave_gain_db(&equaliser.left, sample_rate, &[40.0, 50.0, 63.0, 80.0]);
    let above = third_octave_gain_db(&equaliser.left, sample_rate, &[18_000.0, 20_000.0]);
    for (frequency, gain) in [40.0, 50.0, 63.0].iter().zip(&below) {
        assert!(
            (gain - below[3]).abs() < 0.25,
            "{frequency} Hz: {gain:.2} dB against {:.2} dB at 80 Hz",
            below[3]
        );
    }
    assert!(
        (above[1] - above[0]).abs() < 0.25,
        "20 kHz: {:.2} dB against {:.2} dB at 18 kHz",
        above[1],
        above[0]
    );
}

/// However deep a hole in what is inverted, the equaliser never lifts any
/// frequency more than 20 dB above its typical gain. A flat head with the
/// third octave at 1 kHz `depth` dB down would need `depth` dB there; the
/// regularised inverse √P / (P + β) is largest, 1/(2√β), where P = β, and
/// falls again below that. Over holes from 10 to 60 dB the largest lift
/// reaches the cap and does not pass it by more than the 20 ms filter's
/// ripple at a hole with walls this steep: 0.3 dB, allowed 0.5.
#[test]
fn no_boost_exceeds_the_cap() {
    let sample_rate = 48_000.0;
    let grid_size = design_grid_size(sample_rate);
    let bin_hz = sample_rate as f64 / grid_size as f64;
    let mut closest = f64::MAX;
    for depth in (10..=60).step_by(2).map(f64::from) {
        let power: Vec<f64> = (0..grid_size / 2 + 1)
            .map(|bin| {
                let frequency = bin as f64 * bin_hz;
                if (850.0..1180.0).contains(&frequency) {
                    10f64.powf(-depth / 10.0)
                } else {
                    1.0
                }
            })
            .collect();
        let head = DiffuseStatistics {
            left_power: power.clone(),
            right_power: power,
            cross_power: vec![0.0; grid_size / 2 + 1],
        };
        let equaliser = design_equaliser(&head, sample_rate, grid_size);
        // 23 third octaves from 100 Hz to 16 kHz, one `depth` down: the
        // typical power is 10^(−depth/230), the typical gain depth/23 dB.
        let cap = depth / 23.0 + LARGEST_BOOST_DB;
        let (power, bin_hz) = fine_power(&equaliser.left, sample_rate);
        let largest = power[(500.0 / bin_hz) as usize..(2000.0 / bin_hz) as usize]
            .iter()
            .fold(0f64, |peak, &value| peak.max(10.0 * value.log10()));
        assert!(
            largest <= cap + 0.5,
            "a hole {depth} dB deep is lifted {largest:.2} dB, the cap is {cap:.2} dB"
        );
        closest = closest.min(cap - largest);
    }
    assert!(
        closest < 0.5,
        "the cap is never reached: {closest:.2} dB short"
    );
}

/// On the shipped head the equaliser inverts the head's own diffuse-field
/// response, as the renderer measures it, to within 1 dB in every third
/// octave from 100 Hz to 12.5 kHz; both ears alike to 0.05 dB. From 14.3 to
/// 16 kHz, the trusted half of the next third octave, it is −1.07 dB, and
/// 1.25 dB is allowed there: the set falls 9 dB across that third octave,
/// and a third-octave power average of so steep a slope lies 0.8 dB above
/// its middle, so the smoothed inverse falls short by about that much. The
/// failure lists the equaliser's gain and the product at every band.
#[test]
fn the_shipped_equaliser_inverts_the_heads_diffuse_field() {
    let sample_rate = 48_000.0;
    let path = crate::pipeline::PipelineParams::default().hrtf_path;
    let sofa = sofar::reader::OpenOptions::new()
        .sample_rate(sample_rate)
        .open(&path)
        .expect("the shipped HRIRs");
    let grid_size = design_grid_size(sample_rate);
    let statistics = DiffuseStatistics::measure(&sofa, sample_rate, grid_size);
    let equaliser = design_equaliser(&statistics, sample_rate, grid_size);
    let frequencies = third_octaves(-10, 12);
    let bin_hz = sample_rate as f64 / grid_size as f64;
    let regularisation = regularisation(
        [
            &held_smoothed_power(&statistics.left_power, bin_hz),
            &held_smoothed_power(&statistics.right_power, bin_hz),
        ],
        bin_hz,
    );
    let band_db = |power: &[f64], centre: f64| {
        let lowest = (centre * 2f64.powf(-1.0 / 6.0) / bin_hz).ceil() as usize;
        let highest = (centre * 2f64.powf(1.0 / 6.0) / bin_hz).floor() as usize;
        10.0 * (power[lowest..=highest].iter().sum::<f64>() / (highest - lowest + 1) as f64).log10()
    };
    let left_gain = third_octave_gain_db(&equaliser.left, sample_rate, &frequencies);
    let right_gain = third_octave_gain_db(&equaliser.right, sample_rate, &frequencies);
    let mut report =
        String::from("band Hz | equaliser left dB | right | diffuse dB | product dB\n");
    let mut worst_product = 0f64;
    let mut worst_between_ears = 0f64;
    for (index, &centre) in frequencies.iter().enumerate() {
        let diffuse = band_db(&statistics.left_power, centre);
        let product = left_gain[index] + diffuse;
        report.push_str(&format!(
            "{centre:7.0} | {:+.2} | {:+.2} | {diffuse:+.2} | {product:+.2}\n",
            left_gain[index], right_gain[index]
        ));
        // The third octave at 16 kHz is half above the hold, where nothing
        // is inverted; it is reported, and its trusted half asserted below.
        if centre < HELD_ABOVE_HZ * 0.95 {
            worst_product = worst_product
                .max(product.abs())
                .max((right_gain[index] + band_db(&statistics.right_power, centre)).abs());
        }
        worst_between_ears = worst_between_ears.max((left_gain[index] - right_gain[index]).abs());
    }
    let (boost_index, boost) =
        left_gain
            .iter()
            .enumerate()
            .fold((0, f64::MIN), |best, (index, &gain)| {
                if gain > best.1 {
                    (index, gain)
                } else {
                    best
                }
            });
    let (cut_index, cut) =
        left_gain
            .iter()
            .enumerate()
            .fold((0, f64::MAX), |best, (index, &gain)| {
                if gain < best.1 {
                    (index, gain)
                } else {
                    best
                }
            });
    // The trusted half of the 16 kHz third octave, 14.3 to 16 kHz.
    let half_band_low = HELD_ABOVE_HZ * 2f64.powf(-1.0 / 6.0);
    let trusted_half = band_gain_db(&equaliser.left, sample_rate, half_band_low, HELD_ABOVE_HZ);
    let (first, last) = (
        (half_band_low / bin_hz).ceil() as usize,
        (HELD_ABOVE_HZ / bin_hz).floor() as usize,
    );
    let trusted_half_product = trusted_half
        + 10.0
            * (statistics.left_power[first..=last].iter().sum::<f64>() / (last - first + 1) as f64)
                .log10();
    // What the regularisation takes from the exact inverse, at most, in the
    // trusted range: 10 log10 of (1 + β/P)².
    let held = held_smoothed_power(&statistics.left_power, bin_hz);
    let (lowest, highest) = (
        (HELD_BELOW_HZ / bin_hz).ceil() as usize,
        (HELD_ABOVE_HZ / bin_hz).floor() as usize,
    );
    let regularised_by = held[lowest..=highest]
        .iter()
        .map(|&power| 20.0 * (1.0 + regularisation / power).log10())
        .fold(0f64, f64::max);
    let outside = third_octave_gain_db(&equaliser.left, sample_rate, &[50.0, 20_000.0]);
    report.push_str(&format!(
        "held: {:+.2} dB at 50 Hz, {:+.2} dB at 20 kHz\n",
        outside[0], outside[1]
    ));
    report.push_str(&format!(
        "largest boost {boost:+.2} dB at {:.0} Hz, largest cut {cut:+.2} dB at {:.0} Hz; worst product {worst_product:.2} dB to 12.5 kHz, {trusted_half_product:+.2} dB from 14.3 to 16 kHz; ears apart by {worst_between_ears:.3} dB; the regularisation takes at most {regularised_by:.2} dB",
        frequencies[boost_index], frequencies[cut_index]
    ));
    eprintln!("{report}");
    assert!(worst_product <= 1.0, "{report}");
    assert!(trusted_half_product.abs() <= 1.25, "{report}");
    assert!(worst_between_ears <= 0.05, "{report}");
}
