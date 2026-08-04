// The GPU half of `voxel_color::tonemap` — the six selectable curves, in WGSL.
//
// THIS FILE IS THE SHADER SIDE OF A TWO-SIDED CONTRACT, and it lives here rather than
// in the renderer for that reason. `tonemap.rs` next door holds a Rust implementation
// of the same six curves; this one runs on the GPU. Two implementations of one piece of
// mathematics is a drift hazard, and keeping them in one crate is what lets a test hold
// them together — see `tonemap.rs`'s `wgsl_curve_indices_match_the_rust_enum` and the
// property tests around it. Split across crates, nothing could.
//
// The renderer splices this in through `voxel_color::tonemap::WGSL`; it declares no
// bindings, reads no uniforms and calls nothing outside this file, so it can be
// concatenated anywhere in a module. The caller passes the headroom, the curve index
// and the content peak as plain arguments, and applies exposure and the sRGB encode
// itself — see `dda.wgsl`'s output block for the required ORDER, which is exposure,
// then curve, then encode.
//
// `srgb_decode` / `srgb_encode` deliberately do NOT live here: they are the renderer's
// transfer function, shared with the CAGI pass, which decodes table-derived albedo with
// the same curve and never tonemaps anything.

