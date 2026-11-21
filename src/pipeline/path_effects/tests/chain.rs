use super::*;

/// The four effects the speaker/headphone pipelines build, in the order
/// `pipeline::builders::image_source_path_effect_factories` lists them,
/// compose to: delay (lossless) ×
/// air absorption × ground × wall colour. For a hard wall, no ground
/// (horizontal distance < 0.5 m → unity) and a short path, the chain is
/// transparent at 1 kHz apart from the delay — no stage applies the
/// broadband reflectivity or distance gain a second time (those live in
/// path.gain and in the renderer).
#[test]
fn path_chain_applies_no_hidden_broadband_gain() {
    use crate::pipeline::path::PathEffectChain;
    let atmosphere = AtmosphericParams::default();
    let ground = GroundProperties::default();
    let sample_rate = 48_000.0;
    let mut chain = PathEffectChain::new(vec![
        Box::new(PropagationDelayEffect::new(sample_rate, 8192)),
        Box::new(AirAbsorptionEffect::new(sample_rate)),
        Box::new(GroundEffectFilter::new(sample_rate)),
        Box::new(WallAbsorptionEffect::new(sample_rate)),
    ]);
    let mut path = reflection_path(0, 0.002);
    path.distance = 1.0;
    let materials: [WallMaterial; 6] = std::array::from_fn(|_| WallMaterial::hard_wall());
    chain.update(&PathEffectContext {
        path: &path,
        environment: &test_environment(&materials, &ground, &atmosphere),
        sample_rate,
        source_pos: Vec3::ZERO,
        target_pos: Vec3::new(0.0, 0.0, 0.3),
    });
    let mut input_power = 0.0f64;
    let mut output_power = 0.0f64;
    for sample_index in 0..48_000usize {
        let input = (std::f32::consts::TAU * 1000.0 * sample_index as f32 / sample_rate).sin();
        let output = chain.process_sample(input);
        if sample_index >= 24_000 {
            input_power += (input as f64).powi(2);
            output_power += (output as f64).powi(2);
        }
    }
    let response_db = 10.0 * (output_power / input_power).log10();
    assert!(
        response_db.abs() < 0.2,
        "chain response {response_db:.3} dB"
    );
}
