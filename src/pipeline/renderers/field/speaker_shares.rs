//! How an arc's power is shared out among loudspeakers.
//!
//! An ambient sound from an arc is unrelated sound from every direction in
//! it. Panned direction by direction, each direction's power would go to the
//! speakers its panning gains say, and, the directions being unrelated,
//! their powers add: each speaker's share of the arc is the integral over
//! the arc of its squared panning gain. The panning is the layout's own
//! two-dimensional VBAP (Pulkki, "Virtual sound source positioning using
//! vector base amplitude panning", JAES 45, 1997), seen from the receiver,
//! between the two speakers either side of a direction, with its gains
//! normalised to unit power. Each speaker then plays its own decorrelated
//! copy at its share, so the shares add up to the source's power.
//!
//! Its energy vector (Gerzon, "General metatheory of auditory localisation",
//! AES 92nd Convention, 1992), the power-weighted mean of the speakers'
//! directions, is the mean over the arc of each direction's: it points at
//! the arc's middle and grows shorter as the arc widens.

use atrium_core::speaker::MAX_CHANNELS;

use super::ambient_arc::AmbientArc;

/// The arc is followed in steps no wider than this, in degrees.
const STEP_DEGREES: f32 = 0.25;

/// Each speaker's share of the power of `arc`, for speakers at `headings`
/// (radians, as the listener's heading is given) seen from the receiver; the
/// shares add up to one. A direction no pair of speakers less than half a
/// turn apart encloses goes to the nearest speaker, as the layout's VBAP
/// gives it.
pub(super) fn speaker_shares(arc: AmbientArc, headings: &[f32]) -> [f32; MAX_CHANNELS] {
    let mut shares = [0.0f32; MAX_CHANNELS];
    let count = headings.len().min(MAX_CHANNELS);
    if count == 0 {
        return shares;
    }
    let mut order: [usize; MAX_CHANNELS] = std::array::from_fn(|index| index);
    let order = &mut order[..count];
    let tau = std::f32::consts::TAU;
    order.sort_unstable_by(|&a, &b| {
        headings[a]
            .rem_euclid(tau)
            .total_cmp(&headings[b].rem_euclid(tau))
    });
    let steps = ((arc.width.to_degrees() / STEP_DEGREES).ceil() as usize).max(1);
    let step = arc.width / steps as f32;
    for index in 0..steps {
        let direction = arc.centre - arc.width / 2.0 + (index as f32 + 0.5) * step;
        let (gains, speakers) = panning(direction, headings, order);
        for (gain, speaker) in gains.iter().zip(speakers) {
            shares[speaker] += gain * gain;
        }
    }
    let total: f32 = shares.iter().sum();
    for share in &mut shares {
        *share /= total;
    }
    shares
}

/// VBAP between the two speakers either side of `direction`, gains of unit
/// power, or the nearest speaker alone where no pair encloses it.
fn panning(direction: f32, headings: &[f32], order: &[usize]) -> ([f32; 2], [usize; 2]) {
    let tau = std::f32::consts::TAU;
    let anticlockwise = |from: f32, to: f32| (to - from).rem_euclid(tau);
    if order.len() > 1 {
        for pair in 0..order.len() {
            let (first, second) = (order[pair], order[(pair + 1) % order.len()]);
            let aperture = anticlockwise(headings[first], headings[second]);
            let into = anticlockwise(headings[first], direction);
            if aperture > 0.0 && aperture < std::f32::consts::PI && into <= aperture {
                // g = [l_first l_second]⁻¹ · p, Pulkki's eq. 5 in two dimensions.
                let first_gain = (aperture - into).sin();
                let second_gain = into.sin();
                let norm = (first_gain * first_gain + second_gain * second_gain).sqrt();
                return ([first_gain / norm, second_gain / norm], [first, second]);
            }
        }
    }
    let nearest = order
        .iter()
        .copied()
        .min_by(|&a, &b| {
            let distance = |speaker: usize| {
                let turn = anticlockwise(headings[speaker], direction);
                turn.min(tau - turn)
            };
            distance(a).total_cmp(&distance(b))
        })
        .unwrap_or(0);
    ([1.0, 0.0], [nearest, nearest])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arc(width_degrees: f32, centre_degrees: f32) -> AmbientArc {
        AmbientArc {
            width: width_degrees.to_radians(),
            centre: centre_degrees.to_radians(),
        }
    }

    fn energy_vector(shares: &[f32], headings: &[f32]) -> (f32, f32) {
        let (x, y) = headings
            .iter()
            .zip(shares)
            .fold((0.0, 0.0), |(x, y), (heading, share)| {
                (x + share * heading.cos(), y + share * heading.sin())
            });
        (y.atan2(x).to_degrees(), (x * x + y * y).sqrt())
    }

    /// A square of speakers: an arc between two of them goes to those two
    /// alone, in shares that add up to one and mirror about its middle.
    #[test]
    fn an_arc_between_two_speakers_is_shared_by_them() {
        let headings: Vec<f32> = [45.0f32, 135.0, 225.0, 315.0]
            .iter()
            .map(|degrees| degrees.to_radians())
            .collect();
        let shares = speaker_shares(arc(60.0, 90.0), &headings);
        assert!((shares.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert!((shares[0] - 0.5).abs() < 1e-4 && (shares[1] - 0.5).abs() < 1e-4);
        assert!(shares[2] == 0.0 && shares[3] == 0.0);
    }

    /// The energy vector points at the arc's middle and shortens as it
    /// widens: on a ring of eight it is the mean of VBAP's own over the arc,
    /// within 5° of the middle (VBAP's energy vector itself leans towards
    /// the nearer speaker of a pair, so an arc not centred on a pair leans
    /// a little too).
    #[test]
    fn the_energy_vector_points_at_the_middle_and_shortens_as_the_arc_widens() {
        let headings: Vec<f32> = (0..8)
            .map(|k| (k as f32 * 45.0 + 10.0).to_radians())
            .collect();
        for centre in [0.0f32, 70.0, 200.0] {
            let mut previous_length = f32::MAX;
            for width in [30.0f32, 90.0, 180.0, 300.0] {
                let shares = speaker_shares(arc(width, centre), &headings);
                let (direction, length) = energy_vector(&shares, &headings);
                let off = (direction - centre + 540.0).rem_euclid(360.0) - 180.0;
                assert!(
                    off.abs() < 5.0,
                    "{width}° at {centre}°: points {off:+.1}° off"
                );
                assert!(
                    length < previous_length,
                    "{width}° at {centre}°: length {length}"
                );
                previous_length = length;
            }
        }
    }

    /// A stereo pair 60° apart: a direction behind is in no pair, so it goes
    /// to the nearer speaker.
    #[test]
    fn a_direction_no_pair_encloses_goes_to_the_nearest_speaker() {
        let headings = [60f32.to_radians(), 120f32.to_radians()];
        let shares = speaker_shares(arc(20.0, 200.0), &headings);
        assert!((shares[1] - 1.0).abs() < 1e-6, "{shares:?}");
    }
}
