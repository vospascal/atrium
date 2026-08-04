//! The curves themselves, on the CPU — the other half of what
//! [`super::WGSL`] runs on the GPU.
//!
//! **These used to live inside `#[cfg(test)]`, and promoting them is the point of this
//! module.** The crate claims to own both halves of the tonemap; while the Rust half was
//! test scaffolding that claim was only three quarters true, and nothing outside a test
//! binary could evaluate a curve. `pattern.rs` and `material_graph.rs` over in `voxel-rt`
//! both carry production CPU reference evaluators beside their WGSL for exactly this
//! reason, so this is the crate catching up with the house pattern rather than inventing
//! one.
//!
//! **Mirror the WGSL, always.** Every function here has a counterpart in
//! `shaders/tonemap.wgsl` and the two must agree; where they differ, one is a bug. The
//! tests below check properties (monotonicity, C¹ joins, ceilings) rather than transcribing
//! the same arithmetic a second time, because a test that restates the implementation
//! only proves you can type it twice.
//!
//! ## Five curves are scalar, one is not, and the signatures say so
//!
//! [`reinhard`], [`reinhard_headroom`], [`hdr_knee`], [`hable`] and [`bt2390`] take and
//! return an `f32`: they are per-channel operators, applied independently to R, G and B.
//! [`gt7`] takes and returns `[f32; 3]` because it is a **colour-volume** operator — it
//! blends a per-channel pass with a chroma-preserving pass through ICtCp and cannot be
//! expressed one channel at a time. That is the whole reason it holds highlight hue where
//! the others desaturate, and it is worth having in the type rather than only in prose.
//!
//! [`apply`] dispatches over [`TonemapCurve`] and is the mirror of the shader's
//! `apply_tonemap`. Its `match` is exhaustive, so **a new curve cannot reach `main` without
//! a CPU implementation** — the compiler enforces here what a string test has to enforce on
//! the WGSL side.

#[cfg(test)]
pub(crate) use super::bt2390::PQ_RELATIVE_CEILING;
pub use super::bt2390::{bt2390, pq_from_relative, relative_from_pq};
pub use super::dispatch::apply;
pub use super::gt7::{gt7, gt7_curve, ictcp_to_rgb, rgb_to_ictcp};
pub use super::hable::hable;
#[cfg(test)]
pub(crate) use super::hable::hable_partial;
pub use super::reinhard::{reinhard, reinhard_headroom};
pub use super::{hdr_knee::hdr_knee, TonemapCurve};

#[cfg(test)]
mod tests {
    use super::*;

    /// Reinhard+HDR keeps the SDR curve exactly through scene white, not merely within a
    /// tolerance at a few low samples.
    #[test]
    fn reinhard_headroom_matches_plain_reinhard_through_scene_white() {
        for luminance in [0.0, 0.01, 0.05, 0.1, 0.18, 0.25, 0.5, 1.0] {
            let plain = reinhard(luminance);
            assert_eq!(reinhard_headroom(luminance, 16.0), plain);
        }
    }

    /// The conservative 1x fallback must reproduce SDR for the WHOLE curve. The old
    /// white-point equation became identity at W=1, which brightened mid-tones and left
    /// the compositor to hard-clip highlights.
    #[test]
    fn reinhard_headroom_at_one_x_is_exactly_sdr() {
        for luminance in [0.0, 0.01, 0.18, 0.5, 1.0, 2.0, 16.0, 1.0e6] {
            assert_eq!(reinhard_headroom(luminance, 1.0), reinhard(luminance));
        }
    }

    #[test]
    fn reinhard_headroom_is_monotonic_and_bounded_by_the_display() {
        for headroom in [1.0, 1.6, 4.0, 16.0] {
            let mut previous = -1.0;
            for step in 0..400 {
                let luminance = step as f32 * 0.25;
                let mapped = reinhard_headroom(luminance, headroom);
                assert!(mapped >= previous, "{headroom}x: {previous} -> {mapped}");
                assert!(mapped <= headroom, "{headroom}x: {luminance} -> {mapped}");
                previous = mapped;
            }
        }
    }

    /// And the knee does NOT match, which is the measured explanation for "the room got
    /// brighter". Stated as a test so the difference is a number rather than an anecdote.
    #[test]
    fn the_knee_is_brighter_than_reinhard_everywhere_below_white() {
        for luminance in [0.1, 0.25, 0.5, 0.9] {
            let compressed = reinhard(luminance);
            let untouched = hdr_knee(luminance, 4.0);
            assert!(
                untouched > compressed,
                "at {luminance} linear the knee ({untouched}) should exceed Reinhard \
                 ({compressed})"
            );
        }
        // The headline number: Reinhard maps linear 0.5 to 0.33, the knee leaves it at
        // 0.5. Half a stop of mid-tone difference is not subtle.
        assert!((reinhard(0.5) - 0.3333).abs() < 0.001);
        assert!((hdr_knee(0.5, 4.0) - 0.5).abs() < 0.001);
    }

    /// Reinhard cannot reach above white however bright the input, which is why it is
    /// unusable as an HDR curve and why `can_exceed_white` reports it.
    #[test]
    fn only_the_hdr_curves_can_exceed_white() {
        assert!(reinhard(1.0e6) < 1.0);
        assert!(!TonemapCurve::Reinhard.can_exceed_white());

        assert!(reinhard_headroom(100.0, 4.0) > 1.0);
        assert!(TonemapCurve::ReinhardHeadroom.can_exceed_white());

        assert!(hdr_knee(100.0, 4.0) > 1.0);
        assert!(TonemapCurve::HdrKnee.can_exceed_white());
    }

    /// At zero headroom the knee must degenerate to a hard clip rather than divide by
    /// zero. `room = 0` makes the shoulder term `0/0` if written naively.
    #[test]
    fn the_knee_survives_zero_headroom() {
        for luminance in [0.0, 0.5, 1.0, 4.0, 1.0e6] {
            let mapped = hdr_knee(luminance, 1.0);
            assert!(mapped.is_finite(), "{luminance} produced {mapped}");
            assert!(mapped <= 1.0, "{luminance} exceeded white with no headroom");
        }
    }

    /// The knee's reason for existing: C¹ at white. The obvious form,
    /// `highs/(1+highs) * (headroom-1)`, has derivative `headroom-1` there, so the slope
    /// jumps from 1 to 3 at exactly the luminance most of the image sits near.
    #[test]
    fn the_knee_is_c1_at_white() {
        let headroom = 4.0;
        let step = 1.0e-4;
        let below = (hdr_knee(1.0, headroom) - hdr_knee(1.0 - step, headroom)) / step;
        let above = (hdr_knee(1.0 + step, headroom) - hdr_knee(1.0, headroom)) / step;
        assert!(
            (below - above).abs() < 0.01,
            "slope {below} below white vs {above} above — that kink is what putting \
             `room` in the denominator fixes"
        );
    }

    /// The `/ f(W)` normalisation is the half people drop, and dropping it is invisible
    /// except that everything looks washed — the curve would peak around 0.8 instead of
    /// reaching white. Pinned by checking that the linear white point actually lands ON
    /// white.
    #[test]
    fn hable_normalises_so_its_white_point_reaches_white() {
        let unnormalised_peak = hable_partial(11.2);
        assert!(
            unnormalised_peak < 0.9,
            "f(W) is {unnormalised_peak}; without dividing by it the curve never reaches \
             white, which is the classic mis-implementation"
        );
        // With the bias, W/2 is the input that maps to display white.
        assert!((hable(11.2 / 2.0) - 1.0).abs() < 0.001);
        assert!(hable(0.0).abs() < 1.0e-6);
    }

    /// The clamp is load-bearing, and the reason is not obvious: `hable_partial` is
    /// bounded by `1 - E/F` = 0.933, and f(W) is SMALLER than that, so the normalised ratio
    /// climbs past 1.0 to roughly 1.17 rather than stopping at white. The original relied
    /// on an 8-bit framebuffer clamping it; a float surface would keep it, as a fixed ~17%
    /// overshoot that does not scale with headroom and so is not usable range.
    #[test]
    fn hable_overshoots_white_before_the_clamp_which_is_why_there_is_one() {
        let unclamped = hable_partial(1.0e6 * 2.0) / hable_partial(11.2);
        assert!(
            unclamped > 1.0,
            "expected the raw ratio to exceed white; got {unclamped}"
        );
        assert!(
            unclamped < 1.3,
            "the overshoot should be a modest artifact, not headroom; got {unclamped}"
        );
    }

    /// Hable is an SDR operator: normalising by `f(W)` pins the ceiling at 1.0, so it can
    /// no more carry HDR headroom than plain Reinhard can. Selecting it on an HDR surface
    /// is legal but produces no HDR, and `can_exceed_white` has to say so.
    #[test]
    fn hable_cannot_exceed_white_however_bright_the_input() {
        for luminance in [1.0, 10.0, 100.0, 1.0e6] {
            let mapped = hable(luminance);
            assert!(
                mapped <= 1.0 + 1.0e-4,
                "{luminance} mapped to {mapped}, above white — Hable is SDR by construction"
            );
        }
        assert!(!TonemapCurve::HableFilmic.can_exceed_white());
    }

    /// It is a FILMIC curve, which is the reason to offer it at all: a toe that darkens
    /// the low end relative to Reinhard, and a shoulder that holds the top. If it merely
    /// tracked Reinhard there would be no point having both.
    #[test]
    fn hable_has_a_toe_that_reinhard_does_not() {
        // Deep shadow: the toe pulls it below Reinhard.
        assert!(
            hable(0.02) < reinhard(0.02),
            "hable {} vs reinhard {} — the toe should crush the low end",
            hable(0.02),
            reinhard(0.02)
        );
        // Upper mid: the shoulder holds more than Reinhard's compression does.
        assert!(
            hable(2.0) > reinhard(2.0),
            "hable {} vs reinhard {}",
            hable(2.0),
            reinhard(2.0)
        );
    }

    /// PQ has to round-trip, or every BT.2390 property below is measuring the wrong thing.
    /// Checked at the anchors that matter: SDR white, the PQ ceiling, and a mid value.
    #[test]
    fn pq_round_trips_across_the_range() {
        for relative in [0.0, 0.18, 1.0, 4.0, 10.0, 100.0] {
            let recovered = relative_from_pq(pq_from_relative(relative));
            assert!(
                (recovered - relative).abs() < relative.max(1.0) * 0.001,
                "{relative} round-tripped to {recovered}"
            );
        }
        // The convention bridge: 1.0 relative is 100 cd/m², and PQ signal 1.0 is 10 000.
        assert!((relative_from_pq(1.0) - PQ_RELATIVE_CEILING).abs() < 0.5);
    }

    /// THE POINT OF THIS CURVE: the knee is computed from the display, so it MOVES when
    /// the display does. A brighter panel can show more of the content, so `maxLum` rises,
    /// so `KS = 1.5·maxLum − 0.5` rises, so compression starts later. Nothing hand-placed
    /// can do this — it is the difference between BT.2390 and our own knee.
    #[test]
    fn the_knee_point_moves_with_the_display() {
        let content_peak = 10.0;
        let knee_for = |display_peak: f32| {
            let content_pq = pq_from_relative(content_peak);
            let max_lum = pq_from_relative(display_peak) / content_pq;
            1.5 * max_lum - 0.5
        };
        let dim = knee_for(1.5);
        let bright = knee_for(8.0);
        assert!(
            bright > dim,
            "a brighter display should push the knee later: {dim} -> {bright}"
        );
    }

    /// Below the knee the signal must pass through UNTOUCHED — that is what makes the
    /// mid-tones faithful and is the half of the curve most content lives in.
    #[test]
    fn bt2390_leaves_the_mid_tones_alone() {
        for relative in [0.05, 0.18, 0.5, 1.0] {
            let mapped = bt2390(relative, 4.0, 10.0);
            assert!(
                (mapped - relative).abs() < relative * 0.05,
                "{relative} moved to {mapped}; below the knee should be identity"
            );
        }
    }

    /// And above it, content brighter than the display must be brought DOWN to the
    /// display's peak rather than clipped — the entire job of a display-mapping curve.
    #[test]
    fn bt2390_maps_the_content_peak_into_the_display_peak() {
        let display_peak = 4.0;
        let content_peak = 20.0;
        let mapped = bt2390(content_peak, display_peak, content_peak);
        assert!(
            mapped <= display_peak * 1.02,
            "content peak {content_peak} mapped to {mapped}, above the display's \
             {display_peak}"
        );
        assert!(
            mapped > display_peak * 0.7,
            "mapped to {mapped}, far below the display's {display_peak} — the shoulder \
             should reach most of the way, not throw the range away"
        );
    }

    /// Monotonic, or brighter scene values would render darker — a visible inversion in
    /// exactly the highlights the curve exists to handle.
    #[test]
    fn bt2390_is_monotonic() {
        let mut previous = -1.0;
        for step in 0..200 {
            let relative = step as f32 * 0.25;
            let mapped = bt2390(relative, 4.0, 20.0);
            assert!(
                mapped >= previous - 1.0e-4,
                "at {relative} the curve went backwards: {previous} -> {mapped}"
            );
            previous = mapped;
        }
    }

    /// When the display can already show everything, there is nothing to map and the curve
    /// must be identity — not "nearly identity". Otherwise selecting it on a capable
    /// display would silently alter a picture that needed no alteration.
    #[test]
    fn bt2390_is_identity_when_the_display_covers_the_content() {
        for relative in [0.1, 1.0, 3.9] {
            assert_eq!(bt2390(relative, 4.0, 4.0), relative);
            assert_eq!(bt2390(relative, 8.0, 4.0), relative);
        }
    }

    /// The shader replaces `rgbToUcs(target,target,target).x` with a single
    /// `pq_from_relative(target)`, which is only valid because each ICtCp LMS row sums to
    /// exactly 4096. Worth pinning as arithmetic rather than trusting the comment: if a
    /// coefficient is ever mistyped this is what catches it.
    #[test]
    fn the_ictcp_lms_rows_sum_to_one_which_is_why_the_shader_shortcut_holds() {
        assert_eq!(1688.0 + 2146.0 + 262.0, 4096.0);
        assert_eq!(683.0 + 2951.0 + 462.0, 4096.0);
        assert_eq!(99.0 + 309.0 + 3688.0, 4096.0);

        for target in [1.0_f32, 2.5, 4.0, 10.0] {
            let full = rgb_to_ictcp([target, target, target])[0];
            let shortcut = pq_from_relative(target);
            assert!(
                (full - shortcut).abs() < 1.0e-6,
                "at {target}: full ICtCp gave {full}, the shortcut {shortcut}"
            );
        }
    }

    /// THE REASON TO PREFER GT7. Every per-channel curve desaturates a saturated highlight,
    /// because the brightest channel compresses hardest and the others catch up. GT7's
    /// chroma-preserving half exists to stop that, so a saturated colour should keep more
    /// of its chroma through GT7 than through the same curve applied per channel.
    #[test]
    fn gt7_preserves_chroma_better_than_a_per_channel_curve() {
        let saturated = [6.0_f32, 1.0, 0.4];
        let headroom = 4.0;

        let per_channel = [
            gt7_curve(saturated[0], headroom),
            gt7_curve(saturated[1], headroom),
            gt7_curve(saturated[2], headroom),
        ];
        let volume = gt7(saturated, headroom);

        assert!(
            saturation(volume) > saturation(per_channel),
            "colour-volume {} should hold more chroma than per-channel {}",
            saturation(volume),
            saturation(per_channel)
        );
    }

    fn saturation(rgb: [f32; 3]) -> f32 {
        let max = rgb[0].max(rgb[1]).max(rgb[2]);
        let min = rgb[0].min(rgb[1]).min(rgb[2]);
        if max <= 0.0 {
            0.0
        } else {
            (max - min) / max
        }
    }

    /// It must never exceed the display's peak — that is the contract with the headroom
    /// measurement, and the whole reason for measuring it.
    #[test]
    fn gt7_never_exceeds_the_display_peak() {
        for headroom in [1.0_f32, 1.6, 4.0, 16.0] {
            for luminance in [0.0_f32, 0.5, 1.0, 10.0, 1000.0] {
                let out = gt7([luminance, luminance * 0.5, luminance * 0.1], headroom);
                let ceiling = if headroom <= 1.0 { 1.0 } else { headroom };
                for channel in out {
                    assert!(
                        channel.is_finite(),
                        "headroom {headroom}, input {luminance} produced {channel}"
                    );
                    assert!(
                        channel <= ceiling * 1.001,
                        "headroom {headroom}, input {luminance} produced {channel}, above \
                         the {ceiling} ceiling"
                    );
                }
            }
        }
    }

    /// With no headroom GT7 must still produce a usable SDR image rather than degenerating.
    /// Their `initializeAsSDR` targets 250 cd/m² and normalises back by 1/2.5, which is
    /// what preserves the curve's shape — dropping that would flatten it.
    #[test]
    fn gt7_lands_in_unit_range_on_an_sdr_display() {
        for luminance in [0.0_f32, 0.18, 0.5, 1.0, 4.0, 100.0] {
            let out = gt7([luminance; 3], 1.0);
            for channel in out {
                assert!(
                    (0.0..=1.0 + 1.0e-3).contains(&channel),
                    "{luminance} produced {channel} on an SDR display"
                );
            }
        }
        // And it must actually use the range, not sit crushed near black.
        assert!(gt7([1.0; 3], 1.0)[0] > 0.25);
    }

    /// Monotonic, or brighter scene values render darker.
    #[test]
    fn gt7_is_monotonic() {
        let mut previous = -1.0;
        for step in 0..300 {
            let luminance = step as f32 * 0.05;
            let mapped = gt7([luminance; 3], 4.0)[0];
            assert!(
                mapped >= previous - 1.0e-4,
                "at {luminance} the curve went backwards: {previous} -> {mapped}"
            );
            previous = mapped;
        }
    }

    // ---- The dispatcher ---------------------------------------------------------

    /// [`apply`] must route each variant to the curve that variant NAMES. The `match` is
    /// exhaustive so the compiler guarantees every curve is reachable, but not that the
    /// wiring is right — a copy-paste sending Hable to `reinhard` would still compile, and
    /// would present as "Hable looks wrong" rather than as a dispatch bug.
    #[test]
    fn apply_routes_every_curve_to_its_own_implementation() {
        let (headroom, content_peak) = (4.0, 10.0);
        // Chosen above white so the curves genuinely diverge; at 0.1 several nearly agree.
        let probe = 3.0_f32;
        let expected = [
            (TonemapCurve::Reinhard, reinhard(probe)),
            (
                TonemapCurve::ReinhardHeadroom,
                reinhard_headroom(probe, headroom),
            ),
            (TonemapCurve::HdrKnee, hdr_knee(probe, headroom)),
            (TonemapCurve::HableFilmic, hable(probe)),
            (TonemapCurve::Bt2390, bt2390(probe, headroom, content_peak)),
            (TonemapCurve::Gt7, gt7([probe; 3], headroom)[0]),
        ];
        for (curve, want) in expected {
            let got = apply(curve, [probe; 3], headroom, content_peak)[0];
            assert!(
                (got - want).abs() < 1.0e-6,
                "{curve:?} dispatched to something else: got {got}, want {want}"
            );
        }
    }

    /// Only GT7 mixes channels. Every other curve must treat R, G and B independently, or
    /// the per-channel claim in this module's docs is wrong — and a hue shift would appear
    /// in curves that are supposed to be free of one.
    #[test]
    fn only_gt7_lets_one_channel_affect_another() {
        for curve in TonemapCurve::ALL {
            let alone = apply(curve, [2.0, 0.0, 0.0], 4.0, 10.0)[0];
            let together = apply(curve, [2.0, 1.5, 0.7], 4.0, 10.0)[0];
            let mixes = (alone - together).abs() > 1.0e-6;
            assert_eq!(
                mixes,
                curve == TonemapCurve::Gt7,
                "{curve:?}: red {} on the other channels",
                if mixes { "depends" } else { "does not depend" }
            );
        }
    }

    /// The dispatcher must ignore `content_peak` for every curve that says it does, or
    /// `uses_content_peak` is lying to the overlay and a hidden control would still bite.
    #[test]
    fn content_peak_moves_only_the_curve_that_declares_it() {
        for curve in TonemapCurve::ALL {
            let low = apply(curve, [8.0; 3], 4.0, 4.0);
            let high = apply(curve, [8.0; 3], 4.0, 64.0);
            let responds = (low[0] - high[0]).abs() > 1.0e-6;
            assert_eq!(
                responds,
                curve.uses_content_peak(),
                "{curve:?}: responds={responds}, declares={}",
                curve.uses_content_peak()
            );
        }
    }
}
