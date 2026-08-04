fn tonemap_hdr_knee(color: vec3<f32>, headroom: f32) -> vec3<f32> {
    let room = max(headroom - 1.0, 0.0);
    let positive = max(color, vec3<f32>(0.0));
    let mids = min(positive, vec3<f32>(1.0));
    let highs = max(positive - vec3<f32>(1.0), vec3<f32>(0.0));
    // The `max` on the denominator is NOT precision hygiene, it is correctness at ZERO
    // headroom: `room` and `highs` are then both 0 and the shoulder term evaluates 0/0 =
    // NaN, so every pixel at or below white turns to NaN. A unorm surface would clamp that
    // away; a float surface stores it. This became reachable the moment the unmeasured
    // fallback became 1.0 — a unit test caught it before the display did.
    return mids + highs * room / max(vec3<f32>(room) + highs, vec3<f32>(1.0e-6));
}

