//! A sound's layers, mixed by the node: the checks of
//! `docs/sessions-and-layers-plan.md` ("How we know it is right") that need
//! no scene.

use atrium_core::layers::{LayerMix, LayerSetting};
use atrium_core::source::SoundSource;
use atrium_core::types::Vec3;

use super::SynthNode;
use crate::synth::canopy_wind::CanopyWindSource;
use crate::synth::field_wind::FieldWindSource;
use crate::synth::focus_tone::{FocusToneSettings, FocusToneSource};
use crate::synth::rain::RainSource;
use crate::synth::rain_v2::RainSourceV2;
use crate::synth::river::RiverSource;
use crate::synth::soft_wind::SoftWindSource;
use crate::synth::storm_wind::StormWindSource;
use crate::synth::test_support::amplitude_at;
use crate::synth::wave::WaveSource;
use crate::synth::wisp::WispSource;

const SAMPLE_RATE: f32 = 48_000.0;
const SECONDS: f32 = 3.0;
const SEED: u64 = 7;

/// FNV-1a over the bits of every sample.
fn fingerprint(mut next: impl FnMut() -> f32) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for _ in 0..(SECONDS * SAMPLE_RATE) as usize {
        for byte in next().to_bits().to_le_bytes() {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}

/// A focus tone at 528 Hz: four lines, 528, 264, 132 and 66 Hz.
fn focus_tone() -> FocusToneSource {
    FocusToneSource::new(
        Vec3::ZERO,
        FocusToneSettings {
            pitch_hz: 528.0,
            ..FocusToneSettings::default()
        },
    )
}

/// Each layered generator, made afresh, with its name, how many layers it
/// has, and the fingerprint of its first three seconds taken from the
/// generators before they handed their layers over (the tree of 2026-11-23,
/// commit 28aed92 with its uncommitted work).
type Made = Box<dyn Fn() -> Box<dyn SoundSource>>;
fn layered() -> Vec<(&'static str, usize, u64, Made)> {
    let place = Vec3::ZERO;
    vec![
        (
            "focus tone",
            4,
            0xe891be7ea8ff28c4,
            Box::new(|| Box::new(focus_tone())),
        ),
        (
            "soft wind",
            5,
            0xcbe22b5076edd44c,
            Box::new(move || Box::new(SoftWindSource::new(place, 1.0, 5.0, SEED))),
        ),
        (
            "field wind",
            5,
            0xd102d0f2ed45bc63,
            Box::new(move || Box::new(FieldWindSource::new(place, 2.0, 10.0, SEED))),
        ),
        (
            "canopy wind",
            5,
            0x321d699ea68fc80a,
            Box::new(move || Box::new(CanopyWindSource::new(place, 2.0, 9.0, SEED))),
        ),
        (
            "storm wind",
            6,
            0x3de7cb984a412b16,
            Box::new(move || Box::new(StormWindSource::new(place, 10.0, 22.0, SEED))),
        ),
        (
            "rain",
            3,
            0x5f50e889d7d15358,
            Box::new(move || Box::new(RainSource::new(place, 0.6, SEED))),
        ),
        (
            "rain v2",
            3,
            0xc228b4cc58d80f65,
            Box::new(move || Box::new(RainSourceV2::new(place, 0.6, SEED))),
        ),
        (
            "river",
            4,
            0xfb6bc63427c97006,
            Box::new(move || Box::new(RiverSource::new(place, 0.4, 1.2, SEED))),
        ),
        (
            "waves",
            3,
            0x18435f3038b433b8,
            Box::new(move || Box::new(WaveSource::new(place, 8.0, 0.3, SEED))),
        ),
        (
            "wisp",
            2,
            0x7a6dac8d3718e2c3,
            Box::new(move || Box::new(WispSource::new(place, SEED))),
        ),
    ]
}

/// A node playing `generator` at amplitude 1, its layers set to `settings`
/// from the first sample.
fn node(generator: Box<dyn SoundSource>, settings: &[(usize, LayerSetting)]) -> SynthNode {
    let mut node = SynthNode::new(generator, Vec3::ZERO);
    node.amplitude = 1.0;
    for &(layer, setting) in settings {
        node.set_layer(layer, setting);
    }
    node.settle_layers();
    node
}

fn render(node: &mut SynthNode, seconds: f32) -> Vec<f32> {
    (0..(seconds * SAMPLE_RATE) as usize)
        .map(|_| node.next_sample(SAMPLE_RATE))
        .collect()
}

fn at(level_db: f32) -> LayerSetting {
    LayerSetting {
        level_db,
        ..LayerSetting::UNTOUCHED
    }
}

const MUTED: LayerSetting = LayerSetting {
    level_db: 0.0,
    solo: false,
    mute: true,
};

const SOLO: LayerSetting = LayerSetting {
    level_db: 0.0,
    solo: true,
    mute: false,
};

/// Every layered generator with its layers at 0 dB makes the same bits as
/// before its layers were handed over: by itself, through an untouched mix
/// that is advanced every sample as the node does, and through the node.
#[test]
fn untouched_layers_are_the_generator_as_it_was() {
    for (name, layers, before, made) in layered() {
        let mut generator = made();
        assert_eq!(generator.layer_count(), layers, "{name}");
        let alone = fingerprint(|| generator.next_sample(SAMPLE_RATE));
        assert_eq!(alone, before, "{name}: by itself");

        let mut generator = made();
        let mut mix = LayerMix::untouched();
        let mixed = fingerprint(|| {
            mix.advance(SAMPLE_RATE);
            generator.next_mixed(SAMPLE_RATE, &mut mix)
        });
        assert_eq!(mixed, before, "{name}: through an untouched mix");

        let mut through_node = node(made(), &[]);
        let played = fingerprint(|| through_node.next_sample(SAMPLE_RATE));
        assert_eq!(played, before, "{name}: through the node");
    }
}

/// Every layer of every layered generator is mixed on its own and in a
/// straight line: what one layer adds is the untouched output less the
/// output with that layer muted, and at −6 dB the layer adds 10^(−6/20) of
/// it, whatever the generator does to the sum after (gains, filters).
#[test]
fn each_layer_is_weighed_on_its_own() {
    let weight = 10.0_f64.powf(-6.0 / 20.0);
    let mut worst = f64::NEG_INFINITY;
    for (name, layers, _, made) in layered() {
        let untouched = render(&mut node(made(), &[]), 1.0);
        for layer in 0..layers {
            let without = render(&mut node(made(), &[(layer, MUTED)]), 1.0);
            let lowered = render(&mut node(made(), &[(layer, at(-6.0))]), 1.0);
            let (mut contribution, mut error) = (0.0_f64, 0.0_f64);
            for ((&whole, &rest), &low) in untouched.iter().zip(&without).zip(&lowered) {
                let added = whole as f64 - rest as f64;
                contribution += added * added;
                let expected = rest as f64 + weight * added;
                error += (low as f64 - expected).powi(2);
            }
            if contribution == 0.0 {
                // A layer that is silent in this stretch (no debris below
                // gale force, say) has nothing to weigh.
                assert_eq!(error, 0.0, "{name} layer {layer}");
                continue;
            }
            let error_db = 10.0 * (error / contribution).log10();
            assert!(
                error_db < -60.0,
                "{name} layer {layer}: −6 dB is off by {error_db:.1} dB of the layer"
            );
            worst = worst.max(error_db);
        }
    }
    println!("every layer of every generator at −6 dB: off by at most {worst:.1} dB of the layer");
}

/// −6 dB on the 264 Hz line of a 528 Hz focus tone lowers that line by
/// 6.00 dB, read by a Hann-windowed DFT at each line over one second, and
/// leaves the 528, 132 and 66 Hz lines within 0.01 dB. Returns the changes.
fn line_changes_db() -> [f64; 4] {
    let untouched = render(&mut node(Box::new(focus_tone()), &[]), 2.0);
    let lowered = render(&mut node(Box::new(focus_tone()), &[(1, at(-6.0))]), 2.0);
    let second = SAMPLE_RATE as usize;
    std::array::from_fn(|line| {
        let frequency = 528.0 / (1u32 << line) as f64;
        let before = amplitude_at(&untouched[second..], SAMPLE_RATE, frequency);
        let after = amplitude_at(&lowered[second..], SAMPLE_RATE, frequency);
        20.0 * (after / before).log10()
    })
}

#[test]
fn a_layers_level_lowers_that_layer_alone() {
    let changes = line_changes_db();
    println!("focus tone 528 Hz, −6 dB on 264 Hz: {changes:?}");
    assert!((changes[1] + 6.0).abs() < 0.005, "{changes:?}");
    for line in [0, 2, 3] {
        assert!(changes[line].abs() < 0.01, "{changes:?}");
    }
}

/// The largest step from one sample to the next in `samples`.
fn largest_step(samples: &[f32]) -> f32 {
    samples
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0, f32::max)
}

/// Solo and mute, switched while it plays: a soloed layer is that layer
/// alone, a muted one is gone, and the switch makes no step larger than the
/// signal's own largest step over the same stretch. Returns, for the soft
/// wind's presence layer and the focus tone's 264 Hz line, the largest step
/// in the 50 ms after a mute and after a solo, against the untouched
/// signal's.
#[test]
fn solo_and_mute_are_that_layer_and_do_not_click() {
    // Soloed: the presence layer alone is the output with every other muted.
    let soloed = render(&mut node(made_soft_wind(), &[(3, SOLO)]), 1.0);
    let others_muted: Vec<(usize, LayerSetting)> =
        [0, 1, 2, 4].iter().map(|&layer| (layer, MUTED)).collect();
    let alone = render(&mut node(made_soft_wind(), &others_muted), 1.0);
    assert_eq!(soloed, alone, "a soloed layer is that layer alone");
    // Muted: the output is the untouched one less that layer.
    let all_muted: Vec<(usize, LayerSetting)> = (0..5).map(|layer| (layer, MUTED)).collect();
    assert!(render(&mut node(made_soft_wind(), &all_muted), 1.0)
        .iter()
        .all(|&sample| sample == 0.0));

    for (name, made, layer) in [
        (
            "soft wind presence",
            made_soft_wind as fn() -> Box<dyn SoundSource>,
            3,
        ),
        ("focus tone 264 Hz", || Box::new(focus_tone()), 1),
    ] {
        for (switch, setting) in [("mute", MUTED), ("solo", SOLO)] {
            let half = (0.5 * SAMPLE_RATE) as usize;
            let after = (0.05 * SAMPLE_RATE) as usize;
            let untouched = render(&mut node(made(), &[]), 1.0);
            let mut switched = node(made(), &[]);
            let mut samples = render(&mut switched, 0.5);
            switched.set_layer(layer, setting);
            samples.extend(render(&mut switched, 0.5));
            let own = largest_step(&untouched[half - after..half + after]);
            let at_switch = largest_step(&samples[half - 1..half + after]);
            println!(
                "{name} {switch}: largest step {at_switch:.6} against the signal's own {own:.6}"
            );
            assert!(at_switch <= own, "{name} {switch}: {at_switch} > {own}");
        }
    }
}

fn made_soft_wind() -> Box<dyn SoundSource> {
    Box::new(SoftWindSource::new(Vec3::ZERO, 1.0, 5.0, SEED))
}

/// A binaural beat's two layers are its two voices: muting the right ear
/// silences the right voice and leaves the left as it was.
#[test]
fn a_binaural_beats_layers_are_its_ears() {
    use crate::synth::binaural_beat::{BinauralBeatSettings, BinauralToneSource, Ear};
    let settings = BinauralBeatSettings::default();
    let mut beat = SynthNode::new(
        Box::new(BinauralToneSource::new(Ear::Left, settings)),
        Vec3::ZERO,
    );
    beat.amplitude = 1.0;
    beat.add_second_voice(Box::new(BinauralToneSource::new(Ear::Right, settings)));
    assert_eq!(beat.layer_count(), 2);
    beat.set_layer(1, MUTED);
    beat.settle_layers();
    let mut left = BinauralToneSource::new(Ear::Left, settings);
    for _ in 0..48_000 {
        assert_eq!(beat.next_sample(SAMPLE_RATE), left.next_sample(SAMPLE_RATE));
        assert_eq!(beat.second_voice().unwrap().next_sample(SAMPLE_RATE), 0.0);
    }
    // Soloing the right ear silences the left.
    beat.set_layer(1, SOLO);
    beat.settle_layers();
    assert_eq!(beat.next_sample(SAMPLE_RATE), 0.0);
}
