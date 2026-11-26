//! A value that is fixed or a range, drawn anew each time it is used: `14`,
//! `"12..16 s"`. The unit is optional; when it is written it must be the
//! field's, so `hold: "12..16 dB"` is refused instead of read as seconds.

use serde::Deserialize;

use crate::random::Random;
use crate::RecipeError;

/// A value from `low` to `high`; both equal for a fixed one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub(crate) low: f64,
    pub(crate) high: f64,
}

impl Span {
    pub(crate) fn fixed(value: f64) -> Self {
        Self {
            low: value,
            high: value,
        }
    }

    pub fn low(&self) -> f64 {
        self.low
    }

    pub fn high(&self) -> f64 {
        self.high
    }

    /// The value a share `unit` (0 to 1) of the way from low to high.
    pub fn at(&self, unit: f64) -> f64 {
        self.low + unit.clamp(0.0, 1.0) * (self.high - self.low)
    }

    pub(crate) fn draw(&self, random: &mut Random) -> f64 {
        if self.low == self.high {
            self.low
        } else {
            self.at(random.unit())
        }
    }
}

/// A span as written: a number, or text such as `12..16 s`.
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub(crate) enum SpanDef {
    Number(f64),
    Text(String),
}

impl SpanDef {
    /// The span, in `unit` (`s`, `cents`, `dB`), never below `least`.
    pub(crate) fn resolve(&self, field: &str, unit: &str, least: f64) -> Result<Span, RecipeError> {
        let span = match self {
            SpanDef::Number(value) => Span::fixed(*value),
            SpanDef::Text(text) => parse(text, unit).ok_or_else(|| {
                RecipeError::new(
                    field,
                    format!(
                        "`{text}` is not a value or a range in {unit}, such as `12..16 {unit}`"
                    ),
                )
            })?,
        };
        if !span.low.is_finite() || !span.high.is_finite() {
            return Err(RecipeError::new(field, "is not a finite number"));
        }
        if span.low > span.high {
            return Err(RecipeError::new(field, "runs from high to low"));
        }
        if span.low < least {
            return Err(RecipeError::new(field, format!("is below {least} {unit}")));
        }
        Ok(span)
    }
}

fn parse(text: &str, unit: &str) -> Option<Span> {
    let text = text.trim();
    let numbers = match text.strip_suffix(unit) {
        Some(rest) => rest.trim_end(),
        None => text,
    };
    let number = |part: &str| part.trim().parse::<f64>().ok();
    match numbers.split_once("..") {
        Some((low, high)) => Some(Span {
            low: number(low)?,
            high: number(high)?,
        }),
        None => number(numbers).map(Span::fixed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> SpanDef {
        SpanDef::Text(value.to_owned())
    }

    #[test]
    fn fixed_values_and_ranges_with_or_without_their_unit() {
        assert_eq!(
            SpanDef::Number(14.0).resolve("hold", "s", 0.0),
            Ok(Span::fixed(14.0))
        );
        let range = text("12..16 s").resolve("hold", "s", 0.0).expect("a range");
        assert_eq!((range.low(), range.high()), (12.0, 16.0));
        assert_eq!(
            text("2..11")
                .resolve("drift", "cents", 0.0)
                .map(|span| span.high()),
            Ok(11.0)
        );
        assert_eq!(
            text("27 cents").resolve("detune", "cents", 0.0),
            Ok(Span::fixed(27.0))
        );
    }

    #[test]
    fn a_wrong_unit_or_order_is_refused_by_name() {
        let error = text("12..16 dB").resolve("hold", "s", 0.0).unwrap_err();
        assert_eq!(error.field, "hold");
        assert!(text("16..12 s").resolve("hold", "s", 0.0).is_err());
        assert!(text("-1 s").resolve("hold", "s", 0.0).is_err());
    }

    #[test]
    fn a_draw_stays_in_the_span() {
        let span = Span {
            low: 12.0,
            high: 16.0,
        };
        let mut random = Random::new(3);
        for _ in 0..1000 {
            assert!((12.0..=16.0).contains(&span.draw(&mut random)));
        }
        assert_eq!(span.at(0.5), 14.0);
    }
}
