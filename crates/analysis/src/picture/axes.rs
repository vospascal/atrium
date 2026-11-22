//! A panel with two labelled axes, for the comparison's overlays: curves of
//! several files over frequency, modulation rate or percentile on one scale.

use super::canvas::{Canvas, Colour, BLACK, CHARACTER_WIDTH, GRID, LINE_HEIGHT};

/// How values map onto an axis. A range may run downwards (the LAF
/// percentiles go from L95 on the left to L5 on the right).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Scale {
    Linear(f64, f64),
    Logarithmic(f64, f64),
}

impl Scale {
    /// Where `value` sits from the start (0) to the end (1), unclamped.
    fn fraction(self, value: f64) -> f64 {
        match self {
            Self::Linear(start, end) => (value - start) / (end - start),
            Self::Logarithmic(start, end) => (value / start).ln() / (end / start).ln(),
        }
    }

    fn contains(self, value: f64) -> bool {
        let fraction = self.fraction(value);
        (-1e-9..=1.0 + 1e-9).contains(&fraction)
    }
}

/// A plotting area and its two scales.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Axes {
    pub(crate) left: usize,
    pub(crate) top: usize,
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) x: Scale,
    pub(crate) y: Scale,
}

/// How a file's curve is drawn: the reference thick, the others thinner.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Pen {
    pub(crate) colour: Colour,
    /// Line width (px).
    pub(crate) thickness: usize,
}

impl Axes {
    /// The space a panel needs above its area (title) and below it (tick
    /// labels and the axis name).
    pub(crate) const ABOVE: usize = LINE_HEIGHT + 4;
    pub(crate) const BELOW: usize = 2 * LINE_HEIGHT + 10;

    pub(crate) fn right(&self) -> usize {
        self.left + self.width
    }

    pub(crate) fn bottom(&self) -> usize {
        self.top + self.height
    }

    pub(crate) fn x_pixel(&self, value: f64) -> isize {
        let fraction = self.x.fraction(value).clamp(0.0, 1.0);
        (self.left as f64 + fraction * (self.width - 1) as f64).round() as isize
    }

    /// The row of `value`; values beyond the range sit on the edge.
    pub(crate) fn y_pixel(&self, value: f64) -> isize {
        let fraction = self.y.fraction(value).clamp(0.0, 1.0);
        (self.top as f64 + (1.0 - fraction) * (self.height - 1) as f64).round() as isize
    }

    /// The title above the panel and its outline.
    pub(crate) fn frame(&self, canvas: &mut Canvas, title: &str) {
        canvas.text(self.left, self.top - LINE_HEIGHT - 2, title, BLACK);
        canvas.outline(
            self.left - 1,
            self.top - 1,
            self.width + 2,
            self.height + 2,
            BLACK,
        );
    }

    /// Ticks and labels under the panel, and the axis's name under them.
    pub(crate) fn x_axis(&self, canvas: &mut Canvas, ticks: &[(f64, String)], name: &str) {
        for (value, label) in ticks {
            if !self.x.contains(*value) {
                continue;
            }
            let x = self.x_pixel(*value) as usize;
            for y in (self.top..self.bottom()).step_by(3) {
                canvas.set(x as isize, y as isize, GRID);
            }
            canvas.fill(x, self.bottom(), 1, 6, BLACK);
            canvas.text_centred(x, self.bottom() + 8, label, BLACK);
        }
        canvas.text_centred(
            self.left + self.width / 2,
            self.bottom() + 8 + LINE_HEIGHT,
            name,
            BLACK,
        );
    }

    /// Ticks, labels and grid lines on the left.
    pub(crate) fn y_axis(&self, canvas: &mut Canvas, ticks: &[(f64, String)]) {
        for (value, label) in ticks {
            if !self.y.contains(*value) {
                continue;
            }
            let y = self.y_pixel(*value) as usize;
            canvas.dotted(self.left, y, self.width, 3, GRID);
            canvas.fill(self.left - 6, y, 6, 1, BLACK);
            canvas.text_right(
                self.left - 9,
                y.saturating_sub(LINE_HEIGHT / 2 - 3),
                label,
                BLACK,
            );
        }
    }

    /// A horizontal line across the panel at `value`, with `label` at its
    /// right end inside the panel.
    pub(crate) fn level_line(&self, canvas: &mut Canvas, value: f64, colour: Colour, label: &str) {
        let y = self.y_pixel(value) as usize;
        canvas.dotted(self.left, y, self.width, 2, colour);
        let width = label.chars().count() * CHARACTER_WIDTH;
        canvas.text(
            self.right().saturating_sub(width + 4),
            y.saturating_sub(LINE_HEIGHT - 4),
            label,
            colour,
        );
    }

    /// A curve through `points`, broken where a value is missing or its
    /// position lies outside the x range; with `markers`, a square on each
    /// point, so a curve of few points shows where they are.
    pub(crate) fn curve(
        &self,
        canvas: &mut Canvas,
        points: &[(f64, Option<f64>)],
        pen: Pen,
        markers: bool,
    ) {
        let mut previous: Option<(isize, isize)> = None;
        for (position, value) in points {
            let Some(value) = value.filter(|_| self.x.contains(*position)) else {
                previous = None;
                continue;
            };
            let point = (self.x_pixel(*position), self.y_pixel(value));
            thick_line(canvas, previous.unwrap_or(point), point, pen);
            if markers {
                let half = pen.thickness as isize + 1;
                for dx in -half..=half {
                    for dy in -half..=half {
                        canvas.set(point.0 + dx, point.1 + dy, pen.colour);
                    }
                }
            }
            previous = Some(point);
        }
    }
}

/// A line `pen.thickness` pixels wide: the one-pixel line drawn again beside
/// itself.
pub(crate) fn thick_line(canvas: &mut Canvas, from: (isize, isize), to: (isize, isize), pen: Pen) {
    let thickness = pen.thickness.max(1) as isize;
    let first = -(thickness - 1) / 2;
    for dx in first..first + thickness {
        for dy in first..first + thickness {
            canvas.line(
                (from.0 + dx, from.1 + dy),
                (to.0 + dx, to.1 + dy),
                pen.colour,
            );
        }
    }
}

/// Ticks at 1, 2 and 5 times the powers of ten within `low` to `high`,
/// labelled as the analyser labels frequencies (500, 1k, 2k).
pub(crate) fn logarithmic_ticks(low: f64, high: f64) -> Vec<(f64, String)> {
    let mut ticks = Vec::new();
    let mut decade = 10f64.powf(low.log10().floor());
    while decade <= high {
        for factor in [1.0, 2.0, 5.0] {
            let value = decade * factor;
            if value >= low * (1.0 - 1e-9) && value <= high * (1.0 + 1e-9) {
                let label = if value >= 1000.0 {
                    format!("{}k", value / 1000.0)
                } else if value >= 1.0 {
                    format!("{value}")
                } else {
                    format!("{value:.1}")
                };
                ticks.push((value, label));
            }
        }
        decade *= 10.0;
    }
    ticks
}

/// Ticks every `step` from `low` to `high`, with `digits` decimals.
pub(crate) fn linear_ticks(low: f64, high: f64, step: f64, digits: usize) -> Vec<(f64, String)> {
    let first = (low / step).ceil() as i64;
    let last = (high / step).floor() as i64;
    (first..=last)
        .map(|index| {
            let value = index as f64 * step;
            (value, format!("{value:.digits$}"))
        })
        .collect()
}

/// A range from `low` to `high` widened to whole `step`s, at least one step.
pub(crate) fn rounded_range(low: f64, high: f64, step: f64) -> (f64, f64) {
    let bottom = (low / step).floor() * step;
    let top = ((high / step).ceil() * step).max(bottom + step);
    (bottom, top)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_are_round() {
        let labels: Vec<String> = logarithmic_ticks(0.5, 200.0)
            .into_iter()
            .map(|(_, label)| label)
            .collect();
        assert_eq!(
            labels,
            ["0.5", "1", "2", "5", "10", "20", "50", "100", "200"]
        );
        assert_eq!(logarithmic_ticks(20.0, 20_000.0)[5].1, "1k");
        assert_eq!(
            linear_ticks(-12.0, 12.0, 6.0, 0)
                .into_iter()
                .map(|(value, _)| value)
                .collect::<Vec<_>>(),
            [-12.0, -6.0, 0.0, 6.0, 12.0]
        );
        assert_eq!(rounded_range(-43.0, -9.0, 10.0), (-50.0, 0.0));
    }

    /// A downward range puts its start on the left: L95 left of L5.
    #[test]
    fn a_range_may_run_downwards() {
        let axes = Axes {
            left: 100,
            top: 50,
            width: 101,
            height: 101,
            x: Scale::Linear(100.0, 0.0),
            y: Scale::Linear(0.0, 10.0),
        };
        assert_eq!(axes.x_pixel(95.0), 105);
        assert_eq!(axes.x_pixel(5.0), 195);
        assert_eq!(axes.y_pixel(10.0), 50);
        assert_eq!(axes.y_pixel(20.0), 50);
    }
}
