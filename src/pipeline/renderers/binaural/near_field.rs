//! Which correction each ear gets for a source close to the head.
//!
//! The direct sound's HRIRs were measured at one distance. For a source
//! nearer than that, each ear's response is corrected by the sphere's
//! distance variation function (`crate::audio::near_field`), at the angle
//! between the direction to the source and that ear. This decides the two
//! corrections from where the source is and how the head is turned; it does
//! nothing to a sound, so it is tested without a device.

use std::sync::OnceLock;

use atrium_core::listener::Listener;
use atrium_core::types::Vec3;

use super::direction::to_sofa_coords;
use crate::audio::near_field::{EarCorrection, NearFieldTable, HEAD_RADIUS_METRES};

/// The distance the shipped HRIRs were measured at (m), from the head's
/// centre: the MIT KEMAR set (Gardner and Martin, 1994). Every one of the
/// 710 `SourcePosition` entries in `assets/hrtf/default.sofa` is at 1.4 m;
/// the SOFA reader does not expose them, so it is written here.
pub(super) const MEASURED_DISTANCE_METRES: f32 = 1.4;

/// From this distance on the correction is out of the signal (m). Relative
/// to 1.4 m, the sphere changes no ear by more than 0.22 dB at any frequency
/// from 20 Hz to 20 kHz and any angle here, and the ears' level difference
/// by no more than 0.32 dB: under the half decibel that is about the
/// smallest interaural level change anyone hears. At 1 m it is still 0.53
/// and 0.76 dB, so the correction stays in up to here.
pub(super) const SWITCH_OUT_DISTANCE_METRES: f32 = 1.2;

/// How far behind the line through the head's centre each ear sits, from
/// straight to the side: Duda and Martens' "set back about 10 degrees", the
/// ears at ±100° their interaural figures use, as do Spagnol et al.'s.
const EAR_SET_BACK_DEGREES: f32 = 10.0;

/// The corrections for the standard head and the shipped HRIRs, fitted the
/// first time they are asked for. They depend on neither the device nor the
/// scene, so every renderer shares them. Asked for when the HRIRs are loaded,
/// never on the audio thread.
pub(super) fn shared_table() -> &'static NearFieldTable {
    static TABLE: OnceLock<NearFieldTable> = OnceLock::new();
    TABLE.get_or_init(|| NearFieldTable::new(HEAD_RADIUS_METRES, MEASURED_DISTANCE_METRES))
}

/// The two ears' corrections.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct EarCorrections {
    pub(super) left: EarCorrection,
    pub(super) right: EarCorrection,
}

/// The corrections for a source at `source_position` heard by `listener`,
/// from `table`; `None` from [`SWITCH_OUT_DISTANCE_METRES`] on, where the
/// correction is not in the signal. A source nearer than the table's nearest
/// distance, inside the head or touching it, is taken at that distance.
pub(super) fn near_field_corrections(
    source_position: Vec3,
    listener: &Listener,
    table: &NearFieldTable,
) -> Option<EarCorrections> {
    let (forward, left, up) = to_sofa_coords(source_position, listener);
    let distance = (forward * forward + left * left + up * up).sqrt();
    if distance >= SWITCH_OUT_DISTANCE_METRES.min(MEASURED_DISTANCE_METRES) {
        return None;
    }
    // A source at the head's centre has no direction; straight ahead is as
    // good as any, and gives both ears the same.
    let (forward, left) = if distance > 1e-6 {
        (forward / distance, left / distance)
    } else {
        (1.0, 0.0)
    };
    // Each ear points 100° from straight ahead, level with the head's
    // centre: (cos 100°, ±sin 100°, 0) in (forward, left, up). The angle to
    // the source is the arc cosine of the two directions' dot product.
    let ear_azimuth = (90.0 + EAR_SET_BACK_DEGREES).to_radians();
    let incidence = |side: f32| {
        (forward * ear_azimuth.cos() + left * side * ear_azimuth.sin())
            .clamp(-1.0, 1.0)
            .acos()
    };
    Some(EarCorrections {
        left: table.correction(incidence(1.0), distance),
        right: table.correction(incidence(-1.0), distance),
    })
}

#[cfg(test)]
mod tests;
