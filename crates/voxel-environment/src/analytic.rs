//! Lightweight analytic provider retained for tests and fallback rendering.

use crate::{api::EnvironmentProvider, EnvironmentFrame, SunSettings, WGSL};

/// The non-LUT provider used by tests and as a fallback when a GPU adapter cannot
/// allocate the atmosphere resources. Its public shape intentionally matches the
/// Hillaire provider so consumers do not branch on implementation details.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnalyticProvider {
    settings: SunSettings,
}

impl Default for AnalyticProvider {
    fn default() -> Self {
        Self {
            settings: SunSettings::default(),
        }
    }
}

impl AnalyticProvider {
    pub fn new(settings: SunSettings) -> Self {
        Self { settings }
    }

    pub fn settings_mut(&mut self) -> &mut SunSettings {
        &mut self.settings
    }
}

impl EnvironmentProvider for AnalyticProvider {
    fn frame(&self) -> EnvironmentFrame {
        self.settings.environment_frame()
    }

    fn shader_source(&self) -> &'static str {
        WGSL
    }

    fn settings(&self) -> SunSettings {
        self.settings
    }
}
