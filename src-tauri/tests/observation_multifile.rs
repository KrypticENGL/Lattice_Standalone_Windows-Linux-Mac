//! Programs spread over several files: headers and extra translation units.
mod common;

use common::*;
use lattice_lib::model::*;
use lattice_lib::runtime::{RunRequest, SessionState, SourceFile};

fn run_files(files: &[(&str, &str)]) -> Option<lattice_lib::observe::Observation> {
    let (m, inst) = observed()?;
    let req = RunRequest {
        project: "test".into(),
        files: files.iter().map(|(n, c)| SourceFile { name: (*n).into(), contents: (*c).into() }).collect(),
        observe: false,
    };
    let session = m.run(req, &|_| {});
    assert_eq!(session.state, SessionState::Completed, "{session:#?}");
    assert_eq!(session.exit_code, Some(0), "{session:#?}");
    let obs = inst.take_observation(session.workspace_path.as_ref().unwrap()).expect("observation");
    assert!(obs.issues.is_empty(), "{:?}", obs.issues);
    Some(obs)
}

const NODE_H: &str = "#pragma once
struct Node {
    int value;
    Node* next;
};
inline int twice(int x) {
    int r = x * 2;
    return r;
}
";
const LIST_CPP: &str = "#include \"node.h\"
Node* make(int v) {
    Node* n = new Node{v, nullptr};
    return n;
}
";
const MAIN_CPP: &str = "#include \"node.h\"
Node* make(int v);
int main() {
    Node* a = make(1);
    Node* b = make(2);
    a->next = b;
    int t = twice(a->value);
    delete b;
    delete a;
    return t == 2 ? 0 : 1;
}
";

fn frames_entered(t: &Timeline) -> Vec<(String, Option<String>)> {
    t.events()
        .iter()
        .filter_map(|e| match &e.kind {
            EventKind::FunctionEntered { function, .. } => {
                Some((function.to_string(), e.location.as_ref().map(|l| l.file.to_string())))
            }
            _ => None,
        })
        .collect()
}

fn node_type(s: &RuntimeSnapshot) -> &TypeDef {
    s.types().iter().find(|t| t.name == "Node").expect("Node was declared")
}

#[test]
fn a_struct_defined_in_a_header_is_described_and_its_writes_are_seen_across_files() {
    let Some(obs) = run_files(&[("node.h", NODE_H), ("list.cpp", LIST_CPP), ("main.cpp", MAIN_CPP)]) else { return };
    let t = &obs.timeline;

    // Linking was never the problem: both translation units are instrumented and share one runtime.
    assert_eq!(obs.summary.files, 2);
    assert_eq!(obs.summary.headers, 1);
    assert_eq!(obs.summary.records, 1, "Node is described once, from its header");
    let files: Vec<String> = frames_entered(t).into_iter().filter_map(|(_, f)| f).collect();
    assert!(files.contains(&"main.cpp".to_string()) && files.contains(&"list.cpp".to_string()));

    // `Node` is a real record with its fields, not an opaque name.
    let end = t.snapshot_after(EventSeq(t.len() as u64 - 1)).unwrap();
    let node = node_type(&end);
    let TypeKind::Record { fields, .. } = &node.kind else { panic!("Node is {:?}", node.kind) };
    assert_eq!(fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["value", "next"]);

    // Its contents are visible: made in list.cpp, linked in main.cpp, freed in main.cpp.
    let snaps = snapshots(t);
    let linked = snaps.iter().find(|s| heap_ids(s).len() == 2 && s.outgoing(heap_ids(s)[0]).len() == 1).expect("a -> b");
    let [a, b] = [heap_ids(linked)[0], heap_ids(linked)[1]];
    let ty = linked.object(a).unwrap().ty;
    assert_eq!(linked.value_at(&Place::root(a).field(field_named(linked, ty, "value"))), Some(&Value::int(1)));
    assert_eq!(linked.value_at(&Place::root(b).field(field_named(linked, ty, "value"))), Some(&Value::int(2)));
    assert_eq!(linked.outgoing(a)[0].target, Place::root(b));
    let write = t.events().iter().find(|e| matches!(&e.kind, EventKind::ValueChanged { place, value: Value::Pointer { .. } } if place.object == a && !place.path.is_empty() && e.location.as_ref().is_some_and(|l| l.file.as_ref() == "main.cpp"))).expect("a->next = b, attributed to main.cpp");
    assert_eq!(write.location.as_ref().unwrap().line, 6);
}

#[test]
fn functions_defined_in_a_header_get_frames_attributed_to_the_header() {
    let Some(obs) = run_files(&[("node.h", NODE_H), ("list.cpp", LIST_CPP), ("main.cpp", MAIN_CPP)]) else { return };
    let t = &obs.timeline;
    assert!(frames_entered(t).contains(&("twice".into(), Some("node.h".into()))), "{:?}", frames_entered(t));
    // Its variables are reported, on the header's own lines.
    let r = t.events().iter().find(|e| matches!(&e.kind, EventKind::VariableCreated { variable } if variable.name == "r")).unwrap();
    assert_eq!((r.location.as_ref().unwrap().file.as_ref(), r.location.as_ref().unwrap().line), ("node.h", 7));
    let snaps = snapshots(t);
    assert_eq!(values_of(&snaps, "r"), [Value::int(2)]);
}

const SHAPES_H: &str = "#ifndef SHAPES_H
#define SHAPES_H
#include <vector>
namespace geo {
struct Point {
    int x, y;
    Point(int px, int py) : x(px), y(py) {}
    int sum() const { int s = x + y; return s; }
};
template <class T> T biggest(T a, T b) { return a > b ? a : b; }
}
#endif
";

#[test]
fn namespaced_classes_with_inline_members_template_code_and_std_includes_in_a_header() {
    let a = "#include \"shapes.h\"
int fromA() { geo::Point* p = new geo::Point(1, 2); int s = p->sum(); delete p; return s; }
";
    let m = "#include \"shapes.h\"
int fromA();
int main() {
    geo::Point* q = new geo::Point(3, 4);
    q->x = 10;
    int s = q->sum() + fromA() + geo::biggest(1, 2);
    delete q;
    return s == 10 + 4 + 3 + 2 ? 0 : 1;
}
";
    let Some(obs) = run_files(&[("shapes.h", SHAPES_H), ("a.cpp", a), ("main.cpp", m)]) else { return };
    let t = &obs.timeline;
    // Included from two units, instrumented once (a double rewrite would not even compile).
    assert_eq!(obs.summary.headers, 1);
    let end = t.snapshot_after(EventSeq(t.len() as u64 - 1)).unwrap();
    let point = end.types().iter().find(|ty| ty.name == "geo::Point").expect("qualified name");
    assert!(matches!(&point.kind, TypeKind::Record { fields, .. } if fields.len() == 2));
    // The in-class member (and its local) are framed on the header's lines; the template is left alone.
    let frames = frames_entered(t);
    assert!(frames.contains(&("Point::sum".into(), Some("shapes.h".into()))), "{frames:?}");
    assert!(frames.iter().all(|(f, _)| !f.contains("biggest")), "templates are not instrumented: {frames:?}");
    // The write through the pointer, in main.cpp, to a header-defined type, is seen.
    let snaps = snapshots(t);
    let q = snaps.iter().find(|s| heap_ids(s).len() == 1 && s.objects().any(|o| matches!(&o.value, Value::Aggregate { fields } if fields.first() == Some(&Value::int(10))))).expect("q->x = 10");
    assert!(q.verify_invariants().is_ok());
}

#[test]
fn an_error_in_a_header_leaves_the_program_untouched_for_the_compiler_to_report() {
    let bad_h = "struct Bad { int x; };
int broken() { return undefined_name; }
";
    let main = "#include \"bad.h\"
int main() { return 0; }
";
    let Some((m, inst)) = observed() else { return };
    let req = RunRequest {
        project: "test".into(),
        files: vec![
            SourceFile { name: "bad.h".into(), contents: bad_h.into() },
            SourceFile { name: "main.cpp".into(), contents: main.into() },
        ],
        observe: false,
    };
    let session = m.run(req, &|_| {});
    assert_eq!(session.state, SessionState::CompilationFailed);
    let obs = inst.take_observation(session.workspace_path.as_ref().unwrap()).expect("observation");
    assert!(obs.summary.skipped.as_deref().is_some_and(|s| s.contains("not instrumented")), "{:?}", obs.summary.skipped);
    assert!(session.diagnostics.iter().any(|d| d.file.as_deref().is_some_and(|f| f.ends_with("bad.h"))), "{:?}", session.diagnostics);
}
