//! `ObservationService`: what the application talks to. It answers "can we
//! observe?", supplies an instrumenter for each observed run, keeps the results,
//! and renders them for the UI. No Tauri types here, so all of it is testable.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::Serialize;

use super::discovery::find_libclang;
use super::instrumenter::{Observation, ObservingInstrumenter, Progress, Results};
use super::libclang::Clang;
use super::report::{event_line, snapshot_text};
use crate::model::EventSeq;
use crate::runtime::instrumentation::{Instrumenter, ObserverProvider};
use crate::runtime::ObservationSummary;
use crate::toolchain::CompilerInfo;

/// Events recorded per run before the recording stops and says so. The model keeps
/// every event and every object in memory: measured at about 1.1 KB of peak memory
/// per event (1.07 GB for the 940,000-event loop in `overhead_probe`), so an
/// unbounded loop must not be allowed to grow it without limit. At this default a
/// run costs a few hundred MB at most; a tight 200,000-iteration loop would emit
/// ~900,000 events and is cut off. Teaching-sized programs emit thousands.
pub const DEFAULT_EVENT_LIMIT: u64 = 250_000;

/// Whether observation is possible on this machine, for the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservationStatus {
    pub available: bool,
    pub libclang_path: Option<String>,
    pub libclang_version: Option<String>,
    /// Why not, and what to do about it.
    pub hint: Option<String>,
    pub event_limit: u64,
}

/// The recorded run at one step, as text.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StepView {
    /// The state after this many events (0 = before the program did anything).
    pub step: u64,
    pub total: u64,
    /// The event that led to this state (none at step 0).
    pub event: Option<String>,
    /// The state: call stack with variables, and heap.
    pub text: String,
    /// This state is where the recording stopped (the program ran on unobserved).
    pub truncated_here: bool,
}

const LIBCLANG_HINT: &str = "Runtime observation needs libclang (version 17 or newer). Install LLVM \
(its Windows installer includes libclang.dll) or set LATTICE_LIBCLANG to the full path of libclang.dll, \
then check again.";

pub struct ObservationService {
    results: Results,
    progress: Progress,
    event_limit: u64,
}

impl Default for ObservationService {
    fn default() -> Self {
        Self::new()
    }
}

impl ObservationService {
    pub fn new() -> Self {
        let limit = std::env::var("LATTICE_EVENT_LIMIT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_EVENT_LIMIT);
        Self {
            results: Arc::new(Mutex::new(HashMap::new())),
            progress: Arc::new(Mutex::new(HashMap::new())),
            event_limit: limit,
        }
    }

    pub fn with_event_limit(mut self, limit: u64) -> Self {
        self.event_limit = limit;
        self
    }

    pub fn event_limit(&self) -> u64 {
        self.event_limit
    }

    /// Can runs be observed? Looks for libclang afresh each time, so installing
    /// one and asking again works without restarting.
    pub fn status(&self) -> ObservationStatus {
        self.status_with(find_libclang())
    }

    /// [`status`](Self::status) for a given search result (testable without
    /// touching the environment).
    pub fn status_with(&self, found: Option<std::path::PathBuf>) -> ObservationStatus {
        let (path, version, hint) = match found {
            None => (None, None, Some(LIBCLANG_HINT.to_string())),
            Some(p) => match Clang::load(&p) {
                Ok(c) => (Some(p), Some(c.version.clone()), None),
                Err(e) => (Some(p), None, Some(format!("{e}. {LIBCLANG_HINT}"))),
            },
        };
        ObservationStatus {
            available: hint.is_none(),
            libclang_path: path.map(|p| p.display().to_string()),
            libclang_version: version,
            hint,
            event_limit: self.event_limit,
        }
    }

    /// The result of the observed run whose workspace was `workspace_root`.
    pub fn take(&self, workspace_root: &Path) -> Option<Observation> {
        self.progress.lock().unwrap().remove(workspace_root);
        self.results.lock().unwrap().remove(workspace_root)
    }

    /// Events recorded so far in an observed run that is still going.
    pub fn progress(&self, workspace_root: &Path) -> Option<usize> {
        use std::sync::atomic::Ordering;
        self.progress.lock().unwrap().get(workspace_root).map(|c| c.load(Ordering::Relaxed))
    }

    pub fn summarize(&self, obs: &Observation) -> ObservationSummary {
        let end = obs.timeline.latest();
        let truncated = end.truncated_at().is_some();
        // Open on the last state before the program's frames are popped; a truncated
        // recording simply ends where it ends.
        let events = obs.timeline.events();
        let mut final_step = events.len();
        if !truncated {
            while final_step > 0 && matches!(events[final_step - 1].kind, crate::model::EventKind::FunctionExited { .. }) {
                final_step -= 1;
            }
        }
        ObservationSummary {
            events: events.len() as u64,
            final_step: final_step as u64,
            truncated,
            event_limit: self.event_limit,
            skipped: obs.summary.skipped.clone(),
            issues: obs.issues.clone(),
        }
    }
}

impl ObserverProvider for ObservationService {
    fn instrumenter(&self, compiler: &CompilerInfo, cxx_standard: &str) -> Result<Arc<dyn Instrumenter>, String> {
        let libclang = find_libclang().ok_or_else(|| LIBCLANG_HINT.to_string())?;
        Ok(Arc::new(
            ObservingInstrumenter::new(libclang, &compiler.path, cxx_standard)
                .with_shared(self.results.clone(), self.progress.clone())
                .with_event_limit(self.event_limit),
        ))
    }
}

/// The recorded run after `step` events as data for the visualization (see
/// [`crate::viz`]). `step` is clamped to the length of the run. `focus` lists object ids
/// the caller is inspecting; they come back in `GraphView::focus` whether or not they
/// are drawn.
pub fn graph_at(obs: &Observation, step: u64, focus: &[u64]) -> crate::viz::GraphView {
    let total = obs.timeline.len() as u64;
    let step = step.min(total);
    if step == 0 {
        let initial = obs.timeline.initial();
        let mut g = crate::viz::build(&initial, None, None, 0, total);
        g.focus = crate::viz::object_views(&initial, focus);
        return g;
    }
    let snap = obs.timeline.snapshot_after(EventSeq(step - 1)).expect("step is within the timeline");
    let event = obs.timeline.events().get((step - 1) as usize);
    let text = event.map(|e| event_line(e, &snap));
    let mut g = crate::viz::build(&snap, event, text, step, total);
    g.focus = crate::viz::object_views(&snap, focus);
    g
}

/// The recorded run after `step` events, rendered as text. `step` is clamped to
/// the length of the run.
pub fn view_at(obs: &Observation, step: u64) -> StepView {
    let total = obs.timeline.len() as u64;
    let step = step.min(total);
    if step == 0 {
        return StepView {
            step,
            total,
            event: None,
            text: snapshot_text(&obs.timeline.initial()),
            truncated_here: false,
        };
    }
    let seq = EventSeq(step - 1);
    let snap = obs.timeline.snapshot_after(seq).expect("step is within the timeline");
    StepView {
        step,
        total,
        event: obs.timeline.events().get((step - 1) as usize).map(|e| event_line(e, &snap)),
        text: snapshot_text(&snap),
        truncated_here: snap.truncated_at() == Some(seq),
    }
}
