//! A sound's layers, and the mix that sums them.
//!
//! A generator whose sound is made of distinct parts (a focus tone's lines, a
//! wind's bands, rain's hiss and drops) hands each part over as a layer
//! instead of adding them up itself, and a [`LayerMix`] sums them: a level,
//! solo and mute per layer, the same code for every generator. At 0 dB a
//! layer is weighed by exactly 1, so the untouched mix is the generator as it
//! sounded before, to the bit. Changes glide over [`GLIDE_SECONDS`], so a
//! fader, a solo or a mute never clicks.
//!
//! Nothing here allocates: a mix is a few fixed arrays, changed and read on
//! the audio thread.

/// Most layers one sound can have: the storm wind's six, with room.
pub const MOST_LAYERS: usize = 8;
/// Softest and loudest a layer can be set, against the generator's own
/// level (dB).
pub const LAYER_LEVEL_RANGE_DB: (f32, f32) = (-60.0, 12.0);
/// Time constant a layer's weight moves to a new setting with (s).
const GLIDE_SECONDS: f32 = 0.010;
/// Time constant of the meter of each layer's power (s): long enough to
/// draw its activity calmly, short enough to show a gust.
const METER_SECONDS: f32 = 0.1;
/// Closer than this to its target, a gliding weight takes the target itself,
/// so a fader brought back to 0 dB is exactly 1 again. In `f32` a one-pole
/// glide stalls some 10⁻⁵ short of 1, where a step rounds to nothing; the
/// last 10⁻⁴ (−80 dB of the layer) is taken in one step, which is not heard.
const SNAP: f32 = 1e-4;

/// How one layer is mixed: its level against the generator's own, and
/// whether it is heard alone or not at all. Solo wins over mute: a muted
/// layer that is soloed is heard, as in the scene's own list of sounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerSetting {
    /// dB against the layer as the generator makes it; 0 leaves it as made.
    pub level_db: f32,
    pub solo: bool,
    pub mute: bool,
}

impl LayerSetting {
    /// The layer as the generator makes it.
    pub const UNTOUCHED: Self = Self {
        level_db: 0.0,
        solo: false,
        mute: false,
    };
}

/// The mix of one sound's layers: what each is set to, and the weight each
/// is summed with now, gliding to what the settings ask.
#[derive(Clone, Debug)]
pub struct LayerMix {
    settings: [LayerSetting; MOST_LAYERS],
    /// Where each weight is heading.
    targets: [f32; MOST_LAYERS],
    /// What each layer is multiplied by now.
    weights: [f32; MOST_LAYERS],
    /// Whether every weight is at its target, so nothing glides.
    settled: bool,
    sample_rate_cached: f32,
    /// One-pole step per sample of the glide and of the meter.
    glide_step: f32,
    meter_step: f32,
    /// Each weighed layer's power, smoothed over [`METER_SECONDS`]; not kept
    /// until [`Self::advance`] has been given a rate.
    powers: [f32; MOST_LAYERS],
}

impl LayerMix {
    /// Every layer as the generator makes it, weighed by exactly 1.
    pub const fn untouched() -> Self {
        Self {
            settings: [LayerSetting::UNTOUCHED; MOST_LAYERS],
            targets: [1.0; MOST_LAYERS],
            weights: [1.0; MOST_LAYERS],
            settled: true,
            sample_rate_cached: 0.0,
            glide_step: 1.0,
            meter_step: 0.0,
            powers: [0.0; MOST_LAYERS],
        }
    }

    /// Set layer `layer` to `setting` from now on; the weights glide there.
    /// A layer beyond [`MOST_LAYERS`] or a level that is not a number is
    /// passed over, and a level outside [`LAYER_LEVEL_RANGE_DB`] is clamped.
    pub fn set(&mut self, layer: usize, setting: LayerSetting) {
        if layer >= MOST_LAYERS || !setting.level_db.is_finite() {
            return;
        }
        self.settings[layer] = LayerSetting {
            level_db: setting
                .level_db
                .clamp(LAYER_LEVEL_RANGE_DB.0, LAYER_LEVEL_RANGE_DB.1),
            ..setting
        };
        let any_solo = self.settings.iter().any(|setting| setting.solo);
        for (target, setting) in self.targets.iter_mut().zip(&self.settings) {
            let heard = if any_solo {
                setting.solo
            } else {
                !setting.mute
            };
            *target = if heard {
                10.0_f32.powf(setting.level_db / 20.0)
            } else {
                0.0
            };
        }
        self.settled = false;
    }

    /// What layer `layer` is set to.
    fn setting(&self, layer: usize) -> LayerSetting {
        self.settings
            .get(layer)
            .copied()
            .unwrap_or(LayerSetting::UNTOUCHED)
    }

    /// What layer `layer` comes to once solo and mute are taken into
    /// account, as a setting of its own: its level, and muted when the mix
    /// leaves it out. A layer played as a voice of its own (the right ear of
    /// a binaural beat) is set to this.
    pub fn heard(&self, layer: usize) -> LayerSetting {
        let setting = self.setting(layer);
        LayerSetting {
            level_db: setting.level_db,
            solo: false,
            mute: self.targets.get(layer).is_none_or(|&target| target == 0.0),
        }
    }

    /// Jump every weight to its target, as if the glide had finished: for a
    /// sound built with its layers already set.
    pub fn settle(&mut self) {
        self.weights = self.targets;
        self.settled = true;
    }

    /// Move every weight one sample at `sample_rate` towards its target.
    /// Called once per sample by whoever owns the mix, before the sound's
    /// next sample is summed.
    #[inline]
    pub fn advance(&mut self, sample_rate: f32) {
        if sample_rate != self.sample_rate_cached {
            self.sample_rate_cached = sample_rate;
            let rate = sample_rate.max(1.0);
            self.glide_step = 1.0 - (-1.0 / (GLIDE_SECONDS * rate)).exp();
            self.meter_step = 1.0 - (-1.0 / (METER_SECONDS * rate)).exp();
        }
        if self.settled {
            return;
        }
        let mut settled = true;
        for (weight, &target) in self.weights.iter_mut().zip(&self.targets) {
            *weight += (target - *weight) * self.glide_step;
            if (target - *weight).abs() < SNAP {
                *weight = target;
            } else {
                settled = false;
            }
        }
        self.settled = settled;
    }

    /// The sum of `layers`, the first being layer `first_layer`, each
    /// multiplied by its weight, added in order from the first: at 0 dB the
    /// same bits as adding them up directly. A generator whose layers meet in
    /// more than one place sums each group with its own call.
    #[inline]
    pub fn sum(&mut self, first_layer: usize, layers: &[f32]) -> f32 {
        let mut sum = 0.0;
        for (offset, &sample) in layers.iter().enumerate() {
            let layer = (first_layer + offset).min(MOST_LAYERS - 1);
            let weighed = sample * self.weights[layer];
            if self.meter_step > 0.0 {
                let power = &mut self.powers[layer];
                *power += (weighed * weighed - *power) * self.meter_step;
            }
            sum += weighed;
        }
        sum
    }

    /// The weight layer `layer` is multiplied by now.
    #[cfg(test)]
    fn weight(&self, layer: usize) -> f32 {
        self.weights.get(layer).copied().unwrap_or(0.0)
    }

    /// Each weighed layer's power, smoothed, before what the generator does
    /// to the sum: how much of the sound each layer is, for a meter. Zero
    /// until [`Self::advance`] has run.
    pub fn powers(&self) -> [f32; MOST_LAYERS] {
        self.powers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Untouched, every weight is exactly 1, and a sum is the plain sum to
    /// the bit.
    #[test]
    fn an_untouched_mix_adds_the_layers_as_they_are() {
        let mut mix = LayerMix::untouched();
        let layers = [0.1_f32, -0.37, 1e-7, 0.25];
        let plain = layers[0] + layers[1] + layers[2] + layers[3];
        assert_eq!(mix.sum(0, &layers).to_bits(), plain.to_bits());
        mix.advance(48_000.0);
        assert_eq!(mix.sum(0, &layers).to_bits(), plain.to_bits());
    }

    /// −6 dB is 10^(−6/20) = 0.501187 once the glide is over; back at 0 dB
    /// it is exactly 1 again. A glide of 0.5 comes within the snap of 10⁻⁴
    /// after ln(0.5 / 10⁻⁴) = 8.5 time constants, 85 ms: 200 ms is past it.
    #[test]
    fn a_level_glides_to_its_decibels_and_back_to_one() {
        let mut mix = LayerMix::untouched();
        mix.set(
            1,
            LayerSetting {
                level_db: -6.0,
                ..LayerSetting::UNTOUCHED
            },
        );
        for _ in 0..9_600 {
            mix.advance(48_000.0);
        }
        assert!(
            (mix.weight(1) - 0.501_187).abs() < 1e-6,
            "{}",
            mix.weight(1)
        );
        assert_eq!(mix.weight(0), 1.0);
        mix.set(1, LayerSetting::UNTOUCHED);
        for _ in 0..9_600 {
            mix.advance(48_000.0);
        }
        assert_eq!(mix.weight(1), 1.0);
    }

    /// A solo silences every layer not soloed, even an unmuted one, and lets
    /// a muted soloed one sound; with no solo, mute silences its own layer.
    #[test]
    fn solo_wins_over_mute() {
        let mut mix = LayerMix::untouched();
        mix.set(
            0,
            LayerSetting {
                mute: true,
                ..LayerSetting::UNTOUCHED
            },
        );
        assert!(mix.heard(0).mute && !mix.heard(1).mute);
        mix.set(
            0,
            LayerSetting {
                solo: true,
                mute: true,
                ..LayerSetting::UNTOUCHED
            },
        );
        assert!(!mix.heard(0).mute && mix.heard(1).mute && mix.heard(2).mute);
        mix.settle();
        assert_eq!(mix.sum(0, &[1.0, 1.0, 1.0]), 1.0);
    }

    /// A level beyond the range is clamped, one that is not a number is
    /// ignored, and a layer beyond the most there can be is passed over.
    #[test]
    fn settings_are_kept_in_range() {
        let mut mix = LayerMix::untouched();
        mix.set(
            0,
            LayerSetting {
                level_db: -200.0,
                ..LayerSetting::UNTOUCHED
            },
        );
        assert_eq!(mix.setting(0).level_db, LAYER_LEVEL_RANGE_DB.0);
        mix.set(
            0,
            LayerSetting {
                level_db: f32::NAN,
                ..LayerSetting::UNTOUCHED
            },
        );
        assert_eq!(mix.setting(0).level_db, LAYER_LEVEL_RANGE_DB.0);
        mix.set(
            MOST_LAYERS,
            LayerSetting {
                mute: true,
                ..LayerSetting::UNTOUCHED
            },
        );
        assert_eq!(mix.setting(MOST_LAYERS), LayerSetting::UNTOUCHED);
    }
}
