//! Integer hashing and the easing curves every generator below interpolates with.

/// The integer hash both sides use: Chris Wellons' `lowbias32`.
///
/// Chosen because it is three multiplies and three shifts with no lookup, has
/// avalanche behaviour good enough that neighbouring lattice cells are visually
/// uncorrelated, and — the deciding property — is expressible identically in Rust
/// and WGSL. Rust's `wrapping_mul` and WGSL's `u32` multiply are both mod 2^32, and
/// `>>` on `u32` is logical in both, so the two implementations agree bit for bit.
pub(crate) fn hash_u32(value: u32) -> u32 {
    let mut hashed = value;
    hashed ^= hashed >> 16;
    hashed = hashed.wrapping_mul(0x7feb_352d);
    hashed ^= hashed >> 15;
    hashed = hashed.wrapping_mul(0x846c_a68b);
    hashed ^= hashed >> 16;
    hashed
}

/// Hash a 3D lattice cell to `0.0..1.0`.
///
/// The `as u32` casts are two's-complement reinterpretations, which is exactly
/// what WGSL's `bitcast<u32>` does — so a negative coordinate (and the world's
/// centre is at +512, but a `Voxel` frame layer on a pattern offset can go
/// negative) hashes the same on both sides.
pub(crate) fn hash_cell(cell: [i32; 3], salt: u32) -> f32 {
    let mixed = (cell[0] as u32).wrapping_mul(0x27d4_eb2d)
        ^ (cell[1] as u32).wrapping_mul(0x9e37_79b9)
        ^ (cell[2] as u32).wrapping_mul(0x85eb_ca6b)
        ^ salt.wrapping_mul(0xc2b2_ae35);
    hash_u32(mixed) as f32 / 4_294_967_296.0
}

/// The classic `3t^2 - 2t^3` ease. WGSL's `smoothstep(0, 1, t)` is the same
/// polynomial, and this is applied to an already-clamped `0..1`.
pub(crate) fn ease(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Perlin's quintic fade `6t^5 - 15t^4 + 10t^3`.
///
/// The cubic [`ease`] has a discontinuous second derivative at the lattice, which
/// value noise gets away with and a gradient field does not — it shows as faint
/// creases along the cell planes. Mirrors `pattern_quintic`.
pub(crate) fn quintic(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}
