//! Universal runtime model: a language-runtime-shaped, UI-independent description
//! of what a C++ program's state looks like at each step of its execution.
//!
//! It knows about variables, objects, types, values, pointers, frames, lifetimes
//! and events. It does **not** know about data structures ("list", "tree"),
//! rendering, layout, processes, or how events are obtained. See
//! `docs/runtime-model.md`.
//!
//! ```text
//! observer (future) --RuntimeEvent--> RuntimeState / Timeline --RuntimeSnapshot--> (future) viewer
//! ```
//!
//! Nothing in this module produces events. There is no instrumentation yet.

mod entities;
mod event;
mod ids;
mod source;
mod state;
mod timeline;
mod types;
mod value;

pub use entities::{
    EndReason, Frame, LifeState, Lifetime, Object, Scope, StorageClass, Variable, VariableKind,
};
pub use event::{EventKind, ObjectDecl, RuntimeEvent, VariableDecl};
pub use ids::{EventSeq, FrameId, ObjectId, ScopeId, ThreadId, TypeId, VariableId};
pub use source::SourceLocation;
pub use state::{ApplyError, RuntimeState, TargetStatus};
pub use timeline::{RuntimeSnapshot, Timeline, TimelineError};
pub use types::{
    Enumerator, FieldDecl, Primitive, Qualifiers, RecordKind, TemplateArg, TypeDef, TypeKind,
    TypeTable,
};
pub use value::{Edge, EdgeKind, Place, PointerValue, Step, Target, Unavailable, Value};
