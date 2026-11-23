use atrium_core::commands::SynthParam;
use atrium_core::source::SoundSource;

use super::*;
use crate::synth::test_support::{largest_step, peak_frequency, render};

const SAMPLE_RATE: f32 = 48_000.0;

/// The thirteen pieces of `docs/binaural-beats.md`: name, beat, end beat.
/// Every one has 108 Hz on the left.
const PIECES: [(&str, f32, Option<f32>); 13] = [
    ("Golden Ratio Delta Beat", 1.618, None),
    ("Delta Binaural Beat", 2.0, None),
    ("Falling Delta Session", 3.5, Some(1.5)),
    ("Low Theta Binaural Beat", 4.0, None),
    ("Theta Binaural Beat", 6.0, None),
    ("Falling Theta Session", 7.0, Some(5.0)),
    ("Schumann Resonance", 7.83, None),
    ("Alpha Binaural Beat", 10.0, None),
    ("Falling Alpha Session", 12.0, Some(9.0)),
    ("Rising Beta Session", 16.0, Some(24.0)),
    ("Beta Binaural Beat", 20.0, None),
    ("Rising Gamma Session", 32.0, Some(40.0)),
    ("Gamma Binaural Beat", 40.0, None),
];

fn piece(beat_hz: f32, end_beat_hz: Option<f32>, duration_seconds: f32) -> BinauralBeatSettings {
    BinauralBeatSettings {
        carrier_hz: 108.0,
        beat_hz,
        end_beat_hz,
        duration_seconds,
        master_gain: 1.0,
    }
}

/// `seconds` of both ears' tones, the left and the right.
fn render_pair(settings: BinauralBeatSettings, seconds: f32) -> (Vec<f32>, Vec<f32>) {
    let (mut left, mut right) = BinauralToneSource::pair(settings);
    (
        render(seconds, SAMPLE_RATE, |rate| left.next_sample(rate)),
        render(seconds, SAMPLE_RATE, |rate| right.next_sample(rate)),
    )
}

/// The samples from `from` to `to` seconds.
fn stretch(samples: &[f32], from: f64, to: f64) -> &[f32] {
    let index = |seconds: f64| (seconds * SAMPLE_RATE as f64).round() as usize;
    &samples[index(from)..index(to).min(samples.len())]
}

/// Phase of the component at `frequency_hz` (radians): a Hann-windowed
/// single-bin DFT, the phase it has at the stretch's centre.
fn phase_at(samples: &[f32], frequency_hz: f64) -> f64 {
    use std::f64::consts::TAU;
    let length = samples.len();
    let step = TAU * frequency_hz / SAMPLE_RATE as f64;
    let centre = 0.5 * (length - 1) as f64;
    let (mut real, mut imaginary) = (0.0, 0.0);
    for (index, &sample) in samples.iter().enumerate() {
        let window = 0.5 - 0.5 * (TAU * index as f64 / length as f64).cos();
        let angle = step * (index as f64 - centre);
        real += window * sample as f64 * angle.cos();
        imaginary -= window * sample as f64 * angle.sin();
    }
    imaginary.atan2(real)
}

/// The frequency of the one tone in `samples`, from the FFT's peak between
/// `low_hz` and `high_hz` refined by how far its phase turns from the first
/// half of the stretch to the second: a tone `δ` Hz off the peak's frequency
/// turns `2π δ T` further over the `T` seconds between the halves' centres.
/// For a tone that sweeps linearly, the frequency at the stretch's centre.
fn measured_frequency(samples: &[f32], low_hz: f64, high_hz: f64) -> f64 {
    use std::f64::consts::{PI, TAU};
    let coarse = peak_frequency(samples, SAMPLE_RATE, low_hz, high_hz);
    let half = samples.len() / 2;
    let between_centres = half as f64 / SAMPLE_RATE as f64;
    let turned = phase_at(&samples[half..2 * half], coarse)
        - phase_at(&samples[..half], coarse)
        - TAU * coarse * between_centres;
    let wrapped = (turned + PI).rem_euclid(TAU) - PI;
    coarse + wrapped / (TAU * between_centres)
}

/// Each ear's tone, measured: one 20 s FFT (0.05 Hz bins) of each ear after
/// its fade-in, its peak refined by the phase turned between the two halves.
/// The left ear is on 108 Hz and the right on 108 Hz plus the beat, for a
/// slow, a middling and the fastest beat, to a ten-thousandth of a hertz;
/// the beat measured, right minus left, is the beat set. (The settings are
/// `f32`: 1.618 Hz is 1.61800003 Hz.)
#[test]
fn each_ear_plays_its_own_frequency() {
    for beat_hz in [1.618_f32, 10.0, 40.0] {
        let (left, right) = render_pair(piece(beat_hz, None, NINE_HOURS_SECONDS), 23.0);
        let left_hz = measured_frequency(stretch(&left, 3.0, 23.0), 100.0, 160.0);
        let right_hz = measured_frequency(stretch(&right, 3.0, 23.0), 100.0, 160.0);
        let expected_right = 108.0 + beat_hz as f64;
        println!(
            "beat {beat_hz} Hz: left {left_hz:.6} Hz, right {right_hz:.6} Hz \
             (set {expected_right:.6}), beat measured {:.6} Hz",
            right_hz - left_hz
        );
        assert!((left_hz - 108.0).abs() < 1e-4, "left {left_hz}");
        assert!(
            (right_hz - expected_right).abs() < 1e-4,
            "right {right_hz}, set {expected_right}"
        );
        assert!(
            (right_hz - left_hz - beat_hz as f64).abs() < 1e-4,
            "beat {}",
            right_hz - left_hz
        );
    }
}

/// The right ear's tone is the left's plus the beat, exactly: at the start,
/// the middle and the end of every piece, and after it.
#[test]
fn the_beat_is_the_right_tone_minus_the_left() {
    for (name, beat_hz, end_beat_hz) in PIECES {
        let settings = piece(beat_hz, end_beat_hz, NINE_HOURS_SECONDS);
        for elapsed in [0.0, 16_200.0, 32_400.0, 40_000.0] {
            let left = settings.ear_frequency(Ear::Left, elapsed);
            let right = settings.ear_frequency(Ear::Right, elapsed);
            assert_eq!(left, 108.0, "{name}");
            assert_eq!(
                right - left,
                settings.beat_at(elapsed),
                "{name} at {elapsed} s"
            );
        }
        assert_eq!(
            settings.beat_at(0.0),
            beat_hz as f64,
            "{name} starts on its beat"
        );
        let end = end_beat_hz.unwrap_or(beat_hz) as f64;
        assert!(
            (settings.beat_at(32_400.0) - end).abs() < 1e-12,
            "{name} ends on {end}"
        );
    }
}

/// A session moves its beat in a straight line over its duration and holds
/// it after: 7 to 5 Hz over two minutes is 6 Hz at one minute. Measured from
/// both ears' tones, the beat in 4 s just after the fade-in, in the middle
/// and just before the fade-out is the line's beat at the stretch's centre
/// (a linear sweep's spectrum peaks at its middle frequency), falling and
/// rising sessions alike.
#[test]
fn a_session_moves_its_beat_in_a_straight_line() {
    let falling = piece(7.0, Some(5.0), 120.0);
    assert_eq!(falling.beat_at(0.0), 7.0);
    assert_eq!(falling.beat_at(30.0), 6.5);
    assert_eq!(falling.beat_at(60.0), 6.0);
    assert_eq!(falling.beat_at(120.0), 5.0);
    assert_eq!(falling.beat_at(500.0), 5.0, "held after its end");

    for settings in [falling, piece(16.0, Some(24.0), 120.0)] {
        let (left, right) = render_pair(settings, 120.0);
        // The fade-in ends at 2 s, the fade-out starts at 100 s.
        for (from, to, place) in [
            (2.0, 6.0, "start"),
            (58.0, 62.0, "middle"),
            (96.0, 100.0, "end"),
        ] {
            let centre = 0.5 * (from + to);
            let expected = settings.beat_at(centre);
            let left_hz = measured_frequency(stretch(&left, from, to), 100.0, 160.0);
            let right_hz = measured_frequency(stretch(&right, from, to), 100.0, 160.0);
            let measured = right_hz - left_hz;
            println!(
                "{} to {} Hz over 120 s, {place} ({centre} s): beat {measured:.5} Hz, \
                 the line {expected:.5} Hz (left {left_hz:.5} Hz)",
                settings.beat_hz,
                settings.end_beat_hz.unwrap()
            );
            assert!((left_hz - 108.0).abs() < 1e-4, "{place}: left {left_hz}");
            assert!(
                (measured - expected).abs() < 1e-4,
                "{place}: beat {measured}, the line {expected}"
            );
        }
    }
}

/// The envelope: up from silence over the 2 s fade-in, whole, down over the
/// last 20 s to silence at its duration, and silent after. A piece shorter
/// than 80 s fades over a quarter of its duration at most.
#[test]
fn the_envelope_fades_in_holds_and_fades_out() {
    let nine_hours = piece(10.0, None, NINE_HOURS_SECONDS);
    assert_eq!(nine_hours.envelope_at(0.0), 0.0);
    assert!((nine_hours.envelope_at(1.0) - 0.5).abs() < 1e-12);
    assert_eq!(nine_hours.envelope_at(2.0), 1.0);
    assert_eq!(nine_hours.envelope_at(16_200.0), 1.0);
    assert_eq!(nine_hours.envelope_at(32_380.0), 1.0);
    assert!((nine_hours.envelope_at(32_390.0) - 0.5).abs() < 1e-9);
    assert_eq!(nine_hours.envelope_at(32_400.0), 0.0);
    assert_eq!(nine_hours.envelope_at(40_000.0), 0.0);

    let four_seconds = piece(10.0, None, 4.0);
    assert!(
        (four_seconds.envelope_at(0.5) - 0.5).abs() < 1e-12,
        "a 1 s fade-in"
    );
    assert_eq!(four_seconds.envelope_at(2.0), 1.0);
    assert!(
        (four_seconds.envelope_at(3.5) - 0.5).abs() < 1e-12,
        "a 1 s fade-out"
    );
}

/// At its end it has faded to silence and stays silent: rendered 40 s past
/// a 12 s piece, every sample from its end on is exactly 0, in both ears; it
/// does not start again, not even when given a longer duration afterwards.
/// Across the fade-out its level only falls.
#[test]
fn it_fades_out_and_stops_at_its_end() {
    let settings = piece(10.0, None, 12.0);
    let (mut left, mut right) = BinauralToneSource::pair(settings);
    for (ear, tone) in [("left", &mut left), ("right", &mut right)] {
        let samples = render(52.0, SAMPLE_RATE, |rate| tone.next_sample(rate));
        // The fade-out is the last 3 s (a quarter of 12 s).
        let full = largest_step(stretch(&samples, 6.0, 8.0));
        let peaks: Vec<f32> = (0..30)
            .map(|tenth| {
                let from = 9.0 + tenth as f64 * 0.1;
                stretch(&samples, from, from + 0.1)
                    .iter()
                    .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
            })
            .collect();
        assert!(
            peaks.windows(2).all(|pair| pair[1] <= pair[0] * 1.001),
            "{ear}: the fade-out rises: {peaks:?}"
        );
        let last_sound = samples
            .iter()
            .rposition(|sample| *sample != 0.0)
            .expect("it played");
        let silent_from = (last_sound + 1) as f64 / SAMPLE_RATE as f64;
        println!(
            "{ear}: largest step at full level {full:.6}; silent from {silent_from:.3} s \
             (the piece is 12 s), {:.1} s of silence after",
            52.0 - silent_from
        );
        assert!(full > 0.0);
        assert!(
            silent_from > 11.99 && silent_from <= 12.0,
            "{ear}: {silent_from}"
        );
        assert!(
            stretch(&samples, 12.0, 52.0)
                .iter()
                .all(|sample| *sample == 0.0),
            "{ear} sounds again"
        );
        tone.set_synth_param(SynthParam::Duration, 600.0);
        let after = render(5.0, SAMPLE_RATE, |rate| tone.next_sample(rate));
        assert!(
            after.iter().all(|sample| *sample == 0.0),
            "{ear} started again"
        );
    }
}

/// It starts from silence and ends in silence without a step larger than the
/// tone's own largest step at full level, in either ear.
#[test]
fn it_starts_and_ends_without_a_click() {
    let (left, right) = render_pair(piece(40.0, None, 20.0), 22.0);
    for (ear, samples) in [("left", &left), ("right", &right)] {
        let full = largest_step(stretch(samples, 6.0, 10.0));
        let start = largest_step(stretch(samples, 0.0, 3.0));
        let end = largest_step(stretch(samples, 13.0, 22.0));
        let first = samples[0].abs();
        println!(
            "{ear}: first sample {first}, largest step starting {start:.6}, \
             ending {end:.6}, at full level {full:.6}"
        );
        assert_eq!(first, 0.0, "{ear} starts in silence");
        assert!(start <= full, "{ear}: start {start}, full {full}");
        assert!(end <= full, "{ear}: end {end}, full {full}");
    }
}

/// A change while it plays glides: the largest step in the second around
/// the change is no larger than the tone's own largest step before or after
/// it (2 % for rounding). The references are the tone left as it was and one
/// set to the new value from its start, both past their fade-in. Ending it
/// at once, with a duration shorter than it has played, fades it out over
/// the glide; making a fixed beat a session glides it onto the session's
/// line.
#[test]
fn a_change_while_it_plays_steps_no_more_than_the_tone_itself() {
    let before = piece(10.0, None, NINE_HOURS_SECONDS);
    let changes = [
        (SynthParam::Pitch, 216.0),
        (SynthParam::Pitch, 54.0),
        (SynthParam::Beat, 40.0),
        (SynthParam::Beat, 0.0),
        (SynthParam::EndBeat, 30.0),
        (SynthParam::Duration, 3.0),
        (SynthParam::MasterGain, 2.0),
        (SynthParam::MasterGain, 0.0),
    ];
    let change_at = 4.0;
    for ear in [Ear::Left, Ear::Right] {
        for (param, value) in changes {
            let mut changed = BinauralToneSource::new(ear, before);
            let mut samples = render(change_at as f32, SAMPLE_RATE, |rate| {
                changed.next_sample(rate)
            });
            changed.set_synth_param(param, value);
            samples.extend(render(2.0, SAMPLE_RATE, |rate| changed.next_sample(rate)));

            let mut unchanged = BinauralToneSource::new(ear, before);
            let reference_before = render(6.0, SAMPLE_RATE, |rate| unchanged.next_sample(rate));
            let mut after = BinauralToneSource::new(ear, before);
            after.set_synth_param(param, value);
            let reference_after = render(6.0, SAMPLE_RATE, |rate| after.next_sample(rate));

            let window =
                |samples: &[f32]| largest_step(stretch(samples, change_at - 0.5, change_at + 0.5));
            let step = window(&samples);
            let own = window(&reference_before).max(window(&reference_after));
            println!("{ear:?} ear, {param:?} to {value}: step {step:.6}, the tone's own {own:.6}");
            assert!(
                step <= own * 1.02,
                "{ear:?} ear, {param:?} to {value}: step {step:.6}, the tone's own {own:.6}"
            );
        }
    }
}

/// The step test above would catch a click: the same gain change applied at
/// once, near a crest, steps far more than the tone does.
#[test]
fn without_the_glide_a_gain_change_would_step() {
    let (samples, _) = render_pair(piece(10.0, None, NINE_HOURS_SECONDS), 4.0);
    let own = largest_step(stretch(&samples, 3.0, 4.0));
    let crest = stretch(&samples, 3.0, 4.0)
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    // Doubling at once, on the crest, steps by the crest itself.
    assert!(crest > 10.0 * own, "crest {crest}, own step {own}");
}

/// The same settings give the same samples, bit for bit, in each ear; and
/// the two ears differ.
#[test]
fn the_same_settings_give_the_same_samples() {
    let settings = piece(3.5, Some(1.5), 60.0);
    let (first_left, first_right) = render_pair(settings, 8.0);
    let (second_left, second_right) = render_pair(settings, 8.0);
    let bits = |samples: &[f32]| {
        samples
            .iter()
            .map(|sample| sample.to_bits())
            .collect::<Vec<_>>()
    };
    assert_eq!(bits(&first_left), bits(&second_left));
    assert_eq!(bits(&first_right), bits(&second_right));
    assert_ne!(bits(&first_left), bits(&first_right));
}

/// Each tone is held 12 cm out to its own side of the head.
#[test]
fn each_tone_is_held_beside_its_own_ear() {
    let (left, right) = BinauralToneSource::pair(BinauralBeatSettings::default());
    assert_eq!(left.beside_head(), Some(Vec3::new(0.0, 0.12, 0.0)));
    assert_eq!(right.beside_head(), Some(Vec3::new(0.0, -0.12, 0.0)));
}

/// Out-of-range settings are clamped, non-finite ones and the settings of
/// other generators ignored: a 1 Hz carrier plays 20 Hz, a negative beat 0,
/// a zero duration one second.
#[test]
fn settings_are_clamped_and_others_ignored() {
    let mut tone = BinauralToneSource::new(Ear::Right, BinauralBeatSettings::default());
    tone.set_synth_param(SynthParam::Pitch, 1.0);
    tone.set_synth_param(SynthParam::Beat, -5.0);
    tone.set_synth_param(SynthParam::Duration, 0.0);
    tone.set_synth_param(SynthParam::MasterGain, 9.0);
    assert_eq!(tone.settings.carrier_hz, 20.0);
    assert_eq!(tone.settings.beat_hz, 0.0);
    assert_eq!(tone.settings.duration_seconds, 1.0);
    assert_eq!(tone.settings.master_gain, 2.0);
    tone.set_synth_param(SynthParam::EndBeat, 500.0);
    assert_eq!(tone.settings.end_beat_hz, Some(100.0));

    let mut reference = BinauralToneSource::new(Ear::Right, BinauralBeatSettings::default());
    let mut ignoring = BinauralToneSource::new(Ear::Right, BinauralBeatSettings::default());
    ignoring.set_synth_param(SynthParam::Floor, 300.0);
    ignoring.set_synth_param(SynthParam::Intensity, 1.0);
    ignoring.set_synth_param(SynthParam::Pitch, f32::NAN);
    ignoring.set_synth_param(SynthParam::Beat, f32::INFINITY);
    for _ in 0..48_000 {
        assert_eq!(
            ignoring.next_sample(SAMPLE_RATE),
            reference.next_sample(SAMPLE_RATE)
        );
    }
}
