//! Where an ambient sound arrives from: all round, or an arc of directions
//! along the horizon on the side its place lies on.

use atrium_core::source::{ALL_ROUND_DEGREES, NARROWEST_ARC_DEGREES};
use atrium_core::types::Vec3;

/// An arc of directions along the horizon, fixed in the world: many
/// unrelated contributions, each as strong as the next, from every
/// direction within it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AmbientArc {
    /// How wide it is, radians: less than all round.
    pub width: f32,
    /// The heading of its middle from the receiver, radians, as the
    /// listener's heading is given: 0 along +x, π/2 along +y.
    pub centre: f32,
}

impl AmbientArc {
    /// The arc `degrees` wide of a sound placed at `place`, heard at
    /// `receiver`: centred on the bearing from one to the other over the
    /// ground, whatever their heights. `None` when it is all round (360° or
    /// more). Narrower than the narrowest arc it is that arc. A place right
    /// at the receiver has no side; its arc then lies along +x.
    pub fn toward(degrees: f32, receiver: Vec3, place: Vec3) -> Option<Self> {
        if degrees.is_nan() || degrees >= ALL_ROUND_DEGREES {
            return None;
        }
        let across = place - receiver;
        Some(Self {
            width: degrees.max(NARROWEST_ARC_DEGREES).to_radians(),
            centre: across.y.atan2(across.x),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_arc_faces_its_place_over_the_ground() {
        let receiver = Vec3::new(1.0, 1.0, 1.7);
        let arc = AmbientArc::toward(90.0, receiver, Vec3::new(1.0, 11.0, -3.0)).expect("an arc");
        assert!((arc.centre - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!((arc.width - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        let west = AmbientArc::toward(30.0, receiver, Vec3::new(-9.0, 1.0, 20.0)).expect("an arc");
        assert!((west.centre.abs() - std::f32::consts::PI).abs() < 1e-6);
    }

    #[test]
    fn all_round_is_no_arc_and_too_narrow_is_the_narrowest() {
        let place = Vec3::new(5.0, 0.0, 0.0);
        assert_eq!(AmbientArc::toward(360.0, Vec3::ZERO, place), None);
        assert_eq!(AmbientArc::toward(f32::NAN, Vec3::ZERO, place), None);
        let narrow = AmbientArc::toward(2.0, Vec3::ZERO, place).expect("an arc");
        assert!((narrow.width - 10f32.to_radians()).abs() < 1e-6);
    }
}
