//! Language-server integration: runs `clangd` and bridges its stdio JSON-RPC
//! stream to the webview.
//!
//! The backend is a dumb pipe on purpose. It finds and launches clangd, frames
//! and forwards messages, and reports when the server dies. All LSP semantics
//! (initialize, completion, diagnostics, ...) live in the frontend client, so
//! the two sides cannot drift apart.

pub mod discovery;
pub mod framing;
pub mod uri;

use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tracing::{info, warn};

use crate::config::settings::Settings;
use crate::config::{self, editor_workspace_dir};
use crate::process::{run_process, CancelToken, ProcessSpec, ProcessStatus};
use crate::toolchain::{CompilerInfo, CompilerKind};
use discovery::{ClangdSpec, ClangdStatus};

/// One JSON-RPC message from clangd, as a UTF-8 string.
pub const MESSAGE_EVENT: &str = "lattice://lsp-message";
/// Emitted with [`ExitEvent`] when clangd stops on its own.
pub const EXIT_EVENT: &str = "lattice://lsp-exit";

const STDERR_TAIL_LINES: usize = 20;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExitEvent {
    pub code: Option<i32>,
    /// Last lines clangd wrote to stderr, to explain a crash.
    pub stderr: String,
}

/// Where the editor document lives, as seen by clangd.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LspSession {
    pub root_uri: String,
    pub document_uri: String,
    pub clangd_version: String,
}

struct Running {
    generation: u64,
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
}

struct Shared {
    running: Mutex<Option<Running>>,
    generation: AtomicU64,
}

pub struct LspManager {
    shared: Arc<Shared>,
    settings_path: PathBuf,
    settings: Mutex<Settings>,
    status: Mutex<Option<ClangdStatus>>,
}

impl LspManager {
    pub fn new(settings_path: PathBuf) -> Self {
        let settings = Settings::load(&settings_path);
        Self {
            shared: Arc::new(Shared { running: Mutex::new(None), generation: AtomicU64::new(0) }),
            settings_path,
            settings: Mutex::new(settings),
            status: Mutex::new(None),
        }
    }

    fn discover(&self) -> ClangdStatus {
        let configured = self.settings.lock().unwrap().clangd_path.clone().map(PathBuf::from);
        let status = discovery::resolve(&ClangdSpec::from_environment(configured));
        *self.status.lock().unwrap() = Some(status.clone());
        status
    }

    /// Cached discovery result; probes on first use.
    pub fn status(&self) -> ClangdStatus {
        let cached = self.status.lock().unwrap().clone();
        cached.unwrap_or_else(|| self.discover())
    }

    pub fn rescan(&self) -> ClangdStatus {
        self.discover()
    }

    /// Save (or with `None`/blank, clear) the clangd path. A path that is not
    /// a working clangd is rejected without touching the saved setting.
    pub fn set_path(&self, path: Option<String>) -> Result<ClangdStatus, String> {
        let path = path.map(|p| p.trim().trim_matches('"').to_string()).filter(|p| !p.is_empty());
        if let Some(p) = &path {
            if discovery::probe(Path::new(p)).is_none() {
                return Err(format!("'{p}' is not a working clangd executable (it did not answer `--version` as clangd)."));
            }
        }
        {
            let mut s = self.settings.lock().unwrap();
            s.clangd_path = path;
            s.save(&self.settings_path).map_err(|e| format!("could not save settings: {e}"))?;
        }
        Ok(self.discover())
    }

    /// (Re)start clangd for a single-document workspace. Any running server is stopped first.
    pub fn start(&self, app: AppHandle, file_name: &str, compiler: Option<&CompilerInfo>, cxx_standard: &str) -> Result<LspSession, String> {
        let status = self.status();
        let Some(clangd) = status.clangd else {
            return Err(status.problem.unwrap_or_else(|| "clangd is not available".into()));
        };
        if file_name.is_empty() || file_name.contains(['/', '\\', ':']) {
            return Err(format!("invalid document name: {file_name:?}"));
        }
        self.stop();

        let dir = editor_workspace_dir();
        let doc_path = dir.join(file_name);
        let extra_flags = if discovery::has_resource_dir(&clangd.path) {
            Vec::new()
        } else {
            compiler.map(builtin_header_flags).unwrap_or_default()
        };
        write_workspace(&dir, &doc_path, compiler.map(|c| c.path.as_path()), cxx_standard, &extra_flags)
            .map_err(|e| format!("could not prepare the editor workspace {}: {e}", dir.display()))?;

        let mut cmd = Command::new(&clangd.path);
        cmd.args([
            format!("--compile-commands-dir={}", dir.display()),
            "--background-index=false".into(),
            "--completion-style=detailed".into(),
            "--header-insertion=iwyu".into(),
            "--pch-storage=memory".into(),
            "--log=error".into(),
        ]);
        if let Some(c) = compiler {
            // Lets clangd ask the real compiler for its system include paths, so
            // `std::` resolves against the same standard library the build uses.
            // Forward slashes: the value is a glob, where `\` is an escape character.
            cmd.arg(format!("--query-driver={}", c.path.display().to_string().replace('\\', "/")));
        }
        cmd.current_dir(&dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn().map_err(|e| format!("could not start clangd ({}): {e}", clangd.path.display()))?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        let generation = self.shared.generation.fetch_add(1, Ordering::SeqCst) + 1;
        info!(pid = child.id(), generation, path = %clangd.path.display(), "clangd started");

        // Keep only the tail of stderr; it is shown if clangd dies.
        let tail = Arc::new(Mutex::new(std::collections::VecDeque::<String>::new()));
        {
            let tail = tail.clone();
            thread::spawn(move || {
                use std::io::BufRead;
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    warn!(target: "clangd", "{line}");
                    let mut t = tail.lock().unwrap();
                    if t.len() == STDERR_TAIL_LINES {
                        t.pop_front();
                    }
                    t.push_back(line);
                }
            });
        }

        *self.shared.running.lock().unwrap() =
            Some(Running { generation, child, stdin: Arc::new(Mutex::new(stdin)) });

        let shared = self.shared.clone();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                match framing::read_message(&mut reader) {
                    Ok(Some(body)) => {
                        let _ = app.emit(MESSAGE_EVENT, String::from_utf8_lossy(&body).into_owned());
                    }
                    Ok(None) | Err(_) => break,
                }
            }
            // Report only if this server is still the current one; a deliberate
            // stop or restart bumps the generation first.
            let mut guard = shared.running.lock().unwrap();
            if guard.as_ref().is_some_and(|r| r.generation == generation) {
                let mut dead = guard.take().unwrap();
                drop(guard);
                let code = dead.child.wait().ok().and_then(|s| s.code());
                warn!(?code, "clangd exited unexpectedly");
                thread::sleep(std::time::Duration::from_millis(50)); // let stderr drain
                let stderr = tail.lock().unwrap().iter().cloned().collect::<Vec<_>>().join("\n");
                let _ = app.emit(EXIT_EVENT, ExitEvent { code, stderr });
            }
        });

        Ok(LspSession {
            root_uri: uri::path_to_uri(&dir),
            document_uri: uri::path_to_uri(&doc_path),
            clangd_version: clangd.version,
        })
    }

    /// Frame and write one JSON-RPC message to clangd.
    pub fn send(&self, message: &str) -> Result<(), String> {
        let stdin = {
            let guard = self.shared.running.lock().unwrap();
            guard.as_ref().map(|r| r.stdin.clone())
        };
        let stdin = stdin.ok_or("clangd is not running")?;
        let mut w = stdin.lock().unwrap();
        w.write_all(&framing::encode(message)).and_then(|_| w.flush()).map_err(|e| format!("write to clangd failed: {e}"))
    }

    pub fn stop(&self) {
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
        let running = self.shared.running.lock().unwrap().take();
        if let Some(mut r) = running {
            drop(r.stdin); // clangd exits on stdin EOF; kill covers a hung server
            let _ = r.child.kill();
            let _ = r.child.wait();
            info!("clangd stopped");
        }
    }
}

impl Drop for LspManager {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Prepare the directory clangd treats as the project: the (initially empty)
/// document and a `compile_commands.json` that mirrors the build's flags.
fn write_workspace(dir: &Path, doc: &Path, compiler: Option<&Path>, cxx_standard: &str, extra_flags: &[String]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    if !doc.exists() {
        std::fs::write(doc, "")?;
    }
    // Format Document should match the editor's 4-space indentation, not clang-format's LLVM default.
    let style = dir.join(".clang-format");
    if !style.exists() {
        std::fs::write(style, "BasedOnStyle: LLVM\nIndentWidth: 4\nColumnLimit: 100\n")?;
    }
    let driver = compiler.map(|c| c.display().to_string()).unwrap_or_else(|| "clang++".into());
    let mut arguments = vec![driver, format!("-std={cxx_standard}"), "-Wall".into(), "-Wextra".into()];
    arguments.extend(extra_flags.iter().cloned());
    arguments.push(doc.display().to_string());
    let entry = serde_json::json!([{
        "directory": dir.display().to_string(),
        "file": doc.display().to_string(),
        "arguments": arguments,
    }]);
    std::fs::write(dir.join("compile_commands.json"), serde_json::to_string_pretty(&entry).map_err(std::io::Error::other)?)
}

/// Flags that supply compiler builtin headers (`float.h`, `stdatomic.h`, ...) when clangd has none of its own.
fn builtin_header_flags(compiler: &CompilerInfo) -> Vec<String> {
    let query = |arg: &str| -> Option<String> {
        let spec = ProcessSpec {
            program: compiler.path.clone(),
            args: vec![arg.into()],
            cwd: std::env::temp_dir(),
            env: Default::default(),
            timeout: std::time::Duration::from_secs(10),
            max_output_bytes: 4096,
        };
        let out = run_process(&spec, &CancelToken::new()).ok()?;
        (out.status == ProcessStatus::Exited(0)).then(|| out.stdout.trim().to_string())
    };
    let (flag, is_dir_flag) = match compiler.kind {
        CompilerKind::Clang => ("-print-resource-dir", true),
        CompilerKind::Gcc => ("-print-file-name=include", false),
    };
    let Some(dir) = query(flag).filter(|d| Path::new(d).is_dir()) else {
        warn!("clangd has no builtin headers and the compiler did not provide any; standard headers may report errors");
        return Vec::new();
    };
    info!(%dir, "clangd has no resource dir; using the compiler's builtin headers");
    if is_dir_flag {
        vec![format!("-resource-dir={dir}")]
    } else {
        vec!["-isystem".into(), dir]
    }
}

/// Read a source or header file that clangd pointed at (definition targets in
/// standard-library headers), so the editor can show it in a peek view.
pub fn read_source_file(uri_str: &str) -> Result<String, String> {
    const MAX_BYTES: u64 = 8 * 1024 * 1024;
    const EXTENSIONS: [&str; 12] = ["h", "hh", "hpp", "hxx", "cpp", "cc", "cxx", "c", "inc", "inl", "tcc", "ipp"];
    let path = uri::uri_to_path(uri_str).ok_or_else(|| format!("not a file URI: {uri_str}"))?;
    // Standard library headers (`<vector>`) have no extension; everything else must look like C/C++.
    let ok_type = match path.extension().and_then(|e| e.to_str()) {
        None => true,
        Some(e) => EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)),
    };
    if !ok_type {
        return Err(format!("refusing to read non-C++ file: {}", path.display()));
    }
    let meta = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return Err(format!("{} is not a readable source file", path.display()));
    }
    std::fs::read(&path).map(|b| String::from_utf8_lossy(&b).into_owned()).map_err(|e| format!("{}: {e}", path.display()))
}

/// Default location of the persisted settings file.
pub fn default_settings_path() -> PathBuf {
    config::settings::Settings::default_path()
}
