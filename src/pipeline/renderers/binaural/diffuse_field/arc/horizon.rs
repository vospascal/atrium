//! The measured head along the horizon: what each ear hears, and how alike
//! the two ears are, from every part of a band of directions around it, so
//! that the three spectra of any arc of the band are a sum.
//!
//! An ambient sound from an arc is many unrelated contributions from the
//! directions within it, each as strong as the next. At the ears that is the
//! diffuse-field computation (`statistics`) restricted to those directions:
//! each ear's power, the mean of |H|², and the two ears' cross-spectrum, the
//! mean of H_left · H_right*, every direction weighted by the solid angle it
//! stands for. Unlike all round, an arc to one side leaves the cross-spectrum
//! complex: its phase is the time by which the near ear hears it first.
//!
//! The band is the three measured rings nearest the horizon, at −10°, 0° and
//! +10°, each for the ±5° of elevation it stands for: 15° above and below.
//! The sounds an arc is for lie along the horizon at a distance: water's
//! edge 7 m or more from ears 1.7 m up lies within 15° below it, a tree
//! line 10 m high 30 m off within 15° above it, and the ground's reflection
//! of either within the band too. One band serves every arc.
//!
//! The band is cut into columns half a degree wide and rows a degree high,
//! each point given its nearest measured direction (as `statistics` gives
//! the sphere's), so a measured direction counts for its Voronoi cell within
//! the band, and an arc for the part of each cell it covers, to half a
//! degree, the edge column by the share of it inside.

use realfft::num_complex::Complex;
use sofar::reader::Sofar;

use super::super::statistics::{nearest_measured, on_long_grid, real_spectrum, ShortGrid};

/// How far above and below the horizon the band reaches, in degrees.
pub(in crate::pipeline::renderers::binaural) const BAND_HALF_HEIGHT_DEGREES: f64 = 15.0;

/// Height of a row of the band, in degrees.
const ROW_DEGREES: f64 = 1.0;

/// Width of a column of the band, in degrees: the finest step an arc's edge
/// is followed in, where only a share of the column lies inside.
const COLUMN_DEGREES: f64 = 0.5;

/// Columns round the horizon.
const COLUMNS: usize = (360.0 / COLUMN_DEGREES) as usize;

/// One measured direction's two ear powers and their cross-spectrum, on the
/// short grid.
struct DirectionSpectra {
    left_power: Vec<f64>,
    right_power: Vec<f64>,
    cross: Vec<Complex<f64>>,
}

/// An arc's three spectra on a long grid, as the mean over its directions.
pub(in crate::pipeline::renderers::binaural) struct ArcSpectra {
    pub(in crate::pipeline::renderers::binaural) left_power: Vec<f64>,
    pub(in crate::pipeline::renderers::binaural) right_power: Vec<f64>,
    /// The mean of H_left · H_right*: its phase is positive where the left
    /// ear hears first.
    pub(in crate::pipeline::renderers::binaural) cross: Vec<Complex<f64>>,
}

pub(in crate::pipeline::renderers::binaural) struct Horizon {
    directions: Vec<DirectionSpectra>,
    /// For each column, counted anticlockwise from straight ahead (to the
    /// left), the measured directions within it and the solid angle each
    /// covers there.
    columns: Vec<Vec<(usize, f64)>>,
    short_size: usize,
}

impl Horizon {
    /// The band of the responses in `sofa` at `sample_rate`. Called when the
    /// HRIRs are loaded, never on the audio thread.
    pub(in crate::pipeline::renderers::binaural) fn measure(
        sofa: &Sofar,
        sample_rate: f32,
    ) -> Self {
        let rows = (2.0 * BAND_HALF_HEIGHT_DEGREES / ROW_DEGREES).round() as usize;
        let row_elevation = |row: usize| {
            (-BAND_HALF_HEIGHT_DEGREES + (row as f64 + 0.5) * ROW_DEGREES).to_radians()
        };
        let points = (0..COLUMNS).flat_map(|column| {
            let azimuth = ((column as f64 + 0.5) * COLUMN_DEGREES).to_radians();
            (0..rows).map(move |row| {
                let elevation = row_elevation(row);
                [
                    elevation.cos() * azimuth.cos(),
                    elevation.cos() * azimuth.sin(),
                    elevation.sin(),
                ]
            })
        });
        let (measured, nearest) = nearest_measured(sofa, points);
        // Each point stands for cos(elevation) · Δazimuth · Δelevation of
        // the sphere.
        let cell = COLUMN_DEGREES.to_radians() * ROW_DEGREES.to_radians();
        let mut columns: Vec<Vec<(usize, f64)>> = vec![Vec::new(); COLUMNS];
        for (point, direction) in nearest.into_iter().enumerate() {
            let (column, row) = (point / rows, point % rows);
            let weight = row_elevation(row).cos() * cell;
            match columns[column]
                .iter_mut()
                .find(|(index, _)| *index == direction)
            {
                Some((_, sum)) => *sum += weight,
                None => columns[column].push((direction, weight)),
            }
        }
        let mut grid = ShortGrid::for_directions(&measured, sofa.filter_len(), sample_rate);
        let bins = grid.bins();
        let directions = measured
            .iter()
            .map(|direction| {
                let mut spectra = DirectionSpectra {
                    left_power: vec![0.0; bins],
                    right_power: vec![0.0; bins],
                    cross: vec![Complex::new(0.0, 0.0); bins],
                };
                grid.add(
                    direction,
                    1.0,
                    &mut spectra.left_power,
                    &mut spectra.right_power,
                    &mut spectra.cross,
                );
                spectra
            })
            .collect();
        Self {
            directions,
            columns,
            short_size: grid.size,
        }
    }

    /// How much of each measured direction an arc `width_degrees` wide
    /// covers, centred `centre_degrees` anticlockwise from straight ahead,
    /// as shares that add up to one.
    fn shares(&self, centre_degrees: f64, width_degrees: f64) -> Vec<f64> {
        let mut shares = vec![0.0; self.directions.len()];
        let start = (centre_degrees - width_degrees / 2.0).rem_euclid(360.0);
        let end = start + width_degrees;
        let first = (start / COLUMN_DEGREES).floor() as usize;
        let last = (end / COLUMN_DEGREES).ceil() as usize;
        for unwrapped in first..last {
            let column_start = unwrapped as f64 * COLUMN_DEGREES;
            let inside = ((column_start + COLUMN_DEGREES).min(end) - column_start.max(start))
                / COLUMN_DEGREES;
            if inside <= 0.0 {
                continue;
            }
            for &(direction, weight) in &self.columns[unwrapped % COLUMNS] {
                shares[direction] += inside * weight;
            }
        }
        let total: f64 = shares.iter().sum();
        for share in &mut shares {
            *share /= total;
        }
        shares
    }

    /// The three spectra of the arc `width_degrees` wide centred
    /// `centre_degrees` anticlockwise from straight ahead, on a grid of
    /// `long_size` bins.
    pub(in crate::pipeline::renderers::binaural) fn arc(
        &self,
        centre_degrees: f64,
        width_degrees: f64,
        long_size: usize,
    ) -> ArcSpectra {
        let bins = self.short_size / 2 + 1;
        let mut left_power = vec![0.0; bins];
        let mut right_power = vec![0.0; bins];
        let mut cross = vec![Complex::new(0.0, 0.0); bins];
        for (direction, share) in self
            .directions
            .iter()
            .zip(self.shares(centre_degrees, width_degrees))
        {
            if share == 0.0 {
                continue;
            }
            for bin in 0..bins {
                left_power[bin] += share * direction.left_power[bin];
                right_power[bin] += share * direction.right_power[bin];
                cross[bin] += share * direction.cross[bin];
            }
        }
        let real = |spectrum: &[f64]| -> Vec<f64> {
            on_long_grid(&real_spectrum(spectrum), self.short_size, long_size)
                .iter()
                .map(|bin| bin.re)
                .collect()
        };
        ArcSpectra {
            left_power: real(&left_power),
            right_power: real(&right_power),
            cross: on_long_grid(&cross, self.short_size, long_size),
        }
    }
}
