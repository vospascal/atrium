//! A note as a recipe writes it, in hertz: a name and octave in scientific
//! pitch (`D3`, `Eb2`, `F#4`; A4 is the tuning, 440 Hz unless the recipe says
//! otherwise), with cents off (`D3 +27c`), or a frequency (`528 Hz`).

use crate::RecipeError;

/// The tuning as written: `A = 449 Hz`, `A4 = 432 Hz` or `449 Hz`: the
/// frequency of A4.
pub(crate) fn tuning(text: &str, field: &str) -> Result<f64, RecipeError> {
    let value = text
        .split_once('=')
        .map_or(text, |(name, value)| {
            let name = name.trim();
            if name == "A" || name == "A4" {
                value
            } else {
                ""
            }
        })
        .trim();
    hertz(value)
        .filter(|frequency| *frequency > 0.0)
        .ok_or_else(|| {
            RecipeError::new(
                field,
                format!("`{text}` is not a tuning, such as `A = 440 Hz`"),
            )
        })
}

/// The note's frequency in hertz, A4 being `a4` Hz.
pub(crate) fn frequency(text: &str, a4: f64, field: &str) -> Result<f64, RecipeError> {
    let refused = || {
        RecipeError::new(
            field,
            format!("`{text}` is not a note, such as `D3`, `Eb2`, `D3 +27c` or `528 Hz`"),
        )
    };
    let text = text.trim();
    if let Some(frequency) = hertz(text) {
        return if frequency > 0.0 {
            Ok(frequency)
        } else {
            Err(refused())
        };
    }
    let (name, cents) = match text.split_once(char::is_whitespace) {
        Some((name, offset)) => (name, cents(offset.trim()).ok_or_else(refused)?),
        None => (text, 0.0),
    };
    let semitones = semitones_from_a4(name).ok_or_else(refused)?;
    Ok(a4 * 2f64.powf((semitones + cents / 100.0) / 12.0))
}

/// `528 Hz`, `528hz`: the number, if the text is a frequency.
fn hertz(text: &str) -> Option<f64> {
    let lower = text.trim().to_ascii_lowercase();
    let number = lower.strip_suffix("hz")?.trim();
    number.parse::<f64>().ok().filter(|value| value.is_finite())
}

/// `+27c`, `-12c`, `+27 cents`.
fn cents(text: &str) -> Option<f64> {
    let number = text
        .strip_suffix("cents")
        .or_else(|| text.strip_suffix('c'))?
        .trim();
    number.parse::<f64>().ok().filter(|value| value.is_finite())
}

/// `D3` → −19: semitones from A4.
fn semitones_from_a4(name: &str) -> Option<f64> {
    let mut characters = name.chars();
    let letter = characters.next()?;
    let class: i32 = match letter.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest: String = characters.collect();
    let (accidental, octave) = match rest.chars().next() {
        Some('#') | Some('♯') => (1, &rest[rest.chars().next()?.len_utf8()..]),
        Some('b') | Some('♭') => (-1, &rest[rest.chars().next()?.len_utf8()..]),
        _ => (0, rest.as_str()),
    };
    let octave: i32 = octave.parse().ok()?;
    let midi = 12 * (octave + 1) + class + accidental;
    Some((midi - 69) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(found: f64, expected: f64) -> bool {
        (found - expected).abs() < 0.01
    }

    #[test]
    fn names_in_scientific_pitch() {
        assert!(close(frequency("A4", 440.0, "n").unwrap(), 440.0));
        assert!(close(frequency("D3", 440.0, "n").unwrap(), 146.83));
        assert!(close(frequency("C4", 440.0, "n").unwrap(), 261.63));
        assert!(close(frequency("Eb2", 440.0, "n").unwrap(), 77.78));
        assert!(close(frequency("D#2", 440.0, "n").unwrap(), 77.78));
        assert!(close(frequency("F#4", 440.0, "n").unwrap(), 369.99));
        assert!(close(frequency("Bb1", 440.0, "n").unwrap(), 58.27));
    }

    #[test]
    fn cents_frequencies_and_a_tuning() {
        // 27 cents above D3: 146.83 · 2^(27/1200) = 149.14 Hz.
        assert!(close(frequency("D3 +27c", 440.0, "n").unwrap(), 149.14));
        assert!(close(
            frequency("D3 -27 cents", 440.0, "n").unwrap(),
            144.56
        ));
        assert!(close(frequency("528 Hz", 440.0, "n").unwrap(), 528.0));
        assert!(close(tuning("A = 449 Hz", "t").unwrap(), 449.0));
        assert!(close(tuning("A4 = 432 Hz", "t").unwrap(), 432.0));
        // D3 at A = 449 Hz: 146.83 · 449/440.
        assert!(close(frequency("D3", 449.0, "n").unwrap(), 149.83));
    }

    #[test]
    fn what_is_not_a_note_is_refused_by_field() {
        for text in ["H3", "D", "D3 +27", "Dx3", "0 Hz", ""] {
            let error = frequency(text, 440.0, "chords[0].notes[1]").unwrap_err();
            assert_eq!(error.field, "chords[0].notes[1]", "{text}");
        }
        assert!(tuning("B = 440 Hz", "tuning").is_err());
    }
}
