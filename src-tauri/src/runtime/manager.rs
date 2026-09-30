//! `ExecutionManager`: drives one session through
//! workspace -> instrument -> compile -> run -> cleanup.
//!
//! UI-independent: callers pass a [`RunRequest`] and an observer that is told
//! about every state change.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tracing::{info, warn};

use super::instrumentation::{Instrumenter, PassThrough};
use super::session::{RuntimeSession, SessionId, SessionState, TerminationReason};
use super::workspace::{sweep_stale, SourceFile, Workspace};
use crate::config::{CleanupPolicy, ExecutionConfig};
use crate::process::{run_process, CancelToken, ProcessSpec, ProcessStatus};
use crate::toolchain::{
    CompileOutcome, CompileRequest, Compiler, CompilerDiagnostic, DiscoverySpec, Severity, Toolchain,
    ToolchainStatus,
};

#[derive(Debug, Clone)]
pub struct RunRequest {
    pub project: String,
    pub files: Vec<SourceFile>,
}

pub type StateObserver<'a> = &'a (dyn Fn(&RuntimeSession) + Sync);

pub struct ExecutionManager {
    config: ExecutionConfig,
    toolchain: Mutex<Arc<Toolchain>>,
    instrumenter: Arc<dyn Instrumenter>,
    active: Mutex<HashMap<SessionId, CancelToken>>,
}

impl ExecutionManager {
    pub fn new(config: ExecutionConfig) -> Self {
        Self::with_discovery(config, DiscoverySpec::from_environment())
    }

    pub fn with_discovery(config: ExecutionConfig, discovery: DiscoverySpec) -> Self {
        let toolchain = Arc::new(Toolchain::discover(&discovery));
        Self {
            config,
            toolchain: Mutex::new(toolchain),
            instrumenter: Arc::new(PassThrough),
            active: Mutex::new(HashMap::new()),
        }
    }

    /// Replace the instrumentation stage (future runtime observation).
    pub fn with_instrumenter(mut self, instrumenter: Arc<dyn Instrumenter>) -> Self {
        self.instrumenter = instrumenter;
        self
    }

    pub fn config(&self) -> &ExecutionConfig {
        &self.config
    }

    pub fn toolchain_status(&self) -> ToolchainStatus {
        self.toolchain.lock().unwrap().status()
    }

    /// The compiler builds will use, so editor tooling can mirror it.
    pub fn compiler_info(&self) -> Option<crate::toolchain::CompilerInfo> {
        self.toolchain.lock().unwrap().preferred().map(|c| c.info().clone())
    }

    /// Re-run compiler discovery (e.g. after the user installs one).
    pub fn rescan_toolchain(&self) -> ToolchainStatus {
        let fresh = Arc::new(Toolchain::discover(&DiscoverySpec::from_environment()));
        let status = fresh.status();
        *self.toolchain.lock().unwrap() = fresh;
        status
    }

    /// Remove workspaces left behind by earlier crashed runs.
    pub fn sweep_stale_workspaces(&self) -> usize {
        sweep_stale(&self.config.runtime_root, self.config.stale_workspace_age)
    }

    /// Ask a running session to stop. Returns whether the session was active.
    pub fn stop(&self, id: &str) -> bool {
        let active = self.active.lock().unwrap();
        match active.iter().find(|(k, _)| k.as_str() == id) {
            Some((_, token)) => {
                info!(session = id, "stop requested");
                token.cancel();
                true
            }
            None => false,
        }
    }

    /// Compile and run `request`. Blocks until the session ends; call from a
    /// worker thread. The returned session is always in a terminal state.
    pub fn run(&self, request: RunRequest, observer: StateObserver<'_>) -> RuntimeSession {
        let id = SessionId::new();
        let cancel = CancelToken::new();
        self.active.lock().unwrap().insert(id.clone(), cancel.clone());

        let names = request.files.iter().map(|f| f.name.clone()).collect();
        let mut session = RuntimeSession::new(
            id.clone(),
            request.project.clone(),
            names,
            self.config.run_timeout.as_millis() as u64,
        );
        info!(session = %id, files = request.files.len(), "session created");

        let workspace = self.execute(&mut session, &request, &cancel, observer);
        self.cleanup(&mut session, workspace);
        self.active.lock().unwrap().remove(&id);

        info!(
            session = %id, state = ?session.state, exit_code = ?session.exit_code,
            reason = ?session.termination_reason, duration_ms = ?session.duration_ms,
            "session finished"
        );
        observer(&session);
        session
    }

    fn set_state(session: &mut RuntimeSession, to: SessionState, observer: StateObserver<'_>) {
        if session.transition(to) {
            observer(session);
        }
    }

    fn fail(session: &mut RuntimeSession, message: impl Into<String>, observer: StateObserver<'_>) {
        let message = message.into();
        warn!(session = %session.id, error = %message, "session failed");
        session.error = Some(message);
        Self::set_state(session, SessionState::Failed, observer);
    }

    /// Returns the workspace (if one was created) so the caller can clean it up.
    fn execute(
        &self,
        session: &mut RuntimeSession,
        request: &RunRequest,
        cancel: &CancelToken,
        observer: StateObserver<'_>,
    ) -> Option<Workspace> {
        // --- validate ---------------------------------------------------------
        if let Some(err) = request.files.iter().find_map(|f| f.validate().err()) {
            Self::fail(session, err, observer);
            return None;
        }
        let sources: Vec<String> =
            request.files.iter().filter(|f| f.is_translation_unit()).map(|f| f.name.clone()).collect();
        if sources.is_empty() {
            Self::fail(session, "the project has no .cpp source file to compile", observer);
            return None;
        }
        let compiler: Arc<dyn Compiler> = match self.toolchain.lock().unwrap().preferred() {
            Some(c) => c,
            None => {
                Self::fail(
                    session,
                    self.toolchain_status().hint.unwrap_or_else(|| "no C++ compiler available".into()),
                    observer,
                );
                return None;
            }
        };
        session.compiler = Some(compiler.info().clone());

        // --- workspace --------------------------------------------------------
        let workspace = match Workspace::create(&self.config.runtime_root, &session.id)
            .and_then(|w| w.write_sources(&request.files).map(|_| w))
        {
            Ok(w) => w,
            Err(e) => {
                Self::fail(session, format!("could not prepare the build workspace: {e}"), observer);
                return None;
            }
        };
        session.workspace_path = Some(workspace.root.clone());

        // --- instrumentation boundary (currently a no-op) ----------------------
        let mut plan = match self.instrumenter.prepare(&workspace) {
            Ok(p) => p,
            Err(e) => {
                Self::fail(session, format!("instrumentation failed: {e}"), observer);
                return Some(workspace);
            }
        };

        // --- compile ------------------------------------------------------------
        Self::set_state(session, SessionState::Compiling, observer);
        let exe = workspace.executable_path();
        info!(session = %session.id, compiler = ?session.compiler.as_ref().map(|c| c.kind), "compilation started");
        let compile = compiler.compile(
            &CompileRequest {
                sources,
                extra_args: plan.extra_compile_args.clone(),
                working_dir: workspace.source_dir.clone(),
                output: exe.clone(),
                cxx_standard: self.config.cxx_standard.clone(),
                timeout: self.config.compile_timeout,
                max_output_bytes: self.config.max_output_bytes,
            },
            cancel,
        );
        let compile = match compile {
            Ok(c) => c,
            Err(e) => {
                Self::fail(session, e.to_string(), observer);
                return Some(workspace);
            }
        };
        session.compile_duration_ms = Some(compile.duration.as_millis() as u64);
        session.compiler_output = compile.raw_output.clone();
        session.diagnostics = compile.diagnostics.clone();
        for d in session.diagnostics.iter().filter(|d| d.severity != Severity::Note) {
            log_diagnostic(&session.id, d);
        }
        match compile.outcome {
            CompileOutcome::Succeeded => {}
            CompileOutcome::Failed => {
                Self::set_state(session, SessionState::CompilationFailed, observer);
                return Some(workspace);
            }
            CompileOutcome::TimedOut => {
                session.diagnostics.push(synthetic_error(format!(
                    "Compilation exceeded the {} s time limit and was terminated.",
                    self.config.compile_timeout.as_secs()
                )));
                Self::set_state(session, SessionState::CompilationFailed, observer);
                return Some(workspace);
            }
            CompileOutcome::Cancelled => {
                session.termination_reason = Some(TerminationReason::UserRequested);
                Self::set_state(session, SessionState::Terminated, observer);
                return Some(workspace);
            }
        }
        session.executable_path = Some(exe.clone());
        Self::set_state(session, SessionState::Ready, observer);

        // --- run ----------------------------------------------------------------
        if cancel.is_cancelled() {
            session.termination_reason = Some(TerminationReason::UserRequested);
            Self::set_state(session, SessionState::Terminated, observer);
            return Some(workspace);
        }
        Self::set_state(session, SessionState::Running, observer);

        let mut env = std::mem::take(&mut plan.run_env);
        if let Some(path) = path_with(compiler.runtime_path_dirs()) {
            env.entry("PATH".into()).or_insert(path);
        }
        let outcome = run_process(
            &ProcessSpec {
                program: exe,
                args: Vec::new(),
                cwd: workspace.executable_dir.clone(),
                env,
                timeout: self.config.run_timeout,
                max_output_bytes: self.config.max_output_bytes,
            },
            cancel,
        );
        if let Some(stream) = plan.event_stream.as_mut() {
            stream.close();
        }
        match outcome {
            Err(e) => {
                session.termination_reason = Some(TerminationReason::LaunchFailed);
                Self::fail(session, format!("could not start the program: {e}"), observer);
            }
            Ok(out) => {
                session.stdout = out.stdout;
                session.stderr = out.stderr;
                session.stdout_truncated = out.stdout_truncated;
                session.stderr_truncated = out.stderr_truncated;
                session.run_duration_ms = Some(out.duration.as_millis() as u64);
                let (state, reason, code) = match out.status {
                    ProcessStatus::Exited(0) => (SessionState::Completed, TerminationReason::Exited, Some(0)),
                    ProcessStatus::Exited(c) => (SessionState::Failed, TerminationReason::Exited, Some(c)),
                    ProcessStatus::TimedOut => (SessionState::TimedOut, TerminationReason::Timeout, None),
                    ProcessStatus::Cancelled => (SessionState::Terminated, TerminationReason::UserRequested, None),
                };
                if state == SessionState::TimedOut {
                    warn!(session = %session.id, limit_ms = session.run_timeout_ms, "execution timed out");
                }
                session.exit_code = code;
                session.termination_reason = Some(reason);
                Self::set_state(session, state, observer);
            }
        }
        Some(workspace)
    }

    fn cleanup(&self, session: &mut RuntimeSession, workspace: Option<Workspace>) {
        let Some(ws) = workspace else { return };
        let keep = match self.config.cleanup {
            CleanupPolicy::Always => false,
            CleanupPolicy::Never => true,
            CleanupPolicy::KeepOnFailure => session.state != SessionState::Completed,
        };
        if keep {
            info!(session = %session.id, path = %ws.root.display(), "workspace kept");
            return;
        }
        session.workspace_removed = ws.remove();
        info!(session = %session.id, removed = session.workspace_removed, "workspace cleanup");
    }
}

fn synthetic_error(message: String) -> CompilerDiagnostic {
    CompilerDiagnostic {
        severity: Severity::Error,
        file: None,
        line: None,
        column: None,
        message,
        context: String::new(),
    }
}

fn log_diagnostic(session: &SessionId, d: &CompilerDiagnostic) {
    // Message text only; never the source excerpt.
    info!(session = %session, severity = ?d.severity, file = ?d.file, line = ?d.line, message = %d.message, "compiler diagnostic");
}

/// `dirs` prepended to the current `PATH`.
fn path_with(dirs: Vec<std::path::PathBuf>) -> Option<String> {
    if dirs.is_empty() {
        return None;
    }
    let mut all = dirs;
    if let Some(existing) = std::env::var_os("PATH") {
        all.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(all).ok().map(|p| p.to_string_lossy().into_owned())
}
