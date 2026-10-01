//! Runtime event receiver: turns what an instrumented process wrote into the
//! existing URR ([`Timeline`] of [`RuntimeEvent`]s). No second event model: the
//! runtime writes URR's own serde shape, one JSON object per line.
//!
//! [`Ingest`] is the transport-independent core: feed it bytes as they arrive
//! (from a pipe, live) or all at once (from a file) and it maintains the timeline.
//! Transports only move bytes: see `pipe` (the product transport) and
//! [`receive_file`].

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::model::{RuntimeEvent, Timeline};

/// Incremental decoder of the event stream into a [`Timeline`].
///
/// Stops *applying* at the first problem (a malformed line, or an event the URR
/// rejects), because later events would be interpreted against a state that no
/// longer matches the program. It keeps *consuming* afterwards, so the producer
/// never blocks on a full pipe because of a bad event.
pub struct Ingest {
    pub timeline: Timeline,
    /// Malformed lines, rejected events, truncated output. Empty for a healthy run.
    pub issues: Vec<String>,
    /// Non-empty lines seen (applied or not).
    pub lines: usize,
    partial: Vec<u8>,
    failed: bool,
    applied: Arc<AtomicUsize>,
}

impl Ingest {
    /// `applied` is updated with the number of events in the timeline, so another
    /// thread can watch progress while a run is still in flight.
    pub fn new(applied: Arc<AtomicUsize>) -> Self {
        Ingest {
            timeline: Timeline::new(),
            issues: Vec::new(),
            lines: 0,
            partial: Vec::new(),
            failed: false,
            applied,
        }
    }

    /// Consume a chunk of the stream (need not end on a line boundary).
    pub fn feed(&mut self, bytes: &[u8]) {
        self.partial.extend_from_slice(bytes);
        let mut start = 0;
        while let Some(rel) = self.partial[start..].iter().position(|&b| b == b'\n') {
            let end = start + rel;
            let line = self.partial[start..end].to_vec();
            self.line(&line, true);
            start = end + 1;
        }
        self.partial.drain(..start);
    }

    /// The stream ended. A trailing fragment means the writer was cut off
    /// mid-event (killed, crashed): report it, keep everything before it.
    pub fn finish(&mut self) {
        if self.partial.iter().any(|b| !b.is_ascii_whitespace()) {
            let line = std::mem::take(&mut self.partial);
            self.line(&line, false);
        }
        self.partial.clear();
    }

    fn line(&mut self, bytes: &[u8], complete: bool) {
        if bytes.iter().all(|b| b.is_ascii_whitespace()) {
            return;
        }
        self.lines += 1;
        if self.failed {
            return;
        }
        let n = self.lines - 1;
        let event: RuntimeEvent = match serde_json::from_slice(bytes) {
            Ok(e) => e,
            Err(e) => {
                self.failed = true;
                self.issues.push(if complete {
                    format!("event #{n} is not a valid RuntimeEvent: {e}")
                } else {
                    format!("the last event was truncated ({e})")
                });
                return;
            }
        };
        match self.timeline.push(event) {
            Ok(()) => self.applied.store(self.timeline.len(), Ordering::Relaxed),
            Err(e) => {
                self.failed = true;
                self.issues.push(format!("event #{n} rejected by the runtime model: {e}"));
            }
        }
    }
}

/// Everything read from one run's event stream.
pub struct Received {
    pub timeline: Timeline,
    pub issues: Vec<String>,
    pub lines: usize,
}

impl From<Ingest> for Received {
    fn from(i: Ingest) -> Self {
        Received { timeline: i.timeline, issues: i.issues, lines: i.lines }
    }
}

/// Read an event file into a timeline. Tolerates a missing file (the program never
/// produced an event) and a truncated last line (the process was killed mid-write).
pub fn receive_file(path: &Path) -> Received {
    let mut ingest = Ingest::new(Arc::new(AtomicUsize::new(0)));
    match std::fs::read(path) {
        Ok(bytes) => ingest.feed(&bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => ingest.issues.push(format!("could not read the event stream: {e}")),
    }
    ingest.finish();
    ingest.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ingest() -> Ingest {
        Ingest::new(Arc::new(AtomicUsize::new(0)))
    }

    const E0: &str = r#"{"seq":0,"thread":0,"timestamp_ns":1,"location":null,"kind":{"event":"type_declared","def":{"id":1,"name":"int","kind":{"kind":"primitive","primitive":"int"},"size":4,"qualifiers":{"is_const":false,"is_volatile":false}}}}"#;

    #[test]
    fn lines_split_across_chunks_are_reassembled() {
        let mut i = ingest();
        let bytes = format!("{E0}\n").into_bytes();
        for chunk in bytes.chunks(7) {
            i.feed(chunk);
        }
        i.finish();
        assert_eq!(i.timeline.len(), 1);
        assert!(i.issues.is_empty(), "{:?}", i.issues);
    }

    #[test]
    fn a_truncated_tail_is_reported_and_the_rest_kept() {
        let mut i = ingest();
        i.feed(format!("{E0}\n{{\"seq\":1,\"thr").as_bytes());
        i.finish();
        assert_eq!(i.timeline.len(), 1);
        assert_eq!(i.issues.len(), 1);
        assert!(i.issues[0].contains("truncated"), "{:?}", i.issues);
    }

    #[test]
    fn after_a_bad_event_input_is_consumed_but_not_applied() {
        let mut i = ingest();
        // A valid event, garbage, then another valid-looking line.
        i.feed(format!("{E0}\nnot json\n{E0}\n").as_bytes());
        i.finish();
        assert_eq!(i.timeline.len(), 1);
        assert_eq!(i.lines, 3);
        assert_eq!(i.issues.len(), 1);
    }

    #[test]
    fn progress_is_visible_to_other_threads() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut i = Ingest::new(counter.clone());
        i.feed(format!("{E0}\n").as_bytes());
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }
}
