//! The same arguments give the same file, bit for bit, on every run.

use super::{identical, on_headphones, peak, render, storm_on_speakers, MEADOW, STORM};
use crate::offline::RenderSettings;

/// Thirty seconds of the meadow on headphones, and of the storm on 5.1
/// speakers in VBAP, rendered twice: the same file both times. Another seed
/// is another session, and another file.
#[test]
fn the_same_arguments_give_the_same_file() {
    for (scene, settings) in [
        (MEADOW, on_headphones(30.0)),
        (STORM, storm_on_speakers(30.0)),
    ] {
        let first = render(scene, &settings);
        let second = render(scene, &settings);
        assert_eq!(first.frames().len(), 30 * 48_000 * first.channels());
        assert!(peak(first.frames()) > 1e-4, "{scene}: silent");
        assert!(
            identical(first.frames(), second.frames()),
            "{scene}: two renders differ"
        );
        assert_eq!(first.statistics(), second.statistics());

        let other_seed = render(
            scene,
            &RenderSettings {
                seed: Some(first.seed() + 1),
                ..settings
            },
        );
        assert!(
            !identical(first.frames(), other_seed.frames()),
            "{scene}: another seed gave the same file"
        );
    }
}
