//! Hillaire/Jolifanto atmosphere provider.

mod lut;
mod resources;
mod shaders;

use crate::{api::EnvironmentProvider, EnvironmentFrame, SunSettings};

pub use lut::{AtmosphereLutPasses, LutConfig, LutUpdate};
pub use resources::{
    AtmosphereBindings, AtmosphereResources, AtmosphereUniform, LutKind, ATMOSPHERE_BIND_GROUP,
};

/// Hillaire atmosphere provider. The LUT resources and compute-pass adapter are kept
/// private to this module; consumers only depend on the stable provider contract.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HillaireProvider {
    settings: SunSettings,
}

impl Default for HillaireProvider {
    fn default() -> Self {
        Self {
            settings: SunSettings::default(),
        }
    }
}

impl HillaireProvider {
    pub fn new(settings: SunSettings) -> Self {
        Self { settings }
    }

    pub fn settings_mut(&mut self) -> &mut SunSettings {
        &mut self.settings
    }
}

impl EnvironmentProvider for HillaireProvider {
    fn frame(&self) -> EnvironmentFrame {
        self.settings.environment_frame()
    }

    fn shader_source(&self) -> &'static str {
        shaders::WGSL
    }

    fn settings(&self) -> SunSettings {
        self.settings
    }
}
