//! The runtime event schema: the *only* way state enters the model.
//!
//! Design choices (see `docs/runtime-model.md`):
//! - Events carry the **new** state only. The old state is always recoverable
//!   from the state at `seq - 1`; carrying both doubles the size and can disagree.
//! - All writes are one event, [`EventKind::ValueChanged`], addressed by
//!   [`Place`]. "Field changed", "pointer changed", "array element changed" and
//!   "variable changed" differ only in the path and value shape, which the
//!   consumer can see; separate event kinds would add cases without adding
//!   information.
//! - Creating a reference is creating a variable/object whose value is a
//!   `Reference`, so there is no `ReferenceCreated`.
//! - Events are plain serde data, independent of any transport.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::entities::{EndReason, LifeState, StorageClass, VariableKind};
use super::ids::{EventSeq, FrameId, ObjectId, ScopeId, ThreadId, TypeId, VariableId};
use super::source::SourceLocation;
use super::types::TypeDef;
use super::value::{Place, Value};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuntimeEvent {
    /// Strictly increasing across a run.
    pub seq: EventSeq,
    pub thread: ThreadId,
    /// Monotonic nanoseconds since the start of observation, if the observer
    /// measured it. Never used for ordering.
    pub timestamp_ns: Option<u64>,
    /// Source position this event is attributed to.
    pub location: Option<SourceLocation>,
    pub kind: EventKind,
}

/// Declaration of a newly allocated object.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ObjectDecl {
    pub id: ObjectId,
    pub ty: TypeId,
    pub storage: StorageClass,
    pub address: Option<u64>,
    pub size: Option<u64>,
    /// `Allocated` (raw storage), `Alive` (constructed) or `Unknown`.
    pub state: LifeState,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariableDecl {
    pub id: VariableId,
    pub name: String,
    pub kind: VariableKind,
    pub frame: Option<FrameId>,
    pub scope: Option<ScopeId>,
    /// The storage object this name is bound to; it must already exist.
    pub object: ObjectId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum EventKind {
    /// Add (or idempotently re-add) a type to the type table. Must precede any
    /// object of that type.
    TypeDeclared { def: TypeDef },

    FunctionEntered {
        frame: FrameId,
        function: Arc<str>,
        call_site: Option<SourceLocation>,
    },
    /// Must be the top frame of the event's thread. Ends the frame's open scopes
    /// and variables (and their still-alive automatic objects).
    FunctionExited { frame: FrameId },

    ScopeEntered { scope: ScopeId, frame: FrameId },
    /// Must be the innermost open scope of its frame. Ends the scope's variables
    /// (and their still-alive automatic objects).
    ScopeExited { scope: ScopeId },

    ObjectAllocated { object: ObjectDecl },
    /// `Allocated` -> `Alive`.
    ObjectConstructed { object: ObjectId },
    /// Ends the object's lifetime. Its last value is retained.
    ObjectDestroyed { object: ObjectId, reason: EndReason },

    VariableCreated { variable: VariableDecl },
    /// Ends the name; also ends its automatic object if still alive.
    VariableDestroyed { variable: VariableId },

    /// The value at `place` was overwritten. An empty path replaces the whole
    /// object value.
    ValueChanged { place: Place, value: Value },

    /// The observer stopped reporting (it hit its event budget): the program kept
    /// running, but nothing after this event is known. The state stops here and
    /// must not be presented as the end of the run. Always the observer's last event.
    ObservationTruncated { limit: u64 },
}
