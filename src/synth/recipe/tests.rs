use super::*;
use crate::synth::test_support::{amplitude_at, largest_step, render, root_mean_square};
use atrium_core::layers::LayerSetting;
use atrium_recipe::RecipeDef;

const RATE: f32 = 48_000.0;
/// D5 at A4 = 440 Hz.
const D5: f64 = 587.329_5;

fn recipe(yaml: &str) -> Recipe {
    serde_yaml::from_str::<RecipeDef>(yaml)
        .expect("parses")
        .resolve()
        .expect("valid")
}

fn source(yaml: &str, seed: u64) -> RecipeSource {
    RecipeSource::new(Vec3::ZERO, &recipe(yaml), 1.0, seed)
}

fn stretch(samples: &[f32], from: f32, to: f32) -> &[f32] {
    &samples[(from * RATE) as usize..(to * RATE) as usize]
}

/// The recipe of Astral Ambience "Calm", as `sources/recipes/calm.yaml` has it.
pub(crate) const CALM: &str = "
chords:
  - { name: Am7, notes: [A2, E3, G3, C4] }
  - { name: D, notes: [D2, A2, D3, A3, D4], colour: [E4, F4] }
  - { name: C, notes: [G2, C3, G3, C4], colour: [D4, F4] }
start: Am7
change: { Am7: { D: 1 }, D: { C: 1 }, C: { D: 1 } }
hold: { Am7: 12 s, D: 12..14 s, C: 10..12 s }
swell: { in: 2..6 s, out: 5..24 s, stagger: 0..3 s }
voice: { voices: 5, detune: 27 cents, drift: 2..11 cents, drift_over: 0.5..3 s,
         wander: 8, wander_over: 8..20 s }
";

/// A3 (220 Hz) as a pair 27 cents either side, nothing drifting: lines at
/// 220 · 2^(∓27/1200) = 216.6 and 223.5 Hz, and next to nothing at 220.
#[test]
fn a_pair_sits_either_side_of_its_note() {
    let mut pad = source(
        "chords: [{ name: A, notes: [A3] }]\nhold: 100 s\n\
         voice: { voices: 2, detune: 27 cents, drift: 0, wander: 0, brightness: 0 }",
        1,
    );
    let samples = render(4.0, RATE, |rate| pad.next_sample(rate));
    let steady = stretch(&samples, 1.0, 4.0);
    let low = amplitude_at(steady, RATE, 220.0 * 2f64.powf(-27.0 / 1200.0));
    let high = amplitude_at(steady, RATE, 220.0 * 2f64.powf(27.0 / 1200.0));
    let middle = amplitude_at(steady, RATE, 220.0);
    assert!(
        low > 10.0 * middle && high > 10.0 * middle,
        "{low} {high} {middle}"
    );
    assert!((low / high - 1.0).abs() < 0.1, "{low} {high}");
}

/// Two chords taking turns every 6 s, swelling in over 1 s and out over 1 s:
/// A3 alone at 4.5–5.5 s (after the piece's 4 s fade-in), D5 alone at
/// 8–9 s, both halfway through the change.
/// (D5, 587.3 Hz, is not a harmonic of A3's timbre, as A4 would be.)
#[test]
fn chords_swell_in_and_out_on_their_turns() {
    let mut pad = source(
        "chords: [{ name: low, notes: [A3] }, { name: high, notes: [D5] }]\n\
         change: alternate\nhold: 6 s\nswell: { in: 1 s, out: 1 s }\n\
         voice: { voices: 1, drift: 0, wander: 0, brightness: 0 }",
        1,
    );
    let samples = render(10.0, RATE, |rate| pad.next_sample(rate));
    let level = |from, to, frequency| amplitude_at(stretch(&samples, from, to), RATE, frequency);
    let full = level(4.5, 5.5, 220.0);
    assert!(level(4.5, 5.5, D5) < 0.01 * full);
    let high_full = level(8.0, 9.0, D5);
    assert!((high_full / full - 1.0).abs() < 0.05, "{high_full} {full}");
    assert!(level(8.0, 9.0, 220.0) < 0.01 * full);
    // At 6.5 s each is halfway through its raised cosine: both near half.
    let low_halfway = level(6.4, 6.6, 220.0) / full;
    let high_halfway = level(6.4, 6.6, D5) / full;
    assert!((0.3..0.7).contains(&low_halfway), "{low_halfway}");
    assert!((0.3..0.7).contains(&high_halfway), "{high_halfway}");
}

#[test]
fn it_starts_without_a_click() {
    let mut pad = source(CALM, 3);
    let samples = render(1.0, RATE, |rate| pad.next_sample(rate));
    let loudest = samples
        .iter()
        .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
    assert!(
        largest_step(&samples[..(0.1 * RATE) as usize]) < 0.002,
        "{}",
        largest_step(&samples)
    );
    assert!(loudest > 0.0);
}

#[test]
fn the_same_seed_gives_the_same_samples() {
    let take = |seed| {
        let mut pad = source(CALM, seed);
        render(2.0, RATE, |rate| pad.next_sample(rate))
    };
    assert_eq!(take(4), take(4));
    assert_ne!(take(4), take(5));
}

/// Calm's chords change, its notes swell 20–30 dB, but the whole holds still
/// (measured 1.7 dB standard deviation): over two minutes the level of each
/// 2 s varies by less than 2.5 dB (standard deviation), once its 4 s fade-in
/// is over.
#[test]
fn calm_holds_its_level_while_its_chords_change() {
    let mut pad = source(CALM, 7);
    let samples = render(120.0, RATE, |rate| pad.next_sample(rate));
    let levels: Vec<f64> = samples[(6.0 * RATE) as usize..]
        .chunks((2.0 * RATE) as usize)
        .map(|chunk| 20.0 * root_mean_square(chunk).log10())
        .collect();
    let mean = levels.iter().sum::<f64>() / levels.len() as f64;
    let spread = (levels
        .iter()
        .map(|level| (level - mean).powi(2))
        .sum::<f64>()
        / levels.len() as f64)
        .sqrt();
    assert!(spread < 2.5, "{spread} dB");
}

/// Five voices on A3 spread across ±27 cents: lines at −27, −13.5, 0, +13.5
/// and +27 cents, each about as loud, and little half-way between two.
#[test]
fn five_voices_spread_evenly_across_the_detune() {
    let mut pad = source(
        "chords: [{ name: A, notes: [A3] }]\nhold: 100 s\n\
         voice: { voices: 5, detune: 27 cents, drift: 0, wander: 0, brightness: 0 }",
        1,
    );
    let samples = render(9.0, RATE, |rate| pad.next_sample(rate));
    let steady = stretch(&samples, 5.0, 9.0);
    let at = |cents: f64| amplitude_at(steady, RATE, 220.0 * 2f64.powf(cents / 1200.0));
    let lines: Vec<f64> = [-27.0, -13.5, 0.0, 13.5, 27.0]
        .iter()
        .map(|cents| at(*cents))
        .collect();
    let quietest = lines.iter().cloned().fold(f64::INFINITY, f64::min);
    let loudest = lines.iter().cloned().fold(0.0, f64::max);
    assert!(loudest / quietest < 1.2, "{lines:?}");
    assert!(
        at(-20.25) < 0.2 * quietest,
        "{} against {quietest}",
        at(-20.25)
    );
}

/// A chord whose three notes may each begin up to 6 s late blooms note by
/// note: the times its notes reach half their level lie more than a second
/// apart. (A3, D#4 and B4: no harmonic of one is another.)
#[test]
fn a_staggered_chord_blooms_note_by_note() {
    let mut pad = source(
        "chords: [{ name: X, notes: [A3, D#4, B4] }]\nhold: 100 s\n\
         swell: { in: 1 s, stagger: 0..6 s }\n\
         voice: { voices: 1, drift: 0, wander: 0, brightness: 0 }",
        11,
    );
    let samples = render(12.0, RATE, |rate| pad.next_sample(rate));
    let half_level_at = |frequency: f64| {
        let full = amplitude_at(stretch(&samples, 10.0, 12.0), RATE, frequency);
        (0..40)
            .map(|step| step as f32 * 0.25)
            .find(|from| {
                amplitude_at(stretch(&samples, *from, from + 0.25), RATE, frequency) > 0.5 * full
            })
            .expect("it begins")
    };
    let times = [
        half_level_at(220.0),
        half_level_at(311.127),
        half_level_at(493.883),
    ];
    let earliest = times.iter().cloned().fold(f32::INFINITY, f32::min);
    let latest = times.iter().cloned().fold(0.0, f32::max);
    assert!(latest - earliest > 1.0, "{times:?}");
}

/// A 400 Hz cutoff: a 2-pole Butterworth leaves 110 Hz all but untouched
/// (−0.03 dB) and takes C6 (1046.5 Hz, 2.62 times the corner) down by
/// 10·log10(1 + 2.62⁴) = 16.7 dB.
#[test]
fn a_cutoff_rolls_off_twelve_decibels_an_octave() {
    let yaml = |cutoff: &str| {
        format!(
            "chords: [{{ name: X, notes: [A2, C6] }}]\nhold: 100 s\n\
             voice: {{ voices: 1, drift: 0, wander: 0, brightness: 0{cutoff} }}"
        )
    };
    let level = |yaml: &str, frequency: f64| {
        let mut pad = source(yaml, 1);
        let samples = render(9.0, RATE, |rate| pad.next_sample(rate));
        amplitude_at(stretch(&samples, 5.0, 9.0), RATE, frequency)
    };
    let (open, warm) = (yaml(""), yaml(", cutoff: 400 Hz"));
    let low = 20.0 * (level(&warm, 110.0) / level(&open, 110.0)).log10();
    let high = 20.0 * (level(&warm, 1046.502) / level(&open, 1046.502)).log10();
    assert!(low.abs() < 0.2, "{low} dB at 110 Hz");
    assert!((high + 16.7).abs() < 0.5, "{high} dB at C6");
}

/// A ping on a chord of D3 alone, its range E4 to D5: the note moved two
/// octaves up into it, D5 (587.3 Hz).
#[test]
fn a_ping_strikes_the_chords_note_in_its_range() {
    let mut pad = source(
        "chords: [{ name: D, notes: [D3] }]\nhold: 100 s\n\
         voice: { voices: 1, drift: 0, wander: 0, brightness: 0, cutoff: 100 Hz }\n\
         pings: [{ range: [E4, D5], every: 6 s, run: 1, ring: 0.5 s, level: 0 }]",
        1,
    );
    let samples = render(8.0, RATE, |rate| pad.next_sample(rate));
    // The first run comes 6 s in; the chord, under a 100 Hz cutoff, is far below.
    let struck = stretch(&samples, 6.0, 6.4);
    let pitch = crate::synth::test_support::peak_frequency(struck, RATE, 400.0, 900.0);
    assert!((pitch - 587.33).abs() < 3.0, "{pitch}");
    let before = amplitude_at(stretch(&samples, 5.5, 5.9), RATE, 587.33);
    let after = amplitude_at(stretch(&samples, 6.0, 6.4), RATE, 587.33);
    assert!(after > 10.0 * before, "{before} {after}");
}

/// A swoosh in 1500–4000 Hz: while it swells, the band holds far more than
/// the octave above it (more than 10 times: two-pole skirts leave 8–16 kHz
/// 12–24 dB down over a span three times wider), and before it there is next
/// to nothing.
#[test]
fn a_swoosh_fills_its_band() {
    let mut pad = source(
        "chords: [{ name: D, notes: [D3] }]\nhold: 100 s\n\
         voice: { voices: 1, drift: 0, wander: 0, brightness: 0, cutoff: 100 Hz }\n\
         swoosh: { every: 5 s, rise: 1 s, fall: 1 s, band: 1500..4000 Hz, level: 0 }",
        1,
    );
    let samples = render(8.0, RATE, |rate| pad.next_sample(rate));
    let spectrum = crate::synth::test_support::averaged_power_spectrum(stretch(&samples, 5.6, 6.4));
    let band = crate::synth::test_support::band_power(&spectrum, RATE, 1500.0, 4000.0);
    let above = crate::synth::test_support::band_power(&spectrum, RATE, 8000.0, 16000.0);
    assert!(band > 10.0 * above, "{band} {above}");
    let quiet = crate::synth::test_support::averaged_power_spectrum(stretch(&samples, 4.0, 4.9));
    let before = crate::synth::test_support::band_power(&quiet, RATE, 1500.0, 4000.0);
    assert!(band > 1000.0 * before, "{band} {before}");
}

/// A chord's own colour level: −20 dB under against the recipe's −6. (B4, not
/// A4: A4 is all but D3's third harmonic, 440.5 Hz.)
#[test]
fn a_chord_may_set_its_own_colour_level() {
    let level = |chord_level: &str| {
        let mut pad = source(
            &format!(
                "chords: [{{ name: D, notes: [D3], colour: [B4]{chord_level} }}]\n\
                 hold: 100 s\ncolour_level: -6\n\
                 voice: {{ voices: 1, drift: 0, wander: 0, brightness: 0 }}"
            ),
            1,
        );
        let samples = render(7.0, RATE, |rate| pad.next_sample(rate));
        amplitude_at(stretch(&samples, 5.0, 7.0), RATE, 493.883)
    };
    let difference = 20.0 * (level(", colour_level: -20") / level("")).log10();
    assert!((difference + 14.0).abs() < 0.3, "{difference} dB");
}

/// Pings and a swoosh added to a recipe leave its chords as they were: the
/// same seed gives the same chord samples with the events' layers muted.
#[test]
fn events_leave_the_chords_as_they_were() {
    let chords = "chords: [{ name: D, notes: [D3, A3] }]\nhold: 10 s\n";
    let mut plain = source(chords, 3);
    let mut eventful = source(
        &format!("{chords}pings: [{{}}, {{ range: [C5, G5] }}]\nswoosh: {{ every: 1 s }}\n"),
        3,
    );
    let mut mix = LayerMix::untouched();
    let muted = LayerSetting {
        mute: true,
        ..LayerSetting::UNTOUCHED
    };
    mix.set(2, muted);
    mix.set(3, muted);
    mix.settle();
    let a = render(12.0, RATE, |rate| plain.next_sample(rate));
    let b = render(12.0, RATE, |rate| eventful.next_mixed(rate, &mut mix));
    let largest = a
        .iter()
        .zip(&b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max);
    assert!(largest < 1e-6, "{largest}");
}

/// Two kinds of ping, a lower and a higher, on a chord of D3: each moves the
/// note into its own range (D4 and D5) and sounds at its own level.
#[test]
fn each_kind_of_ping_rings_in_its_range_at_its_level() {
    let mut pad = source(
        "chords: [{ name: D, notes: [D3] }]\nhold: 100 s\n\
         voice: { voices: 1, drift: 0, wander: 0, brightness: 0, cutoff: 100 Hz }\n\
         pings:\n\
         - { range: [C4, B4], every: 6 s, ring: 1 s, level: 0 }\n\
         - { range: [C5, B5], every: 6 s, ring: 1 s, level: -12 }",
        1,
    );
    let samples = render(8.0, RATE, |rate| pad.next_sample(rate));
    let struck = stretch(&samples, 6.0, 6.4);
    let low = amplitude_at(struck, RATE, 293.665);
    let high = amplitude_at(struck, RATE, 587.33);
    let difference = 20.0 * (high / low).log10();
    assert!((difference + 12.0).abs() < 0.5, "{difference} dB");
}
