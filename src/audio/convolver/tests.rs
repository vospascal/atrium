use super::*;

#[test]
fn identity_impulse() {
    // Convolving with [1, 0, 0, ...] should reproduce the input.
    let mut conv = Convolver::new();
    let ir = [1.0, 0.0, 0.0, 0.0];
    conv.init(128, &ir);

    let input: Vec<f32> = (0..128).map(|i| (i as f32) * 0.01).collect();
    let mut output = vec![0.0; 128];
    conv.process(&input, &mut output);

    for i in 0..128 {
        assert!(
            (output[i] - input[i]).abs() < 1e-5,
            "sample {i}: expected {}, got {}",
            input[i],
            output[i]
        );
    }
}

#[test]
fn delay_by_one_sample() {
    // IR = [0, 1] should delay the signal by one sample.
    let mut conv = Convolver::new();
    let ir = [0.0, 1.0];
    conv.init(4, &ir);

    // Block 1: [1, 2, 3, 4]
    let input1 = [1.0, 2.0, 3.0, 4.0];
    let mut out1 = [0.0; 4];
    conv.process(&input1, &mut out1);
    // Expected: [0, 1, 2, 3] — sample 4 goes into overlap.
    assert!((out1[0] - 0.0).abs() < 1e-5);
    assert!((out1[1] - 1.0).abs() < 1e-5);
    assert!((out1[2] - 2.0).abs() < 1e-5);
    assert!((out1[3] - 3.0).abs() < 1e-5);

    // Block 2: [5, 6, 7, 8]
    let input2 = [5.0, 6.0, 7.0, 8.0];
    let mut out2 = [0.0; 4];
    conv.process(&input2, &mut out2);
    // Expected: [4, 5, 6, 7] — overlap from block 1 adds sample 4.
    assert!((out2[0] - 4.0).abs() < 1e-5);
    assert!((out2[1] - 5.0).abs() < 1e-5);
    assert!((out2[2] - 6.0).abs() < 1e-5);
    assert!((out2[3] - 7.0).abs() < 1e-5);
}

#[test]
fn set_response_preserves_overlap() {
    // Swap IR mid-stream — the overlap tail from the old IR must survive.
    let mut conv = Convolver::new();
    let ir = [0.0, 1.0]; // delay-by-1
    conv.init(4, &ir);

    let input1 = [1.0, 0.0, 0.0, 0.0];
    let mut out1 = [0.0; 4];
    conv.process(&input1, &mut out1);
    // out1 = [0, 1, 0, 0], overlap = [0] (sample after last output position)

    // Wait — with IR=[0,1], convolving [1,0,0,0] gives [0,1,0,0,0].
    // Output = [0,1,0,0], overlap = [0]. Not very interesting.
    // Better test: use a longer IR.
    let mut conv2 = Convolver::new();
    let ir2 = [0.0, 0.0, 1.0]; // delay-by-2
    conv2.init(4, &ir2);

    let input = [10.0, 0.0, 0.0, 0.0];
    let mut out = [0.0; 4];
    conv2.process(&input, &mut out);
    // Convolution: [0, 0, 10, 0, 0, 0]
    // Output: [0, 0, 10, 0], overlap = [0, 0]
    assert!((out[0]).abs() < 1e-5);
    assert!((out[1]).abs() < 1e-5);
    assert!((out[2] - 10.0).abs() < 1e-5);

    // Now swap IR to identity — overlap from previous block must survive.
    conv2.set_response(&[1.0, 0.0, 0.0]);

    let input2 = [0.0, 0.0, 0.0, 0.0];
    let mut out2 = [0.0; 4];
    conv2.process(&input2, &mut out2);
    // The overlap [0, 0] gets added — should be silent.
    // This test mainly verifies no crash/corruption on set_response.
    for s in &out2 {
        assert!(s.abs() < 1e-4, "unexpected energy after IR swap: {s}");
    }
}

#[test]
fn partial_block() {
    // Process a block smaller than block_size — must work correctly.
    let mut conv = Convolver::new();
    let ir = [1.0, 0.5];
    conv.init(128, &ir);

    // Only 3 samples instead of 128.
    let input = [1.0, 0.0, 0.0];
    let mut output = [0.0; 3];
    conv.process(&input, &mut output);
    // Convolution of [1,0,0] * [1, 0.5] = [1, 0.5, 0]
    assert!((output[0] - 1.0).abs() < 1e-5);
    assert!((output[1] - 0.5).abs() < 1e-5);
    assert!((output[2] - 0.0).abs() < 1e-5);
}

#[test]
fn long_ir_with_set_response_produces_output() {
    // Mimics HRTF runtime: 200-tap HRIR, block_size=128, with IR swaps.
    let mut conv = Convolver::new();

    // Create a realistic-ish HRIR (decaying sinusoid)
    let filter_len = 200;
    let ir: Vec<f32> = (0..filter_len)
        .map(|i| {
            let t = i as f32 / filter_len as f32;
            (t * 10.0 * std::f32::consts::PI).sin() * (-3.0 * t).exp()
        })
        .collect();

    conv.init(128, &ir);

    // Process 10 blocks of constant input, checking for output energy
    let input = [0.5f32; 128];
    let mut total_energy = 0.0f32;
    for _ in 0..10 {
        let mut output = [0.0f32; 128];
        conv.process(&input, &mut output);
        total_energy += output.iter().map(|s| s * s).sum::<f32>();
    }
    assert!(
        total_energy > 0.01,
        "convolver produced near-silence: energy = {total_energy}"
    );

    // Now swap IR (simulating HRTF direction change) and keep processing
    let ir2: Vec<f32> = (0..filter_len)
        .map(|i| {
            let t = i as f32 / filter_len as f32;
            (t * 8.0 * std::f32::consts::PI).sin() * (-2.5 * t).exp()
        })
        .collect();
    conv.set_response(&ir2);

    let mut post_swap_energy = 0.0f32;
    for _ in 0..10 {
        let mut output = [0.0f32; 128];
        conv.process(&input, &mut output);
        post_swap_energy += output.iter().map(|s| s * s).sum::<f32>();
    }
    assert!(
        post_swap_energy > 0.01,
        "convolver silent after set_response: energy = {post_swap_energy}"
    );
}

#[test]
fn double_buffered_crossfade_like_hrtf() {
    // Two convolvers (A/B) alternating like the HRTF renderer does.
    let filter_len = 200;
    let ir: Vec<f32> = (0..filter_len)
        .map(|i| {
            let t = i as f32 / filter_len as f32;
            (t * 10.0 * std::f32::consts::PI).sin() * (-3.0 * t).exp()
        })
        .collect();

    let mut conv_a = Convolver::new();
    let mut conv_b = Convolver::new();
    conv_a.init(128, &ir);
    conv_b.init(128, &ir);

    let input = [0.5f32; 128];
    let mut active = 0; // 0 = A, 1 = B

    let mut total_energy = 0.0f32;
    for block in 0..20 {
        let mut out_active = [0.0f32; 128];
        let mut out_retiring = [0.0f32; 128];

        // Process through both
        if active == 0 {
            conv_a.process(&input, &mut out_active);
            conv_b.process(&input, &mut out_retiring);
        } else {
            conv_b.process(&input, &mut out_active);
            conv_a.process(&input, &mut out_retiring);
        }

        // Swap every 4 blocks (like FILTER_UPDATE_INTERVAL)
        if block % 4 == 0 && block > 0 {
            let new_active = 1 - active;
            let ir_shifted: Vec<f32> = (0..filter_len)
                .map(|i| {
                    let t = i as f32 / filter_len as f32;
                    let phase = block as f32 * 0.1;
                    ((t * 10.0 + phase) * std::f32::consts::PI).sin() * (-3.0 * t).exp()
                })
                .collect();
            if new_active == 0 {
                conv_a.set_response(&ir_shifted);
            } else {
                conv_b.set_response(&ir_shifted);
            }
            active = new_active;
        }

        let block_energy: f32 = out_active.iter().map(|s| s * s).sum();
        total_energy += block_energy;
    }

    assert!(
        total_energy > 0.1,
        "double-buffered convolvers produced near-silence: energy = {total_energy}"
    );
}

/// Small deterministic noise in [-1, 1] so tests exercise every FFT bin.
fn noise(sample_index: usize) -> f32 {
    let mut state = (sample_index as u32).wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
    state ^= state >> 15;
    state = state.wrapping_mul(0x2C1B_3C6D);
    state ^= state >> 12;
    (state as f32 / u32::MAX as f32) * 2.0 - 1.0
}

/// Reference: textbook linear convolution y[n] = Σ_k h[k]·x[n−k],
/// truncated to the input length (the convolver emits one output per input).
fn direct_convolution(input: &[f32], kernel: &[f32]) -> Vec<f32> {
    (0..input.len())
        .map(|output_index| {
            kernel
                .iter()
                .enumerate()
                .filter(|(tap, _)| *tap <= output_index)
                .map(|(tap, weight)| weight * input[output_index - tap])
                .sum()
        })
        .collect()
}

/// Stream `input` through `convolver` in consecutive blocks of the given
/// lengths (cycled), returning the concatenated output.
fn stream(convolver: &mut Convolver, input: &[f32], block_lengths: &[usize]) -> Vec<f32> {
    let mut output = vec![0.0; input.len()];
    let mut position = 0;
    let mut block = 0;
    while position < input.len() {
        let length = block_lengths[block % block_lengths.len()].min(input.len() - position);
        convolver.process(
            &input[position..position + length],
            &mut output[position..position + length],
        );
        position += length;
        block += 1;
    }
    output
}

fn assert_samples_match(actual: &[f32], expected: &[f32], tolerance: f32, label: &str) {
    assert_eq!(actual.len(), expected.len());
    for (index, (a, e)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (a - e).abs() < tolerance,
            "{label}: sample {index}: expected {e}, got {a}"
        );
    }
}

/// The convolver has zero algorithmic latency: a delta at tap k delays the
/// input by exactly k samples, including across block boundaries
/// (k = 5 > block size 4, so every output sample comes from the overlap).
#[test]
fn delta_at_tap_five_delays_by_five_samples_across_blocks() {
    let mut kernel = [0.0f32; 8];
    kernel[5] = 1.0;
    let mut convolver = Convolver::new();
    convolver.init(4, &kernel);
    let input: Vec<f32> = (0..40).map(noise).collect();
    let output = stream(&mut convolver, &input, &[4]);
    for index in 0..input.len() {
        let expected = if index >= 5 { input[index - 5] } else { 0.0 };
        assert!(
            (output[index] - expected).abs() < 1e-5,
            "sample {index}: expected {expected}, got {}",
            output[index]
        );
    }
}

/// A 3-tap kernel streamed in irregular partial blocks — including blocks
/// of 1 and 2 samples, shorter than the 2-sample overlap tail — matches
/// direct convolution sample for sample.
#[test]
fn three_tap_kernel_matches_direct_convolution_in_partial_blocks() {
    let kernel = [0.5f32, -0.25, 0.125];
    let mut convolver = Convolver::new();
    convolver.init(16, &kernel);
    let input: Vec<f32> = (0..300).map(noise).collect();
    let output = stream(&mut convolver, &input, &[16, 5, 1, 16, 2, 9, 16, 1, 1]);
    assert_samples_match(&output, &direct_convolution(&input, &kernel), 1e-5, "3-tap");
}

/// A 200-tap kernel is longer than the 64-sample block, so the overlap
/// tail spans several blocks (the `i + input_len < overlap_len` branch).
#[test]
fn kernel_longer_than_block_matches_direct_convolution() {
    let kernel: Vec<f32> = (0..200)
        .map(|tap| noise(tap + 10_000) * (-(tap as f32) / 60.0).exp())
        .collect();
    let mut convolver = Convolver::new();
    convolver.init(64, &kernel);
    let input: Vec<f32> = (0..1500).map(noise).collect();
    let output = stream(&mut convolver, &input, &[64, 3, 64, 1, 37, 64]);
    assert_samples_match(
        &output,
        &direct_convolution(&input, &kernel),
        1e-4,
        "200-tap",
    );
}

/// The shared-spectrum API (`forward_fft` once, `process_with_spectrum`
/// per ear) is the HRTF renderer's hot path. Two ears sharing one input
/// spectrum must equal two independent `process` calls, block after block.
#[test]
fn shared_spectrum_path_equals_plain_process_for_both_ears() {
    let left_kernel: Vec<f32> = (0..140)
        .map(|tap| noise(tap + 500) * 0.9f32.powi(tap as i32))
        .collect();
    let right_kernel: Vec<f32> = (0..140)
        .map(|tap| noise(tap + 900) * 0.95f32.powi(tap as i32))
        .collect();
    let mut plain_left = Convolver::new();
    let mut plain_right = Convolver::new();
    let mut shared_left = Convolver::new();
    let mut shared_right = Convolver::new();
    plain_left.init(128, &left_kernel);
    plain_right.init(128, &right_kernel);
    shared_left.init(128, &left_kernel);
    shared_right.init(128, &right_kernel);
    assert_eq!(shared_left.fft_size(), shared_right.fft_size());

    let mut spectrum = vec![Complex::new(0.0, 0.0); shared_left.freq_len()];
    let input: Vec<f32> = (0..128 * 6 + 50).map(noise).collect();
    let mut position = 0;
    for length in [128, 128, 50, 128, 128, 128, 128] {
        let block = &input[position..position + length];
        let mut expected_left = vec![0.0; length];
        let mut expected_right = vec![0.0; length];
        plain_left.process(block, &mut expected_left);
        plain_right.process(block, &mut expected_right);

        let mut actual_left = vec![0.0; length];
        let mut actual_right = vec![0.0; length];
        shared_left.forward_fft(block, &mut spectrum);
        shared_left.process_with_spectrum(&spectrum, length, &mut actual_left);
        shared_right.process_with_spectrum(&spectrum, length, &mut actual_right);

        assert_samples_match(&actual_left, &expected_left, 1e-6, "left ear");
        assert_samples_match(&actual_right, &expected_right, 1e-6, "right ear");
        position += length;
    }
}

/// Linearity: conv(a·x + b·y) = a·conv(x) + b·conv(y).
#[test]
fn convolution_is_linear() {
    let kernel: Vec<f32> = (0..50).map(|tap| noise(tap + 77)).collect();
    let first: Vec<f32> = (0..400).map(noise).collect();
    let second: Vec<f32> = (0..400).map(|index| noise(index + 4000)).collect();
    let (scale_first, scale_second) = (0.7f32, -1.3f32);
    let combined: Vec<f32> = first
        .iter()
        .zip(&second)
        .map(|(x, y)| scale_first * x + scale_second * y)
        .collect();

    let run = |input: &[f32]| {
        let mut convolver = Convolver::new();
        convolver.init(32, &kernel);
        stream(&mut convolver, input, &[32, 7, 32])
    };
    let first_out = run(&first);
    let second_out = run(&second);
    let expected: Vec<f32> = first_out
        .iter()
        .zip(&second_out)
        .map(|(x, y)| scale_first * x + scale_second * y)
        .collect();
    assert_samples_match(&run(&combined), &expected, 1e-4, "linearity");
}

/// Time invariance: delaying the input by 37 samples (not a multiple of
/// the block size, so block boundaries fall at different signal phases)
/// delays the output by exactly 37 samples.
#[test]
fn convolution_is_time_invariant() {
    let kernel: Vec<f32> = (0..90).map(|tap| noise(tap + 123)).collect();
    let input: Vec<f32> = (0..600).map(noise).collect();
    let shift = 37;
    let mut shifted = vec![0.0f32; shift];
    shifted.extend_from_slice(&input);

    let mut convolver = Convolver::new();
    convolver.init(64, &kernel);
    let output = stream(&mut convolver, &input, &[64]);
    let mut convolver = Convolver::new();
    convolver.init(64, &kernel);
    let shifted_output = stream(&mut convolver, &shifted, &[64]);

    assert!(shifted_output[..shift]
        .iter()
        .all(|sample| sample.abs() < 1e-6));
    assert_samples_match(&shifted_output[shift..], &output, 1e-4, "time invariance");
}

/// `set_response` to a kernel of a different length (3 → 200 taps, which
/// also grows the FFT from 32 to 256) keeps the old kernel's tail: output
/// after the swap is Σ over past input of the kernel that was active when
/// that input sample was processed.
#[test]
fn set_response_with_new_length_keeps_the_old_tail() {
    let old_kernel = [0.0f32, 0.0, 1.0]; // delay by 2
    let new_kernel: Vec<f32> = (0..200)
        .map(|tap| noise(tap + 3000) * 0.97f32.powi(tap as i32))
        .collect();
    let mut convolver = Convolver::new();
    convolver.init(16, &old_kernel);
    let input: Vec<f32> = (0..160).map(noise).collect();

    let mut output = vec![0.0f32; input.len()];
    convolver.process(&input[..16], &mut output[..16]);
    convolver.set_response(&new_kernel);
    assert_eq!(convolver.fft_size(), 256);
    for start in (16..input.len()).step_by(16) {
        convolver.process(&input[start..start + 16], &mut output[start..start + 16]);
    }

    // Reference: first 16 input samples through the old kernel, the rest
    // through the new one, summed.
    let mut old_part = input.clone();
    old_part[16..].fill(0.0);
    let mut new_part = input.clone();
    new_part[..16].fill(0.0);
    let expected: Vec<f32> = direct_convolution(&old_part, &old_kernel)
        .iter()
        .zip(direct_convolution(&new_part, &new_kernel))
        .map(|(old, new)| old + new)
        .collect();
    assert_samples_match(&output, &expected, 1e-4, "kernel swap");
}

#[test]
fn continuous_stream_energy() {
    // Feed multiple blocks of a constant signal through a simple IR,
    // verify energy is continuous (no drops at block boundaries).
    let mut conv = Convolver::new();
    let ir = [0.5, 0.3, 0.2]; // Simple decaying IR
    conv.init(8, &ir);

    let mut all_output = Vec::new();
    for _ in 0..4 {
        let input = [1.0; 8];
        let mut output = [0.0; 8];
        conv.process(&input, &mut output);
        all_output.extend_from_slice(&output);
    }

    // After the initial ramp-up (first 2 samples), output should be
    // constant at 0.5 + 0.3 + 0.2 = 1.0 for a DC input.
    let expected_steady = 1.0;
    for (i, &s) in all_output.iter().enumerate().skip(2) {
        assert!(
            (s - expected_steady).abs() < 1e-4,
            "sample {i}: expected {expected_steady}, got {s}"
        );
    }
}
