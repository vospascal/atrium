//! The one-screen text of a cut: what was found where, every decision with
//! the numbers behind it, made for a reader who cannot listen and has only
//! this and the picture.

use std::fmt::Write;

use super::record::{CriterionDecision, Cut, DropReason};
use crate::statistics::standard_deviation;

/// Labels are this wide, so every row's values start in the same column.
const LABEL: usize = 12;
/// The similarity matrix is printed for up to this many takes; beyond, it is
/// in cut.json and the picture.
const MATRIX_TAKES: usize = 24;

/// The summary of `cut`, under `heading` (the recording and the stretch cut).
pub fn cut_summary(heading: &str, cut: &Cut) -> String {
    let mut text = String::new();
    let line = |text: &mut String, label: &str, body: String| {
        let _ = writeln!(text, "{label:<LABEL$}{body}");
    };
    let thresholds = &cut.thresholds;
    let _ = writeln!(text, "== cut of {heading} ==");
    line(
        &mut text,
        "file",
        format!(
            "{} ch, {} Hz, {:.1} s; levels in dB re full scale, of the channels' mean in the band",
            cut.channels, cut.sample_rate, cut.seconds
        ),
    );

    let band = &cut.band;
    let ranges: Vec<String> = band
        .ranges
        .iter()
        .map(|[low, high]| format!("{}-{} Hz", hertz(*low), hertz(*high)))
        .collect();
    let how = if band.given {
        "given by --band".to_owned()
    } else {
        let largest = band
            .bands
            .iter()
            .max_by(|a, b| a.l1_minus_l90.total_cmp(&b.l1_minus_l90));
        let chosen: Vec<String> = band
            .bands
            .iter()
            .filter(|band| band.chosen)
            .map(|band| hertz(band.nominal_hertz))
            .collect();
        match largest {
            Some(largest) => format!(
                "third octaves {} (L1-L90 within {:.0} dB of the largest, {:.1} dB at {} Hz)",
                chosen.join(" "),
                thresholds.band_within_decibels,
                largest.l1_minus_l90,
                hertz(largest.nominal_hertz)
            ),
            None => "no third octave below Nyquist".to_owned(),
        }
    };
    line(
        &mut text,
        "band",
        format!(
            "{}: {how}; a reading every 1 ms averages {:.1} ms",
            ranges.join(" + "),
            1000.0 * band.frame_seconds
        ),
    );
    if !band.given && !band.bands.is_empty() {
        let spreads: Vec<String> = band
            .bands
            .iter()
            .map(|band| {
                format!(
                    "{}{}:{:.0}",
                    if band.chosen { "*" } else { "" },
                    hertz(band.nominal_hertz),
                    band.l1_minus_l90
                )
            })
            .collect();
        for chunk in spreads.chunks(12) {
            line(&mut text, "", format!("L1-L90 dB {}", chunk.join(" ")));
        }
    }
    let (bed_low, bed_high) = cut
        .level
        .bed_decibels
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), value| {
            (low.min(*value), high.max(*value))
        });
    line(
        &mut text,
        "bed",
        format!(
            "{:.1} to {:.1} dB: the {:.0}th percentile over the {:.0} s before or after each moment, whichever the level there reaches",
            bed_low, bed_high, thresholds.bed_percentile, thresholds.bed_window_seconds
        ),
    );
    line(
        &mut text,
        "events",
        format!(
            "{} (start {:.0} dB over the bed, end below {:.0} dB over it)",
            cut.events.len(),
            thresholds.start_above_bed_decibels,
            thresholds.end_above_bed_decibels
        ),
    );

    let criterion = &cut.criterion;
    let fit = match (criterion.one, criterion.two) {
        (Some(one), Some(two)) => format!(
            "one process {:.2}/s (BIC {:.1}); two: fast {:.2}/s ({:.0} % of gaps, mean {}), slow {:.3}/s (mean {}) (BIC {:.1}, {} peak{} in log gap)",
            one.rate,
            one.bic,
            two.fast.rate,
            100.0 * two.fast.share,
            seconds(1.0 / two.fast.rate),
            two.slow.rate,
            seconds(1.0 / two.slow.rate),
            two.bic,
            if two.two_peaks { "two" } else { "one" },
            if two.two_peaks { "s" } else { "" },
        ),
        _ => String::new(),
    };
    let verdict = match (criterion.decision, criterion.criterion_seconds) {
        (CriterionDecision::TwoProcesses, Some(limit)) => {
            format!("{}: gaps shorter join events into one take", seconds(limit))
        }
        (CriterionDecision::TooFewGaps, _) => format!(
            "none: {} gaps, too few to fit two processes; nothing joined",
            criterion.gap_seconds.len()
        ),
        (CriterionDecision::OnePeak, _) => {
            "none: two processes fit better, but the log gaps have one peak; nothing joined"
                .to_owned()
        }
        _ => "none: one process fits as well as two; nothing joined".to_owned(),
    };
    line(&mut text, "criterion", verdict);
    if !fit.is_empty() {
        line(
            &mut text,
            "",
            format!(
                "{} gaps between events (Sibly, Nott & Fletcher 1990): {fit}",
                criterion.gap_seconds.len()
            ),
        );
    }

    let count_of = |reason: DropReason| {
        cut.dropped
            .iter()
            .filter(|dropped| dropped.reason == reason)
            .count()
    };
    let flagged = cut.takes.iter().filter(|take| take.flagged).count();
    line(
        &mut text,
        "takes",
        format!(
            "{} kept ({flagged} flagged: SNR under {:.0} dB); dropped {} shorter than {:.0} ms, {} cut off at the start, {} at the end, {} clipping",
            cut.takes.len(),
            thresholds.flag_below_snr_decibels,
            count_of(DropReason::Shorter),
            1000.0 * thresholds.shortest_take_seconds,
            count_of(DropReason::CutOffAtStart),
            count_of(DropReason::CutOffAtEnd),
            count_of(DropReason::Clips),
        ),
    );
    if !cut.takes.is_empty() {
        line(
            &mut text,
            "",
            "  #  name   start s    end s   dur s  events  level dB  re loudest  SNR dB  centroid Hz"
                .to_owned(),
        );
        for take in &cut.takes {
            line(
                &mut text,
                "",
                format!(
                    "{:>3}  {:<5} {:>7.2}  {:>7.2}  {:>6.2}  {:>6}  {:>8.1}  {:>10.1}  {:>6.1}  {:>11.0}{}",
                    take.number,
                    take.name,
                    take.start_seconds,
                    take.end_seconds,
                    take.duration_seconds,
                    take.events,
                    take.level_decibels,
                    take.relative_to_loudest_decibels,
                    take.snr_decibels,
                    take.centroid_hertz,
                    if take.flagged { "  flagged" } else { "" }
                ),
            );
        }
        let levels: Vec<f64> = cut.takes.iter().map(|take| take.level_decibels).collect();
        line(
            &mut text,
            "level",
            format!(
                "takes spread SD {:.1} dB; not written as level_spread, the takes keep their measured levels",
                standard_deviation(&levels)
            ),
        );
    }

    let gaps = &cut.gaps;
    line(
        &mut text,
        "gaps",
        match (gaps.p10, gaps.median, gaps.p90) {
            (Some(p10), Some(median), Some(p90)) => format!(
                "{} between takes, end to start: 10 % {}, median {}, 90 % {}",
                gaps.seconds.len(),
                seconds(p10),
                seconds(median),
                seconds(p90)
            ),
            _ => "none: fewer than two takes".to_owned(),
        },
    );

    let types = &cut.types;
    let groups: Vec<String> = types
        .groups
        .iter()
        .map(|group| format!("{} {}", group.name, group.takes.len()))
        .collect();
    line(
        &mut text,
        "types",
        format!(
            "{} at average similarity {:.2} or more (provisional, from synthetic songs): {}",
            types.groups.len(),
            thresholds.type_similarity,
            groups.join(", ")
        ),
    );
    let bouts = &cut.bouts;
    if let (Some(shortest), Some(longest)) = (bouts.shortest, bouts.longest) {
        let runs: Vec<String> = bouts
            .runs
            .iter()
            .map(|run| format!("{}x{}", run.type_name, run.takes))
            .collect();
        line(
            &mut text,
            "bouts",
            format!(
                "runs of one type {shortest} to {longest}: {}",
                runs.join(" ")
            ),
        );
    }
    let count = types.similarity.len();
    if count > 1 && count <= MATRIX_TAKES {
        line(
            &mut text,
            "similarity",
            "largest log-mel cross-correlation, takes by number".to_owned(),
        );
        let header: String = (1..=count).map(|number| format!("{number:>5}")).collect();
        line(&mut text, "", format!("   {header}"));
        for (index, row) in types.similarity.iter().enumerate() {
            let values: String = row.iter().map(|value| format!("{value:>5.2}")).collect();
            line(&mut text, "", format!("{:>3}{values}", index + 1));
        }
    } else if count > MATRIX_TAKES {
        line(
            &mut text,
            "similarity",
            format!("{count} takes: the matrix is in cut.json and the picture"),
        );
    }
    text
}

/// 80, 1k25, 2k: as the analyser labels frequencies.
fn hertz(value: f64) -> String {
    if value >= 1000.0 {
        let thousands = value / 1000.0;
        let text = format!("{thousands:.2}");
        let trimmed = text.trim_end_matches('0').trim_end_matches('.');
        trimmed.replacen('.', "k", 1) + if trimmed.contains('.') { "" } else { "k" }
    } else {
        format!("{value:.0}")
    }
}

/// 0.42 s, 3.1 s, 12 s.
fn seconds(value: f64) -> String {
    if value < 1.0 {
        format!("{value:.2} s")
    } else if value < 10.0 {
        format!("{value:.1} s")
    } else {
        format!("{value:.0} s")
    }
}
