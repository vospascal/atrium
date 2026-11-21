use super::super::layout::Speaker;
use super::super::test_support::{
    arrival_power, default_distance, equidistant_quad, omni, room_layout, room_listener,
    total_power, vbap_5_1_layout, world_direction,
};
use super::*;
use crate::panner::DistanceModelType;
use std::f32::consts::PI;

#[test]
fn vbap_source_ahead_activates_front_speakers() {
    let layout = vbap_5_1_layout();
    let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), PI / 2.0); // facing +Y
    let dist = default_distance();

    // Source directly ahead of listener
    let gains = layout.compute_gains_vbap(
        &listener,
        &omni(Vec3::new(3.0, 5.0, 0.0), Vec3::new(0.0, 1.0, 0.0)),
        &dist,
    );

    // Front speakers (L=0, R=1, C=2) should have nonzero gains
    let front_sum = gains.gains[0] + gains.gains[1] + gains.gains[2];
    let rear_sum = gains.gains[4] + gains.gains[5];
    assert!(
        front_sum > rear_sum,
        "front={front_sum} should dominate rear={rear_sum}"
    );
}

#[test]
fn vbap_constant_power() {
    let layout = SpeakerLayout::stereo(Vec3::new(1.0, 3.0, 0.0), Vec3::new(5.0, 3.0, 0.0));
    let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), 0.0);

    // Fixed distance (2m), vary angle — power should stay roughly constant
    let r = 2.0;
    let dist = DistanceParams {
        ref_distance: 1.0,
        max_distance: 20.0,
        rolloff: 1.0,
        model: DistanceModelType::Inverse,
    };

    for angle_deg in (0..360).step_by(30) {
        let angle = (angle_deg as f32).to_radians();
        let source_pos = Vec3::new(
            listener.position.x + r * angle.cos(),
            listener.position.y + r * angle.sin(),
            0.0,
        );
        let gains = layout.compute_gains_vbap(
            &listener,
            &omni(source_pos, Vec3::new(1.0, 0.0, 0.0)),
            &dist,
        );

        let power: f32 = gains.gains[..2].iter().map(|g| g * g).sum();
        // Power = (distance_atten * directivity)²
        let expected_attenuation = distance_gain_at_model(
            listener.position,
            source_pos,
            dist.ref_distance,
            dist.max_distance,
            dist.rolloff,
            dist.model,
        );
        let expected_power = expected_attenuation * expected_attenuation;
        assert!(
            (power - expected_power).abs() < 0.05,
            "angle={}° power={} expected={}",
            angle_deg,
            power,
            expected_power
        );
    }
}

/// A source is as loud behind the listener as ahead of it. On speakers the
/// listener's own ears do the directional hearing; the renderer must not
/// add a second, virtual one. Regression: a "hearing cone" cut everything
/// more than 45° off the listener's nose to 0.3 (−10.5 dB).
#[test]
fn vbap_source_behind_is_as_loud_as_ahead() {
    let layout = vbap_5_1_layout();
    let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), PI / 2.0); // facing +Y
    let dist = default_distance();

    let gains_ahead = layout.compute_gains_vbap(
        &listener,
        &omni(Vec3::new(3.0, 4.0, 0.0), Vec3::new(0.0, 1.0, 0.0)),
        &dist,
    );
    let gains_behind = layout.compute_gains_vbap(
        &listener,
        &omni(Vec3::new(3.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0)),
        &dist,
    );

    // Compared where it is heard: speakers sit at different distances from
    // this listener, so equal loudness means equal power on arrival.
    let power_ahead = arrival_power(&layout, &listener, &gains_ahead);
    let power_behind = arrival_power(&layout, &listener, &gains_behind);
    assert!(
        (power_ahead / power_behind - 1.0).abs() < 0.01,
        "same distance ahead and behind must arrive equally loud: ahead={power_ahead} behind={power_behind}"
    );
}

#[test]
fn vbap_lfe_channel_stays_zero() {
    let layout = vbap_5_1_layout();
    let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), 0.0);
    let dist = default_distance();

    let gains = layout.compute_gains_vbap(
        &listener,
        &omni(Vec3::new(4.0, 3.0, 0.0), Vec3::new(1.0, 0.0, 0.0)),
        &dist,
    );

    assert_eq!(gains.gains[3], 0.0, "LFE should be zero from VBAP");
}

#[test]
fn vbap_source_left_has_more_fl_than_fr() {
    let layout = room_layout();
    let listener = room_listener();
    let dist = default_distance();

    // Source to listener's LEFT (forward-left): (1, 3, 0)
    let gains = layout.compute_gains_vbap(
        &listener,
        &omni(Vec3::new(1.0, 3.0, 0.0), Vec3::new(0.0, 1.0, 0.0)),
        &dist,
    );

    assert!(
        gains.gains[0] > gains.gains[1],
        "source LEFT → FL (ch0) should be louder than FR (ch1): ch0={}, ch1={}",
        gains.gains[0],
        gains.gains[1]
    );
}

#[test]
fn vbap_source_right_has_more_fr_than_fl() {
    let layout = room_layout();
    let listener = room_listener();
    let dist = default_distance();

    // Source to listener's RIGHT (forward-right): (5, 3, 0)
    let gains = layout.compute_gains_vbap(
        &listener,
        &omni(Vec3::new(5.0, 3.0, 0.0), Vec3::new(0.0, 1.0, 0.0)),
        &dist,
    );

    assert!(
        gains.gains[1] > gains.gains[0],
        "source RIGHT → FR (ch1) should be louder than FL (ch0): ch0={}, ch1={}",
        gains.gains[0],
        gains.gains[1]
    );
}

#[test]
fn vbap_source_rear_left_has_more_rl_than_rr() {
    let layout = room_layout();
    let listener = room_listener();
    let dist = default_distance();

    // Source rear-left: behind and to the left → (1, 1, 0)
    let gains = layout.compute_gains_vbap(
        &listener,
        &omni(Vec3::new(1.0, 1.0, 0.0), Vec3::new(0.0, 1.0, 0.0)),
        &dist,
    );

    assert!(
        gains.gains[4] > gains.gains[5],
        "source REAR-LEFT → RL (ch4) should be louder than RR (ch5): ch4={}, ch5={}",
        gains.gains[4],
        gains.gains[5]
    );
}

#[test]
fn vbap_source_rear_right_has_more_rr_than_rl() {
    let layout = room_layout();
    let listener = room_listener();
    let dist = default_distance();

    // Source rear-right: behind and to the right → (5, 1, 0)
    let gains = layout.compute_gains_vbap(
        &listener,
        &omni(Vec3::new(5.0, 1.0, 0.0), Vec3::new(0.0, 1.0, 0.0)),
        &dist,
    );

    assert!(
        gains.gains[5] > gains.gains[4],
        "source REAR-RIGHT → RR (ch5) should be louder than RL (ch4): ch4={}, ch5={}",
        gains.gains[4],
        gains.gains[5]
    );
}

// -- Gotcha: VBAP magnetization effect (Gandemer 2018) --
// Sources "snap" toward speaker positions during movement. Mitigation:
// denser speaker arrays or gain interpolation at <= 10ms intervals.
// This test verifies that gains change smoothly (monotonically) when
// panning between two speakers — no sudden jumps or plateaus.

#[test]
fn vbap_gains_no_large_discontinuities() {
    // Verify VBAP gains don't have sudden jumps when sweeping between speakers.
    // Small bumps are expected (magnetization effect — Gandemer 2018: sources
    // "snap" toward speaker positions), but large discontinuities (> 0.15)
    // would indicate a broken speaker-pair selection.
    let layout = SpeakerLayout::new(
        &[
            Speaker {
                position: Vec3::new(-1.0, 1.0, 0.0),
                channel: 0,
            },
            Speaker {
                position: Vec3::new(1.0, 1.0, 0.0),
                channel: 1,
            },
        ],
        None,
        2,
    );
    let listener = Listener::new(Vec3::new(0.0, 0.0, 0.0), 0.0);

    let steps = 90;
    let mut prev_gains = [0.0_f32; 2];
    let mut max_jump = 0.0_f32;

    for step in 0..=steps {
        let angle =
            std::f32::consts::FRAC_PI_2 - (step as f32 / steps as f32) * std::f32::consts::PI;
        let direction = Vec3::new(angle.sin(), angle.cos(), 0.0);
        let gains = layout.compute_vbap_panning(&listener, direction);

        if step > 0 {
            for ch in 0..2 {
                let jump = (gains.gains[ch] - prev_gains[ch]).abs();
                max_jump = max_jump.max(jump);
                assert!(
                    jump < 0.15,
                    "step {step} ch{ch}: gain discontinuity {jump:.4} \
                     ({:.4} -> {:.4}) — possible broken pair selection",
                    prev_gains[ch],
                    gains.gains[ch]
                );
            }
        }

        prev_gains = [gains.gains[0], gains.gains[1]];
    }

    // Energy should be roughly preserved across the sweep.
    // At endpoints: one speaker at ~1.0. At midpoint: both at ~0.707.
    let midpoint_dir = Vec3::new(0.0, 1.0, 0.0);
    let mid_gains = layout.compute_vbap_panning(&listener, midpoint_dir);
    let energy: f32 = mid_gains.gains[..2].iter().map(|g| g * g).sum();
    assert!(
        (energy - 1.0).abs() < 0.1,
        "midpoint energy {energy:.4} should be ~1.0 (energy preservation)"
    );
}

// ── Hand-derived VBAP / layout values ─────────────────────────────

#[test]
fn vbap_pair_gains_match_pulkki_closed_form() {
    // For a 90° pair at ±45°, solving g = L⁻¹·s gives
    //   g_left = sin(θ + 45°), g_right = cos(θ + 45°),
    // already unit power. θ = 15° → FL = sin 60° = 0.866025, FR = 0.5.
    let layout = equidistant_quad();
    let listener = Listener::new(Vec3::ZERO, 0.0);
    let gains =
        layout.compute_vbap_panning(&listener, world_direction(&listener, 15f32.to_radians()));
    assert!(
        (gains.gains[0] - 0.866_025).abs() < 1e-5,
        "FL={}",
        gains.gains[0]
    );
    assert!((gains.gains[1] - 0.5).abs() < 1e-5, "FR={}", gains.gains[1]);
    assert_eq!(gains.gains[4], 0.0);
    assert_eq!(gains.gains[5], 0.0);

    // Sweep the front pair against the closed form.
    for degrees in -45..=45 {
        let theta = (degrees as f32).to_radians();
        let gains = layout.compute_vbap_panning(&listener, world_direction(&listener, theta));
        let quarter = std::f32::consts::FRAC_PI_4;
        assert!((gains.gains[0] - (theta + quarter).sin()).abs() < 1e-4);
        assert!((gains.gains[1] - (theta + quarter).cos()).abs() < 1e-4);
    }

    // Directly behind: midway between RL and RR → 1/√2 each.
    let behind = layout.compute_vbap_panning(&listener, Vec3::new(-1.0, 0.0, 0.0));
    assert!((behind.gains[4] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
    assert!((behind.gains[5] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
}

#[test]
fn vbap_constant_arrival_power_and_continuity_over_full_sweep_in_room() {
    // Room layout (non-equidistant: C 2.0 m, others √13 = 3.606 m). Each
    // gain is scaled by its speaker's distance over the farthest (√13), so
    // the power arriving at the listener is Σ(g/d)² = 1/13 at every
    // azimuth, and adjacent 0.5° steps (including pair boundaries at the
    // speaker azimuths 0°, ±56.31°, ±123.69°) change no gain by > 0.03.
    let layout = room_layout();
    let listener = room_listener();
    let mut previous: Option<ChannelGains> = None;
    for half_degree in 0..=720 {
        let azimuth = (half_degree as f32 * 0.5 - 180.0).to_radians();
        let gains = layout.compute_vbap_panning(&listener, world_direction(&listener, azimuth));
        let power = arrival_power(&layout, &listener, &gains);
        assert!(
            (power - 1.0 / 13.0).abs() < 1e-5,
            "azimuth {azimuth}: arrival power {power}"
        );
        assert_eq!(gains.gains[3], 0.0, "LFE must never receive VBAP gain");
        if let Some(previous_gains) = previous {
            for channel in 0..6 {
                let step = (gains.gains[channel] - previous_gains.gains[channel]).abs();
                assert!(
                    step < 0.03,
                    "azimuth {azimuth}, channel {channel}: step {step}"
                );
            }
        }
        previous = Some(gains);
    }
}

#[test]
fn vbap_source_on_each_speaker_drives_only_that_speaker() {
    // Direction exactly toward a speaker: g = L⁻¹·s = (1, 0), then scaled by
    // that speaker's distance over the farthest (√13 m): the centre at 2 m
    // plays at 2/√13 = 0.554700, the other four at unity.
    let layout = room_layout();
    let listener = room_listener();
    let farthest = 13.0_f32.sqrt();
    for speaker in layout.speakers() {
        let gains = layout.compute_vbap_panning(&listener, speaker.position - listener.position);
        let own_gain = (speaker.position - listener.position).length() / farthest;
        for channel in 0..6 {
            let expected = if channel == speaker.channel {
                own_gain
            } else {
                0.0
            };
            assert!(
                (gains.gains[channel] - expected).abs() < 1e-4,
                "toward ch{}: ch{channel} = {}",
                speaker.channel,
                gains.gains[channel]
            );
        }
    }
}

#[test]
fn vbap_is_left_right_mirror_symmetric_in_room() {
    // Room layout is symmetric about the listener's forward axis (+Y, x = 3):
    // local azimuth +θ on (FL, RL) must equal −θ on (FR, RR); C equal.
    let layout = room_layout();
    let listener = room_listener();
    for degrees in [5.0_f32, 30.0, 56.0, 90.0, 140.0, 175.0] {
        let theta = degrees.to_radians();
        let left = layout.compute_vbap_panning(&listener, world_direction(&listener, theta));
        let right = layout.compute_vbap_panning(&listener, world_direction(&listener, -theta));
        for (channel, mirror) in [(0, 1), (2, 2), (4, 5)] {
            assert!(
                (left.gains[channel] - right.gains[mirror]).abs() < 1e-4,
                "{degrees}°: ch{channel}={} vs ch{mirror}={}",
                left.gains[channel],
                right.gains[mirror]
            );
        }
    }
}

#[test]
fn vbap_distance_compensation_equalises_arrival_within_pair() {
    // Room listener: C at local 0° (2 m), FL at atan2(3, 2) = 56.31° (√13 m).
    // On the bisector (28.155°) raw VBAP gains are equal (1/√2 each), and
    // each is scaled by its distance over the farthest (√13 m):
    //   g_C = (1/√2)·2/√13 = 0.392232, g_FL = 1/√2 = 0.707107,
    // so the arrivals g/d are equal.
    let layout = room_layout();
    let listener = room_listener();
    let bisector = (3.0_f32.atan2(2.0)) * 0.5;
    let gains = layout.compute_vbap_panning(&listener, world_direction(&listener, bisector));
    assert!(
        (gains.gains[2] - 0.392_232).abs() < 1e-4,
        "C={}",
        gains.gains[2]
    );
    assert!(
        (gains.gains[0] - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-4,
        "FL={}",
        gains.gains[0]
    );
    let arrival_center = gains.gains[2] / 2.0;
    let arrival_front_left = gains.gains[0] / 13.0_f32.sqrt();
    assert!((arrival_center - arrival_front_left).abs() < 1e-4);
}

/// A source at a fixed distance must arrive equally loud from every
/// direction. In the default 5.1 room (listener at centre, C at 2 m, the
/// other four at √13 m) renormalising each pair to Σg² = 1 used to cancel
/// the compensation between pairs: arrival power was 1/4 straight ahead
/// (C only) but 1/13 at 90° left (FL + RL), 10·log10(13/4) = 5.1 dB apart.
#[test]
fn vbap_arrival_level_is_independent_of_azimuth_in_room() {
    let layout = room_layout();
    let listener = room_listener();
    let ahead = layout.compute_vbap_panning(&listener, world_direction(&listener, 0.0));
    let left = layout.compute_vbap_panning(
        &listener,
        world_direction(&listener, std::f32::consts::FRAC_PI_2),
    );
    let level_difference_db = 10.0
        * (arrival_power(&layout, &listener, &ahead) / arrival_power(&layout, &listener, &left))
            .log10();
    assert!(
        level_difference_db.abs() < 0.01,
        "ahead vs left arrival level differs by {level_difference_db:.2} dB"
    );
}

#[test]
fn vbap_full_gains_apply_inverse_distance() {
    // compute_gains_vbap = unit-power panning × distance gain × directivity.
    // Omni source 4 m ahead, inverse model ref 1 rolloff 1 → amplitude 0.25,
    // split between FL/FR at 1/√2 each: 0.176777, Σg² = 0.0625.
    let layout = equidistant_quad();
    let listener = Listener::new(Vec3::ZERO, 0.0);
    let source = omni(Vec3::new(4.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0));
    let gains = layout.compute_gains_vbap(&listener, &source, &default_distance());
    assert!((gains.gains[0] - 0.176_777).abs() < 1e-5);
    assert!((gains.gains[1] - 0.176_777).abs() < 1e-5);
    assert!((total_power(&gains) - 0.0625).abs() < 1e-6);
    // Zero spread is exactly plain VBAP.
    let spread_zero =
        layout.compute_gains_with_spread(&listener, &source, &default_distance(), 0.0);
    assert_eq!(spread_zero.gains, gains.gains);
}

#[test]
fn angle_diff_wraps_to_shortest_signed_arc() {
    // 170° − (−170°) = 340° ≡ −20°; −170° − 170° ≡ +20°; 30° − 10° = 20°.
    let radians = |degrees: f32| degrees.to_radians();
    assert!((angle_diff(radians(170.0), radians(-170.0)) - radians(-20.0)).abs() < 1e-5);
    assert!((angle_diff(radians(-170.0), radians(170.0)) - radians(20.0)).abs() < 1e-5);
    assert!((angle_diff(radians(30.0), radians(10.0)) - radians(20.0)).abs() < 1e-6);
    assert!(angle_diff(PI, -PI).abs() < 1e-5);
}
