//! Render passes. Each pass is a self-contained unit with a consistent shape:
//! `new(device, ...)` creates all GPU resources, `rebind(device, ...)` refreshes
//! size-dependent bindings after a resize, and `encode(...)` records its work
//! into a caller-owned command encoder. The frame loop composes passes; later
//! stages add shadow / CAGI / post passes with the same shape.

pub mod blit;
pub mod dda;
