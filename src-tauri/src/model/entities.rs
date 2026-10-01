//! The things a [`RuntimeState`](super::RuntimeState) holds: objects, variables,
//! frames, scopes. Pure data; no behavior beyond small accessors.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::ids::{EventSeq, FrameId, ObjectId, ScopeId, ThreadId, TypeId, VariableId};
use super::source::SourceLocation;
use super::value::Value;

/// Where an object's storage lives. Metadata for the observer/visualization to
/// use; the model does not enforce storage semantics beyond "automatic storage
/// ends with its variable".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageClass {
    Automatic,
    Static,
    Heap,
    Temporary,
    ThreadLocal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifeState {
    /// Storage exists, no object has been constructed in it yet (after `operator
    /// new`, before the constructor finishes).
    Allocated,
    Alive,
    /// The object's lifetime has ended. Its last value is retained (so dangling
    /// pointers and the timeline still have something to describe) but must not
    /// be presented as current.
    Destroyed,
    /// Observed, but its lifetime is not known (e.g. it predates observation).
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    /// `delete` / `free` / allocator release.
    Freed,
    /// Its scope ended (out of scope).
    ScopeExit,
    /// Its function returned.
    FrameExit,
    ProgramExit,
    Other,
}

/// When an object's life began and ended, in event time, so a timeline can place
/// "created at event 14, destroyed at event 38" without replaying.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lifetime {
    pub state: LifeState,
    pub allocated_at: EventSeq,
    pub ended_at: Option<EventSeq>,
    pub end_reason: Option<EndReason>,
}

/// A root storage object. Its `value` tree contains all nested fields/elements.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Object {
    pub id: ObjectId,
    pub ty: TypeId,
    pub storage: StorageClass,
    /// Runtime address and size: optional metadata, never an identity.
    pub address: Option<u64>,
    pub size: Option<u64>,
    pub value: Value,
    pub lifetime: Lifetime,
    /// Where it was allocated/declared, if the observer said.
    pub origin: Option<SourceLocation>,
}

impl Object {
    pub fn is_destroyed(&self) -> bool {
        self.lifetime.state == LifeState::Destroyed
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VariableKind {
    Local,
    Parameter,
    Global,
    StaticLocal,
}

/// A name bound to storage. A variable never *contains* its value: it binds to a
/// storage [`Object`], exactly like C++ (`int x;` names an `int` object, and
/// `&x` can point at it). Two variables may therefore share or point to objects
/// independently of each other.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Variable {
    pub id: VariableId,
    pub name: String,
    pub kind: VariableKind,
    /// `None` for globals / function statics.
    pub frame: Option<FrameId>,
    /// `None` = function-level scope (lives until the frame exits).
    pub scope: Option<ScopeId>,
    pub object: ObjectId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frame {
    pub id: FrameId,
    pub thread: ThreadId,
    pub function: Arc<str>,
    /// The frame that was on top of this thread's stack when this one was
    /// entered. `None` for the outermost frame of a thread.
    pub caller: Option<FrameId>,
    /// Where the call was made from (in the caller).
    pub call_site: Option<SourceLocation>,
    /// Most recent location reported while this frame was on top.
    pub location: Option<SourceLocation>,
    /// Parameters and locals in creation order.
    pub variables: Vec<VariableId>,
    /// Open scopes, outermost first.
    pub open_scopes: Vec<ScopeId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scope {
    pub id: ScopeId,
    pub frame: FrameId,
    /// Enclosing open scope, `None` if directly in the function body.
    pub parent: Option<ScopeId>,
    pub variables: Vec<VariableId>,
}
