//! A thread beside the audio thread that designs the filters of an arc of a
//! new width, and takes back the ones no sound uses any more.
//!
//! A width set while the scene plays (a slider dragged, an undo, a sound
//! added) needs a table of filters that takes some tenths of a second to
//! design. The audio thread may neither wait for that nor allocate, so it
//! asks this thread through a ring and keeps rendering what it has; the
//! table comes back through another ring, as a pointer. A table the audio
//! thread lets go is handed back through a third ring and freed here:
//! freeing on the audio thread can wait on the allocator's lock.
//!
//! The last few tables designed are kept, so a slider dragged back and forth
//! finds them at once. While a slider is dragged, requests come faster than
//! tables are designed; only the latest width each sound asked for is
//! designed.

use std::sync::Arc;
use std::thread::{self, Thread};
use std::time::Duration;

use super::design::ArcHead;
use super::table::ArcTable;

/// Requests and tables in flight at once, more than there are sounds.
const RING_CAPACITY: usize = 64;

/// Tables the thread keeps after designing them.
const KEPT_TABLES: usize = 4;

/// How long the thread sleeps when nothing wakes it: it is woken for every
/// request, so this only bounds how late it notices it is no longer needed.
const IDLE_SECONDS: f64 = 0.25;

/// What a sound asks for: the table of an arc this many degrees wide.
#[derive(Clone, Copy)]
struct Request {
    source_index: u16,
    width_degrees: u16,
}

/// The audio thread's side of the designer.
pub(in crate::pipeline::renderers::binaural) struct ArcDesigner {
    requests: rtrb::Producer<Request>,
    designed: rtrb::Consumer<Arc<ArcTable>>,
    retired: rtrb::Producer<Arc<ArcTable>>,
    worker: Thread,
}

impl ArcDesigner {
    /// Start the thread for `head`. Called when the HRIRs are loaded, never
    /// on the audio thread.
    pub(in crate::pipeline::renderers::binaural) fn start(head: Arc<ArcHead>) -> Self {
        let (requests, mut incoming) = rtrb::RingBuffer::<Request>::new(RING_CAPACITY);
        let (mut outgoing, designed) = rtrb::RingBuffer::<Arc<ArcTable>>::new(RING_CAPACITY);
        let (retired, mut returned) = rtrb::RingBuffer::<Arc<ArcTable>>::new(RING_CAPACITY);
        let worker = thread::Builder::new()
            .name("arc designer".into())
            .spawn(move || {
                let mut kept: Vec<Arc<ArcTable>> = Vec::new();
                let mut wanted: Vec<Request> = Vec::new();
                let mut undelivered: Vec<Arc<ArcTable>> = Vec::new();
                loop {
                    while let Ok(table) = returned.pop() {
                        drop(table);
                    }
                    while let Ok(request) = incoming.pop() {
                        wanted.retain(|earlier| earlier.source_index != request.source_index);
                        wanted.push(request);
                    }
                    while let Some(table) = undelivered.pop() {
                        if let Err(rtrb::PushError::Full(table)) = outgoing.push(table) {
                            undelivered.push(table);
                            break;
                        }
                    }
                    if let Some(request) = wanted.first().copied() {
                        wanted.retain(|other| other.width_degrees != request.width_degrees);
                        let table = match kept
                            .iter()
                            .find(|table| table.width_degrees() == request.width_degrees)
                        {
                            Some(table) => table.clone(),
                            None => {
                                let table =
                                    Arc::new(ArcTable::design(&head, request.width_degrees));
                                if kept.len() == KEPT_TABLES {
                                    kept.remove(0);
                                }
                                kept.push(table.clone());
                                table
                            }
                        };
                        if let Err(rtrb::PushError::Full(table)) = outgoing.push(table) {
                            undelivered.push(table);
                        }
                        continue;
                    }
                    if incoming.is_abandoned() {
                        return;
                    }
                    thread::park_timeout(Duration::from_secs_f64(IDLE_SECONDS));
                }
            })
            .expect("start the arc designer")
            .thread()
            .clone();
        Self {
            requests,
            designed,
            retired,
            worker,
        }
    }

    /// Ask for the table of an arc `width_degrees` wide for the sound
    /// `source_index`. False when the ring is full: ask again next callback.
    pub(in crate::pipeline::renderers::binaural) fn request(
        &mut self,
        source_index: usize,
        width_degrees: u16,
    ) -> bool {
        let pushed = self
            .requests
            .push(Request {
                source_index: source_index as u16,
                width_degrees,
            })
            .is_ok();
        self.worker.unpark();
        pushed
    }

    /// A table designed since the last call, if any.
    pub(in crate::pipeline::renderers::binaural) fn designed(&mut self) -> Option<Arc<ArcTable>> {
        self.designed.pop().ok()
    }

    /// Hand a table back to be freed on the designer's thread. Should the
    /// ring be full, the table is leaked rather than freed here.
    pub(in crate::pipeline::renderers::binaural) fn retire(&mut self, table: Arc<ArcTable>) {
        if let Err(rtrb::PushError::Full(table)) = self.retired.push(table) {
            std::mem::forget(table);
        }
    }
}

impl Drop for ArcDesigner {
    /// Wake the thread so it sees it is no longer needed and ends.
    fn drop(&mut self) {
        self.worker.unpark();
    }
}
