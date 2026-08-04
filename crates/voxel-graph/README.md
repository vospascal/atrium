# voxel-graph

The **picture** half of the node system: what a graph *is*, never what it *means*.

Boxes, wires, whether the wiring is legal, undo/redo, save/load. Nothing here evaluates
anything. Turning a picture into something runnable is a separate crate per domain —
`voxel-material-graph` first, then the same shape for textures, audio and animation.

```
picture:  [noise] ──┐                  recipe:  1. noise
                    ├─→ [mix] → [out]           2. red
          [red]  ───┘                           3. mix step 1 and step 2
                                                4. output step 3
   ^ this crate                                    ^ voxel-material-graph
```

## What is here, and what is deliberately not

| in the crate | stays in the catalogue crate |
|---|---|
| documents, sockets, fields, validation, history, `NodeRegistry`, `AssetId` | which nodes exist, what each one's sockets are, what a graph of a kind must contain |

2 339 lines across 11 modules, none over 540. `voxel-rt/src/graph.rs` went 6 480 → 4 839 and
is now purely Atrium's node catalogue: the 62 operations, their socket and field
declarations, `GRAPH_CONTRACTS`, and `BUILTIN_NODES`.

| module | holds |
|---|---|
| `id` | the four persisted identities |
| `socket` | value types, evaluation rate, cardinality, the feed rule |
| `field` | authored values, plus `const fn` builders so a catalogue can be a `static` |
| `declaration` | what one node type is — schema, not behaviour |
| `contract` | per-kind rules a graph must satisfy |
| `registry` | a catalogue: declarations **and** contracts, both caller-supplied |
| `document` | `GraphAsset` — the editable picture |
| `validate` | resolving a document, and `Diagnostic`s |
| `history` | edits as values |
| `asset` | durable identity |

There is no single `api.rs`, unusually for this workspace: here the contracts *are* the whole
crate, so one `api.rs` would be the catch-all file the convention forbids rather than a
boundary.

## Why `AssetId` lives here and not with the asset store

It looks misplaced. The code that reads and writes asset files is
`voxel_rt::studio_assets`, so the id "belongs" there — and that is exactly where it was.

**That one placement was the only dependency cycle in `voxel-rt`.** `graph.rs` and
`world_profile.rs` each imported precisely:

```rust
use crate::studio_assets::{AssetId, STUDIO_ASSET_SCHEMA_VERSION};
```

...while `studio_assets` imports from `graph`, `material_graph`, `material_table`,
`variants` and `world_profile`. So the store depended on the documents and the documents
depended on the store — a ten-module knot (`cagi`, `graph`, `material_cacheability`,
`material_graph`, `material_graph_layers`, `material_table`, `studio_assets`, `variants`,
`world_edit`, `world_profile`) that made every one of them unextractable, because Rust
forbids crate cycles.

Moving these two items took `voxel-rt` from that knot to **zero cycles**. Verify with:

```sh
python3 scripts/dep-cycles.py         # exits 0; --raw shows why comments must be stripped
```

The general lesson, worth carrying: an id is *more primitive* than the store that persists
it. A graph carries one, and so does every other saved thing, so putting it next to the
store makes the store a dependency of everything that merely needs to name something.

## `OperationTag`: why a node's kind is a string

A declaration says what kind of node it is. That field used to be `NodeOperation` — an enum
of every node kind in the project, 62 variants across seven families, 47 of them material.
Keeping it would have meant this crate knows what `MixColor` and `PatternLayer` are, and
adding a texture node would mean editing this crate.

It can be a label because **the mechanics never ask what an operation means.** Across all of
validation they do exactly two things with it: compare two of them, and print one into an
author-facing diagnostic (`"graph contains 2 material.output node(s), expected 1"`). Neither
needs the variants. So:

```rust
pub struct OperationTag(pub &'static str);   // "material.output"
```

The catalogue keeps its real enum and its exhaustive `match`, and converts at the boundary.
Declaration sites are untouched — the `node!` macro calls `.tag()`, so they still name a real
variant and a typo is still a compile error:

```rust
node!("material.mix_color", NodeOperation::Material(MaterialNodeOperation::MixColor), ...)
```

**What you give up, and the net:** a typo'd label compiles, and two operations could claim
one. So a catalogue owes a round-trip test —
`voxel-rt::graph::every_operation_tag_round_trips_and_is_unique` checks both directions plus
injectivity, and `every_contract_tag_names_a_known_operation` covers the contract data, where
an unknown tag would mean a rule that silently matches nothing and a graph that validates
clean while violating it.

## Known gap: five modules are mutually dependent

`scripts/dep-cycles.py crates/voxel-graph/src` reports one cycle:
`contract, declaration, document, registry, validate`. Rust permits this inside a crate, so
nothing is blocked — but by this project's own rule it means those five cannot later be split
further, and it should not be discovered by surprise.

Two edges cause it. `declaration → document` is `NodeDeclaration::new_record()`, which would
read better as `NodeRecord::from_declaration()` in `document` — a 10-call-site rename. The
real knot is `document ↔ validate`: `GraphAsset::resolve()` lives in `document` and calls
`validate`'s helpers, while `validate` needs `GraphAsset`. Fixing that means moving resolve
and validate out of the `GraphAsset` impl into free functions in `validate`. Neither was worth
doing inside the extraction that created them.

## Decisions that look wrong and are right

**Only `serde` and `serde_json`, and that is a hard constraint rather than a coincidence.** A
dependency on a voxel, a material or a shader here would make the crate unusable for the
texture and audio graphs it exists to serve. `GraphKind` already declares `Material`,
`Geometry`, `Environment`, `Biome`, `Audio`, `Animation`, `Quality` and `RenderPipeline`; the
crate is only worth extracting if it stays ignorant of all of them.
`cargo tree -p voxel-graph` is the test.

**`serde_json` is load-bearing, not a convenience.** `NodeRecord::unknown_payload` stores the
raw JSON of a node type this build does not recognise, so opening and re-saving a document
authored against a newer catalogue cannot destroy it. Graph hashing also canonicalises through
JSON.

**There is no `NodeRegistry::builtin()` and no `Default`.** This crate owns no nodes, so a
default registry could only ever have meant "somebody else's catalogue" — which is precisely
how the per-kind contracts came to be read from a hidden module-level `static` instead of
being a parameter. Both halves are now arguments to `NodeRegistry::new`, and the catalogue
names them together once (`voxel_rt::graph::CATALOGUE`).

**A `const` shadowing the type name is gone.** `pub const NodeRegistry: NodeRegistry` existed
with `#[allow(non_upper_case_globals)]`, so `NodeRegistry` was both a type and a value
depending on position. That is why `NodeRegistry::builtin()` appeared at 22 call sites.

**The id carries both a timestamp and a counter.** Neither alone suffices: two ids minted
in the same nanosecond tick would collide on the timestamp, and a fresh process restarts
the counter at 1. Pinned by `ids_are_unique_within_a_process`.

**`#[serde(transparent)]` is a compatibility contract, not a formatting preference.** Every
asset file already on disk encodes the id as a bare string. `serialises_as_a_bare_string`
exists so a future `derive` change cannot silently orphan saved projects.

**`atomic_write` in `studio_assets` no longer shares this counter.** It used to borrow
`NEXT_ASSET_ID` for temp-file suffixes — incidental reuse of an unrelated static. The two
have different jobs: an asset id is durable and ends up in a saved file, a temp suffix only
has to be unique among in-flight writes. It now has its own `NEXT_TEMPORARY_SUFFIX`.

## Working on it

```sh
cargo test -p voxel-graph
cargo tree -p voxel-graph        # must show serde and nothing else
python3 scripts/dep-cycles.py    # must stay at zero cycles
```
