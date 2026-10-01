//! End-to-end tests of runtime observation: ordinary C++ in, URR state out.
//!
//! Each test runs the *real* pipeline: libclang analysis -> source instrumentation
//! -> the existing compiler -> the existing execution manager -> events from the
//! running program -> the existing URR. Nothing is faked.
//!
//! Needs a libclang (analysis) and a C++ compiler (build). Without them the tests
//! skip loudly; set LATTICE_REQUIRE_LIBCLANG=1 to fail instead, and
//! LATTICE_LIBCLANG=<path to libclang.dll> to point at one.

mod common;

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use common::*;
use lattice_lib::model::*;
use lattice_lib::observe::discovery::find_libclang;
use lattice_lib::observe::report::{event_line, snapshot_text, timeline_text};
use lattice_lib::observe::Transport;
use lattice_lib::runtime::{ExecutionManager, SessionState};

const NODE_CYCLE: &str = include_str!("programs/node_cycle.cpp");
const NODE_WITH_OUTPUT: &str = include_str!("programs/node_with_output.cpp");
const NODE_REUSE: &str = include_str!("programs/node_reuse.cpp");
const NODE_CRASH: &str = include_str!("programs/node_crash.cpp");

// ---------------------------------------------------------------------------

#[test]
fn node_sample_reaches_the_urr() {
    let Some((session, obs)) = observe(NODE_CYCLE) else { return };

    // The program itself ran normally.
    assert_eq!(session.state, SessionState::Completed, "{session:#?}");
    assert_eq!(session.exit_code, Some(0));
    assert_eq!(session.stdout, "");
    assert!(obs.issues.is_empty(), "{:?}", obs.issues);
    assert!(obs.summary.skipped.is_none());
    assert_eq!(
        (obs.summary.records, obs.summary.news, obs.summary.deletes, obs.summary.assigns),
        (1, 2, 2, 2)
    );

    let t = &obs.timeline;
    println!("Runtime Events:\n{}\n", timeline_text(t));

    // Event order is the real order in which things happened.
    assert_eq!(
        outline(t),
        [
            "allocated 1",
            "constructed 1",
            "changed 1.0",
            "changed 1.1",
            "allocated 2",
            "constructed 2",
            "changed 2.0",
            "changed 2.1",
            "changed 1.1", // a->next = b
            "changed 2.1", // b->next = a
            "destroyed 2", // delete b
            "destroyed 1", // delete a
        ]
    );

    // Sequence numbers are contiguous and locations point at the user's lines.
    let loc = |i: usize| t.events()[i].location.as_ref().map(|l| (l.line, l.column.unwrap_or(0)));
    let find = |name: &str, nth: usize| {
        t.events().iter().enumerate().filter(|(_, e)| format!("{:?}", e.kind).starts_with(name)).nth(nth).unwrap().0
    };
    let heap_alloc = |n: usize| {
        t.events()
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(&e.kind, EventKind::ObjectAllocated { object } if object.storage == StorageClass::Heap))
            .nth(n)
            .map(|(i, _)| i)
            .unwrap()
    };
    assert_eq!(loc(heap_alloc(0)), Some((7, 15)));
    assert_eq!(loc(heap_alloc(1)), Some((8, 15)));
    assert_eq!(loc(find("ObjectDestroyed", 0)), Some((13, 5)));
    assert_eq!(loc(find("ObjectDestroyed", 1)), Some((14, 5)));
    let pointer_writes: Vec<_> = t
        .events()
        .iter()
        .filter(|e| e.location.as_ref().is_some_and(|l| l.line == 10 || l.line == 11))
        .collect();
    assert_eq!(pointer_writes.len(), 2);
    assert_eq!(pointer_writes[0].location.as_ref().unwrap().function.as_deref(), Some("main"));

    // State right after `b->next = a`: the cycle.
    let last_write = t
        .events()
        .iter()
        .rposition(|e| matches!(e.kind, EventKind::ValueChanged { .. }))
        .unwrap() as u64;
    let snap = t.snapshot_after(EventSeq(last_write)).unwrap();
    println!("{}\n", snapshot_text(&snap));

    let heap = heap_ids(&snap);
    let node = snap.object(heap[0]).unwrap().ty;
    assert_eq!(snap.types().name(node), Some("Node"));
    let (value, next) = (field_named(&snap, node, "value"), field_named(&snap, node, "next"));
    let (a, b) = (heap[0], heap[1]);
    assert_eq!(snap.type_of(b).unwrap().name, "Node");
    assert_eq!(snap.value_at(&Place::root(a).field(value)), Some(&Value::int(10)));
    assert_eq!(snap.value_at(&Place::root(b).field(value)), Some(&Value::int(20)));

    let edge = |from: ObjectId, to: ObjectId| Edge {
        source: Place::root(from).field(next),
        kind: EdgeKind::Pointer,
        target: Place::root(to),
    };
    assert_eq!(snap.outgoing(a), [edge(a, b)], "A.next -> B");
    assert_eq!(snap.outgoing(b), [edge(b, a)], "B.next -> A");
    // (The local pointer variable `a` also points at object A: a stack object, not a heap link.)
    let heap_links = |id: ObjectId| -> Vec<Edge> {
        snap.incoming(id).into_iter().filter(|e| heap.contains(&e.source.object)).collect()
    };
    assert_eq!(heap_links(a), [edge(b, a)]);
    assert_eq!(snap.live_objects().filter(|o| o.storage == StorageClass::Heap).count(), 2);
    for id in [a, b] {
        assert_eq!(snap.object(id).unwrap().lifetime.state, LifeState::Alive);
        assert_eq!(snap.object(id).unwrap().storage, StorageClass::Heap);
    }

    // The state before the pointers were written: no links yet.
    let before = t.snapshot_after(EventSeq(last_write - 2)).unwrap();
    assert!(before.outgoing(a).is_empty() && before.outgoing(b).is_empty());
    match before.value_at(&Place::root(a).field(next)) {
        Some(Value::Pointer { pointer }) => assert_eq!(pointer.target, Target::Null),
        other => panic!("expected nullptr, got {other:?}"),
    }

    // End of run: both freed, in the order of the deletes; the cycle's links now dangle.
    let end = t.latest();
    println!("{}", snapshot_text(&end));
    assert_eq!(end.live_objects().count(), 0);
    let (da, db) = (end.object(a).unwrap().lifetime, end.object(b).unwrap().lifetime);
    assert_eq!((da.state, db.state), (LifeState::Destroyed, LifeState::Destroyed));
    assert_eq!((da.end_reason, db.end_reason), (Some(EndReason::Freed), Some(EndReason::Freed)));
    assert!(db.ended_at.unwrap() < da.ended_at.unwrap(), "b was deleted first");
    assert!(da.allocated_at < db.allocated_at);
    assert_eq!(end.target_status(&Target::object(a)), TargetStatus::Dangling);
    assert_eq!(end.outgoing(a), [edge(a, b)], "the last known links are still visible");
    assert_eq!(end.verify_invariants(), Ok(()));
}

#[test]
fn instrumented_program_behaves_exactly_like_the_original() {
    // Same program, same inputs, with and without instrumentation.
    let Some((m, inst)) = observed() else { return };
    let session = m.run(request(NODE_WITH_OUTPUT), &|_| {});
    let obs = inst.take_observation(&session.workspace_path.clone().unwrap()).unwrap();

    let plain = ExecutionManager::new(config());
    let reference = plain.run(request(NODE_WITH_OUTPUT), &|_| {});

    assert_eq!(reference.stdout.trim_end(), "1 -> 2"); // (Windows text mode adds \r)
    assert_eq!(session.stdout, reference.stdout);
    assert_eq!(session.stderr, reference.stderr);
    assert_eq!(session.exit_code, Some(3));
    assert_eq!(session.exit_code, reference.exit_code);
    assert_eq!(session.state, reference.state);
    // Headers were parsed (`#include <cstdio>`) and the events are still right.
    assert!(obs.issues.is_empty(), "{:?}", obs.issues);
    assert_eq!(obs.summary.news, 2);
    assert!(outline(&obs.timeline).ends_with(&["destroyed 2".to_string(), "destroyed 1".to_string()]));
}

#[test]
fn every_allocation_gets_a_fresh_logical_id() {
    // 300 allocate/free rounds. A malloc may hand the same address back more than
    // once; whenever it does, the model must still see two different objects.
    let Some((session, obs)) = observe(NODE_REUSE) else { return };
    assert_eq!(session.state, SessionState::Completed, "{session:#?}");
    assert!(obs.issues.is_empty(), "{:?}", obs.issues);

    let end = obs.timeline.latest();
    let objects: Vec<_> = end.objects().filter(|o| o.storage == StorageClass::Heap).collect();
    assert_eq!(objects.len(), 300, "one object per allocation");
    let ids: Vec<u64> = objects.iter().map(|o| o.id.0).collect();
    assert!(ids.windows(2).all(|w| w[0] < w[1]), "ids are never reused: {ids:?}");
    assert!(objects.iter().all(|o| o.is_destroyed()));
    // Each is destroyed before the next is allocated (lifetimes never overlap).
    for w in objects.windows(2) {
        assert!(w[0].lifetime.ended_at.unwrap() < w[1].lifetime.allocated_at);
    }
    // Values belong to the object, not the address.
    for (i, o) in objects.iter().enumerate() {
        match &o.value {
            Value::Aggregate { fields } => assert_eq!(fields[0], Value::int(i as i64)),
            other => panic!("{other:?}"),
        }
    }
    // Did the allocator actually reuse addresses in this run?
    let mut by_addr: std::collections::BTreeMap<u64, Vec<u64>> = Default::default();
    for o in &objects {
        by_addr.entry(o.address.unwrap()).or_default().push(o.id.0);
    }
    let reused: Vec<_> = by_addr.values().filter(|v| v.len() > 1).collect();
    println!("{} objects, {} distinct addresses, {} addresses reused", objects.len(), by_addr.len(), reused.len());
    // (Whether reuse happens is up to the allocator; when it does, ids still differ.)
    for ids in reused {
        let mut u = ids.clone();
        u.dedup();
        assert_eq!(u.len(), ids.len());
    }
}

#[test]
fn a_crash_does_not_lose_the_events_before_it() {
    let Some((session, obs)) = observe(NODE_CRASH) else { return };
    assert_eq!(session.state, SessionState::Failed, "abort() is a failed run");
    assert!(obs.issues.is_empty(), "{:?}", obs.issues);
    assert_eq!(
        outline(&obs.timeline),
        ["allocated 1", "constructed 1", "changed 1.0", "changed 1.1", "changed 1.0"]
    );
    let end = obs.timeline.latest();
    assert_eq!(end.value_at(&Place::root(ObjectId(1)).field(0)), Some(&Value::int(5)));
    assert_eq!(end.object(ObjectId(1)).unwrap().lifetime.state, LifeState::Alive, "leaked, not destroyed");
    assert_eq!(obs.timeline.events().last().unwrap().location.as_ref().unwrap().line, 10);
}

#[test]
fn programs_that_do_not_compile_are_left_to_the_compiler() {
    let Some((m, inst)) = observed() else { return };
    let src = "int main() {\n    int x = ;\n}\n";
    let session = m.run(request(src), &|_| {});
    assert_eq!(session.state, SessionState::CompilationFailed);
    // The diagnostics are the compiler's, about the user's own line.
    let d = session.diagnostics.iter().find(|d| d.line.is_some()).expect("a positioned diagnostic");
    assert_eq!(d.line, Some(2));
    let obs = inst.take_observation(&session.workspace_path.clone().unwrap()).unwrap();
    assert!(obs.summary.skipped.is_some());
    assert!(obs.timeline.is_empty());
}

#[test]
fn diagnostics_of_instrumented_builds_still_point_at_the_users_lines() {
    // The rewrite never adds a line, so even a compile error that only the
    // *compiler* (not libclang) reports lands on the right line.
    let Some((m, _)) = observed() else { return };
    let src = "struct Node { int value; Node* next; };\nint main() {\n    Node* a = new Node{1, nullptr};\n    a->next = a;\n    undeclared_function();\n}\n";
    let session = m.run(request(src), &|_| {});
    assert_eq!(session.state, SessionState::CompilationFailed);
    assert!(session.diagnostics.iter().any(|d| d.line == Some(5)), "{:#?}", session.diagnostics);
}

#[test]
fn runtime_events_are_plain_urr_json() {
    // The wire format is the URR's own serde form: what the runtime wrote can be
    // re-serialized unchanged by Rust.
    let Some((_, obs)) = observe(NODE_CYCLE) else { return };
    for e in obs.timeline.events() {
        let json = serde_json::to_string(e).unwrap();
        let back: RuntimeEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, e);
    }
    let types = obs.timeline.events().iter().find(|e| matches!(e.kind, EventKind::TypeDeclared { .. })).unwrap();
    let line = event_line(types, &obs.timeline.latest());
    assert!(line.contains("TypeDeclared"), "{line}");
}

// ---------------------------------------------------------------------------

/// `needle` is obtained from `hay` by deleting characters only.
fn is_subsequence(needle: &str, hay: &str) -> bool {
    let mut it = hay.chars();
    needle.chars().all(|c| it.any(|h| h == c))
}

#[test]
fn rewriting_only_inserts_text() {
    use lattice_lib::observe::analysis::analyze;
    use lattice_lib::observe::discovery::analysis_args;
    use lattice_lib::observe::libclang::Clang;
    use lattice_lib::observe::rewrite::instrument;

    let Some(libclang) = find_libclang() else { return };
    let Some(compiler) = ExecutionManager::new(config()).compiler_info() else { return };
    let clang = Clang::load(&libclang).unwrap();
    let dir = temp_root();
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("main.cpp");
    std::fs::write(&file, NODE_CYCLE).unwrap();

    let a = analyze(&clang, &file, NODE_CYCLE, &analysis_args(&compiler.path, "c++20")).unwrap();
    assert!(a.errors.is_empty(), "{:?}", a.errors);
    let out = instrument(NODE_CYCLE, "main.cpp", &a);
    println!("libclang: {}\n--- instrumented source ---\n{out}", clang.version);

    assert!(is_subsequence(NODE_CYCLE, &out), "every original character survives, in order");
    assert_eq!(out.lines().count(), NODE_CYCLE.lines().count(), "no line added or removed");
    assert_eq!((a.news.len(), a.deletes.len(), a.assigns.len(), a.records.len()), (2, 2, 2, 1));
    assert_eq!(a.records[0].name, "Node");
    assert_eq!(a.records[0].fields, ["value", "next"]);
    assert_eq!(a.news[0].ty, "Node");
    let _ = std::fs::remove_dir_all(dir);
}

const STD_PROGRAM: &str = r#"#include <iostream>
#include <string>
#include <vector>

struct Node {
    int value;
    Node* next;
};

int main() {
    std::vector<int> v{1, 2, 3};
    std::string s = "hi";
    Node* a = new Node{static_cast<int>(v.size()), nullptr};
    a->next = a;
    std::cout << s << " " << a->value << std::endl;
    delete a;
}
"#;

#[test]
fn programs_using_the_standard_library_keep_working() {
    let Some((m, inst)) = observed() else { return };
    let session = m.run(request(STD_PROGRAM), &|_| {});
    let obs = inst.take_observation(&session.workspace_path.clone().unwrap()).unwrap();
    assert_eq!(session.state, SessionState::Completed, "{session:#?}");
    assert_eq!(session.stdout.trim_end(), "hi 3");
    assert!(obs.summary.skipped.is_none(), "{:?}", obs.summary.skipped);
    assert!(obs.issues.is_empty(), "{:?}", obs.issues);
    // Only user code is instrumented; std::vector / std::string internals are not.
    assert_eq!(outline(&obs.timeline), [
        "allocated 1", "constructed 1", "changed 1.0", "changed 1.1", "changed 1.1", "destroyed 1"
    ]);
}

/// `cargo test --test observation -- --ignored --nocapture overhead`
#[test]
#[ignore]
fn overhead_probe() {
    const PROGRAM: &str = r#"struct Node { int value; Node* next; };
int main() {
    Node* head = nullptr;
    for (int i = 0; i < 20000; i++) { Node* n = new Node{i, head}; head = n; }
    for (int r = 0; r < 200000; r++) { head->value = r; }
    while (head) { Node* n = head->next; delete head; head = n; }
}
"#;
    let Some((m, inst)) = observed() else { return };
    let plain = ExecutionManager::new(config()).run(request(PROGRAM), &|_| {});
    let session = m.run(request(PROGRAM), &|_| {});
    let obs = inst.take_observation(&session.workspace_path.clone().unwrap()).unwrap();
    println!(
        "plain run: {:?} ms | observed run: {:?} ms (compile {:?} ms) | events: {} | issues: {:?}",
        plain.run_duration_ms, session.run_duration_ms, session.compile_duration_ms, obs.timeline.len(), obs.issues
    );
    println!("summary: {:?}", obs.summary);
}

const NODE_NESTED: &str = include_str!("programs/node_nested.cpp");

#[test]
fn nested_objects_arrays_and_interior_pointers() {
    // Outer contains an Inner and an int[3]; a pointer to the *inside* of the
    // same object. No new object ids for the parts: they are paths into object 1.
    let Some((session, obs)) = observe(NODE_NESTED) else { return };
    assert_eq!(session.state, SessionState::Completed, "{session:#?}");
    assert!(obs.issues.is_empty(), "{:?}", obs.issues);
    println!("{}", timeline_text(&obs.timeline));

    assert_eq!(
        outline(&obs.timeline),
        [
            "allocated 1", "constructed 1",
            "changed 1.0", "changed 1.1", "changed 1.2", // the constructor's result, per top-level field
            "changed 1.0.1",  // o->in.b = 20
            "changed 1.1[1]", // o->arr[1] = 40
            "changed 1.2",    // o->p = &o->in
            "destroyed 1",
        ]
    );

    // The state after the last write, just before the delete.
    let last = obs.timeline.events().iter().rposition(|e| matches!(e.kind, EventKind::ValueChanged { .. })).unwrap() as u64;
    let snap = obs.timeline.snapshot_after(EventSeq(last)).unwrap();
    println!("{}", snapshot_text(&snap));
    let o = heap_ids(&snap)[0];
    assert_eq!(heap_ids(&snap).len(), 1, "parts are not separate objects");
    assert_eq!(snap.value_at(&Place::root(o).field(0).field(0)), Some(&Value::int(1)));
    assert_eq!(snap.value_at(&Place::root(o).field(0).field(1)), Some(&Value::int(20)));
    assert_eq!(snap.value_at(&Place::root(o).field(1).index(0)), Some(&Value::int(3)));
    assert_eq!(snap.value_at(&Place::root(o).field(1).index(1)), Some(&Value::int(40)));
    assert_eq!(snap.value_at(&Place::root(o).field(1).index(2)), Some(&Value::int(5)));

    // `&o->in` has the same address as `*o`, but it designates the *member*:
    // the pointer's static type (Inner*) picks the right place.
    let edge = Edge {
        source: Place::root(o).field(2),
        kind: EdgeKind::Pointer,
        target: Place::root(o).field(0),
    };
    assert_eq!(snap.outgoing(o), [edge.clone()]);
    let from_heap: Vec<Edge> = snap.incoming(o).into_iter().filter(|e| e.source.object == o).collect();
    assert_eq!(from_heap, [edge]);
    assert_eq!(snap.target_status(&Target::to(Place::root(o).field(0))), TargetStatus::Live);
}

// ---- transport ----------------------------------------------------------------

#[test]
fn the_file_transport_gives_the_same_result_as_the_pipe() {
    let Some((m, inst)) = observed_with(|_| {}, Some(Transport::File)) else { return };
    let session = m.run(request(NODE_CYCLE), &|_| {});
    let obs = inst.take_observation(&session.workspace_path.clone().unwrap()).unwrap();
    assert!(obs.issues.is_empty(), "{:?}", obs.issues);
    let piped = observe(NODE_CYCLE).unwrap().1;
    assert_eq!(obs.timeline.len(), piped.timeline.len());
    assert_eq!(outline(&obs.timeline), outline(&piped.timeline));
}

#[cfg(windows)]
mod live {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::Mutex;
    use std::time::Instant;

    const SLEEPER: &str = r#"#include <windows.h>
struct Node { int value; Node* next; };
int main() {
    Node* a = new Node{1, nullptr};
    a->value = 2;
    Sleep(2500);
    delete a;
}
"#;

    #[test]
    fn events_arrive_while_the_program_is_still_running() {
        let Some((m, inst)) = observed() else { return };
        let root: Mutex<Option<PathBuf>> = Mutex::new(None);
        let finished = AtomicBool::new(false);
        let mut seen_while_running = 0usize;

        let session = std::thread::scope(|scope| {
            let poller = scope.spawn(|| {
                let deadline = Instant::now() + Duration::from_secs(60);
                let mut best = 0;
                while Instant::now() < deadline && !finished.load(Ordering::SeqCst) {
                    if let Some(r) = root.lock().unwrap().clone() {
                        if let Some(n) = inst.progress(&r) {
                            if n > 0 && !finished.load(Ordering::SeqCst) {
                                best = n;
                                break;
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                best
            });
            let session = m.run(request(SLEEPER), &|s| {
                if s.state == SessionState::Running {
                    *root.lock().unwrap() = s.workspace_path.clone();
                }
            });
            finished.store(true, Ordering::SeqCst);
            seen_while_running = poller.join().unwrap();
            session
        });

        assert_eq!(session.state, SessionState::Completed, "{session:#?}");
        // The program sleeps for 2.5 s after its last event before exiting: these
        // events were observed by Lattice *during* that time.
        assert!(seen_while_running >= 6, "saw only {seen_while_running} events while it was running");
        let obs = inst.take_observation(&session.workspace_path.clone().unwrap()).unwrap();
        assert!(obs.issues.is_empty(), "{:?}", obs.issues);
        assert!(obs.timeline.len() > seen_while_running, "the delete came after the sleep");
    }

    #[test]
    fn a_hung_program_keeps_its_events_after_a_timeout() {
        let Some((m, inst)) = observed_with(|c| c.run_timeout = Duration::from_secs(3), None) else { return };
        let src = "struct Node { int value; Node* next; };
int main() {
    Node* a = new Node{1, nullptr};
    a->value = 2;
    for (;;) {}
}
";
        let session = m.run(request(src), &|_| {});
        assert_eq!(session.state, SessionState::TimedOut, "{session:#?}");
        let obs = inst.take_observation(&session.workspace_path.clone().unwrap()).unwrap();
        assert!(obs.issues.is_empty(), "{:?}", obs.issues);
        // Nothing was lost to buffering even though the process was killed.
        assert_eq!(
            outline(&obs.timeline),
            ["allocated 1", "constructed 1", "changed 1.0", "changed 1.1", "changed 1.0"]
        );
    }

    #[test]
    fn a_program_that_never_emits_does_not_hang_lattice() {
        // The runtime opens the pipe lazily, so this program never connects. The
        // receiver must still end promptly instead of waiting for a client.
        let Some((m, inst)) = observed() else { return };
        let started = Instant::now();
        // A function-try-block body is not instrumented, so this program emits nothing.
        let session = m.run(request("int main() try { return 0; } catch (...) { return 1; }
"), &|_| {});
        assert_eq!(session.state, SessionState::Completed, "{session:#?}");
        assert!(started.elapsed() < Duration::from_secs(30));
        let obs = inst.take_observation(&session.workspace_path.clone().unwrap()).unwrap();
        assert!(obs.timeline.is_empty());
        assert!(obs.issues.is_empty(), "{:?}", obs.issues);
    }

    #[test]
    fn a_failed_build_does_not_leak_the_pipe_reader() {
        // The pipe exists from `prepare`; a compile error means the program never
        // runs. Dropping the plan must release the reader thread.
        let Some((m, _inst)) = observed() else { return };
        let src = "int main() {
    undeclared();
}
";
        let started = Instant::now();
        let session = m.run(request(src), &|_| {});
        assert_eq!(session.state, SessionState::CompilationFailed);
        assert!(started.elapsed() < Duration::from_secs(30));
    }
}

#[test]
fn headers_libclang_cannot_digest_do_not_stop_instrumentation() {
    // libclang 18 cannot parse GCC's intrinsics headers (pulled in by
    // <windows.h> under MinGW). Errors inside *system* headers must not cost the
    // user their observation; errors in the user's own code still do.
    let Some((m, inst)) = observed() else { return };
    let src = "#include <windows.h>
struct Node { int value; Node* next; };
int main() { Node* a = new Node{1, nullptr}; a->value = 2; Sleep(10); delete a; }
";
    let session = m.run(request(src), &|_| {});
    let obs = inst.take_observation(&session.workspace_path.clone().unwrap()).unwrap();
    assert_eq!(session.state, SessionState::Completed, "{session:#?}");
    assert!(obs.summary.skipped.is_none(), "{:?}", obs.summary.skipped);
    assert_eq!(
        outline(&obs.timeline),
        ["allocated 1", "constructed 1", "changed 1.0", "changed 1.1", "changed 1.0", "destroyed 1"]
    );
}

/// `cargo test --test observation -- --ignored --nocapture ingest_throughput`
#[test]
#[ignore]
fn ingest_throughput_probe() {
    use lattice_lib::observe::receiver::Ingest;
    use std::sync::atomic::AtomicUsize;
    const PROGRAM: &str = r#"struct Node { int value; Node* next; };
int main() {
    Node* head = nullptr;
    for (int i = 0; i < 20000; i++) { Node* n = new Node{i, head}; head = n; }
    for (int r = 0; r < 200000; r++) { head->value = r; }
    while (head) { Node* n = head->next; delete head; head = n; }
}
"#;
    let Some((m, inst)) = observed_with(|_| {}, Some(Transport::File)) else { return };
    let session = m.run(request(PROGRAM), &|_| {});
    let obs = inst.take_observation(&session.workspace_path.clone().unwrap()).unwrap();
    let events = obs.timeline.events().to_vec();
    let mut bytes = Vec::new();
    for e in &events {
        bytes.extend_from_slice(serde_json::to_string(e).unwrap().as_bytes());
        bytes.push(b'\n');
    }
    println!("{} events, {:.1} MB of JSON", events.len(), bytes.len() as f64 / 1e6);

    let t = std::time::Instant::now();
    for line in bytes.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let _: RuntimeEvent = serde_json::from_slice(line).unwrap();
    }
    let parse = t.elapsed();

    let t = std::time::Instant::now();
    let mut ingest = Ingest::new(Arc::new(AtomicUsize::new(0)));
    ingest.feed(&bytes);
    ingest.finish();
    let all = t.elapsed();
    println!("parse only: {parse:?}  ({:.0} events/s)", events.len() as f64 / parse.as_secs_f64());
    println!("parse + apply: {all:?}  ({:.0} events/s)  issues={:?}", events.len() as f64 / all.as_secs_f64(), ingest.issues);

    let t = std::time::Instant::now();
    let mut state = RuntimeState::new();
    for e in &events {
        state.apply(e).unwrap();
    }
    println!("apply only (no timeline): {:?}", t.elapsed());
}
