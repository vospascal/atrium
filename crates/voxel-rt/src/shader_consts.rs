//! Compile-time shader consts: declared once per lever group, rendered two ways.
//!
//! Every quality lever is a WGSL `const` that a preset switch changes, which is why a preset
//! switch means a different pipeline. Historically each lever group wrote its consts straight
//! into the shader text with [`patch_shader_const`] — a `find`/`replace` on
//! `const NAME: type = value;`.
//!
//! That works, and it is why the shipped default is *literally the unpatched file*. But it ties
//! the levers to one representation, and the representation is the problem: text patching cannot
//! be applied to a single fragment, because [`patch_shader_const`] panics when the const it is
//! handed is absent, and each const lives in exactly one of ten files. So as long as levers are
//! text patches, the renderer can only ever compile one big concatenation — which is the thing
//! `tests/shader_composition.rs` exists to replace.
//!
//! The fix is not a second mechanism. A lever group declares its consts into a
//! [`ShaderConstSink`], and there are two sinks:
//!
//! - [`SourcePatcher`] — patches the text, exactly as before.
//! - [`ShaderDefs`] — collects `naga_oil` preprocessor definitions.
//!
//! One declaration, two renderings. The def map cannot disagree with the patched source, because
//! there is only one list and both sinks read it. That is the property a parallel
//! `fn shader_defs()` per group would *not* have had — it would have been 43 values duplicated by
//! hand, where a single wrong one is a wrong pixel and no error.

use std::collections::BTreeMap;

/// The value of one compile-time shader const.
///
/// The two float shapes exist because `naga_oil` definitions are only bool/i32/u32 — there is no
/// float def. Both of this renderer's `f32` levers turn out to be expressible anyway, which is
/// the only reason the migration is possible at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShaderConstValue {
    /// A `bool` const.
    Boolean(bool),
    /// A `u32` const.
    Unsigned(u32),
    /// An `f32` const whose value is always integral, carried as the integer.
    ///
    /// `AO_MAX_DISTANCE` is this: the Rust side is already `max_distance_voxels: u32` and only
    /// ever formatted as `"{}.0"`, so nothing is lost — the WGSL reads `f32(#{AO_MAX_DISTANCE})`.
    IntegralFloat(u32),
    /// An `f32` const carried as `round(value * per_unit)`, divided back on the WGSL side.
    ///
    /// `MATERIAL_PATTERN_STRENGTH` is the only one: a continuous 0..1 slider, deliberately a
    /// const rather than a uniform so naga can fold a zero-strength layer away entirely. At
    /// `per_unit = 1000` the slider quantises to 0.001, finer than its own pixel resolution, and
    /// every value any test or preset uses (1.0) is carried exactly.
    ScaledFloat { scaled: u32, per_unit: u32 },
}

impl ShaderConstValue {
    /// The WGSL literal this value patches into a `const` declaration.
    ///
    /// Kept identical to what the lever groups used to format by hand, so the shipped sources
    /// are unchanged to the byte and `build_shader_source(&balanced) == SHADER_SOURCE` still
    /// holds.
    pub fn wgsl_literal(self) -> String {
        match self {
            Self::Boolean(true) => "true".to_string(),
            Self::Boolean(false) => "false".to_string(),
            Self::Unsigned(value) => format!("{value}u"),
            Self::IntegralFloat(value) => format!("{value}.0"),
            Self::ScaledFloat { scaled, per_unit } => {
                float_literal(scaled as f32 / per_unit as f32)
            }
        }
    }
}

/// A WGSL `f32` literal for `value`.
///
/// `{:?}` rather than `{}` on purpose: Rust's `Display` for floats prints `1` for `1.0`, which
/// WGSL reads as an `i32` and rejects where an `f32` is expected. `Debug` always emits the
/// decimal point, so `1.0` stays `1.0` and `0.5` stays `0.5`.
pub(crate) fn float_literal(value: f32) -> String {
    format!("{value:?}")
}

/// Somewhere a lever group's compile-time consts can be written.
pub(crate) trait ShaderConstSink {
    fn set(&mut self, name: &'static str, value: ShaderConstValue);

    /// Declare a const that legitimately does not exist in every shader.
    ///
    /// Only one group needs this: the water levers live in `water.wgsl`, which the CA pass does
    /// not include, while `WATER_SUN_THROUGH_LIQUID` lives in the shared `world.wgsl` and must
    /// move in both. Absence there is correct, not a dead lever — and keeping it a *separate*
    /// method is what lets [`ShaderConstSink::set`] keep panicking for the genuine case, where a
    /// renamed const would otherwise turn into a silently inert lever.
    fn set_if_present(&mut self, name: &'static str, value: ShaderConstValue) {
        self.set(name, value);
    }

    fn boolean(&mut self, name: &'static str, value: bool) {
        self.set(name, ShaderConstValue::Boolean(value));
    }

    fn unsigned(&mut self, name: &'static str, value: u32) {
        self.set(name, ShaderConstValue::Unsigned(value));
    }

    fn integral_float(&mut self, name: &'static str, value: u32) {
        self.set(name, ShaderConstValue::IntegralFloat(value));
    }

    fn scaled_float(&mut self, name: &'static str, value: f32, per_unit: u32) {
        self.set(
            name,
            ShaderConstValue::ScaledFloat {
                scaled: (value * per_unit as f32).round().max(0.0) as u32,
                per_unit,
            },
        );
    }
}

/// The sink that rewrites shader text — the shipped path.
pub(crate) struct SourcePatcher {
    source: String,
}

impl SourcePatcher {
    pub fn new(source: &str) -> Self {
        Self {
            source: source.to_string(),
        }
    }

    /// The patched source.
    pub fn finish(self) -> String {
        self.source
    }
}

impl ShaderConstSink for SourcePatcher {
    fn set(&mut self, name: &'static str, value: ShaderConstValue) {
        self.source = patch_shader_const(&self.source, name, &value.wgsl_literal());
    }

    fn set_if_present(&mut self, name: &'static str, value: ShaderConstValue) {
        if self.source.contains(&format!("const {name}:")) {
            self.set(name, value);
        }
    }
}

/// The sink that collects preprocessor definitions.
///
/// Keyed by the const's own name, so no shader identifier has to be renamed: the WGSL keeps
/// `const AO_MODE: u32 = ...` and its initialiser becomes `#{AO_MODE}`.
///
/// A `BTreeMap` rather than a `HashMap` because this set is about to become a pipeline cache key,
/// and a cache key has to have a deterministic serialisation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShaderDefs {
    definitions: BTreeMap<&'static str, ShaderConstValue>,
}

impl ShaderDefs {
    pub fn get(&self, name: &str) -> Option<ShaderConstValue> {
        self.definitions.get(name).copied()
    }

    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&'static str, ShaderConstValue)> + '_ {
        self.definitions.iter().map(|(name, value)| (*name, *value))
    }
}

impl ShaderConstSink for ShaderDefs {
    fn set(&mut self, name: &'static str, value: ShaderConstValue) {
        // Last write wins, matching `SourcePatcher`: patching the same const twice leaves the
        // second value in the text.
        self.definitions.insert(name, value);
    }
}

/// `shader_source` with the WGSL `const constant_name` declaration's value replaced.
///
/// Panics when the const is absent, which is deliberate — a lever whose const has been renamed
/// or deleted is a silently dead lever otherwise. It is also exactly why this cannot be applied
/// per fragment: each const lives in one of ten files.
pub(crate) fn patch_shader_const(
    shader_source: &str,
    constant_name: &str,
    new_value_literal: &str,
) -> String {
    let declaration_prefix = format!("const {constant_name}:");
    let declaration_start = shader_source
        .find(&declaration_prefix)
        .unwrap_or_else(|| panic!("shader const `{constant_name}` not found"));
    let equals_offset = shader_source[declaration_start..]
        .find('=')
        .unwrap_or_else(|| panic!("shader const `{constant_name}` has no `=`"))
        + declaration_start;
    let semicolon_offset = shader_source[equals_offset..]
        .find(';')
        .unwrap_or_else(|| panic!("shader const `{constant_name}` has no `;`"))
        + equals_offset;
    format!(
        "{}= {new_value_literal}{}",
        &shader_source[..equals_offset],
        &shader_source[semicolon_offset..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The literals must match what the lever groups formatted by hand before the sink existed,
    /// or the shipped shader source changes and every pipeline cache key with it.
    #[test]
    fn literals_match_the_hand_written_forms() {
        assert_eq!(ShaderConstValue::Boolean(true).wgsl_literal(), "true");
        assert_eq!(ShaderConstValue::Boolean(false).wgsl_literal(), "false");
        assert_eq!(ShaderConstValue::Unsigned(3).wgsl_literal(), "3u");
        assert_eq!(ShaderConstValue::IntegralFloat(8).wgsl_literal(), "8.0");
        assert_eq!(
            ShaderConstValue::ScaledFloat {
                scaled: 1000,
                per_unit: 1000
            }
            .wgsl_literal(),
            "1.0"
        );
    }

    /// The two sinks must agree: whatever the patcher writes as a literal, the def map must
    /// carry as a value that renders to the same literal. This is the property that makes one
    /// declaration safe to render two ways.
    #[test]
    fn both_sinks_receive_the_same_values() {
        let source = "const A: bool = false;\nconst B: u32 = 0u;\nconst C: f32 = 0.0;\n";
        let mut patcher = SourcePatcher::new(source);
        let mut defs = ShaderDefs::default();
        for sink in [
            &mut patcher as &mut dyn ShaderConstSink,
            &mut defs as &mut dyn ShaderConstSink,
        ] {
            sink.boolean("A", true);
            sink.unsigned("B", 7);
            sink.integral_float("C", 8);
        }
        let patched = patcher.finish();
        for (name, value) in defs.iter() {
            assert!(
                patched.contains(&format!(
                    "const {name}: {} = {};",
                    match name {
                        "A" => "bool",
                        "B" => "u32",
                        _ => "f32",
                    },
                    value.wgsl_literal()
                )),
                "def {name} = {value:?} does not match the patched source:\n{patched}"
            );
        }
    }

    /// A continuous slider must survive the round trip at the granularity the sink promises.
    #[test]
    fn scaled_floats_carry_every_value_a_slider_produces() {
        let mut defs = ShaderDefs::default();
        defs.scaled_float("S", 1.0, 1000);
        assert_eq!(defs.get("S").unwrap().wgsl_literal(), "1.0");
        defs.scaled_float("S", 0.0, 1000);
        assert_eq!(defs.get("S").unwrap().wgsl_literal(), "0.0");
        defs.scaled_float("S", 0.625, 1000);
        assert_eq!(defs.get("S").unwrap().wgsl_literal(), "0.625");
    }

    #[test]
    fn patching_an_absent_const_panics_rather_than_silently_doing_nothing() {
        let result = std::panic::catch_unwind(|| {
            patch_shader_const("const A: bool = false;", "MISSING", "true")
        });
        assert!(result.is_err());
    }
}
