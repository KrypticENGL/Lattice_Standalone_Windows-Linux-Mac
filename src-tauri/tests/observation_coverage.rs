//! Runtime-observation coverage: which constructs reach the URR, and, for the ones
//! that do not yet, *exactly where* the information is lost. See
//! `docs/runtime-observation-coverage.md`.
//!
//! Every test runs the real pipeline (libclang analysis -> instrumentation ->
//! compiler -> running program -> URR); see `common`. The tests under "STL" are
//! characterization tests: they pin down today's behaviour so that a future
//! milestone that closes a gap has to update them on purpose.

mod common;

use common::*;
use lattice_lib::model::*;
use lattice_lib::observe::Observation;
use lattice_lib::runtime::SessionState;

/// Run `src` observed and require a clean, complete run.
fn run_ok(src: &str) -> Option<Observation> {
    let (session, obs) = observe(src)?;
    assert_eq!(session.state, SessionState::Completed, "{session:#?}");
    assert_eq!(session.exit_code, Some(0), "{session:#?}");
    assert!(obs.issues.is_empty(), "{:?}", obs.issues);
    assert!(obs.summary.skipped.is_none(), "{:?}", obs.summary.skipped);
    Some(obs)
}

/// The last state in which `main` still has its frame (just before it returns).
fn before_exit(t: &Timeline) -> RuntimeSnapshot {
    snapshots(t).into_iter().rev().find(|s| !s.stack(ThreadId::MAIN).is_empty()).expect("a state with a frame")
}

fn var<'a>(s: &'a RuntimeSnapshot, name: &str) -> &'a Variable {
    s.variables().find(|v| v.name == name).unwrap_or_else(|| panic!("no variable `{name}`"))
}

fn ints(v: &[i64]) -> Value {
    Value::array(v.iter().map(|i| Value::int(*i)).collect())
}

fn array_len(s: &RuntimeSnapshot, o: &Object) -> (Option<String>, Option<u64>) {
    match &s.types().get(o.ty).unwrap().kind {
        TypeKind::Array { element, len } => (s.types().name(*element).map(str::to_string), *len),
        other => panic!("not an array: {other:?}"),
    }
}

fn heap_objects(s: &RuntimeSnapshot) -> Vec<&Object> {
    s.objects().filter(|o| o.storage == StorageClass::Heap).collect()
}

// ---- stack array ----------------------------------------------------------------

#[test]
fn a_stack_array_is_one_array_object_and_its_writes_are_element_writes() {
    let src = "int main() {\n    int arr[5];\n    arr[0] = 10;\n    arr[1] = 20;\n    return 0;\n}\n";
    let Some(obs) = run_ok(src) else { return };
    let t = &obs.timeline;
    let end = before_exit(t);
    let arr = var(&end, "arr");
    let o = end.object(arr.object).unwrap();

    assert_eq!(o.storage, StorageClass::Automatic);
    assert_eq!(array_len(&end, o), (Some("int".into()), Some(5)));
    // Uninitialized elements are reported as such, never as 0.
    let Value::Array { elements } = &o.value else { panic!("{:?}", o.value) };
    assert_eq!(elements[0], Value::int(10));
    assert_eq!(elements[1], Value::int(20));
    assert!(elements[2..].iter().all(|e| e == &Value::unavailable(Unavailable::Uninitialized)), "{elements:?}");

    // Each write is a ValueChanged at (array object, [Index(i)]) on the right line.
    let writes: Vec<(Vec<Step>, Option<u32>)> = t
        .events()
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ValueChanged { place, .. } if place.object == arr.object => {
                Some((place.path.clone(), e.location.as_ref().map(|l| l.line)))
            }
            _ => None,
        })
        .collect();
    assert_eq!(writes, [(vec![Step::Index(0)], Some(3)), (vec![Step::Index(1)], Some(4))]);
}

// ---- dynamic array --------------------------------------------------------------

const DYNAMIC: &str = "int main() {
    int* arr = new int[5];
    for (int i = 0; i < 5; ++i) {
        arr[i] = i * 10;
    }
    delete[] arr;
    return 0;
}
";

#[test]
fn a_dynamic_array_is_a_heap_array_object_distinct_from_the_pointer_to_it() {
    let Some(obs) = run_ok(DYNAMIC) else { return };
    let t = &obs.timeline;
    let snaps = snapshots(t);

    // Three different things: the pointer variable, the array object, its elements.
    let filled = snaps.iter().find(|s| matches!(s.objects().find(|o| o.storage == StorageClass::Heap).map(|o| &o.value), Some(Value::Array { elements }) if elements.last() == Some(&Value::int(40)))).expect("a state with a full array");
    let heap = heap_objects(filled);
    assert_eq!(heap.len(), 1);
    let array = heap[0];
    let arr = var(filled, "arr");
    assert_ne!(arr.object, array.id, "the pointer variable is not the array");

    // Array object: element type int, length 5, all five elements.
    assert_eq!(array_len(filled, array), (Some("int".into()), Some(5)));
    assert_eq!(array.value, ints(&[0, 10, 20, 30, 40]));
    assert_eq!(array.lifetime.state, LifeState::Alive);

    // Pointer variable: type int*, pointing at the first element of the array.
    let pointer_obj = filled.object(arr.object).unwrap();
    assert_eq!(filled.types().name(pointer_obj.ty), Some("int*"));
    assert_eq!(pointer_obj.storage, StorageClass::Automatic);
    match &pointer_obj.value {
        Value::Pointer { pointer } => assert_eq!(pointer.target, Target::to(Place::root(array.id).index(0))),
        other => panic!("{other:?}"),
    }
    assert_eq!(filled.target_status(&Target::to(Place::root(array.id).index(0))), TargetStatus::Live);
    assert_eq!(filled.outgoing(arr.object).len(), 1);

    // The array is freed by `delete[]`, on its own line; the pointer now dangles.
    let end = before_exit(t);
    let freed = end.object(array.id).unwrap();
    assert_eq!((freed.lifetime.state, freed.lifetime.end_reason), (LifeState::Destroyed, Some(EndReason::Freed)));
    let destroy = t.events().iter().find(|e| matches!(e.kind, EventKind::ObjectDestroyed { .. }) && matches!(&e.kind, EventKind::ObjectDestroyed { object, .. } if *object == array.id)).unwrap();
    assert_eq!(destroy.location.as_ref().map(|l| l.line), Some(6));
    assert_eq!(end.target_status(&Target::to(Place::root(array.id).index(0))), TargetStatus::Dangling);
    assert_eq!(end.verify_invariants(), Ok(()));
}

#[test]
fn dynamic_array_writes_are_element_writes_with_their_source_line() {
    let src = "int main() {\n    int* arr = new int[3];\n    arr[0] = 10;\n    arr[1] = 20;\n    delete[] arr;\n}\n";
    let Some(obs) = run_ok(src) else { return };
    let t = &obs.timeline;
    let array = heap_ids(&t.latest())[0];

    // Allocation, construction (elements still indeterminate), then the writes.
    let created = snapshots(t).into_iter().find(|s| s.object(array).is_some_and(|o| o.lifetime.state == LifeState::Alive)).unwrap();
    assert_eq!(
        created.object(array).unwrap().value,
        Value::array(vec![Value::unavailable(Unavailable::Uninitialized); 3]),
        "`new int[3]` has indeterminate elements: they are not 0"
    );
    let writes: Vec<(Vec<Step>, Option<u32>, Value)> = t
        .events()
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ValueChanged { place, value } if place.object == array && !place.path.is_empty() => {
                Some((place.path.clone(), e.location.as_ref().map(|l| l.line), value.clone()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        writes,
        [(vec![Step::Index(0)], Some(3), Value::int(10)), (vec![Step::Index(1)], Some(4), Value::int(20))]
    );
    let snaps = snapshots(t);
    let last_alive = snaps.iter().rev().find(|s| s.object(array).is_some_and(|o| !o.is_destroyed())).unwrap();
    let Value::Array { elements } = &last_alive.object(array).unwrap().value else { panic!() };
    assert_eq!(elements[..2], [Value::int(10), Value::int(20)]);
    assert_eq!(elements[2], Value::unavailable(Unavailable::Uninitialized));
}

#[test]
fn value_initialized_class_and_nested_dynamic_arrays_are_tracked_generically() {
    let src = "struct P { int x; int y; };
int main() {
    int* z = new int[2]();
    P* ps = new P[2]{{1, 2}, {3, 4}};
    ps[1].x = 7;
    int (*m)[2] = new int[2][2];
    m[1][1] = 5;
    delete[] z;
    delete[] ps;
    delete[] m;
}
";
    let Some(obs) = run_ok(src) else { return };
    let snaps = snapshots(&obs.timeline);
    let s = snaps.iter().rev().find(|s| heap_objects(s).iter().filter(|o| !o.is_destroyed()).count() == 3).expect("all three alive at once");
    let heap = heap_objects(s);
    assert_eq!(heap.len(), 3);

    // `new int[2]()`: value-initialized, so zeros (not "uninitialized").
    assert_eq!(heap[0].value, ints(&[0, 0]));
    // `new P[2]{...}`: an array of records, each with its fields; `ps[1].x = 7` is a field of an element.
    assert_eq!(array_len(s, heap[1]), (Some("P".into()), Some(2)));
    assert_eq!(
        heap[1].value,
        Value::array(vec![
            Value::aggregate(vec![Value::int(1), Value::int(2)]),
            Value::aggregate(vec![Value::int(7), Value::int(4)])
        ])
    );
    // `new int[2][2]`: an array whose elements are arrays; the pointer is an `int(*)[2]` to element 0.
    assert_eq!(array_len(s, heap[2]), (Some("int[2]".into()), Some(2)));
    assert_eq!(s.types().name(heap[2].ty), Some("int[2][2]"));
    let Value::Array { elements } = &heap[2].value else { panic!() };
    assert_eq!(elements[1], Value::array(vec![Value::unavailable(Unavailable::Uninitialized), Value::int(5)]));
    let m = s.variable_value(var(s, "m").id).unwrap();
    assert!(matches!(m, Value::Pointer { pointer } if pointer.target == Target::to(Place::root(heap[2].id).index(0))), "{m:?}");
}

#[test]
fn a_pointer_into_the_middle_of_a_dynamic_array_points_at_that_element() {
    let src = "int main() {\n    int* arr = new int[4];\n    int* p = arr + 2;\n    *p = 9;\n    delete[] arr;\n}\n";
    let Some(obs) = run_ok(src) else { return };
    let snaps = snapshots(&obs.timeline);
    let s = snaps.iter().rev().find(|s| s.variables().any(|v| v.name == "p") && heap_objects(s).iter().all(|o| !o.is_destroyed())).unwrap();
    let array = heap_objects(s)[0].id;
    let p = s.variable_value(var(s, "p").id).unwrap();
    assert!(matches!(p, Value::Pointer { pointer } if pointer.target == Target::to(Place::root(array).index(2))), "{p:?}");
    // The write through `p` landed on element 2 of the array.
    let Value::Array { elements } = &s.object(array).unwrap().value else { panic!() };
    assert_eq!(elements[2], Value::int(9));
    // Both pointers are links into the same array object (aliases of one object).
    assert_eq!(s.incoming(array).len(), 2);
}

#[test]
fn allocations_that_do_not_fit_the_array_picture_stay_untracked_and_correct() {
    // A destructor makes the compiler add an array cookie (so the block is not the array); a
    // huge array exceeds what one event can carry. Both must simply run, unobserved.
    let src = "struct D { int v; ~D() {} };
int main() {
    D* ds = new D[3];
    int* big = new int[100000];
    big[5] = 1;
    int sum = big[5];
    delete[] big;
    delete[] ds;
    return sum == 1 ? 0 : 1;
}
";
    let Some(obs) = run_ok(src) else { return };
    assert!(heap_ids(&obs.timeline.latest()).is_empty());
    assert!(obs.timeline.events().iter().all(|e| !matches!(&e.kind, EventKind::ObjectAllocated { object } if object.storage == StorageClass::Heap)));
    let s = before_exit(&obs.timeline);
    // Honest about it: the pointers are plainly "not a tracked object".
    let ds = s.variable_value(var(&s, "ds").id).unwrap();
    assert!(matches!(ds, Value::Pointer { pointer } if pointer.target == Target::Unresolved), "{ds:?}");
}

// ---- user-defined objects, links, cycles ------------------------------------------

const NODE: &str = "struct Node {
    int value;
    Node* next;
};

int main() {
    Node* a = new Node{10, nullptr};
    Node* b = new Node{20, nullptr};
    a->next = b;
    b->next = a;
    delete b;
    delete a;
}
";

#[test]
fn user_defined_objects_links_and_cycles_reach_the_urr() {
    let Some(obs) = run_ok(NODE) else { return };
    let t = &obs.timeline;
    let snaps = snapshots(t);

    // Both nodes alive and linked in both directions: a cycle of two.
    let s = snaps.iter().find(|s| heap_ids(s).len() == 2 && s.outgoing(heap_ids(s)[1]).iter().any(|e| e.target.object == heap_ids(s)[0])).expect("the cycle");
    let [na, nb] = [heap_ids(s)[0], heap_ids(s)[1]];
    let node_ty = s.object(na).unwrap().ty;
    let value = field_named(s, node_ty, "value");
    let next = field_named(s, node_ty, "next");

    assert_eq!(s.value_at(&Place::root(na).field(value)), Some(&Value::int(10)));
    assert_eq!(s.value_at(&Place::root(nb).field(value)), Some(&Value::int(20)));
    let a_next = Edge { source: Place::root(na).field(next), kind: EdgeKind::Pointer, target: Place::root(nb) };
    let b_next = Edge { source: Place::root(nb).field(next), kind: EdgeKind::Pointer, target: Place::root(na) };
    assert_eq!(s.outgoing(na), [a_next]);
    assert_eq!(s.outgoing(nb), [b_next]);

    // After the deletes both are freed; nothing is left alive.
    assert_eq!(t.latest().live_objects().count(), 0);
}

#[test]
fn pointer_relationships_form_incrementally() {
    let Some(obs) = run_ok(NODE) else { return };
    let snaps = snapshots(&obs.timeline);
    // The number of heap-to-heap links over time: 0 -> 1 (a->next = b) -> 2 (b->next = a).
    let links: Vec<usize> = snaps.iter().map(|s| heap_ids(s).iter().map(|id| s.outgoing(*id).iter().filter(|e| e.target.object != *id && heap_ids(s).contains(&e.target.object)).count()).sum()).collect();
    let mut dedup = links.clone();
    dedup.dedup();
    assert_eq!(&dedup[..3], [0, 1, 2], "{links:?}");
}


// ---- library types: described from the compiler's layout, seen through their pointers ------
//
// None of this knows `std::vector` (or any library type) by name. A type the instrumenter
// has no descriptor for is described from the layout libclang reports; storage that library
// code allocates becomes an object once a typed pointer to its start is seen; and memory is
// compared after every statement, so changes made inside library calls are noticed. The
// assertions avoid member names (they differ between standard libraries) and look at
// structure: records, pointers, buffers, values.

/// Does `v` contain `want` anywhere inside it?
fn contains(v: &Value, want: &Value) -> bool {
    if v == want {
        return true;
    }
    match v {
        Value::Aggregate { fields } => fields.iter().any(|f| contains(f, want)),
        Value::Array { elements } => elements.iter().any(|e| contains(e, want)),
        _ => false,
    }
}

/// The *last* state in which some live heap object holds exactly `want`, and that object: by
/// then the pointers that lead to it have been updated too (a new buffer is announced a few
/// events before the container's own pointers change).
fn state_with_heap_value<'a>(snaps: &'a [RuntimeSnapshot], want: &Value) -> Option<(&'a RuntimeSnapshot, ObjectId)> {
    snaps.iter().rev().find_map(|s| {
        s.objects()
            .find(|o| o.storage == StorageClass::Heap && !o.is_destroyed() && &o.value == want)
            .map(|o| (s, o.id))
    })
}

fn array_len_opt(s: &RuntimeSnapshot, o: &Object) -> Option<u64> {
    match &s.types().get(o.ty)?.kind {
        TypeKind::Array { len, .. } => *len,
        _ => None,
    }
}

const VECTOR: &str = "#include <vector>
int main() {
    std::vector<int> v;
    v.push_back(10);
    v.push_back(20);
    v.push_back(30);
    return 0;
}
";

#[test]
fn std_vector_is_a_record_whose_pointers_lead_to_its_buffer() {
    let Some(obs) = run_ok(VECTOR) else { return };
    let t = &obs.timeline;
    let snaps = snapshots(t);

    // The element buffer is a heap array object holding exactly the elements.
    let Some((s, buffer)) = state_with_heap_value(&snaps, &ints(&[10, 20, 30])) else { panic!("no buffer ever held 10, 20, 30") };
    assert_eq!(array_len(s, s.object(buffer).unwrap()), (Some("int".into()), Some(3)));

    // The vector is a real record (not an opaque name), and its pointers lead to the buffer:
    // one at the first element, the rest at "one past the end" (a place the model calls out of bounds).
    let v = var(s, "v");
    let obj = s.object(v.object).unwrap();
    assert!(matches!(s.types().get(obj.ty).unwrap().kind, TypeKind::Record { .. }), "{:?}", s.types().get(obj.ty));
    assert_eq!(s.types().name(obj.ty), Some("std::vector<int>"));
    let edges = s.outgoing(v.object);
    assert!(edges.iter().all(|e| e.target.object == buffer && e.kind == EdgeKind::Pointer), "{edges:?}");
    assert!(edges.iter().any(|e| e.target == Place::root(buffer).index(0)), "a pointer to the first element: {edges:?}");
    let past_end = Place::root(buffer).index(3);
    assert!(edges.iter().any(|e| e.target == past_end), "a pointer one past the last element: {edges:?}");
    assert_eq!(s.target_status(&Target::to(past_end)), TargetStatus::OutOfBounds);

    // Growing replaces the buffer: three buffers of 1, 2 and 3 elements, the vector's pointers
    // following the live one, and only the latest alive at the end.
    let lens: Vec<u64> = t
        .events()
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::ObjectAllocated { object } if object.storage == StorageClass::Heap => match &object.value {
                Value::Array { elements } => Some(elements.len() as u64),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(lens, [1, 2, 3]);
    assert_eq!(s.verify_invariants(), Ok(()));
    assert_eq!(heap_objects(s).iter().filter(|o| !o.is_destroyed()).count(), 1, "only the latest buffer is alive");
    assert_eq!(heap_objects(s).len(), 3, "the two earlier buffers are kept, freed");
    // When the vector itself goes away its buffer goes with it.
    assert_eq!(before_exit(t).live_objects().filter(|o| o.storage == StorageClass::Heap).count(), 0);
}

#[test]
fn std_array_writes_through_operator_index_are_seen() {
    let src = "#include <array>
int main() {
    std::array<int, 3> a = {1, 2, 3};
    a[1] = 5;
    return 0;
}
";
    let Some(obs) = run_ok(src) else { return };
    let t = &obs.timeline;
    let end = before_exit(t);
    let a = var(&end, "a");
    let o = end.object(a.object).unwrap();
    assert!(matches!(end.types().get(o.ty).unwrap().kind, TypeKind::Record { .. }));
    assert!(contains(&o.value, &ints(&[1, 5, 3])), "{:?}", o.value);
    // `a[1] = 5` is a call, not a built-in assignment: it is noticed by comparing memory after
    // the statement, and attributed to that line, as a write to the element.
    let write = t
        .events()
        .iter()
        .find(|e| matches!(&e.kind, EventKind::ValueChanged { place, value } if place.object == a.object && *value == Value::int(5)))
        .expect("the write to a[1]");
    let EventKind::ValueChanged { place, .. } = &write.kind else { unreachable!() };
    assert_eq!(place.path.last(), Some(&Step::Index(1)));
    assert_eq!(write.location.as_ref().map(|l| l.line), Some(4));
}

#[test]
fn a_vector_of_user_structs_describes_its_elements_with_the_users_own_type() {
    let src = "#include <vector>
struct Node { int v; Node* next; };
int main() {
    std::vector<Node> nodes;
    nodes.push_back(Node{1, nullptr});
    nodes.push_back(Node{2, nullptr});
    nodes[0].next = &nodes[1];
    return 0;
}
";
    let Some(obs) = run_ok(src) else { return };
    let snaps = snapshots(&obs.timeline);
    let s = snaps
        .iter()
        .find(|s| {
            s.objects().any(|o| {
                o.storage == StorageClass::Heap
                    && !o.is_destroyed()
                    && array_len_opt(s, o) == Some(2)
                    && s.outgoing(o.id).iter().any(|e| e.target.object == o.id)
            })
        })
        .expect("a buffer of two Nodes in which the first points at the second");
    let buffer = s.objects().find(|o| o.storage == StorageClass::Heap && !o.is_destroyed() && array_len_opt(s, o) == Some(2)).unwrap();
    // The element type is the user's `Node`, the same one its `Node*` fields point at.
    let (element, _) = array_len(s, buffer);
    assert_eq!(element.as_deref(), Some("Node"));
    let node_ty = s.types().iter().find(|t| t.name == "Node" && matches!(t.kind, TypeKind::Record { .. })).expect("Node record").id;
    let next = field_named(s, node_ty, "next");
    let link = Edge { source: Place::root(buffer.id).index(0).field(next), kind: EdgeKind::Pointer, target: Place::root(buffer.id).index(1) };
    assert!(s.outgoing(buffer.id).contains(&link), "{:?}", s.outgoing(buffer.id));
    let Value::Array { elements } = &buffer.value else { panic!() };
    assert_eq!(elements[1], Value::aggregate(vec![Value::int(2), Value::null_pointer()]));
}

#[test]
fn a_library_member_of_a_user_struct_is_described_and_followed() {
    let src = "#include <vector>
struct Bag { int id; std::vector<int> items; };
int main() {
    Bag b;
    b.id = 7;
    b.items.push_back(1);
    b.items.push_back(2);
    return 0;
}
";
    let Some(obs) = run_ok(src) else { return };
    let snaps = snapshots(&obs.timeline);
    let (s, buffer) = state_with_heap_value(&snaps, &ints(&[1, 2])).expect("the items buffer");
    let b = var(s, "b");
    let o = s.object(b.object).unwrap();
    let TypeKind::Record { fields, .. } = &s.types().get(o.ty).unwrap().kind else { panic!() };
    assert_eq!(fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["id", "items"]);
    assert!(matches!(&s.types().get(fields[1].ty).unwrap().kind, TypeKind::Record { .. }), "`items` is described, not opaque");
    assert!(s.outgoing(b.object).iter().all(|e| e.target.object == buffer && e.source.path[0] == Step::Field(1)));
    assert!(contains(&o.value, &Value::int(7)));
}

#[test]
fn a_strings_heap_buffer_is_a_char_array_holding_the_text() {
    let src = "#include <string>
int main() {
    std::string s = \"hello\";
    s += \", this text is long enough that it cannot live inside the string object\";
    return 0;
}
";
    let Some(obs) = run_ok(src) else { return };
    let snaps = snapshots(&obs.timeline);
    let text = "hello, this text is long enough that it cannot live inside the string object";
    let found = snaps.iter().any(|s| {
        s.objects().any(|o| {
            let Value::Array { elements } = &o.value else { return false };
            o.storage == StorageClass::Heap
                && elements.len() >= text.len()
                && elements.iter().zip(text.bytes()).all(|(e, b)| *e == Value::Char { value: b as u32 })
        })
    });
    assert!(found, "no heap char array ever held the text");
}

#[test]
fn smart_pointers_and_linked_containers_show_their_links() {
    let src = "#include <list>
#include <memory>
struct Node { int v; Node* next; };
int main() {
    std::unique_ptr<Node> p(new Node{4, nullptr});
    p->v = 9;
    std::list<int> l;
    l.push_back(1);
    l.push_back(2);
    return 0;
}
";
    let Some(obs) = run_ok(src) else { return };
    let snaps = snapshots(&obs.timeline);
    let s = snaps
        .iter()
        .rev()
        .find(|s| s.variables().any(|v| v.name == "l") && heap_objects(s).iter().filter(|o| !o.is_destroyed()).count() >= 3)
        .expect("a state with the Node and the list's nodes alive");
    // unique_ptr: the Node is the target of a link from the smart pointer's own object.
    let node = s.objects().find(|o| o.storage == StorageClass::Heap && !o.is_destroyed() && contains(&o.value, &Value::int(9))).expect("the Node");
    assert!(s.outgoing(var(s, "p").object).iter().any(|e| e.target.object == node.id), "p -> Node");
    // list: doubly linked, so two nodes link to each other.
    let live: Vec<&Object> = heap_objects(s).into_iter().filter(|o| !o.is_destroyed() && o.id != node.id).collect();
    let mutual = live.iter().any(|a| {
        live.iter().any(|b| {
            a.id != b.id
                && s.outgoing(a.id).iter().any(|e| e.target.object == b.id)
                && s.outgoing(b.id).iter().any(|e| e.target.object == a.id)
        })
    });
    assert!(mutual, "two list nodes point at each other");
}

#[test]
fn calls_into_the_library_that_change_the_programs_memory_are_seen() {
    let src = "#include <algorithm>
#include <cstring>
int main() {
    int buf[4];
    std::memset(buf, 0, sizeof buf);
    std::fill(buf, buf + 4, 7);
    return buf[0] == 7 ? 0 : 1;
}
";
    let Some(obs) = run_ok(src) else { return };
    let t = &obs.timeline;
    let end = before_exit(t);
    assert_eq!(end.object(var(&end, "buf").object).unwrap().value, ints(&[7, 7, 7, 7]));
    // The intermediate state (after memset) was seen too, each on its own line.
    let snaps = snapshots(t);
    assert!(snaps.iter().any(|s| s.variables().any(|v| v.name == "buf") && s.object(var(s, "buf").object).unwrap().value == ints(&[0, 0, 0, 0])));
    let lines: Vec<Option<u32>> = t
        .events()
        .iter()
        .filter(|e| matches!(&e.kind, EventKind::ValueChanged { place, .. } if !place.path.is_empty()))
        .map(|e| e.location.as_ref().map(|l| l.line))
        .collect();
    assert!(lines.contains(&Some(5)) && lines.contains(&Some(6)), "{lines:?}");
}

#[test]
fn a_program_using_many_library_types_stays_correct_and_consistent() {
    let src = r#"#include <vector>
#include <string>
#include <map>
#include <set>
#include <list>
#include <deque>
#include <memory>
#include <optional>
#include <functional>
#include <unordered_map>
#include <sstream>
#include <iostream>
#include <algorithm>
#include <cstring>

struct Item { int id; std::string name; std::vector<int> tags; };

int main() {
    std::vector<Item> items;
    for (int i = 0; i < 4; ++i) {
        Item it;
        it.id = i;
        it.name = "item-number-" + std::to_string(i) + "-with-a-long-enough-name";
        it.tags.push_back(i);
        it.tags.push_back(i * 2);
        items.push_back(it);
    }
    std::map<std::string, int> m;
    m["a"] = 1;
    m["b"] = 2;
    std::set<int> st{3, 1, 2};
    std::deque<int> dq;
    dq.push_back(1);
    dq.push_front(0);
    std::unordered_map<int, std::string> um;
    um[1] = "one";
    std::shared_ptr<Item> sp = std::make_shared<Item>();
    sp->id = 5;
    std::optional<int> op = 3;
    std::function<int(int)> fn = [&](int x) { return x + items.size(); };
    int r = fn(1);
    std::ostringstream os;
    os << "x" << r;
    std::string out = os.str();
    int buf[8];
    std::memset(buf, 0, sizeof buf);
    std::fill(buf, buf + 4, 7);
    std::sort(items.begin(), items.end(), [](const Item& a, const Item& b) { return a.id > b.id; });
    std::cout << out << std::endl;
    return (r == 5 && buf[2] == 7) ? 0 : 1;
}
"#;
    let Some((session, obs)) = observe(src) else { return };
    // The program behaves exactly as without observation...
    assert_eq!(session.state, SessionState::Completed, "{session:#?}");
    assert_eq!(session.exit_code, Some(0));
    assert_eq!(session.stdout.trim(), "x5");
    // ...and every event was accepted by the URR, which ends consistent and empty.
    assert!(obs.issues.is_empty(), "{:?}", obs.issues);
    let end = obs.timeline.latest();
    assert_eq!(end.verify_invariants(), Ok(()));
    assert_eq!(end.live_objects().count(), 0, "everything the program owned was freed");
    let announced = obs
        .timeline
        .events()
        .iter()
        .filter(|e| matches!(&e.kind, EventKind::ObjectAllocated { object } if object.storage == StorageClass::Heap))
        .count();
    assert!(announced > 20, "library storage became objects: {announced}");
}

#[test]
fn a_large_tracked_state_does_not_make_observation_quadratic() {
    // Comparing memory after every statement costs in proportion to what is tracked, so with a
    // lot of state it runs less often (every n-th statement). The run must stay fast, correct and
    // consistent; a change may then be seen a few statements late, never lost or duplicated wrongly.
    let src = "#include <vector>
struct Node { int value; Node* next; };
int main() {
    Node* head = nullptr;
    for (int i = 0; i < 4000; i++) { Node* n = new Node{i, head}; head = n; }
    std::vector<int> v;
    for (int i = 0; i < 3000; i++) { v.push_back(i); }
    int sum = 0;
    for (Node* p = head; p != nullptr; p = p->next) { sum += p->value; }
    while (head) { Node* n = head->next; delete head; head = n; }
    return (sum == 7998000 && v.size() == 3000) ? 0 : 1;
}
";
    let started = std::time::Instant::now();
    let Some((session, obs)) = observe(src) else { return };
    assert_eq!(session.state, SessionState::Completed, "{session:#?}");
    assert_eq!(session.exit_code, Some(0));
    assert!(obs.issues.is_empty(), "{:?}", obs.issues);
    assert_eq!(obs.timeline.latest().verify_invariants(), Ok(()));
    assert!(started.elapsed() < std::time::Duration::from_secs(60), "{:?}", started.elapsed());
    assert_eq!(obs.timeline.latest().live_objects().count(), 0, "everything was freed by the end");
}
