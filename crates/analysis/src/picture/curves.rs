//! The panels under the spectrogram: LAF with its percentile lines, the
//! onsets, and for a pair the width and the ILD, each on the shared time
//! axis.

use super::canvas::{Canvas, Colour, BLACK, CHARACTER_WIDTH, GRID, LINE_HEIGHT, MUTED};
use super::{Layout, Panel};
use crate::report::Report;

const LEFT_EAR: Colour = [31, 119, 180];
const RIGHT_EAR: Colour = [214, 39, 40];
const ONSET: Colour = [40, 40, 40];
const WIDTH_LINE: Colour = [44, 140, 44];
const ILD_LINE: Colour = [128, 70, 170];
const PERCENTILE: Colour = [90, 90, 90];

/// A series sampled every `step` seconds, as one value per pixel column:
/// the largest of the samples in the column (`largest`) or their mean, or
/// for a column narrower than a step the sample under its centre. None
/// where every sample is None.
fn per_column(
    layout: &Layout,
    values: &[Option<f64>],
    step: f64,
    largest: bool,
) -> Vec<Option<f64>> {
    let columns = layout.plot_width();
    (0..columns)
        .map(|column| {
            let start = layout.seconds * column as f64 / columns as f64;
            let end = layout.seconds * (column + 1) as f64 / columns as f64;
            let first = (start / step).floor() as usize;
            let last = ((end / step).ceil() as usize).max(first + 1);
            let found: Vec<f64> = values
                .get(first.min(values.len())..last.min(values.len()))
                .unwrap_or(&[])
                .iter()
                .flatten()
                .copied()
                .collect();
            if found.is_empty() {
                return None;
            }
            Some(if largest {
                found.iter().copied().fold(f64::NEG_INFINITY, f64::max)
            } else {
                found.iter().sum::<f64>() / found.len() as f64
            })
        })
        .collect()
}

/// A series as connected line segments, broken where it is None.
fn draw_series(
    canvas: &mut Canvas,
    layout: &Layout,
    panel: &Panel,
    values: &[Option<f64>],
    range: (f64, f64),
    colour: Colour,
) {
    let mut previous: Option<(isize, isize)> = None;
    for (column, value) in values.iter().enumerate() {
        match value {
            Some(value) => {
                let point = (
                    (layout.left + column) as isize,
                    panel.y_of(*value, range) as isize,
                );
                canvas.line(previous.unwrap_or(point), point, colour);
                previous = Some(point);
            }
            None => previous = None,
        }
    }
}

/// Horizontal value ticks and labels on the left, and grid lines.
fn value_axis(
    canvas: &mut Canvas,
    layout: &Layout,
    panel: &Panel,
    range: (f64, f64),
    ticks: &[f64],
    digits: usize,
) {
    for tick in ticks {
        let y = panel.y_of(*tick, range);
        canvas.dotted(layout.left, y, layout.plot_width(), 3, GRID);
        canvas.fill(layout.left - 6, y, 6, 1, BLACK);
        canvas.text_right(
            layout.left - 10,
            y.saturating_sub(LINE_HEIGHT / 2 - 3),
            &format!("{tick:.digits$}"),
            BLACK,
        );
    }
}

/// LAF per channel (L blue, R red; one channel or a speaker layout: all
/// together, black) with L10, L50 and L90 of all together as dotted lines.
pub(crate) fn laf(canvas: &mut Canvas, layout: &Layout, panel: &Panel, report: &Report) {
    let over_time = &report.over_time;
    let step = over_time.laf_step_seconds;
    let as_options = |series: &[f64]| series.iter().map(|value| Some(*value)).collect::<Vec<_>>();
    let curves: Vec<(Vec<Option<f64>>, Colour)> = if over_time.laf_channels.len() == 2 {
        vec![
            (
                per_column(layout, &as_options(&over_time.laf_channels[0]), step, true),
                LEFT_EAR,
            ),
            (
                per_column(layout, &as_options(&over_time.laf_channels[1]), step, true),
                RIGHT_EAR,
            ),
        ]
    } else {
        vec![(
            per_column(layout, &as_options(&over_time.laf_overall), step, true),
            BLACK,
        )]
    };
    let level = &report.level.overall;
    let values = curves.iter().flat_map(|(curve, _)| curve.iter().flatten());
    let highest = values
        .clone()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max)
        .max(level.l10);
    let lowest = values.copied().fold(f64::INFINITY, f64::min).min(level.l90);
    let top = ((highest + 2.0) / 10.0).ceil() * 10.0;
    let bottom = (((lowest - 2.0) / 10.0).floor() * 10.0).max(top - 80.0);
    let spacing = if top - bottom <= 30.0 { 5.0 } else { 10.0 };
    let ticks: Vec<f64> = (0..)
        .map(|index| bottom + spacing * index as f64)
        .take_while(|tick| *tick <= top + 1e-9)
        .collect();
    value_axis(canvas, layout, panel, (bottom, top), &ticks, 0);
    // L10 is never below L50 nor L50 below L90, so the labels go top to
    // bottom; each is pushed down clear of the one above when they are close.
    let mut label_floor = 0;
    for (name, value) in [("L10", level.l10), ("L50", level.l50), ("L90", level.l90)] {
        let y = panel.y_of(value, (bottom, top));
        canvas.dotted(layout.left, y, layout.plot_width(), 4, PERCENTILE);
        let label = y.saturating_sub(LINE_HEIGHT / 2 - 3).max(label_floor);
        canvas.text(
            layout.right() + 8,
            label,
            &format!("{name} {value:.1}"),
            PERCENTILE,
        );
        label_floor = label + LINE_HEIGHT - 2;
    }
    for (curve, colour) in &curves {
        draw_series(canvas, layout, panel, curve, (bottom, top), *colour);
    }
}

/// One tick per onset across the panel.
pub(crate) fn onsets(canvas: &mut Canvas, layout: &Layout, panel: &Panel, report: &Report) {
    for time in &report.events.onset_seconds {
        let x = layout.x_of(*time);
        canvas.fill(x, panel.top + 2, 1, panel.height - 4, ONSET);
    }
}

/// Width per 100 ms frame, 0 to 1.
pub(crate) fn width(canvas: &mut Canvas, layout: &Layout, panel: &Panel, report: &Report) {
    let range = (0.0, 1.0);
    value_axis(canvas, layout, panel, range, &[0.0, 0.5, 1.0], 1);
    let over_time = &report.over_time;
    let values = per_column(layout, &over_time.width, over_time.frame_seconds, false);
    draw_series(canvas, layout, panel, &values, range, WIDTH_LINE);
}

/// ILD per 100 ms frame, symmetric about 0 dB.
pub(crate) fn ild(canvas: &mut Canvas, layout: &Layout, panel: &Panel, report: &Report) {
    let over_time = &report.over_time;
    let largest = over_time
        .ild_decibels
        .iter()
        .flatten()
        .fold(0.0f64, |largest, value| largest.max(value.abs()));
    let extent = ((largest / 6.0).ceil() * 6.0).clamp(6.0, 30.0);
    let range = (-extent, extent);
    value_axis(
        canvas,
        layout,
        panel,
        range,
        &[-extent, -extent / 2.0, 0.0, extent / 2.0, extent],
        0,
    );
    let zero = panel.y_of(0.0, range);
    canvas.fill(layout.left, zero, layout.plot_width(), 1, MUTED);
    let values = per_column(
        layout,
        &over_time.ild_decibels,
        over_time.frame_seconds,
        false,
    );
    draw_series(canvas, layout, panel, &values, range, ILD_LINE);
}

/// The legend of the LAF panel, written after its title.
pub(crate) fn laf_legend(canvas: &mut Canvas, x: usize, y: usize, report: &Report) {
    if report.over_time.laf_channels.len() == 2 {
        let names = &report.signal.channel_names;
        let first = format!("{}: blue", names[0]);
        canvas.text(x, y, &first, LEFT_EAR);
        let offset = (first.len() + 2) * CHARACTER_WIDTH;
        canvas.text(x + offset, y, &format!("{}: red", names[1]), RIGHT_EAR);
    }
}
