# voxel-material

What a surface **is**: the material table, and the pattern layers that vary it.

The bottom of the voxel stack — `voxel-material` → `voxel-material-graph` → `voxel-rt`. It was
extracted first because everything above needs it and it needed nothing: `material` and
`pattern` had **zero `crate::` dependencies** outside themselves, verified before the move
rather than assumed.

No `wgpu`, no pass, no shader. Only the row layouts the GPU will read (`GpuMaterial`,
`GpuPatternLayer`) and the CPU reference evaluation that must agree with the shader's.

| module | holds |
|---|---|
| `material` | the table, rows, media, face roles, GPU row encoding |
| `pattern` | layers, generators, frames, blends, the CPU noise reference |
| `animation_clock` | the clock a material animates against |
| `world_event` | the event field a material responds to |

The last two are here because they are *inputs to evaluating a surface* — an oscillator node
needs the clock, an event-sensor node needs the field. Both were leaves in `voxel-rt`, so
leaving them above the material table would have made the lowering crate impossible.

## Two things the extraction surfaced

**`GeneratorCost::color()` returned an `egui::Color32`.** UI code in the data layer, which only
became visible when the crate had to compile without egui. It is now `rgb() -> [u8; 3]`; the
overlay wraps it. The accessibility rationale for the ramp — that red-green is the most common
colour-blindness axis, so the scale runs grey→amber→red so hue *and* lightness both carry it —
stayed with the data, where it belongs.

**`PATTERN_GENERATOR_MASK_ALL` lived with the renderer's levers.** It is asserted to be exactly
the union of every generator's bit, in both directions, so it is *derived from the generators*
and belongs beside them. The lever that dials it now reads this value instead of defining it.

## Known follow-up

`pattern` still holds every noise generator in one file — `value_noise`, `perlin_noise`,
`simplex_noise`, `ridged_noise`, `turbulence`, `worley_distances`, `wave`, `checker` — behind
shared helpers. Those are independently selectable implementations, so by this workspace's
convention each belongs in its own file with the helpers in a `common`. Not done.

```sh
cargo test -p voxel-material
cargo tree -p voxel-material    # bytemuck + voxel-core, nothing else
```
