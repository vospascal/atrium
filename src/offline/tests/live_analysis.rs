//! A render measured live, as the app measures the playing scene, against
//! the same render analysed whole: the two cannot disagree beyond what the
//! live analyser's causality costs (see `crates/analysis/README.md`).

use atrium_analysis::{analyse, Column, Live, Report, Settings, Signal};
use atrium_core::probe::Probe;

use super::{on_headphones, render, MEADOW};
use crate::offline::{RenderSettings, Rendered};

/// The render, pushed to `Live` `chunk` frames at a time.
fn live(rendered: &Rendered, chunk: usize) -> Vec<Column> {
    let channels = rendered.channels();
    let mut live = Live::new(
        rendered.sample_rate(),
        channels,
        rendered.full_scale_db_spl(),
    );
    let mut columns = Vec::new();
    for (index, block) in rendered.frames().chunks(chunk * channels).enumerate() {
        live.push(block, (index * chunk) as u64);
        columns.extend(live.take_columns());
    }
    columns
}

/// The render analysed whole, as `analyse` reads it through its sidecar.
fn report(rendered: &Rendered) -> Report {
    analyse(
        &Signal {
            interleaved: rendered.frames(),
            channels: rendered.channels(),
            sample_rate: rendered.sample_rate(),
            channel_names: Vec::new(),
        },
        &Settings {
            full_scale_db_spl: Some(rendered.full_scale_db_spl()),
        },
    )
}

/// The LAeq over `columns`: the energy mean of their overall LAeq.
fn energy_mean(columns: &[Column]) -> f64 {
    let mean = columns
        .iter()
        .map(|column| 10f64.powf(column.leq_overall_decibels / 10.0))
        .sum::<f64>()
        / columns.len() as f64;
    10.0 * mean.log10()
}

/// The meadow on headphones, a minute: birds through the wind. Pushed in
/// device-sized chunks (512) and ragged ones (37) the columns are the same.
/// Every 100 ms frame's width within 0.03 and ILD within 0.3 dB of the
/// report's; each 3 s of the report's strip within 0.2 dB of the columns'
/// overall LAeq over the same 3 s, and the minute's within 0.1 dB of the
/// report's overall LAeq; onsets within one of the report's in the minute.
#[test]
fn a_render_live_reads_as_it_reads_whole() {
    let rendered = render(MEADOW, &on_headphones(60.0));
    let columns = live(&rendered, 512);
    assert_eq!(columns, live(&rendered, 37), "the chunks changed a column");
    let report = report(&rendered);
    let over_time = &report.over_time;

    let mut compared = 0;
    for (frame, (width, ild)) in over_time
        .width
        .iter()
        .zip(&over_time.ild_decibels)
        .enumerate()
    {
        let Some(column) = columns.get(2 * frame + 1) else {
            break;
        };
        match (width, column.width) {
            (Some(whole), Some(live)) => assert!(
                (whole - live).abs() <= 0.03,
                "frame {frame}: width {live} live, {whole} whole"
            ),
            (whole, live) => assert_eq!(whole.is_some(), live.is_some(), "frame {frame}"),
        }
        match (ild, column.ild_decibels) {
            (Some(whole), Some(live)) => assert!(
                (whole - live).abs() <= 0.3,
                "frame {frame}: ILD {live} live, {whole} whole"
            ),
            (whole, live) => assert_eq!(whole.is_some(), live.is_some(), "frame {frame}"),
        }
        compared += 1;
    }
    assert!(compared >= 595, "{compared} frames compared");

    // 3 s segments are 60 columns; the last is not all there live.
    for (index, segment) in over_time.strip.iter().enumerate().take(19) {
        let live = energy_mean(&columns[index * 60..(index + 1) * 60]);
        assert!(
            (live - segment.laeq).abs() <= 0.2,
            "{} s: LAeq {live:.2} live, {:.2} whole",
            segment.start_seconds,
            segment.laeq
        );
    }

    // The minute's overall LAeq, from the columns' own overall level.
    let minute = energy_mean(&columns);
    assert!(
        (minute - report.level.overall.laeq).abs() <= 0.1,
        "LAeq {minute:.2} live, {:.2} whole",
        report.level.overall.laeq
    );

    let covered = columns.len() as f64 * Live::COLUMN_SECONDS;
    let whole = report
        .events
        .onset_seconds
        .iter()
        .filter(|time| **time < covered)
        .count() as f64;
    let live = columns.iter().filter(|column| column.onset).count() as f64;
    // A minute of columns: one onset either way is one a minute.
    assert!(
        (live - whole).abs() <= 1.0,
        "onsets {live} live, {whole} whole, in {covered:.2} s"
    );
}

/// The meadow's wind alone (`--probe ambient`), live: the columns' energy
/// mean is the report's LAeq, 40.8 dB SPL, within 0.1 dB, and 40.8 ± 0.5
/// dB SPL absolutely. (Its unweighted Leq is 41.5; the columns are
/// A-weighted, as the report's LAeq.)
#[test]
fn the_wind_alone_reads_its_level_live() {
    let rendered = render(
        MEADOW,
        &RenderSettings {
            probe: Probe {
                direct: false,
                reflections: false,
                reverb: false,
                ambient: true,
                ..Probe::WHOLE
            },
            ..on_headphones(60.0)
        },
    );
    let columns = live(&rendered, 512);
    let live = energy_mean(&columns);
    let whole = report(&rendered).level.overall.laeq;
    assert!(
        (live - whole).abs() <= 0.1,
        "{live:.2} live, {whole:.2} whole"
    );
    assert!((live - 40.8).abs() <= 0.5, "{live:.2} dB SPL");
}
