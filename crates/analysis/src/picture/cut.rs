//! The cut's picture: 1600 pixels wide, every axis labelled in text.
//!
//! 1. the recording's spectrogram, a strip a minute for a long file, each
//!    take boxed and labelled with its number and type, the dropped ones
//!    boxed grey, the detection band marked;
//! 2. under each strip, the band level with the local bed and both
//!    thresholds;
//! 3. the gaps between events as a histogram over log time, with the two
//!    fitted processes and the bout criterion;
//! 4. the takes' similarity matrix, ordered by type.

use super::axes::{linear_ticks, rounded_range, Axes, Pen, Scale};
use super::canvas::{Canvas, Colour, BLACK, CHARACTER_WIDTH, LINE_HEIGHT, MUTED, WHITE};
use super::spectrogram::{colour, grid, RANGE_DECIBELS};
use super::{time_step, wrap, WIDTH};
use crate::cut::Cut;
use crate::frequency_rows::frequency_fraction;
use crate::signal::{Combination, Signal};

const LEFT: usize = 110;
const PLOT_WIDTH: usize = 1380;
/// A strip spans at most this long (s).
const STRIP_SECONDS: f64 = 60.0;
const SPECTROGRAM_HEIGHT: usize = 220;
const LEVEL_HEIGHT: usize = 140;
const GAP: usize = 12;
const BOTTOM_HEIGHT: usize = 400;
const HALVES: [usize; 2] = [110, 880];
const HALF_WIDTH: usize = 640;

const BAND_LINE: Colour = [0, 220, 255];
const TAKE_BOX: Colour = [255, 255, 255];
const DROPPED_BOX: Colour = [150, 150, 150];
const LEVEL_LINE: Colour = [31, 119, 180];
const BED_LINE: Colour = [90, 90, 90];
const START_LINE: Colour = [214, 39, 40];
const END_LINE: Colour = [255, 127, 14];
const FAST: Colour = [31, 119, 180];
const SLOW: Colour = [255, 127, 14];
const BARS: Colour = [200, 200, 200];

/// The picture of `cut`, of `signal`, under `heading`, as a PNG file.
pub fn cut_picture(heading: &str, signal: &Signal, cut: &Cut) -> Vec<u8> {
    let strip_seconds = cut.seconds.clamp(1e-3, STRIP_SECONDS);
    let strips = ((cut.seconds / strip_seconds).ceil() as usize).max(1);
    let fitting = (WIDTH - 20) / CHARACTER_WIDTH;
    let mut heading_lines = wrap(heading, fitting);
    heading_lines.truncate(2);
    let key = wrap(&key_line(cut), fitting);
    let top = 8 + (heading_lines.len() + key.len()) * LINE_HEIGHT + GAP;
    let strip_height =
        LINE_HEIGHT + SPECTROGRAM_HEIGHT + GAP + LINE_HEIGHT + LEVEL_HEIGHT + 2 * LINE_HEIGHT + GAP;
    let bottom_top = top + strips * strip_height + Axes::ABOVE;
    let height = bottom_top + BOTTOM_HEIGHT + Axes::BELOW + GAP;
    let mut canvas = Canvas::new(WIDTH, height);
    let mut y = 8;
    for line in &heading_lines {
        canvas.text(10, y, line, BLACK);
        y += LINE_HEIGHT;
    }
    for line in &key {
        canvas.text(10, y, line, MUTED);
        y += LINE_HEIGHT;
    }

    let channels: Vec<Vec<f32>> = (0..signal.channels)
        .map(|index| signal.channel(index))
        .collect();
    let scale = Combination::for_channels(signal.channels).scale(signal.channels);
    let columns = ((cut.seconds / strip_seconds * PLOT_WIDTH as f64).round() as usize).max(1);
    let cells = grid(
        &channels,
        signal.sample_rate,
        scale,
        columns,
        SPECTROGRAM_HEIGHT,
    );
    for strip in 0..strips {
        let strip_top = top + strip * strip_height;
        let from = strip as f64 * strip_seconds;
        let to = (from + strip_seconds).min(cut.seconds);
        draw_strip(
            &mut canvas,
            cut,
            &cells,
            strip,
            strip_top,
            (from, to),
            strip_seconds,
        );
    }
    histogram(&mut canvas, cut, bottom_top);
    matrix(&mut canvas, cut, bottom_top);
    canvas.png()
}

/// The numbers a reader looks for first, in one line.
fn key_line(cut: &Cut) -> String {
    let criterion = match cut.criterion.criterion_seconds {
        Some(seconds) => format!("bout criterion {seconds:.2} s"),
        None => "no bout criterion (nothing joined)".to_owned(),
    };
    let gaps = match (cut.gaps.p10, cut.gaps.p90) {
        (Some(p10), Some(p90)) => format!("gaps 10-90 % {p10:.1}-{p90:.1} s"),
        _ => "no gaps".to_owned(),
    };
    format!(
        "band {:.0}-{:.0} Hz | {} events | {} takes kept, {} dropped | {criterion} | {gaps} | types: {}",
        cut.band.low_hertz,
        cut.band.high_hertz,
        cut.events.len(),
        cut.takes.len(),
        cut.dropped.len(),
        cut.types.groups.len()
    )
}

/// One strip: its spectrogram with the takes boxed, and the band level under
/// it.
fn draw_strip(
    canvas: &mut Canvas,
    cut: &Cut,
    cells: &super::spectrogram::Grid,
    strip: usize,
    top: usize,
    (from, to): (f64, f64),
    strip_seconds: f64,
) {
    let x_of = |seconds: f64| -> usize {
        let fraction = ((seconds - from) / strip_seconds).clamp(0.0, 1.0);
        LEFT + ((fraction * PLOT_WIDTH as f64) as usize).min(PLOT_WIDTH - 1)
    };
    let row_of = |frequency: f64, rows: usize| -> usize {
        let fraction = frequency_fraction(frequency).clamp(0.0, 1.0 - 1e-12);
        rows - 1 - (fraction * rows as f64) as usize
    };
    let title = if strip == 0 {
        "Spectrogram (Hz, log; colour = dB re the loudest cell, 70 dB); boxes: takes kept (white: number and type) and dropped (grey); cyan: the detection band"
            .to_owned()
    } else {
        format!("Spectrogram, {from:.0}-{to:.0} s")
    };
    canvas.text(LEFT, top, &title, BLACK);
    let spectrogram_top = top + LINE_HEIGHT;
    let first_column = strip * PLOT_WIDTH;
    let width = cells.columns.saturating_sub(first_column).min(PLOT_WIDTH);
    for row in 0..cells.rows {
        for column in 0..width {
            canvas.set(
                (LEFT + column) as isize,
                (spectrogram_top + row) as isize,
                colour(cells.decibels[row * cells.columns + first_column + column]),
            );
        }
    }
    for (frequency, label) in [(100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")] {
        let y = spectrogram_top + row_of(frequency, SPECTROGRAM_HEIGHT);
        canvas.fill(LEFT - 8, y, 8, 1, BLACK);
        canvas.text_right(
            LEFT - 12,
            y.saturating_sub(LINE_HEIGHT / 2 - 3),
            label,
            BLACK,
        );
    }
    let band_top = spectrogram_top + row_of(cut.band.high_hertz, SPECTROGRAM_HEIGHT);
    let band_bottom = spectrogram_top + row_of(cut.band.low_hertz.max(20.0), SPECTROGRAM_HEIGHT);
    for y in [band_top, band_bottom] {
        canvas.dotted(LEFT, y, width, 4, BAND_LINE);
    }
    let in_strip = |start: f64, end: f64| end > from && start < to;
    for dropped in cut
        .dropped
        .iter()
        .filter(|dropped| in_strip(dropped.start_seconds, dropped.end_seconds))
    {
        let (left, right) = (x_of(dropped.start_seconds), x_of(dropped.end_seconds));
        canvas.outline(
            left,
            band_top,
            (right - left).max(2) + 1,
            band_bottom - band_top + 1,
            DROPPED_BOX,
        );
    }
    for take in cut
        .takes
        .iter()
        .filter(|take| in_strip(take.start_seconds, take.end_seconds))
    {
        let (left, right) = (x_of(take.start_seconds), x_of(take.end_seconds));
        let box_width = (right - left).max(2) + 1;
        canvas.outline(
            left,
            band_top,
            box_width,
            band_bottom - band_top + 1,
            TAKE_BOX,
        );
        canvas.outline(
            left + 1,
            band_top + 1,
            box_width.saturating_sub(2).max(1),
            (band_bottom - band_top).saturating_sub(1).max(1),
            TAKE_BOX,
        );
        let label = format!(
            "{}{}{}",
            take.number,
            take.type_name,
            if take.flagged { "!" } else { "" }
        );
        let label_y = band_top
            .saturating_sub(LINE_HEIGHT)
            .max(spectrogram_top + 1);
        canvas.fill(
            left,
            label_y,
            label.chars().count() * CHARACTER_WIDTH + 2,
            LINE_HEIGHT - 4,
            BLACK,
        );
        canvas.text(left + 1, label_y + 1, &label, WHITE);
    }
    canvas.outline(
        LEFT - 1,
        spectrogram_top - 1,
        PLOT_WIDTH + 2,
        SPECTROGRAM_HEIGHT + 2,
        BLACK,
    );

    // The band level, its bed and the thresholds.
    let level_top = spectrogram_top + SPECTROGRAM_HEIGHT + GAP + LINE_HEIGHT;
    let thresholds = &cut.thresholds;
    canvas.text(
        LEFT,
        level_top - LINE_HEIGHT,
        &format!(
            "Band level (dB, blue), local bed (grey), start +{:.0} dB (red) and end +{:.0} dB (orange) over it",
            thresholds.start_above_bed_decibels, thresholds.end_above_bed_decibels
        ),
        BLACK,
    );
    let step = cut.level.step_seconds;
    let first = (from / step).floor() as usize;
    let last = ((to / step).ceil() as usize).min(cut.level.band_decibels.len());
    let levels = &cut.level.band_decibels[first.min(last)..last];
    let beds = &cut.level.bed_decibels[first.min(last)..last];
    let lowest = beds.iter().copied().fold(f64::INFINITY, f64::min) - 10.0;
    let highest = levels
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max)
        .max(
            beds.iter().copied().fold(f64::NEG_INFINITY, f64::max)
                + thresholds.start_above_bed_decibels,
        )
        + 3.0;
    let (bottom, top_value) = if lowest.is_finite() && highest.is_finite() {
        rounded_range(lowest, highest, 10.0)
    } else {
        (-100.0, 0.0)
    };
    let axes = Axes {
        left: LEFT,
        top: level_top,
        width: PLOT_WIDTH,
        height: LEVEL_HEIGHT,
        x: Scale::Linear(from, from + strip_seconds),
        y: Scale::Linear(bottom, top_value),
    };
    let tick_step = if top_value - bottom > 60.0 {
        20.0
    } else {
        10.0
    };
    axes.y_axis(canvas, &linear_ticks(bottom, top_value, tick_step, 0));
    let per_column = |values: &[f64], offset: f64, largest: bool| -> Vec<(f64, Option<f64>)> {
        let mut points = Vec::with_capacity(PLOT_WIDTH);
        for column in 0..PLOT_WIDTH {
            let start = from + strip_seconds * column as f64 / PLOT_WIDTH as f64;
            let end = from + strip_seconds * (column + 1) as f64 / PLOT_WIDTH as f64;
            let first_index = ((start / step).floor() as usize).max(first) - first;
            let last_index =
                (((end / step).ceil() as usize).max(first + 1) - first).min(values.len());
            if first_index >= last_index || start >= to {
                points.push((start, None));
                continue;
            }
            let slice = &values[first_index..last_index];
            let value = if largest {
                slice.iter().copied().fold(f64::NEG_INFINITY, f64::max)
            } else {
                slice[0]
            };
            points.push((start, Some(value + offset)));
        }
        points
    };
    let pen = |colour: Colour| Pen {
        colour,
        thickness: 1,
    };
    axes.curve(
        canvas,
        &per_column(levels, 0.0, true),
        pen(LEVEL_LINE),
        false,
    );
    axes.curve(
        canvas,
        &per_column(beds, 0.0, false),
        Pen {
            colour: BED_LINE,
            thickness: 2,
        },
        false,
    );
    axes.curve(
        canvas,
        &per_column(beds, thresholds.end_above_bed_decibels, false),
        pen(END_LINE),
        false,
    );
    axes.curve(
        canvas,
        &per_column(beds, thresholds.start_above_bed_decibels, false),
        pen(START_LINE),
        false,
    );
    canvas.outline(
        LEFT - 1,
        level_top - 1,
        PLOT_WIDTH + 2,
        LEVEL_HEIGHT + 2,
        BLACK,
    );
    let seconds_step = time_step(strip_seconds);
    let ticks: Vec<(f64, String)> = (0..)
        .map(|index| from + seconds_step * index as f64)
        .take_while(|time| *time <= from + strip_seconds + 1e-9)
        .map(|time| {
            let digits = usize::from(seconds_step < 1.0);
            (time, format!("{time:.digits$}"))
        })
        .collect();
    axes.x_axis(canvas, &ticks, "Time (s)");
    for (time, _) in &ticks {
        let x = x_of(*time);
        canvas.fill(x, spectrogram_top + SPECTROGRAM_HEIGHT, 1, 6, BLACK);
    }
}

/// The gaps between events over log time, the two fitted processes as
/// counts per bin, and the criterion.
fn histogram(canvas: &mut Canvas, cut: &Cut, top: usize) {
    let gaps = &cut.criterion.gap_seconds;
    let title = format!(
        "{} gaps between events; fit: fast blue, slow orange",
        gaps.len()
    );
    let positive: Vec<f64> = gaps.iter().copied().filter(|gap| *gap > 0.0).collect();
    let smallest = positive.iter().copied().fold(f64::INFINITY, f64::min);
    let largest = positive.iter().copied().fold(0.0f64, f64::max);
    let (low, high) = if positive.is_empty() {
        (0.01, 100.0)
    } else {
        (
            10f64.powf(smallest.log10().floor()).min(0.01),
            10f64.powf(largest.log10().ceil()).max(10.0),
        )
    };
    // Ten bins a decade.
    let decades = (high / low).log10().round() as usize;
    let bins = 10 * decades.max(1);
    let edge = |index: usize| low * 10f64.powf(index as f64 / 10.0);
    let counts: Vec<usize> = (0..bins)
        .map(|bin| {
            positive
                .iter()
                .filter(|gap| **gap >= edge(bin) && **gap < edge(bin + 1))
                .count()
        })
        .collect();
    let expected = |rate: f64, share: f64, bin: usize| {
        gaps.len() as f64 * share * ((-rate * edge(bin)).exp() - (-rate * edge(bin + 1)).exp())
    };
    let two = cut.criterion.two;
    let tallest = counts.iter().copied().max().unwrap_or(0) as f64;
    let tallest = two.map_or(tallest, |two| {
        (0..bins)
            .map(|bin| {
                expected(two.fast.rate, two.fast.share, bin)
                    + expected(two.slow.rate, two.slow.share, bin)
            })
            .fold(tallest, f64::max)
    });
    let (_, ceiling) = rounded_range(
        0.0,
        tallest.max(1.0),
        if tallest > 20.0 { 5.0 } else { 1.0 },
    );
    let axes = Axes {
        left: HALVES[0],
        top,
        width: HALF_WIDTH,
        height: BOTTOM_HEIGHT,
        x: Scale::Logarithmic(low, high),
        y: Scale::Linear(0.0, ceiling),
    };
    axes.frame(canvas, &title);
    let tick = if ceiling > 50.0 {
        10.0
    } else if ceiling > 10.0 {
        5.0
    } else {
        1.0
    };
    axes.y_axis(canvas, &linear_ticks(0.0, ceiling, tick, 0));
    let decades: Vec<(f64, String)> = (0..=decades)
        .map(|decade| {
            let value = low * 10f64.powi(decade as i32);
            (value, format!("{value}"))
        })
        .collect();
    axes.x_axis(
        canvas,
        &decades,
        "Gap (s, log); bars: count per tenth of a decade",
    );
    for (bin, count) in counts.iter().enumerate() {
        if *count == 0 {
            continue;
        }
        let left = axes.x_pixel(edge(bin)) as usize;
        let right = axes.x_pixel(edge(bin + 1)) as usize;
        let y = axes.y_pixel(*count as f64) as usize;
        canvas.fill(
            left + 1,
            y,
            right.saturating_sub(left + 1).max(1),
            axes.bottom() - y,
            BARS,
        );
        canvas.outline(left, y, right - left + 1, axes.bottom() - y, MUTED);
    }
    if let Some(two) = two {
        for (process, colour) in [(two.fast, FAST), (two.slow, SLOW)] {
            let points: Vec<(f64, Option<f64>)> = (0..bins)
                .map(|bin| {
                    (
                        (edge(bin) * edge(bin + 1)).sqrt(),
                        Some(expected(process.rate, process.share, bin)),
                    )
                })
                .collect();
            axes.curve(
                canvas,
                &points,
                Pen {
                    colour,
                    thickness: 2,
                },
                false,
            );
        }
    }
    if let Some(criterion) = cut.criterion.criterion_seconds {
        let x = axes.x_pixel(criterion) as usize;
        canvas.fill(x, axes.top, 2, axes.height, START_LINE);
        canvas.text(
            x + 6,
            axes.top + 4,
            &format!("{criterion:.2} s"),
            START_LINE,
        );
    }
}

/// The similarity matrix, takes ordered by type, then by number.
fn matrix(canvas: &mut Canvas, cut: &Cut, top: usize) {
    let left = HALVES[1];
    canvas.text(
        left,
        top - LINE_HEIGHT - 2,
        "Take similarity by type (bright = alike)",
        BLACK,
    );
    let order: Vec<usize> = cut
        .types
        .groups
        .iter()
        .flat_map(|group| group.takes.iter().map(|number| number - 1))
        .collect();
    let count = order.len();
    if count == 0 {
        canvas.text(left + 10, top + 10, "no takes", MUTED);
        return;
    }
    let label_room = 4 * CHARACTER_WIDTH;
    let side = (BOTTOM_HEIGHT).min(HALF_WIDTH - label_room);
    let cell = (side / count).max(1);
    let size = cell * count;
    let origin = left + label_room;
    for (row, first) in order.iter().enumerate() {
        for (column, second) in order.iter().enumerate() {
            let value = cut.types.similarity[*first][*second].clamp(0.0, 1.0);
            canvas.fill(
                origin + column * cell,
                top + row * cell,
                cell,
                cell,
                colour((value - 1.0) * RANGE_DECIBELS),
            );
        }
    }
    canvas.outline(origin - 1, top - 1, size + 2, size + 2, BLACK);
    // Lines between types and their names.
    let mut start = 0;
    for group in &cut.types.groups {
        let end = start + group.takes.len();
        if end < count {
            canvas.fill(origin, top + end * cell, size, 1, WHITE);
            canvas.fill(origin + end * cell, top, 1, size, WHITE);
        }
        let middle = top + (start + end) * cell / 2;
        canvas.text_right(
            origin - 6,
            middle.saturating_sub(LINE_HEIGHT / 2 - 3),
            &group.name,
            BLACK,
        );
        start = end;
    }
    if count <= 40 && cell >= CHARACTER_WIDTH * 2 {
        for (column, take) in order.iter().enumerate() {
            canvas.text_centred(
                origin + column * cell + cell / 2,
                top + size + 6,
                &(take + 1).to_string(),
                BLACK,
            );
        }
    }
    canvas.text(
        origin,
        top + size + 6 + LINE_HEIGHT,
        &format!(
            "Takes by number; one type at {:.2} or more",
            cut.thresholds.type_similarity
        ),
        MUTED,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cut::{cut, CutSettings};
    use crate::test_signals::{sine, Noise, RATE};

    /// Three tone bursts in noise: a 1600-pixel PNG, and each take's box
    /// drawn white on the spectrogram where the take starts.
    #[test]
    fn takes_are_boxed() {
        let mut samples = Noise::new(3).pink(0.002, 12.0);
        for start in [2.0, 5.5, 9.0] {
            let at = (start * RATE as f64) as usize;
            for (offset, value) in sine(2000.0, 0.2, 0.3).into_iter().enumerate() {
                samples[at + offset] += value;
            }
        }
        let signal = Signal {
            interleaved: &samples,
            channels: 1,
            sample_rate: RATE,
            channel_names: Vec::new(),
        };
        let cut = cut(&signal, &CutSettings::default());
        assert_eq!(cut.takes.len(), 3);
        let png = cut_picture("bursts", &signal, &cut);
        let decoder = png::Decoder::new(std::io::Cursor::new(png));
        let mut reader = decoder.read_info().expect("a PNG");
        let mut pixels = vec![0; reader.output_buffer_size().expect("a size")];
        let info = reader.next_frame(&mut pixels).expect("one frame");
        assert_eq!(info.width as usize, WIDTH);
        let width = info.width as usize;
        // The first strip: the heading and key lines, then the title.
        let fitting = (WIDTH - 20) / CHARACTER_WIDTH;
        let lines = wrap("bursts", fitting).len() + wrap(&key_line(&cut), fitting).len();
        let spectrogram_top = 8 + lines * LINE_HEIGHT + GAP + LINE_HEIGHT;
        let row_of = |frequency: f64| {
            let fraction = frequency_fraction(frequency).clamp(0.0, 1.0 - 1e-12);
            spectrogram_top + SPECTROGRAM_HEIGHT
                - 1
                - (fraction * SPECTROGRAM_HEIGHT as f64) as usize
        };
        let middle = (row_of(cut.band.low_hertz) + row_of(cut.band.high_hertz)) / 2;
        for take in &cut.takes {
            let x = LEFT + (take.start_seconds / cut.seconds * PLOT_WIDTH as f64) as usize;
            let start = (middle * width + x) * 3;
            assert_eq!(&pixels[start..start + 3], &TAKE_BOX, "take {}", take.number);
        }
    }
}
