//! Extension points for future runtime observation. **Nothing here observes
//! anything yet.**
//!
//! Intended pipeline:
//!
//! ```text
//! sources -> Instrumenter::prepare -> compile (+ plan's extra args) -> executable
//!         -> run (+ plan's env) -> RuntimeEventStream -> runtime data model -> visualization
//! ```
//!
//! The event *schema* and the state it builds live in [`crate::model`]
//! (`RuntimeEvent`, `RuntimeState`). The first real strategy is
//! `crate::observe::ObservingInstrumenter` (libclang analysis + source rewrite +
//! in-process runtime); [`PassThrough`] remains the default. The
//! types below only mark *where* those decisions plug in: a strategy that
//! rewrites sources, injects compile/link flags, or attaches to the process at
//! runtime can all be expressed by extending [`BuildPlan`] and implementing
//! [`RuntimeEventStream`] without changing the manager's overall flow.

use std::collections::BTreeMap;

use super::workspace::Workspace;

/// Where runtime events will eventually enter the application. Deliberately
/// schema-free: it has no `next_event` yet because there are no events.
/// Opened before the program starts, closed after it ends.
pub trait RuntimeEventStream: Send {
    fn close(&mut self);
}

/// What an [`Instrumenter`] asks the build and run stages to do differently.
#[derive(Default)]
pub struct BuildPlan {
    pub extra_compile_args: Vec<String>,
    pub run_env: BTreeMap<String, String>,
    pub event_stream: Option<Box<dyn RuntimeEventStream>>,
}

/// A stage between "sources written to workspace" and "compile". The default is
/// [`PassThrough`], which leaves the program completely untouched.
pub trait Instrumenter: Send + Sync {
    fn prepare(&self, workspace: &Workspace) -> Result<BuildPlan, String>;
}

/// Supplies the instrumenter for one particular build. A provider is consulted
/// only for runs that ask to be observed; it needs the compiler the build will use
/// (observation must analyse the program the way that compiler will build it) and
/// can refuse with a message the user should see (a missing tool, say).
pub trait ObserverProvider: Send + Sync {
    fn instrumenter(
        &self,
        compiler: &crate::toolchain::CompilerInfo,
        cxx_standard: &str,
    ) -> Result<std::sync::Arc<dyn Instrumenter>, String>;
}

/// No instrumentation: the user's program is compiled and run exactly as written.
pub struct PassThrough;

impl Instrumenter for PassThrough {
    fn prepare(&self, _workspace: &Workspace) -> Result<BuildPlan, String> {
        Ok(BuildPlan::default())
    }
}
