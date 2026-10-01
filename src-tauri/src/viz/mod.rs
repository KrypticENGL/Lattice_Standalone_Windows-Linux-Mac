//! The visualization's view of the runtime: one URR snapshot as plain, layout-free,
//! UI-facing data.
//!
//! This module depends on the model and on nothing else: it knows nothing about
//! instrumentation, processes or the compiler, and (the other direction) nothing
//! about rendering: no coordinates, sizes, colours or shapes. It answers "what is
//! in the program at this point, and how is it connected?"; a renderer decides how
//! that looks.
//!
//! The shape is deliberately generic. There is no "list", "tree" or "graph" here:
//! a **frame** has **variables**, a **heap object** is a typed tree of **slots**, and
//! a pointer slot has a **target** (null, an object or part of one, untracked, or
//! dangling). A linked list, a tree and an arbitrary graph are all just these.
//!
//! Anchors. Every slot has an address a renderer can use to attach arrows: the
//! object id followed by the path inside it (`12`, `12.1`, `12.1[3]`: `.i` is the
//! i-th field, `[i]` the i-th element). A pointer's `target` carries the same
//! (`object`, `path`), so an arrow is just "from this anchor to that anchor".

use std::collections::HashSet;

use serde::Serialize;

use crate::model::{
    EventKind, LifeState, Object, ObjectId, RuntimeEvent, RuntimeState, Step, StorageClass, Target,
    TargetStatus, TypeId, TypeKind, Unavailable, Value,
};

/// Heap objects shown at once. Beyond this the view says how many it left out.
pub const MAX_OBJECTS: usize = 300;
/// Elements of one array shown; the rest are counted, not listed.
pub const MAX_ARRAY_ELEMENTS: usize = 64;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphView {
    /// The state after this many events (0 = before the program did anything).
    pub step: u64,
    pub total: u64,
    /// The recording ends at this state; the program ran on unobserved.
    pub truncated_here: bool,
    /// Human-readable description of the event that led here.
    pub event: Option<String>,
    /// Source line the event is attributed to (for highlighting in the editor).
    pub line: Option<u32>,
    /// Anchors that changed at this step (`12`, `12.1`, ...), for emphasis.
    pub changed: Vec<String>,
    pub threads: Vec<ThreadView>,
    pub globals: Vec<VariableView>,
    /// Heap (and other un-named) objects. Destroyed ones appear only while something
    /// visible still points at them.
    pub objects: Vec<ObjectView>,
    /// Objects that qualified but did not fit under [`MAX_OBJECTS`].
    pub objects_omitted: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadView {
    pub id: u64,
    /// Outermost frame first.
    pub frames: Vec<FrameView>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameView {
    pub id: u64,
    pub function: String,
    /// Where this frame last reported being.
    pub line: Option<u32>,
    /// Parameters and locals in declaration order.
    pub variables: Vec<VariableView>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VariableView {
    pub name: String,
    /// Declared in a nested block (not at the function's top level).
    pub in_block: bool,
    /// The storage object the name is bound to: the anchor of this variable.
    pub object: u64,
    pub slot: SlotView,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectView {
    pub id: u64,
    /// `alive`, `allocated` (storage, not yet constructed), `destroyed`, `unknown`.
    pub state: &'static str,
    /// `heap`, `static`, `automatic`, ...
    pub storage: String,
    pub address: Option<String>,
    pub slot: SlotView,
}

/// A named, typed place holding a value.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotView {
    /// The field name, `[i]` for an array element, or `None` for a root.
    pub name: Option<String>,
    pub ty: String,
    pub value: ValueView,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ValueView {
    /// Numbers, booleans, characters, strings, enumerators: already formatted.
    Scalar { text: String },
    Pointer { target: TargetView, reference: bool },
    /// Struct/class members, in declaration order (the position is the field index).
    Aggregate { fields: Vec<SlotView> },
    /// Array elements (the position is the index); `omitted` more are not listed.
    Array { elements: Vec<SlotView>, omitted: u32 },
    /// No value, and why (`uninitialized`, `unknown`, `optimized away`, ...).
    Unavailable { reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetView {
    /// `null`, `object` (an object or part of one), or `unresolved` (non-null, untracked).
    pub kind: &'static str,
    pub object: Option<u64>,
    /// Path inside `object` (`""` for the whole object, else `.1`, `.1[3]`, ...).
    pub path: String,
    /// The target's lifetime has ended.
    pub dangling: bool,
}

fn path_text(path: &[Step]) -> String {
    path.iter()
        .map(|s| match s {
            Step::Field(i) => format!(".{i}"),
            Step::Index(i) => format!("[{i}]"),
        })
        .collect()
}

fn anchor(object: ObjectId, path: &[Step]) -> String {
    format!("{}{}", object.0, path_text(path))
}

fn type_name(state: &RuntimeState, ty: Option<TypeId>) -> String {
    ty.and_then(|t| state.types().name(t)).unwrap_or("?").to_string()
}

fn char_text(code: u32) -> String {
    match char::from_u32(code) {
        Some('\n') => "'\\n'".into(),
        Some('\t') => "'\\t'".into(),
        Some('\r') => "'\\r'".into(),
        Some('\0') => "'\\0'".into(),
        Some(c) if !c.is_control() => format!("'{c}'"),
        _ => format!("'\\x{code:02x}'"),
    }
}

fn float_text(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{v:.1}")
    } else {
        format!("{v}")
    }
}

fn target_view(state: &RuntimeState, target: &Target) -> TargetView {
    match target {
        Target::Null => TargetView { kind: "null", object: None, path: String::new(), dangling: false },
        Target::Unresolved => TargetView { kind: "unresolved", object: None, path: String::new(), dangling: false },
        Target::Place { place } => TargetView {
            kind: "object",
            object: Some(place.object.0),
            path: path_text(&place.path),
            dangling: state.target_status(target) == TargetStatus::Dangling,
        },
    }
}

/// The type of the child at `step` of a value of type `ty`, if the type table says.
fn child_type(state: &RuntimeState, ty: Option<TypeId>, step: Step) -> Option<TypeId> {
    let ty = ty?;
    match (step, &state.types().get(ty)?.kind) {
        (Step::Field(i), TypeKind::Record { .. }) => state.types().field(ty, i).map(|f| f.ty),
        (Step::Index(_), TypeKind::Array { element, .. }) => Some(*element),
        _ => None,
    }
}

fn slot(state: &RuntimeState, name: Option<String>, ty: Option<TypeId>, value: &Value) -> SlotView {
    let value = match value {
        Value::Int { value } => ValueView::Scalar { text: value.to_string() },
        Value::UInt { value } => ValueView::Scalar { text: value.to_string() },
        Value::Float { value } => ValueView::Scalar { text: float_text(*value) },
        Value::Bool { value } => ValueView::Scalar { text: value.to_string() },
        Value::Char { value } => ValueView::Scalar { text: char_text(*value) },
        Value::String { value } => ValueView::Scalar { text: format!("{value:?}") },
        Value::Enum { value, enumerator } => {
            ValueView::Scalar { text: enumerator.clone().unwrap_or_else(|| value.to_string()) }
        }
        Value::Pointer { pointer } => {
            ValueView::Pointer { target: target_view(state, &pointer.target), reference: false }
        }
        Value::Reference { target } => ValueView::Pointer { target: target_view(state, target), reference: true },
        Value::Aggregate { fields } => ValueView::Aggregate {
            fields: fields
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let fty = child_type(state, ty, Step::Field(i as u32));
                    let fname = ty
                        .and_then(|t| state.types().field(t, i as u32))
                        .map(|f| f.name.clone())
                        .unwrap_or_else(|| format!("field{i}"));
                    slot(state, Some(fname), fty, v)
                })
                .collect(),
        },
        Value::Array { elements } => {
            let shown = elements.len().min(MAX_ARRAY_ELEMENTS);
            ValueView::Array {
                elements: elements[..shown]
                    .iter()
                    .enumerate()
                    .map(|(i, v)| slot(state, Some(format!("[{i}]")), child_type(state, ty, Step::Index(i as u64)), v))
                    .collect(),
                omitted: (elements.len() - shown) as u32,
            }
        }
        Value::Union { value, .. } => ValueView::Scalar { text: format!("union ({})", slot_text(state, value)) },
        Value::Unavailable { unavailable } => ValueView::Unavailable {
            reason: match unavailable {
                Unavailable::Unknown => "unknown".into(),
                Unavailable::Uninitialized => "uninitialized".into(),
                Unavailable::OptimizedAway => "optimized away".into(),
                Unavailable::Unreadable => "unreadable".into(),
                Unavailable::OutOfScope => "out of scope".into(),
                Unavailable::Invalid { raw } => format!("invalid{}", raw.as_ref().map(|r| format!(" ({r})")).unwrap_or_default()),
            },
        },
    };
    SlotView { name, ty: type_name(state, ty), value }
}

fn slot_text(state: &RuntimeState, v: &Value) -> String {
    match &slot(state, None, None, v).value {
        ValueView::Scalar { text } => text.clone(),
        _ => "…".into(),
    }
}

fn object_state(o: &Object) -> &'static str {
    match o.lifetime.state {
        LifeState::Allocated => "allocated",
        LifeState::Alive => "alive",
        LifeState::Destroyed => "destroyed",
        LifeState::Unknown => "unknown",
    }
}

fn storage_name(s: StorageClass) -> &'static str {
    match s {
        StorageClass::Automatic => "automatic",
        StorageClass::Static => "static",
        StorageClass::Heap => "heap",
        StorageClass::Temporary => "temporary",
        StorageClass::ThreadLocal => "threadLocal",
    }
}

fn variable_view(state: &RuntimeState, v: &crate::model::Variable) -> Option<VariableView> {
    let object = state.object(v.object)?;
    Some(VariableView {
        name: v.name.clone(),
        in_block: v.scope.is_some(),
        object: v.object.0,
        slot: slot(state, None, Some(object.ty), &object.value),
    })
}

/// Anchors an event touched, for emphasis in the view.
fn changed_by(event: &RuntimeEvent) -> Vec<String> {
    match &event.kind {
        EventKind::ValueChanged { place, .. } => vec![anchor(place.object, &place.path)],
        EventKind::ObjectAllocated { object } => vec![object.id.0.to_string()],
        EventKind::ObjectConstructed { object } | EventKind::ObjectDestroyed { object, .. } => {
            vec![object.0.to_string()]
        }
        EventKind::VariableCreated { variable } => vec![variable.object.0.to_string()],
        _ => Vec::new(),
    }
}

/// Describe `state` (the program after `step` of `total` events) for a renderer.
/// `event` is the event that produced it, with its text if the caller has one.
pub fn build(
    state: &RuntimeState,
    event: Option<&RuntimeEvent>,
    event_text: Option<String>,
    step: u64,
    total: u64,
) -> GraphView {
    let truncated_here = match (state.truncated_at(), state.last_seq()) {
        (Some(t), Some(l)) => t == l,
        _ => false,
    };

    // Frames and their variables.
    let mut bound: HashSet<ObjectId> = HashSet::new();
    let mut threads = Vec::new();
    for thread in state.threads() {
        let mut frames = Vec::new();
        for fid in state.stack(thread) {
            let Some(frame) = state.frame(*fid) else { continue };
            let variables: Vec<VariableView> = state
                .frame_variables(*fid)
                .filter_map(|v| {
                    bound.insert(v.object);
                    variable_view(state, v)
                })
                .collect();
            frames.push(FrameView {
                id: fid.0,
                function: frame.function.to_string(),
                line: frame.location.as_ref().map(|l| l.line),
                variables,
            });
        }
        threads.push(ThreadView { id: thread.0, frames });
    }
    let globals: Vec<VariableView> = state
        .globals()
        .iter()
        .filter_map(|id| state.variable(*id))
        .filter_map(|v| {
            bound.insert(v.object);
            variable_view(state, v)
        })
        .collect();

    // Objects that are not someone's named storage: the heap, mostly.
    let mut targeted: HashSet<ObjectId> = HashSet::new();
    let mut note_links = |value: &Value| {
        value.for_each_link(|_, _, t| {
            if let Target::Place { place } = t {
                targeted.insert(place.object);
            }
        });
    };
    for v in state.variables() {
        if let Some(o) = state.object(v.object) {
            note_links(&o.value);
        }
    }
    // Automatic storage that no variable names (yet) is never drawn as an object: a
    // local's storage is announced one event before its name is bound, and that
    // instant is not a thing in the program.
    let candidates: Vec<&Object> = state
        .objects()
        .filter(|o| !bound.contains(&o.id) && o.storage != StorageClass::Automatic)
        .collect();
    for o in &candidates {
        if !o.is_destroyed() {
            note_links(&o.value);
        }
    }
    // A destroyed object is shown only while something visible still points at it.
    let shown: Vec<&Object> =
        candidates.into_iter().filter(|o| !o.is_destroyed() || targeted.contains(&o.id)).collect();
    let omitted = shown.len().saturating_sub(MAX_OBJECTS) as u32;
    let objects: Vec<ObjectView> = shown
        .into_iter()
        .take(MAX_OBJECTS)
        .map(|o| ObjectView {
            id: o.id.0,
            state: object_state(o),
            storage: storage_name(o.storage).to_string(),
            address: o.address.map(|a| format!("0x{a:x}")),
            slot: slot(state, None, Some(o.ty), &o.value),
        })
        .collect();

    GraphView {
        step,
        total,
        truncated_here,
        event: event_text,
        line: event.and_then(|e| e.location.as_ref()).map(|l| l.line),
        changed: event.map(changed_by).unwrap_or_default(),
        threads,
        globals,
        objects,
        objects_omitted: omitted,
    }
}
