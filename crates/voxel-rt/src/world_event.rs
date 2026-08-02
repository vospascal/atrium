//! World events: things that happened somewhere, at a time, with a reach.
//!
//! This is the world-level signal materials react to — a player standing near
//! a wall, a mob passing, a block being struck. It is deliberately not a
//! "camera proximity" facility: the camera is simply the first producer, and a
//! mob system later raises alongside it without the renderer, the shader or
//! any node contract changing.
//!
//! # Why an event rather than a proximity field
//!
//! A sensor that only knows *"something is 2.1 m away"* cannot fade in over
//! 0.4 s or hold after the thing leaves — that needs per-voxel history, and the
//! shading pass keeps none. An event carries a **start timestamp**, so the
//! shader computes an elapsed time and runs an attack/hold/release envelope
//! statelessly, per pixel, exactly. The state lives here, once per event,
//! instead of once per voxel.
//!
//! # The one rule everything rests on
//!
//! [`WorldEventField::raise`] is idempotent on its key: re-raising an OPEN
//! event updates where it is but preserves when it started. That is what
//! separates "the entity is still standing there" from "the entity arrived",
//! and it is the whole temporal model. Re-raising a CLOSED event is an
//! arrival and gets a fresh timestamp.

use crate::animation_clock::AnimationClockSample;

/// How many events the uniform carries. Sized for a handful of entities plus
/// transient impacts; the overflow policy below makes the cap visible rather
/// than silent.
pub const MAX_WORLD_EVENTS: usize = 16;

/// The longest an event may influence anything after it closes.
///
/// This bounds three separate things at once, which is why it is one constant:
/// slot reclamation here, the elapsed-time precision argument in
/// [`crate::animation_clock`], and the total post-close envelope a sensor may
/// author. A sensor's `hold_seconds + release_seconds` must not exceed it, or
/// the sensor would still be fading after its event had been reclaimed — the
/// material graph reports that as a compile diagnostic rather than letting the
/// runtime discover it.
pub const MAX_EVENT_LIFETIME_SECONDS: f32 = 8.0;

/// The presence channel: an entity simply being somewhere. Channel ids are
/// authored on the sensor node, so new kinds cost nothing here.
pub const CHANNEL_PRESENCE: u32 = 0;

/// Identifies an event across frames, so a re-raise is recognised as the same
/// event rather than a new arrival. Producers own their key space.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct EventKey(pub u64);

impl EventKey {
    /// The active eye. The only producer today.
    pub const CAMERA: Self = Self(0);
}

/// What a producer supplies when raising. Timestamps are the field's business,
/// not the caller's — that is what keeps the idempotency rule enforceable.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EventSpec {
    pub position_meters: [f32; 3],
    pub radius_meters: f32,
    pub channel: u32,
    pub strength: f32,
}

/// One event, in the exact layout the GPU receives — and the exact type the
/// CPU material backend evaluates, so a preview and a rendered pixel cannot
/// drift apart.
///
/// **Three explicit 16-byte rows.** The natural field set is 44 bytes at align 4
/// under `#[repr(C)]`; the WGSL struct is 48, because in the uniform address
/// space array elements stride to a multiple of 16. Without the named pad the
/// Rust upload desynchronises from element 1 onward. Same discipline as
/// `GpuMaterial` and `GpuPatternLayer`.
///
/// The rule is [WGSL § Address Space Layout
/// Constraints](https://gpuweb.github.io/gpuweb/wgsl/#address-space-layout-constraints),
/// and it is worth reading rather than recalling: it applies only when the
/// `uniform_buffer_standard_layout` language extension is ABSENT (with it,
/// uniform buffers lay out like storage ones). It is optional, so we never
/// assume it and always pad explicitly. The same section carries the companion
/// rule that catches people out — a struct-typed member must be followed by at
/// least `roundUp(16, SizeOf(S))` bytes, which is a minimum SPACING requirement,
/// not an alignment one.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GpuWorldEvent {
    // ---- row 0 ----
    pub position_meters: [f32; 3],
    /// Beyond this the event contributes nothing. A big entity reaches further.
    pub radius_meters: f32,
    // ---- row 1 ----
    pub started_epoch: f32,
    pub started_remainder_seconds: f32,
    /// Meaningless while `open` is 1.0; the shader tests `open` first.
    pub ended_epoch: f32,
    pub ended_remainder_seconds: f32,
    // ---- row 2 ----
    /// Which kind of event. Sensors filter on it.
    pub channel: u32,
    /// Intensity in `[0, 1]`, clamped on ingest. Multiplies the sensor's
    /// `signal` output only, so `nearness` and `envelope` stay literal.
    pub strength: f32,
    /// 1.0 while the event is ongoing, 0.0 once it has closed. A flag rather
    /// than a sentinel timestamp, so no shader path ever does arithmetic on
    /// `f32::MAX` and produces an infinity.
    pub open: f32,
    pub _pad_row2: f32,
}

impl GpuWorldEvent {
    /// An unused slot: zero radius and zero strength make it contribute
    /// nothing even if a loop bound were ever wrong.
    pub const INACTIVE: Self = Self {
        position_meters: [0.0; 3],
        radius_meters: 0.0,
        started_epoch: 0.0,
        started_remainder_seconds: 0.0,
        ended_epoch: 0.0,
        ended_remainder_seconds: 0.0,
        channel: 0,
        strength: 0.0,
        open: 0.0,
        _pad_row2: 0.0,
    };

    pub fn is_open(&self) -> bool {
        self.open > 0.5
    }

    /// Seconds since this event started. Negative means "not yet".
    pub fn elapsed_since_start(&self, clock: AnimationClockSample) -> f32 {
        clock.elapsed_since(self.started_epoch, self.started_remainder_seconds)
    }

    /// Seconds since this event closed. Zero while it is still open, so a
    /// caller can use it unconditionally.
    pub fn elapsed_since_end(&self, clock: AnimationClockSample) -> f32 {
        if self.is_open() {
            0.0
        } else {
            clock.elapsed_since(self.ended_epoch, self.ended_remainder_seconds)
        }
    }
}

unsafe impl bytemuck::Zeroable for GpuWorldEvent {}
unsafe impl bytemuck::Pod for GpuWorldEvent {}

/// The live event set. Owns event identity across frames — which is the only
/// reason an envelope can work at all.
#[derive(Clone, Debug)]
pub struct WorldEventField {
    events: [GpuWorldEvent; MAX_WORLD_EVENTS],
    keys: [Option<EventKey>; MAX_WORLD_EVENTS],
    count: usize,
    overflow_count: u64,
}

impl Default for WorldEventField {
    fn default() -> Self {
        Self::new()
    }
}

impl WorldEventField {
    pub fn new() -> Self {
        Self {
            events: [GpuWorldEvent::INACTIVE; MAX_WORLD_EVENTS],
            keys: [None; MAX_WORLD_EVENTS],
            count: 0,
            overflow_count: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// How many raises have been rejected for want of a slot. Surfaced in the
    /// overlay so the cap is visible instead of silent.
    pub fn overflow_count(&self) -> u64 {
        self.overflow_count
    }

    /// The active events, for upload and for CPU evaluation.
    pub fn active(&self) -> &[GpuWorldEvent] {
        &self.events[..self.count]
    }

    /// The fixed-size array the uniform buffer takes. Inactive slots are
    /// zeroed rather than absent, matching how the pattern rows upload.
    pub fn upload_array(&self) -> [GpuWorldEvent; MAX_WORLD_EVENTS] {
        self.events
    }

    /// Raise or refresh an event.
    ///
    /// - Key not present: a new event, stamped now. Rejected (and counted) if
    ///   the field is full.
    /// - Key present and OPEN: position, radius, channel and strength update;
    ///   **the start timestamp is preserved**. The rule the envelope depends on.
    /// - Key present but CLOSED: the thing came back. Reopened with a fresh
    ///   timestamp, because a return is an arrival, not a continuation.
    ///
    /// Returns whether the field accepted it.
    pub fn raise(&mut self, key: EventKey, spec: EventSpec, clock: AnimationClockSample) -> bool {
        let strength = spec.strength.clamp(0.0, 1.0);
        if let Some(index) = self.index_of(key) {
            let event = &mut self.events[index];
            let reopening = !event.is_open();
            event.position_meters = spec.position_meters;
            event.radius_meters = spec.radius_meters.max(0.0);
            event.channel = spec.channel;
            event.strength = strength;
            if reopening {
                event.started_epoch = clock.epoch;
                event.started_remainder_seconds = clock.remainder_seconds;
                event.open = 1.0;
            }
            return true;
        }
        if self.count >= MAX_WORLD_EVENTS {
            // Reject rather than evict. Any eviction rule makes an
            // already-lit surface pop dark when an unrelated entity appears
            // somewhere else, which is worse than a new entity failing to
            // register — and the counter makes the loss visible.
            self.overflow_count += 1;
            return false;
        }
        let index = self.count;
        self.events[index] = GpuWorldEvent {
            position_meters: spec.position_meters,
            radius_meters: spec.radius_meters.max(0.0),
            started_epoch: clock.epoch,
            started_remainder_seconds: clock.remainder_seconds,
            ended_epoch: 0.0,
            ended_remainder_seconds: 0.0,
            channel: spec.channel,
            strength,
            open: 1.0,
            _pad_row2: 0.0,
        };
        self.keys[index] = Some(key);
        self.count += 1;
        true
    }

    /// Close an event. It STAYS in the field so sensors can render their
    /// release tails; [`Self::retire_expired`] reclaims the slot later.
    pub fn release(&mut self, key: EventKey, clock: AnimationClockSample) {
        let Some(index) = self.index_of(key) else {
            return;
        };
        let event = &mut self.events[index];
        if !event.is_open() {
            return;
        }
        event.ended_epoch = clock.epoch;
        event.ended_remainder_seconds = clock.remainder_seconds;
        event.open = 0.0;
    }

    /// Reclaim slots whose events closed longer ago than any sensor may still
    /// be fading for. Call once per frame, after raising.
    pub fn retire_expired(&mut self, clock: AnimationClockSample) {
        let mut index = 0;
        while index < self.count {
            let event = self.events[index];
            if !event.is_open() && event.elapsed_since_end(clock) > MAX_EVENT_LIFETIME_SECONDS {
                self.remove(index);
            } else {
                index += 1;
            }
        }
    }

    /// Drop everything. Deterministic/bench mode uses this so every sensor
    /// reads zero and the frame stops depending on where the camera is.
    pub fn clear(&mut self) {
        self.events = [GpuWorldEvent::INACTIVE; MAX_WORLD_EVENTS];
        self.keys = [None; MAX_WORLD_EVENTS];
        self.count = 0;
    }

    fn index_of(&self, key: EventKey) -> Option<usize> {
        self.keys[..self.count]
            .iter()
            .position(|slot| *slot == Some(key))
    }

    /// Shift rather than swap: the uploaded order stays stable frame to frame,
    /// so a retirement cannot reorder unrelated events under the GPU.
    fn remove(&mut self, index: usize) {
        for slot in index..self.count - 1 {
            self.events[slot] = self.events[slot + 1];
            self.keys[slot] = self.keys[slot + 1];
        }
        self.count -= 1;
        self.events[self.count] = GpuWorldEvent::INACTIVE;
        self.keys[self.count] = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation_clock::AnimationClock;

    fn clock_at(seconds: f32) -> AnimationClockSample {
        let mut clock = AnimationClock::new();
        clock.advance(seconds, 1.0);
        clock.sample()
    }

    fn spec_at(x: f32) -> EventSpec {
        EventSpec {
            position_meters: [x, 0.0, 0.0],
            radius_meters: 6.0,
            channel: CHANNEL_PRESENCE,
            strength: 1.0,
        }
    }

    /// The rule the whole temporal model rests on.
    #[test]
    fn raising_an_open_event_again_preserves_its_start_timestamp() {
        let mut field = WorldEventField::new();
        field.raise(EventKey::CAMERA, spec_at(0.0), clock_at(1.0));
        let started = field.active()[0].started_epoch;
        let started_remainder = field.active()[0].started_remainder_seconds;

        field.raise(EventKey::CAMERA, spec_at(3.0), clock_at(5.0));

        let event = field.active()[0];
        assert_eq!(field.len(), 1, "a re-raise must not add a second event");
        assert_eq!(event.started_epoch, started);
        assert_eq!(event.started_remainder_seconds, started_remainder);
        assert_eq!(event.position_meters[0], 3.0, "position must still update");
    }

    #[test]
    fn a_new_key_gets_its_own_timestamp_and_slot() {
        let mut field = WorldEventField::new();
        field.raise(EventKey::CAMERA, spec_at(0.0), clock_at(1.0));
        field.raise(EventKey(7), spec_at(1.0), clock_at(4.0));
        assert_eq!(field.len(), 2);
        assert!(
            field.active()[1].started_remainder_seconds
                > field.active()[0].started_remainder_seconds
        );
    }

    /// Leaving and coming back is an arrival, not a continuation — otherwise a
    /// surface you return to would already be fully lit.
    #[test]
    fn re_raising_a_closed_event_restarts_it() {
        let mut field = WorldEventField::new();
        field.raise(EventKey::CAMERA, spec_at(0.0), clock_at(1.0));
        field.release(EventKey::CAMERA, clock_at(2.0));
        assert!(!field.active()[0].is_open());

        field.raise(EventKey::CAMERA, spec_at(0.0), clock_at(6.0));

        let event = field.active()[0];
        assert!(event.is_open());
        assert!(
            (event.started_remainder_seconds - 6.0).abs() < 1e-3,
            "a return must be stamped now, got {}",
            event.started_remainder_seconds
        );
    }

    #[test]
    fn a_released_event_survives_a_full_length_envelope_then_is_reclaimed() {
        let mut field = WorldEventField::new();
        field.raise(EventKey::CAMERA, spec_at(0.0), clock_at(1.0));
        field.release(EventKey::CAMERA, clock_at(2.0));

        // Still present right up to the budget, so a max-length
        // hold + release can finish rendering.
        field.retire_expired(clock_at(2.0 + MAX_EVENT_LIFETIME_SECONDS - 0.1));
        assert_eq!(field.len(), 1);

        field.retire_expired(clock_at(2.0 + MAX_EVENT_LIFETIME_SECONDS + 0.1));
        assert_eq!(field.len(), 0);
    }

    #[test]
    fn an_open_event_is_never_reclaimed_however_long_it_runs() {
        let mut field = WorldEventField::new();
        field.raise(EventKey::CAMERA, spec_at(0.0), clock_at(1.0));
        field.retire_expired(clock_at(1000.0));
        assert_eq!(field.len(), 1);
    }

    #[test]
    fn the_seventeenth_raise_is_rejected_and_counted_without_disturbing_the_rest() {
        let mut field = WorldEventField::new();
        for index in 0..MAX_WORLD_EVENTS {
            assert!(field.raise(EventKey(index as u64), spec_at(index as f32), clock_at(1.0)));
        }
        let before = field.upload_array();

        let accepted = field.raise(EventKey(999), spec_at(99.0), clock_at(2.0));

        assert!(!accepted);
        assert_eq!(field.len(), MAX_WORLD_EVENTS);
        assert_eq!(field.overflow_count(), 1);
        assert_eq!(
            field.upload_array(),
            before,
            "a rejected raise must not disturb an existing event"
        );
    }

    /// Retirement must not reorder unrelated events, or the GPU sees an
    /// already-lit surface swap to a different event's timestamp.
    #[test]
    fn retiring_one_event_keeps_the_others_in_order() {
        let mut field = WorldEventField::new();
        field.raise(EventKey(1), spec_at(1.0), clock_at(1.0));
        field.raise(EventKey(2), spec_at(2.0), clock_at(1.0));
        field.raise(EventKey(3), spec_at(3.0), clock_at(1.0));
        field.release(EventKey(2), clock_at(1.0));

        field.retire_expired(clock_at(1.0 + MAX_EVENT_LIFETIME_SECONDS + 1.0));

        assert_eq!(field.len(), 2);
        assert_eq!(field.active()[0].position_meters[0], 1.0);
        assert_eq!(field.active()[1].position_meters[0], 3.0);
    }

    #[test]
    fn strength_is_clamped_on_ingest_so_signal_stays_normalised() {
        let mut field = WorldEventField::new();
        let mut spec = spec_at(0.0);
        spec.strength = 4.0;
        field.raise(EventKey::CAMERA, spec, clock_at(1.0));
        assert_eq!(field.active()[0].strength, 1.0);
    }

    /// The Rust upload must match the WGSL uniform-array stride of 48. The
    /// natural `#[repr(C)]` field set is 44 at align 4, so without `_pad_row2`
    /// every element from 1 onward would be read shifted. Spec rule and its
    /// conditions are on the type's doc comment.
    #[test]
    fn gpu_event_matches_the_wgsl_uniform_array_stride() {
        assert_eq!(std::mem::size_of::<GpuWorldEvent>(), 48);
        assert_eq!(std::mem::size_of::<GpuWorldEvent>() % 16, 0);
        assert_eq!(std::mem::align_of::<GpuWorldEvent>(), 4);
    }

    #[test]
    fn clearing_the_field_leaves_nothing_for_a_sensor_to_find() {
        let mut field = WorldEventField::new();
        field.raise(EventKey::CAMERA, spec_at(0.0), clock_at(1.0));
        field.clear();
        assert!(field.is_empty());
        assert_eq!(field.upload_array()[0], GpuWorldEvent::INACTIVE);
    }
}
