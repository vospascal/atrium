//! Shared GPU-adapter seam.
//!
//! Concrete `wgpu` resources remain an implementation detail of the renderer adapter.
//! This module is reserved for the small invalidation/binding contract shared by the
//! Hillaire and fallback providers once the LUT passes are wired.
