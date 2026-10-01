//! End-to-end tests of the execution subsystem against a real compiler.
//! No UI involved. If no compiler is installed the tests skip with a message
//! (set LATTICE_REQUIRE_COMPILER=1 to make that a failure, e.g. in CI).

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lattice_lib::config::{CleanupPolicy, ExecutionConfig};
use lattice_lib::runtime::{
    ExecutionManager, RunRequest, RuntimeSession, SessionState, SourceFile, TerminationReason,
};
use lattice_lib::toolchain::Severity;

const HELLO: &str = include_str!("programs/hello_stdout.cpp");
const STDERR: &str = include_str!("programs/stderr_output.cpp");
const COMPILE_ERROR: &str = include_str!("programs/compile_error.cpp");
const NONZERO: &str = include_str!("programs/nonzero_exit.cpp");
const INFINITE: &str = include_str!("programs/infinite_loop.cpp");
const NO_OUTPUT: &str = include_str!("programs/no_output.cpp");

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn temp_root() -> PathBuf {
    let n = NEXT.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!("lattice-test-{}-{n}", std::process::id()))
}

fn manager(root: PathBuf, timeout: Duration, cleanup: CleanupPolicy) -> Option<ExecutionManager> {
    let m = ExecutionManager::new(ExecutionConfig {
        runtime_root: root,
        run_timeout: timeout,
        cleanup,
        ..ExecutionConfig::default()
    });
    if m.toolchain_status().available {
        return Some(m);
    }
    if std::env::var_os("LATTICE_REQUIRE_COMPILER").is_some() {
        panic!("no C++ compiler available");
    }
    eprintln!("SKIPPED: no C++ compiler found");
    None
}

fn request(src: &str) -> RunRequest {
    RunRequest {
        project: "test".into(),
        files: vec![SourceFile { name: "main.cpp".into(), contents: src.into() }],
        observe: false,
    }
}

fn run(m: &ExecutionManager, src: &str) -> RuntimeSession {
    m.run(request(src), &|_| {})
}

fn default_manager() -> Option<(ExecutionManager, PathBuf)> {
    let root = temp_root();
    manager(root.clone(), Duration::from_secs(10), CleanupPolicy::Always).map(|m| (m, root))
}

#[test]
fn successful_run_captures_stdout() {
    let Some((m, _)) = default_manager() else { return };
    let s = run(&m, HELLO);
    assert_eq!(s.state, SessionState::Completed, "{s:#?}");
    assert_eq!(s.stdout.trim(), "Hello World");
    assert_eq!(s.exit_code, Some(0));
    assert_eq!(s.termination_reason, Some(TerminationReason::Exited));
    assert!(s.stderr.is_empty());
    assert!(s.duration_ms.is_some() && s.ended_at_ms.is_some() && s.run_duration_ms.is_some());
}

#[test]
fn stderr_is_captured_separately() {
    let Some((m, _)) = default_manager() else { return };
    let s = run(&m, STDERR);
    assert_eq!(s.state, SessionState::Completed);
    assert_eq!(s.stderr.trim(), "oops on stderr");
    assert!(s.stdout.is_empty());
}

#[test]
fn compile_failure_reports_diagnostics_and_does_not_run() {
    let Some((m, _)) = default_manager() else { return };
    let s = run(&m, COMPILE_ERROR);
    assert_eq!(s.state, SessionState::CompilationFailed);
    assert!(s.executable_path.is_none());
    assert!(s.exit_code.is_none());
    let err = s.diagnostics.iter().find(|d| d.severity == Severity::Error).expect("an error diagnostic");
    assert_eq!(err.file.as_deref(), Some("main.cpp"));
    assert_eq!(err.line, Some(2));
}

#[test]
fn nonzero_exit_is_failed_with_code() {
    let Some((m, _)) = default_manager() else { return };
    let s = run(&m, NONZERO);
    assert_eq!(s.state, SessionState::Failed);
    assert_eq!(s.exit_code, Some(3));
    assert_eq!(s.termination_reason, Some(TerminationReason::Exited));
    assert_eq!(s.stdout.trim(), "about to fail");
}

#[test]
fn infinite_loop_is_killed_at_timeout() {
    let Some(m) = manager(temp_root(), Duration::from_millis(1500), CleanupPolicy::Always) else { return };
    let t = Instant::now();
    let s = run(&m, INFINITE);
    assert_eq!(s.state, SessionState::TimedOut, "{s:#?}");
    assert_eq!(s.termination_reason, Some(TerminationReason::Timeout));
    assert_eq!(s.exit_code, None);
    assert!(s.run_duration_ms.unwrap() >= 1400);
    assert!(t.elapsed() < Duration::from_secs(30));
    // Output produced before the kill is still delivered.
    assert!(s.stdout.contains("spinning"));
}

#[test]
fn program_with_no_output() {
    let Some((m, _)) = default_manager() else { return };
    let s = run(&m, NO_OUTPUT);
    assert_eq!(s.state, SessionState::Completed);
    assert!(s.stdout.is_empty() && s.stderr.is_empty());
}

#[test]
fn stop_terminates_a_running_program() {
    let Some(m) = manager(temp_root(), Duration::from_secs(60), CleanupPolicy::Always) else { return };
    let m = Arc::new(m);
    let seen: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let handle = {
        let (m, seen) = (m.clone(), seen.clone());
        std::thread::spawn(move || {
            m.run(request(INFINITE), &|s| {
                if s.state == SessionState::Running {
                    *seen.lock().unwrap() = Some(s.id.to_string());
                }
            })
        })
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    let id = loop {
        if let Some(id) = seen.lock().unwrap().clone() {
            break id;
        }
        assert!(Instant::now() < deadline, "session never reached Running");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(m.stop(&id));
    let s = handle.join().unwrap();
    assert_eq!(s.state, SessionState::Terminated);
    assert_eq!(s.termination_reason, Some(TerminationReason::UserRequested));
    assert!(!m.stop(&id), "finished sessions are no longer stoppable");
}

#[test]
fn workspace_is_isolated_and_cleaned_up() {
    let root = temp_root();
    let Some(m) = manager(root.clone(), Duration::from_secs(10), CleanupPolicy::Always) else { return };
    let cwd = std::env::current_dir().unwrap();
    let list = || -> Vec<_> { std::fs::read_dir(&cwd).unwrap().flatten().map(|e| e.file_name()).collect() };
    let before = list();

    let s = run(&m, HELLO);
    let ws = s.workspace_path.clone().expect("workspace path recorded");
    assert!(ws.starts_with(&root), "workspace must live under the runtime root");
    assert!(!ws.starts_with(&cwd));
    assert!(s.workspace_removed && !ws.exists());
    assert_eq!(before, list(), "no build artifacts may appear in the working directory");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn workspace_layout_when_kept() {
    let root = temp_root();
    let Some(m) = manager(root.clone(), Duration::from_secs(10), CleanupPolicy::Never) else { return };
    let s = run(&m, HELLO);
    let ws = s.workspace_path.unwrap();
    for d in ["source", "build", "executable", "output"] {
        assert!(ws.join(d).is_dir(), "missing {d}/");
    }
    assert!(ws.join("source").join("main.cpp").is_file());
    assert!(s.executable_path.unwrap().starts_with(ws.join("executable")));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn state_changes_are_observed_in_order() {
    let Some((m, _)) = default_manager() else { return };
    let states = Mutex::new(Vec::new());
    m.run(request(HELLO), &|s| {
        let mut v = states.lock().unwrap();
        if v.last() != Some(&s.state) {
            v.push(s.state);
        }
    });
    use SessionState::*;
    assert_eq!(*states.lock().unwrap(), vec![Compiling, Ready, Running, Completed]);
}

#[test]
fn invalid_source_names_are_rejected_before_touching_disk() {
    let Some((m, root)) = default_manager() else { return };
    let s = m.run(
        RunRequest {
            project: "t".into(),
            files: vec![SourceFile { name: "..\\evil.cpp".into(), contents: HELLO.into() }],
            observe: false,
        },
        &|_| {},
    );
    assert_eq!(s.state, SessionState::Failed);
    assert!(s.error.is_some());
    assert!(!root.exists());
}

#[test]
fn stale_sweep_only_removes_old_session_dirs() {
    let root = temp_root();
    std::fs::create_dir_all(root.join("s-old")).unwrap();
    std::fs::create_dir_all(root.join("keep-me")).unwrap();
    let removed = lattice_lib::runtime::workspace::sweep_stale(&root, Duration::ZERO);
    assert_eq!(removed, 1);
    assert!(root.join("keep-me").exists());
    let _ = std::fs::remove_dir_all(root);
}
