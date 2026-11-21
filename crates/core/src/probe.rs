//! Listening to one part of the render at a time.
//!
//! A rendered scene is a sum: direct sound, reflections, late reverb and
//! ambient fields, each shaped by a few effects of the air and the ground.
//! Judging any one of them by ear needs the others out of the way. A probe
//! says which parts are let through to the output and which effects are
//! applied; nothing else about the scene changes, so what is heard is exactly
//! that part of the whole.

/// What of the render reaches the output. The default lets everything
/// through, and then the render is what it always is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Probe {
    /// The sound that travels straight from a source.
    pub direct: bool,
    /// The first bounces off the environment's faces.
    pub reflections: bool,
    /// The late reverb. It is still fed while the direct sound is hidden,
    /// so it can be heard alone.
    pub reverb: bool,
    /// Ambient fields: sounds heard everywhere.
    pub ambient: bool,
    /// Treble lost to the air over distance (ISO 9613-1).
    pub air_absorption: bool,
    /// The wavering of a far sound in wind-stirred, sunlit air.
    pub moving_air: bool,
    /// The ground's effect on a path over it (ISO 9613-2).
    pub ground_effect: bool,
    /// The colour a wall gives a reflection.
    pub wall_colouring: bool,
    /// One output channel heard alone: a loudspeaker, or an ear.
    pub only_channel: Option<u8>,
}

impl Probe {
    /// Everything, as rendered.
    pub const WHOLE: Self = Self {
        direct: true,
        reflections: true,
        reverb: true,
        ambient: true,
        air_absorption: true,
        moving_air: true,
        ground_effect: true,
        wall_colouring: true,
        only_channel: None,
    };

    /// Whether this is the render as it always is.
    pub fn is_whole(&self) -> bool {
        *self == Self::WHOLE
    }
}

impl Default for Probe {
    fn default() -> Self {
        Self::WHOLE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_lets_everything_through() {
        assert!(Probe::default().is_whole());
        let reverb_alone = Probe {
            direct: false,
            reflections: false,
            ambient: false,
            ..Probe::WHOLE
        };
        assert!(!reverb_alone.is_whole());
        assert!(!Probe {
            only_channel: Some(2),
            ..Probe::WHOLE
        }
        .is_whole());
    }
}
