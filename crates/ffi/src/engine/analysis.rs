//! What is heard, measured while it plays: the audio thread's output tap
//! drained into the live analyser on the caller's thread.
//!
//! The app polls `analysis` every display frame, as it polls telemetry; the
//! work is done in that call, not on the audio thread.

use std::sync::Arc;

use atrium::engine::tap::TapReader;
use atrium::plan::{Clock, Planner};
use atrium_analysis::{Column, Live};

use super::{lock, Engine};
use crate::api::{AnalysisColumn, AnalysisScale};

/// The spectrum's scale (dB SPL): 0 is the threshold of hearing; 90 is a
/// bird singing a metre away, above any band of a nature scene.
const SPECTRUM_FLOOR_DB: f64 = 0.0;
const SPECTRUM_TOP_DB: f64 = 90.0;

/// How much the tap holds before it drops blocks (s at the highest rate a
/// device is expected to open at). Its rate is not known until the stream
/// is open, after the scene has moved to the audio thread, so it is sized
/// for 96 kHz: 2 s there, 4 s at 48 kHz, ample for a caller that reads it
/// every display frame.
const TAP_SECONDS: f64 = 2.0;
const TAP_HIGHEST_RATE: f64 = 96_000.0;

#[uniffi::export]
impl Engine {
    /// The columns measured since the last call, oldest first. Each is 50 ms
    /// of what the output played; a column comes about 0.15 s after it
    /// sounded, once the measures that look ahead have their samples. Call
    /// it every display frame: what is not read within a few seconds is
    /// dropped, and the analysis starts again after the gap.
    pub fn analysis(&self) -> Vec<AnalysisColumn> {
        lock(&self.analysis).columns(&self.planner)
    }

    /// How to read `analysis`: the spectrum's dB range and rows, and the
    /// time a column covers.
    pub fn analysis_scale(&self) -> AnalysisScale {
        AnalysisScale {
            spectrum_floor_db: SPECTRUM_FLOOR_DB as f32,
            spectrum_top_db: SPECTRUM_TOP_DB as f32,
            row_edges_hertz: Live::row_edges_hertz()
                .into_iter()
                .map(|edge| edge as f32)
                .collect(),
            column_seconds: Live::COLUMN_SECONDS as f32,
        }
    }
}

/// The tap's reader and the analyser it feeds.
pub(super) struct Analysis {
    reader: TapReader,
    /// The audio thread's clock, which the tap's blocks are stamped on.
    clock: Arc<Clock>,
    live: Live,
    sample_rate: u32,
    channels: usize,
    full_scale_db_spl: f64,
    block: Vec<f32>,
    /// Blocks that start before this clock sample were played before the
    /// last seek, and are not read.
    from_clock_sample: u64,
}

impl Analysis {
    /// Frames the tap holds per channel.
    pub(super) fn tap_frames() -> usize {
        (TAP_SECONDS * TAP_HIGHEST_RATE) as usize
    }

    pub(super) fn new(
        reader: TapReader,
        clock: Arc<Clock>,
        sample_rate: u32,
        channels: usize,
        full_scale_db_spl: f64,
    ) -> Self {
        Self {
            reader,
            clock,
            live: Live::new(sample_rate, channels, full_scale_db_spl),
            sample_rate,
            channels,
            full_scale_db_spl,
            block: Vec::new(),
            from_clock_sample: 0,
        }
    }

    /// Feed every block waiting in the tap to the analyser, and take the
    /// columns it finished, placed on the scene's time by `planner`.
    fn columns(&mut self, planner: &Planner) -> Vec<AnalysisColumn> {
        while let Some(block) = self.reader.next_block(&mut self.block) {
            // A block of another shape is not this stream's; the device's
            // channels are fixed while it plays, so none comes.
            if block.clock_sample >= self.from_clock_sample && block.channels == self.channels {
                self.live.push(&self.block, block.clock_sample);
            }
        }
        self.live
            .take_columns()
            .into_iter()
            .map(|column| {
                let scene_seconds = planner.scene_seconds_at(column.clock_sample);
                column_record(column, scene_seconds)
            })
            .collect()
    }

    /// The scene has just moved: what was played before is not of the new
    /// time, and what the analyser holds of it (the columns still waiting
    /// for samples, FAST, the onsets' past second) is let go.
    pub(super) fn seek(&mut self) {
        self.from_clock_sample = self.clock.samples();
        self.live = Live::new(self.sample_rate, self.channels, self.full_scale_db_spl);
    }
}

/// `column` for the app, at `scene_seconds`.
fn column_record(column: Column, scene_seconds: f64) -> AnalysisColumn {
    let as_f32 = |values: Vec<f64>| values.into_iter().map(|value| value as f32).collect();
    AnalysisColumn {
        scene_seconds,
        spectrum: column
            .spectrum_decibels
            .iter()
            .map(|level| spectrum_value(*level))
            .collect(),
        leq_db: as_f32(column.leq_decibels),
        laf_db: as_f32(column.laf_decibels),
        leq_overall_db: column.leq_overall_decibels as f32,
        laf_overall_db: column.laf_overall_decibels as f32,
        onset: column.onset,
        width: column.width.map(|width| width as f32),
        ild_db: column.ild_decibels.map(|ild| ild as f32),
    }
}

/// A band level (dB SPL) on the spectrum's 0–255 scale.
fn spectrum_value(level: f64) -> u8 {
    let fraction = (level - SPECTRUM_FLOOR_DB) / (SPECTRUM_TOP_DB - SPECTRUM_FLOOR_DB);
    (fraction * 255.0).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use atrium::config::SceneConfig;
    use atrium::engine::scene::AudioScene;
    use atrium::engine::tap::output_tap;
    use std::time::{Duration, Instant};

    const RATE: u32 = 48_000;
    const BLOCK: usize = 512;

    /// A scene built and prepared as the output device prepares it, its tap
    /// read by an `Analysis`, its plan running: everything but the device.
    struct Playing {
        scene: AudioScene,
        planner: Planner,
        analysis: Analysis,
        channels: usize,
        block: Vec<f32>,
    }

    impl Playing {
        fn new(path: &str) -> Self {
            std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
                .expect("repository root");
            let document = SceneConfig::load(path).expect("scene loads");
            let full_scale = document.normalization.full_scale_db_spl();
            let built = document.build().expect("scene builds");
            let mut scene = built.scene;
            let channels = scene.speaker_layout.total_channels();
            scene.sample_rate = RATE as f32;
            scene.fit_to_output_device(channels);
            scene.init_pipelines();
            let (tap, reader) = output_tap(channels, Analysis::tap_frames());
            scene.output_tap = Some(tap);
            let analysis = Analysis::new(reader, scene.clock.clone(), RATE, channels, full_scale);
            Self {
                scene,
                planner: built.planner,
                analysis,
                channels,
                block: vec![0.0; BLOCK * channels],
            }
        }

        /// Play `frames` more, as the audio callback does, at most a block
        /// at a time.
        fn play(&mut self, frames: usize) {
            let mut left = frames;
            while left > 0 {
                let now = left.min(BLOCK);
                self.scene
                    .render(&mut self.block[..now * self.channels], self.channels);
                left -= now;
            }
        }

        fn columns(&mut self) -> Vec<AnalysisColumn> {
            self.analysis.columns(&self.planner)
        }
    }

    /// Columns are 50 ms apart in scene seconds, the first at 0. After the
    /// scene is moved to 120 s, nothing played before reaches the new time,
    /// and the next column starts at 120 s plus what has played since (none
    /// here: the move falls between two blocks).
    #[test]
    fn columns_land_on_the_scene_time() {
        let mut playing = Playing::new("scenes/meadow-birds.yaml");
        let mut columns = Vec::new();
        // Read every 2 blocks, about a display frame.
        for _ in 0..(2 * RATE as usize) / (2 * BLOCK) {
            playing.play(2 * BLOCK);
            columns.extend(playing.columns());
        }
        assert!(columns.len() > 30, "{} columns", columns.len());
        assert!(columns[0].scene_seconds.abs() < 1e-9);
        for pair in columns.windows(2) {
            let step = pair[1].scene_seconds - pair[0].scene_seconds;
            assert!((step - 0.05).abs() < 1e-9, "{step}");
        }
        assert_eq!(columns[0].spectrum.len(), 96);
        assert_eq!(columns[0].leq_db.len(), playing.channels);

        // Some of what played before the move is still waiting in the
        // analyser and in the tap; none of it may come out after.
        playing.play(BLOCK);
        playing.planner.seek(120.0);
        playing.analysis.seek();
        playing.play(RATE as usize / 2);
        let after = playing.columns();
        let first = after.first().expect("columns after the move");
        assert!(
            (first.scene_seconds - 120.0).abs() < 0.1,
            "{}",
            first.scene_seconds
        );
        assert!(after.iter().all(|column| column.scene_seconds >= 120.0));
        for pair in after.windows(2) {
            assert!((pair[1].scene_seconds - pair[0].scene_seconds - 0.05).abs() < 1e-9);
        }
    }

    /// Through the device, at zero volume: columns come while the scene
    /// plays, 50 ms apart in its time and behind the playhead; after a seek
    /// to 120 s the next is within 0.1 s of 120 s plus what has played
    /// since. `cargo test -p atrium-ffi -- --ignored columns_while_playing`.
    #[test]
    #[ignore = "opens the default output device"]
    fn columns_while_playing() {
        std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .expect("repository root");
        let engine = Engine::new("scenes/meadow-birds.yaml".into()).expect("scene plays");
        engine.set_volume(0.0).expect("volume");
        let poll = |seconds: f64| {
            let mut columns = Vec::new();
            let until = Instant::now() + Duration::from_secs_f64(seconds);
            while Instant::now() < until {
                columns.extend(engine.analysis());
                std::thread::sleep(Duration::from_millis(16));
            }
            columns
        };
        let columns = poll(1.5);
        assert!(columns.len() > 15, "{} columns", columns.len());
        for pair in columns.windows(2) {
            assert!((pair[1].scene_seconds - pair[0].scene_seconds - 0.05).abs() < 1e-6);
        }
        let last = columns.last().expect("columns").scene_seconds;
        assert!(
            last < engine.scene_time() as f64,
            "a column ahead of the playhead"
        );

        engine.seek(120.0);
        let after = poll(1.0);
        let first = after.first().expect("columns after the seek");
        // Its first sample is the first the device played after the seek,
        // within a callback of it.
        println!("first column after the seek: {} s", first.scene_seconds);
        assert!(
            (120.0..120.1).contains(&first.scene_seconds),
            "{}",
            first.scene_seconds
        );
        assert!(after.iter().all(|column| column.scene_seconds >= 120.0));
    }

    /// The scale is the engine's: 96 rows between 97 edges from 20 Hz to
    /// 20 kHz, and a level maps onto 0–255 across it.
    #[test]
    fn the_scale_reads_the_spectrum() {
        assert_eq!(spectrum_value(SPECTRUM_FLOOR_DB - 10.0), 0);
        assert_eq!(spectrum_value(SPECTRUM_TOP_DB + 10.0), 255);
        assert_eq!(spectrum_value(45.0), 128); // 127.5 rounds up
        let edges = Live::row_edges_hertz();
        assert_eq!(edges.len(), 97);
        assert!((edges[0] - 20.0).abs() < 1e-9 && (edges[96] - 20_000.0).abs() < 1e-6);
    }

    /// `Engine::analysis` costs under 1 ms for 1/60 s of 5.1 at 48 kHz:
    /// the storm scene on its six speakers, read every 800 frames.
    /// `cargo test -p atrium-ffi --release -- --ignored analysis_costs`.
    #[test]
    #[ignore = "a timing; run alone, in release"]
    fn analysis_costs_under_a_millisecond_a_display_frame() {
        let mut playing = Playing::new("scenes/passing-storm.yaml");
        assert_eq!(playing.channels, 6);
        let display_frame = RATE as usize / 60;
        let mut times = Vec::new();
        let mut columns = 0;
        // Render as the device would, 800 frames between reads; time only
        // the reads, after the first second.
        for read in 0..600 {
            playing.play(display_frame);
            let started = Instant::now();
            columns += playing.columns().len();
            if read >= 60 {
                times.push(started.elapsed());
            }
        }
        times.sort();
        let mean = times.iter().sum::<Duration>() / times.len() as u32;
        let median = times[times.len() / 2];
        let worst = times[times.len() - 1];
        let p90 = times[times.len() * 9 / 10];
        let p99 = times[times.len() * 99 / 100];
        println!("analysis(): mean {mean:?}, median {median:?}, 90 % {p90:?}, 99 % {p99:?}, worst {worst:?}, {columns} columns");
        assert!(mean < Duration::from_millis(1), "mean {mean:?}");
    }
}
