// Must match `voxel_color::TonemapCurve::shader_index`.
const TONEMAP_REINHARD: u32 = 0u;
const TONEMAP_REINHARD_HEADROOM: u32 = 1u;
const TONEMAP_KNEE: u32 = 2u;
const TONEMAP_HABLE: u32 = 3u;
const TONEMAP_BT2390: u32 = 4u;
const TONEMAP_GT7: u32 = 5u;

// Dispatch on the runtime curve. Five resident branches, and the worry was that GT7's two
// ICtCp matrices and BT.2390's PQ constants would raise the kernel's register pressure and
// cost every frame whether or not they run — register allocation is decided by the worst
// path, not the taken one.
//
// MEASURED, not assumed (bench section 14, M3 Max): against a source with this dispatch
// collapsed to its Reinhard return, the six-curve shader runs -0.3% on both the aerial and
// the ground shot. The unselected curves are free. `curve` is uniform across the dispatch,
// so the branch is coherent and the cost is the taken path alone.
//
// That is what buys the control its keep: the alternative is a pipeline rebuild per
// selection, which makes comparing two curves on the same frame impossible, and comparison
// is the entire purpose of the control.
fn apply_tonemap(color: vec3<f32>, headroom: f32, curve: u32, content_peak: f32) -> vec3<f32> {
    if (curve == TONEMAP_GT7) {
        return tonemap_gt7(color, headroom);
    }
    if (curve == TONEMAP_BT2390) {
        return tonemap_bt2390(color, headroom, content_peak);
    }
    if (curve == TONEMAP_KNEE) {
        return tonemap_hdr_knee(color, headroom);
    }
    if (curve == TONEMAP_REINHARD_HEADROOM) {
        return tonemap_reinhard_headroom(color, headroom);
    }
    if (curve == TONEMAP_HABLE) {
        return tonemap_hable(color);
    }
    return tonemap_reinhard(color);
}

