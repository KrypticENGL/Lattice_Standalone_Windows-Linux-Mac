//! C++ toolchain abstraction: the [`Compiler`] trait, its request/result types,
//! and compiler discovery.
//!
//! The rest of the application only sees [`Compiler`]; concrete drivers live in
//! submodules (`gnu`: Clang and GCC, which share a command-line and diagnostic
//! format) and can be added or replaced without touching the runtime layer.

pub mod discovery;
pub mod gnu;

use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;

use crate::process::CancelToken;

pub use discovery::{DiscoverySpec, Toolchain, ToolchainStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CompilerKind {
    Clang,
    Gcc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompilerInfo {
    pub kind: CompilerKind,
    pub path: PathBuf,
    /// First line of `--version`.
    pub version: String,
}

#[derive(Debug, Clone)]
pub struct CompileRequest {
    /// Source files, relative to `working_dir` (so diagnostics use short names).
    pub sources: Vec<String>,
    /// Extra compiler arguments, e.g. flags contributed by a future instrumenter.
    pub extra_args: Vec<String>,
    pub working_dir: PathBuf,
    pub output: PathBuf,
    pub cxx_standard: String,
    pub timeout: Duration,
    pub max_output_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    Error,
    Warning,
    Note,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompilerDiagnostic {
    pub severity: Severity,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub message: String,
    /// Source excerpt / caret lines the compiler printed under the message.
    pub context: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CompileOutcome {
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct CompilerResult {
    pub outcome: CompileOutcome,
    pub diagnostics: Vec<CompilerDiagnostic>,
    /// Combined compiler stdout+stderr, for display when diagnostics can't be parsed.
    pub raw_output: String,
    pub duration: Duration,
}

impl CompilerResult {
    pub fn succeeded(&self) -> bool {
        self.outcome == CompileOutcome::Succeeded
    }
}

#[derive(Debug)]
pub enum CompileError {
    /// The compiler could not be started at all.
    Launch(std::io::Error),
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::Launch(e) => write!(f, "could not start the compiler: {e}"),
        }
    }
}

impl std::error::Error for CompileError {}

/// A C++ compiler driver. Implementations must be cheap to share across threads.
pub trait Compiler: Send + Sync {
    fn info(&self) -> &CompilerInfo;

    /// Compile and link `request.sources` into `request.output`.
    /// A compile that merely fails is `Ok` with `outcome == Failed`.
    fn compile(&self, request: &CompileRequest, cancel: &CancelToken) -> Result<CompilerResult, CompileError>;

    /// Directories that must be on `PATH` when running the produced executable
    /// (e.g. MinGW runtime DLLs next to the compiler).
    fn runtime_path_dirs(&self) -> Vec<PathBuf>;
}
