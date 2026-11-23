//! The one exhaustive `match` over the colours: each arm hands the work to
//! that colour's own file.

use super::blue::Blue;
use super::brown::Brown;
use super::green::Green;
use super::grey::Grey;
use super::orange::Orange;
use super::pink::Pink;
use super::red::Red;
use super::violet::Violet;
use super::white::White;
use super::NoiseColour;

/// Most layers a colour hands over: orange's two.
pub(super) const MOST_LAYERS: usize = 2;

/// The generator of one colour.
pub(super) enum Generator {
    White(White),
    Pink(Pink),
    Red(Red),
    Brown(Brown),
    Blue(Blue),
    Violet(Violet),
    Grey(Grey),
    Green(Green),
    /// Two block shapers: boxed, so the other colours are not as large.
    Orange(Box<Orange>),
}

impl Generator {
    pub(super) fn new(colour: NoiseColour, seed: u64) -> Self {
        match colour {
            NoiseColour::White => Self::White(White::new(seed)),
            NoiseColour::Pink => Self::Pink(Pink::new(seed)),
            NoiseColour::Red => Self::Red(Red::new(seed)),
            NoiseColour::Brown => Self::Brown(Brown::new(seed)),
            NoiseColour::Blue => Self::Blue(Blue::new(seed)),
            NoiseColour::Violet => Self::Violet(Violet::new(seed)),
            NoiseColour::Grey => Self::Grey(Grey::new(seed)),
            NoiseColour::Green => Self::Green(Green::new(seed)),
            NoiseColour::Orange => Self::Orange(Box::new(Orange::new(seed))),
        }
    }

    /// How many of [`Self::next_layers`]' values are layers.
    pub(super) fn layer_count(&self) -> usize {
        match self {
            Self::Orange(_) => MOST_LAYERS,
            _ => 1,
        }
    }

    /// The next sample of each layer at `sample_rate`; only the first
    /// [`Self::layer_count`] are used.
    #[inline]
    pub(super) fn next_layers(&mut self, sample_rate: f32) -> [f32; MOST_LAYERS] {
        let only = |sample| [sample, 0.0];
        match self {
            Self::White(white) => only(white.next_sample(sample_rate)),
            Self::Pink(pink) => only(pink.next_sample()),
            Self::Red(red) => only(red.next_sample(sample_rate)),
            Self::Brown(brown) => only(brown.next_sample(sample_rate)),
            Self::Blue(blue) => only(blue.next_sample(sample_rate)),
            Self::Violet(violet) => only(violet.next_sample(sample_rate)),
            Self::Grey(grey) => only(grey.next_sample(sample_rate)),
            Self::Green(green) => only(green.next_sample(sample_rate)),
            Self::Orange(orange) => orange.next_layers(sample_rate),
        }
    }
}
