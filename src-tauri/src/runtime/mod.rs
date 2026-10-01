//! Runtime sessions: compiling and running the user's program.
//!
//! Dependency direction: `runtime` uses `toolchain` (compile) and `process`
//! (execute). Neither depends on `runtime`, and nothing here knows about the UI.
//! See `docs/architecture.md`.

pub mod instrumentation;
pub mod manager;
pub mod session;
pub mod workspace;

pub use manager::{ExecutionManager, RunRequest};
pub use session::{ObservationSummary, RuntimeSession, SessionId, SessionState, TerminationReason};
pub use workspace::SourceFile;
