//! What a face of the environment is made of: how much sound it absorbs in
//! each octave band, and so how much it sends back.

/// Surface absorption coefficients at 6 octave bands (125, 250, 500, 1k, 2k, 4k Hz).
///
/// Values from Yeoward 2021 / ISO 11654. α ranges from 0.0 (fully reflective)
/// to 1.0 (fully absorptive).
#[derive(Clone, Debug)]
pub struct WallMaterial {
    pub name: &'static str,
    /// Absorption coefficients at [125, 250, 500, 1000, 2000, 4000] Hz.
    pub alpha: [f32; 6],
}

impl WallMaterial {
    /// Hard wall as a const for use in static arrays and test fixtures.
    pub const HARD_WALL: WallMaterial = WallMaterial {
        name: "hard_wall",
        alpha: [0.02, 0.02, 0.03, 0.04, 0.05, 0.05],
    };

    /// Hard wall (concrete/plaster). Yeoward 2021 Table 1.
    pub fn hard_wall() -> Self {
        Self {
            name: "hard_wall",
            alpha: [0.02, 0.02, 0.03, 0.04, 0.05, 0.05],
        }
    }

    /// Carpet on concrete. Yeoward 2021 Table 1.
    pub fn carpet() -> Self {
        Self {
            name: "carpet",
            alpha: [0.02, 0.04, 0.08, 0.20, 0.35, 0.40],
        }
    }

    /// Acoustic ceiling tile. Yeoward 2021 Table 1.
    pub fn ceiling_tile() -> Self {
        Self {
            name: "ceiling_tile",
            alpha: [0.20, 0.40, 0.70, 0.80, 0.60, 0.40],
        }
    }

    /// Stone surface (cathedral, castle). Very low absorption, high reflection.
    pub fn stone() -> Self {
        Self {
            name: "stone",
            alpha: [0.02, 0.02, 0.03, 0.03, 0.04, 0.05],
        }
    }

    /// Wood paneling. Moderate absorption.
    pub fn wood() -> Self {
        Self {
            name: "wood",
            alpha: [0.15, 0.11, 0.10, 0.07, 0.06, 0.07],
        }
    }

    /// Glass surface. Low mid-frequency absorption, moderate at high frequencies.
    pub fn glass() -> Self {
        Self {
            name: "glass",
            alpha: [0.35, 0.25, 0.18, 0.12, 0.07, 0.04],
        }
    }

    /// Grass / soft outdoor ground. High absorption.
    pub fn grass() -> Self {
        Self {
            name: "grass",
            alpha: [0.10, 0.20, 0.40, 0.55, 0.60, 0.60],
        }
    }

    /// Fully absorptive surface (outdoor / open air). No reflections.
    pub fn open() -> Self {
        Self {
            name: "open",
            alpha: [1.0, 1.0, 1.0, 1.0, 1.0, 1.0],
        }
    }

    /// A surface that reflects the same fraction of energy in every band, so
    /// its broadband reflectivity is exactly `reflectivity`.
    #[cfg(test)]
    pub(crate) fn with_reflectivity(reflectivity: f32) -> Self {
        Self {
            name: "test",
            alpha: [1.0 - reflectivity; 6],
        }
    }

    /// Broadband reflectivity (energy domain, 0.0–1.0).
    ///
    /// Computed as `1.0 - α_broadband`, where α_broadband is the energy-weighted
    /// average of the mid bands (250 Hz–4 kHz, indices 1–5). The 125 Hz band is
    /// excluded because it contributes little to perceived broadband reflection level.
    pub fn broadband_reflectivity(&self) -> f32 {
        // Energy weights for bands 250, 500, 1k, 2k, 4k Hz.
        // Flat weighting across these perceptually dominant bands.
        let alpha_broadband =
            (self.alpha[1] + self.alpha[2] + self.alpha[3] + self.alpha[4] + self.alpha[5]) / 5.0;
        (1.0 - alpha_broadband).clamp(0.0, 1.0)
    }

    /// Broadband amplitude reflection gain: √(broadband_reflectivity).
    ///
    /// Used by `ImageSourceResolver` for per-wall reflection energy.
    pub fn broadband_reflection_gain(&self) -> f32 {
        self.broadband_reflectivity().sqrt()
    }
}

impl Default for WallMaterial {
    fn default() -> Self {
        Self::hard_wall()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Hand-derived pins (audit 2026-11) ───────────────────────────────

    /// Broadband reflectivity = 1 − mean(α at 250, 500, 1k, 2k, 4k Hz); the
    /// 125 Hz band is excluded. Amplitude gain = √reflectivity.
    /// Hard wall: 1 − 0.19/5 = 0.962 → 0.980816. Glass: 1 − 0.66/5 = 0.868 →
    /// 0.931665. Open: 0 → no reflection at all. Fully absorptive only at 125 Hz:
    /// reflectivity stays 1.
    #[test]
    fn broadband_reflectivity_matches_hand_values() {
        assert!((WallMaterial::hard_wall().broadband_reflectivity() - 0.962).abs() < 1e-6);
        assert!((WallMaterial::hard_wall().broadband_reflection_gain() - 0.980_816).abs() < 1e-5);
        assert!((WallMaterial::glass().broadband_reflectivity() - 0.868).abs() < 1e-6);
        assert!((WallMaterial::glass().broadband_reflection_gain() - 0.931_665).abs() < 1e-5);
        assert_eq!(WallMaterial::open().broadband_reflection_gain(), 0.0);

        let bass_trap_only = WallMaterial {
            name: "bass_trap_only",
            alpha: [1.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        };
        assert_eq!(bass_trap_only.broadband_reflectivity(), 1.0);
    }

    /// The const and the constructor carry the same table (duplicated data).
    #[test]
    fn hard_wall_const_matches_constructor() {
        assert_eq!(
            WallMaterial::HARD_WALL.alpha,
            WallMaterial::hard_wall().alpha
        );
        assert_eq!(WallMaterial::HARD_WALL.name, WallMaterial::hard_wall().name);
    }

    /// Every material reflects between 0 and 1 of the incident energy.
    #[test]
    fn all_materials_are_passive() {
        for material in [
            WallMaterial::hard_wall(),
            WallMaterial::carpet(),
            WallMaterial::ceiling_tile(),
            WallMaterial::stone(),
            WallMaterial::wood(),
            WallMaterial::glass(),
            WallMaterial::grass(),
            WallMaterial::open(),
        ] {
            let gain = material.broadband_reflection_gain();
            assert!((0.0..=1.0).contains(&gain), "{}: {gain}", material.name);
            assert!(material
                .alpha
                .iter()
                .all(|alpha| (0.0..=1.0).contains(alpha)));
        }
    }
}
