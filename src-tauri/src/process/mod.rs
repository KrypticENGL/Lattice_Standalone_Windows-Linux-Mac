//! OS process execution: spawn, capture output, enforce a timeout, terminate.
//!
//! This module knows nothing about C++, compilers or sessions. It is the only
//! place (besides the toolchain drivers that call it) that touches OS process
//! APIs.
//!
//! **Not a security sandbox.** Limits here (wall-clock timeout, output cap,
//! process-tree termination) protect Lattice from runaway programs. They do
//! not restrict what a program may read, write, or send over the network.

mod job;

use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use tracing::{debug, info, warn};

const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Cooperative cancellation flag shared between a caller and a running process.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Clone)]
pub struct ProcessSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub timeout: Duration,
    /// Per-stream capture cap in bytes.
    pub max_output_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessStatus {
    /// The process exited on its own with this code.
    Exited(i32),
    TimedOut,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct ProcessOutcome {
    pub status: ProcessStatus,
    pub stdout: String,
    pub stderr: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub duration: Duration,
}

struct Captured {
    text: String,
    truncated: bool,
}

fn drain<R: Read + Send + 'static>(mut reader: R, cap: usize) -> thread::JoinHandle<Captured> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        let mut truncated = false;
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let room = cap.saturating_sub(buf.len());
                    if n > room {
                        truncated = true;
                    }
                    buf.extend_from_slice(&chunk[..n.min(room)]);
                    // Keep reading (and discarding) so the child never blocks on a full pipe.
                }
            }
        }
        Captured { text: String::from_utf8_lossy(&buf).into_owned(), truncated }
    })
}

/// Run a process to completion, timeout, or cancellation. Blocks the calling thread.
/// The whole process tree is terminated on timeout/cancellation.
pub fn run_process(spec: &ProcessSpec, cancel: &CancelToken) -> std::io::Result<ProcessOutcome> {
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args)
        .current_dir(&spec.cwd)
        .envs(&spec.env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let start = Instant::now();
    let mut child = cmd.spawn()?;
    let tree = job::ProcessTree::attach(&child);
    info!(pid = child.id(), program = %spec.program.display(), "process started");

    let out = drain(child.stdout.take().expect("piped"), spec.max_output_bytes);
    let err = drain(child.stderr.take().expect("piped"), spec.max_output_bytes);

    let deadline = start + spec.timeout;
    let status = loop {
        if let Some(s) = child.try_wait()? {
            // A missing code (killed by signal, non-Windows) is reported as -1.
            break ProcessStatus::Exited(s.code().unwrap_or(-1));
        }
        let stop = if cancel.is_cancelled() {
            Some(ProcessStatus::Cancelled)
        } else if Instant::now() >= deadline {
            Some(ProcessStatus::TimedOut)
        } else {
            None
        };
        if let Some(reason) = stop {
            warn!(pid = child.id(), ?reason, "terminating process tree");
            tree.terminate(&mut child);
            let _ = child.wait();
            break reason;
        }
        thread::sleep(POLL_INTERVAL);
    };

    // With the tree dead, both pipes reach EOF and the readers finish.
    let empty = || Captured { text: String::new(), truncated: false };
    let out = out.join().unwrap_or_else(|_| empty());
    let err = err.join().unwrap_or_else(|_| empty());
    let duration = start.elapsed();
    debug!(?status, ?duration, "process finished");
    Ok(ProcessOutcome {
        status,
        stdout: out.text,
        stderr: err.text,
        stdout_truncated: out.truncated,
        stderr_truncated: err.truncated,
        duration,
    })
}
