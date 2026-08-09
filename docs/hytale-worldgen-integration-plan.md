# Hytale-informed world-generation integration plan

**Status:** research-backed implementation plan; first node-driven terrain/cave/biome slice implemented  
**Recorded:** 2026-08-07

This document turns the useful parts of Hytale WorldGen V2's public design into
an Atrium integration plan. It is not a proposal to copy Hytale's editor or
asset formats. The useful lesson is its execution architecture: author graphs
produce persistent world data in bounded generation stages, while the renderer
consumes the result.

The terrain algorithm currently in `voxel-core` is **not** a compatibility target.
The new generator is greenfield. Existing terrain shapes, height functions,
surface recipes, and old generation code may be deleted or replaced once the
new executor has a minimal working world.

This plan extends, rather than replaces:

- [Material and biome architecture](./material-biome-architecture.md)
- [Streaming / Big Worlds](./streaming-plan.md)
- [Voxel Automata Terrain](./voxel-automata-terrain.md)
- [Voxel RT optimization ledger](./voxel-rt-optimization-ledger.md)

## Outcome

Atrium should support one deterministic authored graph system for:

- large-scale world and biome distribution;
- terrain density and cave carving;
- geology, strata, ores, and surface materials;
- sparse prop placement for vegetation, rocks, buildings, and prefabs;
- environment, tint, audio, and presentation selection;
- Studio live preview with stage- and region-scoped regeneration;
- baked material variants consumed by a small production DDA shader.

The production renderer must not execute terrain, biome, cave, or prop graphs.
Those graphs run when a region or chunk is generated. Their resolved voxel,
material, and instance data is what gets streamed and rendered.

```text
authoring graphs
      |
      v
world/biome field compiler
      |
      v
bounded generation stages
      |
      +--> voxel occupancy + material/variant IDs
      +--> placed prop instances or stamped voxel changes
      +--> environment/tint/audio metadata
      |
      v
immutable generated snapshot
      |
      v
streaming + DDA rendering
```

## What the Hytale research establishes

### Generation and rendering are separate

The Hytale node editor edits world-generation assets. Changes become visible by
generating new chunks or by regenerating a bounded viewport around the player.
The linked documentation does not describe a renderer that evaluates the entire
world graph per visible pixel.

This is the most important difference from Atrium's current material path, where
compiled material graph code and pattern area averaging can still enter the DDA
shader. Baking is therefore not merely a texture optimization; it restores the
correct execution boundary.

### The graph carries typed domains

Hytale exposes different families for density fields, material providers,
position fields, scanners, patterns, assignments, prop distributions, props,
vectors, curves, and world structure. The graph is not one undifferentiated
float network. Each domain has execution and storage semantics.

Atrium already has the corresponding foundation in `voxel-graph`:
`ScalarField`, `MaskField`, `VoxelField`, `PointField`, `SplineField`,
`BiomeField`, `BiomeDefinition`, `SurfaceProfile`, `MaterialBinding`, and
`FeatureSet` socket types. It also distinguishes `Uniform`, `PerMaterial`,
`PerVoxel`, and `PerSample` evaluation rates, independently from temporal
dependence.

### Work is staged and bounded

Hytale's performance report separates biome, terrain, prop, tint, and
environment stages. Its optimization guidance repeatedly recommends:

- use 2D fields where 3D is unnecessary;
- mask an expensive field with cheap bounds before evaluating it;
- sample expensive 3D density less frequently and interpolate;
- reuse an already generated terrain-density buffer;
- cache repeated coordinate queries;
- keep prop scan and dependency bounds small;
- measure the same seed and location over many generated chunks.

These are compiler and executor responsibilities in Atrium. Authors may expose
manual controls, but a graph compiler should infer obvious dimensionality,
common subexpressions, and safe cache points.

### Props begin from sparse candidate positions

Hytale's prop pipeline is approximately:

```text
positions -> spatial/probability filters -> scanner -> pattern
          -> assignment -> prop -> replacement mask -> world write
```

An infinite grid is scaled and jittered into sparse candidates. Density masks,
height bounds, occurrence probabilities, and clusters reduce that set further.
Only surviving candidates perform local scans and block-pattern validation.
This prevents every tree, rock, or building rule from visiting every voxel.

### Exact production cave graphs are not public in these sources

The public documents establish the positive-solid/negative-empty density model
and provide 2D/3D noise, field math, curves, warps, position fields, and graph
edges. They do not publish the exact Orbis cave networks. Cave recipes in this
plan are therefore Atrium designs built from documented primitives, not claims
about Hytale's internal production assets.

The linked Graphs System page is community-written and describes a pre-release
point-and-edge feature. Its point/edge model is useful inspiration for coherent
cave paths, roads, and rivers, but it should not be treated as a stable official
contract.

### Visual reference analysis: the supplied cave graphs

The supplied screenshots make the execution model concrete. They show named
`ExportAs`/`Imported` density assets, `SimplexNoise2D`, `Sum`, `Min`,
`Inverter`, `CurveMapper`, `BaseHeight`, and reusable manual curves. The root
Biome node exposes separate `Terrain`, `MaterialProvider`, `Props`,
`EnvironmentProvider`, and `TintProvider` outputs.

The important cave pattern is:

```text
reusable 2D cave/noise field
        -> curve mapper / remap
        -> BaseHeight context
        -> manual vertical curve
        -> invert/sum/min composition
        -> terrain density
```

This is not a full 3D noise evaluation for every voxel. A 2D X/Z field defines
where the cave or snake-like feature exists, while a curve defines its vertical
cross-section. Ceiling, wall, and floor variants can reuse the same imported
field with different base-height and curve assets. That is a major performance
lesson for Atrium.

The screenshots also imply several concrete compiler requirements:

- `ExportAs`/`Imported` are reusable field functions, not copy/paste. Compile a
  named export once per generation context and share its cache with importers.
- `Skip` should prune a disabled branch before execution, not evaluate it and
  discard the result.
- `BaseHeight` is a named world/framework context value, not just a raw Y
  constant. Atrium needs a cheap `HeightContext`/`SurfaceHeight` input that can
  be shared by terrain, cave, material, and prop stages.
- `ManualCurve` is a first-class asset. It should be constant-folded or lowered
  to a small lookup/interpolation helper, then reused by every dependent field.
- `Min`, `Max`, `Sum`, and `Invert` form a general density-composition/CSG
  vocabulary. Cave types should be authored from these primitives rather than
  hard-coded `SnakeCave` or `CavernCeiling` implementations.
- The root Biome is a stage router. Atrium's biome output should make the same
  separation explicit: terrain density, materials, props, environment, tint,
  and later audio/presentation.

The first Atrium cave prototype should therefore be a 2D-field-plus-vertical-
profile generator. A true 3D noise cave or spline-distance field should be an
extension for cases that need it, not the default path.

### Cave-specific reference: a compiled cave-node network

The additional [unofficial Hytale Server Docs cave reference](https://hytale-docs.pages.dev/modding/worldgen/caves/)
reveals a second, complementary cave architecture. It describes a cave as a
bounded tree of node instances rather than a recursive graph that is evaluated
for every voxel:

```text
CaveType
  -> entry point generator + biome/block/height masks
  -> CaveNodeType templates
  -> weighted child rules and depth limits
  -> CaveNode instances with shape/orientation/bounds
  -> compile: priority sort + chunk index
  -> populate only nodes affecting the requested chunk
```

The documented shape families include pipes, ellipsoids, cylinders, prefabs,
empty connection lines, and noise-distorted variants. Node definitions can also
carry fillings, cave covers, prefabs, environment IDs, priorities, child-count
bounds, and height conditions. The cave generator uses weighted child choices,
chance, repeat counts, orientation inheritance, and a finite child depth.

This suggests that Atrium should support two complementary cave paths:

1. **Field caves:** reusable 2D density plus vertical curves for broad,
   inexpensive snake floors, ceilings, and layered cave regions.
2. **Network caves:** a sparse compiled network of pipes, chambers, connectors,
   and cave prefabs for coherent tunnels and authored underground landmarks.

The network path should be represented internally as four stages:

- `CaveSystemDefinition`: entry distribution, biome/block/height masks, depth,
  fluid, environment, surface connection, and maximum extent;
- `CaveNodeDefinition`: shape, radius/distortion, child rules, filling, covers,
  prefab rules, priority, and height constraints;
- `CompiledCaveNetwork`: deterministic node instances with world bounds and a
  chunk-to-node index;
- `CaveChunkPopulation`: query intersecting nodes, carve/fill local voxels, and
  place covers/prefabs using replacement masks.

`SplineField` remains useful for a continuous field representation, but it is
not sufficient by itself for node children, fluids, covers, priorities, and
chunk-indexed prefab placement. Add a richer internal cave-network artifact only
when the first pipe/ellipsoid prototype has real consumers; do not expose a
public socket type prematurely.

The performance implications are especially important:

- compile the cave network once per region or generation request;
- index node bounds by chunk before population;
- never recurse through child nodes for every voxel;
- cap maximum cave extent and child count because branching grows
  exponentially with depth;
- keep prefab iteration counts bounded;
- minimize chunk overlap and use fixed seeds for reproducibility;
- sort by priority before populating overlapping geometry.

This is a stronger implementation target for connected caves than a single
monolithic `CaveNoise3D` node, while the field path remains the cheaper default.

### Beginner-guide confirmation of the minimal vertical slice

AshleyTheDev's community tutorial provides a useful sanity check for the order
of operations: a biome owns a base-height terrain, a 2D simplex field is summed
into that terrain, a material-provider queue assigns grass/dirt before stone,
and a runtime uses a jittered 2D grid plus a scanner to place tree prefabs. The
article explicitly presents this as a beginner explanation, so it is evidence
of the authoring model rather than an engine-internals specification, but it
matches the official-style references and gives us a good first prototype.

The corresponding Atrium smoke test should be:

```text
BiomeDefinition
  -> BaseHeight / vertical profile
  -> Simplex2D column field
  -> Sum density
  -> solid/air output
  -> surface material queue (grass -> dirt -> stone)
  -> jittered PointField
  -> surface Scanner
  -> tree Feature/Prefab placement
```

### Cellular-automata cave recipe

The [RogueBasin cellular-automata cave method](https://www.roguebasin.com/index.php/Cellular_Automata_Method_for_Generating_Random_Cave-Like_Levels)
is a better fit for the first connected cave prototype than evaluating one
independent 3D noise sample per voxel. It seeds a bounded map with roughly 40%
walls, applies the 4–5 neighbour rule, follows with a few stricter smoothing
passes, and uses a larger-radius cleanup rule to avoid huge open rooms. The
article also calls out the disconnected-region problem and recommends flood
fill/keep-largest-region or explicit connections.

Atrium now exposes this as `Cellular Automata` on `world.cave_carver`. The CPU
backend builds a globally aligned 64x64 field, so adjacent chunks share the
same smoothed cells. Each depth band performs region analysis, connects
sizeable secondary rooms back to the main region with bounded corridors, and
removes isolated pockets.

### Cave-system reference: Seb Lague's cavern generator

The [Seb Lague Procedural Cave Generation repository](https://github.com/SebLague/Procedural-Cave-Generation)
is the stronger reference for a playable cave system rather than a random hole
field. Its tutorial series separates map generation, region analysis, removal
of small disconnected regions, and room/corridor connection. That suggests the
next Atrium cave IR should publish regions and connection candidates before
voxel carving:

```text
seeded CA field
  -> region flood fill
  -> remove tiny regions / keep largest
  -> connect selected regions with bounded corridors
  -> layered 3D cave mask
  -> cave material and prop passes
```

Our current `Cellular Automata` mode performs those pieces per globally aligned
depth band. It does not yet author corridor links as graph data or expose a
region graph; those belong in a separate `Cave Connectivity` node rather than
being hidden inside the noise node. The generated corridors are intentionally
an interim runtime safety net until that node has a real consumer.

This is intentionally a new generator, not a port of the existing island
algorithm. It tests the new field cache, biome-owned terrain, material staging,
and sparse prop execution in one small world.

## MiEx source-code findings: the export boundary we should copy

The [MiEx repository](https://github.com/BramStoutProductions/MiEx) is not a
replacement for the Hytale generator research. It is a Java world exporter and
USD scene builder. That distinction matters: its strongest ideas apply after
Atrium has generated a snapshot, not inside the production DDA shader. The
source observations below were checked against commit `7e77500` (2026-07-10).

### 1. Make generated regions independently loadable

MiEx writes a manifest plus separate export-chunk payload files. The exporter
enumerates enabled bounds, schedules each chunk as an independent task, and
then writes a second conversion stage that turns those payloads into USD files
and payload references. This is the same seam Atrium needs between generation
and rendering:

```text
world graph -> generated snapshot -> region artifact -> streaming/render view
```

Atrium should add a private `GeneratedRegionArtifact` (name can change) with:

- region/chunk coordinates and bounds;
- material/variant IDs and atlas references;
- sparse prop instances and block-instance records;
- optional LOD summaries and cave-visibility metadata;
- a version/hash of the graph and assets that produced it.

The renderer then consumes an artifact without knowing how terrain, caves, or
noise were generated. Studio can replace one artifact while the previous one
remains visible.

Relevant implementation references: [`Exporter.java`](https://github.com/BramStoutProductions/MiEx/blob/7e77500dafdcfb4876c9d52d1b11feb76faf4645/src/nl/bramstout/mcworldexporter/export/Exporter.java),
[`ExportData.java`](https://github.com/BramStoutProductions/MiEx/blob/7e77500dafdcfb4876c9d52d1b11feb76faf4645/src/nl/bramstout/mcworldexporter/export/ExportData.java), and
[`ChunkExporter.java`](https://github.com/BramStoutProductions/MiEx/blob/7e77500dafdcfb4876c9d52d1b11feb76faf4645/src/nl/bramstout/mcworldexporter/export/ChunkExporter.java).

### 2. Treat detail reduction as a generated representation, not a shader branch

MiEx's exporter chooses a level per sampled block and has a four-level cache for
sizes 2, 4, 8, and 16, with padded neighboring chunks. Its cave-removal path
also uses a coarse cache (8x8x8 cells per 16-block section) and checks borders
before filling holes. The transferable rule is not the exact cache layout; it
is that far/background detail is reduced while building the artifact.

For Atrium, implement this as explicit artifact tiers:

| Tier | Stored output | Intended use |
| --- | --- | --- |
| `Full` | voxel occupancy, material/variant IDs, nearby cave/prop detail | DDA and Studio focus area |
| `Reduced` | coarser occupancy/material summary, no small decoration | far background and low-memory machines |
| `Silhouette` | surface/height/material summary only | distant horizon or preview |

The DDA should select a tier by region/camera policy; it should not evaluate
noise again to decide the tier. Cave visibility is a generation/export policy:
for a background tier, remove internal cave surfaces or replace them with a
closed shell only when the selected view cannot expose them.

References: [`LODCache.java`](https://github.com/BramStoutProductions/MiEx/blob/7e77500dafdcfb4876c9d52d1b11feb76faf4645/src/nl/bramstout/mcworldexporter/export/LODCache.java),
[`CaveCache.java`](https://github.com/BramStoutProductions/MiEx/blob/7e77500dafdcfb4876c9d52d1b11feb76faf4645/src/nl/bramstout/mcworldexporter/export/CaveCache.java), and the
[`ChunkExporter` LOD/cave path](https://github.com/BramStoutProductions/MiEx/blob/7e77500dafdcfb4876c9d52d1b11feb76faf4645/src/nl/bramstout/mcworldexporter/export/ChunkExporter.java).

### 3. Use a processor chain after generation

MiEx does not mix all optimization policy into block sampling. It constructs a
mesh processor chain and can apply ray-tracing partitioning, face merging, and
writing in sequence. This maps well to an Atrium artifact pipeline:

```text
resolved snapshot
  -> visibility/cave reduction
  -> material/variant compaction
  -> optional mesh/preview extraction
  -> artifact writer
```

The face optimizer is relevant to a future meshed renderer or Studio preview,
not to the voxel DDA itself. The ray-tracing optimizer is also renderer-specific:
it builds a BVH-like partition to reduce empty bounding-box space. We should not
claim its reported ray-tracer speedup transfers to Vulkan DDA without a
benchmark. The general lesson does transfer: spatially partition sparse detail
so a ray/query rejects whole groups cheaply.

References: [`MeshProcessors.java`](https://github.com/BramStoutProductions/MiEx/blob/7e77500dafdcfb4876c9d52d1b11feb76faf4645/src/nl/bramstout/mcworldexporter/export/processors/MeshProcessors.java),
[`FaceOptimiser.java`](https://github.com/BramStoutProductions/MiEx/blob/7e77500dafdcfb4876c9d52d1b11feb76faf4645/src/nl/bramstout/mcworldexporter/export/processors/FaceOptimiser.java), and
[`RaytracingOptimiser.java`](https://github.com/BramStoutProductions/MiEx/blob/7e77500dafdcfb4876c9d52d1b11feb76faf4645/src/nl/bramstout/mcworldexporter/export/processors/RaytracingOptimiser.java).

### 4. Keep material identity small: atlas plus modifiers

MiEx's atlas creator packs textures, repeats/pads them, and rewrites faces to
refer to atlas coordinates. During export, biome tint, face color, normals, and
other modifiers are resolved before the mesh is written. This supports the
material strategy already chosen for Atrium:

```text
base material + bounded variant selector + biome/tint modifiers
       -> baked 16x16 tile/atlas entry
       -> compact material ID in generated voxels
```

Do not create one GPU material for every noise result. Keep a bounded variant
set per material (for example the proposed 20 variants), store only a compact
selector per voxel/face, and invalidate only the affected atlas entries during
Studio editing. Atlas packing is a production optimization; Studio can keep a
temporary uncached preview for the material being edited.

References: [`AtlasCreator.java`](https://github.com/BramStoutProductions/MiEx/blob/7e77500dafdcfb4876c9d52d1b11feb76faf4645/src/nl/bramstout/mcworldexporter/atlas/AtlasCreator.java),
[`GeneratedTextures.java`](https://github.com/BramStoutProductions/MiEx/blob/7e77500dafdcfb4876c9d52d1b11feb76faf4645/src/nl/bramstout/mcworldexporter/export/GeneratedTextures.java), and
[`Modifier.java`](https://github.com/BramStoutProductions/MiEx/blob/7e77500dafdcfb4876c9d52d1b11feb76faf4645/src/nl/bramstout/mcworldexporter/modifier/Modifier.java).

### 5. Separate sparse instances from dense terrain

MiEx writes selected blocks as individual instances and keeps point locators and
entities separate from the dense mesh. This is the right model for trees,
rocks, buildings, and interactive props in Atrium: generate a sparse instance
record first, then stamp voxel changes only for props that declare a physical
voxel footprint. Do not inflate the dense voxel buffer for every decorative
object.

This also gives us a clean live-edit path: replacing one prop asset can update
instance records without regenerating terrain or rebaking unrelated materials.
The relevant data is assembled in [`ChunkExporter.java`](https://github.com/BramStoutProductions/MiEx/blob/7e77500dafdcfb4876c9d52d1b11feb76faf4645/src/nl/bramstout/mcworldexporter/export/ChunkExporter.java)
and written by [`Exporter.java`](https://github.com/BramStoutProductions/MiEx/blob/7e77500dafdcfb4876c9d52d1b11feb76faf4645/src/nl/bramstout/mcworldexporter/export/Exporter.java).

### 6. Use deterministic coordinate noise only for cheap variation

MiEx uses a small preloaded noise table and coordinate-based lookups for model
selection, vegetation offsets, animation offsets, and random texture choices.
That is useful evidence for the proposed bounded variation system: derive a
stable variant index from `(world seed, material ID, integer coordinate, salt)`
and use it to select one of the pre-baked tiles. It avoids evaluating a large
noise graph in the DDA while preserving spatially stable variation.

The table itself is not a generator cache. Atrium should still cache expensive
field outputs at the graph's inferred evaluation rate; use coordinate hashing
only for the final inexpensive selector.

References: [`Noise.java`](https://github.com/BramStoutProductions/MiEx/blob/7e77500dafdcfb4876c9d52d1b11feb76faf4645/src/nl/bramstout/mcworldexporter/export/Noise.java) and
[`BakedBlockState.java`](https://github.com/BramStoutProductions/MiEx/blob/7e77500dafdcfb4876c9d52d1b11feb76faf4645/src/nl/bramstout/mcworldexporter/model/BakedBlockState.java).

### 7. Cache source chunks with bounded lifetime and memory-aware workers

MiEx keeps loaded chunks in a bounded cache, unloads old data in a background
collector, and sizes its worker pool from both available processors and a
per-thread memory budget. The latter is particularly relevant to older Windows
machines: parallel generation is only faster while it fits memory and does not
thrash.

Atrium should expose a generation memory budget, cap concurrent region jobs,
and evict snapshot/field caches by byte size rather than by count alone. A
benchmark should record cache hit rate, peak resident bytes, queue latency, and
frame time on the older Vulkan machine.

References: [`Chunk.java`](https://github.com/BramStoutProductions/MiEx/blob/7e77500dafdcfb4876c9d52d1b11feb76faf4645/src/nl/bramstout/mcworldexporter/world/Chunk.java) and
[`ThreadPool.java`](https://github.com/BramStoutProductions/MiEx/blob/7e77500dafdcfb4876c9d52d1b11feb76faf4645/src/nl/bramstout/mcworldexporter/parallel/ThreadPool.java).

### MiEx conclusions for Atrium

Adopt now:

1. an immutable, independently loadable generated-region artifact;
2. explicit full/reduced/silhouette detail tiers;
3. a post-generation processor chain;
4. bounded material variants plus atlas entries;
5. sparse prop instances separate from dense voxels;
6. deterministic coordinate-based variant selection;
7. byte-budgeted caches and worker concurrency.

Do not copy directly:

- USD payload/file formats;
- Java's mesh/BVH implementation into the voxel DDA;
- MiEx's exporter noise table as a substitute for graph-field caching;
- its cave-removal heuristic as the authoritative world-generation cave model.

The next implementation seam should be the generated-region artifact and a CPU
writer/reader for one minimal biome world. Once that artifact is stable, add
the baked material atlas and reduced-detail tier before expanding the cave and
prop node catalogue.

## Current Atrium foundation and the real gap

Atrium already has more of this design than the current world output suggests:

- `crates/voxel-graph/src/socket.rs` has the required field and asset socket
  categories plus spatial evaluation rates.
- `crates/voxel-rt/src/biome.rs` classifies environment fields into weighted
  biome influences and selects reusable surface, palette, feature, audio, and
  animation profiles.
- `crates/voxel-rt/src/world_profile.rs` already orders features into
  `Carving`, `Geology`, `Surface`, `Vegetation`, `Decoration`, and
  `PostProcess` stages.
- `crates/voxel-rt/src/graph/nodes/world/` already declares generated terrain,
  composition, preview, and world-output nodes.
- `crates/voxel-rt/src/world_profile_runtime.rs` is a real consumer for physical
  surface-layer projections before GPU upload.
- `crates/voxel-material-graph/src/cacheability.rs` already derives material
  cacheability from graph declarations instead of a node-name allowlist.

The first greenfield slice now also includes:

- signed, replayable chunk generation in `voxel-core`;
- a node-authored `world.cave_carver` with shape, height, scale, threshold, and
  salt properties;
- a `biome.noise` node for reusable deterministic scalar fields;
- a `biome.cellular_automata` node for bounded biome-cell propagation;
- a real Biomes tab whose starter graph is
  `Biome Noise -> Biome Sampler -> Biome Blend -> Cellular Automata -> Output`.

The graph is the authoring source of truth. The runtime has a deliberately small
CPU backend: cave-carver properties compile into `CaveProgram`, and the Biomes
Update World action now lowers the cellular-automata controls into that same
program before rebuilding the visible world. Full typed biome-field execution
and material/prop selection remain the next lowering seams.

The remaining gap is broader execution. `CompiledWorldProgram` validates the
active World graph and lowers the current terrain/cave slice, but it does not
yet lower a complete typed world graph into all ordered generation stages.
Feature requests are resolved, but biome delimiters, material providers, and
prop generator graphs do not yet have full world-generation backends.

The old `VoxelWorld::generate` implementation is not part of the target
architecture. It may be used temporarily as a viewer/test fixture while the new
executor is being built, but no new feature should be designed around its
heightmap, island cutoff, material recipe, or monolithic generation flow.

That executor is the central integration seam. Do not create a second biome
model or a separate Hytale-like graph system beside the existing one.

## Architectural decisions

### D1 — Generated voxels remain authoritative

Noise, SDFs, splines, and curves are generators. Their output is written into
the voxel world/brickmap. DDA traverses generated occupancy and reads resolved
material data; it does not re-evaluate the generator.

### D2 — Compile authoring graphs into a generation program

The saved graph remains canonical. Compilation should produce a private,
backend-friendly DAG/IR with:

- resolved typed operations;
- topological order;
- constant folding;
- common-subexpression identity;
- spatial dimensionality (`2D column`, `3D voxel`, `surface`, or `sparse`);
- required neighbor/apron bounds;
- cache and sampling policy;
- deterministic seed/salt information;
- stage ownership and dependency metadata.

Start with one honest CPU implementation behind private modules. Extract a new
crate or introduce backend traits only when a second executor actually exists
or the measured blast radius requires the boundary.

### D3 — Generation follows an explicit stage order

The initial order is the existing `FeatureStage` order:

1. base biome and environmental fields;
2. base terrain density and occupancy;
3. `Carving` — caves, tunnels, ravines;
4. `Geology` — strata, ore bodies, rock replacement;
5. `Surface` — soil, grass, sand, snow, cave-wall roles;
6. `Vegetation` — trees, bushes, large organic features;
7. `Decoration` — stones, flowers, small prefabs;
8. `PostProcess` — explicitly bounded cleanup or derived attributes;
9. publish the immutable generated snapshot.

Tint, environment, audio, and material presentation are projections of the
same resolved biome context, but they do not mutate terrain unless their
declared update class says they are a physical generation operation.

### D3a — Each stage is a node-driven pass

The Hytale screenshots and the multi-pass description point to a useful rule:
the graph exposes pass boundaries, while each pass can contain field math,
noise, random selection, or cellular automata. The first Atrium chain is:

```text
Biome Definition
  -> Biome Noise
  -> Biome Sampler
  -> Biome Blend
  -> Biome Cellular Automata
  -> World Terrain
  -> Cave Carver
  -> Surface/Material Composer
  -> Output snapshot
```

The cellular-automata node is bounded to the generation region. Its random input
is a deterministic coordinate field, so propagation is stable for a world seed
and independent of worker order. The same pattern can later drive vegetation
and decoration: a cheap noise node creates candidate cells, masks/scanners
reject most candidates, and a sparse placement pass writes only survivors.

Compilation lowers each pass into a CPU generation stage with explicit cache
scope (`column`, `voxel`, or `sparse`), then publishes an immutable snapshot for
the DDA. The renderer never evaluates these nodes per pixel.

### D4 — Biomes select reusable programs

A biome should continue to interpret environmental fields and reference:

- a terrain/density program;
- a material palette and surface profile;
- reusable carving/geology/prop feature sets;
- environment, tint, audio, and animation profiles.

A biome does not own material internals or duplicate cave algorithms. For
example, several underground biomes can reuse `karst-caves` with different
parameters and cave-wall palettes.

### D5 — Determinism is independent of scheduling

Every stochastic decision derives from:

```text
world seed + integer world/anchor coordinate + stable operation salt
```

Results must not depend on thread order, chunk arrival order, scanner traversal
order, or which neighbor generated first. Cross-chunk features are owned by a
canonical anchor chunk and generated with the required apron.

### D6 — Studio live editing swaps complete snapshots

Studio may evaluate live graphs, but editing must never expose a half-generated
world to the renderer. A change produces a new bounded snapshot asynchronously;
the previous snapshot remains active until the replacement is ready.

An edit invalidates only its dependent stages and regions:

| Edit | Minimum invalidation |
| --- | --- |
| Material color/pattern | That material's baked atlas region |
| Surface material rule | Surface stage in affected preview chunks |
| Prop asset | Instances of that prop or its owning feature stage |
| Prop distribution | Owning prop stage in affected regions |
| Cave parameters | Carving and every later physical stage |
| Biome detail graph | That biome's dependent chunks |
| World biome distribution | Biome classification and all dependent stages |

### D7 — Production material graphs bake by default

Static material noise and area averaging produce final 16x16 color/property
tiles during baking. Generation stores a material ID plus a variant selector.
The production DDA performs a direct atlas lookup.

Studio may keep a separate live-material preview path. Normal world startup,
especially on older Vulkan implementations, must not create or prewarm the
large live-material pipeline when only baked materials are active.

## Planned execution artifacts

| Artifact | Domain | Lifetime | Typical contents |
| --- | --- | --- | --- |
| Column field | X/Z | chunk/region cache | biome weights, height, temperature, humidity |
| Scalar field | X/Y/Z | one generation stage | terrain or cave density |
| Mask field | 2D or 3D | one or more dependent stages | biome, depth, cave, replacement masks |
| Voxel field | X/Y/Z integer cells | generated snapshot | occupancy, material ID, variant seed/ID |
| Point field | sparse anchors | prop stage | tree, rock, prefab candidates |
| Spline field | sparse connected paths | world/region cache | cave tunnels, roads, rivers |
| Placed prop list | sparse instances | generated snapshot or stamp queue | asset, transform, stable ID, bounds |
| Stage report | per generated request | diagnostics/history | time, samples, cache hits, output bounds |

`Placed prop list` may require a new graph socket type once both assignment and
placement nodes exist. Do not publish the type before those real consumers are
implemented.

## First node families

The list is deliberately smaller than Hytale's catalogue. Each family earns
extensions through real authored worlds.

### Scalar and mask fields

- Constant scalar/mask
- World position X/Y/Z
- Simplex/fBm 2D
- Simplex/fBm 3D
- Add, multiply, min, max, absolute, clamp
- Smooth min/max where blending is required
- Curve/remap
- Range mask and invert
- Mix selected by a cheap mask
- Sample Y and interpolate
- Explicit cache/share node only where inference is insufficient
- Imported reusable field

The compiler should automatically recognize a field independent of Y and cache
it per X/Z column. A 3D domain warp must remain marked 3D so it cannot silently
reuse an invalid 2D cache.

### Biome/world structure

- Environment field source
- Biome definition/profile reference
- Biome delimiter/selector
- Biome blend
- Default biome
- World structure output

The current biome resolver allows multiple weighted influences. Initially
benchmark a default of two active terrain influences in transitions while
preserving the existing hard capability for more. Every additional active
terrain biome can require another density evaluation.

Surface and underground biome classification should be separable. A surface
desert may contain several underground cave biomes selected by depth, moisture,
geology, and cave-system identity. Atrium now has an explicit
`BiomeType::{Surface, Cave}` classification for routing, while the existing
`Underground` trait remains a broader descriptive tag.

### Cave/carving

- Density-to-carving-mask
- Depth/surface-distance limiter
- Biome mask
- Ridged or absolute 3D-noise cave field
- Spline distance field
- Chamber/ellipsoid field
- Union/intersection/subtraction through field math
- Carving output

### Sparse positions

- Square or triangular 2D grid
- 3D grid where genuinely necessary
- Scale
- Deterministic jitter
- Cluster around distributor anchors
- Bound to region
- Height/depth restriction
- Density/biome mask
- Occurrence probability
- Offset/vector offset
- Union
- Cache/share

### Scanners and placement patterns

- Origin
- Linear X/Y/Z scan
- Radial scan
- Ordered scanner queue
- Block/material or semantic-role check
- Floor, ceiling, wall, and surface transition
- Gap/clearance volume
- Boolean and/or/not
- Density/biome condition, evaluated last after cheap spatial tests

### Assignment and props

- Constant assignment
- Weighted deterministic assignment
- Height/density/biome assignment
- Skip chance
- Manual voxel prop
- Prefab prop
- Cuboid/primitive prop for testing
- Static and weighted rotation
- Offset
- Union/sequence
- Placement cap
- Replacement/block mask

## Cave generation recipes

### Recipe A — inexpensive noise caves

```text
Simplex3D
  -> Absolute/Ridged
  -> Curve/Range Mask
  -> Depth Mask
  -> Biome Mask
  -> Carving Output
```

Treat the result as a normalized `cave_presence` mask. The final occupancy rule
is conceptually:

```text
solid = base_terrain_density > 0 && cave_presence < carve_threshold
```

Using an explicit normalized mask avoids sign ambiguity between density/SDF
implementations. Noise caves are cheap and local but can look disconnected or
worm-like without careful remapping.

### Recipe B — coherent spline caves

```text
sparse underground anchors
  -> proximity/graph connections
  -> spline paths
  -> distance-to-spline field
  -> radius curve + 3D noise distortion
  -> depth/biome mask
  -> Carving Output
```

This supports connected tunnels, entrances, authored landmarks, and later
navigation analysis. Generate/store the coarse spline network per region, then
sample only the pieces overlapping each chunk plus its apron.

### Recipe C — cave systems with chambers

Combine spline tunnels with sparse ellipsoid chambers. Weighted assignments can
select chamber types, while local fields vary radius and wall roughness. The
same stable cave-system ID can feed underground biome selection, ore placement,
audio, and props.

### Minecraft comparison: seed identity and layered caves

Minecraft confirms that the replay key should be a persisted world **seed**, not
an opaque hash of generated blocks. A 64-bit seed reproduces the same world only
when the generator version, world settings, and authored data are also the same.
Atrium now stores a `u64 world_seed` in `ProjectManifest`; older projects
default to seed `1`. The future greenfield generator should consume the full 64
bits and derive stable per-stage/per-coordinate hashes from it.

Minecraft's modern cave system is layered rather than one monolithic cave node:

```text
terrain density
  + cheese cavern field
  + spaghetti tunnel field
  + noodle tunnel field
  + legacy carvers/ravines
  + aquifer water/lava field
  -> cave mask + fluid state + cave biome context
```

That is a good target for Atrium. Keep `BiomeType::Cave` as the routing
classification, then let each cave biome select reusable field programs,
network caves, fluids, and decoration. The [official Minecraft 1.18 notes](https://feedback.minecraft.net/hc/en-us/articles/4415128577293-Minecraft-Java-Edition-1-18)
describe the three noise-cave forms, their combination with older carvers, and
aquifers as the source of local underground water levels.

### Derived cave context

Carving should publish inexpensive derived facts for later stages instead of
forcing them to recompute cave density:

- distance to cave;
- cave-system stable ID;
- cave surface normal/exposure where required;
- floor/ceiling/wall classification;
- connected/open-to-surface flag if available.

This enables features such as a surface tree that marks a cave below it:

```text
forest candidates
  -> surface scanner
  -> valid soil/clearance pattern
  -> downward cave-context check
  -> special-tree assignment
  -> placement mask
```

## Prop generation and cross-chunk rules

### Complete prop flow

```text
jittered sparse grid
  -> biome/density/height filter
  -> occurrence filter
  -> surface scanner
  -> floor/ceiling/wall/clearance pattern
  -> weighted assignment
  -> orientation and offset
  -> replacement mask
  -> placed instance or voxel stamp
```

Cheap predicates must precede expensive ones. A density noise test should not
run for every possible voxel when a height bound or floor check can reject most
candidates first.

### Canonical ownership

Large props cross chunk boundaries. To remain deterministic:

1. derive candidate anchors from world coordinates, not chunk-local indices;
2. assign each anchor to exactly one owner chunk;
3. query anchors from the owner chunk plus the maximum prop/scanner apron;
4. sort accepted props by stable stage, priority, and anchor ID;
5. apply replacement masks in that canonical order;
6. clip writes to the requested output region while preserving neighboring
   context for validation.

This generalizes the canonical tree/apron approach already recorded in the
streaming plan.

### Replacement policy

Every prop write sees both source and destination semantics. The minimum policy
supports:

- do not place selected source roles/assets;
- do not replace protected destination roles/assets;
- source-specific allowlists for replaceable destinations;
- explicit behavior for air, fluid, terrain, vegetation, and authored/player
  voxels.

Player-authored edits should be protected by default during regeneration unless
the user explicitly requests destructive world rebuilding.

## Material baking integration

The material and world plans meet at surface/geology assignment:

1. Material Studio compiles the graph and identifies static/cacheable layers.
2. The baker evaluates pattern noise and area averaging into final atlas tiles.
3. A configurable unique-layer budget expands independent variants, capped by
   the Material Output policy.
4. The world generator selects a deterministic variant using material, world
   seed, block/brick coordinate, and face category.
5. Generated world data stores a compact variant selector or stable variant
   seed, depending on the chosen packing.
6. Production DDA reads the final tile directly.

For the proposed face budgets, top, bottom, and side variants are separate
lists, not a `top x bottom x side` cross-product. Only one face category is read
for a hit.

`PatternFrame::World` needs an explicit authoring decision:

- **baked variants:** discrete per-block stochastic appearance;
- **baked world cells:** preserve more spatial continuity at greater storage
  cost;
- **live Studio/fallback:** retain exact continuous evaluation where required.

The UI must label the semantic change instead of implying that a discrete
variant bake is mathematically identical to continuous world-space noise.

## Studio workflow

### Preview scopes

- **Material preview:** regenerate one material atlas region.
- **Prop preview:** regenerate one feature in a bounded test volume.
- **Biome viewport:** regenerate dirty stages in a radius around the camera.
- **World build:** generate or stream normal regions with production settings.

### Update behavior

- Debounce rapid graph edits.
- Cancel obsolete queued generations.
- Generate off the render thread through the existing world-host direction.
- Keep the last valid snapshot if compilation or generation fails.
- Upload completed brick/material deltas atomically.
- Show dirty, generating, ready, and failed states in Studio.
- Permit an explicitly slower live path while editing one material/world asset.

### Diagnostics shown on nodes/outputs

- inferred domain: uniform, 2D column, 3D voxel, surface, or sparse;
- update class and invalidation scope;
- estimated/sample-count cost;
- cacheable/shared/live state;
- required neighbor/apron radius;
- variant expansion count and atlas bytes;
- warnings for 3D warp, gradients, broad transitions, or large prop bounds;
- stage timing from the most recent preview generation.

## Performance and correctness gates

Every slice below must have a headless deterministic test and a measured gate.
Do not optimize from screenshots or compare Hytale video frame rates to Atrium's
DDA timing.

### Required stage report

For each generated region/chunk, record:

- total and per-stage wall time;
- coordinates/voxels/candidates sampled;
- output and apron dimensions;
- cache hits/misses and shared-field evaluations;
- accepted/rejected prop candidates by reason;
- voxels changed per feature;
- generated bytes and upload bytes;
- cold-start separately from warmed generation.

Benchmark the same seed, coordinates, view/build radius, and machine. Aggregate
enough chunks to stabilize the measurement; Hytale's guidance recommends at
least 500 chunks for its own reports, which is a useful long-run benchmark size
for Atrium once streaming generation exists.

### Correctness invariants

- identical output regardless of worker count or scheduling;
- identical border voxels regardless of which chunk requests them;
- no graph cycle or missing asset reaches execution;
- no prop is duplicated because two chunks see the same anchor;
- replacement masks protect authored/player data;
- stage invalidation produces the same final result as a clean full build;
- renderer output references only complete generated snapshots;
- production DDA contains no terrain/biome/prop graph evaluator;
- baked-only startup does not create the live material pipeline.

## Implementation slices

### S0 — Greenfield generator contract

- [ ] Define a bounded generation request and immutable outcome internally.
- [ ] Define the new world coordinate, chunk, apron, seed, and stage contracts.
- [ ] Add a headless test volume and stable digest of generated output.
- [ ] Record renderer/DDA baselines independently of the old terrain algorithm.
- [ ] Make stage order and dependency invalidation a pure tested decision.
- [ ] Add a minimal generated world from the new field executor, even if it is
      initially only a simple density field.

**Gate:** the new generator produces a deterministic, seam-free minimal world.
No byte-for-byte comparison with the legacy terrain is required or desired.

### S1 — Scalar/mask field compiler and CPU executor

- [ ] Lower a minimal typed DAG: constants, coordinates, 2D/3D noise, math,
      curve/remap, mask, mix.
- [ ] Infer 2D column fields and cache them.
- [ ] Add explicit sampled-Y interpolation for expensive 3D branches.
- [ ] Deduplicate shared subgraphs within one generation context.
- [ ] Emit per-node/sample and per-stage metrics.

**Gate:** deterministic, seam-free field samples across adjacent requests;
measured reuse for a shared 2D field; no renderer/shader integration. The new
field output becomes the source of truth for terrain shape.

### S2 — World structure and biome field

- [ ] Compile environmental fields into `BiomeField` samples.
- [ ] Resolve default biome, delimiters/selectors, priority, and transitions.
- [ ] Cache column biome influences for all downstream stages.
- [ ] Reuse the existing `BiomeRegistry`, profiles, and palettes.

**Gate:** a headless biome map is deterministic, transitions normalize, and
downstream stages reuse the same cached sample rather than resolving again.

### S3 — Cave carving vertical slice

- [ ] Implement noise-cave mask, depth mask, biome mask, and carving output.
- [ ] Apply `Carving` before geology/surface stages.
- [ ] Publish derived cave context for downstream consumers.
- [ ] Add one bounded Studio cave preview.
- [ ] Prototype a compiled cave network with one pipe and one ellipsoid node,
      chunk bounds, deterministic child depth, and local population.
- [ ] Add distorted shapes and cave prefabs only after node-count and chunk-cost
      measurements exist.

**Gate:** caves cross chunk/request borders without seams; disabling the feature
is byte-identical to base terrain; carving time is present in the stage report.

### S4 — Geology, surfaces, and material variants

- [ ] Apply depth/exposure rules for surface, subsurface, rock, and cave walls.
- [ ] Add strata/ore replacement as bounded geology features.
- [ ] Connect resolved material roles to baked atlas material/variant IDs.
- [ ] Regenerate only affected stages/material regions after edits.

**Gate:** a production world renders through atlas lookup without live pattern
noise or area averaging in DDA.

### S5 — Sparse prop generation

- [ ] Implement grid, scale, jitter, bound, density mask, and occurrence fields.
- [ ] Implement surface/linear scanners and floor/ceiling/wall patterns.
- [ ] Implement constant and weighted assignment.
- [ ] Place one manual voxel prop and one real prefab.
- [ ] Add replacement masks, stable ownership, ordering, and apron handling.

**Gate:** trees/rocks are identical across generation order and worker count;
no duplicates or clipped cross-border props; rejected-candidate metrics identify
where prop time is spent.

### S6 — Biome-aware cave and prop composition

- [ ] Select reusable carving and prop feature sets through biome influences.
- [ ] Support underground biome selection without duplicating surface biomes.
- [ ] Implement one cross-stage heuristic, such as a cave-marker tree.
- [ ] Cap and measure work in biome transition regions.

**Gate:** changing one biome's feature set regenerates only affected chunks and
dependent stages; clean and incremental builds have identical digests.

### S7 — Studio invalidation and snapshot swapping

- [ ] Compile dependency/invalidation metadata from graph outputs.
- [ ] Run generation asynchronously and cancel stale jobs.
- [ ] Preserve the last valid snapshot on errors.
- [ ] Display node domain, stage, cache, bounds, and measured timing.
- [ ] Keep live material/world preview paths explicit and opt-in.

**Gate:** rapid edits do not stall or corrupt the renderer; a material-only edit
does not rebuild terrain; a cave edit rebuilds carving and later stages only.

### S8 — Streaming integration and optimization

- [ ] Route generated chunks through the existing `VoxelSource`/streaming seam.
- [ ] Add chunk/region caches with validated keys and bounded memory.
- [ ] Prioritize nearby chunk jobs and discard obsolete distant work.
- [ ] Consider a compute executor only after CPU stage measurements justify it.

**Gate:** stable generation delivery around a moving camera, bounded cache
growth, no seams, and no regression to the renderer's entity/draw-call wins.

## Known risks

- **Biome transitions multiply work.** Blending two terrain biomes can require
  two density evaluations; broad transitions must be explicit and measured.
- **3D warping destroys cheap reuse.** Domain changes can invalidate column
  caches and widen dependency bounds.
- **Gradients resample fields.** Cache/reuse terrain or cave buffers instead of
  importing and recomputing their roots from material/prop logic.
- **Large props expand every dependency stage.** Scanner, pattern, and content
  bounds all contribute to the required apron.
- **Incremental generation is easy to make incorrect.** Every incremental result
  must be checked against a clean full-build digest.
- **Player edits need ownership.** Regeneration must distinguish generated voxels
  from protected authored/player changes.
- **World-continuous material patterns do not automatically survive variant
  baking.** The author chooses discrete variants, a larger spatial bake, or the
  live fallback.
- **GPU execution can recreate the original problem.** A monolithic compute
  worldgen shader would again stress older Vulkan compilers. Begin with small
  specialized CPU operations and only move measured kernels.

## Open decisions to resolve with prototypes

- Generation request/chunk dimensions and maximum apron.
- Whether spline networks are cached by region, zone, or another coarse cell.
- Default active biome influence count in terrain transitions.
- Material variant packing: stored ID versus deterministic stored seed.
- Representation of placed props before voxel stamping or instanced rendering.
- Persistence policy for generated chunks versus deterministic regeneration.
- Cave connectivity/nav analysis and whether it is generation metadata.
- Which fields deserve persisted caches versus request-local caches.
- When a second executor justifies a dedicated `voxel-worldgen` crate.

## External research resources

### Hytale/Hypixel documentation mirrored by HytaleModding

- [Worldgen tutorial index and biome editing](https://hytalemodding.dev/en/docs/official-documentation/worldgen/worldgen-tutorial/README)
- [Density generation concepts](https://hytalemodding.dev/en/docs/official-documentation/worldgen/worldgen-tutorial/density-generation-concepts)
- [Material generation concepts](https://hytalemodding.dev/en/docs/official-documentation/worldgen/worldgen-tutorial/materials-generation-concepts)
- [Prop generation concepts](https://hytalemodding.dev/en/docs/official-documentation/worldgen/worldgen-tutorial/prop-generation-concepts)
- [Worldgen optimization](https://hytalemodding.dev/en/docs/official-documentation/worldgen/worldgen-tutorial/optimization-wip)
- [Density node reference](https://hytalemodding.dev/en/docs/official-documentation/worldgen/technical-hytale-generator/density)
- [Curves](https://hytalemodding.dev/en/docs/official-documentation/worldgen/technical-hytale-generator/curves)
- [Vector providers](https://hytalemodding.dev/en/docs/official-documentation/worldgen/technical-hytale-generator/vector-provider)
- [World structure](https://hytalemodding.dev/en/docs/official-documentation/worldgen/technical-hytale-generator/world-structure)
- [Materials](https://hytalemodding.dev/en/docs/official-documentation/worldgen/technical-hytale-generator/material)
- [Material providers](https://hytalemodding.dev/en/docs/official-documentation/worldgen/technical-hytale-generator/material-providers)
- [Positions providers](https://hytalemodding.dev/en/docs/official-documentation/worldgen/technical-hytale-generator/positions-provider)
- [Prop distributions](https://hytalemodding.dev/en/docs/official-documentation/worldgen/technical-hytale-generator/prop-distributions)
- [Assignments](https://hytalemodding.dev/en/docs/official-documentation/worldgen/technical-hytale-generator/assignments)
- [Scanners](https://hytalemodding.dev/en/docs/official-documentation/worldgen/technical-hytale-generator/scanners)
- [Patterns](https://hytalemodding.dev/en/docs/official-documentation/worldgen/technical-hytale-generator/patterns)
- [Props](https://hytalemodding.dev/en/docs/official-documentation/worldgen/technical-hytale-generator/props)
- [Block masks](https://hytalemodding.dev/en/docs/official-documentation/worldgen/technical-hytale-generator/block-mask)

### Hytale first-party articles

- [The Future of World Generation](https://hytale.com/news/2026/1/the-future-of-world-generation)
- [Worldgen Introduction](https://hytale.com/news/2019/1/worldgen-introduction)

### Minecraft comparison references

- [Minecraft Java Edition 1.18 notes](https://feedback.minecraft.net/hc/en-us/articles/4415128577293-Minecraft-Java-Edition-1-18)
  — official notes for noise caves, aquifers, legacy carvers, and seed-dependent
  generation.
- [Minecraft world seed reference](https://minecraft.wiki/w/Seed_%28world_generation%29)
  — community reference for seed-width and version-dependent reproducibility.

### MiEx exporter and source references

- [MiEx repository](https://github.com/BramStoutProductions/MiEx)
- [MiEx README/features](https://github.com/BramStoutProductions/MiEx/blob/main/README.md)
- [Exporter and chunk payload scheduling](https://github.com/BramStoutProductions/MiEx/blob/main/src/nl/bramstout/mcworldexporter/export/Exporter.java)
- [Chunk generation, LOD/cave caches, modifiers, and instances](https://github.com/BramStoutProductions/MiEx/blob/main/src/nl/bramstout/mcworldexporter/export/ChunkExporter.java)
- [LOD cache](https://github.com/BramStoutProductions/MiEx/blob/main/src/nl/bramstout/mcworldexporter/export/LODCache.java), [cave cache](https://github.com/BramStoutProductions/MiEx/blob/main/src/nl/bramstout/mcworldexporter/export/CaveCache.java)
- [Mesh processor chain](https://github.com/BramStoutProductions/MiEx/blob/main/src/nl/bramstout/mcworldexporter/export/processors/MeshProcessors.java), [face optimizer](https://github.com/BramStoutProductions/MiEx/blob/main/src/nl/bramstout/mcworldexporter/export/processors/FaceOptimiser.java), and [ray-tracing optimizer](https://github.com/BramStoutProductions/MiEx/blob/main/src/nl/bramstout/mcworldexporter/export/processors/RaytracingOptimiser.java)
- [Texture atlas creation](https://github.com/BramStoutProductions/MiEx/blob/main/src/nl/bramstout/mcworldexporter/atlas/AtlasCreator.java), [deterministic exporter noise](https://github.com/BramStoutProductions/MiEx/blob/main/src/nl/bramstout/mcworldexporter/export/Noise.java)

### Supplementary/community material

- [Graphs System](https://hytalemodding.dev/en/docs/official-documentation/worldgen/worldgen-tutorial/graphs-system)
  — community-written, pre-release point/edge feature documentation.
- [Cave Generation reference](https://hytale-docs.pages.dev/modding/worldgen/caves/)
  — unofficial server-side cave API/reference; useful for the compiled
  cave-node-network model, but not a Hypixel Studios specification.
- [Surprisingly Simple World Generation in Hytale](https://medium.com/@ashleythedev/surprisingly-simple-world-generation-in-hytale-fcdc1e8bff0e)
  — community beginner tutorial showing the minimal biome, density, material
  queue, and jittered-grid prop flow.
- [CurseForge: edit and create biomes](https://support.curseforge.com/support/solutions/articles/9000273607-world-gen-how-to-edit-and-create-biomes)
- [Community biome learning example](https://www.reddit.com/r/hytale/comments/1rqb673/from_zero_to_v2_my_first_biome_in_14_days_of/)
  — useful for authoring experience, not performance evidence.

## Definition of completion

This integration is complete only when an authored World graph can generate a
deterministic streamed region containing blended biomes, at least one cave
system, resolved surface/geology materials with baked variants, and sparse
cross-chunk props; Studio can regenerate a bounded dirty subset; the production
DDA consumes only the generated result; and headless tests prove clean and
incremental builds agree.
