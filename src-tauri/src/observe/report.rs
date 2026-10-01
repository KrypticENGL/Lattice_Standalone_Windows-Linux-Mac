//! Plain-text rendering of URR events and state, for logs and tests. This is a
//! *diagnostic view* of the model, not a visualization: no layout, no shapes.

use crate::model::{
    EventKind, LifeState, Object, Place, RuntimeEvent, RuntimeState, Step, Target, TargetStatus,
    Timeline, TypeId, Unavailable, Value,
};

/// `Object#3`, or `total#7` when a live variable is bound to the object.
fn object_label(state: &RuntimeState, id: crate::model::ObjectId) -> String {
    match state.variables().find(|v| v.object == id) {
        Some(v) => format!("{}#{}", v.name, id.0),
        None => format!("Object#{}", id.0),
    }
}

/// `Object#1.next[2]`: the place with field names resolved through the type table.
pub fn place_name(state: &RuntimeState, place: &Place) -> String {
    let mut out = object_label(state, place.object);
    let mut ty = state.object(place.object).map(|o| o.ty);
    for step in &place.path {
        match step {
            Step::Field(i) => {
                let f = ty.and_then(|t| state.types().field(t, *i));
                out.push('.');
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
    }
    out
}

fn target_text(state: &RuntimeState, target: &Target) -> String {
    match target {
        Target::Null => "nullptr".into(),
        Target::Unresolved => "<untracked>".into(),
        Target::Place { place } => {
            let dangling = state.target_status(target) == TargetStatus::Dangling;
            format!("→ {}{}", place_name(state, place), if dangling { " (dangling)" } else { "" })
        }
    }
}

pub fn value_text(state: &RuntimeState, v: &Value) -> String {
    match v {
        Value::Int { value } => value.to_string(),
        Value::UInt { value } => value.to_string(),
        Value::Float { value } => value.to_string(),
        Value::Bool { value } => value.to_string(),
        Value::Char { value } => char::from_u32(*value).map(|c| format!("{c:?}")).unwrap_or_else(|| value.to_string()),
        Value::String { value } => format!("{value:?}"),
        Value::Enum { value, enumerator } => enumerator.clone().unwrap_or_else(|| value.to_string()),
        Value::Pointer { pointer } => target_text(state, &pointer.target),
        Value::Reference { target } => format!("&{}", target_text(state, target)),
        Value::Aggregate { fields } => {
            format!("{{{}}}", fields.iter().map(|f| value_text(state, f)).collect::<Vec<_>>().join(", "))
        }
        Value::Array { elements } => {
            format!("[{}]", elements.iter().map(|f| value_text(state, f)).collect::<Vec<_>>().join(", "))
        }
        Value::Union { value, .. } => value_text(state, value),
        Value::Unavailable { unavailable } => match unavailable {
            Unavailable::Unknown => "<unknown>".into(),
            Unavailable::Uninitialized => "<uninitialized>".into(),
            Unavailable::OptimizedAway => "<optimized away>".into(),
            Unavailable::Unreadable => "<unreadable>".into(),
            Unavailable::OutOfScope => "<out of scope>".into(),
            Unavailable::Invalid { raw } => format!("<invalid {}>", raw.clone().unwrap_or_default()),
        },
    }
}

fn type_name(state: &RuntimeState, ty: TypeId) -> String {
    state.types().name(ty).unwrap_or("?").to_string()
}

/// One line per event, named exactly like the URR's `EventKind`. `state` is the
/// state *after* the event (so types and fields it mentions are resolvable).
pub fn event_line(e: &RuntimeEvent, state: &RuntimeState) -> String {
    let loc = e
        .location
        .as_ref()
        .map(|l| format!("{}:{}{}", l.file, l.line, l.column.map(|c| format!(":{c}")).unwrap_or_default()))
        .unwrap_or_else(|| "-".into());
    let (name, detail) = match &e.kind {
        EventKind::TypeDeclared { def } => ("TypeDeclared", format!("{} (type {})", def.name, def.id.0)),
        EventKind::FunctionEntered { frame, function, .. } => ("FunctionEntered", format!("{function} ({frame})")),
        EventKind::FunctionExited { frame } => ("FunctionExited", frame.to_string()),
        EventKind::ScopeEntered { scope, .. } => ("ScopeEntered", scope.to_string()),
        EventKind::ScopeExited { scope } => ("ScopeExited", scope.to_string()),
        EventKind::ObjectAllocated { object } => (
            "ObjectAllocated",
            format!(
                "Object#{} : {} ({:?}, {:?}{})",
                object.id.0,
                type_name(state, object.ty),
                object.storage,
                object.state,
                object.address.map(|a| format!(", 0x{a:x}")).unwrap_or_default()
            ),
        ),
        EventKind::ObjectConstructed { object } => ("ObjectConstructed", format!("Object#{}", object.0)),
        EventKind::ObjectDestroyed { object, reason } => {
            ("ObjectDestroyed", format!("Object#{} ({reason:?})", object.0))
        }
        EventKind::VariableCreated { variable } => (
            "VariableCreated",
            format!("{} ({:?}) -> Object#{}", variable.name, variable.kind, variable.object.0),
        ),
        EventKind::VariableDestroyed { variable } => ("VariableDestroyed", variable.to_string()),
        EventKind::ValueChanged { place, value } => {
            ("ValueChanged", format!("{} = {}", place_name(state, place), value_text(state, value)))
        }
        EventKind::ObservationTruncated { limit } => {
            ("ObservationTruncated", format!("event limit {limit} reached; nothing after this was recorded"))
        }
    };
    format!("#{:<3} {:<17} {:<14} {}", e.seq.0, name, loc, detail)
}

/// Every event of a timeline, one per line, each rendered against the state at
/// that event (so a pointer is only "dangling" once its target really is).
/// Replays a snapshot per event: a log view, not a hot path.
pub fn timeline_text(t: &Timeline) -> String {
    t.events()
        .iter()
        .map(|e| match t.snapshot_after(e.seq) {
            Ok(state) => event_line(e, &state),
            Err(_) => format!("#{} <unavailable>", e.seq.0),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn object_text(state: &RuntimeState, o: &Object) -> String {
    let life = match o.lifetime.state {
        LifeState::Allocated => "allocated".to_string(),
        LifeState::Alive => "alive".to_string(),
        LifeState::Unknown => "lifetime unknown".to_string(),
        LifeState::Destroyed => format!("destroyed at #{}", o.lifetime.ended_at.map(|s| s.0).unwrap_or(0)),
    };
    let mut s = format!("Object #{}: {} [{:?}, {}]", o.id.0, type_name(state, o.ty), o.storage, life);
    match (&o.value, state.types().fields(o.ty)) {
        (Value::Aggregate { fields }, Some(decls)) => {
            for (decl, v) in decls.iter().zip(fields) {
                s.push_str(&format!("\n    {}{}{}", decl.name, sep(Some(v)), value_text(state, v)));
            }
        }
        (v, _) => s.push_str(&format!("\n    = {}", value_text(state, v))),
    }
    s
}

fn sep(v: Option<&Value>) -> &'static str {
    // `next → Object#2` reads better than `next = → Object#2`.
    match v {
        Some(Value::Pointer { pointer }) if !matches!(pointer.target, Target::Null) => " ",
        _ => " = ",
    }
}

/// The textual form of the URR: the call stack with each frame's variables, then
/// the heap. (A diagnostic view, not a visualization.)
pub fn snapshot_text(state: &RuntimeState) -> String {
    let head = match state.last_seq() {
        Some(seq) => format!(
            "Runtime Snapshot after event #{}{}:",
            seq.0,
            state.last_location().map(|l| format!(" ({}:{})", l.file, l.line)).unwrap_or_default()
        ),
        None => "Runtime Snapshot (initial):".to_string(),
    };
    let mut lines = vec![head];

    let threads: Vec<_> = state.threads().collect();
    for thread in &threads {
        lines.push(if threads.len() > 1 {
            format!("  Call stack, thread {} (innermost first):", thread.0)
        } else {
            "  Call stack (innermost first):".to_string()
        });
        for frame_id in state.stack(*thread).iter().rev() {
            let Some(frame) = state.frame(*frame_id) else { continue };
            let at = frame.location.as_ref().map(|l| format!("  at {}:{}", l.file, l.line)).unwrap_or_default();
            lines.push(format!("    {}(){}", frame.function, at));
            for v in state.frame_variables(*frame_id) {
                let ty = state.type_of(v.object).map(|t| t.name.as_str()).unwrap_or("?");
                let value = state.variable_value(v.id).map(|x| value_text(state, x)).unwrap_or_default();
                let scope = if v.scope.is_some() { " (block)" } else { "" };
                lines.push(format!("        {} {}{}{}{}", ty, v.name, sep(state.variable_value(v.id)), value, scope));
            }
        }
    }
    if !state.globals().is_empty() {
        lines.push("  Globals:".to_string());
        for id in state.globals() {
            if let Some(v) = state.variable(*id) {
                let value = state.variable_value(v.id).map(|x| value_text(state, x)).unwrap_or_default();
                lines.push(format!("        {} = {}", v.name, value));
            }
        }
    }

    let heap: Vec<&Object> = state.objects().filter(|o| o.storage == crate::model::StorageClass::Heap).collect();
    if !heap.is_empty() {
        lines.push("  Heap:".to_string());
        for o in heap {
            lines.push(format!("    {}", object_text(state, o).replace('\n', "\n    ")));
        }
    }
    lines.join("\n")
}
