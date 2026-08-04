# voxel-material-graph

The **recipe** half of the node system.

```
picture:  [noise] ──┐                  recipe:  1. noise
                    ├─→ [mix] → [out]           2. red
          [red]  ───┘                           3. mix step 1 and step 2
                                                4. output step 3
   ^ voxel-graph                                   ^ this crate
```

An editable `GraphAsset` is never evaluated directly. `compile()` turns it into a flat, ordered
instruction list, and **two backends read that one list** — the WGSL emitter and the CPU
preview. That is the entire reason the list exists: if each read the picture itself they would
drift, and the editor's swatch would lie about the frame.

| module | holds |
|---|---|
| `operation` | what a material node does, as a typed value |
| `nodes` | one file per node — 50 declarations |
| `declare` | the `socket!`/`node!` builders and shared field atoms |
| `contract` | what a material graph must contain |
| `lowering` | picture → instructions, then WGSL and the CPU evaluator |
| `layers` | projecting a graph onto the pattern-layer stack |
| `cacheability` | which layers can be evaluated once instead of per pixel per frame |

## No `wgpu` — and that is checkable

This crate *generates* WGSL text and never compiles a pipeline or binds a resource. The day it
needs a device, the split is in the wrong place.

**`naga` is a runtime dependency, not a dev one, deliberately.** `lowering::validate_wgsl`
parses and validates every generated function before the renderer sees it, so a bad graph fails
at compile-the-graph time with a message naming the node — instead of at pipeline creation,
where the error names a line in a 6000-line concatenation.

## How the catalogue composes

`NodeRegistry` takes **families** — `&[&[NodeDeclaration]]` — not one flat slice. That is what
makes composition real: this crate exports `NODES` and `CONTRACTS`, `voxel-rt` names them
alongside its own world family, and neither restates the other's nodes:

```rust
static FAMILIES: &[&[NodeDeclaration]] = &[voxel_material_graph::NODES, nodes::world::NODES];
```

Rust cannot concatenate `&'static` slices in a `const`, so the nesting *is* the composition.
Adding a texture or audio domain is one entry there plus its crate; nothing existing changes.

This crate also has its own narrower `CATALOGUE` (its 50 nodes + the material contract), which
is what lets it be tested without a renderer. A graph validated against `voxel-rt`'s wider one
is validated against a superset, so nothing here needs to know that exists.

## `test_support` is not `#[cfg(test)]`

`graph_with_output`, `graph_driving_roughness` and `node` are ordinary `pub` items behind
`#[doc(hidden)]`, because `voxel-rt`'s shader-assembly tests need them and a `cfg(test)` module
is invisible across a crate boundary. The alternative was a second copy of the builders in the
renderer — exactly the drift this split exists to prevent.

Four tests moved the other way, into `voxel-rt`'s `passes::dda`: they exercise the *assembled*
shader, which is the renderer's job. This crate generates a function and validates it
standalone; only there is it spliced into `world.wgsl` + the binding prelude + the environment +
the colour path. Those four were this crate's only reach back into the renderer.

```sh
cargo test -p voxel-material-graph
cargo tree -p voxel-material-graph    # no wgpu
```
