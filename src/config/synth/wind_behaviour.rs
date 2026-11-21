//! A wind's repertoire as a preset writes it, shared by every wind.

use serde::Deserialize;

use crate::synth::wind_behaviour::{WindBehaviour, WindEvent, WindMove};

/// What the wind does now and then, as a preset writes it:
///
/// ```yaml
/// behaviour:
///   rest: [40, 120]          # steady wind between two moves (s)
///   repertoire:
///     - { move: gust,  chance: 3, strength: [0.3, 0.8], seconds: [3, 8] }
///     - { move: swell, chance: 2, strength: [0.2, 0.5], seconds: [10, 25] }
///     - { move: lull,  chance: 1, strength: [0.3, 0.6], seconds: [8, 20] }
/// ```
#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub(in crate::config) struct BehaviourParams {
    pub rest: [f32; 2],
    pub repertoire: Vec<MoveParams>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub(in crate::config) struct MoveParams {
    #[serde(rename = "move")]
    pub kind: MoveKind,
    #[serde(default = "one")]
    pub chance: f32,
    pub strength: [f32; 2],
    pub seconds: [f32; 2],
}

#[derive(Deserialize, Clone, Copy, Debug)]
#[serde(rename_all = "lowercase")]
pub(in crate::config) enum MoveKind {
    Gust,
    Swell,
    Lull,
}

fn one() -> f32 {
    1.0
}

impl BehaviourParams {
    pub(super) fn to_behaviour(&self) -> WindBehaviour {
        WindBehaviour {
            rest: (self.rest[0], self.rest[1]),
            events: self
                .repertoire
                .iter()
                .map(|entry| WindEvent {
                    kind: match entry.kind {
                        MoveKind::Gust => WindMove::Gust,
                        MoveKind::Swell => WindMove::Swell,
                        MoveKind::Lull => WindMove::Lull,
                    },
                    chance: entry.chance,
                    strength: (entry.strength[0], entry.strength[1]),
                    seconds: (entry.seconds[0], entry.seconds[1]),
                })
                .collect(),
        }
    }
}
