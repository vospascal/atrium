//! The shader set must be an import DAG, and every fragment must be composable in isolation.
//!
//! The fragment table itself lives in `passes::composer` — this file does not restate it, it
//! *builds* it a second way. The renderer concatenates those fragments into one WGSL module, which
//! works because WGSL resolves module-scope declarations in any order, but it gives no signal about
//! which *direction* a dependency runs: everything lands in one namespace, so a back-edge compiles
//! exactly as happily as a forward edge.
//!
//! That is not hypothetical. `voxel-environment`'s `dispatch.wgsl` read `lighting.sky_ambient.w` —
//! the renderer's uniform — for long enough to survive a crate extraction and be written up in a
//! README as deliberate. Nothing could have caught it, because under `concat!` there was nothing to
//! catch.
//!
//! So this builds the same fragments as a `naga_oil` import graph, which can only express a DAG,
//! and validates both entry points. When it fails, it fails naming a file and a line in that file
//! rather than an offset into a 70 KB string.
//!
//! `naga_oil` is a dev-dependency on purpose: nothing at runtime composes yet, and an unused
//! composer in the renderer would be the speculative surface this workspace's own rules ban.

use std::collections::HashMap;

use naga_oil::compose::{
    ComposableModuleDescriptor, Composer, NagaModuleDescriptor, ShaderDefValue, ShaderLanguage,
};
use voxel_rt::passes::binding::{WorldBinding, WORLD_BIND_GROUP};
use voxel_rt::passes::composer::{Composition, Fragment};
use voxel_rt::shader_consts::ShaderDefs;

/// Every module-scope declaration in a WGSL source.
///
/// Column 0 is the discriminator — this codebase never indents a module-scope declaration, and
/// every `var` inside a function body is indented. Getting that wrong reports cycles that are not
/// there, because a function-local `var origin` looks like a global.
fn module_scope_items(source: &str) -> Vec<String> {
    let mut items = Vec::new();
    for line in source.lines() {
        let declaration = if line.starts_with('@') {
            // A binding declaration carries its attributes on the same line.
            match line.find(" var") {
                Some(offset) => &line[offset + 1..],
                None => continue,
            }
        } else if line.starts_with("fn ")
            || line.starts_with("virtual fn ")
            || line.starts_with("struct ")
            || line.starts_with("const ")
            || line.starts_with("var")
        {
            line
        } else {
            continue;
        };

        let declaration = declaration.strip_prefix("virtual ").unwrap_or(declaration);
        // `var<storage, read>` has a space *inside* the address space, so splitting on the first
        // space yields `read` rather than the variable's name.
        let rest = if let Some(after) = declaration.strip_prefix("var<") {
            after.split_once('>').map(|(_, rest)| rest).unwrap_or("")
        } else {
            declaration
                .split_once(' ')
                .map(|(_, rest)| rest)
                .unwrap_or("")
        };
        let name: String = rest
            .trim_start()
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            items.push(name);
        }
    }
    items
}

/// Struct member names — `naga_oil`'s identifier check reaches inside struct definitions.
fn struct_member_names(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut inside = false;
    for line in source.lines() {
        if line.starts_with("struct ") {
            inside = true;
            continue;
        }
        if inside {
            if line.starts_with('}') {
                inside = false;
                continue;
            }
            if let Some((name, _)) = line.trim_start().split_once(':') {
                let name = name.trim();
                if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    names.push(name.to_string());
                }
            }
        }
    }
    names
}

fn is_freestanding(text: &str, at: usize, length: usize) -> bool {
    let before = text[..at].chars().next_back();
    let after = text[at + length..].chars().next();
    before.is_none_or(|c| !c.is_alphanumeric() && c != '_')
        && after.is_none_or(|c| !c.is_alphanumeric() && c != '_')
}

/// Every fragment either pass uses, deduplicated by file — `world.wgsl` and the environment appear
/// in both tables.
fn all_fragments() -> Vec<Fragment> {
    let mut all = Composition::shading().into_fragments();
    for fragment in Composition::volume().into_fragments() {
        if !all.iter().any(|known| known.file == fragment.file) {
            all.push(fragment);
        }
    }
    all
}

/// The `naga_oil` source for one fragment: its generated header, then its text unchanged.
///
/// The `#import` item lists are **generated** from each provider's own declarations rather than
/// written by hand. `naga_oil` has no wildcard import, so the alternatives were qualifying ~250
/// call sites or maintaining 93 item names in headers. A generated list is neither — it cannot
/// drift from the provider, and no shader body changes at all.
fn oil_source(fragment: &Fragment, all: &[Fragment]) -> String {
    let own = module_scope_items(fragment.source);
    let mut header = String::new();
    if let Some(module) = fragment.module {
        header.push_str(&format!("#define_import_path {module}\n"));
    }
    for import in fragment.imports {
        let provider = all
            .iter()
            .find(|candidate| candidate.module == Some(*import))
            .unwrap_or_else(|| panic!("{} imports unknown module {import}", fragment.file));
        let items: Vec<String> = module_scope_items(provider.source)
            .into_iter()
            .filter(|name| !own.contains(name))
            .collect();
        if items.is_empty() {
            continue;
        }
        header.push_str(&format!("#import {import}::{{{}}}\n", items.join(", ")));
    }
    let prelude = if fragment.declares_bindings {
        WorldBinding::wgsl_prelude()
    } else {
        String::new()
    };
    format!("{header}\n{prelude}{}", fragment.source)
}

/// Register every composable module. Declaration order *is* topological order: `naga_oil` requires
/// a module's imports to already be registered, so a table in the wrong order fails here rather
/// than producing something subtly wrong.
///
/// The shading table is registered first because it is the superset; the CA table's fragments are
/// the same objects.
fn composer_with_modules(all: &[Fragment], defs: &HashMap<String, ShaderDefValue>) -> Composer {
    let mut composer = Composer::default();
    // A module must follow its imports. Repeatedly register whatever is now satisfiable, which
    // makes this independent of the table's own ordering.
    let mut registered: Vec<&str> = Vec::new();
    let mut remaining: Vec<&Fragment> = all
        .iter()
        .filter(|fragment| fragment.module.is_some())
        .collect();
    while !remaining.is_empty() {
        let ready = remaining
            .iter()
            .position(|fragment| {
                fragment
                    .imports
                    .iter()
                    .all(|import| registered.contains(import))
            })
            .unwrap_or_else(|| {
                panic!(
                    "no fragment's imports are satisfied — the graph has a cycle among {:?}",
                    remaining.iter().map(|f| f.file).collect::<Vec<_>>()
                )
            });
        let fragment = remaining.remove(ready);
        let source = oil_source(fragment, all);
        if let Err(error) = composer.add_composable_module(ComposableModuleDescriptor {
            source: &source,
            file_path: fragment.file,
            language: ShaderLanguage::Wgsl,
            shader_defs: defs.clone(),
            ..Default::default()
        }) {
            panic!(
                "{} did not compose:\n{}",
                fragment.file,
                error.emit_to_string(&composer)
            );
        }
        registered.push(fragment.module.expect("composable module has a path"));
    }
    composer
}

/// Both compute entry points must compose into a validated naga module.
///
/// This is the layering regression test. A fragment that reaches "upward" — into a consumer's
/// uniform, or into an entry point — cannot be expressed as an import and fails here.
#[test]
fn both_entry_points_compose_into_a_validated_module() {
    let all = all_fragments();
    let defs = HashMap::new();
    let mut composer = composer_with_modules(&all, &defs);

    let entry_points: Vec<&Fragment> = all
        .iter()
        .filter(|fragment| fragment.module.is_none())
        .collect();
    assert_eq!(entry_points.len(), 2, "expected two compute entry points");

    for fragment in entry_points {
        let source = oil_source(fragment, &all);
        let module = match composer.make_naga_module(NagaModuleDescriptor {
            source: &source,
            file_path: fragment.file,
            shader_defs: defs.clone(),
            ..Default::default()
        }) {
            Ok(module) => module,
            Err(error) => panic!(
                "{} did not compose:\n{}",
                fragment.file,
                error.emit_to_string(&composer)
            ),
        };
        assert_eq!(
            module.entry_points.len(),
            1,
            "{} should expose exactly one compute entry point",
            fragment.file
        );
        // Composition tree-shakes unused globals, so this is a subset check: every group-0 binding
        // the module kept must be a slot the allocator owns.
        for (_, variable) in module.global_variables.iter() {
            if let Some(binding) = &variable.binding {
                if binding.group != WORLD_BIND_GROUP {
                    continue;
                }
                assert!(
                    WorldBinding::ALL
                        .iter()
                        .any(|slot| slot.index() == binding.binding),
                    "{} binds group {} slot {}, which no WorldBinding declares",
                    fragment.file,
                    binding.group,
                    binding.binding
                );
            }
        }
    }
}

/// A declared import the fragment does not read is dead weight, and it hides the real shape of the
/// graph — the thing the table exists to make visible.
#[test]
fn every_declared_import_is_actually_used() {
    let all = all_fragments();
    for fragment in &all {
        let own = module_scope_items(fragment.source);
        for import in fragment.imports {
            let provider = all
                .iter()
                .find(|candidate| candidate.module == Some(*import))
                .expect("declared import names a known module");
            let used = module_scope_items(provider.source).into_iter().any(|name| {
                !own.contains(&name)
                    && fragment
                        .source
                        .match_indices(&name)
                        .any(|(at, _)| is_freestanding(fragment.source, at, name.len()))
            });
            assert!(
                used,
                "{} declares `#import {import}` but reads nothing from it",
                fragment.file
            );
        }
    }
}

/// Nothing may import from an entry point, because `naga_oil` cannot make one a module.
#[test]
fn no_fragment_imports_from_an_entry_point() {
    let all = all_fragments();
    for fragment in &all {
        for import in fragment.imports {
            assert!(
                all.iter()
                    .any(|candidate| candidate.module == Some(*import)),
                "{} imports {import}, which is not a composable module",
                fragment.file
            );
        }
    }
}

/// A struct member in a composable module may not have a name ending in a digit.
///
/// `naga_oil` writes each composable module back out as WGSL, re-parses it, and rejects the module
/// if any name changed. naga's `Namer` appends a separator to any identifier ending in a digit
/// (`proc/namer.rs`: `base.ends_with(char::is_numeric)`), so such a name does not survive the round
/// trip.
///
/// **Only members.** Functions, consts, types and globals are safe: `naga_oil` decorates those with
/// a module suffix before the round trip, and the decorated name does not end in a digit. Struct
/// members are left undecorated, so they are the whole exposure — which is why `world.wgsl`'s
/// padding had to be renamed and `pattern_hash_u32` did not.
///
/// Entry points are exempt: they are never composable modules, so `dda.wgsl` keeps `_pad0.._pad4`
/// and `cagi.wgsl` keeps `CAGI_RULE_DIFFUSION_26`.
#[test]
fn no_composable_module_declares_a_digit_terminated_struct_member() {
    for fragment in all_fragments() {
        if fragment.module.is_none() {
            continue;
        }
        let offenders: Vec<String> = struct_member_names(fragment.source)
            .into_iter()
            .filter(|name| name.ends_with(|c: char| c.is_ascii_digit()))
            .collect();
        assert!(
            offenders.is_empty(),
            "{} has struct members naga_oil cannot carry through a module \
             (their names end in a digit): {offenders:?}",
            fragment.file
        );
    }
}

/// The composed graph must survive every lever permutation the app ships, not just the defaults.
///
/// A lever that guards a cross-fragment call can turn a dependency on or off, so "it composes at
/// default settings" is not the same claim as "it composes".
#[test]
fn every_quality_preset_composes() {
    let all = all_fragments();
    for spec in voxel_rt::variants::QUALITY_PRESETS {
        let quality = spec.resolve();
        for (label, defs) in [
            ("shading", quality.shading_shader_defs()),
            ("volume", quality.volume_shader_defs()),
        ] {
            // The levers are patched into the fragment text, exactly as the renderer does it, so
            // this composes the same bytes the pipeline would compile.
            let patched = patched_fragments(&all, &defs);
            let mut composer = composer_with_modules(&patched, &HashMap::new());
            for fragment in patched.iter().filter(|f| f.module.is_none()) {
                let source = oil_source(fragment, &patched);
                if let Err(error) = composer.make_naga_module(NagaModuleDescriptor {
                    source: &source,
                    file_path: fragment.file,
                    ..Default::default()
                }) {
                    panic!(
                        "{:?}/{label}: {} did not compose:\n{}",
                        spec.preset,
                        fragment.file,
                        error.emit_to_string(&composer)
                    );
                }
            }
        }
    }
}

/// `all_fragments` with each fragment's lever consts patched in.
///
/// Leaks the patched text so the returned fragments keep `&'static str` sources. A test process
/// exits; the alternative is threading a lifetime through every helper above for no benefit.
fn patched_fragments(all: &[Fragment], defs: &ShaderDefs) -> Vec<Fragment> {
    all.iter()
        .map(|fragment| {
            let patched = Composition::patch_fragment(fragment.source, defs);
            Fragment {
                module: fragment.module,
                file: fragment.file,
                source: Box::leak(patched.into_boxed_str()),
                imports: fragment.imports,
                declares_bindings: fragment.declares_bindings,
            }
        })
        .collect()
}
