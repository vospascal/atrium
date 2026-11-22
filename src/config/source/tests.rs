//! Tests of resolving a sound against its preset.

use super::*;

fn kind(yaml: &str) -> Result<ResolvedSound, String> {
    match serde_yaml::from_str::<SourceDef>(yaml).map_err(|error| error.to_string())? {
        SourceDef::Sample(sample) => sample.sound().map_err(|error| error.to_string()),
        SourceDef::Synth(_) => Err("parsed as a generator".into()),
    }
}

/// Every recording kind is written the same way: its takes, and a rule.
/// A looping kind has one take and does not rest.
#[test]
fn a_looping_kind_has_one_take_and_the_loops_rule() {
    let yaml =
        "reference_spl: 68.0\ntakes:\n  - { file: assets/campfire.mp3, name: Fire }\nrule: loops\n";
    match kind(yaml).expect("a looping kind reads") {
        ResolvedSound::Sample { takes, rests } => {
            assert_eq!(takes.len(), 1);
            assert_eq!(
                (takes[0].file.as_str(), takes[0].name.as_str()),
                ("assets/campfire.mp3", "Fire")
            );
            assert_eq!(takes[0].chance, 1.0);
            assert!(rests.is_none());
        }
        ResolvedSound::Synth { .. } => panic!("a recording, not a generator"),
    }
}

fn two_takes() -> ResolvedSound {
    kind("reference_spl: 60\ntakes:\n  - { file: a.mp3, name: Low }\n  - { file: b.mp3, name: High }\nrule: { rests: { gap: [3, 8], vary: { level: 2 } } }\n")
            .expect("reads")
}

/// A sound's own `when:` changes its kind's rests and chances, by take
/// name, and keeps the kind's variation.
#[test]
fn when_changes_rests_and_chances_of_one_sound() {
    let when = When {
        gap: Some([20.0, 40.0]),
        chance: [("High".to_string(), 0.0)].into(),
        bout: Some([2, 3]),
    };
    let ResolvedSound::Sample { takes, rests } = with_when(two_takes(), &when).expect("applies")
    else {
        panic!("a recording");
    };
    assert_eq!((takes[0].chance, takes[1].chance), (1.0, 0.0));
    let rests = rests.expect("still rests");
    assert_eq!(rests.gap_seconds, (20.0, 40.0));
    assert_eq!(rests.level_variation_db, 2.0, "the kind's variation stays");
    assert_eq!(rests.bout, (2, 3));
    let backwards = When {
        bout: Some([3, 1]),
        ..When::default()
    };
    assert!(
        with_when(two_takes(), &backwards).is_err(),
        "a bout runs from few to many"
    );
}

/// Anything that loops can be made to come and go, and one take with no
/// rest at all loops again.
#[test]
fn a_gap_makes_a_loop_come_and_go_and_none_makes_it_loop() {
    let fire =
        || kind("reference_spl: 68\ntakes:\n  - { file: fire.mp3 }\nrule: loops\n").expect("reads");
    let coming = When {
        gap: Some([5.0, 9.0]),
        ..When::default()
    };
    let ResolvedSound::Sample { rests, .. } = with_when(fire(), &coming).expect("applies") else {
        panic!("a recording");
    };
    assert_eq!(rests.expect("rests now").gap_seconds, (5.0, 9.0));
    let always = When {
        gap: Some([0.0, 0.0]),
        ..When::default()
    };
    let ResolvedSound::Sample { rests, .. } = with_when(fire(), &always).expect("applies") else {
        panic!("a recording");
    };
    assert!(rests.is_none(), "one take with no rest loops");
}

#[test]
fn when_refuses_what_it_cannot_mean() {
    let unknown = When {
        chance: [("Song".to_string(), 2.0)].into(),
        ..When::default()
    };
    assert!(
        with_when(two_takes(), &unknown).is_err(),
        "no take of that name"
    );
    let silent = When {
        chance: [("Low".to_string(), 0.0), ("High".to_string(), 0.0)].into(),
        ..When::default()
    };
    assert!(
        with_when(two_takes(), &silent).is_err(),
        "nothing left to choose"
    );
    let backwards = When {
        gap: Some([9.0, 2.0]),
        ..When::default()
    };
    assert!(with_when(two_takes(), &backwards).is_err());
}

#[test]
fn a_resting_kind_reads_its_gap_variation_and_chances() {
    let yaml = "reference_spl: 60\ntakes:\n  - { file: a.mp3, chance: 3 }\n  - { file: b-song.mp3 }\nrule:\n  rests:\n    gap: [3, 8]\n    vary: { level: 2, pitch: 15 }\n";
    match kind(yaml).expect("a resting kind reads") {
        ResolvedSound::Sample { takes, rests } => {
            assert_eq!(takes[0].chance, 3.0);
            assert_eq!(
                takes[1].name, "B Song",
                "a take without a name is named after its file"
            );
            assert_eq!(
                rests,
                Some(Rests {
                    level_variation_db: 2.0,
                    pitch_variation_cents: 15.0,
                    ..Rests::plain(3.0, 8.0)
                })
            );
        }
        ResolvedSound::Synth { .. } => panic!("a recording, not a generator"),
    }
}

/// What a kind file can get wrong, each refused with a reason that says
/// what to write instead. The one-file `path:` of before is one of them.
#[test]
fn a_kind_that_cannot_work_is_refused() {
    let two = "takes:\n  - { file: a.mp3 }\n  - { file: b.mp3 }\n";
    for (yaml, reason) in [
        (
            format!("reference_spl: 60\n{two}rule: loops\n"),
            "one take round and round",
        ),
        (
            "reference_spl: 60\ntakes: []\nrule: loops\n".to_string(),
            "at least one take",
        ),
        (
            format!("reference_spl: 60\n{two}rule: shuffles\n"),
            "is not a rule",
        ),
        (
            format!("reference_spl: 60\n{two}rule: {{ rests: {{ gap: [8, 3] }} }}\n"),
            "short to long",
        ),
        (
            "reference_spl: 60\ntakes:\n  - { file: a.mp3, chance: 0 }\nrule: loops\n".to_string(),
            "more than 0",
        ),
    ] {
        let refusal = kind(&yaml)
            .err()
            .unwrap_or_else(|| panic!("accepted:\n{yaml}"));
        assert!(refusal.contains(reason), "{refusal}");
    }
    assert!(kind("path: assets/campfire.mp3\nreference_spl: 68.0\n").is_err());
}

/// A measured level spread is read with the rests, and one that cannot be
/// right is refused.
#[test]
fn a_resting_kind_reads_its_level_spread() {
    let yaml = |spread: &str| {
        format!("reference_spl: 78\ntakes:\n  - {{ file: a.mp3 }}\n  - {{ file: b.mp3 }}\nrule:\n  rests:\n    gap: [3, 8]\n    level_spread: {spread}\n")
    };
    match kind(&yaml(
        "{ individual: 3.8, utterance: 2.5, range: [-15.4, 12.3] }",
    ))
    .expect("a spread reads")
    {
        ResolvedSound::Sample { rests, .. } => assert_eq!(
            rests.expect("rests").level_spread,
            LevelSpread {
                individual_db: 3.8,
                utterance_db: 2.5,
                range_db: (-15.4, 12.3),
            }
        ),
        ResolvedSound::Synth { .. } => panic!("a recording, not a generator"),
    }
    for wrong in [
        "{ individual: -1, utterance: 2, range: [-5, 5] }",
        "{ individual: 1, utterance: 2, range: [1, 5] }",
        "{ individual: 1, utterance: 2, range: [-5, -1] }",
        "{ individual: 1, utterance: 2 }",
        "{ individual: 1, utterance: 2, range: [-5, 5], loud: 3 }",
    ] {
        assert!(kind(&yaml(wrong)).is_err(), "{wrong} should be refused");
    }
}
