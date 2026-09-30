//! clangd discovery. Nothing is hard-coded to one install. Search order:
//! the path saved in settings, `LATTICE_CLANGD`, `PATH`, then well-known
//! Windows install locations (LLVM, Scoop, MSYS2, JetBrains IDEs' bundled LLVM).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use tracing::{info, warn};

use crate::process::{run_process, CancelToken, ProcessSpec, ProcessStatus};

/// Environment variable naming an explicit clangd executable.
pub const OVERRIDE_ENV: &str = "LATTICE_CLANGD";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ClangdSource {
    Settings,
    Environment,
    Path,
    KnownLocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClangdInfo {
    pub path: PathBuf,
    /// First line of `clangd --version`.
    pub version: String,
    pub source: ClangdSource,
}

#[derive(Debug, Clone)]
pub struct ClangdSpec {
    pub configured: Option<PathBuf>,
    pub env_override: Option<PathBuf>,
    pub path_dirs: Vec<PathBuf>,
    pub extra_dirs: Vec<PathBuf>,
}

impl ClangdSpec {
    pub fn from_environment(configured: Option<PathBuf>) -> Self {
        let path_dirs = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        let mut extra_dirs = Vec::new();
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            let Some(pf) = std::env::var_os(var) else { continue };
            let pf = PathBuf::from(pf);
            extra_dirs.push(pf.join("LLVM").join("bin"));
            // CLion and other JetBrains IDEs bundle LLVM at <IDE>\bin\clang\win\x64\bin.
            if let Ok(entries) = std::fs::read_dir(pf.join("JetBrains")) {
                for e in entries.flatten() {
                    extra_dirs.push(e.path().join("bin").join("clang").join("win").join("x64").join("bin"));
                }
            }
        }
        if let Some(home) = std::env::var_os("USERPROFILE") {
            extra_dirs.push(Path::new(&home).join("scoop").join("apps").join("llvm").join("current").join("bin"));
        }
        for d in [r"C:\msys64\clang64\bin", r"C:\msys64\ucrt64\bin", r"C:\msys64\mingw64\bin"] {
            extra_dirs.push(PathBuf::from(d));
        }
        Self {
            configured,
            env_override: std::env::var_os(OVERRIDE_ENV).filter(|v| !v.is_empty()).map(PathBuf::from),
            path_dirs,
            extra_dirs,
        }
    }
}

/// What the UI needs to know about clangd availability.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClangdStatus {
    pub available: bool,
    pub clangd: Option<ClangdInfo>,
    /// The path saved in settings, if any (shown in the settings field even when invalid).
    pub configured_path: Option<String>,
    /// Why `available` is false, in words fit for display.
    pub problem: Option<String>,
    pub searched: Vec<String>,
}

/// Run `path --version`; `Some(first line)` only if it really is clangd.
pub fn probe(path: &Path) -> Option<String> {
    let spec = ProcessSpec {
        program: path.to_path_buf(),
        args: vec!["--version".into()],
        cwd: std::env::temp_dir(),
        env: BTreeMap::new(),
        timeout: Duration::from_secs(15),
        max_output_bytes: 16 * 1024,
    };
    let out = run_process(&spec, &CancelToken::new()).ok()?;
    if out.status != ProcessStatus::Exited(0) {
        return None;
    }
    let first = out.stdout.lines().next()?.trim().to_string();
    first.to_ascii_lowercase().contains("clangd").then_some(first)
}

/// Whether clangd ships its builtin headers (`<prefix>/lib/clang/<version>/include`).
/// Some redistributions (e.g. JetBrains IDEs' bundled clangd) omit them, which
/// breaks parsing of the standard library unless the compiler's are used instead.
pub fn has_resource_dir(clangd: &Path) -> bool {
    let Some(root) = clangd.parent().and_then(|bin| bin.parent()) else { return false };
    std::fs::read_dir(root.join("lib").join("clang"))
        .map(|entries| entries.flatten().any(|e| e.path().join("include").is_dir()))
        .unwrap_or(false)
}

fn find_in(dirs: &[PathBuf], file: &str) -> Option<PathBuf> {
    dirs.iter().map(|d| d.join(file)).find(|p| p.is_file())
}

pub fn resolve(spec: &ClangdSpec) -> ClangdStatus {
    let configured_path = spec.configured.as_ref().map(|p| p.display().to_string());
    let mut searched = Vec::new();

    let found = |info: ClangdInfo, searched: Vec<String>| ClangdStatus {
        available: true,
        clangd: Some(info),
        configured_path: configured_path.clone(),
        problem: None,
        searched,
    };
    let failure = |problem: String, searched: Vec<String>| ClangdStatus {
        available: false,
        clangd: None,
        configured_path: configured_path.clone(),
        problem: Some(problem),
        searched,
    };

    // An explicit choice is honoured or reported, never silently replaced.
    for (label, candidate, source) in [
        ("the clangd path in Settings", &spec.configured, ClangdSource::Settings),
        (OVERRIDE_ENV, &spec.env_override, ClangdSource::Environment),
    ] {
        let Some(path) = candidate else { continue };
        searched.push(format!("{label}: {}", path.display()));
        return match probe(path) {
            Some(version) => {
                info!(path = %path.display(), %version, "clangd found");
                found(ClangdInfo { path: path.clone(), version, source }, searched)
            }
            None => {
                warn!(path = %path.display(), "configured clangd is not usable");
                failure(format!("{label} ({}) is not a working clangd executable.", path.display()), searched)
            }
        };
    }

    let file = if cfg!(windows) { "clangd.exe" } else { "clangd" };
    searched.push("clangd on PATH".into());
    searched.push("known install locations (LLVM, Scoop, MSYS2, JetBrains IDEs)".into());
    for (dirs, source) in [(&spec.path_dirs, ClangdSource::Path), (&spec.extra_dirs, ClangdSource::KnownLocation)] {
        if let Some(path) = find_in(dirs, file) {
            match probe(&path) {
                Some(version) => {
                    info!(path = %path.display(), %version, "clangd found");
                    return found(ClangdInfo { path, version, source }, searched);
                }
                None => warn!(path = %path.display(), "candidate is not a working clangd"),
            }
        }
    }
    failure(
        "clangd was not found. Install LLVM (https://releases.llvm.org) so clangd is on PATH, \
         or set its location in the clangd settings."
            .into(),
        searched,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> ClangdSpec {
        ClangdSpec { configured: None, env_override: None, path_dirs: vec![], extra_dirs: vec![] }
    }

    #[test]
    fn detects_a_missing_resource_dir() {
        let root = std::env::temp_dir().join(format!("lattice-llvm-{}", std::process::id()));
        let clangd = root.join("bin").join("clangd.exe");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        assert!(!has_resource_dir(&clangd));
        std::fs::create_dir_all(root.join("lib").join("clang").join("21").join("include")).unwrap();
        assert!(has_resource_dir(&clangd));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn nothing_found_explains_itself() {
        let s = resolve(&empty());
        assert!(!s.available);
        assert!(s.problem.unwrap().contains("not found"));
        assert!(!s.searched.is_empty());
    }

    #[test]
    fn bad_configured_path_is_reported_not_replaced() {
        let s = resolve(&ClangdSpec { configured: Some(PathBuf::from(r"Z:\nope\clangd.exe")), ..empty() });
        assert!(!s.available);
        assert_eq!(s.configured_path.as_deref(), Some(r"Z:\nope\clangd.exe"));
        assert!(s.problem.unwrap().contains("Settings"));
    }
}
