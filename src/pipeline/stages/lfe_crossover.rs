//! LFE bass management with Linkwitz-Riley 4th-order (LR4) crossover at 120 Hz.
//!
//! When the layout has an LFE channel:
//! - LFE receives LR4 lowpass (two cascaded Butterworth 2nd-order LP sections)
//! - All non-LFE channels receive LR4 highpass (two cascaded Butterworth HP sections)
//! - Bass content removed from mains is redirected (summed) into the LFE channel
//!
//! LR4's reconstruction property: LP(f) + HP(f) is an *allpass* — flat
//! magnitude at all frequencies, with LP and HP mutually in phase. Each branch
//! is -6 dB at crossover, so their sum is 0 dB. Note LP + HP ≠ 1 as a transfer
//! function (the sum carries the allpass phase), which is why the bass redirect
//! uses an explicit parallel lowpass rather than `original - highpassed`.
//!
//! No-op for layouts without LFE (e.g. stereo, quad).

use atrium_core::speaker::MAX_CHANNELS;

use crate::audio::filters::Biquad;
use crate::pipeline::mix_stage::{MixContext, MixStage};

/// LFE crossover cutoff frequency in Hz.
const LFE_CUTOFF_HZ: f32 = 120.0;

/// Linkwitz-Riley 4th-order filter: two cascaded identical Butterworth biquads.
#[derive(Clone)]
struct Lr4Filter {
    stage1: Biquad,
    stage2: Biquad,
}

impl Lr4Filter {
    fn lowpass(cutoff_hz: f32, sample_rate: f32) -> Self {
        Self {
            stage1: Biquad::lowpass(cutoff_hz, sample_rate),
            stage2: Biquad::lowpass(cutoff_hz, sample_rate),
        }
    }

    fn highpass(cutoff_hz: f32, sample_rate: f32) -> Self {
        Self {
            stage1: Biquad::highpass(cutoff_hz, sample_rate),
            stage2: Biquad::highpass(cutoff_hz, sample_rate),
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        self.stage2.process(self.stage1.process(x))
    }

    fn reset(&mut self) {
        self.stage1.reset();
        self.stage2.reset();
    }
}

/// LFE bass management stage with Linkwitz-Riley 4th-order crossover.
///
/// Only active when the layout has an LFE channel and that channel is not
/// masked by the channel mode. Applies:
/// - LR4 lowpass to the LFE channel
/// - LR4 highpass to all non-LFE channels
/// - Redirects bass removed from mains into the LFE
///
/// No-op for layouts without an LFE and for channel modes without a subwoofer.
pub struct LfeBassManagementStage {
    /// LR4 lowpass for the LFE channel.
    lfe_lowpass: Option<Lr4Filter>,
    /// LR4 highpass per non-LFE channel. Only populated for active channels.
    main_highpass: [Option<Lr4Filter>; MAX_CHANNELS],
    /// Parallel LR4 lowpass per non-LFE channel, for the bass redirect.
    ///
    /// The redirected bass must be a *true* LR4 lowpass of each main channel.
    /// `original - highpassed` is NOT that: LP4 + HP4 sums to an allpass, not
    /// to unity, so `1 - HP4` equals `LP4 + (1 - allpass)` — a term that peaks
    /// at 1.5× at the crossover (vs the correct 0.5×) and leaks midrange into
    /// the LFE at only −9 dB @ 1 kHz.
    main_lowpass: [Option<Lr4Filter>; MAX_CHANNELS],
    /// Cached LFE channel index.
    lfe_channel: Option<usize>,
}

impl Default for LfeBassManagementStage {
    fn default() -> Self {
        Self {
            lfe_lowpass: None,
            main_highpass: std::array::from_fn(|_| None),
            main_lowpass: std::array::from_fn(|_| None),
            lfe_channel: None,
        }
    }
}

impl LfeBassManagementStage {
    pub fn new() -> Self {
        Self::default()
    }
}

impl MixStage for LfeBassManagementStage {
    fn init(&mut self, ctx: &MixContext) {
        let lfe = match ctx.layout.lfe_channel() {
            // LFE channel must fit within the actual output buffer width.
            Some(ch) if ch < ctx.channels => ch,
            _ => {
                self.lfe_lowpass = None;
                self.lfe_channel = None;
                self.main_highpass = std::array::from_fn(|_| None);
                self.main_lowpass = std::array::from_fn(|_| None);
                return;
            }
        };

        self.lfe_channel = Some(lfe);
        self.lfe_lowpass = Some(Lr4Filter::lowpass(LFE_CUTOFF_HZ, ctx.sample_rate));

        // Create HP + parallel LP filters for all non-LFE channels.
        self.main_highpass = std::array::from_fn(|ch| {
            if ch < ctx.channels && ch != lfe {
                Some(Lr4Filter::highpass(LFE_CUTOFF_HZ, ctx.sample_rate))
            } else {
                None
            }
        });
        self.main_lowpass = std::array::from_fn(|ch| {
            if ch < ctx.channels && ch != lfe {
                Some(Lr4Filter::lowpass(LFE_CUTOFF_HZ, ctx.sample_rate))
            } else {
                None
            }
        });
    }

    fn process(&mut self, buffer: &mut [f32], ctx: &MixContext) {
        let lfe = match self.lfe_channel {
            Some(ch) if ch < ctx.channels => ch,
            _ => return,
        };
        // No subwoofer on this rig (quad or stereo channel mode): leave the
        // mains full-range. Splitting them here would send everything below
        // the crossover to a channel nothing is connected to. Checked every
        // buffer because the channel mode can change while running.
        if !ctx.layout.is_channel_active(lfe) {
            return;
        }
        let lfe_lp = match self.lfe_lowpass.as_mut() {
            Some(f) => f,
            None => return,
        };

        let channels = ctx.channels;
        let num_frames = buffer.len() / channels;

        for frame in 0..num_frames {
            let base = frame * channels;

            // Sum bass content redirected from main channels: a true parallel
            // LR4 lowpass per channel. All LR4 branches share the same phase
            // response, so the redirected bass from multiple mains (and the
            // lowpassed LFE input) sums coherently.
            let mut bass_sum = 0.0f32;
            for ch in 0..channels.min(MAX_CHANNELS) {
                let (Some(hp), Some(lp)) =
                    (&mut self.main_highpass[ch], &mut self.main_lowpass[ch])
                else {
                    continue;
                };
                let idx = base + ch;
                let original = buffer[idx];
                buffer[idx] = hp.process(original);
                bass_sum += lp.process(original);
            }

            // LFE channel: lowpass existing LFE content, then add redirected bass
            // (already LR4-lowpassed above — it bypasses the LFE LP to avoid
            // double-filtering).
            let lfe_idx = base + lfe;
            buffer[lfe_idx] = lfe_lp.process(buffer[lfe_idx]) + bass_sum;
        }
    }

    fn reset(&mut self) {
        if let Some(ref mut f) = self.lfe_lowpass {
            f.reset();
        }
        for f in self.main_highpass.iter_mut().flatten() {
            f.reset();
        }
        for f in self.main_lowpass.iter_mut().flatten() {
            f.reset();
        }
    }

    fn name(&self) -> &str {
        "lfe_bass_management"
    }
}

#[cfg(test)]
mod tests;
