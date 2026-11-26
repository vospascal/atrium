//! The one-screen comparison: the reference's value, then each other file's
//! value and its difference, one row per measure; the curves (third
//! octaves, modulation spectrum, correlations, LAF distribution) as rows of
//! the reference, each file and its difference. For a reader who cannot
//! listen: every number has its unit, every column its file.

use std::fmt::Write;

use super::comparison::{
    ClassDifference, Comparison, Difference, Differences, Place, TextureClass,
};
use super::file_colour;
use super::level::LEVEL_NOTICEABLE_DECIBELS;
use super::space::WIDTH_NOTICEABLE;
use super::texture::CONVERGED_DECIBELS;
use crate::summary::{frequency_label, rate_label};

/// The group's name, then the measure's, then the values.
const GROUP: usize = 12;
const MEASURE: usize = 28;
/// The reference's value, each file's value, each difference.
const VALUE: usize = 9;
const DIFFERENCE: usize = 9;
/// The name in front of a curve's row.
const ROW: usize = 12;
/// The matched partials shown per file (the JSON has them all).
const SHOWN_MATCHES: usize = 4;

/// The comparison as one screen of text.
pub fn comparison_summary(comparison: &Comparison) -> String {
    let mut text = String::new();
    let reference = &comparison.reference;
    let others = &comparison.others;
    let _ = writeln!(
        text,
        "== Comparison: {} (the reference) against {} ==",
        reference.name,
        others
            .iter()
            .map(|other| other.file.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let _ = writeln!(
        text,
        "{:<GROUP$}{} | {}",
        "ref (black)", reference.name, reference.heading
    );
    for (index, other) in others.iter().enumerate() {
        let _ = writeln!(
            text,
            "{:<GROUP$}{} | {}",
            format!("{} ({})", index + 1, file_colour(index + 1).0),
            other.file.name,
            other.file.heading
        );
    }
    let _ = writeln!(
        text,
        "{:<GROUP$}a difference is the file's value minus the reference's: + = more in the file",
        ""
    );

    let mut header = format!("{:<GROUP$}{:<MEASURE$}{:>VALUE$}", "", "", "ref");
    for index in 1..=others.len() {
        let _ = write!(
            header,
            "{:>VALUE$}{:>DIFFERENCE$}",
            index,
            format!("diff {index}")
        );
    }
    let _ = writeln!(text, "{header}");

    write_level(&mut text, comparison);
    write_colour(&mut text, comparison);
    write_texture(&mut text, comparison);
    write_events(&mut text, comparison);
    write_space(&mut text, comparison);
    write_movement(&mut text, comparison);
    write_over_time(&mut text, comparison);
    text
}

/// A file's row of a curve and the row of its differences.
type CurveRow = (Vec<Option<f64>>, Vec<Option<f64>>);

/// One measure: the reference's value, and each file's with its difference.
/// A trailing `*` marks a difference beyond its just-noticeable difference.
fn row(
    text: &mut String,
    group: &str,
    measure: &str,
    others: &[Differences],
    value: &dyn Fn(&Differences) -> Option<(Difference, bool)>,
    digits: usize,
) {
    let reference = others
        .iter()
        .find_map(value)
        .map_or("-".to_owned(), |(found, _)| {
            format!("{:.digits$}", found.reference)
        });
    let mut line = format!("{group:<GROUP$}{measure:<MEASURE$}{reference:>VALUE$}");
    for other in others {
        match value(other) {
            Some((found, noticeable)) => {
                let mark = if noticeable { "*" } else { "" };
                let _ = write!(
                    line,
                    "{:>VALUE$}{:>DIFFERENCE$}",
                    format!("{:.digits$}", found.other),
                    format!("{:+.digits$}{mark}", found.difference)
                );
            }
            None => {
                let _ = write!(line, "{:>VALUE$}{:>DIFFERENCE$}", "-", "-");
            }
        }
    }
    let _ = writeln!(text, "{}", line.trim_end());
}

/// A curve: the column headings, then the reference's row, then each file's
/// row and its difference row.
fn curve(
    text: &mut String,
    heading: &str,
    columns: &[String],
    width: usize,
    rows: &[CurveRow],
    reference: &[Option<f64>],
    digits: usize,
) {
    let cell = |value: Option<f64>, signed: bool| match value {
        Some(value) if signed => format!("{value:>+width$.digits$}"),
        Some(value) => format!("{value:>width$.digits$}"),
        None => format!("{:>width$}", "-"),
    };
    let mut line = format!("{heading:>ROW$}");
    for column in columns {
        let _ = write!(line, "{column:>width$}");
    }
    let _ = writeln!(text, "{line}");
    let mut line = format!("{:>ROW$}", "ref");
    for value in reference {
        line += &cell(*value, false);
    }
    let _ = writeln!(text, "{line}");
    for (index, (values, differences)) in rows.iter().enumerate() {
        let mut line = format!("{:>ROW$}", index + 1);
        for value in values {
            line += &cell(*value, false);
        }
        let _ = writeln!(text, "{line}");
        let mut line = format!("{:>ROW$}", format!("diff {}", index + 1));
        for value in differences {
            line += &cell(*value, true);
        }
        let _ = writeln!(text, "{line}");
    }
}

fn write_level(text: &mut String, comparison: &Comparison) {
    let others = &comparison.others;
    let shape = |value: fn(&Differences) -> Difference| {
        move |other: &Differences| Some((value(other), false))
    };
    row(
        text,
        "level shape",
        "L10-L90 (dB, how eventful)",
        others,
        &shape(|other| other.level.l10_minus_l90),
        1,
    );
    row(
        text,
        "",
        "LAFmax over sounding (dB)",
        others,
        &shape(|other| other.level.lafmax_over_while_sounding),
        1,
    );
    row(
        text,
        "",
        "crest, peak over Leq (dB)",
        others,
        &shape(|other| other.level.crest_decibels),
        1,
    );
    row(
        text,
        "",
        "LC-LA (dB, low end)",
        others,
        &shape(|other| other.level.lc_minus_la),
        1,
    );
    let uncalibrated: Vec<&str> = std::iter::once(&comparison.reference)
        .chain(others.iter().map(|other| &other.file))
        .filter(|file| file.unit == crate::report::LevelUnit::FullScale)
        .map(|file| file.name.as_str())
        .collect();
    if others.iter().any(|other| other.level.absolute.is_some()) {
        let absolute = |noticeable: fn(&crate::compare::AbsoluteLevels) -> (Difference, bool)| {
            move |other: &Differences| other.level.absolute.as_ref().map(noticeable)
        };
        row(
            text,
            "level",
            "LAeq (dB SPL)",
            others,
            &absolute(|absolute| (absolute.laeq, absolute.laeq_noticeable)),
            1,
        );
        row(
            text,
            "",
            "Leq (dB SPL)",
            others,
            &absolute(|absolute| (absolute.leq, absolute.leq_noticeable)),
            1,
        );
        let _ = writeln!(
            text,
            "{:<GROUP$}* beyond {LEVEL_NOTICEABLE_DECIBELS:.0} dB, the just-noticeable difference of level (ISO 3382-1)",
            ""
        );
    }
    if !uncalibrated.is_empty() {
        let _ = writeln!(
            text,
            "{:<GROUP$}absolute level not compared for {}: no calibration (dBFS says nothing of SPL)",
            if others.iter().any(|other| other.level.absolute.is_some()) {
                ""
            } else {
                "level"
            },
            uncalibrated.join(", ")
        );
    }
}

fn write_colour(text: &mut String, comparison: &Comparison) {
    let others = &comparison.others;
    row(
        text,
        "colour",
        "tilt 100 Hz-10 kHz (dB/oct)",
        others,
        &|other| Some((other.colour.tilt_decibels_per_octave, false)),
        2,
    );
    let mut line = format!(
        "{:<GROUP$}{:<MEASURE$}{:>VALUE$}",
        "",
        "centroid (Hz; ratio)",
        others.first().map_or("-".to_owned(), |other| format!(
            "{:.0}",
            other.colour.centroid_hertz.reference
        ))
    );
    for other in others {
        let centroid = other.colour.centroid_hertz;
        let _ = write!(
            line,
            "{:>VALUE$}{:>DIFFERENCE$}",
            format!("{:.0}", centroid.other),
            centroid
                .ratio
                .map_or("-".to_owned(), |ratio| format!("x{ratio:.2}"))
        );
    }
    let _ = writeln!(text, "{line}");
    let mut line = format!(
        "{:<GROUP$}{:<MEASURE$}{:>VALUE$}",
        "", "RMS of band diffs 50-16k (dB)", ""
    );
    for other in others {
        let _ = write!(
            line,
            "{:>VALUE$}{:>DIFFERENCE$}",
            "",
            format!("{:.1}", other.colour.rms_difference_decibels)
        );
    }
    let _ = writeln!(text, "{line}");
    let _ = writeln!(
        text,
        "{:<GROUP$}third octaves, dB relative to each file's own Leq:",
        ""
    );
    // The bands every file has, from the first other's (the reference's are
    // in each); split in two so a row stays readable.
    let Some(first) = others.first() else {
        return;
    };
    let centres: Vec<f64> = first
        .colour
        .bands
        .iter()
        .map(|band| band.centre_hertz)
        .collect();
    for half in centres.chunks(15) {
        let find = |other: &Differences, centre: f64| {
            other
                .colour
                .bands
                .iter()
                .find(|band| band.centre_hertz == centre)
                .map(|band| band.decibels)
        };
        let reference: Vec<Option<f64>> = half
            .iter()
            .map(|centre| find(first, *centre).map(|found| found.reference))
            .collect();
        let rows: Vec<_> = others
            .iter()
            .map(|other| {
                let values = half.iter().map(|centre| find(other, *centre));
                (
                    values.clone().map(|found| found.map(|f| f.other)).collect(),
                    values.map(|found| found.map(|f| f.difference)).collect(),
                )
            })
            .collect();
        let columns: Vec<String> = half.iter().map(|centre| frequency_label(*centre)).collect();
        curve(text, "Hz", &columns, 5, &rows, &reference, 0);
    }
}

fn write_texture(text: &mut String, comparison: &Comparison) {
    let others = &comparison.others;
    let _ = writeln!(
        text,
        "{:<GROUP$}SNR per statistic class (McDermott & Simoncelli 2011): 10 log10(sum s^2 / sum (s - s')^2),",
        "texture"
    );
    let _ = writeln!(
        text,
        "{:<GROUP$}s the reference's; the paper took >= {CONVERGED_DECIBELS:.0} dB as converged. In brackets the scatter floor:",
        ""
    );
    let _ = writeln!(
        text,
        "{:<GROUP$}the SNR two takes of the reference's sound reach by chance (from its two halves).",
        ""
    );
    let _ = writeln!(
        text,
        "{:<GROUP$}! = below {CONVERGED_DECIBELS:.0} dB and below the floor: a difference; ~ = below {CONVERGED_DECIBELS:.0} dB, within the floor",
        ""
    );
    let cell = VALUE + DIFFERENCE;
    for (class_index, class) in TextureClass::ALL.iter().enumerate() {
        let measure = match class {
            TextureClass::EnvelopeMean => "M1 envelope mean (re level)",
            TextureClass::Variance => "M2 variance / mean^2",
            TextureClass::Skewness => "M3 skewness",
            TextureClass::Kurtosis => "M4 kurtosis",
            TextureClass::Correlation => "C envelope correlation",
            TextureClass::ModulationPower => "modulation power",
            TextureClass::ModulationCorrelation => "C1 modulation correlation",
        };
        let mut line = format!("{:<GROUP$}{measure:<MEASURE$}{:>VALUE$}", "", "");
        for other in others {
            let found = &other.texture.classes[class_index];
            let floor = found
                .scatter_floor_decibels
                .map_or(String::new(), |floor| format!(" ({floor:.1})"));
            let snr = match found.snr_decibels {
                None => "identical".to_owned(),
                Some(snr) => format!("{snr:.1} dB{floor} {}", mark(found)),
            };
            let _ = write!(line, "{snr:>cell$}");
        }
        let _ = writeln!(text, "{}", line.trim_end());
    }
    for (index, other) in others.iter().enumerate() {
        let classes = &other.texture.classes;
        let within: Vec<&str> = classes
            .iter()
            .filter(|class| !class.converged && class.within_scatter)
            .map(|class| class.class.symbol())
            .collect();
        if !within.is_empty() {
            let _ = writeln!(
                text,
                "{:<GROUP$}{} below {CONVERGED_DECIBELS:.0} dB but within the scatter floor: {}",
                "",
                index + 1,
                within.join(", ")
            );
        }
        let below: Vec<&ClassDifference> = classes
            .iter()
            .filter(|class| !class.converged && !class.within_scatter)
            .collect();
        if below.is_empty() {
            continue;
        }
        let _ = writeln!(
            text,
            "{:<GROUP$}{} differs, where most (ref -> file, share of the class's squared error):",
            "",
            index + 1
        );
        for class in below {
            let places: Vec<String> = class
                .largest
                .iter()
                .map(|place| describe(class.class, place))
                .collect();
            let _ = writeln!(
                text,
                "{:<GROUP$}  {:<11}{}",
                "",
                class.class.symbol(),
                places.join("; ")
            );
        }
    }
    if let Some(first) = others.first() {
        let _ = writeln!(
            text,
            "{:<GROUP$}M1 compared after the level difference is removed: {}",
            "",
            others
                .iter()
                .enumerate()
                .map(|(index, other)| format!(
                    "{} {:+.1} dB",
                    index + 1,
                    other.texture.level_removed_decibels
                ))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let _ = writeln!(
            text,
            "{:<GROUP$}modulation power, mean over subbands, % of the envelope's variance:",
            ""
        );
        let percent = |value: f64| Some(100.0 * value);
        let reference: Vec<Option<f64>> = first
            .texture
            .modulation_spectrum
            .iter()
            .map(|rate| percent(rate.values.reference))
            .collect();
        let rows: Vec<_> = others
            .iter()
            .map(|other| {
                let rates = &other.texture.modulation_spectrum;
                (
                    rates
                        .iter()
                        .map(|rate| percent(rate.values.other))
                        .collect(),
                    rates
                        .iter()
                        .map(|rate| percent(rate.values.difference))
                        .collect(),
                )
            })
            .collect();
        let columns: Vec<String> = first
            .texture
            .modulation_spectrum
            .iter()
            .map(|rate| rate_label(rate.modulation_hertz))
            .collect();
        curve(text, "mod. Hz", &columns, 5, &rows, &reference, 1);

        let _ = writeln!(
            text,
            "{:<GROUP$}C, envelope correlation, mean over subband pairs this far apart (1 = move together):",
            ""
        );
        let reference: Vec<Option<f64>> = first
            .texture
            .correlation_by_offset
            .iter()
            .map(|offset| Some(offset.values.reference))
            .collect();
        let rows: Vec<_> = others
            .iter()
            .map(|other| {
                let offsets = &other.texture.correlation_by_offset;
                (
                    offsets
                        .iter()
                        .map(|offset| Some(offset.values.other))
                        .collect(),
                    offsets
                        .iter()
                        .map(|offset| Some(offset.values.difference))
                        .collect(),
                )
            })
            .collect();
        let columns: Vec<String> = first
            .texture
            .correlation_by_offset
            .iter()
            .map(|offset| offset.offset.to_string())
            .collect();
        curve(text, "apart", &columns, 6, &rows, &reference, 2);

        let _ = writeln!(
            text,
            "{:<GROUP$}C1, neighbouring subbands' modulation bands, mean, per band (Hz):",
            ""
        );
        let bands = &first.texture.modulation_correlation_by_band;
        let reference: Vec<Option<f64>> = bands
            .iter()
            .map(|band| Some(band.values.reference))
            .collect();
        let rows: Vec<_> = others
            .iter()
            .map(|other| {
                let bands = &other.texture.modulation_correlation_by_band;
                (
                    bands.iter().map(|band| Some(band.values.other)).collect(),
                    bands
                        .iter()
                        .map(|band| Some(band.values.difference))
                        .collect(),
                )
            })
            .collect();
        let columns: Vec<String> = bands
            .iter()
            .map(|band| rate_label(band.modulation_hertz))
            .collect();
        curve(text, "mod. Hz", &columns, 6, &rows, &reference, 2);
    }
}

/// `!` for a class that differs beyond chance, `~` for one below 20 dB
/// that two takes of the reference would reach anyway, nothing for one that
/// converged.
fn mark(class: &ClassDifference) -> &'static str {
    match (class.converged, class.within_scatter) {
        (true, _) => " ",
        (false, true) => "~",
        (false, false) => "!",
    }
}

/// A place a class's error lies, as "500 Hz 0.031 -> 0.046 (40 %)".
fn describe(class: TextureClass, place: &Place) -> String {
    let (digits, scale, unit) = match class {
        TextureClass::EnvelopeMean => (3, 1.0, ""),
        TextureClass::Variance => (3, 1.0, ""),
        TextureClass::ModulationPower => (1, 100.0, " %"),
        _ => (2, 1.0, ""),
    };
    let at = match place.partner_hertz {
        Some(partner) => format!(
            "{}+{} Hz",
            frequency_label(place.hertz),
            frequency_label(partner)
        ),
        None if matches!(
            class,
            TextureClass::ModulationPower | TextureClass::ModulationCorrelation
        ) =>
        {
            format!("{} Hz mod.", rate_label(place.hertz))
        }
        None => format!("{} Hz", frequency_label(place.hertz)),
    };
    format!(
        "{at} {:.digits$}{unit} -> {:.digits$}{unit} ({:.0} %)",
        scale * place.values.reference,
        scale * place.values.other,
        100.0 * place.share_of_error
    )
}

fn write_events(text: &mut String, comparison: &Comparison) {
    let others = &comparison.others;
    row(
        text,
        "events",
        "onsets per minute",
        others,
        &|other| Some((other.events.onsets_per_minute, false)),
        1,
    );
    let ratios: Vec<String> = others
        .iter()
        .enumerate()
        .map(|(index, other)| {
            format!(
                "{}: {}",
                index + 1,
                other.events.onsets_ratio.map_or(
                    "none (the reference has no onsets)".to_owned(),
                    |ratio| format!("x{ratio:.2}")
                )
            )
        })
        .collect();
    let _ = writeln!(
        text,
        "{:<GROUP$}{:<MEASURE$}{}",
        "",
        "  ratio, file over ref",
        ratios.join(", ")
    );
    for (measure, value) in [
        (
            "interval median (s)",
            (|other: &Differences| other.events.interval_median_seconds)
                as fn(&Differences) -> Option<Difference>,
        ),
        ("interval 10 % (s)", |other| {
            other.events.interval_p10_seconds
        }),
        ("interval 90 % (s)", |other| {
            other.events.interval_p90_seconds
        }),
    ] {
        if others.iter().any(|other| value(other).is_some()) {
            row(
                text,
                "",
                measure,
                others,
                &|other| value(other).map(|found| (found, false)),
                2,
            );
        }
    }
}

fn write_space(text: &mut String, comparison: &Comparison) {
    let others = &comparison.others;
    let Some(first) = others.iter().find_map(|other| other.space.as_ref()) else {
        let _ = writeln!(
            text,
            "{:<GROUP$}not compared: compared only between two two-channel files",
            "space"
        );
        return;
    };
    row(
        text,
        "space",
        "width (1 - IACC, 0.5-2 kHz)",
        others,
        &|other| {
            let space = other.space.as_ref()?;
            Some((space.width?, space.width_noticeable))
        },
        2,
    );
    let _ = writeln!(
        text,
        "{:<GROUP$}* beyond {WIDTH_NOTICEABLE}, the just-noticeable difference of IACC (Cox, Davies & Lam 1993).",
        ""
    );
    let _ = writeln!(
        text,
        "{:<GROUP$}per octave: IACC; envelope coherence (1 = the ears' levels move together); ILD standard deviation (dB)",
        ""
    );
    let columns: Vec<String> = first
        .octave_bands
        .iter()
        .map(|band| frequency_label(band.centre_hertz))
        .collect();
    type Pick = fn(&super::comparison::OctaveDifference) -> Option<Difference>;
    for (name, pick, digits) in [
        ("IACC Hz", (|band| band.iacc) as Pick, 2),
        ("coher. Hz", |band| band.envelope_coherence, 2),
        ("ILD sd Hz", |band| band.ild_spread_decibels, 1),
    ] {
        let reference: Vec<Option<f64>> = first
            .octave_bands
            .iter()
            .map(|band| pick(band).map(|found| found.reference))
            .collect();
        let rows: Vec<_> = others
            .iter()
            .map(|other| {
                let bands: Vec<Option<Difference>> = first
                    .octave_bands
                    .iter()
                    .map(|wanted| {
                        other
                            .space
                            .as_ref()?
                            .octave_bands
                            .iter()
                            .find(|band| band.centre_hertz == wanted.centre_hertz)
                            .and_then(pick)
                    })
                    .collect();
                (
                    bands.iter().map(|found| found.map(|f| f.other)).collect(),
                    bands
                        .iter()
                        .map(|found| found.map(|f| f.difference))
                        .collect(),
                )
            })
            .collect();
        curve(text, name, &columns, 7, &rows, &reference, digits);
    }
}

fn write_movement(text: &mut String, comparison: &Comparison) {
    let others = &comparison.others;
    type Pick = fn(&Differences) -> Option<Difference>;
    let rows: [(&str, &str, Pick, usize); 16] = [
        (
            "movement",
            "share in partials",
            |other| Some(other.movement.tonal_share),
            2,
        ),
        (
            "",
            "partials found",
            |other| Some(other.movement.partials.count),
            0,
        ),
        (
            "",
            "  median lines (voices)",
            |other| other.movement.partials.lines_median,
            1,
        ),
        (
            "",
            "  median width (Hz)",
            |other| other.movement.partials.width_median_hertz,
            1,
        ),
        (
            "",
            "  median level p10-p90 (dB)",
            |other| other.movement.partials.level_p10_p90_median_decibels,
            1,
        ),
        (
            "",
            "  median pitch sd (cents)",
            |other| {
                other
                    .movement
                    .partials
                    .pitch_standard_deviation_median_cents
            },
            1,
        ),
        (
            "",
            "  detuned between the ears",
            |other| other.movement.partials.detuned,
            0,
        ),
        (
            "",
            "broadband level sd (dB)",
            |other| Some(other.movement.slow.broadband_standard_deviation_decibels),
            1,
        ),
        (
            "",
            "broadband mod. slope",
            |other| other.movement.slow.broadband_modulation_slope,
            2,
        ),
        (
            "",
            "octaves move together",
            |other| other.movement.slow.all_pairs_correlation,
            2,
        ),
        (
            "",
            "centroid sd (octaves)",
            |other| Some(other.movement.slow.centroid_standard_deviation_octaves),
            2,
        ),
        (
            "",
            "centroid against level",
            |other| other.movement.slow.centroid_level_correlation,
            2,
        ),
        (
            "",
            "swells per minute",
            |other| Some(other.movement.swells.per_minute),
            1,
        ),
        (
            "",
            "  rise median (s)",
            |other| other.movement.swells.rise_median_seconds,
            1,
        ),
        (
            "",
            "  decay median (s)",
            |other| other.movement.swells.decay_median_seconds,
            1,
        ),
        (
            "",
            "  skew of level change",
            |other| other.movement.swells.rate_of_change_skewness,
            2,
        ),
    ];
    for (group, measure, pick, digits) in rows {
        row(
            text,
            group,
            measure,
            others,
            &|other| pick(other).map(|found| (found, false)),
            digits,
        );
    }
    let _ = writeln!(
        text,
        "{:<GROUP$}detuned: the ears' phase turns at 0.3 Hz or more, steadily from one 10 s to the next; octaves move together: mean correlation of their 1 s levels; slope over 0.01-0.5 Hz (-1 = 1/f)",
        ""
    );

    if let Some(first) = others.first() {
        let columns: Vec<String> = first
            .movement
            .slow
            .octave_bands
            .iter()
            .map(|band| frequency_label(band.centre_hertz))
            .collect();
        type Band = fn(&super::comparison::SlowBandDifference) -> Option<Difference>;
        for (name, pick, digits) in [
            (
                "level sd Hz",
                (|band| Some(band.standard_deviation_decibels)) as Band,
                1,
            ),
            ("mod. slope", |band| band.modulation_slope, 2),
        ] {
            let reference: Vec<Option<f64>> = first
                .movement
                .slow
                .octave_bands
                .iter()
                .map(|band| pick(band).map(|found| found.reference))
                .collect();
            let rows: Vec<CurveRow> = others
                .iter()
                .map(|other| {
                    let bands: Vec<Option<Difference>> = first
                        .movement
                        .slow
                        .octave_bands
                        .iter()
                        .map(|wanted| {
                            other
                                .movement
                                .slow
                                .octave_bands
                                .iter()
                                .find(|band| band.centre_hertz == wanted.centre_hertz)
                                .and_then(pick)
                        })
                        .collect();
                    (
                        bands.iter().map(|found| found.map(|f| f.other)).collect(),
                        bands
                            .iter()
                            .map(|found| found.map(|f| f.difference))
                            .collect(),
                    )
                })
                .collect();
            curve(text, name, &columns, 7, &rows, &reference, digits);
        }
    }

    let pulse = |found: &Option<crate::report::Pulse>| {
        found.as_ref().map_or("none".to_owned(), |pulse| {
            format!(
                "{:.2} Hz, depth {:.2}, rise {:.0} ms, decay {:.0} ms",
                pulse.rate_hertz,
                pulse.depth,
                1000.0 * pulse.rise_seconds,
                1000.0 * pulse.decay_seconds
            )
        })
    };
    let beats = |list: &[f64]| {
        if list.is_empty() {
            "none".to_owned()
        } else {
            list.iter()
                .map(|beat| format!("{beat:+.2} Hz"))
                .collect::<Vec<_>>()
                .join(", ")
        }
    };
    if let Some(first) = others.first() {
        let _ = writeln!(
            text,
            "{:<GROUP$}{:<MEASURE$}ref: {} | binaural beats: {}",
            "",
            "pulse",
            pulse(&first.movement.pulse.reference),
            beats(&first.movement.binaural_beats.reference)
        );
    }
    for (index, other) in others.iter().enumerate() {
        let _ = writeln!(
            text,
            "{:<GROUP$}{:<MEASURE$}{}: {} | binaural beats: {}",
            "",
            "",
            index + 1,
            pulse(&other.movement.pulse.other),
            beats(&other.movement.binaural_beats.other)
        );
    }
    for (index, other) in others.iter().enumerate() {
        let pairs = &other.movement.matched_partials;
        let shown: Vec<String> = pairs
            .iter()
            .take(SHOWN_MATCHES)
            .map(|pair| {
                let signed = |value: Option<Difference>, digits: usize| {
                    value.map_or("-".to_owned(), |found| {
                        format!("{:+.digits$}", found.difference)
                    })
                };
                format!(
                    "{:.1} Hz: level {} dB, wander {} dB, pitch sd {} c, width {} Hz",
                    pair.frequency_hertz.reference,
                    signed(Some(pair.relative_decibels), 1),
                    signed(pair.level_p10_p90_decibels, 1),
                    signed(pair.pitch_standard_deviation_cents, 1),
                    signed(Some(pair.width_hertz), 1)
                )
            })
            .collect();
        let _ = writeln!(
            text,
            "{:<GROUP$}{:<MEASURE$}{}: {}",
            "",
            if index == 0 {
                "same partials, file - ref"
            } else {
                ""
            },
            index + 1,
            if shown.is_empty() {
                "none within 3 %".to_owned()
            } else {
                format!(
                    "{}{}",
                    shown.join("; "),
                    if pairs.len() > SHOWN_MATCHES {
                        format!("; and {} more", pairs.len() - SHOWN_MATCHES)
                    } else {
                        String::new()
                    }
                )
            }
        );
    }
}

fn write_over_time(text: &mut String, comparison: &Comparison) {
    let others = &comparison.others;
    let Some(first) = others.first() else {
        return;
    };
    let relative = others
        .iter()
        .any(|other| other.over_time.laf_relative_to_l50);
    let unit = if relative {
        "dB relative to each file's own L50 (a file is uncalibrated)"
    } else {
        "dB SPL"
    };
    let _ = writeln!(
        text,
        "{:<GROUP$}LAF distribution, LN = the level exceeded N % of the time, {unit}:",
        "over time"
    );
    let shown: Vec<usize> = first
        .over_time
        .laf_exceeded
        .iter()
        .enumerate()
        .filter(|(_, found)| {
            [95.0, 90.0, 75.0, 50.0, 25.0, 10.0, 5.0].contains(&found.exceeded_percent)
        })
        .map(|(index, _)| index)
        .collect();
    let columns: Vec<String> = shown
        .iter()
        .map(|index| {
            format!(
                "L{:.0}",
                first.over_time.laf_exceeded[*index].exceeded_percent
            )
        })
        .collect();
    let reference: Vec<Option<f64>> = shown
        .iter()
        .map(|index| Some(first.over_time.laf_exceeded[*index].values.reference))
        .collect();
    let rows: Vec<_> = others
        .iter()
        .map(|other| {
            let found = &other.over_time.laf_exceeded;
            (
                shown
                    .iter()
                    .map(|index| found.get(*index).map(|f| f.values.other))
                    .collect(),
                shown
                    .iter()
                    .map(|index| found.get(*index).map(|f| f.values.difference))
                    .collect(),
            )
        })
        .collect();
    curve(text, "", &columns, 7, &rows, &reference, 1);
    for (index, other) in others.iter().enumerate() {
        let widths: Vec<String> = other
            .over_time
            .width_exceeded
            .iter()
            .map(|found| {
                format!(
                    "W{:.0} {:.2} -> {:.2}",
                    found.exceeded_percent, found.values.reference, found.values.other
                )
            })
            .collect();
        let moments = if other.lines_up {
            let laf = other
                .over_time
                .largest_laf
                .map_or("-".to_owned(), |moment| {
                    format!(
                        "{:+.1} dB at {:.1} s",
                        moment.values.difference, moment.seconds
                    )
                });
            let width = other
                .over_time
                .largest_width
                .map_or("-".to_owned(), |moment| {
                    format!(
                        "{:+.2} at {:.1} s",
                        moment.values.difference, moment.seconds
                    )
                });
            format!("lines up with the reference: LAF differs most {laf}, width {width}")
        } else {
            "does not line up with the reference (not one scene, seed and stretch): distributions only".to_owned()
        };
        let _ = writeln!(text, "{:<GROUP$}{} {moments}", "", index + 1);
        if !widths.is_empty() {
            let _ = writeln!(
                text,
                "{:<GROUP$}  width per 100 ms exceeded N % of the time, ref -> file: {}",
                "",
                widths.join(", ")
            );
        }
    }
}
