// Procedural audio synthesis — environmental sound sources.
//
// Ported from the TypeScript spatial-audio-garden AudioWorklet processors.
// All generators are allocation-free in the hot path and suitable for
// real-time audio threads (no heap alloc, no locks, no syscalls).
//
// Architecture difference from TS: the original processors included inline
// reverb (Freeverb). Here, sources produce dry mono samples — the existing
// FdnReverb processor in the audio pipeline handles reverberation.

pub mod noise;
pub mod rain;
pub mod rain_v2;
pub mod wave;
pub mod wind;
