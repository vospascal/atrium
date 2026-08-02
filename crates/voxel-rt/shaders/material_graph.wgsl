// The material-graph dispatch. `material_graph.rs::inject_into_dda` rewrites the
// marker below into one branch per material slot and appends the generated
// functions after this file. `graph_active` keeps a slot with no graph on the
// material-table path unchanged.
//
// The shared ABI and helpers live in `graph_prelude.wgsl`, concatenated before
// this file.

fn material_graph_surface(material: u32, position: vec3<f32>, normal: vec3<f32>) -> GraphMaterial {
    // GRAPH_DISPATCH_POINT
    let row = materials[material];
    return GraphMaterial(vec4<f32>(row.albedo, 1.0), row.roughness,
                         vec4<f32>(row.emission, 1.0), false, false, false,
                         pattern_animation_identity());
}
