//! Event log + snapshots: "give me the runtime state at execution step N".
//!
//! Strategy: keep the full event log, plus full [`RuntimeState`] copies as
//! checkpoints. A checkpoint is taken once at least
//! `max(checkpoint_interval, number of objects)` events have passed since the last
//! one, so a copy (cost ~ state size) is paid for by at least that many events:
//! amortized O(1) copy cost per event however large the state grows. (A fixed
//! interval made total cost O(events x state); a 300k-event run with 20k live
//! objects took two minutes that way.)
//!
//! `snapshot_after(n)` clones the nearest earlier checkpoint and replays the
//! events after it: O(state) per query, independent of run length. The latest
//! state is maintained incrementally, so live viewing never replays. Memory is
//! O(events + state) per checkpoint, with checkpoints spaced >= state size apart.
//! Structural sharing (persistent maps) is the obvious next step if needed; the
//! API does not expose the representation, so it can change.

use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

use super::event::RuntimeEvent;
use super::ids::EventSeq;
use super::state::{ApplyError, RuntimeState};

/// An immutable view of the runtime at one point in execution. Cheap to clone
/// (shared), safe to hand to another thread, never changes after creation.
#[derive(Clone, Debug)]
pub struct RuntimeSnapshot {
    state: Arc<RuntimeState>,
}

impl RuntimeSnapshot {
    pub fn new(state: RuntimeState) -> Self {
        RuntimeSnapshot { state: Arc::new(state) }
    }

    /// Sequence number of the last event applied; `None` for the initial state.
    pub fn sequence(&self) -> Option<EventSeq> {
        self.state.last_seq()
    }
}

impl Deref for RuntimeSnapshot {
    type Target = RuntimeState;
    fn deref(&self) -> &RuntimeState {
        &self.state
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimelineError {
    /// `push` needs `event.seq` to be exactly the next sequence number.
    NonContiguous { expected: EventSeq, got: EventSeq },
    /// The event was rejected by the state; the timeline is unchanged.
    Rejected(ApplyError),
    /// No such event yet.
    OutOfRange { requested: EventSeq, len: u64 },
}

impl fmt::Display for TimelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for TimelineError {}

pub struct Timeline {
    events: Vec<RuntimeEvent>,
    /// `(events applied, state after them)`, ascending.
    checkpoints: Vec<(usize, Arc<RuntimeState>)>,
    /// Minimum number of events between checkpoints.
    interval: usize,
    live: RuntimeState,
}

impl Timeline {
    /// Minimum events between checkpoints unless overridden.
    pub const DEFAULT_CHECKPOINT_INTERVAL: usize = 256;

    pub fn new() -> Self {
        Self::with_checkpoint_interval(Self::DEFAULT_CHECKPOINT_INTERVAL)
    }

    pub fn with_checkpoint_interval(interval: usize) -> Self {
        Timeline {
            events: Vec::new(),
            checkpoints: Vec::new(),
            interval: interval.max(1),
            live: RuntimeState::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn events(&self) -> &[RuntimeEvent] {
        &self.events
    }

    /// Number of stored checkpoints (diagnostics and tests).
    pub fn checkpoint_count(&self) -> usize {
        self.checkpoints.len()
    }

    /// Append the next event. Sequence numbers must be `0, 1, 2, ...`. A rejected
    /// event is not recorded.
    pub fn push(&mut self, event: RuntimeEvent) -> Result<(), TimelineError> {
        let expected = EventSeq(self.events.len() as u64);
        if event.seq != expected {
            return Err(TimelineError::NonContiguous { expected, got: event.seq });
        }
        self.live.apply(&event).map_err(TimelineError::Rejected)?;
        self.events.push(event);
        let applied = self.events.len();
        let since = applied - self.checkpoints.last().map_or(0, |c| c.0);
        if since >= self.interval.max(self.live.object_count()) {
            self.checkpoints.push((applied, Arc::new(self.live.clone())));
        }
        Ok(())
    }

    /// State before any event.
    pub fn initial(&self) -> RuntimeSnapshot {
        RuntimeSnapshot::new(RuntimeState::new())
    }

    /// State after the most recent event.
    pub fn latest(&self) -> RuntimeSnapshot {
        RuntimeSnapshot::new(self.live.clone())
    }

    /// State after event `seq` has been applied.
    pub fn snapshot_after(&self, seq: EventSeq) -> Result<RuntimeSnapshot, TimelineError> {
        let len = self.events.len() as u64;
        if seq.0 >= len {
            return Err(TimelineError::OutOfRange { requested: seq, len });
        }
        let applied = seq.0 as usize + 1; // events 0..applied
        let usable = self.checkpoints.partition_point(|c| c.0 <= applied);
        let (mut state, from) = match usable.checked_sub(1) {
            Some(k) => ((*self.checkpoints[k].1).clone(), self.checkpoints[k].0),
            None => (RuntimeState::new(), 0),
        };
        for e in &self.events[from..applied] {
            state.apply(e).map_err(TimelineError::Rejected)?;
        }
        Ok(RuntimeSnapshot::new(state))
    }
}

impl Default for Timeline {
    fn default() -> Self {
        Self::new()
    }
}
