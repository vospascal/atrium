//! The one-screen text: every number with its unit and what it is, one line
//! per group, then the file over time in twenty steps. Made for a reader who
//! cannot listen and has only this and the picture, so nothing is left to
//! be inferred from position alone; and the layout is fixed, so the
//! summaries of several files given one after another line up row for row.

use std::fmt::Write;

use crate::report::{Report, Subband};

/// Labels are this wide, so every row's values start in the same column.
const LABEL: usize = 12;

/// The subbands the texture rows show: low, low-mid, presence, air.
const SHOWN_SUBBANDS: [f64; 4] = [125.0, 500.0, 2000.0, 8000.0];

/// The summary of `report`, under `heading` (the file and how it was made).
pub fn summary(heading: &str, report: &Report) -> String {
    let mut text = String::new();
    let unit = report.unit.symbol();
    let facts = &report.signal;
    let level = &report.level.overall;
    let line = |text: &mut String, label: &str, body: String| {
        let _ = writeln!(text, "{label:<LABEL$}{body}");
    };

    let _ = writeln!(text, "== {heading} ==");
    let calibration = match facts.full_scale_db_spl {
        Some(full_scale) => format!("levels in dB SPL (RMS 1.0 = {full_scale:.1} dB SPL)"),
        None => "levels in dBFS (RMS 1.0 = 0 dBFS; a full-scale sine is -3.0 dBFS)".to_owned(),
    };
    let combination = match facts.combination.as_str() {
        "ears_mean" => "overall = mean of the two ears' powers",
        "room_sum" => "overall = sum of the speakers' powers",
        _ => "overall = the one channel",
    };
    line(
        &mut text,
        "file",
        format!(
            "{} ch ({}), {} Hz, {:.1} s; {calibration}; {combination}",
            facts.channel_names.len(),
            facts.channel_names.join(" "),
            facts.sample_rate,
            facts.seconds
        ),
    );

    line(
        &mut text,
        "level",
        format!(
            "LAeq {:.1} {unit} | LCeq {:.1} {unit} (LC-LA {:+.1} dB, low end) | Leq {:.1} {unit} | while sounding {:.1} {unit}",
            level.laeq,
            level.lceq,
            level.lceq - level.laeq,
            level.leq,
            level.while_sounding
        ),
    );
    line(
        &mut text,
        "",
        format!(
            "LAFmax {:.1} {unit} (over while sounding {:+.1} dB) | peak {:.1} {unit}, crest {:.1} dB",
            level.lafmax,
            level.lafmax - level.while_sounding,
            level.peak,
            level.crest_decibels
        ),
    );
    line(
        &mut text,
        "",
        format!(
            "L90 {:.1} {unit} (the bed) | L50 {:.1} {unit} | L10 {:.1} {unit} | L10-L90 {:.1} dB (how eventful)",
            level.l90,
            level.l50,
            level.l10,
            level.l10 - level.l90
        ),
    );
    if report.level.channels.len() > 1 {
        let channels: Vec<String> = report
            .level
            .channels
            .iter()
            .map(|channel| format!("{} {:.1}", channel.channel, channel.laeq))
            .collect();
        line(
            &mut text,
            "channels",
            format!("LAeq ({unit}): {}", channels.join(", ")),
        );
    }

    let colour = &report.colour;
    line(
        &mut text,
        "colour",
        format!(
            "centroid {:.0} Hz; third-octave levels relative to Leq (dB):",
            colour.centroid_hertz
        ),
    );
    for half in colour.third_octaves.chunks(15) {
        let mut frequencies = format!("{:>LABEL$}", "Hz");
        let mut values = format!("{:>LABEL$}", "dB re Leq");
        for band in half {
            let _ = write!(frequencies, "{:>5}", frequency_label(band.centre_hertz));
            let _ = write!(values, "{:>5.0}", band.relative_decibels.max(-99.0));
        }
        let _ = writeln!(text, "{frequencies}\n{values}");
    }

    write_texture(&mut text, report);

    let events = &report.events;
    let seconds_or_none =
        |value: Option<f64>| value.map_or("none".to_owned(), |seconds| format!("{seconds:.2} s"));
    line(
        &mut text,
        "events",
        format!(
            "onsets {:.1} /min ({} in {:.1} s); interval between onsets: median {}, 10 % {}, 90 % {}",
            events.onsets_per_minute,
            events.onset_seconds.len(),
            facts.seconds,
            seconds_or_none(events.interval_median_seconds),
            seconds_or_none(events.interval_p10_seconds),
            seconds_or_none(events.interval_p90_seconds)
        ),
    );

    write_space(&mut text, report);
    write_strip(&mut text, report);
    text
}

fn write_texture(text: &mut String, report: &Report) {
    let texture = &report.texture;
    let _ = writeln!(
        text,
        "{:<LABEL$}32 cochlear subbands, envelopes compressed ^0.3 at 400 Hz (McDermott & Simoncelli 2011)",
        "texture"
    );
    let shown: Vec<&Subband> = SHOWN_SUBBANDS
        .iter()
        .filter_map(|wanted| {
            texture.subbands.iter().min_by(|a, b| {
                (a.centre_hertz / wanted)
                    .ln()
                    .abs()
                    .total_cmp(&(b.centre_hertz / wanted).ln().abs())
            })
        })
        .collect();
    let mut header = format!("{:>LABEL$}", "subband Hz");
    let mut variation = format!("{:>LABEL$}", "M2 var/mean2");
    let mut skew = format!("{:>LABEL$}", "M3 skewness");
    let mut peaky = format!("{:>LABEL$}", "M4 kurtosis");
    for subband in &shown {
        let _ = write!(header, "{:>8}", frequency_label(subband.centre_hertz));
        let _ = write!(variation, "{:>8.3}", subband.variance_over_mean_squared);
        let _ = write!(skew, "{:>8.2}", subband.skewness);
        let _ = write!(peaky, "{:>8.2}", subband.kurtosis);
    }
    let _ = writeln!(
        text,
        "{header}   (ratios; M4 of a Gaussian is 3)\n{variation}\n{skew}\n{peaky}"
    );

    let filters = texture.modulation_centres_hertz.len();
    let sounding: Vec<&Subband> = texture
        .subbands
        .iter()
        .filter(|subband| subband.mean > 0.0)
        .collect();
    let mut rates = format!("{:>LABEL$}", "mod. Hz");
    let mut powers = format!("{:>LABEL$}", "% of var.");
    for filter in 0..filters {
        let average = if sounding.is_empty() {
            0.0
        } else {
            sounding
                .iter()
                .map(|subband| subband.modulation_power[filter])
                .sum::<f64>()
                / sounding.len() as f64
        };
        let _ = write!(
            rates,
            "{:>5}",
            rate_label(texture.modulation_centres_hertz[filter])
        );
        let _ = write!(powers, "{:>5.1}", 100.0 * average);
    }
    let _ = writeln!(
        text,
        "{:>LABEL$}modulation power per filter (Q 2), mean over subbands, % of the envelope's variance:\n{rates}\n{powers}",
        ""
    );

    let mean_of = |values: &[f64]| {
        if values.is_empty() {
            0.0
        } else {
            values.iter().sum::<f64>() / values.len() as f64
        }
    };
    let correlation_at = |offset: usize| {
        texture
            .correlations
            .iter()
            .find(|correlation| correlation.offset == offset)
            .map_or(0.0, |correlation| mean_of(&correlation.values))
    };
    let c1: Vec<String> = texture
        .c1
        .iter()
        .map(|band| {
            format!(
                "{} Hz {:.2}",
                rate_label(band.modulation_centre_hertz),
                mean_of(&band.neighbours)
            )
        })
        .collect();
    let _ = writeln!(
        text,
        "{:>LABEL$}C, envelope correlation, mean: subbands 1 apart {:.2}, 5 apart {:.2} (1 = move together)\n{:>LABEL$}C1, neighbours' modulation bands, mean: {}",
        "",
        correlation_at(1),
        correlation_at(5),
        "",
        c1.join(", ")
    );
}

fn write_space(text: &mut String, report: &Report) {
    let Some(space) = &report.space else {
        let _ = writeln!(text, "{:<LABEL$}none: one channel", "space");
        return;
    };
    if let Some(pair) = &space.pair {
        let number = |value: Option<f64>, digits: usize| {
            value.map_or("-".to_owned(), |value| format!("{value:.digits$}"))
        };
        let _ = writeln!(
            text,
            "{:<LABEL$}{}/{}: width {} (1 - IACC, 500 Hz-2 kHz; 0 = one source, 1 = diffuse); ITD median {} ms (left leads +), IQR {} ms, over {} of {} frames",
            "space",
            pair.left,
            pair.right,
            number(pair.width, 2),
            number(pair.itd_median_milliseconds, 2),
            number(pair.itd_interquartile_milliseconds, 2),
            pair.itd_frames,
            pair.frames
        );
        let mut header = format!("{:>LABEL$}", "octave Hz");
        let mut iacc = format!("{:>LABEL$}", "IACC");
        let mut frames = format!("{:>LABEL$}", "IACC frames");
        let mut coherence = format!("{:>LABEL$}", "env. coher.");
        let mut ild = format!("{:>LABEL$}", "ILD R-L dB");
        let mut spread = format!("{:>LABEL$}", "ILD sd dB");
        for band in &pair.octave_bands {
            let _ = write!(header, "{:>7}", frequency_label(band.centre_hertz));
            let _ = write!(iacc, "{:>7}", number(band.iacc, 2));
            let _ = write!(frames, "{:>7}", number(band.iacc_median_of_frames, 2));
            let _ = write!(coherence, "{:>7}", number(band.envelope_coherence, 2));
            let _ = write!(ild, "{:>7}", number(band.ild_mean_decibels, 1));
            let _ = write!(
                spread,
                "{:>7}",
                number(band.ild_standard_deviation_decibels, 1)
            );
        }
        let _ = writeln!(
            text,
            "{header}   (IACC: whole file; frames: median of 100 ms;\n{iacc}    env. coher.: do the ears' band levels\n{frames}    move together, 1 = yes)\n{coherence}\n{ild}\n{spread}"
        );
    }
    if let Some(distribution) = &space.distribution {
        let unit = report.unit.symbol();
        let _ = writeln!(
            text,
            "{:<LABEL$}front (L C R) {:.1} {unit}, surround (Ls Rs) {:.1} {unit}, front - surround {:+.1} dB",
            "room",
            distribution.front,
            distribution.surround,
            distribution.front_minus_surround_decibels
        );
    }
}

fn write_strip(text: &mut String, report: &Report) {
    let strip = &report.over_time.strip;
    let step = strip
        .first()
        .map_or(0.0, |segment| segment.end_seconds - segment.start_seconds);
    let _ = writeln!(
        text,
        "{:<LABEL$}{} steps of {step:.1} s",
        "over time",
        strip.len()
    );
    let mut starts = format!("{:>LABEL$}", "start s");
    let mut levels = format!("{:>LABEL$}", "LAeq");
    let mut onsets = format!("{:>LABEL$}", "onsets");
    let mut widths = format!("{:>LABEL$}", "width");
    // Whole seconds when the steps are; tenths for a short file.
    let digits = if step >= 2.0 { 0 } else { 1 };
    for segment in strip {
        let _ = write!(starts, "{:>5.digits$}", segment.start_seconds);
        let _ = write!(levels, "{:>5.0}", segment.laeq.max(-999.0));
        let _ = write!(onsets, "{:>5}", segment.onsets);
        let _ = write!(
            widths,
            "{:>5}",
            segment
                .width
                .map_or("-".to_owned(), |width| format!("{width:.2}"))
        );
    }
    let _ = writeln!(
        text,
        "{starts}\n{levels}   {}\n{onsets}   per step",
        report.unit.symbol()
    );
    if report
        .space
        .as_ref()
        .is_some_and(|space| space.pair.is_some())
    {
        let _ = writeln!(text, "{widths}   1 - IACC");
    }
}

/// 25, 1k, 1k25, 12k5: a nominal frequency in four characters.
fn frequency_label(hertz: f64) -> String {
    if hertz < 1000.0 {
        return format!("{hertz:.0}");
    }
    let thousands = hertz / 1000.0;
    let whole = thousands.trunc();
    let rest = ((thousands - whole) * 100.0).round() as u32;
    match rest {
        0 => format!("{whole:.0}k"),
        rest if rest % 10 == 0 => format!("{whole:.0}k{}", rest / 10),
        rest => format!("{whole:.0}k{rest:02}"),
    }
}

/// 0.5, 4.5, 12, 200: a modulation rate in at most four characters.
fn rate_label(hertz: f64) -> String {
    if hertz < 10.0 {
        format!("{hertz:.1}")
    } else {
        format!("{hertz:.0}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::{Settings, Signal};
    use crate::test_signals::{interleave, Noise, RATE};

    #[test]
    fn frequencies_are_named_in_four_characters() {
        assert_eq!(frequency_label(31.5), "32");
        assert_eq!(frequency_label(1250.0), "1k25");
        assert_eq!(frequency_label(1600.0), "1k6");
        assert_eq!(frequency_label(12_500.0), "12k5");
        assert_eq!(frequency_label(20_000.0), "20k");
        // The cochlear centres are not nominal: 8844 Hz.
        assert_eq!(frequency_label(8844.0), "8k84");
    }

    /// Two files of different lengths and channels give summaries whose
    /// rows line up: the same labels in the same order, each value row as
    /// long as its counterpart.
    #[test]
    fn summaries_line_up() {
        let mut noise = Noise::new(21);
        let mono = noise.white(0.1, 4.0);
        let left = noise.white(0.1, 6.0);
        let right = noise.white(0.05, 6.0);
        let stereo = interleave(&[&left, &right]);
        let analyse = |samples: &[f32], channels: usize| {
            crate::analyse(
                &Signal {
                    interleaved: samples,
                    channels,
                    sample_rate: RATE,
                    channel_names: Vec::new(),
                },
                &Settings::default(),
            )
        };
        let first = summary("mono", &analyse(&mono, 1));
        let second = summary("stereo", &analyse(&stereo, 2));
        let rows = |text: &str, label: &str| {
            text.lines()
                .find(|line| line.trim_start().starts_with(label))
                .map(str::len)
        };
        for label in [
            "Hz",
            "dB re Leq",
            "M2 var/mean2",
            "% of var.",
            "start s",
            "LAeq",
            "onsets",
        ] {
            assert_eq!(rows(&first, label), rows(&second, label), "{label}");
            assert!(rows(&first, label).is_some(), "{label}");
        }
        assert!(second.contains("IACC"));
        assert!(first.contains("none: one channel"));
        assert!(first.lines().count() <= 45, "one screen:\n{first}");
        assert!(second.lines().count() <= 45, "one screen:\n{second}");
    }
}
