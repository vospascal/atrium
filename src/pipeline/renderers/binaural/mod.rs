//! HrtfRenderer — binaural rendering to stereo headphones via HRTF convolution.
//!
//! The **direct path** gets full HRTF convolution: the HRIR for the source's
//! direction is loaded from a SOFA file and applied via overlap-add FFT.
//! Two convolver pairs (A/B double-buffer) enable click-free IR crossfading
//! when the source direction changes.
//!
//! **Reflection paths** use cheap equal-power stereo panning from their
//! azimuth angle — no FFT convolution. This keeps CPU cost manageable
//! (reflections contribute spaciousness, not precise localization).
//!
//! The HRIRs were measured at one distance. A **direct path closer than
//! that** gets each ear corrected by the rigid sphere's distance variation
//! function (`near_field`), so a source beside the head is heard beside it
//! and not merely loud. Reflections arrive from image sources far away and
//! never need it.
//!
//! **Ambient sound** has no direction to look up. It joins one bus, and two
//! fixed filters derived from all the measured directions together give
//! each ear what this head hears of a diffuse field (`diffuse_field`).
//!
//! **Equalised for headphones**, the head's diffuse-field response is
//! divided out of every source's convolved direct sound, once, on their sum
//! (`diffuse_field`'s equaliser): it holds the measurement loudspeaker and
//! the dummy head's ear canal, which the listener's own ear canal would
//! otherwise hear on top of its own. A diffuse field then reaches the ears
//! flat. The probe can switch it off (`equalised_share`).

mod diffuse_field;
mod direction;
mod equalised_share;
mod itd_delay;
mod near_field;
mod renderer;
mod source_state;

pub use renderer::HrtfRenderer;
