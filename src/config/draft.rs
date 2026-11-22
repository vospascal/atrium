//! A draft preset for a kind cut from a recording: its takes and how it
//! rests, as measured by `atrium_analysis::cut` (docs/cutter-plan.md, "What
//! goes into the preset"). Written as text, comments and all, for a person to
//! read, fill in and copy to `sources/`.
//!
//! - `takes`: every kept take that is not flagged, named by type and number.
//! - `rule.rests.gap`: the 10th and 90th percentiles of the gaps between
//!   takes, end to start, as the engine's gap runs.
//! - `rule.rests.bout`: the shortest and longest run of one type, when there
//!   are two types or more and some run is longer than one.
//! - `level_spread` is left out: the engine sets one level for all takes
//!   together (`built_source.rs`), so the takes keep their measured
//!   differences, and a spread on top would count them twice.
//! - `reference_spl` is left out, so the preset does not load until it is
//!   filled in: a recording's dBFS says nothing of SPL.
//! - `name`, `group`, `width`, `vary.pitch` and `follows` are not in a
//!   recording; they are copied from the kind's existing preset, if there is
//!   one.

use atrium_analysis::Cut;
use serde_yaml::Value;

/// What the draft is of.
pub struct Draft<'a> {
    /// The recording cut, as given (with its `@from-to`).
    pub recording: &'a str,
    /// The kind, as `--name` gave it: the preset's name unless the existing
    /// preset has one.
    pub name: &'a str,
    /// Each kept take's file, in the cut's take order.
    pub take_files: &'a [String],
    /// The kind's existing preset: its path and its text.
    pub existing: Option<(&'a str, &'a str)>,
}

/// The draft preset's text.
pub fn draft_preset(cut: &Cut, draft: &Draft) -> Result<String, String> {
    if draft.take_files.len() != cut.takes.len() {
        return Err(format!(
            "{} take files for {} takes",
            draft.take_files.len(),
            cut.takes.len()
        ));
    }
    let existing: Option<(&str, Value)> = draft
        .existing
        .map(|(path, text)| {
            serde_yaml::from_str::<Value>(text)
                .map(|value| (path, value))
                .map_err(|error| format!("{path}: {error}"))
        })
        .transpose()?;
    let from_existing = |key: &str| -> Option<(&str, &Value)> {
        let (path, value) = existing.as_ref()?;
        Some((*path, value.get(key)?))
    };

    let mut text = String::new();
    let mut line = |line: &str| {
        text.push_str(line);
        text.push('\n');
    };
    line(&format!(
        "# Draft of a kind, cut from {} by `cut` (docs/cutter-plan.md).",
        draft.recording
    ));
    line("# Listen to the takes before copying them and this file across: the takes to");
    line("# assets/, this to sources/, the take paths below changed to match.");
    line("#");
    line("# reference_spl is left out on purpose, so this preset does not load until it");
    line("# is filled in. A recording's level in dBFS says nothing of the sound's level in");
    line("# the world: give its level at 1 m while it sounds, from the literature, as the");
    line("# birds' came from Brumm et al. 2025.");
    line("# reference_spl:            # dB SPL at 1 m, while it sounds");
    line("");
    let name = from_existing("name")
        .and_then(|(_, value)| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| title(draft.name));
    line(&format!("name: {}", scalar(&Value::String(name))));
    if let Some((path, group)) = from_existing("group") {
        line(&format!(
            "group: {}              # from {path}",
            scalar(group)
        ));
    }
    if let Some((path, width)) = from_existing("width") {
        line(&format!(
            "width: {}                # from {path}",
            scalar(width)
        ));
    }
    line("");

    let flagged = cut.takes.iter().filter(|take| take.flagged).count();
    let kept: Vec<String> = cut
        .takes
        .iter()
        .zip(draft.take_files)
        .filter(|(take, _)| !take.flagged)
        .map(|(take, file)| {
            format!(
                "  - {{ file: {}, name: {} }}",
                scalar(&Value::String(file.clone())),
                take.name
            )
        })
        .collect();
    if flagged > 0 {
        line(&format!(
            "# {flagged} flagged take(s) left out: under {:.0} dB above their bed, the bed would",
            cut.thresholds.flag_below_snr_decibels
        ));
        line("# gate in and out with them.");
    }
    if kept.is_empty() {
        line("takes: []                  # no take kept: nothing to play");
    } else {
        line("takes:");
        for take in &kept {
            line(take);
        }
    }
    line("");

    line("rule:");
    line("  rests:");
    match (cut.gaps.p10, cut.gaps.p90) {
        (Some(p10), Some(p90)) => {
            line(&format!(
                "    # Measured: {} gaps between takes, end to start; their 10th and 90th",
                cut.gaps.seconds.len()
            ));
            line("    # percentiles. The rule draws evenly inside; cut.json has them all.");
            line(&format!("    gap: [{}, {}]", rounded(p10), rounded(p90)));
        }
        _ => line(
            "    # No gap measured (fewer than two takes): fill one in. gap: [shortest, longest]",
        ),
    }
    let two_types = cut.types.groups.len() >= 2;
    if let (true, Some(shortest), Some(longest)) =
        (two_types, cut.bouts.shortest, cut.bouts.longest)
    {
        if longest > 1 {
            line(&format!(
                "    bout: [{shortest}, {longest}]            # measured: runs of one type"
            ));
        }
    }
    if let Some((path, pitch)) = from_existing("rule")
        .and_then(|(path, rule)| Some((path, rule.get("rests")?.get("vary")?.get("pitch")?)))
    {
        line(&format!(
            "    vary: {{ pitch: {} }}       # from {path}",
            scalar(pitch)
        ));
    }
    line("    # level_spread left out: the engine sets one level for all takes together,");
    line("    # so they keep their measured differences; a spread on top would count them");
    line("    # twice. In the field the takes' spread also holds the animal turning and");
    line("    # moving, not only its voice.");

    if let Some((path, Value::Sequence(follows))) = from_existing("follows") {
        line("");
        line(&format!("# From {path}."));
        line("follows:");
        for follow in follows {
            line(&format!("  - {}", flow(follow)));
        }
    }
    Ok(text)
}

/// `frog` as `Frog`, `blue-tit` as `Blue tit`.
fn title(kind: &str) -> String {
    let words = kind.replace(['-', '_'], " ");
    let mut characters = words.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// Seconds to two decimals, without trailing zeros: 1.9, 0.35, 12.
fn rounded(seconds: f64) -> String {
    let text = format!("{seconds:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// A scalar as YAML writes it on one line.
fn scalar(value: &Value) -> String {
    serde_yaml::to_string(value)
        .map(|text| text.trim_end().to_owned())
        .unwrap_or_default()
}

/// A value in YAML's flow style, `{ key: value, … }` and `[a, b]`.
fn flow(value: &Value) -> String {
    match value {
        Value::Mapping(mapping) => {
            let fields: Vec<String> = mapping
                .iter()
                .map(|(key, value)| format!("{}: {}", scalar(key), flow(value)))
                .collect();
            format!("{{ {} }}", fields.join(", "))
        }
        Value::Sequence(items) => {
            let items: Vec<String> = items.iter().map(flow).collect();
            format!("[{}]", items.join(", "))
        }
        Value::Tagged(tagged) => flow(&tagged.value),
        other => scalar(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_numbers() {
        assert_eq!(title("frog"), "Frog");
        assert_eq!(title("blue-tit"), "Blue tit");
        assert_eq!(rounded(1.904), "1.9");
        assert_eq!(rounded(12.0), "12");
        let follow: Value =
            serde_yaml::from_str("{ condition: rain, from: 1, over: [0.05, 0.35] }").unwrap();
        assert_eq!(
            flow(&follow),
            "{ condition: rain, from: 1, over: [0.05, 0.35] }"
        );
    }
}
