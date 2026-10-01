//! User-facing timeline steps: the raw event stream grouped into the few things a
//! reader would call "one step" (a statement, a call, a return, the end of a scope).
//!
//! This is presentation only. The raw events, the snapshots and the `step` numbers
//! the rest of the app uses are untouched: a [`TimelineStep`] just says which run of
//! raw events it covers (`start..end`, as raw step numbers), so a UI can jump
//! between boundaries and still ask the backend for the exact state at any of them.
//!
//! Grouping rules:
//! - `FunctionEntered` plus the parameter set-up that follows it is one **call** step.
//! - Scope/variable/object teardown, ending in `FunctionExited` if the function
//!   returns, is one **return** (or **scope end**) step.
//! - Everything else is grouped by source line within a frame: declaring a variable,
//!   allocating its object, initializing its fields and assigning to it is one
//!   **statement**. A write to something that already existed before the statement
//!   starts a new step, so one-line loops do not collapse into a single step.
//! - `TypeDeclared` and `ScopeEntered` carry nothing a reader needs; they join the
//!   step that follows.

use std::collections::HashSet;

use serde::Serialize;

use crate::model::{
    EndReason, EventKind, ObjectId, Place, RuntimeEvent, RuntimeState, Step, Target, Value,
    VariableDecl,
};
use crate::observe::report::value_text;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum StepKind {
    Call,
    Return,
    /// A scope ended (locals destroyed) without the function returning.
    ScopeEnd,
    Statement,
    /// The recording stopped here.
    Truncated,
}

/// One user-facing step: a run of raw events presented as a single change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineStep {
    pub kind: StepKind,
    /// Raw step number before this step's first event (state after `start` events).
    pub start: u64,
    /// Raw step number after this step's last event; the state to show for the step.
    pub end: u64,
    /// Raw events folded into this step.
    pub events: u64,
    pub label: String,
    /// Source line of the step.
    pub line: Option<u32>,
    /// Call depth (0 = `main`'s frame) the step happens in.
    pub depth: u32,
}

struct Group {
    kind: StepKind,
    start: usize,
    end: usize,
    line: Option<u32>,
    depth: u32,
    /// Objects declared by this statement (for the "write to something old" split).
    new_objects: HashSet<ObjectId>,
    written: HashSet<Place>,
}

impl Group {
    fn new(kind: StepKind, start: usize, line: Option<u32>, depth: u32) -> Group {
        Group { kind, start, end: start, line, depth, new_objects: HashSet::new(), written: HashSet::new() }
    }
}

fn line_of(e: &RuntimeEvent) -> Option<u32> {
    e.location.as_ref().map(|l| l.line)
}

fn is_cleanup(k: &EventKind) -> bool {
    match k {
        EventKind::ScopeExited { .. } | EventKind::VariableDestroyed { .. } => true,
        EventKind::ObjectDestroyed { reason, .. } => *reason != EndReason::Freed,
        _ => false,
    }
}

/// Split `events` into groups (pass 1: no state needed).
fn group(events: &[RuntimeEvent]) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    let mut depth: u32 = 0;
    // Index of the first not-yet-attached "silent" event (type/scope entry).
    let mut pending: Option<usize> = None;

    for (i, e) in events.iter().enumerate() {
        let line = line_of(e);
        match &e.kind {
            EventKind::TypeDeclared { .. } | EventKind::ScopeEntered { .. } => {
                pending.get_or_insert(i);
                continue;
            }
            EventKind::FunctionEntered { .. } => {
                let start = pending.take().unwrap_or(i);
                groups.push(Group::new(StepKind::Call, start, line, depth));
                depth += 1;
            }
            EventKind::FunctionExited { .. } => {
                depth = depth.saturating_sub(1);
                match groups.last_mut() {
                    Some(g) if g.kind == StepKind::ScopeEnd => {
                        g.kind = StepKind::Return;
                        g.line = line.or(g.line);
                    }
                    _ => {
                        let start = pending.take().unwrap_or(i);
                        groups.push(Group::new(StepKind::Return, start, line, depth + 1));
                    }
                }
            }
            EventKind::ObservationTruncated { .. } => {
                let start = pending.take().unwrap_or(i);
                groups.push(Group::new(StepKind::Truncated, start, line, depth));
            }
            k if is_cleanup(k) => match groups.last_mut() {
                Some(g) if g.kind == StepKind::ScopeEnd => {}
                _ => {
                    let start = pending.take().unwrap_or(i);
                    groups.push(Group::new(StepKind::ScopeEnd, start, line, depth));
                }
            },
            k => {
                let continues = match groups.last_mut() {
                    // Parameter set-up stays on the call's line (or has no line).
                    Some(g) if g.kind == StepKind::Call => line.is_none() || line == g.line || g.line.is_none(),
                    Some(g) if g.kind == StepKind::Statement && g.depth == depth => {
                        let same_line = line.is_none() || g.line.is_none() || line == g.line;
                        let rewrites_old = match k {
                            EventKind::ValueChanged { place, .. } => {
                                !g.new_objects.contains(&place.object) && g.written.contains(place)
                            }
                            _ => false,
                        };
                        same_line && !rewrites_old
                    }
                    _ => false,
                };
                if !continues {
                    let start = pending.take().unwrap_or(i);
                    groups.push(Group::new(StepKind::Statement, start, line, depth));
                }
            }
        }
        let g = groups.last_mut().expect("a group is open");
        g.end = i + 1;
        g.line = g.line.or(line);
        match &e.kind {
            EventKind::ObjectAllocated { object } => {
                g.new_objects.insert(object.id);
            }
            EventKind::ValueChanged { place, .. } => {
                g.written.insert(place.clone());
            }
            _ => {}
        }
    }
    // Silent events at the very end belong to the last step (or to a step of their own).
    if let Some(p) = pending {
        match groups.last_mut() {
            Some(g) => g.end = events.len(),
            None => groups.push({
                let mut g = Group::new(StepKind::Statement, p, None, 0);
                g.end = events.len();
                g
            }),
        }
    }
    groups
}

fn shorten(mut s: String) -> String {
    const MAX: usize = 100;
    if s.chars().count() > MAX {
        s = s.chars().take(MAX - 1).collect::<String>() + "…";
    }
    s
}

/// The name a reader knows `object` by: its variable, or a variable pointing at it.
/// `(name, reached_through_pointer)`.
fn object_name(state: &RuntimeState, id: ObjectId) -> Option<(String, bool)> {
    if let Some(v) = state.variables().find(|v| v.object == id) {
        return Some((v.name.clone(), false));
    }
    state
        .variables()
        .find(|v| match state.object(v.object).map(|o| &o.value) {
            Some(Value::Pointer { pointer: p }) => {
                matches!(&p.target, Target::Place { place } if place.object == id && place.path.is_empty())
            }
            _ => false,
        })
        .map(|v| (v.name.clone(), true))
}

fn place_label(state: &RuntimeState, place: &Place) -> String {
    let (mut out, mut ty) = match object_name(state, place.object) {
        Some((name, true)) if place.path.is_empty() => return format!("*{name}"),
        Some((name, true)) => (format!("{name}->"), state.object(place.object).map(|o| o.ty)),
        Some((name, false)) => (name, state.object(place.object).map(|o| o.ty)),
        None => (format!("Object#{}", place.object.0), state.object(place.object).map(|o| o.ty)),
    };
    let mut first = true;
    for step in &place.path {
        match step {
            Step::Field(i) => {
                let f = ty.and_then(|t| state.types().field(t, *i));
                if !(first && out.ends_with("->")) {
                    out.push('.');
                }
                out.push_str(f.map(|f| f.name.as_str()).unwrap_or("?"));
                ty = f.map(|f| f.ty);
            }
            Step::Index(i) => {
                out.push_str(&format!("[{i}]"));
                ty = ty.and_then(|t| match &state.types().get(t)?.kind {
                    crate::model::TypeKind::Array { element, .. } => Some(*element),
                    _ => None,
                });
            }
        }
        first = false;
    }
    out
}

/// A value as a reader would say it: a pointer to something a variable also points to
/// is that variable's name; everything else as in the log view.
fn value_label(state: &RuntimeState, value: &Value) -> String {
    if let Value::Pointer { pointer } = value {
        if let Target::Place { place } = &pointer.target {
            if let Some(v) = state.variables().find(|v| {
                matches!(state.object(v.object).map(|o| &o.value),
                    Some(Value::Pointer { pointer: q }) if q.target == pointer.target)
            }) {
                return v.name.clone();
            }
            let _ = place;
        }
    }
    value_text(state, value)
}

fn type_name(state: &RuntimeState, id: ObjectId) -> String {
    state
        .object(id)
        .and_then(|o| state.types().name(o.ty))
        .unwrap_or("object")
        .to_string()
}

fn function_of(e: &RuntimeEvent) -> Option<&str> {
    match &e.kind {
        EventKind::FunctionEntered { function, .. } => Some(function),
        _ => None,
    }
}

fn label(kind: StepKind, evs: &[RuntimeEvent], state: &RuntimeState, stack: &[String]) -> String {
    match kind {
        StepKind::Truncated => "Recording stopped (event limit reached)".into(),
        StepKind::Call => {
            let name = evs.iter().find_map(function_of).unwrap_or("function");
            let args: Vec<String> = evs
                .iter()
                .filter_map(|e| match &e.kind {
                    EventKind::VariableCreated { variable } => Some(variable),
                    _ => None,
                })
                .map(|v| {
                    let val = state.object(v.object).map(|o| value_label(state, &o.value)).unwrap_or_default();
                    format!("{}={}", v.name, val)
                })
                .collect();
            shorten(format!("Call {name}({})", args.join(", ")))
        }
        StepKind::Return => {
            let name = stack.last().map(String::as_str).unwrap_or("function");
            format!("Return from {name}")
        }
        StepKind::ScopeEnd => {
            let names: Vec<String> = evs
                .iter()
                .filter_map(|e| match &e.kind {
                    EventKind::VariableDestroyed { variable } => {
                        state.variable(*variable).map(|v| v.name.clone())
                    }
                    _ => None,
                })
                .collect();
            if names.is_empty() {
                "End of scope".into()
            } else {
                shorten(format!("End of scope: {}", names.join(", ")))
            }
        }
        StepKind::Statement => statement_label(evs, state),
    }
}

fn statement_label(evs: &[RuntimeEvent], state: &RuntimeState) -> String {
    let mut declared: Vec<&VariableDecl> = Vec::new();
    let mut allocated: Vec<ObjectId> = Vec::new();
    let mut group_objects: HashSet<ObjectId> = HashSet::new();
    let mut freed: Vec<ObjectId> = Vec::new();
    for e in evs {
        match &e.kind {
            EventKind::VariableCreated { variable } => declared.push(variable),
            EventKind::ObjectAllocated { object } => {
                allocated.push(object.id);
                group_objects.insert(object.id);
            }
            EventKind::ObjectDestroyed { object, reason: EndReason::Freed } => freed.push(*object),
            _ => {}
        }
    }
    let var_objects: HashSet<ObjectId> = declared.iter().map(|v| v.object).collect();
    let mut consumed: HashSet<ObjectId> = HashSet::new();
    let mut parts: Vec<String> = Vec::new();

    for v in &declared {
        let value = state.object(v.object).map(|o| &o.value);
        let heap_target = match value {
            Some(Value::Pointer { pointer }) => match &pointer.target {
                Target::Place { place } if place.path.is_empty() && allocated.contains(&place.object) => {
                    Some(place.object)
                }
                _ => None,
            },
            _ => None,
        };
        parts.push(match (heap_target, value) {
            (Some(obj), _) => {
                consumed.insert(obj);
                let init = state.object(obj).map(|o| value_text(state, &o.value)).unwrap_or_default();
                format!("{} = new {} {}", v.name, type_name(state, obj), init)
            }
            (None, Some(val)) if !val.is_unavailable() => format!("{} = {}", v.name, value_label(state, val)),
            _ => format!("Declare {}", v.name),
        });
    }
    for id in allocated.iter().filter(|id| !var_objects.contains(id) && !consumed.contains(id)) {
        let init = state.object(*id).map(|o| value_text(state, &o.value)).unwrap_or_default();
        parts.push(format!("new {} {}", type_name(state, *id), init));
    }

    // Writes to things that existed before this statement.
    let mut seen: HashSet<&Place> = HashSet::new();
    let mut writes: Vec<String> = Vec::new();
    for e in evs.iter().rev() {
        if let EventKind::ValueChanged { place, value } = &e.kind {
            if !group_objects.contains(&place.object) && !var_objects.contains(&place.object) && seen.insert(place) {
                writes.push(format!("{} = {}", place_label(state, place), value_label(state, value)));
            }
        }
    }
    writes.reverse();
    let extra = writes.len().saturating_sub(2);
    writes.truncate(2);
    parts.extend(writes);
    if extra > 0 {
        parts.push(format!("+{extra} more"));
    }
    for id in freed {
        parts.push(match object_name(state, id) {
            Some((name, _)) => format!("delete {name}"),
            None => format!("delete Object#{}", id.0),
        });
    }

    if parts.is_empty() {
        "Update state".into()
    } else {
        shorten(parts.join("; "))
    }
}

/// The user-facing steps of a run. Step `n` of the UI (1-based) is `steps[n - 1]`;
/// step 0 is the state before the program did anything.
pub fn aggregate(events: &[RuntimeEvent]) -> Vec<TimelineStep> {
    let groups = group(events);
    let mut out = Vec::with_capacity(groups.len());
    let mut state = RuntimeState::new();
    let mut stack: Vec<String> = Vec::new();
    let mut applied = 0usize;
    for g in groups {
        let mut popped: Option<String> = None;
        for e in &events[applied..g.end] {
            if let Some(f) = function_of(e) {
                stack.push(f.to_string());
            }
            if matches!(e.kind, EventKind::FunctionExited { .. }) {
                popped = stack.pop();
            }
            // A rejected event cannot happen for a recorded timeline (it was validated on push).
            let _ = state.apply(e);
        }
        applied = g.end;
        let evs = &events[g.start..g.end];
        let text = match (&g.kind, &popped) {
            (StepKind::Return, Some(name)) => format!("Return from {name}"),
            _ => label(g.kind, evs, &state, &stack),
        };
        out.push(TimelineStep {
            kind: g.kind,
            start: g.start as u64,
            end: g.end as u64,
            events: (g.end - g.start) as u64,
            label: text,
            line: g.line,
            depth: g.depth,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn ev(seq: u64, line: u32, kind: EventKind) -> RuntimeEvent {
        RuntimeEvent {
            seq: EventSeq(seq),
            thread: ThreadId::MAIN,
            timestamp_ns: None,
            location: Some(SourceLocation::new("main.cpp", line)),
            kind,
        }
    }

    #[test]
    fn a_statement_is_one_step_and_raw_events_stay_covered() {
        let node = TypeDef::new(
            TypeId(1),
            "Node",
            TypeKind::Record {
                record: RecordKind::Struct,
                fields: vec![FieldDecl::new("value", TypeId(2)), FieldDecl::new("next", TypeId(3))],
                template_args: vec![],
            },
        );
        let obj = |id: u64, ty: u64, storage, value| ObjectDecl {
            id: ObjectId(id),
            ty: TypeId(ty),
            storage,
            address: None,
            size: None,
            state: LifeState::Alive,
            value,
        };
        let mut kinds = vec![
            EventKind::TypeDeclared { def: node },
            EventKind::TypeDeclared { def: TypeDef::new(TypeId(2), "int", TypeKind::Primitive { primitive: Primitive::Int }) },
            EventKind::TypeDeclared { def: TypeDef::new(TypeId(3), "Node*", TypeKind::Pointer { pointee: TypeId(1) }) },
        ]
        .into_iter()
        .map(|k| (1u32, k))
        .collect::<Vec<_>>();
        kinds.extend([
            (1, EventKind::FunctionEntered { frame: FrameId(1), function: "main".into(), call_site: None }),
            // Node* a = new Node{10, nullptr};
            (
                2,
                EventKind::ObjectAllocated {
                    object: obj(10, 1, StorageClass::Heap, Value::aggregate(vec![Value::int(10), Value::null_pointer()])),
                },
            ),
            (2, EventKind::ObjectAllocated { object: obj(11, 3, StorageClass::Automatic, Value::pointer_to(ObjectId(10))) }),
            (
                2,
                EventKind::VariableCreated {
                    variable: VariableDecl {
                        id: VariableId(1),
                        name: "a".into(),
                        kind: VariableKind::Local,
                        frame: Some(FrameId(1)),
                        scope: None,
                        object: ObjectId(11),
                    },
                },
            ),
            (2, EventKind::ValueChanged { place: Place::root(ObjectId(11)), value: Value::pointer_to(ObjectId(10)) }),
            // Node* b = new Node{20, nullptr};
            (
                3,
                EventKind::ObjectAllocated {
                    object: obj(12, 1, StorageClass::Heap, Value::aggregate(vec![Value::int(20), Value::null_pointer()])),
                },
            ),
            (3, EventKind::ObjectAllocated { object: obj(13, 3, StorageClass::Automatic, Value::pointer_to(ObjectId(12))) }),
            (
                3,
                EventKind::VariableCreated {
                    variable: VariableDecl {
                        id: VariableId(2),
                        name: "b".into(),
                        kind: VariableKind::Local,
                        frame: Some(FrameId(1)),
                        scope: None,
                        object: ObjectId(13),
                    },
                },
            ),
            // a->next = b;
            (5, EventKind::ValueChanged { place: Place::root(ObjectId(10)).field(1), value: Value::pointer_to(ObjectId(12)) }),
            (7, EventKind::FunctionExited { frame: FrameId(1) }),
        ]);
        let events: Vec<RuntimeEvent> =
            kinds.into_iter().enumerate().map(|(i, (line, k))| ev(i as u64, line, k)).collect();

        // The raw stream is valid and untouched.
        let mut t = Timeline::new();
        for e in &events {
            t.push(e.clone()).unwrap();
        }

        let steps = aggregate(&events);
        let labels: Vec<_> = steps.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Call main()",
                "a = new Node {10, nullptr}",
                "b = new Node {20, nullptr}",
                "a->next = b",
                "Return from main",
            ]
        );
        // Steps tile the raw events with no gap or overlap.
        assert_eq!(steps[0].start, 0);
        for w in steps.windows(2) {
            assert_eq!(w[0].end, w[1].start);
        }
        assert_eq!(steps.last().unwrap().end, events.len() as u64);
        assert_eq!(steps.iter().map(|s| s.events).sum::<u64>(), events.len() as u64);
    }
}
