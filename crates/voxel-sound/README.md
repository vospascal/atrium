# voxel-sound

`voxel-sound` owns the relationship between gameplay sound cues and the supplied
audio catalog. Its public API is intentionally semantic: the application asks
for a footstep, jump, or landing sample and never reaches into `assets/`.

The generated island is grass-topped, so the first implementation uses grass
walk samples for footsteps and jumps, and dirt landing samples for soft terrain
landings. The corrected `voxel_sound.movement.default` section in
`assets/sounds.json` is the source of truth: `build.rs` validates every indexed
file and generates the private embedded lookup table from it.

Adding surface-specific sound should extend `movement.rs` with a resolved ground
material input. It must not expose asset paths to consumers.
