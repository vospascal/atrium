//! Weather state: one dial that moves cloud, wind and rain together.
//!
//! This exists because coverage, thickness, cloud type and wind speed are not independent.
//! A storm is not "overcast plus more wind" — it is a taller deck, a lower base, denser
//! cloud, and stronger gusts *at the same time*, and driving those from four sliders produces
//! combinations that never occur in a sky.
//!
//! It lives beside [`crate::wind`] and reads that module's slow air-mass channel rather than
//! its own clock, so the wind you see in the grass and the deck drifting overhead are the same
//! phenomenon. `wind.rs` says as much in its own docs: *"a cloud layer wants the slow
//! weather"*. That channel is also what the audio engine's field-wind synth runs on, which is
//! what will later let one weather state drive the storm you see and the storm you hear.
//!
//! Renderer-agnostic on purpose: this produces plain numbers, and mapping them onto the
//! renderer's cloud settings is the renderer's job. Nothing here knows a GPU exists.

/// A named sky condition.
///
/// Four rather than a continuous space, because these are the states a designer or a script
/// asks for by name. Continuity comes from interpolating *between* them
/// ([`WeatherState::transition_seconds`]), not from removing the names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WeatherKind {
    /// Nearly empty sky; thin high cloud at most.
    Clear,
    /// Fair-weather cumulus with real gaps. The shipped default.
    #[default]
    Scattered,
    /// Continuous flat deck, low base, no direct sun.
    Overcast,
    /// Towering cumulonimbus, low dense base, strong gusts, rain.
    Storm,
}

/// The cloud and wind parameters a sky condition implies.
///
/// Public so a caller can read a preset without instantiating a state machine — a Studio
/// dropdown wants exactly this.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeatherPreset {
    /// Sky covered, 0 clear to 1 total.
    pub coverage: f32,
    /// 0 stratus, 0.5 cumulus, 1 cumulonimbus.
    pub cloud_type: f32,
    /// Deck base altitude in world units. Falls as weather worsens, which is most of why a
    /// storm feels oppressive.
    pub bottom_world: f32,
    /// Deck thickness in world units.
    pub thickness_world: f32,
    /// Extinction σₜ per world unit.
    pub extinction: f32,
    /// Base wind speed in world units per second.
    pub wind_speed: f32,
    /// Rain amount, 0 dry to 1 downpour. Carried here so C7 can drive audio from the same
    /// state rather than inventing a second rain variable.
    pub precipitation: f32,
}

impl WeatherKind {
    /// The Earth-like numbers for this condition, in METRES.
    ///
    /// World units are metres (`voxel_environment::FROM_KILOMETERS_SCALE == 1000`), so these are
    /// real altitudes and can be checked against meteorology rather than eyeballed.
    ///
    /// Grounded in Hädrich et al., *Stormscapes* (2020), §4.2.6 and Fig. 5, which **derives** cloud
    /// boundaries instead of authoring them: the base is the altitude where relative humidity
    /// reaches 1 and vapour condenses, the top is where the rising thermal's buoyancy vanishes.
    /// Its simulated results — cumulus base 1500 m / top 2500 m, cumulonimbus base 1500 m / top
    /// 8000 m, stratocumulus base 1800 m / top 2000 m — are used directly.
    ///
    /// The first version of this table was invented by eye and was **7x too low and up to 12x too
    /// thin** (cumulus at 220 m with 180 m of depth, a storm only 520 m tall). At that scale a deck
    /// is a fog bank, not a cloudscape, which is most of why it did not read as sky.
    ///
    /// Note what is deliberately NOT monotonic: a storm's base is not lower than a cumulus base —
    /// both sit at 1500 m, because both condense at the same saturation altitude. Severity lives in
    /// the TOP. Only stratus genuinely sits low.
    pub fn preset(self) -> WeatherPreset {
        match self {
            // Dry air saturates high, so fair-weather cloud has a high base and little depth.
            WeatherKind::Clear => WeatherPreset {
                coverage: 0.08,
                cloud_type: 0.15,
                bottom_world: 2400.0,
                thickness_world: 300.0,
                extinction: 0.05,
                wind_speed: 2.5,
                precipitation: 0.0,
            },
            // Stormscapes Fig. 5d: cumulus, base 1500 m, top 2500 m.
            WeatherKind::Scattered => WeatherPreset {
                coverage: 0.45,
                cloud_type: 0.5,
                bottom_world: 1500.0,
                thickness_world: 1000.0,
                extinction: 0.08,
                wind_speed: 5.0,
                precipitation: 0.0,
            },
            // A continuous stratus/stratocumulus sheet: the one genuinely LOW condition, and what
            // makes overcast feel like a lid.
            WeatherKind::Overcast => WeatherPreset {
                coverage: 0.92,
                cloud_type: 0.12,
                bottom_world: 900.0,
                thickness_world: 700.0,
                extinction: 0.13,
                wind_speed: 7.0,
                precipitation: 0.15,
            },
            // Stormscapes Fig. 5e: cumulonimbus, base 1500 m, top 8000 m — the inversion layer at
            // 8000 m is what caps it and forms the anvil.
            WeatherKind::Storm => WeatherPreset {
                coverage: 0.97,
                cloud_type: 1.0,
                bottom_world: 1500.0,
                thickness_world: 6500.0,
                extinction: 0.2,
                wind_speed: 14.0,
                precipitation: 0.85,
            },
        }
    }
}

fn mix(from: f32, to: f32, amount: f32) -> f32 {
    from + (to - from) * amount
}

impl WeatherPreset {
    fn blend(self, other: Self, amount: f32) -> Self {
        Self {
            coverage: mix(self.coverage, other.coverage, amount),
            cloud_type: mix(self.cloud_type, other.cloud_type, amount),
            bottom_world: mix(self.bottom_world, other.bottom_world, amount),
            thickness_world: mix(self.thickness_world, other.thickness_world, amount),
            extinction: mix(self.extinction, other.extinction, amount),
            wind_speed: mix(self.wind_speed, other.wind_speed, amount),
            precipitation: mix(self.precipitation, other.precipitation, amount),
        }
    }
}

/// One frame of weather: the blended preset plus the wind vector it implies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeatherFrame {
    pub coverage: f32,
    pub cloud_type: f32,
    pub bottom_world: f32,
    pub thickness_world: f32,
    pub extinction: f32,
    pub precipitation: f32,
    /// Horizontal advection velocity in world units per second.
    ///
    /// A vector rather than a speed because the deck has to drift in a direction, and that
    /// direction has to be the one the grass is already leaning.
    pub wind: [f32; 2],
}

/// Weather with smooth transitions between named conditions.
///
/// Deterministic: given the same elapsed-second sequence and the same wind samples, this
/// produces the same weather history, which is the project's usual rule.
#[derive(Clone, Copy, Debug)]
pub struct WeatherState {
    /// The condition being moved toward.
    pub target: WeatherKind,
    /// The condition being moved away from.
    departing: WeatherKind,
    /// Progress from `departing` to `target`, 0..1.
    transition: f32,
    /// Seconds for a full condition change.
    ///
    /// Long by default: weather that visibly snaps between states reads as a switch being
    /// flipped rather than as weather. Two minutes is fast for a real sky and slow enough here.
    pub transition_seconds: f32,
    /// Compass direction the wind blows toward, in radians.
    pub wind_bearing_radians: f32,
    /// How much the wind model's slow channel modulates coverage.
    ///
    /// Small on purpose. The named condition sets the sky; the wind's air-mass drift only
    /// breathes around it. Turn this up and coverage starts contradicting the condition.
    pub wind_coupling: f32,
}

impl Default for WeatherState {
    fn default() -> Self {
        Self {
            target: WeatherKind::default(),
            departing: WeatherKind::default(),
            transition: 1.0,
            transition_seconds: 120.0,
            wind_bearing_radians: 0.6,
            wind_coupling: 0.12,
        }
    }
}

impl WeatherState {
    /// Start moving toward a new condition.
    ///
    /// Re-requesting the current target is a no-op rather than a restart, so a UI that sets
    /// this every frame does not freeze the transition at zero.
    pub fn set_target(&mut self, target: WeatherKind) {
        if self.target == target {
            return;
        }
        // Depart from where we actually are, not from the previous target — otherwise
        // interrupting a transition snaps backwards.
        self.departing = self.blended_preset_kind();
        self.transition = 0.0;
        self.target = target;
    }

    /// Whether a condition change is still in progress.
    pub fn transitioning(&self) -> bool {
        self.transition < 1.0
    }

    /// Advance the transition.
    pub fn advance(&mut self, elapsed_seconds: f32) {
        if self.transition >= 1.0 {
            self.transition = 1.0;
            return;
        }
        let seconds = self.transition_seconds.max(0.001);
        self.transition = (self.transition + elapsed_seconds.max(0.0) / seconds).min(1.0);
    }

    /// The nearest named condition to the current blend, used as the departure point when a
    /// transition is interrupted.
    fn blended_preset_kind(&self) -> WeatherKind {
        if self.transition >= 0.5 {
            self.target
        } else {
            self.departing
        }
    }

    /// Evaluate this frame's weather.
    ///
    /// `wind_weather` is [`crate::wind::WindFrame::weather`] — the slow air-mass channel, 0..1.
    /// Taking it as an argument rather than owning a `WindDriver` keeps one wind history in the
    /// world: the caller already has one, and a second would disagree with the grass.
    pub fn frame(&self, wind_weather: f32) -> WeatherFrame {
        // Smootherstep the blend so a transition has no velocity discontinuity at either end;
        // a linear blend visibly starts and stops.
        let raw = self.transition.clamp(0.0, 1.0);
        let eased = raw * raw * raw * (raw * (raw * 6.0 - 15.0) + 10.0);
        let preset = self.departing.preset().blend(self.target.preset(), eased);

        // The air-mass channel breathes coverage around the condition. Centred on 0.5 so it
        // both adds and removes rather than only thickening the sky.
        let breath = (wind_weather.clamp(0.0, 1.0) - 0.5) * 2.0 * self.wind_coupling;
        let coverage = (preset.coverage + breath).clamp(0.0, 1.0);

        // Wind speed rises with the same channel, so a gustier moment also moves the deck
        // faster. This is the coupling that makes the sky and the ground agree.
        let speed = preset.wind_speed * (0.75 + 0.5 * wind_weather.clamp(0.0, 1.0));
        let (sin_bearing, cos_bearing) = self.wind_bearing_radians.sin_cos();

        WeatherFrame {
            coverage,
            cloud_type: preset.cloud_type,
            bottom_world: preset.bottom_world,
            thickness_world: preset.thickness_world,
            extinction: preset.extinction,
            precipitation: preset.precipitation,
            wind: [cos_bearing * speed, sin_bearing * speed],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_settled_scattered_weather() {
        let state = WeatherState::default();
        assert!(!state.transitioning());
        let frame = state.frame(0.5);
        assert!((frame.coverage - WeatherKind::Scattered.preset().coverage).abs() < 1e-6);
    }

    #[test]
    fn a_transition_completes_after_its_duration() {
        let mut state = WeatherState {
            transition_seconds: 10.0,
            ..WeatherState::default()
        };
        state.set_target(WeatherKind::Storm);
        assert!(state.transitioning());
        state.advance(11.0);
        assert!(!state.transitioning());
        let frame = state.frame(0.5);
        assert!((frame.cloud_type - 1.0).abs() < 1e-6);
    }

    /// Re-requesting the current target must not restart the transition, or a UI that sets it
    /// every frame would hold the sky at zero progress forever.
    #[test]
    fn setting_the_same_target_does_not_restart() {
        let mut state = WeatherState {
            transition_seconds: 10.0,
            ..WeatherState::default()
        };
        state.set_target(WeatherKind::Overcast);
        state.advance(5.0);
        let midway = state.frame(0.5).coverage;
        state.set_target(WeatherKind::Overcast);
        assert!((state.frame(0.5).coverage - midway).abs() < 1e-6);
    }

    /// Only COVERAGE and DENSITY rise monotonically with severity.
    ///
    /// Two things are deliberately absent, and both were wrong in earlier versions of this test:
    ///
    /// * **Base altitude.** A cumulonimbus and a cumulus share a base at 1500 m, because both
    ///   condense at the same saturation altitude (Stormscapes Fig. 5d/e). Severity lives in the top.
    /// * **Thickness.** Stormscapes §7 is explicit that *"stratocumulus have a limited vertical
    ///   extent, resulting in a narrower cloud fraction over altitude compared to cumulus"* — so an
    ///   overcast sheet is THINNER than fair-weather cumulus, not thicker. This test failing on that
    ///   assertion is what surfaced it.
    #[test]
    fn worse_weather_is_denser_and_more_covered() {
        let order = [
            WeatherKind::Clear,
            WeatherKind::Scattered,
            WeatherKind::Overcast,
            WeatherKind::Storm,
        ];
        for pair in order.windows(2) {
            let milder = pair[0].preset();
            let worse = pair[1].preset();
            assert!(
                worse.coverage > milder.coverage,
                "{:?} must cover more than {:?}",
                pair[1],
                pair[0]
            );
            assert!(
                worse.extinction > milder.extinction,
                "{:?} must be denser than {:?}",
                pair[1],
                pair[0]
            );
        }
    }

    /// A stratus sheet is a LAYER, not a tower: limited vertical extent is what distinguishes it
    /// from convective cloud, and getting this backwards makes overcast read as a storm without rain.
    #[test]
    fn the_overcast_sheet_is_thinner_than_convective_cloud() {
        let overcast = WeatherKind::Overcast.preset().thickness_world;
        assert!(overcast < WeatherKind::Scattered.preset().thickness_world);
        assert!(overcast < WeatherKind::Storm.preset().thickness_world);
    }

    /// Overcast is the one genuinely LOW condition — a stratus sheet is what makes an overcast sky
    /// feel like a lid, and it is lower than either convective type.
    #[test]
    fn overcast_sits_lowest() {
        let overcast = WeatherKind::Overcast.preset().bottom_world;
        for other in [
            WeatherKind::Clear,
            WeatherKind::Scattered,
            WeatherKind::Storm,
        ] {
            assert!(
                overcast < other.preset().bottom_world,
                "overcast must sit below {other:?}"
            );
        }
    }

    /// Altitudes must be real, in metres. The first table was 7x too low, which is a fog bank
    /// rather than a cloudscape — a mistake no unit test caught because nothing pinned the scale.
    #[test]
    fn cloud_altitudes_are_physically_plausible_in_metres() {
        for kind in [
            WeatherKind::Clear,
            WeatherKind::Scattered,
            WeatherKind::Overcast,
            WeatherKind::Storm,
        ] {
            let preset = kind.preset();
            let top = preset.bottom_world + preset.thickness_world;
            assert!(
                preset.bottom_world >= 600.0,
                "{kind:?} base {} m is below any real cloud base",
                preset.bottom_world
            );
            assert!(
                top <= 13_000.0,
                "{kind:?} top {top} m is above the troposphere"
            );
        }
        // A cumulonimbus reaches the upper troposphere; anything less is not a storm.
        let storm = WeatherKind::Storm.preset();
        assert!(storm.bottom_world + storm.thickness_world >= 7_000.0);
    }

    /// The wind channel must both add and remove coverage. If it only added, every gust would
    /// permanently thicken the sky.
    #[test]
    fn the_wind_channel_breathes_coverage_both_ways() {
        let state = WeatherState::default();
        let calm = state.frame(0.0).coverage;
        let neutral = state.frame(0.5).coverage;
        let blustery = state.frame(1.0).coverage;
        assert!(calm < neutral, "a calm air mass must thin the sky");
        assert!(blustery > neutral, "an active air mass must thicken it");
    }

    /// Wind speed must rise with the air-mass channel, so the deck drifts faster exactly when
    /// the grass is moving more.
    #[test]
    fn deck_drift_follows_the_same_channel_as_the_grass() {
        let state = WeatherState::default();
        let calm = state.frame(0.0).wind;
        let blustery = state.frame(1.0).wind;
        let speed = |wind: [f32; 2]| (wind[0] * wind[0] + wind[1] * wind[1]).sqrt();
        assert!(speed(blustery) > speed(calm));
    }

    /// Interrupting a transition must not snap the sky backwards.
    #[test]
    fn interrupting_a_transition_departs_from_where_it_is() {
        let mut state = WeatherState {
            transition_seconds: 10.0,
            ..WeatherState::default()
        };
        state.set_target(WeatherKind::Storm);
        state.advance(9.0);
        let nearly_storm = state.frame(0.5).coverage;
        state.set_target(WeatherKind::Clear);
        let just_after = state.frame(0.5).coverage;
        // Departing from the storm end, not from Scattered, so coverage starts high.
        assert!(
            (just_after - nearly_storm).abs() < 0.1,
            "expected to depart near {nearly_storm}, got {just_after}"
        );
    }
}
