//! The working generator's mixer: a fader, solo and mute for each of the
//! soft wind's five layers, and the undo stack of mixer changes.
//!
//! A fader at 0 dB is the layer at the level the generator gives it, so the
//! untouched mix is the generator as it sounded before, sample for sample.

use serde::{Deserialize, Serialize};

use crate::synth::soft_wind::SoftWindSource;

/// One of the soft wind's layers, from the lowest band to the highest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Layer {
    Low,
    Body,
    Mid,
    Presence,
    Air,
}

impl Layer {
    pub const ALL: [Layer; 5] = [
        Layer::Low,
        Layer::Body,
        Layer::Mid,
        Layer::Presence,
        Layer::Air,
    ];

    /// The name a session file uses.
    pub fn name(self) -> &'static str {
        match self {
            Layer::Low => "low",
            Layer::Body => "body",
            Layer::Mid => "mid",
            Layer::Presence => "presence",
            Layer::Air => "air",
        }
    }

    /// The layer a session file names.
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|layer| layer.name() == name)
    }

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|layer| *layer == self)
            .unwrap_or_default()
    }
}

/// The gain each layer has in `generator`: what a fader at 0 dB keeps.
pub(crate) fn layer_gains(generator: &SoftWindSource) -> [f32; 5] {
    [
        generator.low_gain,
        generator.body_gain,
        generator.mid_gain,
        generator.presence_gain,
        generator.air_gain,
    ]
}

/// One layer's strip.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Fader {
    /// Relative to the layer as the generator gives it (dB).
    pub level_db: f32,
    pub solo: bool,
    pub mute: bool,
}

/// The faders of all five layers.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LayerMix {
    faders: [Fader; 5],
}

impl LayerMix {
    pub fn fader(&self, layer: Layer) -> Fader {
        self.faders[layer.index()]
    }

    /// Set `layer`'s strip. A level that is not a number leaves it as it was.
    pub fn set(&mut self, layer: Layer, fader: Fader) {
        if fader.level_db.is_finite() {
            self.faders[layer.index()] = fader;
        }
    }

    /// Each layer's gain, given the gains the generator itself has: a
    /// muted layer is silent, and while any layer is soloed only the soloed
    /// ones sound.
    pub fn gains(&self, own: [f32; 5]) -> [f32; 5] {
        let any_solo = self.faders.iter().any(|fader| fader.solo);
        let mut gains = [0.0; 5];
        for ((gain, fader), own) in gains.iter_mut().zip(&self.faders).zip(own) {
            let heard = !fader.mute && (!any_solo || fader.solo);
            *gain = if heard {
                own * 10.0_f32.powf(fader.level_db / 20.0)
            } else {
                0.0
            };
        }
        gains
    }
}

/// The session's mixer changes, newest last, for undo.
#[derive(Clone, Debug, Default)]
pub struct MixHistory {
    /// The mix before each change, and the layer whose level the change
    /// moved, if that was all it did.
    before: Vec<(LayerMix, Option<Layer>)>,
}

impl MixHistory {
    /// Record that `mix` is about to become `next`. Moving the same layer's
    /// level again straight after is one change, as a fader is dragged
    /// through many values; a solo or mute is always a change of its own.
    pub fn record(&mut self, mix: &LayerMix, next: &LayerMix) {
        if mix == next {
            return;
        }
        let moved = Layer::ALL.into_iter().find(|layer| {
            let (was, will) = (mix.fader(*layer), next.fader(*layer));
            was.level_db != will.level_db && was.solo == will.solo && was.mute == will.mute
        });
        let level_only = moved.filter(|moved| {
            Layer::ALL
                .iter()
                .all(|layer| layer == moved || mix.fader(*layer) == next.fader(*layer))
        });
        if let (Some(layer), Some((_, Some(last)))) = (level_only, self.before.last()) {
            if *last == layer {
                return;
            }
        }
        self.before.push((*mix, level_only));
    }

    /// The mix before the newest change, which is forgotten.
    pub fn undo(&mut self) -> Option<LayerMix> {
        self.before.pop().map(|(mix, _)| mix)
    }

    pub fn can_undo(&self) -> bool {
        !self.before.is_empty()
    }
}
