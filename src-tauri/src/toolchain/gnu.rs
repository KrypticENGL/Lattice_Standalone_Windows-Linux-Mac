//! Driver for GCC-compatible compilers: `clang++` and `g++`.
//! They share flags and the `file:line:col: severity: message` diagnostic format.

use std::collections::BTreeMap;
use std::path::PathBuf;

use tracing::{info, warn};

use super::{
    CompileError, CompileOutcome, CompileRequest, Compiler, CompilerDiagnostic, CompilerInfo,
    CompilerKind, CompilerResult, Severity,
};
use crate::process::{run_process, CancelToken, ProcessSpec, ProcessStatus};

pub struct GnuCompiler {
    info: CompilerInfo,
}

impl GnuCompiler {
    /// Probe `path` with `--version`. Returns `None` if it doesn't run or doesn't
    /// look like a GCC-compatible driver.
    pub fn probe(path: PathBuf) -> Option<Self> {
        let spec = ProcessSpec {
            program: path.clone(),
            args: vec!["--version".into()],
            cwd: std::env::temp_dir(),
            env: BTreeMap::new(),
            timeout: std::time::Duration::from_secs(15),
            max_output_bytes: 16 * 1024,
        };
        let out = run_process(&spec, &CancelToken::new()).ok()?;
        if out.status != ProcessStatus::Exited(0) {
            return None;
        }
        let version = out.stdout.lines().next()?.trim().to_string();
        let kind = if version.to_ascii_lowercase().contains("clang") {
            CompilerKind::Clang
        } else if version.contains("g++") || version.contains("gcc") || version.contains("GCC") {
            CompilerKind::Gcc
        } else {
            return None;
        };
        Some(Self { info: CompilerInfo { kind, path, version } })
    }
}

impl Compiler for GnuCompiler {
    fn info(&self) -> &CompilerInfo {
        &self.info
    }

    fn compile(&self, req: &CompileRequest, cancel: &CancelToken) -> Result<CompilerResult, CompileError> {
        let mut args = vec![
            format!("-std={}", req.cxx_standard),
            "-O0".into(),
            "-g".into(),
            "-Wall".into(),
            "-Wextra".into(),
            "-fdiagnostics-color=never".into(),
        ];
        args.extend(req.sources.iter().cloned());
        args.extend(req.extra_args.iter().cloned());
        args.push("-o".into());
        args.push(req.output.to_string_lossy().into_owned());

        let spec = ProcessSpec {
            program: self.info.path.clone(),
            args,
            cwd: req.working_dir.clone(),
            env: BTreeMap::new(),
            timeout: req.timeout,
            max_output_bytes: req.max_output_bytes,
        };
        let out = run_process(&spec, cancel).map_err(CompileError::Launch)?;

        let raw_output = format!("{}{}", out.stdout, out.stderr);
        let outcome = match out.status {
            ProcessStatus::Exited(0) => CompileOutcome::Succeeded,
            ProcessStatus::Exited(_) => CompileOutcome::Failed,
            ProcessStatus::TimedOut => CompileOutcome::TimedOut,
            ProcessStatus::Cancelled => CompileOutcome::Cancelled,
        };
        let mut diagnostics = parse_diagnostics(&raw_output);
        if outcome == CompileOutcome::Failed && !diagnostics.iter().any(|d| d.severity == Severity::Error) {
            // e.g. linker errors, which have no file:line:col prefix.
            warn!("compile failed without a parsable error; surfacing raw output");
            diagnostics.push(CompilerDiagnostic {
                severity: Severity::Error,
                file: None,
                line: None,
                column: None,
                message: raw_output.trim().to_string(),
                context: String::new(),
            });
        }
        info!(?outcome, diagnostics = diagnostics.len(), duration = ?out.duration, "compilation finished");
        Ok(CompilerResult { outcome, diagnostics, raw_output, duration: out.duration })
    }

    fn runtime_path_dirs(&self) -> Vec<PathBuf> {
        self.info.path.parent().map(|p| vec![p.to_path_buf()]).unwrap_or_default()
    }
}

const MARKERS: [(&str, Severity); 5] = [
    (": fatal error: ", Severity::Error),
    (": error: ", Severity::Error),
    (": warning: ", Severity::Warning),
    (": note: ", Severity::Note),
    (": remark: ", Severity::Note),
];

/// Parse GCC/Clang text diagnostics. Lines that aren't diagnostics (source
/// excerpts, carets) are attached to the preceding diagnostic as `context`.
pub fn parse_diagnostics(output: &str) -> Vec<CompilerDiagnostic> {
    let mut diags: Vec<CompilerDiagnostic> = Vec::new();
    for line in output.lines() {
        let line = line.trim_end();
        if let Some(d) = parse_line(line) {
            diags.push(d);
        } else if let Some(last) = diags.last_mut() {
            if !line.is_empty() {
                if !last.context.is_empty() {
                    last.context.push('\n');
                }
                last.context.push_str(line);
            }
        }
    }
    diags
}

fn parse_line(line: &str) -> Option<CompilerDiagnostic> {
    let (idx, marker, severity) = MARKERS
        .iter()
        .filter_map(|(m, s)| line.find(m).map(|i| (i, *m, *s)))
        .min_by_key(|(i, _, _)| *i)?;
    let prefix = &line[..idx];
    let message = line[idx + marker.len()..].to_string();

    // Strip up to two trailing numeric components (line, column) from the prefix.
    // Splitting from the right keeps a Windows drive letter ("C:\x.cpp") intact.
    let mut rest = prefix;
    let mut nums: Vec<u32> = Vec::new();
    while nums.len() < 2 {
        match rest.rsplit_once(':') {
            Some((head, tail)) if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) => {
                nums.push(tail.parse().ok()?);
                rest = head;
            }
            _ => break,
        }
    }
    nums.reverse();
    let (line_no, col) = match nums.as_slice() {
        [l, c] => (Some(*l), Some(*c)),
        [l] => (Some(*l), None),
        _ => (None, None),
    };
    // Without a line number the prefix is a tool name ("g++.exe", "clang"), not a file.
    let file = if line_no.is_some() { Some(rest.to_string()) } else { None };
    Some(CompilerDiagnostic { severity, file, line: line_no, column: col, message, context: String::new() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_error_with_position() {
        let d = parse_diagnostics("main.cpp:3:5: error: 'x' was not declared in this scope\n    3 |     x = 1;\n      |     ^\n");
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].severity, Severity::Error);
        assert_eq!(d[0].file.as_deref(), Some("main.cpp"));
        assert_eq!((d[0].line, d[0].column), (Some(3), Some(5)));
        assert!(d[0].context.contains("x = 1;"));
    }

    #[test]
    fn keeps_windows_drive_letter() {
        let d = parse_diagnostics("C:\\src\\a.cpp:10:2: warning: unused variable 'y'");
        assert_eq!(d[0].file.as_deref(), Some("C:\\src\\a.cpp"));
        assert_eq!(d[0].line, Some(10));
        assert_eq!(d[0].severity, Severity::Warning);
    }

    #[test]
    fn tool_prefix_has_no_file() {
        let d = parse_diagnostics("g++.exe: error: nope.cpp: No such file or directory");
        assert_eq!(d[0].file, None);
        assert_eq!(d[0].line, None);
    }
}
