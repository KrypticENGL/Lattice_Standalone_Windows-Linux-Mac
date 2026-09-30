//! Application configuration. Keep constants and settings here, not inline.

use std::path::PathBuf;
use std::time::Duration;

pub const APP_NAME: &str = "Lattice";

/// When a session's temporary workspace is deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupPolicy {
    /// Always delete the workspace once the session ends (default).
    Always,
    /// Keep the workspace when the session did not complete cleanly (debugging aid).
    KeepOnFailure,
    /// Never delete (debugging aid).
    Never,
}

/// Limits and locations for the execution engine.
#[derive(Debug, Clone)]
pub struct ExecutionConfig {
    /// Root under which `<session id>/` workspaces are created.
    pub runtime_root: PathBuf,
    pub compile_timeout: Duration,
    pub run_timeout: Duration,
    /// Per-stream cap on captured output; the excess is discarded and flagged.
    pub max_output_bytes: usize,
    pub cleanup: CleanupPolicy,
    /// Workspaces older than this are removed by the startup sweep.
    pub stale_workspace_age: Duration,
    /// C++ standard passed to the compiler (`-std=<value>`).
    pub cxx_standard: String,
}

impl ExecutionConfig {
    /// `%LOCALAPPDATA%\Lattice\Runtime` (falls back to the OS temp dir).
    pub fn default_runtime_root() -> PathBuf {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(APP_NAME)
            .join("Runtime")
    }
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            runtime_root: Self::default_runtime_root(),
            compile_timeout: Duration::from_secs(60),
            run_timeout: Duration::from_secs(10),
            max_output_bytes: 1024 * 1024,
            cleanup: CleanupPolicy::Always,
            stale_workspace_age: Duration::from_secs(24 * 3600),
            cxx_standard: "c++20".into(),
        }
    }
}
