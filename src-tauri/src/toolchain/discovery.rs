//! Compiler discovery. Nothing is hard-coded to one install: search order is
//! an explicit override, then `clang++`, then `g++`, each looked up on `PATH`
//! and in a few well-known Windows install locations.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use tracing::{debug, info, warn};

use super::gnu::GnuCompiler;
use super::{Compiler, CompilerInfo};

/// Environment variable naming an explicit compiler executable.
pub const OVERRIDE_ENV: &str = "LATTICE_CXX";

/// Where to look. Injectable so discovery can be tested without the real machine.
#[derive(Debug, Clone)]
pub struct DiscoverySpec {
    pub override_path: Option<PathBuf>,
    pub path_dirs: Vec<PathBuf>,
    pub extra_dirs: Vec<PathBuf>,
}

impl DiscoverySpec {
    pub fn from_environment() -> Self {
        let path_dirs = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        let mut extra_dirs = Vec::new();
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(pf) = std::env::var_os(var) {
                extra_dirs.push(Path::new(&pf).join("LLVM").join("bin"));
            }
        }
        if let Some(home) = std::env::var_os("USERPROFILE") {
            extra_dirs.push(Path::new(&home).join("scoop").join("apps").join("llvm").join("current").join("bin"));
        }
        for d in [
            r"C:\msys64\clang64\bin",
            r"C:\msys64\ucrt64\bin",
            r"C:\msys64\mingw64\bin",
            r"C:\ProgramData\mingw64\mingw64\bin",
        ] {
            extra_dirs.push(PathBuf::from(d));
        }
        Self { override_path: std::env::var_os(OVERRIDE_ENV).map(PathBuf::from), path_dirs, extra_dirs }
    }

    fn find(&self, stem: &str) -> Option<PathBuf> {
        let file = if cfg!(windows) { format!("{stem}.exe") } else { stem.to_string() };
        self.path_dirs
            .iter()
            .chain(self.extra_dirs.iter())
            .map(|d| d.join(&file))
            .find(|p| p.is_file())
    }
}

/// The compilers found on this machine, most preferred first.
pub struct Toolchain {
    compilers: Vec<Arc<dyn Compiler>>,
    searched: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolchainStatus {
    pub available: bool,
    pub selected: Option<CompilerInfo>,
    pub candidates: Vec<CompilerInfo>,
    /// What was searched for, shown to the user when nothing was found.
    pub searched: Vec<String>,
    pub hint: Option<String>,
}

impl Toolchain {
    pub fn discover(spec: &DiscoverySpec) -> Self {
        let mut candidates: Vec<PathBuf> = Vec::new();
        let mut searched = Vec::new();
        if let Some(p) = &spec.override_path {
            searched.push(format!("{OVERRIDE_ENV}={}", p.display()));
            candidates.push(p.clone());
        }
        for stem in ["clang++", "g++"] {
            searched.push(format!("{stem} on PATH and known install locations"));
            if let Some(p) = spec.find(stem) {
                candidates.push(p);
            }
        }

        let mut compilers: Vec<Arc<dyn Compiler>> = Vec::new();
        for path in candidates {
            if compilers.iter().any(|c| c.info().path == path) {
                continue;
            }
            match GnuCompiler::probe(path.clone()) {
                Some(c) => {
                    info!(path = %path.display(), version = %c.info().version, kind = ?c.info().kind, "compiler found");
                    compilers.push(Arc::new(c));
                }
                None => warn!(path = %path.display(), "candidate is not a usable compiler"),
            }
        }
        if compilers.is_empty() {
            warn!("no supported C++ compiler found");
        } else {
            debug!(count = compilers.len(), "compiler discovery complete");
        }
        Self { compilers, searched }
    }

    /// The compiler to use: the override if valid, otherwise Clang, otherwise GCC.
    pub fn preferred(&self) -> Option<Arc<dyn Compiler>> {
        self.compilers.first().cloned()
    }

    pub fn status(&self) -> ToolchainStatus {
        let selected = self.compilers.first().map(|c| c.info().clone());
        ToolchainStatus {
            available: selected.is_some(),
            hint: selected.is_none().then(|| {
                format!(
                    "No C++ compiler found. Install LLVM/Clang (https://releases.llvm.org) or MinGW-w64 \
                     and make sure clang++ or g++ is on PATH, or set {OVERRIDE_ENV} to the compiler executable."
                )
            }),
            candidates: self.compilers.iter().map(|c| c.info().clone()).collect(),
            searched: self.searched.clone(),
            selected,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_spec_finds_nothing() {
        let t = Toolchain::discover(&DiscoverySpec { override_path: None, path_dirs: vec![], extra_dirs: vec![] });
        assert!(t.preferred().is_none());
        let s = t.status();
        assert!(!s.available);
        assert!(s.hint.is_some());
    }

    #[test]
    fn bogus_override_is_rejected() {
        let t = Toolchain::discover(&DiscoverySpec {
            override_path: Some(PathBuf::from("Z:\\definitely\\not\\here\\clang++.exe")),
            path_dirs: vec![],
            extra_dirs: vec![],
        });
        assert!(t.preferred().is_none());
    }
}
