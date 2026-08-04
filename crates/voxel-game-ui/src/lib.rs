//! Game-facing panels: the settings window and the performance readout.
//!
//! Both are toggled windows rather than always-on overlay, for the reason the settings module
//! records: a debug UI that covers the render is measuring the wrong thing.
//!
//! Deliberately independent of `voxel-studio`. The two reference each other zero times in either
//! direction, which is why they are siblings rather than a chain — `voxel` composes both.

pub mod performance_panel;
pub mod settings_panel;
