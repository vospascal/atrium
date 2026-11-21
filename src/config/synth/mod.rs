//! Procedural synth definitions. [`SynthSpec`] is the tagged union a source
//! definition's `synth:` field selects; each variant's parameters and the code
//! that configures its generator live in that synth's own file.

mod canopy_wind;
mod field_wind;
mod rain;
mod rain_v2;
mod river;
mod soft_wind;
mod storm_wind;
mod thunder;
mod waves;
mod wind_behaviour;
mod wisp;

#[cfg(test)]
mod tests;

use atrium_core::source::SoundSource;
use atrium_core::types::Vec3;
use serde::Deserialize;

use canopy_wind::CanopyWindSynthParams;
use field_wind::FieldWindSynthParams;
use rain::RainSynthParams;
use rain_v2::RainV2SynthParams;
use river::RiverSynthParams;
use soft_wind::SoftWindSynthParams;
use storm_wind::StormWindSynthParams;
use thunder::ThunderSynthParams;
use waves::WaveSynthParams;
use wisp::WispSynthParams;

/// Which procedural generator to run, with its parameters. Tagged by the
/// `synth:` field. Every parameter is optional — unset fields keep the
/// generator's own defaults, so YAML only states what it wants to change.
#[derive(Deserialize, Clone, Debug)]
#[serde(tag = "synth", rename_all = "snake_case")]
pub(in crate::config) enum SynthSpec {
    CanopyWind(CanopyWindSynthParams),
    FieldWind(FieldWindSynthParams),
    SoftWind(SoftWindSynthParams),
    StormWind(StormWindSynthParams),
    Rain(RainSynthParams),
    RainV2(RainV2SynthParams),
    River(RiverSynthParams),
    Thunder(ThunderSynthParams),
    Waves(WaveSynthParams),
    Wisp(WispSynthParams),
}

impl SynthSpec {
    pub(in crate::config) fn kind_name(&self) -> &'static str {
        match self {
            SynthSpec::CanopyWind(_) => "canopy_wind",
            SynthSpec::FieldWind(_) => "field_wind",
            SynthSpec::SoftWind(_) => "soft_wind",
            SynthSpec::StormWind(_) => "storm_wind",
            SynthSpec::Rain(_) => "rain",
            SynthSpec::RainV2(_) => "rain_v2",
            SynthSpec::River(_) => "river",
            SynthSpec::Thunder(_) => "thunder",
            SynthSpec::Waves(_) => "waves",
            SynthSpec::Wisp(_) => "wisp",
        }
    }

    /// Construct the generator at `position`. `default_seed` applies when the
    /// params don't pin one — the scene builder passes a slot-stable seed so
    /// reloading a scene sounds statistically identical.
    /// A generator that is told when to sound has a rhythm: its takes and
    /// rests, for the session's plan. One that sounds all the time has none.
    pub(in crate::config) fn rhythm(&self) -> Option<crate::plan::Rhythm> {
        match self {
            SynthSpec::Thunder(params) => Some(params.rhythm()),
            _ => None,
        }
    }

    pub(in crate::config) fn build_generator(
        &self,
        position: Vec3,
        default_seed: u64,
    ) -> Box<dyn SoundSource> {
        match self {
            SynthSpec::CanopyWind(params) => params.build_generator(position, default_seed),
            SynthSpec::FieldWind(params) => params.build_generator(position, default_seed),
            SynthSpec::SoftWind(params) => params.build_generator(position, default_seed),
            SynthSpec::StormWind(params) => params.build_generator(position, default_seed),
            SynthSpec::Rain(params) => params.build_generator(position, default_seed),
            SynthSpec::RainV2(params) => params.build_generator(position, default_seed),
            SynthSpec::River(params) => params.build_generator(position, default_seed),
            SynthSpec::Thunder(params) => params.build_generator(position, default_seed),
            SynthSpec::Waves(params) => params.build_generator(position, default_seed),
            SynthSpec::Wisp(params) => params.build_generator(position, default_seed),
        }
    }
}
