//! The picture: 1600 pixels wide, panels on one shared time axis, every
//! axis labelled in text.
//!
//! 1. the spectrogram, log frequency 20 Hz to 20 kHz, with its dB scale;
//! 2. LAF per channel with L10, L50 and L90;
//! 3. the onsets;
//! 4. for a pair: width and ILD per 100 ms frame.
//!
//! - `canvas`: the raster and the drawing it needs; `font`: the letters
//! - `spectrogram`: the spectrogram's cells and colours
//! - `curves`: the other panels
//! - `comparison`: the comparison's picture, its overlays on `axes`
//! - `cut`: the cut's picture

mod axes;
mod canvas;
mod comparison;
mod curves;
mod cut;
mod font;
mod spectrogram;

pub use comparison::comparison_picture;
pub use cut::cut_picture;

use canvas::{Canvas, BLACK, CHARACTER_WIDTH, LINE_HEIGHT, MUTED, WHITE};
use spectrogram::{colour, grid, RANGE_DECIBELS};

use crate::frequency_rows::frequency_fraction;
use crate::report::Report;
use crate::signal::{Combination, Signal};

const WIDTH: usize = 1600;
/// Room for the value labels on the left and the colour scale and
/// percentile labels on the right (px).
const LEFT: usize = 110;
const RIGHT: usize = 190;
const SPECTROGRAM_HEIGHT: usize = 420;
const LAF_HEIGHT: usize = 220;
const ONSET_HEIGHT: usize = 44;
const PAIR_HEIGHT: usize = 120;
/// Between a panel and the title of the next (px).
const GAP: usize = 14;

/// The frequencies labelled on the spectrogram; the ones in bold get a line
/// across it.
const FREQUENCY_TICKS: [(f64, &str, bool); 10] = [
    (20.0, "20", false),
    (50.0, "50", false),
    (100.0, "100", true),
    (200.0, "200", false),
    (500.0, "500", false),
    (1000.0, "1k", true),
    (2000.0, "2k", false),
    (5000.0, "5k", false),
    (10_000.0, "10k", true),
    (20_000.0, "20k", false),
];

/// The picture of `report` for `signal`, under `heading`, as a PNG file.
pub fn picture(heading: &str, signal: &Signal, report: &Report) -> Vec<u8> {
    draw(heading, signal, report).0.png()
}

/// A panel's place on the canvas.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Panel {
    pub(crate) top: usize,
    pub(crate) height: usize,
}

impl Panel {
    /// The row of `value` on a scale from `range.0` (bottom) to `range.1`.
    pub(crate) fn y_of(&self, value: f64, range: (f64, f64)) -> usize {
        let fraction = ((value - range.0) / (range.1 - range.0)).clamp(0.0, 1.0);
        self.top + ((1.0 - fraction) * (self.height - 1) as f64).round() as usize
    }

    fn bottom(&self) -> usize {
        self.top + self.height
    }
}

/// Where everything goes.
pub(crate) struct Layout {
    pub(crate) left: usize,
    pub(crate) seconds: f64,
    pub(crate) spectrogram: Panel,
    laf: Panel,
    onsets: Panel,
    width: Option<Panel>,
    ild: Option<Panel>,
    time_labels: usize,
    height: usize,
}

impl Layout {
    fn new(seconds: f64, pair: bool) -> Self {
        // Two lines of heading, one of key numbers, then the first title.
        let mut top = 3 * LINE_HEIGHT + 2 * GAP + LINE_HEIGHT;
        let mut panel = |height: usize| {
            let placed = Panel { top, height };
            top += height + GAP + LINE_HEIGHT;
            placed
        };
        let spectrogram = panel(SPECTROGRAM_HEIGHT);
        let laf = panel(LAF_HEIGHT);
        let onsets = panel(ONSET_HEIGHT);
        let (width, ild) = if pair {
            (Some(panel(PAIR_HEIGHT)), Some(panel(PAIR_HEIGHT)))
        } else {
            (None, None)
        };
        let time_labels = top - GAP - LINE_HEIGHT + 10;
        Self {
            left: LEFT,
            seconds,
            spectrogram,
            laf,
            onsets,
            width,
            ild,
            time_labels,
            height: time_labels + 2 * LINE_HEIGHT + 6,
        }
    }

    pub(crate) fn plot_width(&self) -> usize {
        WIDTH - LEFT - RIGHT
    }

    pub(crate) fn right(&self) -> usize {
        self.left + self.plot_width()
    }

    /// The pixel column whose stretch of time holds `seconds`.
    pub(crate) fn x_of(&self, seconds: f64) -> usize {
        let fraction = (seconds / self.seconds).clamp(0.0, 1.0);
        self.left + ((fraction * self.plot_width() as f64) as usize).min(self.plot_width() - 1)
    }

    /// The spectrogram row holding `frequency`.
    pub(crate) fn y_of_frequency(&self, frequency: f64) -> usize {
        let rows = self.spectrogram.height;
        let fraction = frequency_fraction(frequency).clamp(0.0, 1.0 - 1e-12);
        self.spectrogram.top + rows - 1 - (fraction * rows as f64) as usize
    }

    fn panels(&self) -> Vec<Panel> {
        [
            Some(self.spectrogram),
            Some(self.laf),
            Some(self.onsets),
            self.width,
            self.ild,
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

/// The canvas and where things are on it.
fn draw(heading: &str, signal: &Signal, report: &Report) -> (Canvas, Layout) {
    let pair = report
        .space
        .as_ref()
        .is_some_and(|space| space.pair.is_some());
    let layout = Layout::new(report.signal.seconds, pair);
    let mut canvas = Canvas::new(WIDTH, layout.height);
    let unit = report.unit.symbol();
    let level = &report.level.overall;
    let fitting = (WIDTH - 20) / CHARACTER_WIDTH;
    for (index, line) in wrap(heading, fitting).iter().take(2).enumerate() {
        canvas.text(10, 8 + index * LINE_HEIGHT, line, BLACK);
    }
    let width = report
        .space
        .as_ref()
        .and_then(|space| space.pair.as_ref())
        .and_then(|pair| pair.width)
        .map_or(String::new(), |width| format!(" | width {width:.2}"));
    canvas.text(
        10,
        8 + 2 * LINE_HEIGHT,
        &format!(
            "LAeq {:.1} {unit} | L90 {:.1} | L10 {:.1} | LAFmax {:.1} | centroid {:.0} Hz | onsets {:.0} /min{width} | {:.1} s",
            level.laeq,
            level.l90,
            level.l10,
            level.lafmax,
            report.colour.centroid_hertz,
            report.events.onsets_per_minute,
            report.signal.seconds
        ),
        MUTED,
    );

    draw_spectrogram(&mut canvas, &layout, signal);

    let title = |canvas: &mut Canvas, panel: &Panel, text: &str| {
        canvas.text(layout.left, panel.top - LINE_HEIGHT, text, BLACK);
    };
    let laf_title = format!("LAF, A-weighted FAST level ({unit}); dotted: L10, L50, L90 of all;");
    title(&mut canvas, &layout.laf, &laf_title);
    curves::laf_legend(
        &mut canvas,
        layout.left + (laf_title.len() + 1) * CHARACTER_WIDTH,
        layout.laf.top - LINE_HEIGHT,
        report,
    );
    curves::laf(&mut canvas, &layout, &layout.laf, report);
    title(
        &mut canvas,
        &layout.onsets,
        &format!(
            "Onsets (spectral flux, librosa method): {}, {:.1} per minute",
            report.events.onset_seconds.len(),
            report.events.onsets_per_minute
        ),
    );
    curves::onsets(&mut canvas, &layout, &layout.onsets, report);
    if let Some(panel) = layout.width {
        title(
            &mut canvas,
            &panel,
            "Width per 100 ms = 1 - IACC at 500 Hz-2 kHz (0 = one source, 1 = diffuse)",
        );
        curves::width(&mut canvas, &layout, &panel, report);
    }
    if let Some(panel) = layout.ild {
        title(
            &mut canvas,
            &panel,
            "ILD per 100 ms, right minus left (dB; + = louder at the right ear)",
        );
        curves::ild(&mut canvas, &layout, &panel, report);
    }

    draw_time_axis(&mut canvas, &layout);
    for panel in layout.panels() {
        canvas.outline(
            layout.left - 1,
            panel.top - 1,
            layout.plot_width() + 2,
            panel.height + 2,
            BLACK,
        );
    }
    (canvas, layout)
}

fn draw_spectrogram(canvas: &mut Canvas, layout: &Layout, signal: &Signal) {
    let panel = layout.spectrogram;
    canvas.text(
        layout.left,
        panel.top - LINE_HEIGHT,
        "Spectrogram: frequency (Hz, log scale) over time; colour = level in dB re the loudest cell (scale at right)",
        BLACK,
    );
    let channels: Vec<Vec<f32>> = (0..signal.channels)
        .map(|index| signal.channel(index))
        .collect();
    let scale = Combination::for_channels(signal.channels).scale(signal.channels);
    let cells = grid(
        &channels,
        signal.sample_rate,
        scale,
        layout.plot_width(),
        panel.height,
    );
    for row in 0..cells.rows {
        for column in 0..cells.columns {
            canvas.set(
                (layout.left + column) as isize,
                (panel.top + row) as isize,
                colour(cells.decibels[row * cells.columns + column]),
            );
        }
    }
    for (frequency, label, lined) in FREQUENCY_TICKS {
        let y = layout.y_of_frequency(frequency);
        if lined {
            canvas.dotted(layout.left, y, layout.plot_width(), 8, WHITE);
        }
        canvas.fill(layout.left - 8, y, 8, 1, BLACK);
        canvas.text_right(
            layout.left - 12,
            y.saturating_sub(LINE_HEIGHT / 2 - 3),
            label,
            BLACK,
        );
    }

    // The colour scale, 0 dB at the top to -70 at the bottom.
    let bar_left = layout.right() + 16;
    let bar_width = 22;
    for row in 0..panel.height {
        let decibels = -RANGE_DECIBELS * row as f64 / (panel.height - 1) as f64;
        canvas.fill(bar_left, panel.top + row, bar_width, 1, colour(decibels));
    }
    canvas.outline(
        bar_left - 1,
        panel.top - 1,
        bar_width + 2,
        panel.height + 2,
        BLACK,
    );
    for step in 0..=7 {
        let decibels = -10 * step;
        let y = panel.top + ((panel.height - 1) as f64 * step as f64 / 7.0).round() as usize;
        canvas.fill(bar_left + bar_width, y, 5, 1, BLACK);
        canvas.text(
            bar_left + bar_width + 8,
            y.saturating_sub(LINE_HEIGHT / 2 - 3),
            &decibels.to_string(),
            BLACK,
        );
    }
    canvas.text(
        bar_left - 4,
        panel.top - LINE_HEIGHT - GAP,
        "dB re max",
        BLACK,
    );
}

/// Ticks at round seconds on every panel, gridlines on all but the
/// spectrogram, and the labels under the last.
fn draw_time_axis(canvas: &mut Canvas, layout: &Layout) {
    let step = time_step(layout.seconds);
    let ticks: Vec<f64> = (0..)
        .map(|index| step * index as f64)
        .take_while(|time| *time <= layout.seconds + 1e-9)
        .collect();
    let panels = layout.panels();
    for time in &ticks {
        let x = layout.x_of(*time);
        for (index, panel) in panels.iter().enumerate() {
            if index > 0 {
                for y in (panel.top..panel.bottom()).step_by(3) {
                    canvas.set(x as isize, y as isize, canvas::GRID);
                }
            }
            canvas.fill(x, panel.bottom(), 1, 7, BLACK);
        }
        let digits = if step < 1.0 { 1 } else { 0 };
        canvas.text_centred(x, layout.time_labels, &format!("{time:.digits$}"), BLACK);
    }
    canvas.text_centred(
        layout.left + layout.plot_width() / 2,
        layout.time_labels + LINE_HEIGHT + 2,
        "Time (s)",
        BLACK,
    );
}

/// `text` in lines of at most `width` characters, broken after a space
/// where it can be.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut rest: Vec<char> = text.chars().collect();
    while rest.len() > width {
        let cut = rest[..width]
            .iter()
            .rposition(|character| *character == ' ')
            .filter(|position| *position > 0)
            .map_or(width, |position| position + 1);
        lines.push(rest[..cut].iter().collect::<String>().trim_end().to_owned());
        rest.drain(..cut);
    }
    lines.push(rest.into_iter().collect());
    lines
}

/// About ten ticks across, at 1, 2 or 5 times a power of ten seconds.
fn time_step(seconds: f64) -> f64 {
    let rough = (seconds / 10.0).max(1e-3);
    let power = 10f64.powf(rough.log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|factor| factor * power)
        .find(|step| *step >= rough)
        .unwrap_or(10.0 * power)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::Settings;
    use crate::test_signals::{sine, RATE};

    fn luminance(pixels: &[u8], width: usize, x: usize, y: usize) -> f64 {
        let start = (y * width + x) * 3;
        0.299 * pixels[start] as f64
            + 0.587 * pixels[start + 1] as f64
            + 0.114 * pixels[start + 2] as f64
    }

    /// A 1 kHz tone from 2 to 4 s in a 6 s file: decoded from the PNG, the
    /// brightest spectrogram row is the 1 kHz row (± 2 rows: the window's
    /// main lobe is ± 23 Hz, a row at 1 kHz is 17 Hz), and the row is bright
    /// from the 2 s tick to the 4 s tick, give or take half the 85 ms window
    /// (9 px at this scale) and a pixel.
    #[test]
    fn a_tone_sits_at_its_frequency_and_time() {
        let mut samples = vec![0.0f32; 2 * RATE as usize];
        samples.extend(sine(1000.0, 0.5, 2.0));
        samples.extend(vec![0.0f32; 2 * RATE as usize]);
        let signal = Signal {
            interleaved: &samples,
            channels: 1,
            sample_rate: RATE,
            channel_names: Vec::new(),
        };
        let report = crate::analyse(&signal, &Settings::default());
        let png = picture("tone", &signal, &report);
        let layout = Layout::new(report.signal.seconds, false);

        let decoder = png::Decoder::new(std::io::Cursor::new(png));
        let mut reader = decoder.read_info().expect("a PNG");
        let mut pixels = vec![0; reader.output_buffer_size().expect("a size")];
        let info = reader.next_frame(&mut pixels).expect("one frame");
        assert_eq!(info.width as usize, WIDTH);
        let width = info.width as usize;

        let panel = layout.spectrogram;
        let middle = layout.x_of(2.5)..layout.x_of(3.5);
        let brightest = (panel.top..panel.bottom())
            .max_by(|a, b| {
                let mean = |y: usize| {
                    middle
                        .clone()
                        .map(|x| luminance(&pixels, width, x, y))
                        .sum::<f64>()
                };
                mean(*a).total_cmp(&mean(*b))
            })
            .unwrap();
        let expected = layout.y_of_frequency(1000.0);
        assert!(
            brightest.abs_diff(expected) <= 2,
            "row {brightest}, 1 kHz at {expected}"
        );

        // Bright, and not one of the reference line's lone white dots: its
        // neighbours are bright too.
        let lit = |x: usize| luminance(&pixels, width, x, brightest) > 200.0;
        let bright: Vec<usize> = (layout.left + 1..layout.right() - 1)
            .filter(|x| lit(*x) && lit(x - 1) && lit(x + 1))
            .collect();
        let (first, last) = (bright[0], bright[bright.len() - 1]);
        let tolerance = (0.0425 / 6.0 * layout.plot_width() as f64).ceil() as usize + 1;
        assert!(
            first.abs_diff(layout.x_of(2.0)) <= tolerance,
            "starts at {first}, 2 s at {}",
            layout.x_of(2.0)
        );
        assert!(
            last.abs_diff(layout.x_of(4.0)) <= tolerance,
            "ends at {last}, 4 s at {}",
            layout.x_of(4.0)
        );
        // The ticks are drawn there, under the spectrogram.
        for time in [2.0, 4.0] {
            let tick = layout.x_of(time);
            assert_eq!(luminance(&pixels, width, tick, panel.bottom() + 3), 0.0);
        }
    }

    #[test]
    fn long_headings_wrap_at_spaces() {
        assert_eq!(wrap("one two three", 8), ["one two", "three"]);
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("short", 80), ["short"]);
    }

    #[test]
    fn time_steps_are_round() {
        assert_eq!(time_step(60.0), 10.0);
        assert_eq!(time_step(6.0), 1.0);
        assert_eq!(time_step(14.0), 2.0);
        assert_eq!(time_step(3.0), 0.5);
    }
}
