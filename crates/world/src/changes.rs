//! What changed between two versions of a scene's sounds.
//!
//! Undo restores an earlier version of a scene, and the engine has to turn
//! the version playing into that one without stopping. This says, sound by
//! sound, the smallest thing that does it: a sound to add or take out, a
//! move, a new level, beam or size, a new name, or, when nothing smaller fits,
//! building the sound again. Working it out needs no engine, so it is tested
//! on its own.

use crate::{Entities, Entity, EntityId, Place};

/// One thing to do to the playing scene.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// A sound to take out.
    Removed { id: EntityId },
    /// A sound to add.
    Added { id: EntityId, entity: Entity },
    /// Only its place differs.
    Moved { id: EntityId, place: Place },
    /// Its level differs, and the scene sets one.
    Level { id: EntityId, level: f32 },
    /// Its beam differs, and the scene sets both its width and what is heard
    /// behind it.
    Beam {
        id: EntityId,
        width: f32,
        behind: f32,
    },
    /// Which way it faces differs, and the scene sets it (compass degrees).
    Facing { id: EntityId, facing: f32 },
    /// How wide it sounds differs, and the scene sets it (0 a point, 1 all
    /// around).
    Size { id: EntityId, size: f32 },
    /// What a list shows for it: its name, colour or group. Nothing heard.
    Identity {
        id: EntityId,
        name: Option<String>,
        color: Option<String>,
        group: Option<String>,
    },
    /// Anything else, or a part going back to its preset's own value: the
    /// sound is built again as `entity`.
    Rebuilt { id: EntityId, entity: Entity },
}

/// The changes that turn the sounds of `from` into those of `to`: first the
/// sounds taken out, so their voices are free, then the ones added, in `to`'s
/// order, then the ones that differ.
pub fn changes(from: &Entities, to: &Entities) -> Vec<Change> {
    let mut found = Vec::new();
    for (id, _) in from.iter() {
        if to.get(id).is_none() {
            found.push(Change::Removed { id: id.clone() });
        }
    }
    for (id, entity) in to.iter() {
        if from.get(id).is_none() {
            found.push(Change::Added {
                id: id.clone(),
                entity: entity.clone(),
            });
        }
    }
    for (id, wanted) in to.iter() {
        if let Some(now) = from.get(id) {
            found.extend(changes_of_one(id, now, wanted));
        }
    }
    found
}

/// The smallest changes from `now` to `wanted` for one sound.
fn changes_of_one(id: &EntityId, now: &Entity, wanted: &Entity) -> Vec<Change> {
    if now == wanted {
        return Vec::new();
    }
    let rebuilt = || {
        vec![Change::Rebuilt {
            id: id.clone(),
            entity: wanted.clone(),
        }]
    };
    // Parts only a new build can change.
    let same_sound =
        now.sound.preset == wanted.sound.preset && now.sound.audio == wanted.sound.audio;
    let same_build = same_sound
        && now.shape.ambient == wanted.shape.ambient
        && now.when == wanted.when
        && now.follows == wanted.follows
        && now.moves == wanted.moves;
    if !same_build {
        return rebuilt();
    }
    let mut found = Vec::new();
    if now.sound.level != wanted.sound.level {
        let Some(level) = wanted.sound.level else {
            return rebuilt();
        };
        found.push(Change::Level {
            id: id.clone(),
            level,
        });
    }
    if (now.shape.width, now.shape.behind) != (wanted.shape.width, wanted.shape.behind) {
        let (Some(width), Some(behind)) = (wanted.shape.width, wanted.shape.behind) else {
            return rebuilt();
        };
        found.push(Change::Beam {
            id: id.clone(),
            width,
            behind,
        });
    }
    if now.shape.facing != wanted.shape.facing {
        let Some(facing) = wanted.shape.facing else {
            return rebuilt();
        };
        found.push(Change::Facing {
            id: id.clone(),
            facing,
        });
    }
    if now.shape.size != wanted.shape.size {
        let Some(size) = wanted.shape.size else {
            return rebuilt();
        };
        found.push(Change::Size {
            id: id.clone(),
            size,
        });
    }
    if now.place != wanted.place {
        found.push(Change::Moved {
            id: id.clone(),
            place: wanted.place,
        });
    }
    if (&now.name, &now.color, &now.group) != (&wanted.name, &wanted.color, &wanted.group) {
        found.push(Change::Identity {
            id: id.clone(),
            name: wanted.name.clone(),
            color: wanted.color.clone(),
            group: wanted.group.clone(),
        });
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Moves, Shape, Sound, When};

    fn sound(preset: &str, x: f32) -> Entity {
        Entity {
            name: None,
            color: None,
            group: None,
            sound: Sound {
                preset: Some(preset.into()),
                audio: None,
                level: None,
            },
            shape: Shape::default(),
            place: Place { x, y: 0.0, z: 0.0 },
            when: When::default(),
            follows: None,
            moves: Moves::Still,
        }
    }

    fn id(name: &str) -> EntityId {
        EntityId::new(name).expect("usable")
    }

    fn scene(sounds: Vec<(&str, Entity)>) -> Entities {
        Entities::new(
            sounds
                .into_iter()
                .map(|(name, entity)| (id(name), entity))
                .collect(),
        )
        .expect("a scene")
    }

    #[test]
    fn the_same_scene_needs_nothing() {
        let meadow = scene(vec![("robin", sound("sources/robin.yaml", 1.0))]);
        assert!(changes(&meadow, &meadow).is_empty());
    }

    /// Taken out first, so a voice is free; then added in the wanted order.
    #[test]
    fn sounds_come_and_go_with_removals_first() {
        let before = scene(vec![
            ("robin", sound("sources/robin.yaml", 1.0)),
            ("frog", sound("sources/frog.yaml", 2.0)),
        ]);
        let after = scene(vec![
            ("robin", sound("sources/robin.yaml", 1.0)),
            ("bell", sound("sources/bell.yaml", 3.0)),
        ]);
        assert_eq!(
            changes(&before, &after),
            vec![
                Change::Removed { id: id("frog") },
                Change::Added {
                    id: id("bell"),
                    entity: sound("sources/bell.yaml", 3.0)
                },
            ]
        );
    }

    /// A drag, a level, a beam, a turn, a size and a new name are each done
    /// as they are, without building the sound again.
    #[test]
    fn small_changes_stay_small() {
        let before = scene(vec![("robin", sound("sources/robin.yaml", 1.0))]);
        let mut changed = sound("sources/robin.yaml", 4.0);
        changed.sound.level = Some(60.0);
        changed.shape.width = Some(120.0);
        changed.shape.behind = Some(-12.0);
        changed.shape.facing = Some(90.0);
        changed.shape.size = Some(0.4);
        changed.name = Some("Robin in the hedge".into());
        let after = scene(vec![("robin", changed)]);
        assert_eq!(
            changes(&before, &after),
            vec![
                Change::Level {
                    id: id("robin"),
                    level: 60.0
                },
                Change::Beam {
                    id: id("robin"),
                    width: 120.0,
                    behind: -12.0
                },
                Change::Facing {
                    id: id("robin"),
                    facing: 90.0
                },
                Change::Size {
                    id: id("robin"),
                    size: 0.4
                },
                Change::Moved {
                    id: id("robin"),
                    place: Place {
                        x: 4.0,
                        y: 0.0,
                        z: 0.0
                    }
                },
                Change::Identity {
                    id: id("robin"),
                    name: Some("Robin in the hedge".into()),
                    color: None,
                    group: None
                },
            ]
        );
    }

    /// What only a build can change, or a part going back to its preset's
    /// own value, builds the sound again as wanted.
    #[test]
    fn the_rest_builds_the_sound_again() {
        let mut leveled = sound("sources/robin.yaml", 1.0);
        leveled.sound.level = Some(60.0);
        let plain = sound("sources/robin.yaml", 1.0);
        let mut resting = sound("sources/robin.yaml", 1.0);
        resting.when.gap = Some([2.0, 4.0]);
        for (before, after) in [
            (leveled.clone(), plain.clone()),
            (plain.clone(), resting.clone()),
            (plain.clone(), sound("sources/frog.yaml", 1.0)),
        ] {
            let found = changes(
                &scene(vec![("robin", before)]),
                &scene(vec![("robin", after.clone())]),
            );
            assert_eq!(
                found,
                vec![Change::Rebuilt {
                    id: id("robin"),
                    entity: after
                }]
            );
        }
    }
}
