use super::*;
#[test]
fn write_listening_samples() {
    let Ok(directory) = std::env::var("THUNDER_OUT") else {
        return;
    };
    std::fs::create_dir_all(&directory).expect("directory");
    let names = ["1-crack", "2-clap", "3-rolling", "4-distant"];
    for take in 0..4u8 {
        for seed in 1..=3u64 {
            let mut thunder = ThunderSource::new(Vec3::ZERO, seed * 7);
            thunder.schedule(Utterance {
                take,
                at_sample: 4_800,
                gain: 1.0,
                rate: 1.0,
                skip_seconds: 0.0,
            });
            let samples: Vec<f32> = (0..((clap_seconds(take) + 0.5) * 48_000.0) as usize)
                .map(|_| thunder.next_sample(48_000.0))
                .collect();
            // Each at the same peak, so they can be compared by ear.
            let peak = samples
                .iter()
                .fold(0f32, |peak, sample| peak.max(sample.abs()))
                .max(1e-6);
            // Up to where it has died away to 50 dB under its peak.
            let end = samples
                .iter()
                .rposition(|sample| sample.abs() > peak * 0.003)
                .map_or(samples.len(), |last| last + 4_800);
            let scaled: Vec<f32> = samples[..end.min(samples.len())]
                .iter()
                .map(|sample| sample * 0.8 / peak)
                .collect();
            crate::audio::wav::write_wav(
                std::path::Path::new(&format!("{directory}/{}-{seed}.wav", names[take as usize])),
                &scaled,
                1,
                48_000,
                crate::audio::wav::SampleFormat::Pcm16,
            )
            .expect("write");
        }
    }
}
