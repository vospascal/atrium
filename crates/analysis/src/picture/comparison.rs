//! The comparison's picture: 1600 pixels wide, every file's curve on one
//! set of axes, the reference black and thick and each other file in its
//! own colour, named in the legend at the top.
//!
//! 1. third-octave colour relative to Leq, and the difference from the
//!    reference under it;
//! 2. the modulation spectrum, the LAF distribution and the texture SNR per
//!    class;
//! 3. M2 and M4 per subband, and C between neighbouring subbands;
//! 4. for two-channel files: IACC and envelope coherence per octave;
//! 5. each file's spectrogram, on one frequency axis, time scale and range.

use super::axes::{linear_ticks, logarithmic_ticks, rounded_range, thick_line, Axes, Pen, Scale};
use super::canvas::{Canvas, BLACK, CHARACTER_WIDTH, LINE_HEIGHT, MUTED};
use super::spectrogram::{colour, grid, RANGE_DECIBELS};
use super::{time_step, wrap, WIDTH};
use crate::compare::{
    compare, correlation_at_offset, file_colour, laf_exceeded, modulation_spectrum,
    relative_band_levels, Compared, Comparison, Differences, TextureClass,
};
use crate::frequency_rows::frequency_fraction;
use crate::signal::Combination;

/// Panels across the whole width, leaving room for the value labels on the
/// left and the spectrogram's colour scale on the right (px).
const FULL_LEFT: usize = 110;
const FULL_WIDTH: usize = 1380;
/// Two panels side by side, and three.
const HALVES: [usize; 2] = [110, 880];
const HALF_WIDTH: usize = 640;
const THIRDS: [usize; 3] = [100, 630, 1160];
const THIRD_WIDTH: usize = 400;
/// Between one panel's axis name and the next panel's title (px).
const GAP: usize = 12;
const COLOUR_HEIGHT: usize = 300;
const DIFFERENCE_HEIGHT: usize = 170;
const ROW_HEIGHT: usize = 220;
const STRIP_HEIGHT: usize = 120;
/// The reference's curve is thicker than the others', so it stands out
/// where they overlap.
const REFERENCE_THICKNESS: usize = 3;
const OTHER_THICKNESS: usize = 2;
/// Characters of the key differences on one line before it wraps.
const KEY_FITTING: usize = (WIDTH - 20) / CHARACTER_WIDTH;
/// The texture SNR's scale tops out here; an identical class is drawn to it.
const SNR_TOP: f64 = 60.0;

/// The comparison of `others` against `reference`, as a PNG file.
pub fn comparison_picture(reference: &Compared, others: &[Compared]) -> Vec<u8> {
    let comparison = compare(reference, others);
    let files: Vec<&Compared> = std::iter::once(reference).chain(others).collect();
    draw(&comparison, &files).png()
}

/// One file's points: a position on the x axis and the value there, if any.
type Curve = Vec<(f64, Option<f64>)>;

fn pen(index: usize) -> Pen {
    Pen {
        colour: file_colour(index).1,
        thickness: if index == 0 {
            REFERENCE_THICKNESS
        } else {
            OTHER_THICKNESS
        },
    }
}

fn draw(comparison: &Comparison, files: &[&Compared]) -> Canvas {
    let pairs = files
        .iter()
        .all(|file| file.report.signal.channel_names.len() == 2);
    let key_lines: Vec<String> = comparison
        .others
        .iter()
        .enumerate()
        .flat_map(|(index, other)| wrap(&key_differences(index + 1, other), KEY_FITTING))
        .collect();
    let heading_lines = 2 + files.len() + key_lines.len();
    let mut top = 8 + heading_lines * LINE_HEIGHT + GAP + Axes::ABOVE;
    let mut next = |height: usize| {
        let placed = top;
        top += height + Axes::BELOW + GAP + Axes::ABOVE;
        placed
    };
    let colour_top = next(COLOUR_HEIGHT);
    let difference_top = next(DIFFERENCE_HEIGHT);
    let modulation_top = next(ROW_HEIGHT);
    let subband_top = next(ROW_HEIGHT);
    let space_top = pairs.then(|| next(ROW_HEIGHT));
    let strips_top = top;
    let strip_step = LINE_HEIGHT + STRIP_HEIGHT + 6;
    let height = strips_top + files.len() * strip_step + Axes::BELOW + GAP;

    let mut canvas = Canvas::new(WIDTH, height);
    heading(&mut canvas, comparison, files, &key_lines);
    colour_panels(&mut canvas, comparison, files, colour_top, difference_top);
    modulation_panel(&mut canvas, files, modulation_top);
    laf_panel(&mut canvas, comparison, files, modulation_top);
    snr_panel(&mut canvas, comparison, modulation_top);
    subband_panels(&mut canvas, files, subband_top);
    if let Some(space_top) = space_top {
        space_panels(&mut canvas, files, space_top);
    }
    spectrograms(&mut canvas, files, strips_top, strip_step);
    canvas
}

/// The title, the legend (a sample of each file's line, its name and how
/// it was made) and the differences that matter most, a line per file.
fn heading(
    canvas: &mut Canvas,
    comparison: &Comparison,
    files: &[&Compared],
    key_lines: &[String],
) {
    let fitting = (WIDTH - 20) / CHARACTER_WIDTH;
    let mut y = 8;
    canvas.text(
        10,
        y,
        &format!(
            "Comparison against the reference {}: differences are the file's value minus the reference's",
            comparison.reference.name
        ),
        BLACK,
    );
    y += LINE_HEIGHT;
    canvas.text(
        10,
        y,
        "Legend: the reference is black and thick; each other file has its colour",
        MUTED,
    );
    y += LINE_HEIGHT;
    for (index, file) in files.iter().enumerate() {
        let (name, _) = file_colour(index);
        let sample_y = (y + LINE_HEIGHT / 2 - 3) as isize;
        thick_line(canvas, (10, sample_y), (50, sample_y), pen(index));
        let label = if index == 0 {
            format!("ref ({name}): {} | {}", file.name, file.heading)
        } else {
            format!("{index} ({name}): {} | {}", file.name, file.heading)
        };
        let shown = wrap(&label, fitting - 6).remove(0);
        canvas.text(60, y, &shown, pen(index).colour);
        y += LINE_HEIGHT;
    }
    for line in key_lines {
        canvas.text(10, y, line, BLACK);
        y += LINE_HEIGHT;
    }
}

/// One line of the differences a reader looks for first.
fn key_differences(index: usize, other: &Differences) -> String {
    let snr = |class: &crate::compare::ClassDifference| {
        format!(
            "{} {:.0}",
            class.class.symbol(),
            class.snr_decibels.unwrap_or(f64::INFINITY)
        )
    };
    let classes = &other.texture.classes;
    let differs: Vec<String> = classes
        .iter()
        .filter(|class| !class.converged && !class.within_scatter)
        .map(snr)
        .collect();
    let within: Vec<String> = classes
        .iter()
        .filter(|class| !class.converged && class.within_scatter)
        .map(snr)
        .collect();
    let mut texture = if differs.is_empty() {
        "no texture class differs beyond chance".to_owned()
    } else {
        format!(
            "texture differs (SNR < 20 dB and below the scatter floor): {}",
            differs.join(", ")
        )
    };
    if !within.is_empty() {
        texture += &format!("; < 20 dB but within the floor: {}", within.join(", "));
    }
    let width = other
        .space
        .as_ref()
        .and_then(|space| space.width)
        .map_or(String::new(), |width| {
            format!(" | width {:+.2}", width.difference)
        });
    format!(
        "{index} vs ref: colour RMS {:.1} dB, tilt {:+.2} dB/oct | L10-L90 {:+.1} dB{width} | {texture}",
        other.colour.rms_difference_decibels,
        other.colour.tilt_decibels_per_octave.difference,
        other.level.l10_minus_l90.difference,
    )
}

/// The third-octave colour of every file, and under it each file's
/// difference from the reference.
fn colour_panels(
    canvas: &mut Canvas,
    comparison: &Comparison,
    files: &[&Compared],
    colour_top: usize,
    difference_top: usize,
) {
    let curves: Vec<Curve> = files
        .iter()
        .map(|file| {
            relative_band_levels(file.report)
                .into_iter()
                .map(|(centre, level)| (centre, Some(level)))
                .collect()
        })
        .collect();
    let values = curves.iter().flatten().filter_map(|(_, value)| *value);
    let highest = values.clone().fold(f64::NEG_INFINITY, f64::max);
    let lowest = values.fold(f64::INFINITY, f64::min).max(highest - 80.0);
    let range = rounded_range(lowest, highest, 10.0);
    let frequency = Scale::Logarithmic(20.0, 20_000.0);
    let axes = Axes {
        left: FULL_LEFT,
        top: colour_top,
        width: FULL_WIDTH,
        height: COLOUR_HEIGHT,
        x: frequency,
        y: Scale::Linear(range.0, range.1),
    };
    axes.frame(
        canvas,
        "Colour: third-octave band level relative to the file's own Leq (dB), 25 Hz to 20 kHz",
    );
    axes.y_axis(canvas, &linear_ticks(range.0, range.1, 10.0, 0));
    axes.x_axis(
        canvas,
        &logarithmic_ticks(20.0, 20_000.0),
        "Frequency (Hz, log scale)",
    );
    // The reference last, so it lies on top where the curves meet.
    for (index, curve) in curves.iter().enumerate().rev() {
        axes.curve(canvas, curve, pen(index), true);
    }

    let differences: Vec<Curve> = comparison
        .others
        .iter()
        .map(|other| {
            other
                .colour
                .bands
                .iter()
                .map(|band| (band.centre_hertz, Some(band.decibels.difference)))
                .collect()
        })
        .collect();
    let largest = differences
        .iter()
        .flatten()
        .filter_map(|(_, value)| value.map(f64::abs))
        .fold(0.0f64, f64::max);
    // Whole 5 dB up to 10 dB, whole 10 dB beyond, so the ticks fall on
    // round numbers.
    let extent = if largest <= 10.0 {
        ((largest / 5.0).ceil() * 5.0).max(5.0)
    } else {
        ((largest / 10.0).ceil() * 10.0).min(60.0)
    };
    let axes = Axes {
        top: difference_top,
        height: DIFFERENCE_HEIGHT,
        y: Scale::Linear(-extent, extent),
        ..axes
    };
    axes.frame(
        canvas,
        "Colour difference: the file's band level minus the reference's, both relative to Leq (dB; + = more in the file)",
    );
    let step = if extent <= 10.0 {
        2.5
    } else {
        nice_step(2.0 * extent)
    };
    axes.y_axis(
        canvas,
        &linear_ticks(-extent, extent, step, if step < 5.0 { 1 } else { 0 }),
    );
    axes.x_axis(
        canvas,
        &logarithmic_ticks(20.0, 20_000.0),
        "Frequency (Hz, log scale)",
    );
    let zero = axes.y_pixel(0.0);
    thick_line(
        canvas,
        (axes.left as isize, zero),
        (axes.right() as isize - 1, zero),
        pen(0),
    );
    canvas.text(axes.left + 6, zero as usize + 4, "0 = the reference", BLACK);
    for (index, curve) in differences.iter().enumerate() {
        axes.curve(canvas, curve, pen(index + 1), true);
    }
}

/// The modulation spectrum, the mean over subbands, of every file.
fn modulation_panel(canvas: &mut Canvas, files: &[&Compared], top: usize) {
    let curves: Vec<Curve> = files
        .iter()
        .map(|file| {
            let texture = &file.report.texture;
            texture
                .modulation_centres_hertz
                .iter()
                .zip(modulation_spectrum(texture))
                .map(|(rate, power)| (*rate, Some(100.0 * power)))
                .collect()
        })
        .collect();
    let highest = curves
        .iter()
        .flatten()
        .filter_map(|(_, value)| *value)
        .fold(0.0f64, f64::max);
    let step = nice_step(highest);
    let range = rounded_range(0.0, highest, step);
    let axes = Axes {
        left: THIRDS[0],
        top,
        width: THIRD_WIDTH,
        height: ROW_HEIGHT,
        x: Scale::Logarithmic(0.5, 200.0),
        y: Scale::Linear(range.0, range.1),
    };
    axes.frame(canvas, "Modulation power, mean over subbands (%)");
    axes.y_axis(canvas, &linear_ticks(range.0, range.1, step, 0));
    axes.x_axis(
        canvas,
        &logarithmic_ticks(0.5, 200.0),
        "Modulation rate (Hz, log)",
    );
    for (index, curve) in curves.iter().enumerate().rev() {
        axes.curve(canvas, curve, pen(index), true);
    }
}

/// The LAF distribution of every file: LN from N = 95 to 5.
fn laf_panel(canvas: &mut Canvas, comparison: &Comparison, files: &[&Compared], top: usize) {
    let relative = comparison
        .others
        .iter()
        .any(|other| other.over_time.laf_relative_to_l50);
    let curves: Vec<Curve> = files
        .iter()
        .map(|file| {
            laf_exceeded(file.report, relative)
                .into_iter()
                .map(|(percent, level)| (percent, Some(level)))
                .collect()
        })
        .collect();
    let values = curves.iter().flatten().filter_map(|(_, value)| *value);
    let highest = values.clone().fold(f64::NEG_INFINITY, f64::max);
    let lowest = values.fold(f64::INFINITY, f64::min);
    let step = nice_step(highest - lowest);
    let range = rounded_range(lowest, highest, step);
    let axes = Axes {
        left: THIRDS[1],
        top,
        width: THIRD_WIDTH,
        height: ROW_HEIGHT,
        x: Scale::Linear(100.0, 0.0),
        y: Scale::Linear(range.0, range.1),
    };
    axes.frame(
        canvas,
        if relative {
            "LAF distribution, dB re own L50"
        } else {
            "LAF distribution (dB SPL)"
        },
    );
    axes.y_axis(
        canvas,
        &linear_ticks(range.0, range.1, step, usize::from(step < 1.0)),
    );
    let ticks: Vec<(f64, String)> = [95.0, 75.0, 50.0, 25.0, 5.0]
        .into_iter()
        .map(|percent| (percent, format!("L{percent:.0}")))
        .collect();
    axes.x_axis(canvas, &ticks, "LN: exceeded N % of the time");
    for (index, curve) in curves.iter().enumerate().rev() {
        axes.curve(canvas, curve, pen(index), true);
    }
}

/// The texture SNR per class, a bar per file, with the paper's 20 dB.
fn snr_panel(canvas: &mut Canvas, comparison: &Comparison, top: usize) {
    let classes = TextureClass::ALL.len();
    let lowest = comparison
        .others
        .iter()
        .flat_map(|other| other.texture.classes.iter())
        .filter_map(|class| class.snr_decibels)
        .fold(0.0f64, f64::min);
    let bottom = (lowest / 10.0).floor() * 10.0;
    let axes = Axes {
        left: THIRDS[2],
        top,
        width: THIRD_WIDTH,
        height: ROW_HEIGHT,
        x: Scale::Linear(-0.5, classes as f64 - 0.5),
        y: Scale::Linear(bottom, SNR_TOP),
    };
    axes.frame(canvas, "Texture SNR (dB); black line: floor");
    axes.y_axis(canvas, &linear_ticks(bottom, SNR_TOP, 10.0, 0));
    let ticks: Vec<(f64, String)> = TextureClass::ALL
        .iter()
        .enumerate()
        .map(|(index, class)| {
            let label = match class {
                TextureClass::ModulationPower => "mod",
                other => other.symbol(),
            };
            (index as f64, label.to_owned())
        })
        .collect();
    axes.x_axis(canvas, &ticks, "Statistic class; top = identical");
    let others = comparison.others.len().max(1);
    let slot = 0.8 / others as f64;
    let zero = axes.y_pixel(0.0);
    for (file, other) in comparison.others.iter().enumerate() {
        for (index, class) in other.texture.classes.iter().enumerate() {
            let snr = class.snr_decibels.unwrap_or(SNR_TOP).min(SNR_TOP);
            let start = axes.x_pixel(index as f64 - 0.4 + slot * file as f64);
            let end = axes.x_pixel(index as f64 - 0.4 + slot * (file as f64 + 1.0)) - 1;
            let level = axes.y_pixel(snr);
            let (upper, lower) = (level.min(zero), level.max(zero));
            canvas.fill(
                start as usize,
                upper as usize,
                (end - start).max(1) as usize,
                (lower - upper).max(1) as usize,
                file_colour(file + 1).1,
            );
            // The scatter floor: a bar reaching this line differs no more
            // than two takes of the reference's sound would.
            if let Some(floor) = class.scatter_floor_decibels {
                let y = axes.y_pixel(floor.min(SNR_TOP)) as usize;
                canvas.fill(start as usize - 3, y, (end - start) as usize + 6, 3, BLACK);
            }
        }
    }
    axes.level_line(canvas, 20.0, BLACK, "20 dB");
}

/// M2 and M4 per subband, and C between neighbouring subbands.
fn subband_panels(canvas: &mut Canvas, files: &[&Compared], top: usize) {
    type Pick = fn(&crate::report::Subband) -> f64;
    let per_subband = |pick: Pick| -> Vec<Curve> {
        files
            .iter()
            .map(|file| {
                file.report
                    .texture
                    .subbands
                    .iter()
                    .filter(|subband| subband.mean > 0.0)
                    .map(|subband| (subband.centre_hertz, Some(pick(subband))))
                    .collect()
            })
            .collect()
    };
    let correlations: Vec<Curve> = files
        .iter()
        .map(|file| {
            let texture = &file.report.texture;
            let Some(neighbours) = texture
                .correlations
                .iter()
                .find(|correlation| correlation.offset == 1)
            else {
                return Vec::new();
            };
            neighbours
                .values
                .iter()
                .enumerate()
                .filter_map(|(pair, value)| {
                    let lower = texture.subbands.get(pair)?;
                    let upper = texture.subbands.get(pair + 1)?;
                    (lower.mean > 0.0 && upper.mean > 0.0).then(|| {
                        (
                            (lower.centre_hertz * upper.centre_hertz).sqrt(),
                            Some(*value),
                        )
                    })
                })
                .collect()
        })
        .collect();
    let mean_correlation = files
        .iter()
        .map(|file| correlation_at_offset(&file.report.texture, 1).unwrap_or(0.0))
        .map(|mean| format!("{mean:.2}"))
        .collect::<Vec<_>>()
        .join("/");
    // Each panel's curves, title, and a value marked across it.
    type Marked = Option<(f64, &'static str)>;
    let panels: [(Vec<Curve>, String, Marked); 3] = [
        (
            per_subband(|subband| subband.variance_over_mean_squared),
            "M2 per subband (variance / mean^2)".to_owned(),
            None,
        ),
        (
            per_subband(|subband| subband.kurtosis),
            "M4 per subband (kurtosis)".to_owned(),
            Some((3.0, "Gaussian 3")),
        ),
        (
            correlations,
            format!("C, neighbours (means {mean_correlation})"),
            Some((0.0, "")),
        ),
    ];
    for (column, (curves, title, line)) in panels.into_iter().enumerate() {
        let values = curves.iter().flatten().filter_map(|(_, value)| *value);
        let mut highest = values.clone().fold(f64::NEG_INFINITY, f64::max);
        let mut lowest = values.fold(f64::INFINITY, f64::min);
        if let Some((value, _)) = line {
            highest = highest.max(value);
            lowest = lowest.min(value);
        }
        if column == 0 {
            lowest = 0.0;
        }
        let step = nice_step(highest - lowest);
        let range = rounded_range(lowest, highest, step);
        let axes = Axes {
            left: THIRDS[column],
            top,
            width: THIRD_WIDTH,
            height: ROW_HEIGHT,
            x: Scale::Logarithmic(40.0, 16_000.0),
            y: Scale::Linear(range.0, range.1),
        };
        axes.frame(canvas, &title);
        let digits = if step >= 1.0 {
            0
        } else if step >= 0.1 {
            1
        } else {
            2
        };
        axes.y_axis(canvas, &linear_ticks(range.0, range.1, step, digits));
        axes.x_axis(
            canvas,
            &logarithmic_ticks(40.0, 16_000.0),
            "Subband centre (Hz, log)",
        );
        if let Some((value, label)) = line {
            axes.level_line(canvas, value, MUTED, label);
        }
        for (index, curve) in curves.iter().enumerate().rev() {
            axes.curve(canvas, curve, pen(index), true);
        }
    }
}

/// IACC and envelope coherence per octave, for two-channel files.
fn space_panels(canvas: &mut Canvas, files: &[&Compared], top: usize) {
    type Pick = fn(&crate::report::BinauralBand) -> Option<f64>;
    let panels: [(Pick, &str); 2] = [
        (
            |band| band.iacc,
            "IACC per octave, whole file (0 = unlike, 1 = alike ears)",
        ),
        (
            |band| band.envelope_coherence,
            "Envelope coherence per octave (1 = levels move together)",
        ),
    ];
    for (column, (pick, title)) in panels.into_iter().enumerate() {
        let curves: Vec<Curve> = files
            .iter()
            .map(|file| {
                file.report
                    .space
                    .as_ref()
                    .and_then(|space| space.pair.as_ref())
                    .map_or(Vec::new(), |pair| {
                        pair.octave_bands
                            .iter()
                            .map(|band| (band.centre_hertz, pick(band)))
                            .collect()
                    })
            })
            .collect();
        let lowest = curves
            .iter()
            .flatten()
            .filter_map(|(_, value)| *value)
            .fold(0.0f64, f64::min);
        let bottom = (lowest / 0.2).floor() * 0.2;
        let axes = Axes {
            left: HALVES[column],
            top,
            width: HALF_WIDTH,
            height: ROW_HEIGHT,
            x: Scale::Logarithmic(88.0, 11_300.0),
            y: Scale::Linear(bottom, 1.0),
        };
        axes.frame(canvas, title);
        axes.y_axis(canvas, &linear_ticks(bottom, 1.0, 0.2, 1));
        let ticks: Vec<(f64, String)> = [125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0]
            .into_iter()
            .map(|centre| {
                let label = if centre >= 1000.0 {
                    format!("{}k", centre / 1000.0)
                } else {
                    format!("{centre}")
                };
                (centre, label)
            })
            .collect();
        axes.x_axis(canvas, &ticks, "Octave band (Hz)");
        for (index, curve) in curves.iter().enumerate().rev() {
            axes.curve(canvas, curve, pen(index), true);
        }
    }
}

/// Each file's spectrogram as a strip, one time scale for all (the longest
/// file spans the width), the same log frequency axis and the same 70 dB
/// below each file's own loudest cell.
fn spectrograms(canvas: &mut Canvas, files: &[&Compared], top: usize, step: usize) {
    let longest = files
        .iter()
        .map(|file| file.report.signal.seconds)
        .fold(0.0f64, f64::max)
        .max(1e-6);
    let mut strip_top = top;
    for (index, file) in files.iter().enumerate() {
        let (name, rgb) = file_colour(index);
        let label = if index == 0 {
            "ref".to_owned()
        } else {
            index.to_string()
        };
        canvas.text(
            FULL_LEFT,
            strip_top,
            &format!(
                "Spectrogram {label} ({name}): {} - frequency (Hz, log) over time, colour = dB re its own loudest cell",
                file.name
            ),
            if index == 0 { BLACK } else { rgb },
        );
        let panel_top = strip_top + LINE_HEIGHT;
        let columns =
            ((file.report.signal.seconds / longest * FULL_WIDTH as f64).round() as usize).max(1);
        match &file.signal {
            Some(signal) => {
                let channels: Vec<Vec<f32>> = (0..signal.channels)
                    .map(|channel| signal.channel(channel))
                    .collect();
                let scale = Combination::for_channels(signal.channels).scale(signal.channels);
                let cells = grid(&channels, signal.sample_rate, scale, columns, STRIP_HEIGHT);
                for row in 0..cells.rows {
                    for column in 0..cells.columns {
                        canvas.set(
                            (FULL_LEFT + column) as isize,
                            (panel_top + row) as isize,
                            colour(cells.decibels[row * cells.columns + column]),
                        );
                    }
                }
            }
            None => canvas.text(
                FULL_LEFT + 10,
                panel_top + STRIP_HEIGHT / 2 - LINE_HEIGHT / 2,
                "no samples: this file was read from its report JSON",
                MUTED,
            ),
        }
        canvas.outline(
            FULL_LEFT - 1,
            panel_top - 1,
            columns + 2,
            STRIP_HEIGHT + 2,
            if index == 0 { BLACK } else { rgb },
        );
        for (frequency, text) in [(100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")] {
            let fraction = frequency_fraction(frequency);
            let y = panel_top + STRIP_HEIGHT - 1 - (fraction * STRIP_HEIGHT as f64) as usize;
            canvas.fill(FULL_LEFT - 8, y, 8, 1, BLACK);
            canvas.text_right(
                FULL_LEFT - 12,
                y.saturating_sub(LINE_HEIGHT / 2 - 3),
                text,
                BLACK,
            );
        }
        strip_top += step;
    }

    // The time axis under the last strip, for every strip.
    let bottom = strip_top - 6;
    let seconds_step = time_step(longest);
    let mut time = 0.0;
    while time <= longest + 1e-9 {
        let x = FULL_LEFT + ((time / longest) * (FULL_WIDTH - 1) as f64).round() as usize;
        canvas.fill(x, bottom, 1, 6, BLACK);
        let digits = usize::from(seconds_step < 1.0);
        canvas.text_centred(x, bottom + 8, &format!("{time:.digits$}"), BLACK);
        time += seconds_step;
    }
    canvas.text_centred(
        FULL_LEFT + FULL_WIDTH / 2,
        bottom + 8 + LINE_HEIGHT,
        "Time (s), one scale for every strip",
        BLACK,
    );

    // The colour scale beside the strips.
    let bar_left = FULL_LEFT + FULL_WIDTH + 16;
    let bar_width = 18;
    let bar_top = top + LINE_HEIGHT;
    let bar_height = strip_top - step + STRIP_HEIGHT + LINE_HEIGHT - bar_top;
    for row in 0..bar_height {
        let decibels = -RANGE_DECIBELS * row as f64 / (bar_height - 1) as f64;
        canvas.fill(bar_left, bar_top + row, bar_width, 1, colour(decibels));
    }
    canvas.outline(
        bar_left - 1,
        bar_top - 1,
        bar_width + 2,
        bar_height + 2,
        BLACK,
    );
    for tick in 0..=7 {
        let y = bar_top + ((bar_height - 1) as f64 * tick as f64 / 7.0).round() as usize;
        canvas.fill(bar_left + bar_width, y, 4, 1, BLACK);
        canvas.text(
            bar_left + bar_width + 6,
            y.saturating_sub(LINE_HEIGHT / 2 - 3),
            &format!("{}", -10 * tick),
            BLACK,
        );
    }
    canvas.text(bar_left - 6, top, "dB", BLACK);
}

/// A step that cuts `span` into about four to eight parts: 1, 2 or 5 times
/// a power of ten.
fn nice_step(span: f64) -> f64 {
    let rough = (span / 5.0).max(1e-6);
    let power = 10f64.powf(rough.log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|factor| factor * power)
        .find(|step| *step >= rough)
        .unwrap_or(10.0 * power)
}
