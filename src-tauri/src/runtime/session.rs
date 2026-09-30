//! The runtime session record and its lifecycle state machine.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::toolchain::{CompilerDiagnostic, CompilerInfo};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    /// Unique per process run and, in practice, across runs (time + counter + pid).
    pub fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        Self(format!("{}{:x}-{:x}-{:x}", Self::PREFIX, now_ms(), n, std::process::id()))
    }
    /// Every workspace directory starts with this, so the stale sweep never
    /// touches anything else that happens to live in the runtime root.
    pub const PREFIX: &'static str = "s-";

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for SessionId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionState {
    Idle,
    Compiling,
    CompilationFailed,
    Ready,
    Running,
    /// Ran and exited with code 0.
    Completed,
    /// Could not be built/started, or exited with a non-zero code.
    Failed,
    TimedOut,
    /// Stopped on request.
    Terminated,
}

impl SessionState {
    pub fn is_terminal(self) -> bool {
        use SessionState::*;
        matches!(self, CompilationFailed | Completed | Failed | TimedOut | Terminated)
    }

    pub fn can_transition_to(self, to: SessionState) -> bool {
        use SessionState::*;
        matches!(
            (self, to),
            (Idle, Compiling)
                | (Idle, Failed) // setup failed before compiling (no compiler, no workspace)
                | (Compiling, CompilationFailed)
                | (Compiling, Ready)
                | (Compiling, Failed) // compiler could not be launched
                | (Compiling, Terminated)
                | (Ready, Running)
                | (Ready, Failed)
                | (Ready, Terminated)
                | (Running, Completed)
                | (Running, Failed)
                | (Running, TimedOut)
                | (Running, Terminated)
        )
    }
}

/// Why the session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TerminationReason {
    /// The program ran to its own exit (any exit code).
    Exited,
    /// Killed because it exceeded the configured time limit.
    Timeout,
    /// Killed because the user asked to stop it.
    UserRequested,
    /// The program could not be started.
    LaunchFailed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSession {
    pub id: SessionId,
    pub project: String,
    pub source_files: Vec<String>,
    pub state: SessionState,
    pub compiler: Option<CompilerInfo>,
    pub diagnostics: Vec<CompilerDiagnostic>,
    /// Raw compiler output (diagnostics are parsed from this).
    pub compiler_output: String,
    pub executable_path: Option<PathBuf>,
    pub workspace_path: Option<PathBuf>,
    pub workspace_removed: bool,
    pub started_at_ms: u64,
    pub ended_at_ms: Option<u64>,
    /// Whole session, compile included.
    pub duration_ms: Option<u64>,
    pub compile_duration_ms: Option<u64>,
    pub run_duration_ms: Option<u64>,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub exit_code: Option<i32>,
    pub termination_reason: Option<TerminationReason>,
    /// Lattice-side failure (not the user's program), e.g. "no compiler found".
    pub error: Option<String>,
    /// Configured run timeout, echoed so the UI can report it.
    pub run_timeout_ms: u64,
}

impl RuntimeSession {
    pub fn new(id: SessionId, project: String, source_files: Vec<String>, run_timeout_ms: u64) -> Self {
        Self {
            id,
            project,
            source_files,
            state: SessionState::Idle,
            compiler: None,
            diagnostics: Vec::new(),
            compiler_output: String::new(),
            executable_path: None,
            workspace_path: None,
            workspace_removed: false,
            started_at_ms: now_ms(),
            ended_at_ms: None,
            duration_ms: None,
            compile_duration_ms: None,
            run_duration_ms: None,
            stdout: String::new(),
            stderr: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            exit_code: None,
            termination_reason: None,
            error: None,
            run_timeout_ms,
        }
    }

    /// Move to `to`. An illegal transition is a Lattice bug: it is logged and
    /// rejected (debug builds panic) rather than silently applied.
    pub fn transition(&mut self, to: SessionState) -> bool {
        if !self.state.can_transition_to(to) {
            tracing::error!(session = %self.id, from = ?self.state, ?to, "illegal session state transition");
            debug_assert!(false, "illegal transition {:?} -> {:?}", self.state, to);
            return false;
        }
        tracing::info!(session = %self.id, from = ?self.state, ?to, "session state changed");
        self.state = to;
        if to.is_terminal() {
            let end = now_ms();
            self.ended_at_ms = Some(end);
            self.duration_ms = Some(end.saturating_sub(self.started_at_ms));
        }
        true
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::SessionState::*;

    #[test]
    fn legal_and_illegal_transitions() {
        assert!(Idle.can_transition_to(Compiling));
        assert!(Compiling.can_transition_to(Ready));
        assert!(Ready.can_transition_to(Running));
        assert!(Running.can_transition_to(TimedOut));
        assert!(!Idle.can_transition_to(Running));
        assert!(!Completed.can_transition_to(Running));
        assert!(!CompilationFailed.can_transition_to(Ready));
    }

    #[test]
    fn terminal_states_have_no_exits() {
        let all = [Idle, Compiling, CompilationFailed, Ready, Running, Completed, Failed, TimedOut, Terminated];
        for s in all.into_iter().filter(|s| s.is_terminal()) {
            assert!(all.iter().all(|t| !s.can_transition_to(*t)), "{s:?} should be final");
        }
    }
}
