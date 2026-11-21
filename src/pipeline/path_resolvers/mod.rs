//! Concrete `PathResolver` implementations.
//!
//! - `DirectPathResolver`: always returns 1 direct line-of-sight path.
//! - `ImageSourceResolver`: 1 direct + up to 6 first-order wall reflections.

mod direct;
mod image_source;
#[cfg(test)]
mod tests;

pub use direct::DirectPathResolver;
pub use image_source::ImageSourceResolver;
