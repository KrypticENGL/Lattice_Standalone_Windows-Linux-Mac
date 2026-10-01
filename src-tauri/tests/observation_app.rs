//! The application-facing side of observation: availability, per-run opt-in,
//! the step views the UI shows, and the event budget. Everything the Tauri
//! commands do is a thin call into what is tested here.

mod common;

use std::sync::Arc;

use common::*;
use lattice_lib::model::*;
use lattice_lib::observe::discovery::find_libclang;
use lattice_lib::observe::{view_at, Observation, ObservationService};
use lattice_lib::runtime::instrumentation::{Instrumenter, ObserverProvider};
use lattice_lib::runtime::{ExecutionManager, RunRequest, RuntimeSession, SessionState};
use lattice_lib::toolchain::CompilerInfo;

const FRAMES: &str = include_str!("programs/frames.cpp");

const LOOP: &str = r#"#include <cstdio>
struct Node { int value; Node* next; };
int main() {
    Node* n = new Node{0, nullptr};
    for (int i = 0; i < 1000; i++) {
        n->value = i;
    }
    delete n;
    std::printf("done\n");
    return 0;
}
"#;

/// A manager wired like the application's (service as observer), or None to skip.
fn app_like(limit: u64) -> Option<(ExecutionManager, Arc<ObservationService>)> {
    if find_libclang().is_none() {
        if std::env::var_os("LATTICE_REQUIRE_LIBCLANG").is_some() {
            panic!("no libclang found");
        }
        eprintln!("SKIPPED: no libclang found (set LATTICE_LIBCLANG)");
        return None;
    }
    let service = Arc::new(ObservationService::new().with_event_limit(limit));
    let manager = ExecutionManager::new(config()).with_observer(service.clone());
    if !manager.toolchain_status().available {
        eprintln!("SKIPPED: no C++ compiler found");
        return None;
    }
    Some((manager, service))
}

fn run(m: &ExecutionManager, src: &str, observe: bool) -> RuntimeSession {
    m.run(RunRequest { observe, ..request(src) }, &|_| {})
}

/// What the `run_program` command does after a run.
fn take(service: &ObservationService, session: &RuntimeSession) -> Option<Observation> {
    session.workspace_path.as_deref().and_then(|root| service.take(root))
}

struct Refusing;
impl ObserverProvider for Refusing {
    fn instrumenter(&self, _: &CompilerInfo, _: &str) -> Result<Arc<dyn Instrumenter>, String> {
        Err("libclang is not installed".into())
    }
}

#[test]
fn status_explains_what_is_missing_and_what_to_do() {
    let service = ObservationService::new();
    let s = service.status_with(None);
    assert!(!s.available);
    let hint = s.hint.expect("a hint");
    assert!(hint.contains("libclang") && hint.contains("LATTICE_LIBCLANG"), "{hint}");
    assert!(s.libclang_path.is_none() && s.libclang_version.is_none());

    // A path that is not a working libclang is reported, with the path.
    let bogus = std::env::temp_dir().join("definitely-not-libclang.dll");
    let s = service.status_with(Some(bogus.clone()));
    assert!(!s.available);
    assert_eq!(s.libclang_path.as_deref(), Some(bogus.display().to_string().as_str()));
    assert!(s.hint.unwrap().contains("could not load"));
}

#[test]
fn status_reports_the_libclang_that_will_be_used() {
    let Some(path) = find_libclang() else { return };
    let s = ObservationService::new().with_event_limit(123).status_with(Some(path));
    assert!(s.available, "{s:?}");
    assert!(s.libclang_version.as_deref().is_some_and(|v| v.contains("clang version")), "{s:?}");
    assert!(s.hint.is_none());
    assert_eq!(s.event_limit, 123);
    // Serialized for the UI in camelCase.
    let json = serde_json::to_value(&s).unwrap();
    assert!(json.get("libclangVersion").is_some() && json.get("eventLimit").is_some());
}

#[test]
fn asking_for_observation_that_is_not_possible_fails_clearly() {
    let plain = ExecutionManager::new(config());
    if !plain.toolchain_status().available {
        return;
    }
    // No observer at all.
    let s = run(&plain, LOOP, true);
    assert_eq!(s.state, SessionState::Failed);
    assert!(s.error.as_deref().unwrap_or("").contains("Observation is not available"), "{s:#?}");
    assert!(s.compile_duration_ms.is_none(), "nothing was compiled");
    assert!(s.stdout.is_empty(), "and nothing ran");

    // An observer that refuses (libclang missing): the reason reaches the user, and
    // the same manager still runs un-observed programs normally.
    let m = ExecutionManager::new(config()).with_observer(Arc::new(Refusing));
    let s = run(&m, LOOP, true);
    assert_eq!(s.state, SessionState::Failed);
    let msg = s.error.unwrap();
    assert!(msg.contains("Observation is not available") && msg.contains("libclang is not installed"), "{msg}");
    let ok = run(&m, LOOP, false);
    assert_eq!(ok.state, SessionState::Completed, "{ok:#?}");
    assert_eq!(ok.stdout.trim(), "done");
}

#[test]
fn observed_and_plain_runs_share_one_manager() {
    let Some((m, service)) = app_like(0) else { return };
    let plain = run(&m, LOOP, false);
    assert_eq!(plain.state, SessionState::Completed, "{plain:#?}");
    assert!(take(&service, &plain).is_none(), "an un-observed run leaves no recording");

    let observed = run(&m, LOOP, true);
    assert_eq!(observed.state, SessionState::Completed, "{observed:#?}");
    assert_eq!(observed.stdout, plain.stdout, "observation does not change the program");
    let obs = take(&service, &observed).expect("a recording");
    assert!(obs.issues.is_empty(), "{:?}", obs.issues);
    assert!(obs.timeline.len() > 1000, "{} events", obs.timeline.len());
}

#[test]
fn the_step_view_shows_the_program_at_any_point() {
    let Some((m, service)) = app_like(0) else { return };
    let session = run(&m, FRAMES, true);
    assert_eq!(session.exit_code, Some(0), "{session:#?}");
    let obs = take(&service, &session).unwrap();
    let summary = service.summarize(&obs);
    assert_eq!(summary.events, obs.timeline.len() as u64);
    assert!(!summary.truncated && summary.skipped.is_none() && summary.issues.is_empty());

    // The recording opens just before `main` returns: its frame and variables are
    // still there, rather than the empty picture after everything was popped.
    assert_eq!(summary.final_step, summary.events - 1, "only main's own exit is skipped");
    let opening = view_at(&obs, summary.final_step);
    assert!(opening.text.contains("main()") && opening.text.contains("int total = 14"), "{}", opening.text);

    // Step 0: before the program did anything.
    let first = view_at(&obs, 0);
    assert_eq!((first.step, first.total, first.event.clone()), (0, summary.events, None));
    assert!(first.text.contains("Runtime Snapshot (initial)"), "{}", first.text);

    // The last step: everything returned; the final event is shown.
    let last = view_at(&obs, summary.events);
    assert_eq!(last.step, summary.events);
    assert!(last.event.as_deref().unwrap().contains("FunctionExited"), "{:?}", last.event);
    assert!(!last.text.contains("Call stack"), "{}", last.text);
    assert!(!last.truncated_here);

    // Somewhere in the middle there is a call stack with `square` on top of `main`.
    let mid = (1..=summary.events).map(|s| view_at(&obs, s)).find(|v| v.text.contains("square()") && v.text.contains("int n = 1")).unwrap();
    assert!(mid.text.contains("main()"), "{}", mid.text);
    assert!(mid.event.is_some());

    // Steps past the end are clamped, not errors.
    assert_eq!(view_at(&obs, summary.events + 50), last);
    // The view is plain data for the UI (camelCase).
    let json = serde_json::to_value(&mid).unwrap();
    assert!(json.get("truncatedHere").is_some() && json.get("step").is_some());
}

#[test]
fn a_run_over_its_event_budget_is_truncated_and_says_so() {
    let limit = 150;
    let Some((m, service)) = app_like(limit) else { return };
    let plain = run(&m, LOOP, false);
    let session = run(&m, LOOP, true);

    // The program is unaffected: it ran to completion with the same output.
    assert_eq!(session.state, SessionState::Completed, "{session:#?}");
    assert_eq!(session.stdout, plain.stdout);
    assert_eq!(session.exit_code, Some(0));

    let obs = take(&service, &session).unwrap();
    assert!(obs.issues.is_empty(), "{:?}", obs.issues);
    // `limit` events, then the marker, then nothing, although the loop ran 1000 times.
    assert_eq!(obs.timeline.len() as u64, limit + 1);
    let last = obs.timeline.events().last().unwrap();
    assert_eq!(last.kind, EventKind::ObservationTruncated { limit });
    let writes = obs.timeline.events().iter().filter(|e| matches!(e.kind, EventKind::ValueChanged { .. })).count();
    assert!(writes < 1000, "the loop's writes were not all recorded ({writes})");

    // The summary and the views say so, instead of presenting a partial run as complete.
    let summary = service.summarize(&obs);
    assert!(summary.truncated);
    assert_eq!((summary.events, summary.event_limit), (limit + 1, limit));
    let end = view_at(&obs, summary.events);
    assert!(end.truncated_here);
    assert!(end.event.as_deref().unwrap().contains("ObservationTruncated"), "{:?}", end.event);
    assert!(!view_at(&obs, summary.events - 1).truncated_here);
    // The frozen state is still a valid, consistent state ("as far as we know").
    assert_eq!(obs.timeline.latest().verify_invariants(), Ok(()));
    assert!(obs.timeline.latest().truncated_at().is_some());
}

#[test]
fn a_limit_of_zero_means_no_limit() {
    let Some((m, service)) = app_like(0) else { return };
    let session = run(&m, LOOP, true);
    let obs = take(&service, &session).unwrap();
    assert!(!service.summarize(&obs).truncated);
    assert!(obs.timeline.latest().truncated_at().is_none());
}
