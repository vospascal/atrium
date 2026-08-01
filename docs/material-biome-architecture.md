# Material and biome architecture

This document records the dependency rules for Atrium's material and procedural
world foundation. The authored assets are canonical. Renderer tables, generated
WGSL, chunk commands, and runtime requests are compiled projections.

## Ownership

```text
generated environment fields + runtime environment state
                            |
                            v
                     biome resolver
                            |
                            v
                       biome profile
              +-------------+-------------+
              |             |             |
              v             v             v
       surface rules   feature sets   runtime profiles
              |             |          (audio/animation)
              v             v             |
       material roles  generation          |
              |        requests            |
              v                            |
       material palette                    |
              |                            |
              +------------+---------------+
                           v
               compiled runtime projections
```

The world produces environmental facts. Biomes interpret those facts and select
reusable profiles. Surface profiles assign semantic material roles such as
`ground.surface`; palettes bind those roles to intrinsic material assets.
Materials never reference concrete biome IDs.

Physical changes and presentation changes are different outputs:

- `AddVoxelLayer` and feature requests modify world composition.
- presentation modifiers alter resolved material behavior without owning the
  material graph.
- audio and animation are runtime requests and do not mutate world composition.

## Canonical node schema

Each `NodeDeclaration` owns its title, description, category, preview mode,
supported graph kinds, sockets, editable fields, defaults, hard constraints,
soft UI ranges, steps, enum choices, and read-only status.

Graph commands materialize those defaults atomically and reject values outside
the declaration. Graph Studio reads the same declarations for its catalog,
canvas, inspector, and connector menu. Its field renderer is a generic type
projection: booleans become checkboxes, colors become color pickers, constrained
numbers become numeric controls, and text with choices becomes a dropdown. It
contains no property-name or node-type widget configuration. Material lowering
also reads declaration defaults, so compilers do not carry a second set of
fallback values.

Each declaration also carries a typed operation ID. Execution remains
backend-specific, but compilers dispatch through that operation rather than a
second node-name switch. UI metadata does not depend on egui, and domain models
do not depend on wgpu or renderer state.

Every persisted material has a required canonical graph. Missing graphs,
dangling IDs, schema errors, and shader compilation failures reject the project;
the renderer does not silently substitute a material row. The material table and
packed pattern stack are runtime projections.

A material graph has one typed surface flow:

```text
Flat / Noise / Speckle -----> pattern input
                                      |
shading inputs -> Material Surface -> Pattern Layer -> ... -> Material Output
```

`Material Output` owns only the final `MaterialSurface` input. Layer order is
the connection order, layer enablement is an ordinary declared boolean field,
and adding/removing a layer rewires the chain. There is no separate layer-count
node or numeric order property that can disagree with graph topology.

Pattern generators and pattern application are separate concerns. Flat, Noise,
and Speckle nodes produce a typed `MaskField` and own their sampling controls;
Pattern Layer consumes that field and owns target, blend, strength, face filters,
color, and emission. Reconnecting a generator replaces the previous source, so
new generator kinds extend the registry without adding a generator dropdown or
UI orchestration branch.

### Declarative graph constraints

Socket declarations own a `Cardinality { minimum, maximum }` alongside their
direction (input/output list), value type, and evaluation rate. Graph contracts
use the same cardinality model for operation instance limits. Material Output and
Material Surface are exactly-one operations, Pattern Layer is bounded by the
renderer layer limit, surface-chain sockets are exactly-one, normal value inputs
are optional-single, and reusable value/pattern outputs may fan out.

The material graph contract declares a `MaterialSurface` flow from Material
Surface, through zero or more Pattern Layers, to Material Output. Resolution,
connection planning, command validation, catalog availability, socket drawing,
hover compatibility, and save validation all consume these declarations. The
canvas does not keep a second list of legal connections or singleton node types.

Compound edits are graph transactions. Adding or removing a Pattern Layer and
its links is one undoable change, and a failed command restores the entire prior
graph. Dragging from an occupied input rewires its single connection; releasing
the endpoint on empty canvas disconnects it. Graphs may be incomplete while the
user is editing and show model-derived diagnostics, but invalid graphs cannot be
saved or compiled.

## Update classification

Every derived output carries two independent dimensions:

- spatial granularity: global, region, chunk, voxel, or sample;
- update frequency: compile time, world event, simulation tick, or frame.

This prevents a continuously animated shader uniform from forcing chunk
regeneration and prevents a physical snow layer from being treated as a cheap
per-frame material change.

## Determinism

Procedural decisions use the world seed, integer world position, and a stable
rule salt. Results therefore do not depend on thread scheduling, traversal
order, or which neighboring chunk generated first.

## Extension rules

Adding a material node requires a declaration and its backend execution. No UI
branch or creation-default branch is permitted.

Adding a biome normally requires only a biome selector and references to
existing profiles. New palettes, surface profiles, modifiers, feature sets,
audio profiles, and animation profiles are reusable independently.

Cross-domain references are validated when a world profile is compiled and
again before a persisted profile becomes active. Invalid authored state never
replaces the last compiled runtime state.

Project compilation resolves material identities to concrete voxel slots and
accepts only executable graph/runtime handles registered by the owning backend.
A parseable graph file is not automatically an executable modifier or feature.
The compiled profile pre-indexes domain assets and pre-sorts surface rules.

`AddVoxelLayer` has a concrete brickmap consumer. It evaluates generated surface
facts and runtime season/weather state before initial GPU upload, and project
profile changes rebuild the generated world through the same adapter. Other
projection consumers remain separate by design: feature, presentation, audio,
and animation backends must register executable handles before profiles may
reference them.
