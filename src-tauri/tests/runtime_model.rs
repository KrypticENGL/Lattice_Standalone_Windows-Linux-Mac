//! Unit tests for the universal runtime model.
//!
//! The events below are hand-built *test fixtures* describing what a future
//! observer would report for small C++ snippets (quoted in each test). Nothing
//! here observes a real program. After every event the state's internal
//! invariants are re-verified.

use std::collections::BTreeSet;
use std::sync::Arc;

use lattice_lib::model::*;

const MAIN: ThreadId = ThreadId::MAIN;

// ---- fixture helpers ------------------------------------------------------

struct Script {
    timeline: Timeline,
    seq: u64,
}

impl Script {
    fn new() -> Self {
        Script { timeline: Timeline::new(), seq: 0 }
    }

    fn with_interval(n: usize) -> Self {
        Script { timeline: Timeline::with_checkpoint_interval(n), seq: 0 }
    }

    fn event(&self, thread: ThreadId, location: Option<SourceLocation>, kind: EventKind) -> RuntimeEvent {
        RuntimeEvent { seq: EventSeq(self.seq), thread, timestamp_ns: None, location, kind }
    }

    fn try_push_on(
        &mut self,
        thread: ThreadId,
        location: Option<SourceLocation>,
        kind: EventKind,
    ) -> Result<(), TimelineError> {
        let e = self.event(thread, location, kind);
        let r = self.timeline.push(e);
        if r.is_ok() {
            self.seq += 1;
        }
        r
    }

    fn try_push(&mut self, kind: EventKind) -> Result<(), TimelineError> {
        self.try_push_on(MAIN, None, kind)
    }

    fn push(&mut self, kind: EventKind) {
        self.try_push(kind).expect("event rejected");
        self.state().verify_invariants().expect("invariants");
    }

    fn push_on(&mut self, thread: ThreadId, location: Option<SourceLocation>, kind: EventKind) {
        self.try_push_on(thread, location, kind).expect("event rejected");
        self.state().verify_invariants().expect("invariants");
    }

    fn state(&self) -> RuntimeSnapshot {
        self.timeline.latest()
    }
}

const INT: TypeId = TypeId(1);
const NODE: TypeId = TypeId(2);
const NODE_PTR: TypeId = TypeId(3);

fn int_type() -> EventKind {
    EventKind::TypeDeclared {
        def: TypeDef::new(INT, "int", TypeKind::Primitive { primitive: Primitive::Int }).with_size(4),
    }
}

/// `struct Node { int value; Node* next; };` declared in the "wrong" order on
/// purpose: Node refers to `Node*`, which refers back to Node.
fn declare_node_types(s: &mut Script) {
    s.push(int_type());
    s.push(EventKind::TypeDeclared {
        def: TypeDef::new(
            NODE,
            "Node",
            TypeKind::Record {
                record: RecordKind::Struct,
                fields: vec![FieldDecl::new("value", INT), FieldDecl::new("next", NODE_PTR)],
                template_args: vec![],
            },
        )
        .with_size(16),
    });
    s.push(EventKind::TypeDeclared {
        def: TypeDef::new(NODE_PTR, "Node*", TypeKind::Pointer { pointee: NODE }).with_size(8),
    });
}

fn decl(id: u64, ty: TypeId, storage: StorageClass, value: Value) -> ObjectDecl {
    ObjectDecl {
        id: ObjectId(id),
        ty,
        storage,
        address: None,
        size: None,
        state: LifeState::Alive,
        value,
    }
}

fn alloc(s: &mut Script, id: u64, ty: TypeId, storage: StorageClass, value: Value) {
    s.push(EventKind::ObjectAllocated { object: decl(id, ty, storage, value) });
}

fn node(value: i64, next: Value) -> Value {
    Value::aggregate(vec![Value::int(value), next])
}

fn enter(s: &mut Script, frame: u64, name: &str) {
    s.push(EventKind::FunctionEntered { frame: FrameId(frame), function: name.into(), call_site: None });
}

fn bind(s: &mut Script, var: u64, name: &str, frame: u64, object: u64) {
    s.push(EventKind::VariableCreated {
        variable: VariableDecl {
            id: VariableId(var),
            name: name.into(),
            kind: VariableKind::Local,
            frame: Some(FrameId(frame)),
            scope: None,
            object: ObjectId(object),
        },
    });
}

/// A local: automatic storage + the variable naming it.
fn local(s: &mut Script, frame: u64, var: u64, name: &str, ty: TypeId, value: Value) -> ObjectId {
    let obj = 1000 + var;
    alloc(s, obj, ty, StorageClass::Automatic, value);
    bind(s, var, name, frame, obj);
    ObjectId(obj)
}

fn set(s: &mut Script, place: Place, value: Value) {
    s.push(EventKind::ValueChanged { place, value });
}

/// Generic reachability over links only. Knows nothing about what shape it walks.
fn reachable(state: &RuntimeState, start: ObjectId) -> Vec<ObjectId> {
    let mut seen = BTreeSet::from([start]);
    let mut order = vec![start];
    let mut i = 0;
    while i < order.len() {
        for e in state.outgoing(order[i]) {
            if seen.insert(e.target.object) {
                order.push(e.target.object);
            }
        }
        i += 1;
    }
    order
}

fn ptr_target(state: &RuntimeState, place: &Place) -> Target {
    match state.value_at(place).expect("place") {
        Value::Pointer { pointer } => pointer.target.clone(),
        other => panic!("not a pointer: {other:?}"),
    }
}

// ---- 1-2: variables -------------------------------------------------------

#[test]
fn primitive_variable() {
    // int x = 10;
    let mut s = Script::new();
    s.push(int_type());
    enter(&mut s, 1, "main");
    let x = local(&mut s, 1, 1, "x", INT, Value::int(10));

    let st = s.state();
    let var = st.variable(VariableId(1)).unwrap();
    assert_eq!(var.name, "x");
    assert_eq!(var.object, x);
    assert_eq!(st.variable_value(VariableId(1)), Some(&Value::int(10)));
    assert_eq!(st.type_of(x).unwrap().name, "int");
    assert_eq!(st.object(x).unwrap().lifetime.state, LifeState::Alive);
    assert!(st.outgoing(x).is_empty());
}

#[test]
fn multiple_variables_are_independent() {
    // int x = 10; int y = 20;
    let mut s = Script::new();
    s.push(int_type());
    enter(&mut s, 1, "main");
    let x = local(&mut s, 1, 1, "x", INT, Value::int(10));
    let y = local(&mut s, 1, 2, "y", INT, Value::int(20));
    assert_ne!(x, y);

    set(&mut s, Place::root(x), Value::int(11));

    let st = s.state();
    let names: Vec<_> = st.frame_variables(FrameId(1)).map(|v| v.name.as_str()).collect();
    assert_eq!(names, ["x", "y"]);
    assert_eq!(st.variable_value(VariableId(1)), Some(&Value::int(11)));
    assert_eq!(st.variable_value(VariableId(2)), Some(&Value::int(20)));
}

// ---- 3: objects -----------------------------------------------------------

#[test]
fn user_defined_object() {
    // struct Node { int value; Node* next; };  Node n{10, nullptr};
    let mut s = Script::new();
    declare_node_types(&mut s);
    enter(&mut s, 1, "main");
    let n = local(&mut s, 1, 1, "n", NODE, node(10, Value::null_pointer()));

    let st = s.state();
    assert_eq!(st.type_of(n).unwrap().name, "Node");
    // Field names come from the type table, not from the value.
    assert_eq!(st.types().field(NODE, 0).unwrap().name, "value");
    assert_eq!(st.types().field(NODE, 1).unwrap().name, "next");
    assert_eq!(st.value_at(&Place::root(n).field(0)), Some(&Value::int(10)));

    set(&mut s, Place::root(n).field(0), Value::int(99));
    assert_eq!(s.state().value_at(&Place::root(n).field(0)), Some(&Value::int(99)));
}

// ---- 4-6: pointers --------------------------------------------------------

#[test]
fn pointer_relationship() {
    // Node a; Node b; Node* p = &b;
    let mut s = Script::new();
    declare_node_types(&mut s);
    enter(&mut s, 1, "main");
    let _a = local(&mut s, 1, 1, "a", NODE, node(1, Value::null_pointer()));
    let b = local(&mut s, 1, 2, "b", NODE, node(2, Value::null_pointer()));
    let p = local(&mut s, 1, 3, "p", NODE_PTR, Value::pointer_to(b));

    let st = s.state();
    let out = st.outgoing(p);
    assert_eq!(
        out,
        vec![Edge { source: Place::root(p), kind: EdgeKind::Pointer, target: Place::root(b) }]
    );
    // The same fact from the other side, without scanning anything.
    assert_eq!(st.incoming(b), out);
    assert_eq!(st.target_status(&out[0].target_as_target()), TargetStatus::Live);
    // The variable and the object it points to are separate entities.
    assert_ne!(st.variable(VariableId(3)).unwrap().object, b);
}

trait EdgeExt {
    fn target_as_target(&self) -> Target;
}
impl EdgeExt for Edge {
    fn target_as_target(&self) -> Target {
        Target::to(self.target.clone())
    }
}

#[test]
fn multiple_aliases_share_one_object() {
    // Node node; Node* a = &node; Node* b = &node;
    let mut s = Script::new();
    declare_node_types(&mut s);
    enter(&mut s, 1, "main");
    let n = local(&mut s, 1, 1, "node", NODE, node(7, Value::null_pointer()));
    let a = local(&mut s, 1, 2, "a", NODE_PTR, Value::pointer_to(n));
    let b = local(&mut s, 1, 3, "b", NODE_PTR, Value::pointer_to(n));

    let st = s.state();
    let inc = st.incoming(n);
    assert_eq!(inc.len(), 2);
    let sources: BTreeSet<_> = inc.iter().map(|e| e.source.object).collect();
    assert_eq!(sources, BTreeSet::from([a, b]));
    assert!(inc.iter().all(|e| e.target == Place::root(n)));

    // Writing through the object is seen by both: there is one object.
    set(&mut s, Place::root(n).field(0), Value::int(8));
    let st = s.state();
    for p in [a, b] {
        let Target::Place { place } = ptr_target(&st, &Place::root(p)) else { panic!() };
        assert_eq!(st.value_at(&place.clone().field(0)), Some(&Value::int(8)));
    }

    // Re-pointing one alias updates the reverse index.
    let other = local(&mut s, 1, 4, "other", NODE, node(0, Value::null_pointer()));
    set(&mut s, Place::root(b), Value::pointer_to(other));
    let st = s.state();
    assert_eq!(st.incoming(n).len(), 1);
    assert_eq!(st.incoming(other).len(), 1);
}

#[test]
fn null_pointer() {
    // Node* p = nullptr;
    let mut s = Script::new();
    declare_node_types(&mut s);
    enter(&mut s, 1, "main");
    let p = local(&mut s, 1, 1, "p", NODE_PTR, Value::null_pointer());

    let st = s.state();
    assert_eq!(ptr_target(&st, &Place::root(p)), Target::Null);
    assert_eq!(st.target_status(&Target::Null), TargetStatus::Null);
    assert!(st.outgoing(p).is_empty(), "null is not a link to anything");
    assert!(!st.variable_value(VariableId(1)).unwrap().is_unavailable());
    // Null is distinct from "non-null but untracked".
    assert_ne!(Target::Null, Target::Unresolved);
    assert_eq!(st.target_status(&Target::Unresolved), TargetStatus::Unresolved);
}

// ---- 7-8: graphs ----------------------------------------------------------

fn three_heap_nodes(s: &mut Script) -> (ObjectId, ObjectId, ObjectId) {
    declare_node_types(s);
    enter(s, 1, "main");
    for (id, v) in [(10, 1), (11, 2), (12, 3)] {
        alloc(s, id, NODE, StorageClass::Heap, node(v, Value::null_pointer()));
    }
    (ObjectId(10), ObjectId(11), ObjectId(12))
}

#[test]
fn object_graph_a_to_b_to_c() {
    // Node* a = new Node{1}; a->next = new Node{2}; a->next->next = new Node{3};
    let mut s = Script::new();
    let (a, b, c) = three_heap_nodes(&mut s);
    local(&mut s, 1, 1, "head", NODE_PTR, Value::pointer_to(a));
    set(&mut s, Place::root(a).field(1), Value::pointer_to(b));
    set(&mut s, Place::root(b).field(1), Value::pointer_to(c));

    let st = s.state();
    assert_eq!(st.outgoing(a)[0].target, Place::root(b));
    assert_eq!(st.outgoing(b)[0].target, Place::root(c));
    assert!(st.outgoing(c).is_empty());
    assert_eq!(st.outgoing(a)[0].source, Place::root(a).field(1), "link source names the field");
    assert_eq!(reachable(&st, a), [a, b, c]);
    assert_eq!(reachable(&st, c), [c]);
    assert_eq!(st.incoming(b).len(), 1);
}

#[test]
fn cyclic_relationship() {
    // A->next = B; B->next = A;  (plus a self-loop for good measure)
    let mut s = Script::new();
    let (a, b, c) = three_heap_nodes(&mut s);
    set(&mut s, Place::root(a).field(1), Value::pointer_to(b));
    set(&mut s, Place::root(b).field(1), Value::pointer_to(a));
    set(&mut s, Place::root(c).field(1), Value::pointer_to(c));

    let st = s.state();
    assert_eq!(reachable(&st, a), [a, b], "traversal terminates on a cycle");
    assert_eq!(st.incoming(a)[0].source, Place::root(b).field(1));
    assert_eq!(st.incoming(b)[0].source, Place::root(a).field(1));
    assert_eq!(st.incoming(c).len(), 1, "self-loop is an ordinary link");
    assert_eq!(st.outgoing(c)[0].target, Place::root(c));

    // Breaking the cycle removes the link from both directions.
    set(&mut s, Place::root(b).field(1), Value::null_pointer());
    let st = s.state();
    assert_eq!(reachable(&st, a), [a, b]);
    assert!(st.incoming(a).is_empty());
}

#[test]
fn self_referential_allocation() {
    // An object may point at itself in its very first value.
    let mut s = Script::new();
    declare_node_types(&mut s);
    alloc(&mut s, 5, NODE, StorageClass::Heap, node(1, Value::pointer_to(ObjectId(5))));
    assert_eq!(s.state().incoming(ObjectId(5)).len(), 1);
}

// ---- 9: arrays ------------------------------------------------------------

#[test]
fn arrays() {
    // int arr[3] = {1, 2, 3};
    let mut s = Script::new();
    s.push(int_type());
    let arr_ty = TypeId(20);
    s.push(EventKind::TypeDeclared {
        def: TypeDef::new(arr_ty, "int[3]", TypeKind::Array { element: INT, len: Some(3) }).with_size(12),
    });
    enter(&mut s, 1, "main");
    let arr = local(
        &mut s,
        1,
        1,
        "arr",
        arr_ty,
        Value::array(vec![Value::int(1), Value::int(2), Value::int(3)]),
    );

    let st = s.state();
    match st.variable_value(VariableId(1)).unwrap() {
        Value::Array { elements } => assert_eq!(elements.len(), 3),
        other => panic!("not an array: {other:?}"),
    }
    assert_eq!(st.value_at(&Place::root(arr).index(2)), Some(&Value::int(3)));
    assert_eq!(st.value_at(&Place::root(arr).index(3)), None);

    // One element changes; the others do not.
    set(&mut s, Place::root(arr).index(1), Value::int(20));
    let st = s.state();
    assert_eq!(st.value_at(&Place::root(arr).index(0)), Some(&Value::int(1)));
    assert_eq!(st.value_at(&Place::root(arr).index(1)), Some(&Value::int(20)));
    // Out-of-range writes are rejected, not silently grown.
    assert!(matches!(
        s.try_push(EventKind::ValueChanged { place: Place::root(arr).index(3), value: Value::int(0) }),
        Err(TimelineError::Rejected(ApplyError::BadPlace(_)))
    ));

    // `int* q = &arr[2];` points at an element, not the whole array.
    s.push(EventKind::TypeDeclared {
        def: TypeDef::new(TypeId(21), "int*", TypeKind::Pointer { pointee: INT }),
    });
    let q = local(&mut s, 1, 2, "q", TypeId(21), Value::pointer_to_place(Place::root(arr).index(2)));
    let st = s.state();
    let e = &st.outgoing(q)[0];
    assert_eq!(e.target, Place::root(arr).index(2));
    assert_eq!(st.value_at(&e.target), Some(&Value::int(3)));
    assert_eq!(st.incoming(arr)[0].source, Place::root(q));
}

#[test]
fn array_of_pointers() {
    // Node* slots[3] = {&a, nullptr, &b};
    let mut s = Script::new();
    let (a, b, _c) = three_heap_nodes(&mut s);
    let ty = TypeId(30);
    s.push(EventKind::TypeDeclared {
        def: TypeDef::new(ty, "Node*[3]", TypeKind::Array { element: NODE_PTR, len: Some(3) }),
    });
    let slots = local(
        &mut s,
        1,
        1,
        "slots",
        ty,
        Value::array(vec![Value::pointer_to(a), Value::null_pointer(), Value::pointer_to(b)]),
    );

    let st = s.state();
    let out = st.outgoing(slots);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].source, Place::root(slots).index(0));
    assert_eq!(out[1].source, Place::root(slots).index(2));
    assert_eq!(reachable(&st, slots), [slots, a, b]);
}

// ---- 10: nesting ----------------------------------------------------------

#[test]
fn nested_objects() {
    // struct C { int z; }; struct B { C c; }; struct A { B b; };  A a{{{5}}};
    let mut s = Script::new();
    s.push(int_type());
    let (c_ty, b_ty, a_ty) = (TypeId(40), TypeId(41), TypeId(42));
    for (id, name, member, member_ty) in
        [(c_ty, "C", "z", INT), (b_ty, "B", "c", c_ty), (a_ty, "A", "b", b_ty)]
    {
        s.push(EventKind::TypeDeclared {
            def: TypeDef::new(
                id,
                name,
                TypeKind::Record {
                    record: RecordKind::Struct,
                    fields: vec![FieldDecl::new(member, member_ty)],
                    template_args: vec![],
                },
            ),
        });
    }
    enter(&mut s, 1, "main");
    let a = local(
        &mut s,
        1,
        1,
        "a",
        a_ty,
        Value::aggregate(vec![Value::aggregate(vec![Value::aggregate(vec![Value::int(5)])])]),
    );

    let z = Place::root(a).field(0).field(0).field(0);
    assert_eq!(s.state().value_at(&z), Some(&Value::int(5)));

    // One object, nested by containment; no ids were minted for B or C.
    assert_eq!(s.state().live_objects().count(), 1);

    set(&mut s, z.clone(), Value::int(6));
    assert_eq!(s.state().value_at(&z), Some(&Value::int(6)));

    // A pointer to the nested subobject `&a.b.c` is a link with a deep target.
    s.push(EventKind::TypeDeclared {
        def: TypeDef::new(TypeId(43), "B*", TypeKind::Pointer { pointee: b_ty }),
    });
    let p = local(&mut s, 1, 2, "p", TypeId(43), Value::pointer_to_place(Place::root(a).field(0).field(0)));
    let e = &s.state().outgoing(p)[0];
    assert_eq!(e.target.object, a);
    assert_eq!(e.target.path.len(), 2);
}

#[test]
fn union_value_addresses_only_the_active_member() {
    let mut s = Script::new();
    s.push(int_type());
    let ty = TypeId(50);
    s.push(EventKind::TypeDeclared {
        def: TypeDef::new(
            ty,
            "U",
            TypeKind::Record {
                record: RecordKind::Union,
                fields: vec![FieldDecl::new("i", INT), FieldDecl::new("f", INT)],
                template_args: vec![],
            },
        ),
    });
    alloc(&mut s, 1, ty, StorageClass::Static, Value::Union { active: Some(0), value: Box::new(Value::int(3)) });
    let st = s.state();
    assert_eq!(st.value_at(&Place::root(ObjectId(1)).field(0)), Some(&Value::int(3)));
    assert_eq!(st.value_at(&Place::root(ObjectId(1)).field(1)), None);
}

// ---- 11: lifetime ---------------------------------------------------------

#[test]
fn object_lifetime_allocated_alive_destroyed() {
    // Node* p = new Node{1}; ... delete p;
    let mut s = Script::new();
    declare_node_types(&mut s);
    enter(&mut s, 1, "main");
    let raw = ObjectDecl {
        state: LifeState::Allocated,
        ..decl(10, NODE, StorageClass::Heap, Value::unavailable(Unavailable::Uninitialized))
    };
    s.push(EventKind::ObjectAllocated { object: raw });
    let obj = ObjectId(10);
    let allocated_at = s.state().object(obj).unwrap().lifetime.allocated_at;
    assert_eq!(s.state().object(obj).unwrap().lifetime.state, LifeState::Allocated);

    // Double construction is a lifetime error.
    s.push(EventKind::ObjectConstructed { object: obj });
    assert_eq!(s.state().object(obj).unwrap().lifetime.state, LifeState::Alive);
    assert!(matches!(
        s.try_push(EventKind::ObjectConstructed { object: obj }),
        Err(TimelineError::Rejected(ApplyError::BadLifetime { .. }))
    ));

    set(&mut s, Place::root(obj), node(1, Value::null_pointer()));
    let p = local(&mut s, 1, 1, "p", NODE_PTR, Value::pointer_to(obj));
    assert_eq!(s.state().live_objects().count(), 2);

    s.push(EventKind::ObjectDestroyed { object: obj, reason: EndReason::Freed });
    let st = s.state();
    let o = st.object(obj).unwrap();
    assert_eq!(o.lifetime.state, LifeState::Destroyed);
    assert_eq!(o.lifetime.end_reason, Some(EndReason::Freed));
    assert!(o.lifetime.ended_at.unwrap() > allocated_at);
    // The object and its last value are retained for the timeline...
    assert_eq!(st.value_at(&Place::root(obj).field(0)), Some(&Value::int(1)));
    assert_eq!(st.live_objects().count(), 1);
    // ...and `p` is now a dangling pointer, with no write needed to `p`.
    let target = ptr_target(&st, &Place::root(p));
    assert_eq!(st.target_status(&target), TargetStatus::Dangling);
    assert_eq!(st.incoming(obj).len(), 1, "the dangling link is still visible");

    // Use-after-free and double-free are reported and change nothing.
    let before = st.last_seq();
    assert_eq!(
        s.try_push(EventKind::ValueChanged { place: Place::root(obj).field(0), value: Value::int(2) }),
        Err(TimelineError::Rejected(ApplyError::WriteToDestroyed(obj)))
    );
    assert!(matches!(
        s.try_push(EventKind::ObjectDestroyed { object: obj, reason: EndReason::Freed }),
        Err(TimelineError::Rejected(ApplyError::BadLifetime { .. }))
    ));
    assert_eq!(s.state().last_seq(), before);
    assert_eq!(s.timeline.len() as u64, s.seq);
}

#[test]
fn address_reuse_does_not_resurrect_a_dangling_pointer() {
    // delete p; q = new Node;  -- q may get p's old address, but it is a new object.
    let mut s = Script::new();
    declare_node_types(&mut s);
    enter(&mut s, 1, "main");
    let mut first = decl(10, NODE, StorageClass::Heap, node(1, Value::null_pointer()));
    first.address = Some(0x1000);
    s.push(EventKind::ObjectAllocated { object: first });
    let p = local(&mut s, 1, 1, "p", NODE_PTR, Value::pointer_to(ObjectId(10)));
    s.push(EventKind::ObjectDestroyed { object: ObjectId(10), reason: EndReason::Freed });
    let mut second = decl(11, NODE, StorageClass::Heap, node(2, Value::null_pointer()));
    second.address = Some(0x1000);
    s.push(EventKind::ObjectAllocated { object: second });

    let st = s.state();
    let target = ptr_target(&st, &Place::root(p));
    assert_eq!(target, Target::object(ObjectId(10)));
    assert_eq!(st.target_status(&target), TargetStatus::Dangling);
    assert_eq!(st.object(ObjectId(11)).unwrap().address, st.object(ObjectId(10)).unwrap().address);
    assert!(st.incoming(ObjectId(11)).is_empty());
}

#[test]
fn scope_and_frame_exit_end_automatic_storage() {
    // void f() { int a; { int b; } }   -- b ends at the inner brace, a at return
    let mut s = Script::new();
    s.push(int_type());
    enter(&mut s, 1, "f");
    let a = local(&mut s, 1, 1, "a", INT, Value::int(1));
    s.push(EventKind::ScopeEntered { scope: ScopeId(1), frame: FrameId(1) });
    alloc(&mut s, 2000, INT, StorageClass::Automatic, Value::int(2));
    s.push(EventKind::VariableCreated {
        variable: VariableDecl {
            id: VariableId(2),
            name: "b".into(),
            kind: VariableKind::Local,
            frame: Some(FrameId(1)),
            scope: Some(ScopeId(1)),
            object: ObjectId(2000),
        },
    });
    let b = ObjectId(2000);
    assert_eq!(s.state().frame_variables(FrameId(1)).count(), 2);

    // Scopes must close innermost-first; frames only from the top.
    s.push(EventKind::ScopeExited { scope: ScopeId(1) });
    let st = s.state();
    assert!(st.variable(VariableId(2)).is_none());
    assert_eq!(st.object(b).unwrap().lifetime.state, LifeState::Destroyed);
    assert_eq!(st.object(b).unwrap().lifetime.end_reason, Some(EndReason::ScopeExit));
    assert_eq!(st.object(a).unwrap().lifetime.state, LifeState::Alive);
    assert!(st.scope(ScopeId(1)).is_none());

    s.push(EventKind::FunctionExited { frame: FrameId(1) });
    let st = s.state();
    assert_eq!(st.object(a).unwrap().lifetime.end_reason, Some(EndReason::FrameExit));
    assert!(st.frame(FrameId(1)).is_none());
    assert!(st.stack(MAIN).is_empty());
}

#[test]
fn heap_objects_outlive_their_frame() {
    // Node* f() { return new Node{1}; }  -- the frame ends, the allocation does not
    let mut s = Script::new();
    declare_node_types(&mut s);
    enter(&mut s, 1, "f");
    alloc(&mut s, 10, NODE, StorageClass::Heap, node(1, Value::null_pointer()));
    local(&mut s, 1, 1, "tmp", NODE_PTR, Value::pointer_to(ObjectId(10)));
    s.push(EventKind::FunctionExited { frame: FrameId(1) });

    let st = s.state();
    assert_eq!(st.object(ObjectId(10)).unwrap().lifetime.state, LifeState::Alive);
    assert_eq!(st.object(ObjectId(1001)).unwrap().lifetime.state, LifeState::Destroyed);
}

#[test]
fn nested_scopes_inner_must_close_first() {
    let mut s = Script::new();
    enter(&mut s, 1, "f");
    s.push(EventKind::ScopeEntered { scope: ScopeId(1), frame: FrameId(1) });
    s.push(EventKind::ScopeEntered { scope: ScopeId(2), frame: FrameId(1) });
    assert_eq!(s.state().scope(ScopeId(2)).unwrap().parent, Some(ScopeId(1)));
    assert!(matches!(
        s.try_push(EventKind::ScopeExited { scope: ScopeId(1) }),
        Err(TimelineError::Rejected(ApplyError::NotInnermostScope(_)))
    ));
}

// ---- 12: stack frames -----------------------------------------------------

#[test]
fn stack_frames_main_foo_bar() {
    let mut s = Script::new();
    s.push(int_type());
    let at = |line| Some(SourceLocation::new("main.cpp", line).with_column(5));
    s.push_on(
        MAIN,
        at(10),
        EventKind::FunctionEntered { frame: FrameId(1), function: "main".into(), call_site: None },
    );
    s.push_on(
        MAIN,
        at(11),
        EventKind::FunctionEntered { frame: FrameId(2), function: "foo".into(), call_site: at(11) },
    );
    s.push_on(
        MAIN,
        at(3),
        EventKind::FunctionEntered { frame: FrameId(3), function: "bar".into(), call_site: at(4) },
    );
    let param = 500;
    alloc(&mut s, param, INT, StorageClass::Automatic, Value::int(1));
    s.push(EventKind::VariableCreated {
        variable: VariableDecl {
            id: VariableId(1),
            name: "n".into(),
            kind: VariableKind::Parameter,
            frame: Some(FrameId(3)),
            scope: None,
            object: ObjectId(param),
        },
    });

    let st = s.state();
    let names: Vec<_> = st.stack(MAIN).iter().map(|f| st.frame(*f).unwrap().function.to_string()).collect();
    assert_eq!(names, ["main", "foo", "bar"]);
    assert_eq!(st.frame(FrameId(1)).unwrap().caller, None);
    assert_eq!(st.frame(FrameId(2)).unwrap().caller, Some(FrameId(1)));
    assert_eq!(st.frame(FrameId(3)).unwrap().caller, Some(FrameId(2)));
    assert_eq!(st.top_frame(MAIN).unwrap().function.as_ref(), "bar");
    assert_eq!(st.frame(FrameId(3)).unwrap().call_site.as_ref().unwrap().line, 4);
    assert_eq!(st.frame(FrameId(3)).unwrap().location.as_ref().unwrap().line, 3);
    assert_eq!(st.frame_variables(FrameId(3)).next().unwrap().kind, VariableKind::Parameter);
    assert_eq!(st.frame_variables(FrameId(1)).count(), 0);

    // A statement executing in `bar` moves bar's location, not its callers'.
    set_at(&mut s, at(4), Place::root(ObjectId(param)), Value::int(2));
    let st = s.state();
    assert_eq!(st.frame(FrameId(3)).unwrap().location.as_ref().unwrap().line, 4);
    assert_eq!(st.frame(FrameId(2)).unwrap().location.as_ref().unwrap().line, 11);
    assert_eq!(st.last_location().unwrap().line, 4);

    // Only the top frame can exit.
    assert_eq!(
        s.try_push(EventKind::FunctionExited { frame: FrameId(1) }),
        Err(TimelineError::Rejected(ApplyError::NotTopFrame(FrameId(1))))
    );
    s.push(EventKind::FunctionExited { frame: FrameId(3) });
    let st = s.state();
    assert_eq!(st.stack(MAIN), [FrameId(1), FrameId(2)]);
    assert!(st.variable(VariableId(1)).is_none(), "bar's parameter left with bar");
    assert_eq!(st.top_frame(MAIN).unwrap().function.as_ref(), "foo");
}

fn set_at(s: &mut Script, loc: Option<SourceLocation>, place: Place, value: Value) {
    s.push_on(MAIN, loc, EventKind::ValueChanged { place, value });
}

#[test]
fn threads_have_independent_stacks() {
    let mut s = Script::new();
    let worker = ThreadId(7);
    enter(&mut s, 1, "main");
    s.push_on(
        worker,
        None,
        EventKind::FunctionEntered { frame: FrameId(2), function: "work".into(), call_site: None },
    );
    let st = s.state();
    assert_eq!(st.stack(MAIN), [FrameId(1)]);
    assert_eq!(st.stack(worker), [FrameId(2)]);
    assert_eq!(st.frame(FrameId(2)).unwrap().caller, None, "a thread's first frame has no caller");
    assert_eq!(st.threads().collect::<Vec<_>>(), [MAIN, worker]);

    s.push_on(worker, None, EventKind::FunctionExited { frame: FrameId(2) });
    assert_eq!(s.state().threads().collect::<Vec<_>>(), [MAIN]);
}

#[test]
fn globals_have_no_frame_and_survive_calls() {
    let mut s = Script::new();
    s.push(int_type());
    alloc(&mut s, 1, INT, StorageClass::Static, Value::int(0));
    s.push(EventKind::VariableCreated {
        variable: VariableDecl {
            id: VariableId(1),
            name: "counter".into(),
            kind: VariableKind::Global,
            frame: None,
            scope: None,
            object: ObjectId(1),
        },
    });
    enter(&mut s, 1, "main");
    s.push(EventKind::FunctionExited { frame: FrameId(1) });
    let st = s.state();
    assert_eq!(st.globals(), [VariableId(1)]);
    assert_eq!(st.object(ObjectId(1)).unwrap().lifetime.state, LifeState::Alive);

    // A local without a frame is malformed.
    assert!(matches!(
        s.try_push(EventKind::VariableCreated {
            variable: VariableDecl {
                id: VariableId(2),
                name: "x".into(),
                kind: VariableKind::Local,
                frame: None,
                scope: None,
                object: ObjectId(1),
            }
        }),
        Err(TimelineError::Rejected(ApplyError::InvalidVariable(..)))
    ));
}

// ---- references -----------------------------------------------------------

#[test]
fn references_are_links_distinct_from_pointers() {
    // int x = 1; int& r = x;
    let mut s = Script::new();
    s.push(int_type());
    let ref_ty = TypeId(60);
    s.push(EventKind::TypeDeclared {
        def: TypeDef::new(ref_ty, "int&", TypeKind::LValueReference { referent: INT }),
    });
    enter(&mut s, 1, "main");
    let x = local(&mut s, 1, 1, "x", INT, Value::int(1));
    let r = local(&mut s, 1, 2, "r", ref_ty, Value::reference_to(Place::root(x)));

    let st = s.state();
    let e = &st.outgoing(r)[0];
    assert_eq!(e.kind, EdgeKind::Reference);
    assert_eq!(e.target, Place::root(x));
    assert_eq!(st.types().pointee(ref_ty), Some(INT));
    assert_eq!(st.incoming(x)[0].kind, EdgeKind::Reference);
}

#[test]
fn pointer_to_pointer() {
    // Node* p = &n; Node** pp = &p;
    let mut s = Script::new();
    declare_node_types(&mut s);
    enter(&mut s, 1, "main");
    let n = local(&mut s, 1, 1, "n", NODE, node(1, Value::null_pointer()));
    let p = local(&mut s, 1, 2, "p", NODE_PTR, Value::pointer_to(n));
    s.push(EventKind::TypeDeclared {
        def: TypeDef::new(TypeId(61), "Node**", TypeKind::Pointer { pointee: NODE_PTR }),
    });
    let pp = local(&mut s, 1, 3, "pp", TypeId(61), Value::pointer_to(p));
    assert_eq!(reachable(&s.state(), pp), [pp, p, n]);
}

// ---- 13: events -> snapshots ---------------------------------------------

/// Build a short but varied run and return its events.
fn build_run(interval: usize) -> Script {
    let mut s = Script::with_interval(interval);
    declare_node_types(&mut s);
    enter(&mut s, 1, "main");
    alloc(&mut s, 10, NODE, StorageClass::Heap, node(1, Value::null_pointer()));
    let head = local(&mut s, 1, 1, "head", NODE_PTR, Value::pointer_to(ObjectId(10)));
    alloc(&mut s, 11, NODE, StorageClass::Heap, node(2, Value::null_pointer()));
    set(&mut s, Place::root(ObjectId(10)).field(1), Value::pointer_to(ObjectId(11)));
    set(&mut s, Place::root(ObjectId(11)).field(0), Value::int(22));
    alloc(&mut s, 12, NODE, StorageClass::Heap, node(3, Value::null_pointer()));
    set(&mut s, Place::root(ObjectId(11)).field(1), Value::pointer_to(ObjectId(12)));
    set(&mut s, Place::root(head), Value::null_pointer());
    s.push(EventKind::ObjectDestroyed { object: ObjectId(12), reason: EndReason::Freed });
    s.push(EventKind::FunctionExited { frame: FrameId(1) });
    s
}

fn fingerprint(st: &RuntimeState) -> String {
    let objects: Vec<_> = st.objects().collect();
    let vars: Vec<_> = st.variables().collect();
    let frames: Vec<_> = st.stack(MAIN).iter().map(|f| st.frame(*f)).collect();
    let inc: Vec<_> = st.objects().map(|o| st.incoming(o.id)).collect();
    format!("{:?}|{:?}|{:?}|{:?}|{:?}", st.last_seq(), objects, vars, frames, inc)
}

#[test]
fn events_reconstruct_state_at_every_step() {
    // Every checkpoint interval, including ones that do not divide the length.
    for interval in [1, 2, 3, 5, 100] {
        let s = build_run(interval);
        let events = s.timeline.events().to_vec();
        assert_eq!(events.len(), s.timeline.len());
        let mut reference = RuntimeState::new();
        for (n, e) in events.iter().enumerate() {
            reference.apply(e).unwrap();
            let snap = s.timeline.snapshot_after(EventSeq(n as u64)).unwrap();
            assert_eq!(snap.sequence(), Some(EventSeq(n as u64)));
            assert_eq!(fingerprint(&snap), fingerprint(&reference), "interval {interval}, step {n}");
            snap.verify_invariants().unwrap();
        }
        assert_eq!(fingerprint(&s.state()), fingerprint(&reference));
    }
}

#[test]
fn snapshots_show_the_structure_evolving() {
    let s = build_run(3);
    let find = |pred: &dyn Fn(&RuntimeSnapshot) -> bool| {
        (0..s.timeline.len() as u64)
            .find(|n| pred(&s.timeline.snapshot_after(EventSeq(*n)).unwrap()))
            .expect("state never reached")
    };
    let a = ObjectId(10);
    let linked = find(&|st| st.outgoing(a).first().map(|e| e.target.object) == Some(ObjectId(11)));
    let grown = find(&|st| reachable(st, a).len() == 3);
    let freed = find(&|st| st.object(ObjectId(12)).is_some_and(|o| o.is_destroyed()));
    assert!(linked < grown && grown < freed);

    // Before the link existed there was none.
    let before = s.timeline.snapshot_after(EventSeq(linked - 1)).unwrap();
    assert!(before.outgoing(a).iter().all(|e| e.target.object != ObjectId(11)));
    // At `freed`, the tail is dangling; the snapshot just before it is not.
    let at = s.timeline.snapshot_after(EventSeq(freed)).unwrap();
    let edge = at.outgoing(ObjectId(11)).pop().unwrap();
    assert_eq!(at.target_status(&Target::to(edge.target.clone())), TargetStatus::Dangling);
    let prev = s.timeline.snapshot_after(EventSeq(freed - 1)).unwrap();
    assert_eq!(prev.target_status(&Target::to(edge.target)), TargetStatus::Live);
}

#[test]
fn snapshots_are_immutable_and_initial_state_is_empty() {
    let mut s = Script::new();
    s.push(int_type());
    enter(&mut s, 1, "main");
    let x = local(&mut s, 1, 1, "x", INT, Value::int(1));
    let snap = s.state();
    set(&mut s, Place::root(x), Value::int(2));

    assert_eq!(snap.value_at(&Place::root(x)), Some(&Value::int(1)), "older snapshot unaffected");
    assert_eq!(s.state().value_at(&Place::root(x)), Some(&Value::int(2)));
    let init = s.timeline.initial();
    assert_eq!(init.sequence(), None);
    assert_eq!(init.object_count(), 0);
}

#[test]
fn timeline_rejects_bad_sequence_and_unknown_steps() {
    let mut s = Script::new();
    s.push(int_type());
    let skipped = RuntimeEvent { seq: EventSeq(5), thread: MAIN, timestamp_ns: None, location: None, kind: int_type() };
    assert_eq!(
        s.timeline.push(skipped),
        Err(TimelineError::NonContiguous { expected: EventSeq(1), got: EventSeq(5) })
    );
    assert!(matches!(s.timeline.snapshot_after(EventSeq(1)), Err(TimelineError::OutOfRange { .. })));
    assert_eq!(s.timeline.len(), 1);

    // Direct state application also enforces strictly increasing order.
    let mut st = RuntimeState::new();
    let e0 = RuntimeEvent { seq: EventSeq(3), thread: MAIN, timestamp_ns: None, location: None, kind: int_type() };
    st.apply(&e0).unwrap();
    let e1 = RuntimeEvent { seq: EventSeq(3), ..e0.clone() };
    assert!(matches!(st.apply(&e1), Err(ApplyError::OutOfOrder { .. })));
}

#[test]
fn rejected_events_leave_no_trace() {
    let mut s = Script::new();
    declare_node_types(&mut s);
    enter(&mut s, 1, "main");
    let n = local(&mut s, 1, 1, "n", NODE, node(1, Value::null_pointer()));
    let before = fingerprint(&s.state());

    let bad = [
        // pointer to an object that was never reported
        EventKind::ValueChanged { place: Place::root(n).field(1), value: Value::pointer_to(ObjectId(999)) },
        // path that does not exist
        EventKind::ValueChanged { place: Place::root(n).field(9), value: Value::int(0) },
        EventKind::ValueChanged { place: Place::root(ObjectId(999)), value: Value::int(0) },
        // unknown type
        EventKind::ObjectAllocated { object: decl(50, TypeId(777), StorageClass::Heap, Value::int(0)) },
        // duplicate object
        EventKind::ObjectAllocated { object: decl(n.0, INT, StorageClass::Heap, Value::int(0)) },
        // duplicate frame
        EventKind::FunctionEntered { frame: FrameId(1), function: "again".into(), call_site: None },
        // unknown references
        EventKind::ScopeEntered { scope: ScopeId(1), frame: FrameId(42) },
        EventKind::ScopeExited { scope: ScopeId(42) },
        EventKind::VariableDestroyed { variable: VariableId(42) },
        EventKind::ObjectDestroyed { object: ObjectId(42), reason: EndReason::Freed },
        EventKind::FunctionExited { frame: FrameId(42) },
        // conflicting redefinition of a type
        EventKind::TypeDeclared { def: TypeDef::new(INT, "not int", TypeKind::Void) },
    ];
    for kind in bad {
        assert!(s.try_push(kind.clone()).is_err(), "should reject {kind:?}");
        assert_eq!(fingerprint(&s.state()), before);
    }
    // Re-declaring an identical type is harmless.
    s.push(int_type());
}

// ---- 14: unavailable values ----------------------------------------------

#[test]
fn unavailable_is_distinct_from_every_real_value() {
    let reasons = [
        Unavailable::Unknown,
        Unavailable::Uninitialized,
        Unavailable::OptimizedAway,
        Unavailable::Unreadable,
        Unavailable::OutOfScope,
        Unavailable::Invalid { raw: Some("0x07".into()) },
        Unavailable::Invalid { raw: None },
    ];
    let real = [
        Value::int(0),
        Value::uint(0),
        Value::boolean(false),
        Value::Char { value: 0 },
        Value::String { value: String::new() },
        Value::null_pointer(),
        Value::aggregate(vec![]),
        Value::array(vec![]),
        Value::float(0.0),
    ];
    for (i, r) in reasons.iter().enumerate() {
        let v = Value::unavailable(r.clone());
        assert!(v.is_unavailable());
        for other in &real {
            assert!(!other.is_unavailable());
            assert_ne!(&v, other);
        }
        for (j, r2) in reasons.iter().enumerate() {
            assert_eq!(i == j, r == r2, "reasons are pairwise distinct");
        }
    }

    // In a live state: an optimized-away field next to a real one.
    let mut s = Script::new();
    declare_node_types(&mut s);
    enter(&mut s, 1, "main");
    let n = local(&mut s, 1, 1, "n", NODE, node(0, Value::null_pointer()));
    set(&mut s, Place::root(n).field(0), Value::unavailable(Unavailable::OptimizedAway));
    let st = s.state();
    let field = st.value_at(&Place::root(n).field(0)).unwrap();
    assert!(field.is_unavailable());
    assert_ne!(field, &Value::int(0), "0 and optimized-away are different facts");
    assert!(!st.value_at(&Place::root(n).field(1)).unwrap().is_unavailable());
    // An unavailable pointer is not a null pointer and creates no link.
    set(&mut s, Place::root(n).field(1), Value::unavailable(Unavailable::Unreadable));
    assert!(s.state().outgoing(n).is_empty());
}

// ---- types ----------------------------------------------------------------

#[test]
fn types_cover_templates_functions_enums_and_recursion() {
    let mut s = Script::new();
    declare_node_types(&mut s);
    let vec_ty = TypeId(70);
    s.push(EventKind::TypeDeclared {
        def: TypeDef::new(
            vec_ty,
            "std::vector<int>",
            TypeKind::Record {
                record: RecordKind::Class,
                fields: vec![],
                template_args: vec![TemplateArg::Type(INT), TemplateArg::Other("std::allocator<int>".into())],
            },
        ),
    });
    s.push(EventKind::TypeDeclared {
        def: TypeDef::new(
            TypeId(71),
            "int (*)(int, ...)",
            TypeKind::Function { ret: INT, params: vec![INT], variadic: true },
        ),
    });
    s.push(EventKind::TypeDeclared {
        def: TypeDef::new(
            TypeId(72),
            "Color",
            TypeKind::Enum {
                underlying: Some(INT),
                enumerators: vec![Enumerator { name: "Red".into(), value: 0 }],
                scoped: true,
            },
        ),
    });
    s.push(EventKind::TypeDeclared { def: TypeDef::new(TypeId(73), "Fwd", TypeKind::Opaque) });

    let st = s.state();
    assert_eq!(st.types().len(), 7);
    // Recursive: Node -> Node* -> Node.
    assert_eq!(st.types().pointee(st.types().field(NODE, 1).unwrap().ty), Some(NODE));
    assert_eq!(st.types().name(vec_ty), Some("std::vector<int>"));
    assert!(st.types().fields(INT).is_none());
}

// ---- serialization --------------------------------------------------------

#[test]
fn events_round_trip_through_json() {
    let s = build_run(4);
    let loc = SourceLocation::new("main.cpp", 12).with_column(5).with_function("main");
    let mut events = s.timeline.events().to_vec();
    events.push(RuntimeEvent {
        seq: EventSeq(events.len() as u64),
        thread: ThreadId(3),
        timestamp_ns: Some(123),
        location: Some(loc),
        kind: EventKind::ValueChanged {
            place: Place::root(ObjectId(1)).field(2).index(3),
            value: Value::Union { active: Some(1), value: Box::new(Value::Enum { value: 2, enumerator: Some("B".into()) }) },
        },
    });
    events.push(RuntimeEvent {
        seq: EventSeq(events.len() as u64),
        thread: MAIN,
        timestamp_ns: None,
        location: None,
        kind: EventKind::ValueChanged {
            place: Place::root(ObjectId(1)),
            value: Value::array(vec![
                Value::unavailable(Unavailable::Invalid { raw: Some("x".into()) }),
                Value::Reference { target: Target::Unresolved },
                Value::Pointer { pointer: PointerValue { address: Some(0xdead), target: Target::Null } },
                Value::Float { value: 1.5 },
            ]),
        },
    });

    let json = serde_json::to_string(&events).unwrap();
    let back: Vec<RuntimeEvent> = serde_json::from_str(&json).unwrap();
    assert_eq!(back, events);

    // The wire form is self-describing (no Rust-specific layout).
    let one = serde_json::to_value(&events[events.len() - 1]).unwrap();
    assert_eq!(one["kind"]["event"], "value_changed");

    // Replaying the deserialized stream gives the same state as the original.
    let mut a = RuntimeState::new();
    let mut b = RuntimeState::new();
    for e in &events[..s.timeline.len()] {
        a.apply(e).unwrap();
    }
    for e in &back[..s.timeline.len()] {
        b.apply(e).unwrap();
    }
    assert_eq!(fingerprint(&a), fingerprint(&b));
}

#[test]
fn source_locations_share_their_strings() {
    let file: Arc<str> = "main.cpp".into();
    let a = SourceLocation::new(file.clone(), 1);
    let b = SourceLocation::new(file.clone(), 2);
    assert!(Arc::ptr_eq(&a.file, &b.file));
}

// ---- scale ----------------------------------------------------------------

#[test]
fn many_objects_keep_queries_local() {
    // A 20_000-node chain: building and querying must stay linear, and
    // "who points at X" must not scan the world.
    let mut s = Script::with_interval(4096);
    s.push(int_type());
    s.push(EventKind::TypeDeclared {
        def: TypeDef::new(
            NODE,
            "Node",
            TypeKind::Record {
                record: RecordKind::Struct,
                fields: vec![FieldDecl::new("value", INT), FieldDecl::new("next", NODE_PTR)],
                template_args: vec![],
            },
        ),
    });
    s.push(EventKind::TypeDeclared {
        def: TypeDef::new(NODE_PTR, "Node*", TypeKind::Pointer { pointee: NODE }),
    });
    let n = 20_000u64;
    for i in 0..n {
        let next_ptr = if i == 0 { Value::null_pointer() } else { Value::pointer_to(ObjectId(i - 1)) };
        s.try_push(EventKind::ObjectAllocated {
            object: decl(i, NODE, StorageClass::Heap, node(i as i64, next_ptr)),
        })
        .unwrap();
    }
    let state = s.state();
    assert_eq!(state.object_count() as u64, n);
    assert_eq!(state.incoming(ObjectId(n / 2)).len(), 1);
    assert_eq!(reachable(&state, ObjectId(n - 1)).len() as u64, n);
    state.verify_invariants().unwrap();
}

#[test]
fn deeply_nested_values_do_not_overflow_the_stack() {
    let mut v = Value::pointer_to(ObjectId(1));
    for _ in 0..2_000 {
        v = Value::aggregate(vec![Value::int(0), v]);
    }
    let mut links = 0;
    v.for_each_link(|path, kind, _| {
        links += 1;
        assert_eq!(path.len(), 2_000);
        assert_eq!(kind, EdgeKind::Pointer);
    });
    assert_eq!(links, 1);
    // `v` is dropped here; the nesting is bounded by the observer's own depth.
}

// ---- architecture guard ---------------------------------------------------

#[test]
fn model_is_independent_of_the_rest_of_the_application() {
    // The model may only use std, serde and itself. No UI, process, toolchain,
    // runtime-session or framework types, and no notion of rendering.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("model");
    let forbidden = [
        "tauri", "crate::app", "crate::lsp", "crate::process", "crate::toolchain",
        "crate::runtime", "crate::config", "super::super", "windows_sys", "std::process",
        "std::fs", "std::net", "unsafe",
    ];
    let mut checked = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "rs") {
            // Only code: strip comment lines so docs may mention these words.
            let code: String = std::fs::read_to_string(&path)
                .unwrap()
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("
");
            for word in forbidden {
                assert!(!code.contains(word), "{} must not contain `{word}`", path.display());
            }
            checked += 1;
        }
    }
    assert!(checked >= 8);
}

#[test]
fn checkpoint_cost_is_amortized_and_snapshots_stay_exact() {
    // 20_000 objects then 20_000 writes, with a tiny minimum interval. A fixed
    // interval would take ~2_500 full copies of a growing state; the adaptive
    // policy spaces copies at least one state size apart.
    let mut s = Script::with_interval(16);
    s.push(int_type());
    s.push(EventKind::TypeDeclared {
        def: TypeDef::new(
            NODE,
            "Node",
            TypeKind::Record {
                record: RecordKind::Struct,
                fields: vec![FieldDecl::new("value", INT), FieldDecl::new("next", NODE_PTR)],
                template_args: vec![],
            },
        ),
    });
    s.push(EventKind::TypeDeclared {
        def: TypeDef::new(NODE_PTR, "Node*", TypeKind::Pointer { pointee: NODE }),
    });
    let n = 20_000u64;
    for i in 0..n {
        s.try_push(EventKind::ObjectAllocated {
            object: decl(i, NODE, StorageClass::Heap, node(0, Value::null_pointer())),
        })
        .unwrap();
    }
    for i in 0..n {
        s.try_push(EventKind::ValueChanged {
            place: Place::root(ObjectId(i)).field(0),
            value: Value::int(i as i64),
        })
        .unwrap();
    }
    assert!(s.timeline.checkpoint_count() < 64, "{} checkpoints", s.timeline.checkpoint_count());

    // Spot-check snapshots across the run against a from-scratch replay.
    let events = s.timeline.events().to_vec();
    for probe in [0usize, 5, 1_000, 19_999, 20_002, 30_000, events.len() - 1] {
        let mut reference = RuntimeState::new();
        for e in &events[..=probe] {
            reference.apply(e).unwrap();
        }
        let snap = s.timeline.snapshot_after(EventSeq(probe as u64)).unwrap();
        assert_eq!(snap.sequence(), reference.last_seq());
        assert_eq!(snap.object_count(), reference.object_count());
        let sample = ObjectId(((probe as u64) * 7919) % n);
        assert_eq!(
            snap.object(sample).map(|o| o.value.clone()),
            reference.object(sample).map(|o| o.value.clone()),
            "at event {probe}"
        );
    }
}

#[test]
fn truncation_marks_where_the_record_ends() {
    let mut s = Script::new();
    s.push(int_type());
    enter(&mut s, 1, "main");
    let x = local(&mut s, 1, 1, "x", INT, Value::int(1));
    assert_eq!(s.state().truncated_at(), None);

    s.push(EventKind::ObservationTruncated { limit: 4 });
    let st = s.state();
    assert_eq!(st.truncated_at(), Some(EventSeq(s.seq - 1)));
    // Nothing is cleaned up: the frame and variable are still there. The state is
    // "as far as we know", not "the end of the program".
    assert_eq!(st.stack(MAIN).len(), 1);
    assert_eq!(st.variable_value(VariableId(1)), Some(&Value::int(1)));
    assert!(st.object(x).is_some_and(|o| !o.is_destroyed()));

    // Earlier snapshots are not truncated; the flag survives later queries and replay.
    let before = s.timeline.snapshot_after(EventSeq(s.seq - 2)).unwrap();
    assert_eq!(before.truncated_at(), None);
    assert_eq!(s.timeline.snapshot_after(EventSeq(s.seq - 1)).unwrap().truncated_at(), st.truncated_at());

    // The marker survives the wire.
    let e = s.timeline.events().last().unwrap().clone();
    let back: RuntimeEvent = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
    assert_eq!(back, e);
    assert_eq!(serde_json::to_value(&e).unwrap()["kind"]["event"], "observation_truncated");
}

#[test]
#[ignore]
fn size_probe() {
    println!("RuntimeEvent: {} bytes", std::mem::size_of::<RuntimeEvent>());
    println!("EventKind:    {} bytes", std::mem::size_of::<EventKind>());
    println!("Value:        {} bytes", std::mem::size_of::<Value>());
    println!("Object:       {} bytes", std::mem::size_of::<Object>());
    println!("SourceLocation: {} bytes", std::mem::size_of::<SourceLocation>());
}
