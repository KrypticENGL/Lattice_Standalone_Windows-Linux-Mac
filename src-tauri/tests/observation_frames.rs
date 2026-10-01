//! Observation of function frames, scopes and variables: the "program" half of
//! the picture (the heap half is in `observation.rs`). Every test runs the real
//! pipeline against a real compiler; see `common`.

mod common;

use common::*;
use lattice_lib::model::*;
use lattice_lib::observe::report::{snapshot_text, timeline_text};
use lattice_lib::observe::Observation;
use lattice_lib::runtime::SessionState;

const FRAMES: &str = include_str!("programs/frames.cpp");

/// Run `src` observed; skip (None) without libclang/compiler; assert it completed
/// with exit code 0 and that the URR accepted every event.
fn run_ok(src: &str) -> Option<(lattice_lib::runtime::RuntimeSession, Observation)> {
    let (session, obs) = observe(src)?;
    assert_eq!(session.state, SessionState::Completed, "{session:#?}");
    assert_eq!(session.exit_code, Some(0), "{session:#?}");
    assert!(obs.issues.is_empty(), "{:?}", obs.issues);
    assert!(obs.summary.skipped.is_none(), "{:?}", obs.summary.skipped);
    Some((session, obs))
}

fn ints(v: &[i64]) -> Vec<Value> {
    v.iter().map(|i| Value::int(*i)).collect()
}

#[test]
fn functions_locals_and_scopes_reach_the_urr() {
    let Some((_, obs)) = run_ok(FRAMES) else { return };
    let t = &obs.timeline;
    println!("{}", timeline_text(t));
    let snaps = snapshots(t);

    // The call stack: main -> square (three times), main -> link; then empty.
    let entered: Vec<String> = t
        .events()
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::FunctionEntered { function, .. } => Some(function.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(entered, ["main", "square", "square", "square", "link"]);
    let stacks: Vec<Vec<String>> = snaps.iter().map(|s| stack_names(s)).collect();
    assert_eq!(stacks.iter().map(Vec::len).max(), Some(2));
    assert!(stacks.iter().any(|s| s == &["main", "square"]));
    assert!(stacks.iter().any(|s| s == &["main", "link"]));
    assert!(stacks.last().unwrap().is_empty(), "everything returned");

    // Frames point at where they are in the source and who called them.
    let square_entry =
        t.events().iter().find(|e| matches!(&e.kind, EventKind::FunctionEntered { function, .. } if function.as_ref() == "square")).unwrap();
    assert_eq!(square_entry.location.as_ref().map(|l| l.line), Some(6));
    let inside = snaps
        .iter()
        .find(|s| stack_names(s) == ["main", "square"] && s.top_frame(ThreadId::MAIN).is_some_and(|f| f.variables.len() == 2))
        .expect("a moment with both n and result");
    let frame = *inside.stack(ThreadId::MAIN).last().unwrap();
    println!("{}
", snapshot_text(inside));
    assert_eq!(var_names(inside, frame), ["n", "result"]);
    let kinds: Vec<_> = inside.frame_variables(frame).map(|v| v.kind).collect();
    assert_eq!(kinds, [VariableKind::Parameter, VariableKind::Local]);
    assert_eq!(inside.frame(frame).unwrap().caller, inside.stack(ThreadId::MAIN).first().copied());
    assert_eq!(inside.top_frame(ThreadId::MAIN).unwrap().location.as_ref().map(|l| l.line), Some(7));

    // Each call has its own parameter and local: separate objects, separate values.
    let n = history(&snaps, "n");
    assert_eq!(n.len(), 3);
    assert_eq!(n.iter().map(|(_, v)| v.clone()).collect::<Vec<_>>(), [ints(&[1]), ints(&[2]), ints(&[3])]);
    assert_eq!(history(&snaps, "result").iter().map(|(_, v)| v.clone()).collect::<Vec<_>>(), [ints(&[1]), ints(&[4]), ints(&[9])]);
    assert_eq!(history(&snaps, "sq").len(), 3, "a fresh `sq` per iteration");

    // The loop counter is one variable counting up, and gone when the loop is.
    assert_eq!(values_of(&snaps, "i"), ints(&[1, 2, 3, 4]));
    assert_eq!(values_of(&snaps, "total"), ints(&[0, 1, 5, 14]));
    let after_loop = snaps.iter().position(|s| stack_names(s) == ["main", "link"]).unwrap();
    assert!(snaps[after_loop].variables().all(|v| v.name != "i" && v.name != "sq"));

    // A block's variable lives exactly as long as the block.
    assert_eq!(values_of(&snaps, "inner"), ints(&[14, 15]));
    let present: Vec<bool> = snaps.iter().map(|s| s.variables().any(|v| v.name == "inner")).collect();
    let (first, last) = (present.iter().position(|b| *b).unwrap(), present.iter().rposition(|b| *b).unwrap());
    assert!(present[first..=last].iter().all(|b| *b), "one contiguous lifetime");
    let inner_obj = history(&snaps, "inner")[0].0;
    let end = t.latest();
    let o = end.object(inner_obj).unwrap();
    assert_eq!((o.storage, o.lifetime.state, o.lifetime.end_reason), (StorageClass::Automatic, LifeState::Destroyed, Some(EndReason::ScopeExit)));

    // Pointers between the stack and the heap. Inside `link`, `b` points at the
    // stack variable `second`; afterwards the heap node's `next` does too.
    let in_link = snaps.iter().rev().find(|s| stack_names(s) == ["main", "link"] && var_names(s, *s.stack(ThreadId::MAIN).last().unwrap()).len() == 2).unwrap();
    let main = *in_link.stack(ThreadId::MAIN).first().unwrap();
    let main_var = |s: &RuntimeSnapshot, n: &str| s.frame_variables(main).find(|v| v.name == n).unwrap().clone();
    let second = main_var(in_link, "second");
    let link_frame = *in_link.stack(ThreadId::MAIN).last().unwrap();
    let b = in_link.frame_variables(link_frame).find(|v| v.name == "b").unwrap();
    match in_link.variable_value(b.id) {
        Some(Value::Pointer { pointer }) => assert_eq!(pointer.target, Target::object(second.object)),
        other => panic!("{other:?}"),
    }
    let heap = heap_ids(in_link)[0];
    let node_ty = in_link.object(heap).unwrap().ty;
    let next = field_named(in_link, node_ty, "next");
    let edge = Edge { source: Place::root(heap).field(next), kind: EdgeKind::Pointer, target: Place::root(second.object) };
    let after_link = snaps.iter().rev().find(|s| stack_names(s) == ["main"] && s.outgoing(heap).len() == 1).unwrap();
    println!("{}
", snapshot_text(after_link));
    assert_eq!(after_link.outgoing(heap), [edge]);
    assert_eq!(after_link.object(second.object).unwrap().storage, StorageClass::Automatic);

    // At the very end nothing is alive and the model is consistent.
    assert_eq!(end.live_objects().count(), 0);
    assert!(end.stack(ThreadId::MAIN).is_empty());
    assert_eq!(end.variables().count(), 0);
    assert_eq!(end.verify_invariants(), Ok(()));
}

#[test]
fn recursion_builds_one_frame_per_call() {
    let src = r#"int fact(int n) {
    if (n <= 1) {
        return 1;
    }
    int rest = fact(n - 1);
    return n * rest;
}

int main() {
    return fact(4) == 24 ? 0 : 1;
}
"#;
    let Some((_, obs)) = run_ok(src) else { return };
    let snaps = snapshots(&obs.timeline);
    let deepest = snaps.iter().max_by_key(|s| s.stack(ThreadId::MAIN).len()).unwrap();
    assert_eq!(stack_names(deepest), ["main", "fact", "fact", "fact", "fact"]);
    // Four live activations of the same function, each with its own `n`.
    let ns: Vec<Value> = deepest
        .stack(ThreadId::MAIN)
        .iter()
        .skip(1)
        .map(|f| {
            let v = deepest.frame_variables(*f).find(|v| v.name == "n").unwrap();
            deepest.variable_value(v.id).cloned().unwrap()
        })
        .collect();
    assert_eq!(ns, ints(&[4, 3, 2, 1]));
    let frames: std::collections::HashSet<_> = deepest.stack(ThreadId::MAIN).iter().collect();
    assert_eq!(frames.len(), 5, "distinct frame ids");
    // `rest` exists once the recursive call has returned: 1, 2, 6 in that order.
    let rests: Vec<Value> = history(&snaps, "rest").into_iter().map(|(_, mut v)| v.remove(0)).collect();
    assert_eq!(rests, ints(&[1, 2, 6]));
    assert!(snaps.last().unwrap().stack(ThreadId::MAIN).is_empty());
}

#[test]
fn references_are_bindings_to_the_thing_they_name() {
    let src = r#"void bump(int& x) {
    x++;
}

int main() {
    int a = 1;
    int& r = a;
    bump(a);
    r += 10;
    return a == 12 ? 0 : 1;
}
"#;
    let Some((_, obs)) = run_ok(src) else { return };
    let snaps = snapshots(&obs.timeline);
    // `a` is changed through both references; there is one `a`.
    assert_eq!(values_of(&snaps, "a"), ints(&[1, 2, 12]));
    let a_obj = history(&snaps, "a")[0].0;

    // The parameter is a reference variable whose value is what it is bound to.
    let in_bump = snaps.iter().find(|s| stack_names(s) == ["main", "bump"] && s.variables().any(|v| v.name == "x")).unwrap();
    let x = in_bump.variables().find(|v| v.name == "x").unwrap();
    assert_eq!(x.kind, VariableKind::Parameter);
    assert_eq!(in_bump.variable_value(x.id), Some(&Value::reference_to(Place::root(a_obj))));
    assert_eq!(in_bump.type_of(x.object).unwrap().name, "int&");

    let last_with_r = snaps.iter().rev().find(|s| s.variables().any(|v| v.name == "r")).unwrap();
    let r = last_with_r.variables().find(|v| v.name == "r").unwrap();
    assert_eq!(last_with_r.variable_value(r.id), Some(&Value::reference_to(Place::root(a_obj))));
    // A reference is a link: it shows up as an edge from its own object to `a`.
    let edges = last_with_r.outgoing(r.object);
    assert_eq!(edges.len(), 1);
    assert_eq!((edges[0].kind, &edges[0].target), (EdgeKind::Reference, &Place::root(a_obj)));
}

#[test]
fn uninitialized_variables_arrays_and_range_for() {
    let src = r#"int main() {
    int u;
    u = 5;
    int arr[3] = {1, 2, 3};
    arr[1] = 20;
    int sum = 0;
    for (int e : arr) {
        sum += e;
    }
    int big[3];
    return sum == 24 ? 0 : 1;
}
"#;
    let Some((_, obs)) = run_ok(src) else { return };
    let snaps = snapshots(&obs.timeline);

    // Declared-but-unset is not a number: it is "uninitialized", then 5.
    let uninit = Value::unavailable(Unavailable::Uninitialized);
    assert_eq!(values_of(&snaps, "u"), [uninit.clone(), Value::int(5)]);
    assert_ne!(uninit, Value::int(0));

    // Arrays are structured values; one element changes.
    let arr = |a: [i64; 3]| Value::array(ints(&a));
    assert_eq!(values_of(&snaps, "arr"), [arr([1, 2, 3]), arr([1, 20, 3])]);
    assert_eq!(values_of(&snaps, "big"), [Value::array(vec![uninit.clone(), uninit.clone(), uninit])]);
    let arr_obj = history(&snaps, "arr")[0].0;
    let change = obs
        .timeline
        .events()
        .iter()
        .find_map(|e| match &e.kind {
            EventKind::ValueChanged { place, value } if place.object == arr_obj => Some((place.path.clone(), value.clone())),
            _ => None,
        })
        .unwrap();
    assert_eq!(change, (vec![Step::Index(1)], Value::int(20)), "one element, not the whole array");

    // Every iteration of a range-for has a fresh variable; each ends with its iteration.
    let e = history(&snaps, "e");
    assert_eq!(e.iter().map(|(_, v)| v.clone()).collect::<Vec<_>>(), [ints(&[1]), ints(&[20]), ints(&[3])]);
    let end = obs.timeline.latest();
    assert!(e.iter().all(|(o, _)| end.object(*o).unwrap().lifetime.end_reason == Some(EndReason::ScopeExit)));
    assert_eq!(values_of(&snaps, "sum"), ints(&[0, 1, 21, 24]));
}

#[test]
fn early_returns_and_exceptions_unwind_frames_in_order() {
    let src = r#"int risky(int depth) {
    int local = depth;
    if (depth == 0) {
        throw 7;
    }
    return risky(depth - 1) + local;
}

int early(int x) {
    if (x > 0) {
        int positive = x;
        return positive;
    }
    return -1;
}

int main() {
    int caught = 0;
    try {
        risky(2);
    } catch (int e) {
        caught = e;
    }
    int r = early(5);
    return (caught == 7 && r == 5) ? 0 : 1;
}
"#;
    let Some((_, obs)) = run_ok(src) else { return };
    let snaps = snapshots(&obs.timeline);
    // The throw unwinds three `risky` frames, then execution continues in main.
    let deepest = snaps.iter().map(|s| stack_names(s)).max_by_key(Vec::len).unwrap();
    assert_eq!(deepest, ["main", "risky", "risky", "risky"]);
    let catch_point = snaps.iter().position(|s| s.variables().any(|v| v.name == "caught" && s.variable_value(v.id) == Some(&Value::int(7)))).unwrap();
    assert!(snaps[catch_point..].iter().all(|s| stack_names(s).len() <= 2));
    assert_eq!(values_of(&snaps, "caught"), ints(&[0, 7]));
    // `return positive;` leaves the block and the function: both end.
    assert_eq!(values_of(&snaps, "positive"), ints(&[5]));
    let (obj, _) = history(&snaps, "positive")[0].clone();
    assert_eq!(obs.timeline.latest().object(obj).unwrap().lifetime.end_reason, Some(EndReason::ScopeExit));
    let entered = obs.timeline.events().iter().filter(|e| matches!(e.kind, EventKind::FunctionEntered { .. })).count();
    let exited = obs.timeline.events().iter().filter(|e| matches!(e.kind, EventKind::FunctionExited { .. })).count();
    assert_eq!((entered, exited), (5, 5), "every frame that was entered was left, exceptions included");
    assert_eq!(obs.timeline.latest().verify_invariants(), Ok(()));
}

#[test]
fn methods_constructors_and_destructors_get_frames() {
    let src = r#"struct Counter {
    int n;
    Counter(int start) : n(start) {}
    void bump() { n++; }
    ~Counter() { n = -1; }
};

int main() {
    Counter c(5);
    c.bump();
    c.bump();
    return c.n == 7 ? 0 : 1;
}
"#;
    let Some((_, obs)) = run_ok(src) else { return };
    let entered: Vec<String> = obs
        .timeline
        .events()
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::FunctionEntered { function, .. } => Some(function.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(entered, ["main", "Counter::Counter", "Counter::bump", "Counter::bump", "Counter::~Counter"]);
    let snaps = snapshots(&obs.timeline);
    // The object is a value with a field; the methods change that field in place.
    let agg = |n: i64| Value::aggregate(vec![Value::int(n)]);
    assert_eq!(values_of(&snaps, "c"), [agg(5), agg(6), agg(7), agg(-1)]);
    // The destructor runs while `main`'s frame is still the caller.
    let in_dtor = snaps.iter().find(|s| stack_names(s) == ["main", "Counter::~Counter"]).unwrap();
    assert!(in_dtor.variables().any(|v| v.name == "c"));
    assert!(obs.timeline.latest().stack(ThreadId::MAIN).is_empty());
}

#[test]
fn compound_assignment_and_increments_are_observed() {
    let src = r#"int main() {
    int x = 1;
    x += 4;
    x *= 3;
    ++x;
    x--;
    int y = x++;
    long big = 10;
    big <<= 2;
    return (x == 16 && y == 15 && big == 40) ? 0 : 1;
}
"#;
    let Some((_, obs)) = run_ok(src) else { return };
    let snaps = snapshots(&obs.timeline);
    assert_eq!(values_of(&snaps, "x"), ints(&[1, 5, 15, 16, 15, 16]));
    assert_eq!(values_of(&snaps, "y"), ints(&[15]), "`y = x++` takes the old value");
    assert_eq!(values_of(&snaps, "big"), ints(&[10, 40]));
}

#[test]
fn functions_that_cannot_be_framed_are_left_alone() {
    // A function-try-block body cannot take a frame guard: nothing inside it may
    // attach to the caller's frame either.
    let src = r#"int f(int x) try {
    int y = x;
    return y;
} catch (...) {
    return 0;
}

int main() {
    int z = f(1);
    return z == 1 ? 0 : 1;
}
"#;
    let Some((_, obs)) = run_ok(src) else { return };
    let snaps = snapshots(&obs.timeline);
    assert!(snaps.iter().all(|s| s.variables().all(|v| v.name != "y" && v.name != "x")));
    assert_eq!(values_of(&snaps, "z"), ints(&[1]));
    assert_eq!(obs.timeline.latest().verify_invariants(), Ok(()));
}

#[test]
fn loops_with_missing_parts_behave_identically() {
    // The complete `for (decl; cond; inc)` form is observed; the others are left as
    // they are, and must still compile and run the same.
    let src = r#"#include <cstdio>
int main() {
    int n = 0;
    for (int i = 0; ; i++) {
        if (i > 2) {
            break;
        }
        n += i;
    }
    for (int j = 0; j < 3;) {
        j++;
        n += j;
    }
    int k = 0;
    for (; k < 3; k++) {
        n += k;
    }
    std::printf("%d\n", n);
    return 0;
}
"#;
    let Some((session, obs)) = run_ok(src) else { return };
    assert_eq!(session.stdout.trim(), "12"); // (0+1+2) + (1+2+3) + (0+1+2)
    assert_eq!(session.stdout, run_plain(src).stdout);
    assert!(obs.issues.is_empty());
}

const KITCHEN_SINK: &str = r#"#include <cstdio>
#include <string>
#include <vector>

int g_counter = 0;
int g_table[4] = {1, 2, 3, 4};

enum Color { Red, Green };

struct Point {
    int x;
    int y;
    Point(int a, int b) : x(a), y(b) {}
    int sum() const { return x + y; }
    void scale(int k) { x *= k; y *= k; }
};

struct Flags {
    unsigned a : 3;
    unsigned b : 5;
};

constexpr int twice(int v) { return v * 2; }

template <class T>
T maxof(T a, T b) {
    T r = a;
    if (b > a) {
        r = b;
    }
    return r;
}

int counter() {
    static int calls = 0;
    calls++;
    g_counter += 2;
    return calls;
}

int classify(int v) {
    switch (v) {
        case 0:
            return 10;
        case 1: {
            int local = v * 100;
            return local;
        }
        default:
            break;
    }
    return -1;
}

int find_first_negative(const int* a, int n) {
    for (int i = 0; i < n; i++) {
        if (a[i] < 0) {
            return i;
        }
    }
    return -1;
}

int main() {
    int total = 0;
    for (int i = 0; i < 3; i++) {
        for (int j = 0; j < 2; j++) {
            total += i * j;
        }
    }
    int k = 3;
    while (k > 0) {
        int step = k;
        total += step;
        k--;
    }
    do {
        total++;
    } while (total < 12);
    if (int t = total % 5) {
        total += t;
    }
    Point p(2, 3);
    p.scale(2);
    Color c = Green;
    c = Red;
    Flags f{};
    f.a = 5;
    auto add = [&](int v) {
        int doubled = v * 2;
        total += doubled;
        return doubled;
    };
    add(3);
    std::vector<int> v = {4, 5, 6};
    v.push_back(7);
    int vsum = 0;
    for (int e : v) {
        vsum += e;
    }
    std::string s = "hi";
    s += "!";
    const int base = twice(4);
    int arr[5] = {3, 1, -2, 4, 0};
    int* q = arr;
    q++;
    *q = 9;
    int idx = find_first_negative(arr, 5);
    int cls = classify(1) + classify(0) + classify(7);
    int m = maxof(3, 8);
    volatile int vol = 0;
    vol++;
    counter();
    counter();
    int jumped = 0;
    for (int a = 0; a < 3; a++) {
        for (int b = 0; b < 3; b++) {
            if (a == 1 && b == 1) {
                goto done;
            }
            jumped++;
        }
    }
done:
    std::printf("%d %d %d %d %d %d %d %d %d %d %d\n", total, p.sum(), vsum, base, idx,
                static_cast<int>(s.size()), cls, m, g_counter, jumped, static_cast<int>(c));
    return 0;
}
"#;

#[test]
fn a_kitchen_sink_of_constructs_behaves_identically() {
    // Loops of every kind, switch with a braced case, goto out of nested loops,
    // lambdas, statics and globals, bit-fields, volatile, constexpr and templates,
    // STL locals, methods and constructors: the rewrite must not change a thing the
    // program does, and everything it reports must be consistent.
    let Some((session, obs)) = run_ok(KITCHEN_SINK) else { return };
    let reference = run_plain(KITCHEN_SINK);
    assert_eq!(reference.state, SessionState::Completed, "{reference:#?}");
    assert_eq!(session.stdout, reference.stdout);
    assert_eq!(session.stderr, reference.stderr);
    assert_eq!(session.exit_code, reference.exit_code);
    println!("{}", session.stdout);

    let end = obs.timeline.latest();
    assert!(end.stack(ThreadId::MAIN).is_empty(), "every frame returned, goto included");
    assert_eq!(end.variables().count(), 0);
    assert_eq!(end.live_objects().count(), 0);
    assert_eq!(end.verify_invariants(), Ok(()));
    assert!(obs.summary.functions >= 6 && obs.summary.locals > 10, "{:?}", obs.summary);
    assert!(obs.timeline.len() > 200, "{} events", obs.timeline.len());

    // Spot checks that values are right, not just plentiful.
    let snaps = snapshots(&obs.timeline);
    assert_eq!(values_of(&snaps, "jumped").last(), Some(&Value::int(4)));
    assert_eq!(values_of(&snaps, "m"), ints(&[8]));
    assert_eq!(values_of(&snaps, "idx"), ints(&[2]));
}
