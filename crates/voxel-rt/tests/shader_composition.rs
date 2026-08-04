//! The shader set must be an import DAG, and every fragment must be composable in isolation.
//!
//! The renderer still assembles its compute modules by concatenating WGSL text (see
//! `passes::dda::SHADER_SOURCE`). That works — WGSL resolves module-scope declarations in any
//! order — but it gives no signal about which *direction* a dependency runs, because every
//! fragment lands in one namespace. A back-edge compiles exactly as happily as a forward edge.
//!
//! That is not hypothetical. `voxel-environment`'s `dispatch.wgsl` read `lighting.sky_ambient.w`
//! — the renderer's uniform — for long enough to survive a crate extraction and be written up in
//! a README as deliberate. Nothing could have caught it, because under `concat!` there was
//! nothing to catch.
//!
//! So this test builds the same fragments as a `naga_oil` import graph, which can only express a
//! DAG, and validates both entry points. It is the regression test for layering. When it fails,
//! it fails naming a file and a line in that file rather than an offset into a 70 KB string.
//!
//! **This is not how the pipelines are built.** Switching the runtime over needs the compile-time
//! levers to become `naga_oil` `shader_defs` first: `patch_shader_const` panics when the const it
//! is given is absent, so it can only be applied to the whole concatenation, not to one fragment.
//! Until then this test is the consumer of the composition table, and that is deliberate — an
//! unused `pub` composer in the renderer would be the speculative surface this workspace bans.

use std::collections::HashMap;

use naga_oil::compose::{
    ComposableModuleDescriptor, Composer, NagaModuleDescriptor, ShaderDefValue, ShaderLanguage,
};
use voxel_rt::passes::binding::{WorldBinding, WORLD_BIND_GROUP};

/// One WGSL fragment and where it sits in the import graph.
///
/// This table is the single declaration of what each compute module is made of. Before it, the
/// list lived twice — once in `passes::dda`'s `concat!` and once in `passes::cagi`'s — and each
/// pass therefore had to know the whole ingredient list including the parts owned by other
/// crates.
struct Fragment {
    /// `naga_oil` module path, or `None` for an entry-point shader. An entry point cannot be a
    /// composable module, so nothing may import from it — which is also why `dda.wgsl` and
    /// `cagi.wgsl` are exempt from the identifier rule described in `digit_terminated`.
    module: Option<&'static str>,
    file: &'static str,
    source: &'static str,
    /// Module paths this fragment reads from. Declared rather than inferred: an import that is
    /// not needed is dead weight, and `unused_imports_are_not_declared` fails on one.
    imports: &'static [&'static str],
    /// Prepend Stage A's generated binding-index consts. Each module gets its own copy; they
    /// are module-scoped so there is no collision, and this keeps `@group(G_WORLD)` a plain
    /// same-module const reference rather than a cross-module one.
    declares_bindings: bool,
}

const WORLD: &str = include_str!("../shaders/world.wgsl");
const PATTERN: &str = include_str!("../shaders/pattern.wgsl");
const WATER: &str = include_str!("../shaders/water.wgsl");
const CAGI_VOLUME: &str = include_str!("../shaders/cagi_volume.wgsl");
const DDA: &str = include_str!("../shaders/dda.wgsl");
const CAGI: &str = include_str!("../shaders/cagi.wgsl");

fn fragments() -> Vec<Fragment> {
    vec![
        Fragment {
            module: Some("vx::world"),
            file: "world.wgsl",
            source: WORLD,
            imports: &[],
            declares_bindings: true,
        },
        Fragment {
            module: Some("vx::environment"),
            file: "environment.wgsl",
            source: voxel_environment::HillaireEnvironment::WGSL,
            imports: &[],
            declares_bindings: false,
        },
        Fragment {
            module: Some("vx::tonemap"),
            file: "tonemap.wgsl",
            source: voxel_color::tonemap::WGSL,
            imports: &[],
            declares_bindings: false,
        },
        Fragment {
            module: Some("vx::graph_prelude"),
            file: "graph_prelude.wgsl",
            source: voxel_material_graph::WGSL_PRELUDE,
            imports: &["vx::world"],
            declares_bindings: false,
        },
        Fragment {
            module: Some("vx::pattern"),
            file: "pattern.wgsl",
            source: PATTERN,
            imports: &["vx::world", "vx::graph_prelude"],
            declares_bindings: true,
        },
        Fragment {
            module: Some("vx::water"),
            file: "water.wgsl",
            source: WATER,
            imports: &["vx::world"],
            declares_bindings: false,
        },
        Fragment {
            module: Some("vx::cagi_volume"),
            file: "cagi_volume.wgsl",
            source: CAGI_VOLUME,
            imports: &["vx::world", "vx::environment"],
            declares_bindings: true,
        },
        Fragment {
            module: Some("vx::graph_dispatch"),
            file: "material_graph.wgsl",
            source: voxel_material_graph::WGSL_DISPATCH,
            imports: &["vx::world", "vx::graph_prelude"],
            declares_bindings: false,
        },
        // The two entry points. Both are sinks: nothing imports from them, which is the
        // property that makes the whole set expressible as a DAG.
        Fragment {
            module: None,
            file: "dda.wgsl",
            source: DDA,
            imports: &[
                "vx::world",
                "vx::environment",
                "vx::pattern",
                "vx::water",
                "vx::cagi_volume",
                "vx::graph_dispatch",
                "vx::tonemap",
            ],
            declares_bindings: true,
        },
        Fragment {
            module: None,
            file: "cagi.wgsl",
            source: CAGI,
            imports: &["vx::world", "vx::environment", "vx::cagi_volume"],
            declares_bindings: true,
        },
    ]
}

/// Every module-scope declaration in a WGSL source.
///
/// Column 0 is the discriminator — this codebase never indents a module-scope declaration, and
/// every `var` inside a function body is indented. Getting that wrong reports cycles that are
/// not there, because a function-local `var origin` looks like a global.
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
        // `var<storage, read>` has a space *inside* the address space, so splitting on the
        // first space yields `read` rather than the variable's name.
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

/// Stage A's indices, as this module's own consts.
fn binding_prelude() -> String {
    WorldBinding::wgsl_prelude()
}

/// Build the `naga_oil` source for one fragment: its header, then its text unchanged.
///
/// The `#import` item lists are **generated** from each provider's own declarations rather than
/// written by hand. That is the difference between this and the obvious migration: `naga_oil`
/// gives no wildcard import, so the alternative is either qualifying ~250 call sites or
/// maintaining 93 item names in headers. A generated list is neither — it cannot drift from the
/// provider, and no shader body changes at all.
fn compose_source(fragment: &Fragment, all: &[Fragment]) -> String {
    let own: Vec<String> = module_scope_items(fragment.source);
    let mut header = String::new();
    if let Some(module) = fragment.module {
        header.push_str(&format!("#define_import_path {module}\n"));
    }
    for import in fragment.imports {
        let provider = all
            .iter()
            .find(|candidate| candidate.module == Some(*import))
            .unwrap_or_else(|| panic!("{} imports unknown module {import}", fragment.file));
        // Skip anything this fragment declares itself; a duplicate import would shadow it.
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
        binding_prelude()
    } else {
        String::new()
    };
    format!("{header}\n{prelude}{}", fragment.source)
}

/// Register every composable module, in declaration order, then return the composer.
///
/// Declaration order *is* topological order: `naga_oil` requires a module's imports to already
/// be registered, so a table in the wrong order fails here rather than producing something
/// subtly wrong.
fn composer_with_modules(all: &[Fragment], defs: &HashMap<String, ShaderDefValue>) -> Composer {
    let mut composer = Composer::default();
    for fragment in all {
        if fragment.module.is_none() {
            continue;
        }
        let source = compose_source(fragment, all);
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
    }
    composer
}

/// Both compute entry points must compose into a validated naga module.
///
/// This is the layering regression test. A fragment that reaches "upward" — into a consumer's
/// uniform, or into the entry point — cannot be expressed as an import and fails here.
#[test]
fn both_entry_points_compose_into_a_validated_module() {
    let all = fragments();
    let defs = HashMap::new();
    let mut composer = composer_with_modules(&all, &defs);

    for fragment in all.iter().filter(|fragment| fragment.module.is_none()) {
        let source = compose_source(fragment, &all);
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
        // Every binding the composed module kept must be a slot the allocator owns. Composition
        // tree-shakes unused globals, so this is a subset check, not an equality one.
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

/// A declared import that the fragment does not actually read is dead weight, and it hides the
/// real shape of the graph — the thing this whole table exists to make visible.
#[test]
fn every_declared_import_is_actually_used() {
    let all = fragments();
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
    let all = fragments();
    for fragment in &all {
        for import in fragment.imports {
            let provider = all
                .iter()
                .find(|candidate| candidate.module == Some(*import));
            assert!(
                provider.is_some(),
                "{} imports {import}, which is not a composable module",
                fragment.file
            );
        }
    }
}

/// A struct member in a composable module may not have a name ending in a digit.
///
/// `naga_oil` writes each composable module back out as WGSL, re-parses it, and rejects the
/// module if any name changed. naga's `Namer` appends a separator to any identifier ending in a
/// digit (`proc/namer.rs`: `base.ends_with(char::is_numeric)`), so such a name does not survive
/// the round trip.
///
/// **Only members.** Functions, consts, types and globals are safe, because `naga_oil` decorates
/// those with a module suffix before the round trip and the decorated name does not end in a
/// digit. Struct members are left undecorated, so they are the whole exposure — which is why
/// `world.wgsl`'s padding had to be renamed and `pattern_hash_u32` did not.
///
/// Entry points are exempt: they are never composable modules, so `dda.wgsl` keeps
/// `_pad0.._pad4` and `cagi.wgsl` keeps `CAGI_RULE_DIFFUSION_26`.
#[test]
fn no_composable_module_declares_a_digit_terminated_struct_member() {
    for fragment in fragments() {
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

/// Struct members are checked by `naga_oil` too, not just module-scope items.
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
