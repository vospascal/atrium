// Re-export core spatial modules so existing `crate::spatial::*` imports keep working.
pub use atrium_core::directivity;
pub use atrium_core::listener;
pub use atrium_core::panner;

// source stays local — TestNode depends on the audio crate.
pub mod source;
