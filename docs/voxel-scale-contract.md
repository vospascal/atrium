# Voxel scale contract

This is the current authority for world and detail scale.

## World geometry

- The physical world is **125 × 32 × 125 metres**.
- The authoritative gameplay lattice is **125 × 32 × 125 world voxels**.
- One world voxel is an indivisible **1 × 1 × 1 metre block**.
- World generation, click add/remove, world profiles, collision geometry, and
  future gameplay mutations address this lattice.
- A generated world voxel maps directly to one uniform 8³ brickmap cell. This
  preserves uniform-brick traversal and memory optimizations without exposing
  the renderer's detail grid as gameplay geometry.

## Detail geometry

- One world voxel has an optional **8 × 8 × 8 detail tile**.
- One detail cell is **0.125 metre**.
- Detail cells are reserved for imported/authored assets and material surface
  detail. They are not an ordinary terrain edit unit.
- Replacing or removing a world voxel replaces its entire detail tile. This is
  intentional: an asset occupying a block belongs to that block.

## Material patterns

- `texels_per_voxel` means texels per **one-metre world-voxel face**.
- Eight texels therefore produce **0.125 m** material texels.
- World-framed patterns remain continuous across neighbouring world voxels.
- Face-framed and voxel-framed patterns restart at one-metre boundaries, not at
  renderer detail-cell boundaries.

## Studio

- Built-in Single, Wall, Cube, plate, and emitter previews use one-metre world
  voxels.
- Imported `.vox` subjects are assets and retain 0.125 m cells.
- Studio's material/node authoring remains in `voxel-rt`; it does not depend on
  Bevy.
- The day/night sky is part of `voxel-rt` lighting and is available in Studio.

## Package boundary

- `voxel-core`: scale constants, logical world generation, terrain import, `.vox`
  parsing, wind, and water simulation. It has no renderer dependency.
- `voxel-rt`: brickmap/DDA renderer, world authority, Studio, node graph,
  materials, lighting, water optics, CAGI, camera, and character control.
- `atrium`, `atrium-core`, `atrium-bevy`, `atrium-behavior`: preserved sound
  engine and its Bevy-based audio editor/integration.
- `bevy-ui`: Atrium sound editor application. It is not a voxel renderer.
- The former Bevy voxel sandbox is removed.

## Studio node-system dependencies

The graph data model (`voxel-rt/src/graph.rs`) depends on stable asset IDs,
serializable graph/schema types, and `serde`. Material graph compilation and the
egui editor depend on that model. Project persistence connects graphs to Studio
assets and material assignments. Rendering consumes compiled graph programs;
world generation does not depend on the graph editor.
