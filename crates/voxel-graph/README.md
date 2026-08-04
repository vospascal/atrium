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

## Status: mid-extraction

Honest state, because a reader will otherwise wonder why a crate this small exists.

| in the crate | still in `voxel-rt` |
|---|---|
| `AssetId`, `STUDIO_ASSET_SCHEMA_VERSION` | documents, sockets, validation, history, `NodeRegistry` |

`graph.rs` (~4 000 lines of mechanics plus ~1 500 of node declarations) moves here next.
`AssetId` came first on purpose — see below.

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

## Decisions that look wrong and are right

**`serde` is the only dependency, and that is a hard constraint, not a coincidence.** A
dependency on a voxel, a material or a shader here would make the crate unusable for the
texture and audio graphs it exists to serve. `GraphKind` already declares `Material`,
`Geometry`, `Environment`, `Biome`, `Audio`, `Animation`, `Quality` and `RenderPipeline`;
the crate is only worth extracting if it stays ignorant of all of them.
`cargo tree -p voxel-graph` is the test, and it is a real one — the mechanics currently in
`graph.rs` reference five `pattern` constants (`MAX_PATTERN_LAYERS`, `MAX_NOISE_OCTAVES`,
`MINIMUM_TILE_ASPECT`, `MAXIMUM_TILE_ASPECT`, `MAXIMUM_TILE_GAP`) that must leave with the
node catalogue, not arrive with the mechanics.

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
