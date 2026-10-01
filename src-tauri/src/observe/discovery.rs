//! Locating the analysis frontend (libclang) and describing the *real* build to it.
//!
//! libclang only analyses; the instrumented program is compiled by the compiler
//! the user's builds already use. So analysis must see the same standard library
//! headers and target as that compiler, which we ask the compiler itself for
//! rather than guessing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::process::{run_process, CancelToken, ProcessSpec, ProcessStatus};

/// Environment variable naming an explicit `libclang.dll`.
pub const LIBCLANG_ENV: &str = "LATTICE_LIBCLANG";

/// Find libclang: explicit override, then `PATH`, then the standard LLVM install.
///
/// Distribution note: official LLVM Windows releases ship `bin/libclang.dll`, so it
/// can be discovered next to an installed LLVM, or bundled with Lattice (Apache-2.0
/// with LLVM exception). Finding one is *not* guaranteed on a user's machine.
pub fn find_libclang() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(LIBCLANG_ENV).map(PathBuf::from) {
        return p.is_file().then_some(p);
    }
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    for var in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(pf) = std::env::var_os(var) {
            dirs.push(Path::new(&pf).join("LLVM").join("bin"));
        }
    }
    dirs.into_iter().map(|d| d.join("libclang.dll")).find(|p| p.is_file())
}

fn capture(program: &Path, args: &[&str]) -> Option<(String, String)> {
    let out = run_process(
        &ProcessSpec {
            program: program.to_path_buf(),
            args: args.iter().map(|s| s.to_string()).collect(),
            cwd: std::env::temp_dir(),
            env: BTreeMap::new(),
            timeout: Duration::from_secs(20),
            max_output_bytes: 1 << 20,
        },
        &CancelToken::new(),
    )
    .ok()?;
    (out.status == ProcessStatus::Exited(0)).then_some((out.stdout, out.stderr))
}

/// Target triple and system include directories of `compiler`, as libclang
/// arguments. Empty if the compiler cannot be queried (analysis then relies on
/// libclang's own defaults).
pub fn build_environment_args(compiler: &Path) -> Vec<String> {
    let mut args = Vec::new();
    if let Some((out, _)) = capture(compiler, &["-dumpmachine"]) {
        let triple = out.trim();
        if !triple.is_empty() {
            args.push(format!("--target={triple}"));
        }
    }
    // `-v` prints the include search list on stderr.
    if let Some((_, err)) = capture(compiler, &["-E", "-x", "c++", "-v", "-"]) {
        let mut inside = false;
        let mut dirs = Vec::new();
        for line in err.lines() {
            if line.starts_with("#include <...>") {
                inside = true;
            } else if line.starts_with("End of search list") {
                break;
            } else if inside {
                let d = line.trim();
                if !d.is_empty() {
                    dirs.push(d.to_string());
                }
            }
        }
        if !dirs.is_empty() {
            args.push("-nostdinc".into());
            args.push("-nostdinc++".into());
            for d in dirs {
                args.push("-isystem".into());
                args.push(d);
            }
        }
    }
    args
}

/// Full libclang argument list for analysing a C++ translation unit the way
/// `compiler` would build it.
pub fn analysis_args(compiler: &Path, cxx_standard: &str) -> Vec<String> {
    // No error limit: with the default (20), errors inside a header libclang
    // cannot digest stop the parse early and truncate the AST of the user's code.
    let mut args =
        vec!["-x".into(), "c++".into(), format!("-std={cxx_standard}"), "-ferror-limit=0".into()];
    args.extend(build_environment_args(compiler));
    args
}
