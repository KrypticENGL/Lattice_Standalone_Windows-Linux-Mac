//! The facts the stack-frame inspector is built from, as the backend produces them:
//! frame position, variable kinds, object lifetimes in timeline steps, and `focus`.
//! (The inspector's own logic, a pure function of this data, is tested in
//! `src/visualization/inspector.test.tsx`.) Real pipeline; see `common`.

mod common;

use common::*;
use lattice_lib::model::*;
use lattice_lib::observe::{graph_at, Observation};
use lattice_lib::runtime::SessionState;
use lattice_lib::viz::GraphView;

const PROGRAM: &str = "struct Node { int data; Node* next; };
int first(Node* head) {
    int d = head->data;
    return d;
}
int main() {
    Node* list = new Node{10, nullptr};
    int d = first(list);
    delete list;
    return d == 10 ? 0 : 1;
}
";

fn run() -> Option<Observation> {
    let (session, obs) = observe(PROGRAM)?;
    assert_eq!(session.state, SessionState::Completed, "{session:#?}");
    assert_eq!(session.exit_code, Some(0));
    assert!(obs.issues.is_empty(), "{:?}", obs.issues);
    Some(obs)
}

fn every_step(obs: &Observation, focus: &[u64]) -> Vec<GraphView> {
    (0..=obs.timeline.len() as u64).map(|s| graph_at(obs, s, focus)).collect()
}

fn heap_node(obs: &Observation) -> (ObjectId, usize) {
    obs.timeline
        .events()
        .iter()
        .enumerate()
        .find_map(|(i, e)| match &e.kind {
            EventKind::ObjectAllocated { object } if object.storage == StorageClass::Heap => Some((object.id, i)),
            _ => None,
        })
        .expect("a heap allocation")
}

#[test]
fn frames_know_their_position_function_and_file() {
    let Some(obs) = run() else { return };
    let graphs = every_step(&obs, &[]);
    let inside = graphs.iter().find(|g| g.threads.first().is_some_and(|t| t.frames.len() == 2)).expect("a state inside `first`");
    let frames = &inside.threads[0].frames;

    assert_eq!((frames[0].function.as_str(), frames[0].depth), ("main", 0));
    assert_eq!((frames[1].function.as_str(), frames[1].depth), ("first", 1));
    assert_eq!(frames[0].file.as_deref(), Some("main.cpp"));
    assert_eq!(frames[1].file.as_deref(), Some("main.cpp"));
    assert_eq!(frames[0].thread, frames[1].thread);
    assert!(frames[1].line.is_some_and(|l| (2..=4).contains(&l)), "{:?}", frames[1].line);
    assert!(frames[0].id != frames[1].id);

    // The same frame keeps its id for as long as it lives: that is what a selection holds on to.
    let main_ids: Vec<u64> = graphs.iter().filter_map(|g| g.threads.first()?.frames.first().map(|f| f.id)).collect();
    assert!(main_ids.windows(2).all(|w| w[0] == w[1]), "{main_ids:?}");
}

#[test]
fn variables_say_whether_they_are_parameters_or_locals() {
    let Some(obs) = run() else { return };
    let graphs = every_step(&obs, &[]);
    let inside = graphs
        .iter()
        .find(|g| g.threads.first().is_some_and(|t| t.frames.len() == 2 && t.frames[1].variables.len() == 2))
        .expect("`first` with both head and d");
    let first = &inside.threads[0].frames[1];
    assert_eq!(first.variables.iter().map(|v| (v.name.as_str(), v.kind)).collect::<Vec<_>>(), [("head", "parameter"), ("d", "local")]);
    let main = &inside.threads[0].frames[0];
    assert!(main.variables.iter().all(|v| v.kind == "local"));

    // The value of the parameter is a pointer to the heap node: the relationship the inspector shows.
    let (node, _) = heap_node(&obs);
    let head = &first.variables[0];
    match &head.slot.value {
        lattice_lib::viz::ValueView::Pointer { target, .. } => {
            assert_eq!((target.kind, target.object, target.dangling), ("object", Some(node.0), false));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn object_lifetimes_are_in_timeline_steps() {
    let Some(obs) = run() else { return };
    let (node, alloc_index) = heap_node(&obs);
    let graphs = every_step(&obs, &[]);

    // Step N is the state after N events, so the event at index i is step i + 1.
    let alloc_step = alloc_index as u64 + 1;
    let alive = graphs.iter().find(|g| g.objects.iter().any(|o| o.id == node.0)).unwrap();
    let o = alive.objects.iter().find(|o| o.id == node.0).unwrap();
    assert_eq!(o.lifetime.allocated_step, alloc_step);
    assert_eq!((o.lifetime.ended_step, o.lifetime.end_reason), (None, None));
    assert_eq!(o.lifetime.origin_line, Some(7));

    let destroy_index = obs
        .timeline
        .events()
        .iter()
        .position(|e| matches!(&e.kind, EventKind::ObjectDestroyed { object, .. } if *object == node))
        .unwrap();
    let freed_step = destroy_index as u64 + 1;
    let freed = graphs[freed_step as usize].objects.iter().find(|o| o.id == node.0).expect("still drawn while `list` points at it");
    assert_eq!(freed.state, "destroyed");
    assert_eq!(freed.lifetime.ended_step, Some(freed_step));
    assert_eq!(freed.lifetime.end_reason, Some("freed"));
    // The step the inspector prints ("freed at event #N") is the step the slider would show.
    assert_eq!(graphs[freed_step as usize].step, freed_step);
    assert_eq!(graphs[freed_step as usize - 1].objects.iter().find(|o| o.id == node.0).unwrap().state, "alive");
    // And the pointer that now dangles says so.
    let main = &graphs[freed_step as usize].threads[0].frames[0];
    let list = main.variables.iter().find(|v| v.name == "list").unwrap();
    assert!(matches!(&list.slot.value, lattice_lib::viz::ValueView::Pointer { target, .. } if target.dangling));
}

#[test]
fn focus_describes_objects_that_are_not_otherwise_drawn() {
    let Some(obs) = run() else { return };
    let (node, _) = heap_node(&obs);
    let total = obs.timeline.len() as u64;

    // Nothing asked, nothing extra.
    assert!(graph_at(&obs, total, &[]).focus.is_empty());

    // After everything has ended the canvas draws nothing, but the inspector can still say what #node was.
    let end = graph_at(&obs, total, &[node.0]);
    assert!(end.objects.iter().all(|o| o.id != node.0), "not drawn: nothing points at it");
    assert_eq!(end.focus.len(), 1);
    let o = &end.focus[0];
    assert_eq!((o.id, o.state, o.lifetime.end_reason), (node.0, "destroyed", Some("freed")));
    assert!(o.lifetime.ended_step.is_some());
    // Its last contents are kept: data = 10.
    match &o.slot.value {
        lattice_lib::viz::ValueView::Aggregate { fields } => {
            assert!(matches!(&fields[0].value, lattice_lib::viz::ValueView::Scalar { text } if text == "10"), "{fields:?}");
        }
        other => panic!("{other:?}"),
    }

    // A variable's own storage can be focused too (it is never in `objects`), in the order asked;
    // ids that do not exist yet are skipped rather than failing.
    let graphs = every_step(&obs, &[]);
    let in_main = graphs.iter().position(|g| g.threads.first().is_some_and(|t| t.frames[0].variables.iter().any(|v| v.name == "list"))).unwrap();
    let list_obj = graphs[in_main].threads[0].frames[0].variables.iter().find(|v| v.name == "list").unwrap().object;
    let g = graph_at(&obs, in_main as u64, &[999_999, list_obj, node.0]);
    assert_eq!(g.focus.iter().map(|o| o.id).collect::<Vec<_>>(), [list_obj, node.0]);
    assert_eq!(g.focus[0].storage, "automatic");
    assert!(g.objects.iter().all(|o| o.id != list_obj));

    // Before the program ran, asking about an object that does not exist yet is simply empty.
    assert!(graph_at(&obs, 0, &[node.0]).focus.is_empty());
}

#[test]
fn the_view_serializes_with_the_names_the_frontend_expects() {
    let Some(obs) = run() else { return };
    let (node, _) = heap_node(&obs);
    let g = graph_at(&obs, obs.timeline.len() as u64, &[node.0]);
    let json = serde_json::to_value(&g).unwrap();
    let focus = &json["focus"][0];
    assert_eq!(focus["lifetime"]["endReason"], "freed");
    assert!(focus["lifetime"]["allocatedStep"].is_u64());
    assert!(focus["lifetime"]["endedStep"].is_u64());

    let inside = every_step(&obs, &[])
        .into_iter()
        .find(|g| g.threads.first().is_some_and(|t| t.frames.len() == 2 && !t.frames[1].variables.is_empty()))
        .unwrap();
    let json = serde_json::to_value(&inside).unwrap();
    let f = &json["threads"][0]["frames"][1];
    assert_eq!((f["function"].as_str(), f["depth"].as_u64(), f["file"].as_str()), (Some("first"), Some(1), Some("main.cpp")));
    assert_eq!(f["variables"][0]["kind"], "parameter");
}
