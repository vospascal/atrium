//! Audio-thread allocation detection and per-stage memory profiling.
//!
//! # Feature gate
//!
//! Behind the **`memprof`** Cargo feature. When disabled, no global allocator
//! is replaced and all tracking code is `#[cfg]`-gated out of existence.
//!
//! ```toml
//! cargo run --features memprof -- scenes/default.yaml
//! ```
//!
//! # How it works
//!
//! [`TrackingAllocator`] wraps the system allocator and counts bytes/allocations
//! while a thread-local tracking flag is active. Only the audio thread enables
//! this flag (inside `render()`), so main-thread allocations are ignored.
//!
//! ## Registration (in `main.rs`):
//!
//! ```rust,ignore
//! #[cfg(feature = "memprof")]
//! #[global_allocator]
//! static ALLOC: atrium::engine::memprof::TrackingAllocator =
//!     atrium::engine::memprof::TrackingAllocator;
//! ```
//!
//! ## Per-stage tracking (in `render()`):
//!
//! ```rust,ignore
//! self.memprof.begin_callback();     // reset counters, enable tracking
//! // ... source tick ...
//! self.memprof.record_stage(MemStage::SourceTick);  // snapshot delta
//! // ... mix ...
//! self.memprof.record_stage(MemStage::Mix);
//! // ... telemetry ...
//! self.memprof.record_stage(MemStage::Telemetry);
//! self.memprof.finish_callback();    // disable tracking, record total
//! ```
//!
//! After `finish_callback()`, inspect `memprof.total`, `memprof.source_tick`,
//! `memprof.mix`, etc. for allocation counts per stage. Any non-zero value
//! in the audio callback is a real-time safety violation worth investigating.
//!
//! # Design notes
//!
//! - Uses `thread_local!` `Cell<bool>` for exact audio-thread scoping
//! - Atomics use `Relaxed` ordering — sufficient for single-writer counters
//! - Zero external dependencies (pure `std::alloc`)

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

thread_local! {
    static TRACKING: Cell<bool> = Cell::new(false);
}

static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
static ALLOC_COUNT: AtomicU64 = AtomicU64::new(0);

pub struct TrackingAllocator;

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if TRACKING.with(|flag| flag.get()) {
            ALLOC_BYTES.fetch_add(layout.size() as u64, Relaxed);
            ALLOC_COUNT.fetch_add(1, Relaxed);
        }
        System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MemStats {
    pub bytes: u64,
    pub allocs: u64,
}

#[derive(Clone, Copy, Debug)]
pub enum MemStage {
    SourceTick,
    Mix,
    Telemetry,
}

pub struct MemProfiler {
    pub total: MemStats,
    pub source_tick: MemStats,
    pub mix: MemStats,
    pub telemetry: MemStats,
    last_bytes: u64,
    last_allocs: u64,
}

impl MemProfiler {
    pub fn new() -> Self {
        Self {
            total: MemStats::default(),
            source_tick: MemStats::default(),
            mix: MemStats::default(),
            telemetry: MemStats::default(),
            last_bytes: 0,
            last_allocs: 0,
        }
    }

    pub fn begin_callback(&mut self) {
        start_tracking();
        self.total = MemStats::default();
        self.source_tick = MemStats::default();
        self.mix = MemStats::default();
        self.telemetry = MemStats::default();
        self.last_bytes = 0;
        self.last_allocs = 0;
    }

    pub fn record_stage(&mut self, stage: MemStage) {
        let (bytes, allocs) = snapshot();
        let delta = MemStats {
            bytes: bytes.saturating_sub(self.last_bytes),
            allocs: allocs.saturating_sub(self.last_allocs),
        };
        self.last_bytes = bytes;
        self.last_allocs = allocs;
        match stage {
            MemStage::SourceTick => self.source_tick = delta,
            MemStage::Mix => self.mix = delta,
            MemStage::Telemetry => self.telemetry = delta,
        }
    }

    pub fn finish_callback(&mut self) {
        let (bytes, allocs) = stop_tracking();
        self.total = MemStats { bytes, allocs };
    }
}

pub fn start_tracking() {
    ALLOC_BYTES.store(0, Relaxed);
    ALLOC_COUNT.store(0, Relaxed);
    TRACKING.with(|flag| flag.set(true));
}

pub fn stop_tracking() -> (u64, u64) {
    TRACKING.with(|flag| flag.set(false));
    (ALLOC_BYTES.load(Relaxed), ALLOC_COUNT.load(Relaxed))
}

pub fn snapshot() -> (u64, u64) {
    (ALLOC_BYTES.load(Relaxed), ALLOC_COUNT.load(Relaxed))
}
