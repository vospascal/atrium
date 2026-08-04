// Fullscreen-triangle blit: samples the compute-written storage texture onto
// the swapchain. Three vertices, no vertex buffer.
//
// Color contract: the rgba8unorm storage texture holds DISPLAY-READY
// (sRGB-encoded) values — the DDA pass shades the project's sRGB-authored
// material albedos as-is (see shaders/dda.wgsl). The swapchain format is sRGB, so the
// hardware re-encodes fragment output on store; decoding here makes that a
// round trip (presented bytes == storage-texture bytes, no double encode).
// At render scale 1.0 the sampler is nearest, so decoding after the sample
// is exact. Below 1.0 (render-scale lever) the sampler is linear: the
// upscale interpolates sRGB-encoded values before the decode — technically
// a gamma-space blend, visually fine for a perf lever and standard practice.

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

// Patched by `OutputFormat::patch_blit_source` (src/output_format.rs). TRUE only
// when the swapchain format carries the sRGB transfer function and will therefore
// re-encode this shader's output — decoding here makes that an exact round trip.
// A 10-bit `Rgb10a2Unorm` surface applies no transfer, so the already-encoded
// value must pass straight through; decoding into it would present a washed-out
// image, and passing through into an sRGB surface would present a dark one.
const BLIT_DECODES_SRGB: bool = true;

@group(0) @binding(0) var source_texture: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    let uv = vec2<f32>(f32((vertex_index << 1u) & 2u), f32(vertex_index & 2u));
    var output: VertexOutput;
    output.clip_position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    output.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return output;
}

// Piecewise IEC 61966-2-1 sRGB electro-optical transfer function.
fn srgb_to_linear(encoded: vec3<f32>) -> vec3<f32> {
    let linear_low = encoded / 12.92;
    let linear_high = pow((encoded + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(linear_high, linear_low, encoded <= vec3<f32>(0.04045));
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let encoded_sample = textureSample(source_texture, source_sampler, input.uv);
    if (!BLIT_DECODES_SRGB) {
        return encoded_sample;
    }
    return vec4<f32>(srgb_to_linear(encoded_sample.rgb), encoded_sample.a);
}
