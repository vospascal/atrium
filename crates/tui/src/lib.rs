use std::io::{self, Write};

use crossterm::style::{self, Stylize};
use crossterm::{cursor, execute, terminal};

/// Static device/scene info collected at startup.
pub struct DeviceInfo {
    pub device_name: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub render_mode: String,
    pub scene_path: String,
    pub source_names: Vec<String>,
    /// Pipeline mix stage names (for pipeline display).
    pub pipeline_post: Vec<String>,
    /// Labels for each output channel (e.g. ["FL", "FR", "C", "LFE", "RL", "RR"]).
    pub channel_labels: Vec<String>,
}

/// Per-source live status from telemetry.
#[derive(Clone, Debug)]
pub struct SourceStatus {
    pub distance: f32,
    pub gain_db: f32,
    pub is_muted: bool,
    /// Current render mode name (changes at runtime).
    pub render_mode: String,
}

/// Per-channel peak level from telemetry.
#[derive(Clone, Debug)]
pub struct ChannelStatus {
    /// Peak amplitude in dBFS.
    pub peak_db: f32,
}

/// Terminal dashboard that updates in place.
pub struct Dashboard {
    info: DeviceInfo,
    /// Number of lines we printed last frame (so we know how far to move up).
    lines_printed: usize,
}

impl Dashboard {
    pub fn new(info: DeviceInfo) -> Self {
        Self {
            info,
            lines_printed: 0,
        }
    }

    /// Build the full pipeline stage list: render mode + mix stages.
    fn pipeline_stages<'a>(&'a self, current_mode: &'a str) -> Vec<&'a str> {
        let mut stages: Vec<&str> = vec![current_mode];
        stages.extend(self.info.pipeline_post.iter().map(|s| s.as_str()));
        stages
    }

    /// Render one frame of the dashboard. Call at ~15 Hz from the telemetry loop.
    pub fn update(&mut self, sources: &[SourceStatus], channels: &[ChannelStatus]) {
        let mut out = io::stdout();

        // Move cursor up to overwrite previous frame
        if self.lines_printed > 0 {
            let _ = execute!(out, cursor::MoveUp(self.lines_printed as u16));
        }

        let mut lines = 0;

        // Determine current render mode from first source (all share the same mode)
        let current_mode = sources
            .first()
            .map(|s| s.render_mode.as_str())
            .unwrap_or(self.info.render_mode.as_str());

        // Header line
        Self::clear_line(&mut out);
        writeln!(
            out,
            " {} \u{2502} {}Hz/{}ch \u{2502} {}",
            self.info.device_name, self.info.sample_rate, self.info.channels, current_mode,
        )
        .ok();
        lines += 1;

        // Pipeline box
        let stages = self.pipeline_stages(current_mode);
        let max_len = stages.iter().map(|s| s.len()).max().unwrap_or(0);
        let box_width = max_len + 2; // 1 padding each side

        // Top border
        Self::clear_line(&mut out);
        writeln!(out, " \u{250C}{}\u{2510}", "\u{2500}".repeat(box_width)).ok();
        lines += 1;

        for (i, stage) in stages.iter().enumerate() {
            // Stage name row
            Self::clear_line(&mut out);
            writeln!(out, " \u{2502} {:<width$} \u{2502}", stage, width = max_len).ok();
            lines += 1;

            // Arrow row (skip after last stage)
            if i < stages.len() - 1 {
                Self::clear_line(&mut out);
                let pad_left = max_len / 2;
                let rest = max_len.saturating_sub(pad_left + 1);
                writeln!(
                    out,
                    " \u{2502} {:>pad$}\u{25BC}{:<rest$} \u{2502}",
                    "",
                    "",
                    pad = pad_left,
                    rest = rest,
                )
                .ok();
                lines += 1;
            }
        }

        // Bottom border
        Self::clear_line(&mut out);
        writeln!(out, " \u{2514}{}\u{2518}", "\u{2500}".repeat(box_width)).ok();
        lines += 1;

        // Channel output meters
        let bar_width = 40;
        Self::clear_line(&mut out);
        writeln!(out, " \u{2500}\u{2500} Channels \u{2500}\u{2500}").ok();
        lines += 1;

        for (i, ch) in channels.iter().enumerate() {
            let label = self
                .info
                .channel_labels
                .get(i)
                .map(|s| s.as_str())
                .unwrap_or("?");
            let bar = meter_bar(ch.peak_db, bar_width);
            Self::clear_line(&mut out);
            writeln!(out, " {:<4} {} {:>6.1} dB", label, bar, ch.peak_db).ok();
            lines += 1;
        }

        // Source summary (one line)
        if !sources.is_empty() {
            Self::clear_line(&mut out);
            let active = sources.iter().filter(|s| !s.is_muted).count();
            writeln!(out, " {active}/{} sources active", sources.len()).ok();
            lines += 1;
        }

        let _ = out.flush();
        self.lines_printed = lines;
    }

    fn clear_line(out: &mut io::Stdout) {
        let _ = execute!(out, terminal::Clear(terminal::ClearType::CurrentLine));
        let _ = execute!(out, cursor::MoveToColumn(0));
    }
}

/// Render a horizontal meter bar from dB value with ANSI colors.
/// `width` is the number of character cells for the bar.
///
/// Maps -60 dB → empty, 0 dB → full.
/// Filled portion is bright green, empty track is dark gray — like Claude's usage bars.
fn meter_bar(db: f32, width: usize) -> String {
    let clamped = db.clamp(-60.0, 0.0);
    let fraction = (clamped + 60.0) / 60.0;
    let filled = (fraction * width as f32).round() as usize;
    let empty = width.saturating_sub(filled);

    // Pick color based on level: green → yellow → red
    let fill_color = if db > -6.0 {
        style::Color::Red
    } else if db > -20.0 {
        style::Color::Yellow
    } else {
        style::Color::Green
    };

    let track_color = style::Color::Rgb {
        r: 60,
        g: 60,
        b: 60,
    };

    format!(
        "{}{}",
        "\u{2588}".repeat(filled).with(fill_color),
        "\u{2588}".repeat(empty).with(track_color),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Count visible █ characters (ignoring ANSI escape sequences).
    fn count_blocks(s: &str) -> usize {
        s.matches('\u{2588}').count()
    }

    #[test]
    fn meter_bar_full_at_0db() {
        let bar = meter_bar(0.0, 10);
        assert_eq!(count_blocks(&bar), 10);
    }

    #[test]
    fn meter_bar_empty_at_neg60() {
        // All 10 blocks are track (dark gray), none filled
        let bar = meter_bar(-60.0, 10);
        assert_eq!(count_blocks(&bar), 10); // all track blocks
    }

    #[test]
    fn meter_bar_half_at_neg30() {
        // -30 dB = 50% of range = 5 filled + 5 track = 10 total blocks
        let bar = meter_bar(-30.0, 10);
        assert_eq!(count_blocks(&bar), 10);
    }

    #[test]
    fn meter_bar_clamps_below_neg60() {
        let bar = meter_bar(-100.0, 10);
        assert_eq!(count_blocks(&bar), 10); // all track
    }

    #[test]
    fn meter_bar_clamps_above_0() {
        let bar = meter_bar(6.0, 10);
        assert_eq!(count_blocks(&bar), 10); // all filled
    }

    #[test]
    fn meter_bar_color_zones() {
        // Green zone (quiet) — ANSI green is \x1b[38;5;10m
        let bar = meter_bar(-40.0, 10);
        assert!(
            bar.contains("\x1b["),
            "bar should contain ANSI escape codes"
        );

        // Different colors for different levels
        let quiet = meter_bar(-40.0, 10);
        let loud = meter_bar(-3.0, 10);
        // They should produce different color codes
        assert_ne!(
            quiet, loud,
            "quiet and loud bars should have different colors"
        );
    }

    #[test]
    fn meter_bar_constant_block_count() {
        // Every bar should have exactly `width` █ characters (filled + track)
        for db in [-60, -45, -30, -15, 0] {
            let bar = meter_bar(db as f32, 20);
            assert_eq!(
                count_blocks(&bar),
                20,
                "bar at {db} dB should have 20 blocks"
            );
        }
    }
}
