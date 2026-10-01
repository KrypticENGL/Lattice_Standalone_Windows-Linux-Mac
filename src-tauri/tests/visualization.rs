//! The visualization view model (`viz`): what a renderer is given for one step.
//! Edge cases are driven by hand-built model state (fixtures, not application
//! behaviour); one test uses a real observed run end to end.

mod common;

use std::sync::Arc;

use lattice_lib::model::*;
use lattice_lib::observe::discovery::find_libclang;
use lattice_lib::observe::{graph_at, ObservationService};
use lattice_lib::runtime::{ExecutionManager, RunRequest};
use lattice_lib::viz::{self, GraphView, SlotView, TargetView, ValueView, MAX_ARRAY_ELEMENTS, MAX_OBJECTS};

// ---- fixtures ---------------------------------------------------------------

const INT: TypeId = TypeId(1);
const NODE: TypeId = TypeId(2);
const NODE_PTR: TypeId = TypeId(3);
const ARR: TypeId = TypeId(4);

struct World {
    state: RuntimeState,
    seq: u64,
}

impl World {
    fn new() -> Self {
        let mut w = World { state: RuntimeState::new(), seq: 0 };
        w.push(EventKind::TypeDeclared {
            def: TypeDef::new(INT, "int", TypeKind::Primitive { primitive: Primitive::Int }).with_size(4),
        });
        w.push(EventKind::TypeDeclared {
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
        w.push(EventKind::TypeDeclared {
            def: TypeDef::new(NODE_PTR, "Node*", TypeKind::Pointer { pointee: NODE }),
        });
        w.push(EventKind::TypeDeclared {
            def: TypeDef::new(ARR, "int[100]", TypeKind::Array { element: INT, len: Some(100) }),
        });
        w
    }

    fn event(&mut self, kind: EventKind) -> RuntimeEvent {
        let e = RuntimeEvent {
            seq: EventSeq(self.seq),
            thread: ThreadId::MAIN,
            timestamp_ns: None,
            location: Some(SourceLocation::new("main.cpp", 10 + self.seq as u32)),
            kind,
        };
        self.seq += 1;
        e
    }

    fn push(&mut self, kind: EventKind) -> RuntimeEvent {
        let e = self.event(kind);
        self.state.apply(&e).expect("event rejected");
        e
    }

    fn alloc(&mut self, id: u64, ty: TypeId, storage: StorageClass, value: Value) -> RuntimeEvent {
        self.push(EventKind::ObjectAllocated {
            object: ObjectDecl {
                id: ObjectId(id),
                ty,
                storage,
                address: Some(0x1000 + id * 16),
                size: None,
                state: LifeState::Alive,
                value,
            },
        })
    }

    fn node(&mut self, id: u64, value: i64, next: Value) {
        self.alloc(id, NODE, StorageClass::Heap, Value::aggregate(vec![Value::int(value), next]));
    }

    fn main_frame(&mut self) {
        self.push(EventKind::FunctionEntered { frame: FrameId(1), function: "main".into(), call_site: None });
    }

    /// A local pointer variable `name` (object `id`) in main.
    fn local_ptr(&mut self, id: u64, var: u64, name: &str, value: Value) {
        self.alloc(id, NODE_PTR, StorageClass::Automatic, value);
        self.push(EventKind::VariableCreated {
            variable: VariableDecl {
                id: VariableId(var),
                name: name.into(),
                kind: VariableKind::Local,
                frame: Some(FrameId(1)),
                scope: None,
                object: ObjectId(id),
            },
        });
    }

    fn set(&mut self, place: Place, value: Value) -> RuntimeEvent {
        self.push(EventKind::ValueChanged { place, value })
    }

    fn graph(&self) -> GraphView {
        viz::build(&self.state, None, None, self.seq, self.seq)
    }
}

fn pointer_target(slot: &SlotView) -> &TargetView {
    match &slot.value {
        ValueView::Pointer { target, .. } => target,
        other => panic!("not a pointer: {other:?}"),
    }
}

fn field<'a>(slot: &'a SlotView, name: &str) -> &'a SlotView {
    match &slot.value {
        ValueView::Aggregate { fields } => fields.iter().find(|f| f.name.as_deref() == Some(name)).expect("field"),
        other => panic!("not an aggregate: {other:?}"),
    }
}

fn scalar(slot: &SlotView) -> &str {
    match &slot.value {
        ValueView::Scalar { text } => text,
        other => panic!("not a scalar: {other:?}"),
    }
}

// ---- tests --------------------------------------------------------------------

#[test]
fn a_linked_structure_is_just_slots_and_pointers() {
    // head -> A -> B -> C -> A  (a cycle). Nothing here knows "list" or "ring".
    let mut w = World::new();
    w.main_frame();
    for (id, v) in [(10, 1), (11, 2), (12, 3)] {
        w.node(id, v, Value::null_pointer());
    }
    for (from, to) in [(10, 11), (11, 12), (12, 10)] {
        w.set(Place::root(ObjectId(from)).field(1), Value::pointer_to(ObjectId(to)));
    }
    w.local_ptr(1, 1, "head", Value::pointer_to(ObjectId(10)));
    let g = w.graph();

    // The stack: one frame with one variable whose value points at A.
    assert_eq!(g.threads.len(), 1);
    let frame = &g.threads[0].frames[0];
    assert_eq!(frame.function, "main");
    let head = &frame.variables[0];
    assert_eq!((head.name.as_str(), head.in_block, head.object), ("head", false, 1));
    assert_eq!(head.slot.ty, "Node*");
    assert_eq!(
        pointer_target(&head.slot),
        &TargetView { kind: "object", object: Some(10), path: String::new(), dangling: false }
    );

    // The heap: three objects, each a record of `value` and `next`.
    assert_eq!(g.objects.iter().map(|o| o.id).collect::<Vec<_>>(), [10, 11, 12]);
    assert!(g.objects.iter().all(|o| o.state == "alive" && o.storage == "heap"));
    let a = &g.objects[0];
    assert_eq!(a.slot.ty, "Node");
    assert_eq!(scalar(field(&a.slot, "value")), "1");
    assert_eq!(field(&a.slot, "value").ty, "int");
    assert_eq!(pointer_target(field(&a.slot, "next")).object, Some(11));
    assert_eq!(pointer_target(field(&g.objects[2].slot, "next")).object, Some(10), "the cycle closes");

    // The stack variable's own storage is *not* listed as a heap object.
    assert!(g.objects.iter().all(|o| o.id != 1));
}

#[test]
fn dangling_targets_stay_visible_only_while_something_points_at_them() {
    let mut w = World::new();
    w.main_frame();
    w.node(10, 1, Value::null_pointer());
    w.node(11, 2, Value::null_pointer());
    w.node(12, 3, Value::null_pointer()); // C, unreferenced
    w.set(Place::root(ObjectId(10)).field(1), Value::pointer_to(ObjectId(11))); // A -> B
    w.local_ptr(1, 1, "head", Value::pointer_to(ObjectId(10)));
    w.push(EventKind::ObjectDestroyed { object: ObjectId(11), reason: EndReason::Freed });
    w.push(EventKind::ObjectDestroyed { object: ObjectId(12), reason: EndReason::Freed });
    let g = w.graph();

    let ids: Vec<u64> = g.objects.iter().map(|o| o.id).collect();
    assert_eq!(ids, [10, 11], "freed C is gone; freed B stays because A.next still points at it");
    let b = g.objects.iter().find(|o| o.id == 11).unwrap();
    assert_eq!(b.state, "destroyed");
    let link = pointer_target(field(&g.objects[0].slot, "next"));
    assert_eq!((link.object, link.dangling), (Some(11), true));
}

#[test]
fn interior_pointers_name_the_part_they_point_at() {
    // p = &node.next (field 1 of object 10)
    let mut w = World::new();
    w.main_frame();
    w.node(10, 1, Value::null_pointer());
    w.local_ptr(1, 1, "p", Value::pointer_to_place(Place::root(ObjectId(10)).field(1)));
    let g = w.graph();
    let t = pointer_target(&g.threads[0].frames[0].variables[0].slot);
    assert_eq!((t.object, t.path.as_str()), (Some(10), ".1"));
}

#[test]
fn null_and_untracked_pointers_are_distinct_from_object_pointers() {
    let mut w = World::new();
    w.main_frame();
    w.local_ptr(1, 1, "n", Value::null_pointer());
    w.local_ptr(2, 2, "u", Value::Pointer { pointer: PointerValue { address: Some(0xdead), target: Target::Unresolved } });
    let vars = &w.graph().threads[0].frames[0].variables;
    assert_eq!(pointer_target(&vars[0].slot).kind, "null");
    assert_eq!(pointer_target(&vars[1].slot).kind, "unresolved");
    assert!(pointer_target(&vars[1].slot).object.is_none());
}

#[test]
fn arrays_are_capped_and_say_how_much_was_left_out() {
    let mut w = World::new();
    w.main_frame();
    let elems: Vec<Value> = (0..100).map(Value::int).collect();
    w.alloc(5, ARR, StorageClass::Heap, Value::array(elems));
    let g = w.graph();
    match &g.objects[0].slot.value {
        ValueView::Array { elements, omitted } => {
            assert_eq!(elements.len(), MAX_ARRAY_ELEMENTS);
            assert_eq!(*omitted, 100 - MAX_ARRAY_ELEMENTS as u32);
            assert_eq!(elements[3].name.as_deref(), Some("[3]"));
            assert_eq!(elements[3].ty, "int");
            assert_eq!(scalar(&elements[3]), "3");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn values_are_formatted_and_absence_is_explained() {
    let mut w = World::new();
    w.main_frame();
    let ty = |n: u64, name: &str, p: Primitive| {
        EventKind::TypeDeclared { def: TypeDef::new(TypeId(n), name, TypeKind::Primitive { primitive: p }) }
    };
    w.push(ty(20, "double", Primitive::Double));
    w.push(ty(21, "char", Primitive::Char));
    w.push(ty(22, "bool", Primitive::Bool));
    for (id, ty, v) in [
        (30, TypeId(20), Value::float(1.0)),
        (31, TypeId(20), Value::float(2.5)),
        (32, TypeId(21), Value::Char { value: 'a' as u32 }),
        (33, TypeId(21), Value::Char { value: 10 }),
        (34, TypeId(22), Value::boolean(true)),
        (35, INT, Value::unavailable(Unavailable::Uninitialized)),
        (36, INT, Value::unavailable(Unavailable::OptimizedAway)),
        (37, INT, Value::unavailable(Unavailable::Invalid { raw: Some("7".into()) })),
    ] {
        w.alloc(id, ty, StorageClass::Heap, v);
    }
    let g = w.graph();
    let slot = |id: u64| &g.objects.iter().find(|o| o.id == id).unwrap().slot;
    assert_eq!(scalar(slot(30)), "1.0");
    assert_eq!(scalar(slot(31)), "2.5");
    assert_eq!(scalar(slot(32)), "'a'");
    assert_eq!(scalar(slot(33)), "'\\n'");
    assert_eq!(scalar(slot(34)), "true");
    for (id, reason) in [(35, "uninitialized"), (36, "optimized away"), (37, "invalid (7)")] {
        assert_eq!(slot(id).value, ValueView::Unavailable { reason: reason.into() });
    }
}

#[test]
fn the_view_says_what_changed_at_this_step() {
    let mut w = World::new();
    let e = w.push(EventKind::FunctionEntered { frame: FrameId(1), function: "main".into(), call_site: None });
    assert!(viz::build(&w.state, Some(&e), None, 1, 1).changed.is_empty());

    let e = w.alloc(10, NODE, StorageClass::Heap, Value::aggregate(vec![Value::int(1), Value::null_pointer()]));
    assert_eq!(viz::build(&w.state, Some(&e), None, 2, 2).changed, ["10"]);

    // A write to a field names the field: object 10, field 1.
    let e = w.set(Place::root(ObjectId(10)).field(1), Value::pointer_to(ObjectId(10)));
    let g = viz::build(&w.state, Some(&e), Some("wrote next".into()), 3, 3);
    assert_eq!(g.changed, ["10.1"]);
    assert_eq!(g.event.as_deref(), Some("wrote next"));
    assert_eq!(g.line, Some(e.location.as_ref().unwrap().line), "the editor can highlight this line");

    let e = w.push(EventKind::ObjectDestroyed { object: ObjectId(10), reason: EndReason::Freed });
    assert_eq!(viz::build(&w.state, Some(&e), None, 4, 4).changed, ["10"]);
}

#[test]
fn too_many_objects_are_counted_not_dropped_silently() {
    let mut w = World::new();
    w.main_frame();
    for i in 0..(MAX_OBJECTS as u64 + 5) {
        w.node(100 + i, i as i64, Value::null_pointer());
    }
    let g = w.graph();
    assert_eq!(g.objects.len(), MAX_OBJECTS);
    assert_eq!(g.objects_omitted, 5);
}

#[test]
fn nested_block_variables_and_globals_are_reported() {
    let mut w = World::new();
    w.alloc(1, INT, StorageClass::Static, Value::int(7));
    w.push(EventKind::VariableCreated {
        variable: VariableDecl {
            id: VariableId(1),
            name: "counter".into(),
            kind: VariableKind::Global,
            frame: None,
            scope: None,
            object: ObjectId(1),
        },
    });
    w.main_frame();
    w.push(EventKind::ScopeEntered { scope: ScopeId(1), frame: FrameId(1) });
    w.alloc(2, INT, StorageClass::Automatic, Value::int(1));
    w.push(EventKind::VariableCreated {
        variable: VariableDecl {
            id: VariableId(2),
            name: "i".into(),
            kind: VariableKind::Local,
            frame: Some(FrameId(1)),
            scope: Some(ScopeId(1)),
            object: ObjectId(2),
        },
    });
    let g = w.graph();
    assert_eq!(g.globals.len(), 1);
    assert_eq!((g.globals[0].name.as_str(), scalar(&g.globals[0].slot)), ("counter", "7"));
    let i = &g.threads[0].frames[0].variables[0];
    assert!(i.in_block, "declared in a nested block");
    assert!(g.objects.is_empty(), "named storage is shown as a variable, not as an object");
}

#[test]
fn the_end_of_a_truncated_recording_is_flagged() {
    let mut w = World::new();
    w.main_frame();
    assert!(!w.graph().truncated_here);
    w.push(EventKind::ObservationTruncated { limit: 5 });
    assert!(w.graph().truncated_here);
    w.state = {
        // Later states of a replay are not "here".
        let mut s = w.state.clone();
        let e = RuntimeEvent {
            seq: EventSeq(w.seq),
            thread: ThreadId::MAIN,
            timestamp_ns: None,
            location: None,
            kind: EventKind::TypeDeclared { def: TypeDef::new(TypeId(99), "x", TypeKind::Void) },
        };
        s.apply(&e).unwrap();
        s
    };
    assert!(!w.graph().truncated_here);
}

#[test]
fn the_view_serializes_for_the_ui_in_a_stable_shape() {
    let mut w = World::new();
    w.main_frame();
    w.node(10, 1, Value::null_pointer());
    w.local_ptr(1, 1, "head", Value::pointer_to(ObjectId(10)));
    let json = serde_json::to_value(w.graph()).unwrap();
    assert!(json.get("truncatedHere").is_some() && json.get("objectsOmitted").is_some());
    let head = &json["threads"][0]["frames"][0]["variables"][0];
    assert_eq!(head["slot"]["value"]["kind"], "pointer");
    assert_eq!(head["slot"]["value"]["target"]["kind"], "object");
    assert_eq!(head["slot"]["value"]["target"]["object"], 10);
    assert_eq!(head["slot"]["value"]["reference"], false);
    let node = &json["objects"][0]["slot"]["value"];
    assert_eq!(node["kind"], "aggregate");
    assert_eq!(node["fields"][0]["name"], "value");
    assert_eq!(node["fields"][0]["value"]["kind"], "scalar");
}

#[test]
fn the_view_model_stays_independent_of_everything_but_the_model() {
    // Not instrumentation, processes, the app shell: and no layout vocabulary.
    let src = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/viz/mod.rs")).unwrap();
    let code: String =
        src.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
    for word in ["crate::observe", "crate::runtime", "crate::app", "crate::process", "crate::toolchain", "tauri", "unsafe"] {
        assert!(!code.contains(word), "viz must not reference `{word}`");
    }
    for word in ["width", "height", "color", "colour", "position", "pixel"] {
        assert!(!code.to_lowercase().contains(word), "viz must not carry layout vocabulary (`{word}`)");
    }
}

// ---- a real observed run -------------------------------------------------------

#[test]
fn a_real_run_reaches_the_view_model() {
    if find_libclang().is_none() {
        eprintln!("SKIPPED: no libclang found");
        return;
    }
    let service = Arc::new(ObservationService::new().with_event_limit(0));
    let m = ExecutionManager::new(common::config()).with_observer(service.clone());
    if !m.toolchain_status().available {
        return;
    }
    let src = include_str!("programs/frames.cpp");
    let session = m.run(RunRequest { observe: true, ..common::request(src) }, &|_| {});
    assert_eq!(session.exit_code, Some(0), "{session:#?}");
    let obs = session.workspace_path.as_deref().and_then(|r| service.take(r)).expect("a recording");
    let total = obs.timeline.len() as u64;
    let graphs: Vec<GraphView> = (0..=total).map(|s| graph_at(&obs, s, &[])).collect();

    // Step 0: nothing yet. Last step: everything returned, nothing left.
    assert!(graphs[0].threads.is_empty() && graphs[0].objects.is_empty() && graphs[0].event.is_none());
    let last = graphs.last().unwrap();
    assert!(last.threads.is_empty() && last.event.as_deref().unwrap().contains("FunctionExited"));

    // Inside `square`: main below it, both with variables and source lines.
    let inside = graphs
        .iter()
        .find(|g| {
            g.threads.first().is_some_and(|t| t.frames.len() == 2 && t.frames[1].function == "square" && t.frames[1].variables.len() == 2)
        })
        .expect("a step inside square");
    let frames = &inside.threads[0].frames;
    assert_eq!(frames[0].function, "main");
    assert!(frames[0].variables.iter().any(|v| v.name == "first") && frames[0].variables.iter().any(|v| v.name == "i" && v.in_block));
    assert_eq!(frames[1].variables[0].name, "n");
    assert_eq!(scalar(&frames[1].variables[0].slot), "1");
    assert!(frames[1].line.is_some() && inside.line.is_some());

    // After `link(first, &second)` the heap node points at the stack variable `second`:
    // an arrow from the heap into a frame, expressed as nothing but a target.
    let linked = graphs
        .iter()
        .rev()
        .find(|g| {
            g.threads.first().is_some_and(|t| t.frames.len() == 1)
                && g.objects.first().is_some_and(|o| o.slot.ty == "Node" && o.state == "alive")
                && g.objects.first().is_some_and(|o| pointer_target(field(&o.slot, "next")).kind == "object")
        })
        .expect("a step with the link");
    let second = linked.threads[0].frames[0].variables.iter().find(|v| v.name == "second").unwrap();
    let t = pointer_target(field(&linked.objects[0].slot, "next"));
    assert_eq!((t.object, t.dangling), (Some(second.object), false));
    let first = linked.threads[0].frames[0].variables.iter().find(|v| v.name == "first").unwrap();
    assert_eq!(pointer_target(&first.slot).object, Some(linked.objects[0].id));

    // Clamped past the end, like the text view.
    assert_eq!(graph_at(&obs, total + 99, &[]), *last);
}

#[test]
fn storage_announced_before_its_variable_is_not_drawn_as_an_object() {
    // A local's object exists one event before the variable naming it.
    let mut w = World::new();
    w.main_frame();
    w.alloc(1, NODE_PTR, StorageClass::Automatic, Value::null_pointer());
    assert!(w.graph().objects.is_empty(), "unbound automatic storage is not a heap object");
    assert!(w.graph().threads[0].frames[0].variables.is_empty());

    w.push(EventKind::VariableCreated {
        variable: VariableDecl {
            id: VariableId(1),
            name: "n".into(),
            kind: VariableKind::Local,
            frame: Some(FrameId(1)),
            scope: None,
            object: ObjectId(1),
        },
    });
    let g = w.graph();
    assert!(g.objects.is_empty());
    assert_eq!(g.threads[0].frames[0].variables[0].name, "n");
}
