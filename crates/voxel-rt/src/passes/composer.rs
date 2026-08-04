//! What each compute module is made of — one table, two renderings.
//!
//! Both compute shaders are assembled from ten WGSL fragments: five of this crate's own files
//! plus the environment sampler, the tonemap curves, the material-graph ABI and its dispatch,
//! which belong to `voxel-environment`, `voxel-color` and `voxel-material-graph`. Until this
//! module existed that list lived twice, in a `concat!` inside `passes::dda` and another inside
//! `passes::cagi`, so each pass had to know the whole ingredient list including the parts it does
//! not own.
//!
//! # Two renderings, one list
//!
//! [`Composition::build`] produces a [`ShaderProgram`]: the fragments built as a `naga_oil` import
//! graph, **and** the same fragments concatenated. The module is what the device compiles; the
//! joined source keys the pipeline cache and is what a dump shows.
//!
//! Composing rather than concatenating is the point. `naga_oil` can only express a DAG, so a
//! fragment reaching into a consumer's uniform cannot be spelled as an import — which is exactly
//! the bug `voxel-environment`'s `dispatch.wgsl` carried for months, reading the renderer's
//! `lighting.sky_ambient.w` while a `concat!` resolved it happily in one flat namespace. Errors now
//! name a file and a line inside it rather than an offset into 70 KB of concatenation.
//!
//! The joined source is still byte-for-byte what the old `concat!` produced
//! (`joined_source_matches_the_shipped_concatenation` pins it per preset), which is what let the
//! cache keep its existing key: a preset switch hits and misses exactly as it did before.
//!
//! # Levers are patched per fragment
//!
//! Every one of the 39 compile-time lever consts is declared in exactly one fragment
//! (`every_lever_const_belongs_to_exactly_one_fragment` pins that), so a fragment can be patched
//! with the whole [`ShaderDefs`] set and take only the consts it actually declares. That is what
//! makes composition possible at all: `patch_shader_const` panics on an absent const, so before
//! the ownership was known, levers could only be applied to one big concatenation.

use crate::passes::binding::WorldBinding;
use crate::shader_consts::{ShaderConstSink, ShaderDefs, SourcePatcher};

/// One WGSL fragment and where it sits in the import graph.
pub struct Fragment {
    /// `naga_oil` module path, or `None` for an entry-point shader.
    ///
    /// An entry point can never be a composable module, so nothing may import from it. Both of
    /// this renderer's entry points are sinks in the graph, which is the property that makes the
    /// whole set expressible as a DAG.
    pub module: Option<&'static str>,
    /// Reported in composition errors, so a failure names a file rather than an offset into a
    /// 70 KB string.
    pub file: &'static str,
    pub source: &'static str,
    /// Module paths this fragment reads from.
    pub imports: &'static [&'static str],
    /// Whether to prepend Stage A's generated binding-index consts.
    ///
    /// Each module gets its own copy. They are module-scoped, so there is no collision, and it
    /// keeps `@group(G_WORLD)` a plain same-module const reference rather than a cross-module one.
    pub declares_bindings: bool,
}

/// The shading pass's fragments, in the order the shipped source concatenates them.
///
/// Order is load-bearing twice over: it must reproduce the old `concat!` byte for byte, and
/// `naga_oil` requires a module's imports to be registered before the module itself, so a table in
/// the wrong order fails loudly rather than composing something subtly different.
pub fn shading_fragments() -> Vec<Fragment> {
    vec![
        world(),
        pattern(),
        cagi_volume(),
        water(),
        Fragment {
            module: None,
            file: "dda.wgsl",
            source: include_str!("../../shaders/dda.wgsl"),
            imports: &[
                "vx::world",
                "vx::pattern",
                "vx::cagi_volume",
                "vx::water",
                "vx::graph_dispatch",
                "vx::environment",
                "vx::tonemap",
            ],
            declares_bindings: true,
        },
        graph_prelude(),
        graph_dispatch(),
        environment(),
        tonemap(),
    ]
}

/// The CA (light volume) pass's fragments, in shipped concatenation order.
pub fn volume_fragments() -> Vec<Fragment> {
    vec![
        world(),
        cagi_volume(),
        Fragment {
            module: None,
            file: "cagi.wgsl",
            source: include_str!("../../shaders/cagi.wgsl"),
            imports: &["vx::world", "vx::cagi_volume", "vx::environment"],
            declares_bindings: true,
        },
        environment(),
    ]
}

fn world() -> Fragment {
    Fragment {
        module: Some("vx::world"),
        file: "world.wgsl",
        source: include_str!("../../shaders/world.wgsl"),
        imports: &[],
        declares_bindings: true,
    }
}

fn pattern() -> Fragment {
    Fragment {
        module: Some("vx::pattern"),
        file: "pattern.wgsl",
        source: include_str!("../../shaders/pattern.wgsl"),
        imports: &["vx::world", "vx::graph_prelude"],
        declares_bindings: true,
    }
}

fn cagi_volume() -> Fragment {
    Fragment {
        module: Some("vx::cagi_volume"),
        file: "cagi_volume.wgsl",
        source: include_str!("../../shaders/cagi_volume.wgsl"),
        imports: &["vx::world", "vx::environment"],
        declares_bindings: true,
    }
}

fn water() -> Fragment {
    Fragment {
        module: Some("vx::water"),
        file: "water.wgsl",
        source: include_str!("../../shaders/water.wgsl"),
        imports: &["vx::world"],
        declares_bindings: false,
    }
}

fn graph_prelude() -> Fragment {
    Fragment {
        module: Some("vx::graph_prelude"),
        file: "graph_prelude.wgsl",
        source: voxel_material_graph::WGSL_PRELUDE,
        imports: &["vx::world"],
        declares_bindings: false,
    }
}

fn graph_dispatch() -> Fragment {
    Fragment {
        module: Some("vx::graph_dispatch"),
        file: "material_graph.wgsl",
        source: voxel_material_graph::WGSL_DISPATCH,
        imports: &["vx::world", "vx::graph_prelude"],
        declares_bindings: false,
    }
}

/// The environment sampler, from the crate that owns it.
///
/// Named through the adapter's `WGSL` const rather than [`voxel_environment::EnvironmentGpu`]'s
/// runtime method because a fragment table is built before any device exists. That naming is the
/// honest cost of assembling a shader at startup, not a leak — see the const's own documentation.
fn environment() -> Fragment {
    Fragment {
        module: Some("vx::environment"),
        file: "environment.wgsl",
        source: voxel_environment::HillaireEnvironment::WGSL,
        imports: &[],
        declares_bindings: false,
    }
}

fn tonemap() -> Fragment {
    Fragment {
        module: Some("vx::tonemap"),
        file: "tonemap.wgsl",
        source: voxel_color::tonemap::WGSL,
        imports: &[],
        declares_bindings: false,
    }
}

/// A text edit applied to ONE named fragment after its lever consts are patched.
///
/// Two things need this, and both used to be applied to the whole concatenation only because that
/// was the only string there was: the output-format patch rewrites the storage-texture declaration
/// in `dda.wgsl`, and the material-graph injection fills the `GRAPH_DISPATCH_POINT` marker in
/// `material_graph.wgsl` and appends the generated functions after it. Naming the fragment is what
/// lets both survive a per-fragment composition.
pub struct FragmentEdit<'a> {
    pub file: &'static str,
    /// Owned rather than borrowed so a caller can pass a closure that captures — the
    /// output-format patch captures the format by value.
    pub apply: Box<dyn Fn(&str) -> String + 'a>,
}

/// A compilable shader: the joined WGSL and the composed module built from the same fragments.
///
/// Both, deliberately. The module is what the device compiles; the source is what keys the pipeline
/// cache and what a dump shows. Keeping the existing key means a preset switch hits or misses the
/// cache exactly as it did before this change — re-keying on the def set would have been a second
/// behaviour change riding along with the compile path.
pub struct ShaderProgram {
    pub source: String,
    pub module: naga::Module,
}

/// A fragment set plus the lever values to compile it with.
pub struct Composition {
    fragments: Vec<Fragment>,
}

impl Composition {
    /// The shading pass's composition.
    pub fn shading() -> Self {
        Self {
            fragments: shading_fragments(),
        }
    }

    /// The CA pass's composition.
    pub fn volume() -> Self {
        Self {
            fragments: volume_fragments(),
        }
    }

    /// Take the table, for a consumer that needs to rebuild the set a different way — the layering
    /// test builds it as a `naga_oil` import graph.
    pub fn into_fragments(self) -> Vec<Fragment> {
        self.fragments
    }

    /// One fragment's text with `defs` applied to the consts that fragment declares.
    ///
    /// A fragment takes only its own consts — every lever const has exactly one owner
    /// (`every_lever_const_belongs_to_exactly_one_fragment`) — which is why the whole set can be
    /// handed to every fragment.
    pub fn patch_fragment(source: &str, defs: &ShaderDefs) -> String {
        let mut patcher = SourcePatcher::new(source);
        for (name, value) in defs.iter() {
            patcher.set_if_present(name, value);
        }
        patcher.finish()
    }

    /// Every fragment's text with `defs` applied, in table order.
    pub fn patched_fragments(&self, defs: &ShaderDefs) -> Vec<String> {
        self.patched_fragments_with_edits(defs, &[])
    }

    /// Every fragment's text with `defs` applied, then any [`FragmentEdit`] for that fragment.
    pub fn patched_fragments_with_edits(
        &self,
        defs: &ShaderDefs,
        edits: &[FragmentEdit],
    ) -> Vec<String> {
        self.fragments
            .iter()
            .map(|fragment| {
                let mut text = Self::patch_fragment(fragment.source, defs);
                for edit in edits.iter().filter(|edit| edit.file == fragment.file) {
                    text = (edit.apply)(&text);
                }
                text
            })
            .collect()
    }

    /// The whole set as one WGSL module: the generated binding prelude, then every fragment.
    ///
    /// Byte-for-byte what the `concat!` blocks in `passes::dda` and `passes::cagi` produced, which
    /// `joined_source_matches_the_shipped_concatenation` pins against a recorded dump.
    pub fn joined_source(&self, defs: &ShaderDefs) -> String {
        self.joined_source_with_edits(defs, &[])
    }

    /// The same, with per-fragment edits applied first.
    pub fn joined_source_with_edits(&self, defs: &ShaderDefs, edits: &[FragmentEdit]) -> String {
        let prelude = WorldBinding::wgsl_prelude();
        let patched = self.patched_fragments_with_edits(defs, edits);
        let mut source =
            String::with_capacity(prelude.len() + patched.iter().map(String::len).sum::<usize>());
        source.push_str(&prelude);
        for fragment in patched {
            source.push_str(&fragment);
        }
        source
    }

    /// The `naga_oil` source for one fragment: its generated header, then its text unchanged.
    ///
    /// The `#import` item lists are **generated** from each provider's own declarations rather than
    /// written by hand. `naga_oil` has no wildcard import, so the alternatives were qualifying ~250
    /// call sites across the WGSL or maintaining 93 item names in headers. A generated list is
    /// neither — it cannot drift from the provider, and no shader body changed at all.
    fn oil_source(fragment: &Fragment, text: &str, all: &[(&'static str, String)]) -> String {
        let own = module_scope_items(text);
        let mut header = String::new();
        if let Some(module) = fragment.module {
            header.push_str(&format!("#define_import_path {module}\n"));
        }
        for import in fragment.imports {
            let provider = all
                .iter()
                .find(|(module, _)| module == import)
                .map(|(_, text)| text.as_str())
                .unwrap_or_else(|| panic!("{} imports unknown module {import}", fragment.file));
            let items: Vec<String> = module_scope_items(provider)
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
        format!("{header}\n{prelude}{text}")
    }

    /// Build the joined source and the composed module from the same patched fragments.
    ///
    /// Composition is what makes a wrong-direction dependency a hard error: `naga_oil` can only
    /// express a DAG, so a fragment reaching into a consumer's uniform cannot be spelled as an
    /// import. Errors name the file and line inside it, rather than an offset into 70 KB of
    /// concatenation.
    ///
    /// Panics on a composition failure. Every fragment set the app can build is composed by
    /// `tests/shader_composition.rs` across every preset, so reaching here with a broken graph
    /// means the shaders changed without that test being run — a build-time mistake, not a runtime
    /// condition to recover from.
    pub fn build(&self, defs: &ShaderDefs, edits: &[FragmentEdit]) -> ShaderProgram {
        let patched = self.patched_fragments_with_edits(defs, edits);

        // Module path -> patched text, for generating import item lists.
        let modules: Vec<(&'static str, String)> = self
            .fragments
            .iter()
            .zip(&patched)
            .filter_map(|(fragment, text)| fragment.module.map(|module| (module, text.clone())))
            .collect();

        // Validate the module's structure, but let `wgpu` decide what the DEVICE can do.
        //
        // `naga_oil`'s default capability set is the WebGPU baseline, which does not include
        // `STORAGE_TEXTURE_16BIT_NORM_FORMATS` — so composing the 10-bit and HDR output paths
        // failed here on a device that supports them perfectly well. Guessing a capability set
        // would mean a second, less informed copy of the check `create_shader_module` already
        // performs against the real adapter, and its false negatives look exactly like shader
        // bugs. The engine's actual requirements live in `voxel_color::REQUIRED_DEVICE_FEATURES`
        // and are asserted at adapter selection.
        let mut composer = naga_oil::compose::Composer::default()
            .with_capabilities(naga::valid::Capabilities::all());
        // A module must be registered after its imports. Repeatedly take whatever is satisfiable,
        // so the table's own order does not have to be topological as well as concatenation-correct.
        let mut registered: Vec<&str> = Vec::new();
        let mut remaining: Vec<(&Fragment, &String)> = self
            .fragments
            .iter()
            .zip(&patched)
            .filter(|(fragment, _)| fragment.module.is_some())
            .collect();
        while !remaining.is_empty() {
            let ready = remaining
                .iter()
                .position(|(fragment, _)| {
                    fragment
                        .imports
                        .iter()
                        .all(|import| registered.contains(import))
                })
                .unwrap_or_else(|| {
                    panic!(
                        "no fragment's imports are satisfied — the graph has a cycle among {:?}",
                        remaining
                            .iter()
                            .map(|(fragment, _)| fragment.file)
                            .collect::<Vec<_>>()
                    )
                });
            let (fragment, text) = remaining.remove(ready);
            let source = Self::oil_source(fragment, text, &modules);
            if let Err(error) =
                composer.add_composable_module(naga_oil::compose::ComposableModuleDescriptor {
                    source: &source,
                    file_path: fragment.file,
                    language: naga_oil::compose::ShaderLanguage::Wgsl,
                    ..Default::default()
                })
            {
                panic!(
                    "{} did not compose:\n{}",
                    fragment.file,
                    error.emit_to_string(&composer)
                );
            }
            registered.push(fragment.module.expect("composable module has a path"));
        }

        let (entry, entry_text) = self
            .fragments
            .iter()
            .zip(&patched)
            .find(|(fragment, _)| fragment.module.is_none())
            .expect("a composition has exactly one entry-point fragment");
        let entry_source = Self::oil_source(entry, entry_text, &modules);
        let module = match composer.make_naga_module(naga_oil::compose::NagaModuleDescriptor {
            source: &entry_source,
            file_path: entry.file,
            ..Default::default()
        }) {
            Ok(module) => module,
            Err(error) => panic!(
                "{} did not compose:\n{}",
                entry.file,
                error.emit_to_string(&composer)
            ),
        };

        ShaderProgram {
            source: self.joined_source_with_edits(defs, edits),
            module,
        }
    }
}

/// Every module-scope declaration in a WGSL source.
///
/// Column 0 is the discriminator — this codebase never indents a module-scope declaration, and
/// every `var` inside a function body is indented. Getting that wrong reports cycles that are not
/// there, because a function-local `var origin` looks like a global.
pub fn module_scope_items(source: &str) -> Vec<String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::variants::{RenderQuality, QUALITY_PRESETS};

    /// Every lever const must be declared in exactly one fragment.
    ///
    /// This is the property the whole per-fragment patching scheme rests on. Two fragments
    /// declaring the same const would both be patched — harmless by luck today, but it would
    /// mean the const has no single owner. Zero fragments declaring it means a dead lever.
    #[test]
    fn every_lever_const_belongs_to_exactly_one_fragment() {
        let defs = RenderQuality::default().shading_shader_defs();
        assert!(!defs.is_empty());
        let mut volume_defs = RenderQuality::default().volume_shader_defs();
        for (name, value) in defs.iter() {
            volume_defs.set(name, value);
        }

        // Both fragment sets together cover every fragment that exists.
        let mut all = Composition::shading().fragments;
        for fragment in Composition::volume().fragments {
            if !all.iter().any(|known| known.file == fragment.file) {
                all.push(fragment);
            }
        }

        for (name, _) in volume_defs.iter() {
            let owners: Vec<&str> = all
                .iter()
                .filter(|fragment| {
                    fragment
                        .source
                        .lines()
                        .any(|line| line.starts_with(&format!("const {name}:")))
                })
                .map(|fragment| fragment.file)
                .collect();
            assert_eq!(
                owners.len(),
                1,
                "lever const {name} is declared in {owners:?}, expected exactly one fragment"
            );
        }
    }

    /// The joined source must be byte-identical to what the two `concat!` blocks produced, for
    /// every preset. The recorded dumps are the reference; a difference here is a changed pipeline
    /// cache key and a possible changed pixel.
    #[test]
    fn joined_source_matches_the_shipped_concatenation() {
        for spec in QUALITY_PRESETS {
            let quality = spec.resolve();
            assert_eq!(
                Composition::shading().joined_source(&quality.shading_shader_defs()),
                crate::passes::dda::build_shader_source(&quality),
                "{:?}: shading joined source differs from the shipped concatenation",
                spec.preset
            );
            assert_eq!(
                Composition::volume().joined_source(&quality.volume_shader_defs()),
                crate::passes::cagi::build_shader_source(&quality),
                "{:?}: volume joined source differs from the shipped concatenation",
                spec.preset
            );
        }
    }
}
