//! When a sound plays: its rest between takes, how often it repeats one, and
//! each take's chance.

use super::{lock, rounded, sound_in, Engine};
use crate::api::{Bout, EngineError, Gap};

#[uniffi::export]
impl Engine {
    /// Let the sound in `slot` rest between `shortest` and `longest` seconds
    /// between two takes. With both 0 it plays all the time. Anything with
    /// takes can rest, including a recording that loops: it then comes and
    /// goes. `scene()` reports the new gap from then on.
    ///
    /// Within the same way of playing this is immediate. Changing between
    /// looping and resting builds the sound again in its voice, which takes a
    /// moment, like `add_sound`. A generator has no takes and is left alone.
    pub fn set_source_gap(
        &self,
        slot: u16,
        shortest: f32,
        longest: f32,
    ) -> Result<(), EngineError> {
        if !(shortest.is_finite() && longest.is_finite() && shortest >= 0.0 && longest >= shortest)
        {
            return Ok(());
        }
        let (shortest, longest) = (rounded(shortest, 10.0), rounded(longest, 10.0));
        let mut edits = lock(&self.edits);
        let mut scene = lock(&self.scene);
        let Some(source) = sound_in(&mut scene.description, slot) else {
            return Ok(());
        };
        if source.takes.is_empty() {
            return Ok(());
        }
        let loops_now = source.gap.is_none();
        let loops_after = longest == 0.0 && source.takes.len() == 1;
        if let Some(entity) = scene.entity_in(slot) {
            entity.when.gap = Some([shortest, longest]);
        }
        if loops_now != loops_after {
            return self.rebuild(&mut edits, &mut scene, slot);
        }
        if loops_after {
            return Ok(());
        }
        self.planner.set_gap(slot as usize, shortest, longest);
        if let Some(source) = sound_in(&mut scene.description, slot) {
            source.gap = Some(Gap { shortest, longest });
        }
        Ok(())
    }

    /// Let the sound in `slot` utter one take `fewest` to `most` times
    /// running before it chooses another: 1 and 1 changes every time.
    /// Immediate; a sound that loops or has no takes is left alone.
    pub fn set_source_bout(&self, slot: u16, fewest: u32, most: u32) -> Result<(), EngineError> {
        if fewest < 1 || most < fewest {
            return Ok(());
        }
        let mut scene = lock(&self.scene);
        let Some(source) = sound_in(&mut scene.description, slot) else {
            return Ok(());
        };
        if source.gap.is_none() {
            return Ok(());
        }
        self.planner.set_bout(slot as usize, fewest, most);
        source.bout = Bout { fewest, most };
        if let Some(entity) = scene.entity_in(slot) {
            entity.when.bout = Some([fewest, most]);
        }
        Ok(())
    }

    /// Choose take `take` of the sound in `slot` with `chance` next to its
    /// others: 3 three times as often as 1, and 0 never. The last take that
    /// can still be chosen cannot be set to 0.
    pub fn set_take_chance(&self, slot: u16, take: u16, chance: f32) -> Result<(), EngineError> {
        if !(chance.is_finite() && chance >= 0.0) {
            return Ok(());
        }
        let chance = rounded(chance, 10.0);
        let mut scene = lock(&self.scene);
        let Some(source) = sound_in(&mut scene.description, slot) else {
            return Ok(());
        };
        let Some(name) = source
            .takes
            .get(take as usize)
            .map(|take| take.name.clone())
        else {
            return Ok(());
        };
        let others_can_be_chosen = source
            .takes
            .iter()
            .enumerate()
            .any(|(index, other)| index != take as usize && other.chance > 0.0);
        if chance == 0.0 && !others_can_be_chosen {
            return Ok(());
        }
        self.planner
            .set_chance(slot as usize, take as usize, chance);
        source.takes[take as usize].chance = chance;
        if let Some(entity) = scene.entity_in(slot) {
            entity.when.chance.insert(name, chance);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::telemetry_when;
    use super::super::AUDITION_SLOT;
    use super::*;
    use atrium::config::SceneConfig;

    /// When a sound plays: its rest and its takes' chances change while it
    /// plays, a loop can be made to come and go and back, and a muted sound
    /// stays muted through that. At zero volume:
    /// `cargo test -p atrium-ffi -- --ignored --test-threads=1`.
    #[test]
    #[ignore = "opens the default output device"]
    fn rests_and_chances_change_while_playing() {
        std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .expect("repository root");
        let engine = Engine::new("scenes/frog.yaml".into()).expect("scene should play");
        engine.set_volume(0.0).expect("volume");
        let gap_of = |slot: u16| {
            engine
                .scene()
                .sources
                .iter()
                .find(|source| source.slot == slot)
                .expect("the sound")
                .gap
        };
        assert!(gap_of(0).is_some(), "a frog croaks now and then");

        engine.set_source_muted(0, true).expect("mute");
        engine.set_source_gap(0, 0.0, 0.0).expect("no rest");
        assert_eq!(gap_of(0), None, "one take with no rest loops");
        telemetry_when(&engine, |telemetry| {
            telemetry
                .sources
                .iter()
                .any(|source| source.slot == 0 && source.muted)
        });
        engine.set_source_gap(0, 1.0, 2.04).expect("rests again");
        assert_eq!(
            gap_of(0),
            Some(Gap {
                shortest: 1.0,
                longest: 2.0
            }),
            "kept to a tenth of a second"
        );
        engine
            .set_source_gap(0, 3.0, 1.0)
            .expect("a backwards gap is ignored");
        assert_eq!(gap_of(0).map(|gap| gap.longest), Some(2.0));

        let cricket = engine
            .add_sound("sources/cricket.yaml".into(), None)
            .expect("a cricket builds");
        assert!(cricket.takes.len() > 1);
        for take in 0..cricket.takes.len() as u16 {
            engine
                .set_take_chance(cricket.slot, take, 0.0)
                .expect("chance");
        }
        let chances: Vec<f32> = engine
            .scene()
            .sources
            .iter()
            .find(|source| source.slot == cricket.slot)
            .expect("cricket")
            .takes
            .iter()
            .map(|take| take.chance)
            .collect();
        assert_eq!(
            chances.iter().filter(|chance| **chance > 0.0).count(),
            1,
            "the last take that can be chosen keeps its chance: {chances:?}"
        );

        // Bouts, and one take heard on its own from the audition voice.
        engine.set_source_bout(cricket.slot, 2, 3).expect("bout");
        engine
            .set_source_bout(cricket.slot, 3, 1)
            .expect("a backwards bout is ignored");
        let bout = engine
            .scene()
            .sources
            .iter()
            .find(|source| source.slot == cricket.slot)
            .expect("cricket")
            .bout;
        assert_eq!(bout, Bout { fewest: 2, most: 3 });
        engine
            .audition_take(cricket.slot, Some(0))
            .expect("one take on its own");
        telemetry_when(&engine, |telemetry| {
            telemetry
                .sources
                .iter()
                .any(|source| source.slot as usize == AUDITION_SLOT && source.take == Some(0))
        });
        assert_eq!(
            engine.scene().sources.len(),
            2,
            "an audition is not a layer"
        );
        engine.audition_take(cricket.slot, None).expect("stops");

        let saved = std::env::temp_dir().join(format!("atrium-rests-{}.yaml", std::process::id()));
        let saved = saved.to_str().expect("utf-8").to_owned();
        let _ = std::fs::remove_file(&saved);
        engine
            .save_scene(saved.clone(), String::new())
            .expect("saves");
        let written = SceneConfig::load(&saved).expect("reads");
        let (_, frogs) = written.sounds.iter().next().expect("frogs");
        assert_eq!(frogs.when.gap, Some([1.0, 2.0]));
        let (_, cricket) = written.sounds.iter().nth(1).expect("cricket");
        assert_eq!(
            cricket
                .when
                .chance
                .values()
                .filter(|chance| **chance == 0.0)
                .count(),
            chances.len() - 1
        );
        assert_eq!(cricket.when.bout, Some([2, 3]));
        written.build().expect("the saved scene builds");
        let _ = std::fs::remove_file(&saved);
    }
}
