# voxel-rt — Ray-Traced Voxel Renderer (Rewrite Plan)

New workspace crate `crates/voxel-rt`, built **next to** `voxel-sandbox` (which keeps
working untouched). Architecture modeled on xima's engine (Felix Maier / Voxile):
everything flows through **one shared primitive — DDA traversal of the voxel grid** —
for primary rays, lighting, and (later) audio rays. No meshes, no rasterized geometry.

Stack: `winit` + `wgpu` (Metal on macOS dev, Vulkan on Quest 3 later) + `egui`
overlay. World data comes from `voxel-core` (`VoxelWorld`, RLE + `unpack_chunk` +
`solid_occupancy_bits`), which was designed renderer-agnostic for exactly this.

Lighting is **CAGI** (Cellular Automata Global Illumination, xima's technique):
integer flood-fill light propagation through the voxel grid — multi-bounce, noiseless,
no denoiser, amortizable, Quest-viable. NOT Monte Carlo path tracing.

Process rules (same as voxel-sandbox arc): stages in order, any deviation flagged and
approved first, optimize aggressively, stage-gate = Pascal runs the app and eyeballs
it, checkbox roadmap shown every turn.

**Architecture rule — as modular as possible (Pascal, 2026-07-29):** hard seams
between platform/windowing ↔ GPU device ↔ render passes ↔ world data ↔ camera ↔
overlay. Each render pass is its own module with a narrow interface (resources in,
commands out) so passes can be added/swapped without touching others — the pass
list will grow Stage by Stage (test pattern → DDA → shadow → CAGI → post). The
brickmap stays renderer-independent (it doubles as the audio-ray structure); the
camera stays windowing-independent (it doubles as the VR head pose slot); the
platform layer stays thin (it gets replaced by OpenXR on Quest). Modular means
clean seams — never compat shims or forwarding layers.

---

## Stages

### Stage 0 — Scaffold ✅ (gate passed 2026-07-29: 8 ms / 120 fps, vsync-capped)
Binary crate `voxel-rt` in the workspace. `winit` window, `wgpu` device/surface
(Metal), fullscreen **compute** pass writing a test pattern to a storage texture,
blitted to the swapchain. `egui` overlay with FPS counter.
**Gate:** app opens, animated test pattern, FPS readout.

### Stage 1 — Brickmap + primary-ray DDA ✅ (gate passed 2026-07-29: ~4 ms uncapped; brickmap 61.7 ms / 71,941 bricks)
Two-level brickmap built from `VoxelWorld`: dense brick-pointer grid (8³-voxel
bricks), per-occupied-brick occupancy bits + material bytes; palette from the
`Voxel` enum (hues matched to voxel-sandbox's). Upload to storage buffers.
Fullscreen compute: per-pixel two-level DDA (coarse brick step, fine step inside
occupied bricks), flat voxel colors, face-normal shading, sky background. Fly
camera (WASD + mouse). CPU-side round-trip test: brickmap.get == VoxelWorld.get.
**Gate:** fly around the island at interactive FPS; report resolution + frame time.

### Stage 2 — Direct light ⬜
One sun shadow ray per primary hit (same DDA), sky ambient by face orientation,
simple tonemap. **Gate:** crisp voxel shadows, no acne, FPS report.

### Stage 3 — CAGI light volume ⬜
Light volume over the island (brick-resolution or per-voxel, decide by memory
budget). Compute pass: inject sunlight + emissive voxels, integer flood-fill
propagation (few iterations/frame, amortized; re-flood dirty regions on edits).
Sample volume in shading for multi-bounce GI. **Gate:** place a lantern → warm
light bleeds around corners, zero noise.

### Stage 4 — Look pass ⬜
Fog, depth of field (lens-sample ray origins), tonemap curve, palette polish.
Target: the Voxile screenshot mood. **Gate:** screenshot-worthy still.

### Stage 5 — Audio bridge ⬜
`VoxelDdaResolver` in atrium: CPU DDA over the same occupancy grid → direct +
occlusion + early-reflection `PathContribution`s, background thread, `rtrb` to the
audio thread. **Gate:** fly behind a hill → source muffles; enclosed space → reverb
tightens.

### Stage 6 — Quest spike ⬜
Cross-compile `aarch64-linux-android`, OpenXR loader init, single-eye DDA render
on device (crib wgpu↔OpenXR interop from bevy_mod_openxr). **Gate:** runs on
Quest 3.

---

## Non-goals (for now)
Monte Carlo path tracing, meshing of any kind, infinite streaming (island first),
water simulation coupling, Bevy interop. voxel-sandbox stays as-is throughout.
