//! Weather → cloud deck: the one place `voxel-core`'s weather model meets the renderer's.
//!
//! Two crates deliberately do not know about each other. `voxel_core::weather` is pure
//! deterministic math with no GPU in it, and `voxel_environment::CloudSettings` is a GPU
//! payload description with no notion of a storm. This module is the seam, and it is the only
//! file that has to change when either side gains a field.
//!
//! It also owns the wind driver, because the deck's drift and the weather's severity must come
//! from ONE wind history. Two drivers would produce a sky whose clouds move at a speed the
//! weather does not agree with.

use voxel_core::weather::{WeatherKind, WeatherState};
use voxel_core::wind::{WindDriver, WindFrame, WindShape};
use voxel_environment::CloudSettings;

/// Drives the cloud deck from a weather state and a wind history.
pub struct SkyWeather {
    /// Named condition plus its transition. Public so a panel can retarget it.
    pub state: WeatherState,
    /// Authored deck parameters the weather does not set — scattering, step counts, powder.
    ///
    /// Separate from the weather because they are *quality and taste* knobs, not sky
    /// conditions: a storm and a clear day want the same phase function, and step counts belong
    /// to the performance tier rather than to the weather.
    pub deck: CloudSettings,
    /// Stop the weather model from writing the deck's shape.
    ///
    /// Without this the panel's coverage and thickness sliders would be dead controls: the
    /// weather frame rewrites those fields *every* frame, not only when a condition changes,
    /// because the wind's slow channel breathes coverage continuously. So a dragged slider
    /// would be overwritten before it was ever drawn.
    ///
    /// Cleared by [`Self::set_target`], so picking a condition hands control back to the
    /// weather rather than requiring the flag be found and unticked.
    pub manual: bool,
    /// Global multiplier for the condition's wind speed. `1.0` preserves the authored weather
    /// presets; it also scales the hand-dialled deck's current wind in manual mode.
    pub wind_speed_scale: f32,
    /// Compass direction the cloud deck moves toward, in degrees. Zero is +X and 90 is +Z.
    /// Stored as degrees here because that is what the weather panel exposes; the core weather
    /// model continues to consume radians.
    pub wind_direction_degrees: f32,
    manual_wind_speed: f32,
    wind: WindDriver,
    last_wind: WindFrame,
}

impl SkyWeather {
    /// Start from the authored hand-dialled baseline with a seeded wind history.
    ///
    /// Seeded rather than randomised so a session reproduces its own sky, which is the
    /// project's usual determinism rule and what makes a perf capture comparable.
    pub fn new(seed: u64) -> Self {
        let deck = CloudSettings::default();
        let manual_wind_speed = deck.wind[0].hypot(deck.wind[1]);
        Self {
            state: WeatherState::default(),
            deck,
            // The shipped baseline is the hand-dialled look. Selecting a named weather
            // condition calls `set_target`, which explicitly releases this mode.
            manual: true,
            wind_speed_scale: 1.0,
            wind_direction_degrees: 0.6_f32.to_degrees(),
            manual_wind_speed,
            wind: WindDriver::new(seed, WindShape::default()),
            last_wind: WindFrame::default(),
        }
    }

    /// This frame's wind, for anything else that wants to agree with the sky.
    pub fn wind(&self) -> WindFrame {
        self.last_wind
    }

    /// The mean wind bearing in radians — the angle the deck drifts along.
    ///
    /// An accessor rather than three callers writing `wind_direction_degrees.to_radians()`:
    /// [`SkyWeather::advance`] converts it for the weather state and the water wave field
    /// needs the same number, and a sea travelling on a bearing the sky disagreed with is
    /// exactly the failure this struct's one-wind-history rule exists to prevent.
    pub fn wind_bearing_radians(&self) -> f32 {
        self.wind_direction_degrees.to_radians()
    }

    /// Current cloud advection speed in metres per second, after the user control is applied.
    pub fn wind_speed_meters_per_second(&self) -> f32 {
        self.base_wind_speed_meters_per_second() * self.wind_speed_scale.max(0.0)
    }

    /// Set the current cloud advection speed in metres per second.
    ///
    /// The weather model still supplies the base speed for each condition; this converts the
    /// direct UI value into the internal multiplier so a storm remains faster than clear weather
    /// when the condition changes.
    pub fn set_wind_speed_meters_per_second(&mut self, speed: f32) {
        self.wind_speed_scale =
            speed.max(0.0) / self.base_wind_speed_meters_per_second().max(0.001);
    }

    /// Request a new sky condition, and hand deck control back to the weather.
    pub fn set_target(&mut self, target: WeatherKind) {
        self.manual = false;
        self.state.set_target(target);
    }

    fn base_wind_speed_meters_per_second(&self) -> f32 {
        if self.manual {
            self.manual_wind_speed
        } else {
            self.state
                .frame(self.last_wind.weather)
                .wind
                .into_iter()
                .map(|component| component * component)
                .sum::<f32>()
                .sqrt()
        }
    }

    /// Advance wind, weather and the deck's advection by one frame.
    ///
    /// Returns the deck to submit. The order matters: wind first, because the weather reads its
    /// slow channel, and the deck's own wind offset integrates the velocity the weather just
    /// produced — so a gust reaches the clouds on the frame it happens rather than the next.
    pub fn advance(&mut self, elapsed_seconds: f32) -> CloudSettings {
        self.state.wind_bearing_radians = self.wind_bearing_radians();
        self.last_wind = self.wind.advance(elapsed_seconds);
        self.state.advance(elapsed_seconds);
        let frame = self.state.frame(self.last_wind.weather);

        if !self.manual {
            self.deck.coverage = frame.coverage;
            self.deck.cloud_type = frame.cloud_type;
            self.deck.bottom_world = frame.bottom_world;
            self.deck.thickness_world = frame.thickness_world;
            self.deck.extinction = frame.extinction;
            self.deck.precipitation = frame.precipitation;
            self.deck.wind = frame.wind;
        }
        let base_speed = if self.manual {
            self.manual_wind_speed
        } else {
            frame.wind[0].hypot(frame.wind[1])
        };
        let speed = base_speed * self.wind_speed_scale.max(0.0);
        let direction = self.state.wind_bearing_radians;
        self.deck.wind = [direction.cos() * speed, direction.sin() * speed];
        // Advection runs either way. A hand-dialled deck that hangs motionless in the sky reads
        // as a bug rather than as a setting, and the wind keeps blowing regardless of who is
        // choosing the coverage.
        self.deck.advance(elapsed_seconds);
        self.deck
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// W3: the sea travels on the bearing the sky drifts on, across the whole seam.
    ///
    /// This is the assertion the one-wind-history rule in this module's header reduces to
    /// once water became the third consumer. It deliberately goes the long way — through
    /// `deck.wind`, the vector the CLOUD shader actually advects with, and through
    /// `WaveField::component(0)`, the direction the WATER shader actually uses — rather
    /// than comparing two copies of the bearing. Two identical angles prove nothing if
    /// one side then builds its vector with the axes swapped.
    #[test]
    fn the_sea_and_the_sky_share_one_bearing() {
        use crate::water::WaveField;

        for degrees in [0.0_f32, 34.4, 90.0, 217.5, 359.0] {
            let mut sky = SkyWeather::new(11);
            sky.wind_direction_degrees = degrees;
            let deck = sky.advance(1.0 / 60.0);

            let drift_speed = deck.wind[0].hypot(deck.wind[1]);
            assert!(
                drift_speed > 0.0,
                "the deck must be moving to have a bearing"
            );
            let drift = [deck.wind[0] / drift_speed, deck.wind[1] / drift_speed];

            let waves = WaveField::from_wind(sky.wind(), sky.wind_bearing_radians(), 1.0);
            // Component 0 carries no directional fan, so it IS the mean bearing.
            let travel = waves.component(0).direction;

            assert!(
                (drift[0] - travel.x).abs() < 1e-5 && (drift[1] - travel.y).abs() < 1e-5,
                "at {degrees} deg the deck drifts {drift:?} but the waves travel \
                 ({}, {})",
                travel.x,
                travel.y
            );
        }
    }

    /// And the wave field reads the same wind frame the weather does, not a second one.
    #[test]
    fn the_wave_field_reads_this_frames_wind() {
        let mut sky = SkyWeather::new(11);
        for _ in 0..240 {
            sky.advance(1.0 / 60.0);
        }
        let wind = sky.wind();
        let waves = crate::water::WaveField::from_wind(wind, sky.wind_bearing_radians(), 1.0);
        assert_eq!(waves.speed_meters_per_second, wind.speed);
        assert_eq!(waves.gust, wind.gust);
        assert_eq!(waves.eddy, wind.eddy);
        assert!(
            waves.total_steepness() > 0.0,
            "a live wind history must raise waves"
        );
    }

    #[test]
    fn advancing_drifts_the_deck() {
        let mut sky = SkyWeather::new(11);
        let before = sky.deck.wind_offset;
        let deck = sky.advance(1.0);
        assert_ne!(deck.wind_offset, before, "the deck must advect");
    }

    #[test]
    fn starts_in_the_authored_hand_dialled_mode() {
        let sky = SkyWeather::new(11);
        assert!(sky.manual);
        assert_eq!(sky.deck, CloudSettings::default());
    }

    /// A storm must reach the deck, not just the weather state.
    #[test]
    fn a_storm_reaches_the_deck() {
        let mut sky = SkyWeather::new(11);
        sky.state.transition_seconds = 1.0;
        sky.set_target(WeatherKind::Storm);
        let mut deck = sky.deck;
        for _ in 0..90 {
            deck = sky.advance(1.0 / 60.0);
        }
        let clear = WeatherKind::Clear.preset();
        assert!(deck.coverage > clear.coverage);
        assert!(deck.bottom_world < clear.bottom_world);
        assert!(deck.thickness_world > clear.thickness_world);
        assert!(deck.precipitation > clear.precipitation);
    }

    /// Taste knobs must survive a weather change — a storm must not reset the phase function or
    /// the step budget, which is the whole reason they are separate fields.
    #[test]
    fn weather_does_not_overwrite_authored_deck_knobs() {
        let mut sky = SkyWeather::new(11);
        sky.deck.primary_steps = 17;
        sky.deck.forward_scatter = 0.61;
        sky.state.transition_seconds = 0.5;
        sky.set_target(WeatherKind::Overcast);
        let deck = sky.advance(1.0);
        assert_eq!(deck.primary_steps, 17);
        assert!((deck.forward_scatter - 0.61).abs() < 1e-6);
    }

    /// The panel's shape sliders must survive a frame, or they are dead controls. This is the
    /// bug `manual` exists for: the weather frame rewrites coverage EVERY frame, not only on a
    /// condition change, because the wind's slow channel breathes it continuously.
    #[test]
    fn manual_mode_keeps_a_hand_dialled_deck() {
        let mut sky = SkyWeather::new(11);
        sky.manual = true;
        sky.deck.coverage = 0.13;
        sky.deck.thickness_world = 404.0;
        for _ in 0..30 {
            sky.advance(1.0 / 60.0);
        }
        assert!((sky.deck.coverage - 0.13).abs() < 1e-6);
        assert!((sky.deck.thickness_world - 404.0).abs() < 1e-6);
    }

    /// A hand-dialled deck must still drift, or it reads as a bug rather than a setting.
    #[test]
    fn manual_mode_still_advects() {
        let mut sky = SkyWeather::new(11);
        sky.manual = true;
        let before = sky.deck.wind_offset;
        sky.advance(1.0);
        assert_ne!(sky.deck.wind_offset, before);
    }

    #[test]
    fn authored_wind_controls_override_manual_deck_velocity() {
        let mut sky = SkyWeather::new(11);
        sky.manual = true;
        sky.set_wind_speed_meters_per_second(10.5);
        sky.wind_direction_degrees = 90.0;
        let deck = sky.advance(1.0);
        assert!(deck.wind[0].abs() < 1.0e-5);
        assert!(deck.wind[1] > 10.0);
        assert!((sky.wind_speed_meters_per_second() - 10.5).abs() < 1.0e-4);
    }

    /// Picking a condition must hand control back, rather than silently doing nothing because a
    /// flag is still set somewhere the user has forgotten about.
    #[test]
    fn choosing_a_condition_releases_manual_mode() {
        let mut sky = SkyWeather::new(11);
        sky.manual = true;
        sky.deck.coverage = 0.13;
        sky.set_target(WeatherKind::Storm);
        assert!(!sky.manual);
        sky.advance(1.0 / 60.0);
        assert!(sky.deck.coverage > 0.13, "the weather must drive again");
    }

    /// The shadow map must resolve at least one texel per world voxel across the whole world.
    ///
    /// This test exists in `voxel-rt` rather than in `voxel-environment` because it is the only
    /// crate that can see both numbers: the environment owns the shadow-map extent and resolution,
    /// `voxel-core` owns the world's size, and the environment crate deliberately does not depend on
    /// the world. So the relationship between them was previously asserted nowhere — and the extent
    /// shipped 8x too coarse, giving the ground a smooth wash rather than cloud shadows.
    #[test]
    fn cloud_shadow_extent_resolves_a_world_voxel_per_texel() {
        let metres_per_texel = voxel_environment::CLOUD_SHADOW_EXTENT_WORLD
            / voxel_environment::CLOUD_SHADOW_EDGE as f32;
        assert!(
            metres_per_texel <= voxel_core::world::WORLD_VOXEL_SIZE_METERS,
            "{metres_per_texel} m per shadow texel is coarser than a {} m world voxel",
            voxel_core::world::WORLD_VOXEL_SIZE_METERS
        );
        // And the map must still cover the world it is centred on, with room for the viewer to
        // stand off-centre.
        let world_metres =
            voxel_core::world::WORLD_VOXELS_X as f32 * voxel_core::world::WORLD_VOXEL_SIZE_METERS;
        assert!(
            voxel_environment::CLOUD_SHADOW_EXTENT_WORLD >= world_metres * 2.0,
            "extent {} m does not comfortably cover a {world_metres} m world",
            voxel_environment::CLOUD_SHADOW_EXTENT_WORLD
        );
    }

    /// The cloud deck sits BEYOND the tracer's give-up radius, so the sky-miss path must never
    /// bound its cloud march by that radius.
    ///
    /// This is the bug that made the whole deck invisible. `MAX_TRACE_DISTANCE` is a sentinel
    /// meaning "hit nothing", but it was passed to `sky_color_at_distance` as if it were a depth,
    /// and the march clamped to it. Since the trace radius is shorter than the lowest cloud base,
    /// the clamp emptied the march for every direction more than ~44 degrees off vertical.
    ///
    /// Checked here because this crate is the only one that sees both numbers: the trace radius
    /// lives in this crate's WGSL, the cloud altitudes in `voxel-core`'s weather presets.
    #[test]
    fn the_deck_is_out_of_reach_of_the_trace_radius_so_the_sky_march_must_be_unbounded() {
        let world = include_str!("../shaders/world.wgsl");
        let trace_radius: f32 = world
            .lines()
            .find_map(|line| {
                line.split_once("const MAX_TRACE_DISTANCE: f32 =")
                    .map(|(_, value)| value)
            })
            .expect("MAX_TRACE_DISTANCE is declared in world.wgsl")
            .trim()
            .trim_end_matches(';')
            .parse()
            .expect("MAX_TRACE_DISTANCE parses as a float");
        let trace_metres = trace_radius * voxel_core::world::WORLD_VOXEL_SIZE_METERS;

        let lowest_base = [
            voxel_core::weather::WeatherKind::Clear,
            voxel_core::weather::WeatherKind::Scattered,
            voxel_core::weather::WeatherKind::Overcast,
            voxel_core::weather::WeatherKind::Storm,
        ]
        .into_iter()
        .map(|kind| kind.preset().bottom_world)
        .fold(f32::INFINITY, f32::min);

        // The deck is a horizontal slab, so the distance at which a ray ENTERS it is
        // `(base - eye) / direction.y` — which diverges as the ray approaches horizontal. No finite
        // bound can keep the deck visible to the horizon, and the horizon is where most of the
        // sky's cloud is. Checked at a thoroughly ordinary 20 degrees of elevation, against the
        // LOWEST deck any preset asks for, which is the most favourable case for a bound.
        let eye_height =
            voxel_core::world::WORLD_VOXELS_Y as f32 * voxel_core::world::WORLD_VOXEL_SIZE_METERS;
        let entry_at_20_degrees = (lowest_base - eye_height) / 20.0_f32.to_radians().sin();
        assert!(
            entry_at_20_degrees > trace_metres,
            "the lowest deck ({lowest_base} m) is entered at {entry_at_20_degrees} m looking 20 \
             degrees up, which the {trace_metres} m trace radius still reaches — re-derive \
             whether the sky march may be bounded again"
        );

        // And the miss path must therefore hand the march no bound at all.
        let dispatch = voxel_environment::HillaireEnvironment::WGSL;
        let sky_at_distance = dispatch
            .split_once("fn sky_color_at_distance")
            .expect("sky_color_at_distance exists")
            .1;
        let body = &sky_at_distance[..sky_at_distance
            .find("\nfn ")
            .unwrap_or(sky_at_distance.len())];
        // Asserted POSITIVELY — that the unbounded literal is present — rather than negatively that
        // `distance_world` is absent. A negative string match passes when the code is merely
        // reformatted, which is the failure mode where a guard silently stops guarding.
        assert!(
            body.contains("1.0e7"),
            "the sky-miss path no longer marches the deck unbounded; a finite bound here clips \
             every cloud toward the horizon"
        );
    }

    /// Same seed, same sky. A perf capture that cannot be repeated cannot be compared.
    #[test]
    fn the_same_seed_reproduces_the_same_sky() {
        let run = || {
            let mut sky = SkyWeather::new(23);
            let mut coverage = Vec::new();
            for _ in 0..120 {
                coverage.push(sky.advance(1.0 / 60.0).coverage);
            }
            coverage
        };
        assert_eq!(run(), run());
    }
}
