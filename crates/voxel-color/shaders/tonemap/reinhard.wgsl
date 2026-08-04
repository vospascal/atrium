// Simple Reinhard: maps [0, inf) radiance into [0, 1). Stage 4 refines this.
fn tonemap_reinhard(color: vec3<f32>) -> vec3<f32> {
    return color / (vec3<f32>(1.0, 1.0, 1.0) + color);
}

// ---- Extended-range output (OutputDepth::HdrFloat) --------------------------
//
// NOTHING IS PATCHED HERE ANY MORE. The mode used to flip a
// `const OUTPUT_EXTENDED_RANGE` that chose between Reinhard and the knee; both the curve
// and the headroom now arrive in `lighting.output_params`, so the only depth-dependent
// edit left to this file is the storage-texture TYPE, which has to be a source
// substitution because the format is part of the WGSL type.
//
// Display headroom now arrives in `lighting.output_params.x`, MEASURED per frame rather
// than assumed — see `voxel_color::headroom` and `lighting.rs`'s `OutputParams`.
//
// It was `const OUTPUT_HDR_HEADROOM: f32 = 4.0`, on the reasoning that a uniform would
// mean a new binding and the bind group layout is what broke this feature three times.
// The reasoning was sound and the conclusion was still wrong: real EDR headroom moves
// while the app runs — the brightness slider changes it, thermal state changes it,
// dragging the window to another display changes it — so a const meant claiming 4x on a
// panel that might have 1.0x and letting the compositor clamp the difference, which reads
// as a bright, blown picture. A const would also mean a pipeline rebuild per slider tick.
//
// No new binding was needed after all: `lighting` is already bound here and already
// uploaded every frame, so a live headroom costs nothing.

// Roll highlights into the display's headroom while leaving everything up to SDR
// white EXACTLY as authored.
//
// Reinhard cannot serve an HDR surface: `L/(1+L)` has a fixed ceiling of 1.0, so it
// destroys the extended range before the compositor ever sees it. This keeps 0..1
// untouched and compresses only what is above it, asymptotically approaching
// `headroom`.
//
// THE DENOMINATOR IS `room`, NOT 1.0, and that is the whole subtlety. The obvious
// form — `highlights / (1 + highlights) * (headroom - 1)` — has derivative
// `headroom - 1` at white, so the slope jumps from 1 to 3 at exactly 1.0 and leaves a
// visible kink where most content sits. Putting `room` in the denominator makes the
// derivative 1 there, so the curve is C1 across white, with the same endpoints.
// The `max(color, 0)` matters MORE here than on the Reinhard path. `srgb_encode` is
// `pow`, which returns NaN for a negative base; a unorm surface clamps that NaN to
// zero and hides it, but a float surface stores it and the compositor renders whatever
// it makes of it. Radiance should never be negative, so this guards a bug rather than
// shaping a look.
// Reinhard with a bounded HDR continuation. Plain Reinhard is kept EXACTLY through scene
// white, then the unused upper half of its unit-range output is reparameterised into a
// zero-slope shoulder that approaches the measured display headroom.
//
// `base = L/(1+L)` puts scene white at 0.5. Above there, `t = 2*base-1` runs from 0 at
// scene white to 1 as radiance tends to infinity. Adding `room*t²` gives four properties
// the old extended-Reinhard white-point operator did not have:
//
// 1. headroom 1.0 is EXACTLY plain Reinhard for every nonnegative input;
// 2. all inputs through scene white are EXACTLY plain Reinhard at every headroom;
// 3. the join is C¹ because the added term has zero derivative at t=0;
// 4. the output is bounded by `headroom`, so the compositor never has to clip a curve
//    that was supposedly fitted to the display.
//
// Ours, not Reinhard et al. eq. 4. That equation's W is an INPUT white point: at W=1 it
// simplifies to identity, not plain Reinhard, and at high L it is unbounded. Passing the
// display's OUTPUT headroom as W was a category error that made the no-headroom fallback
// brighter and clipped instead of matching SDR.
fn tonemap_reinhard_headroom(color: vec3<f32>, headroom: f32) -> vec3<f32> {
    let positive = max(color, vec3<f32>(0.0));
    let base = tonemap_reinhard(positive);
    let room = max(headroom - 1.0, 0.0);
    let continuation = max(2.0 * base - vec3<f32>(1.0), vec3<f32>(0.0));
    return base + room * continuation * continuation;
}

