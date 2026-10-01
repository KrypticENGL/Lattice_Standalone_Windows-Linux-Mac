//! Logical identifiers. All are opaque, never reused within one run, and
//! assigned by the *observer* (the future instrumentation), which is the only
//! party that sees raw addresses and can tell "same object" from "new object at
//! a recycled address". The model only checks uniqueness; it never invents ids.

use serde::{Deserialize, Serialize};

macro_rules! id_type {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub struct $name(pub u64);

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}#{}", stringify!($name), self.0)
            }
        }
    };
}

id_type!(
    /// A root storage object: a stack variable's storage, a heap allocation, a
    /// global, a temporary. Sub-objects (fields, elements) are addressed by
    /// [`Place`](super::Place) paths, not by their own id.
    ObjectId
);
id_type!(
    /// A named binding in a scope (local, parameter, global).
    VariableId
);
id_type!(
    /// One activation of a function.
    FrameId
);
id_type!(
    /// A lexical block inside a frame.
    ScopeId
);
id_type!(
    /// A type in the [`TypeTable`](super::TypeTable).
    TypeId
);

/// Execution context an event came from. Single-threaded programs use
/// [`ThreadId::MAIN`] throughout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ThreadId(pub u64);

impl ThreadId {
    pub const MAIN: ThreadId = ThreadId(0);
}

/// Position in the total order of observed events. Event `n` is the `n`-th event
/// the observer reported; "state at `n`" is the state *after* applying it. It is
/// an observation order, not a happens-before relation between threads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EventSeq(pub u64);

impl std::fmt::Display for EventSeq {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "@{}", self.0)
    }
}
