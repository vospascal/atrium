//! The `material` node family — one file per node.

use voxel_graph::NodeDeclaration;

pub mod add_scalar;
pub mod base_color;
pub mod clamp_scalar;
pub mod color_ramp;
pub mod constant_color;
pub mod constant_scalar;
pub mod direction;
pub mod dot_vector;
pub mod emission;
pub mod emission_strength;
pub mod event_sensor;
pub mod face_color;
pub mod face_roughness;
pub mod fbm;
pub mod mix_color;
pub mod multiply_scalar;
pub mod noise;
pub mod normal;
pub mod normal_component;
pub mod normalize_vector;
pub mod oscillator;
pub mod output;
pub mod passthrough_scalar;
pub mod pattern_checker;
pub mod pattern_flat;
pub mod pattern_layer;
pub mod pattern_noise;
pub mod pattern_perlin;
pub mod pattern_ridged;
pub mod pattern_simplex;
pub mod pattern_speckle;
pub mod pattern_tile_edge;
pub mod pattern_tile_tone;
pub mod pattern_turbulence;
pub mod pattern_wave;
pub mod pattern_worley;
pub mod pattern_worley_edge;
pub mod pattern_worley_smooth;
pub mod position;
pub mod position_component;
pub mod remap_scalar;
pub mod reroute_color;
pub mod reroute_scalar;
pub mod reroute_vector;
pub mod roughness;
pub mod surface;
pub mod tessellation;
pub mod time;
pub mod vector_add;
pub mod vector_scale;

/// Every `material` node, in catalogue order.
pub const NODES: &[NodeDeclaration] = &[
    constant_scalar::DECLARATION,
    output::DECLARATION,
    surface::DECLARATION,
    constant_color::DECLARATION,
    add_scalar::DECLARATION,
    mix_color::DECLARATION,
    clamp_scalar::DECLARATION,
    position::DECLARATION,
    normal::DECLARATION,
    base_color::DECLARATION,
    roughness::DECLARATION,
    emission::DECLARATION,
    emission_strength::DECLARATION,
    face_color::DECLARATION,
    face_roughness::DECLARATION,
    pattern_flat::DECLARATION,
    pattern_noise::DECLARATION,
    pattern_speckle::DECLARATION,
    pattern_perlin::DECLARATION,
    pattern_simplex::DECLARATION,
    pattern_ridged::DECLARATION,
    pattern_turbulence::DECLARATION,
    pattern_worley::DECLARATION,
    pattern_worley_edge::DECLARATION,
    pattern_worley_smooth::DECLARATION,
    pattern_wave::DECLARATION,
    pattern_checker::DECLARATION,
    pattern_tile_tone::DECLARATION,
    pattern_tile_edge::DECLARATION,
    tessellation::DECLARATION,
    pattern_layer::DECLARATION,
    multiply_scalar::DECLARATION,
    direction::DECLARATION,
    time::DECLARATION,
    oscillator::DECLARATION,
    event_sensor::DECLARATION,
    remap_scalar::DECLARATION,
    noise::DECLARATION,
    fbm::DECLARATION,
    color_ramp::DECLARATION,
    vector_add::DECLARATION,
    vector_scale::DECLARATION,
    normalize_vector::DECLARATION,
    dot_vector::DECLARATION,
    position_component::DECLARATION,
    normal_component::DECLARATION,
    passthrough_scalar::DECLARATION,
    reroute_scalar::DECLARATION,
    reroute_color::DECLARATION,
    reroute_vector::DECLARATION,
];
