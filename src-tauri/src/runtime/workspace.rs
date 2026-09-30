//! Per-session temporary workspace and user source management.
//!
//! ```text
//! <runtime root>/<session id>/
//!     source/       user files, written verbatim
//!     build/        intermediate compiler output
//!     executable/   the produced program; also its working directory when run
//!     output/       reserved for files the program or future tooling produces
//! ```
//!
//! Nothing is ever written to the user's own project directory.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tracing::{info, warn};

use super::session::SessionId;

/// One user-provided source file. `name` is a bare file name, never a path.
#[derive(Debug, Clone)]
pub struct SourceFile {
    pub name: String,
    pub contents: String,
}

const COMPILED_EXTENSIONS: [&str; 3] = ["cpp", "cc", "cxx"];
const OTHER_EXTENSIONS: [&str; 3] = ["h", "hpp", "hh"];

impl SourceFile {
    /// Reject names that could escape the workspace or that aren't C++ sources.
    pub fn validate(&self) -> Result<(), String> {
        let n = &self.name;
        if n.is_empty() || n.contains(['/', '\\', ':', '\0']) || n == "." || n == ".." || n.starts_with('-') {
            return Err(format!("invalid source file name: {n:?}"));
        }
        if !self.is_translation_unit() && !self.has_extension(&OTHER_EXTENSIONS) {
            return Err(format!("unsupported source file type: {n:?}"));
        }
        Ok(())
    }

    /// Whether this file is passed to the compiler (headers are only written to disk).
    pub fn is_translation_unit(&self) -> bool {
        self.has_extension(&COMPILED_EXTENSIONS)
    }

    fn has_extension(&self, exts: &[&str]) -> bool {
        Path::new(&self.name)
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| exts.iter().any(|x| x.eq_ignore_ascii_case(e)))
    }
}

#[derive(Debug)]
pub struct Workspace {
    pub root: PathBuf,
    pub source_dir: PathBuf,
    pub build_dir: PathBuf,
    pub executable_dir: PathBuf,
    pub output_dir: PathBuf,
}

impl Workspace {
    pub fn create(runtime_root: &Path, id: &SessionId) -> io::Result<Self> {
        let root = runtime_root.join(id.as_str());
        let ws = Self {
            source_dir: root.join("source"),
            build_dir: root.join("build"),
            executable_dir: root.join("executable"),
            output_dir: root.join("output"),
            root,
        };
        for d in [&ws.source_dir, &ws.build_dir, &ws.executable_dir, &ws.output_dir] {
            fs::create_dir_all(d)?;
        }
        info!(session = %id, "workspace created");
        Ok(ws)
    }

    pub fn write_sources(&self, files: &[SourceFile]) -> io::Result<()> {
        for f in files {
            fs::write(self.source_dir.join(&f.name), &f.contents)?;
        }
        Ok(())
    }

    pub fn executable_path(&self) -> PathBuf {
        let name = if cfg!(windows) { "program.exe" } else { "program" };
        self.executable_dir.join(name)
    }

    /// Delete the whole workspace. Returns whether it is gone.
    pub fn remove(&self) -> bool {
        match fs::remove_dir_all(&self.root) {
            Ok(()) => true,
            Err(e) if e.kind() == io::ErrorKind::NotFound => true,
            Err(e) => {
                warn!(error = %e, "could not remove workspace");
                false
            }
        }
    }
}

/// Remove leftover session directories older than `max_age` (crashed or
/// force-killed runs). Only touches directories named like session ids.
pub fn sweep_stale(runtime_root: &Path, max_age: Duration) -> usize {
    let Ok(entries) = fs::read_dir(runtime_root) else { return 0 };
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(SessionId::PREFIX) || !entry.path().is_dir() {
            continue;
        }
        let age = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| now.duration_since(t).ok());
        if age.is_some_and(|a| a >= max_age) && fs::remove_dir_all(entry.path()).is_ok() {
            removed += 1;
        }
    }
    if removed > 0 {
        info!(removed, "stale workspaces removed");
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(name: &str) -> SourceFile {
        SourceFile { name: name.into(), contents: String::new() }
    }

    #[test]
    fn validates_names() {
        assert!(f("main.cpp").validate().is_ok());
        assert!(f("util.HPP").validate().is_ok());
        for bad in ["", "..", "../x.cpp", "a\\b.cpp", "C:x.cpp", "-o.cpp", "notes.txt", "main"] {
            assert!(f(bad).validate().is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn only_cpp_files_are_compiled() {
        assert!(f("a.cpp").is_translation_unit());
        assert!(!f("a.h").is_translation_unit());
    }
}
