//! The output tap: a copy of every block the audio thread finishes, for
//! measuring what is heard while it plays.
//!
//! Two lock-free rings, one of samples and one of block headers. The audio
//! thread writes a block's samples first and its header after, and only when
//! both fit; a block that does not fit is dropped whole, never in part. The
//! reader takes a header and then exactly its samples, so the rings cannot
//! fall out of step, and a dropped block shows as a jump in the headers'
//! clock. Nothing on the audio side blocks or allocates.

/// The shortest block a header is reserved for (frames). Output devices
/// call back with 64 frames or more; at 32 the header ring cannot fill
/// before the sample ring does.
const SHORTEST_BLOCK_FRAMES: usize = 32;

/// What one block was: where it starts on the scene's clock, and its shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TapBlock {
    /// The audio clock's sample count at the block's first frame.
    pub clock_sample: u64,
    pub frames: usize,
    pub channels: usize,
}

/// The audio thread's end: `AudioScene::render` pushes its finished output
/// here.
pub struct OutputTap {
    samples: rtrb::Producer<f32>,
    blocks: rtrb::Producer<TapBlock>,
}

/// The reader's end.
pub struct TapReader {
    samples: rtrb::Consumer<f32>,
    blocks: rtrb::Consumer<TapBlock>,
}

/// A tap that holds `frames` frames of `channels` channels before it drops
/// blocks.
pub fn output_tap(channels: usize, frames: usize) -> (OutputTap, TapReader) {
    let (samples, sample_reader) = rtrb::RingBuffer::new(channels.max(1) * frames);
    let (blocks, block_reader) = rtrb::RingBuffer::new((frames / SHORTEST_BLOCK_FRAMES).max(1));
    (
        OutputTap { samples, blocks },
        TapReader {
            samples: sample_reader,
            blocks: block_reader,
        },
    )
}

impl OutputTap {
    /// Copy one finished block of interleaved `output`, which starts at
    /// `clock_sample`; drop it whole if the reader has fallen behind.
    pub(crate) fn push(&mut self, output: &[f32], channels: usize, clock_sample: u64) {
        // Only this end adds to either ring, so room seen here is still
        // there below.
        if self.blocks.is_full() || self.samples.slots() < output.len() {
            return;
        }
        if self.samples.push_entire_slice(output).is_err() {
            return;
        }
        let _ = self.blocks.push(TapBlock {
            clock_sample,
            frames: output.len() / channels.max(1),
            channels,
        });
    }
}

impl TapReader {
    /// The next whole block, its samples interleaved into `samples`
    /// (resized to fit), or `None` when none is waiting.
    pub fn next_block(&mut self, samples: &mut Vec<f32>) -> Option<TapBlock> {
        let block = self.blocks.pop().ok()?;
        samples.resize(block.frames * block.channels, 0.0);
        // The samples were written before their header, so they are there.
        self.samples
            .pop_entire_slice(samples)
            .expect("a block's samples are written before its header");
        Some(block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(value: f32, frames: usize) -> Vec<f32> {
        vec![value; frames * 2]
    }

    /// Blocks come out as they went in, header and samples together.
    #[test]
    fn blocks_come_out_whole() {
        let (mut tap, mut reader) = output_tap(2, 1024);
        tap.push(&block(0.25, 256), 2, 1000);
        tap.push(&block(0.5, 128), 2, 1256);
        let mut samples = Vec::new();
        assert_eq!(
            reader.next_block(&mut samples),
            Some(TapBlock {
                clock_sample: 1000,
                frames: 256,
                channels: 2
            })
        );
        assert_eq!(samples, block(0.25, 256));
        let second = reader.next_block(&mut samples).expect("a second block");
        assert_eq!((second.clock_sample, second.frames), (1256, 128));
        assert_eq!(samples, block(0.5, 128));
        assert_eq!(reader.next_block(&mut samples), None);
    }

    /// A full tap that is never read drops whole blocks without panicking;
    /// once read, it holds the blocks that fitted, and the next block after
    /// a drop starts later on the clock than the last one read ended.
    #[test]
    fn a_full_tap_drops_whole_blocks() {
        let (mut tap, mut reader) = output_tap(2, 1000);
        let mut clock = 0;
        for index in 0..10 {
            tap.push(&block(index as f32, 300), 2, clock);
            clock += 300;
        }
        let mut samples = Vec::new();
        let mut read = Vec::new();
        while let Some(block) = reader.next_block(&mut samples) {
            assert!(samples.iter().all(|sample| *sample == samples[0]));
            read.push(block.clock_sample);
        }
        // 1000 frames hold three blocks of 300; the rest were dropped.
        assert_eq!(read, vec![0, 300, 600]);
        tap.push(&block(10.0, 300), 2, clock);
        let after = reader.next_block(&mut samples).expect("room again");
        assert_eq!(after.clock_sample, 3000);
        assert!(after.clock_sample > 600 + 300, "the gap shows on the clock");
        assert_eq!(samples, block(10.0, 300));
    }

    /// The header ring is the one that fills when blocks are tiny: a block
    /// whose header has no room is dropped, its samples not written.
    #[test]
    fn a_full_header_ring_drops_the_samples_too() {
        let (mut tap, mut reader) = output_tap(1, 64);
        // 64 / 32 = 2 headers, room for 64 samples.
        for clock in 0..4 {
            tap.push(&[clock as f32], 1, clock);
        }
        let mut samples = Vec::new();
        assert_eq!(
            reader.next_block(&mut samples).map(|b| b.clock_sample),
            Some(0)
        );
        assert_eq!(samples, [0.0]);
        assert_eq!(
            reader.next_block(&mut samples).map(|b| b.clock_sample),
            Some(1)
        );
        assert_eq!(samples, [1.0]);
        assert_eq!(reader.next_block(&mut samples), None);
    }
}
