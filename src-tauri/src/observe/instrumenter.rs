//! `ObservingInstrumenter`: the [`Instrumenter`] that makes a build observable.
//!
//! It rewrites the workspace's copies of the sources (never the user's own), adds
//! the lattice-runtime to the build through `BuildPlan`, and arranges for the
//! program's events to come back as a URR [`Timeline`]: live over a named pipe
//! while the program runs, or from a file afterwards.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tracing::{info, warn};

use super::analysis::{analyze_project, norm_path, Analysis};
use super::discovery::analysis_args;
use super::libclang::Clang;
use super::pipe::PipeServer;
use super::receiver::Ingest;
use super::rewrite::instrument;
use crate::model::Timeline;
use crate::runtime::instrumentation::{BuildPlan, Instrumenter, RuntimeEventStream};
use crate::runtime::workspace::Workspace;

const RUNTIME_HEADER: &str = include_str!("runtime_support/lattice_runtime.h");
const RUNTIME_SOURCE: &str = include_str!("runtime_support/lattice_runtime.cpp");

/// Environment variable through which the runtime learns where to send events.
/// A path: a file or a named pipe. The runtime does not care which.
pub const SINK_ENV: &str = "LATTICE_EVENT_SINK";

/// How events travel from the program to Lattice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// A Windows named pipe: events arrive while the program runs. The default on Windows.
    Pipe,
    /// A file in the session workspace, read after the program exits.
    File,
}

impl Default for Transport {
    fn default() -> Self {
        if cfg!(windows) {
            Transport::Pipe
        } else {
            Transport::File
        }
    }
}

/// What the instrumenter did to the sources.
#[derive(Debug, Clone, Default)]
pub struct InstrumentationSummary {
    pub files: usize,
    /// Project headers instrumented (in addition to `files`, the translation units).
    pub headers: usize,
    pub records: usize,
    pub news: usize,
    pub deletes: usize,
    pub assigns: usize,
    pub functions: usize,
    pub locals: usize,
    /// Set when the sources were left untouched (and why).
    pub skipped: Option<String>,
}

/// The outcome of one observed run.
pub struct Observation {
    pub timeline: Timeline,
    /// Problems with the event stream itself; empty for a healthy run.
    pub issues: Vec<String>,
    pub events_read: usize,
    pub summary: InstrumentationSummary,
}

pub(super) type Results = Arc<Mutex<HashMap<PathBuf, Observation>>>;
pub(super) type Progress = Arc<Mutex<HashMap<PathBuf, Arc<AtomicUsize>>>>;

pub struct ObservingInstrumenter {
    libclang: PathBuf,
    args: Vec<String>,
    transport: Transport,
    /// Stop recording after this many events (0 = no limit).
    event_limit: u64,
    results: Results,
    progress: Progress,
}

impl ObservingInstrumenter {
    /// `compiler` is the compiler the build will use (analysis mirrors its target
    /// and headers); `libclang` is the analysis frontend.
    pub fn new(libclang: PathBuf, compiler: &Path, cxx_standard: &str) -> Self {
        Self {
            libclang,
            args: analysis_args(compiler, cxx_standard),
            transport: Transport::default(),
            event_limit: 0,
            results: Arc::new(Mutex::new(HashMap::new())),
            progress: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// An instrumenter that publishes into stores shared with others (the
    /// [`ObservationService`](super::ObservationService) creates one per run).
    pub(super) fn with_shared(mut self, results: Results, progress: Progress) -> Self {
        self.results = results;
        self.progress = progress;
        self
    }

    /// Record at most `limit` events; the runtime then marks the stream truncated
    /// and stops observing (0 = unlimited).
    pub fn with_event_limit(mut self, limit: u64) -> Self {
        self.event_limit = limit;
        self
    }

    pub fn with_transport(mut self, transport: Transport) -> Self {
        self.transport = transport;
        self
    }

    /// Events applied so far for the session whose workspace is `workspace_root`.
    /// Readable *while the program runs* (with the pipe transport); `None` if
    /// unknown or observation was skipped for that session.
    pub fn progress(&self, workspace_root: &Path) -> Option<usize> {
        self.progress.lock().unwrap().get(workspace_root).map(|c| c.load(Ordering::Relaxed))
    }

    /// The observation of the session whose workspace was `workspace_root`
    /// (`RuntimeSession::workspace_path`), available once the run has ended.
    pub fn take_observation(&self, workspace_root: &Path) -> Option<Observation> {
        self.progress.lock().unwrap().remove(workspace_root);
        self.results.lock().unwrap().remove(workspace_root)
    }
}

/// The project's own headers in the workspace: [`norm_path`] -> (path, text).
fn project_headers(dir: &Path) -> Result<HashMap<String, (PathBuf, String)>, String> {
    let mut out = HashMap::new();
    for entry in std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten() {
        let path = entry.path();
        let is_header = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| ["h", "hpp", "hh"].iter().any(|x| x.eq_ignore_ascii_case(e)));
        if is_header {
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            out.insert(norm_path(&path.to_string_lossy()), (path, text));
        }
    }
    Ok(out)
}

fn tally(summary: &mut InstrumentationSummary, a: &Analysis) {
    summary.records += a.records.len();
    summary.news += a.news.len();
    summary.deletes += a.deletes.len();
    summary.assigns += a.assigns.len() + a.postfixes.len();
    summary.functions += a.functions.len();
    summary.locals += a.locals.iter().map(|l| l.vars.len()).sum::<usize>()
        + a.fors.iter().map(|f| f.vars.len()).sum::<usize>()
        + a.range_fors.len();
}

fn translation_units(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| ["cpp", "cc", "cxx"].iter().any(|x| x.eq_ignore_ascii_case(e)))
        })
        .collect();
    files.sort();
    Ok(files)
}

impl Instrumenter for ObservingInstrumenter {
    fn prepare(&self, ws: &Workspace) -> Result<BuildPlan, String> {
        let clang = Clang::load(&self.libclang)?;
        let mut summary = InstrumentationSummary::default();
        let mut rewritten: Vec<(PathBuf, String)> = Vec::new();

        let headers = project_headers(&ws.source_dir)?;
        let header_text: HashMap<String, String> = headers.iter().map(|(k, (_, t))| (k.clone(), t.clone())).collect();
        // A header included by several units is rewritten once: the first analysis wins.
        let mut headers_done: HashSet<String> = HashSet::new();

        for path in translation_units(&ws.source_dir)? {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string();
            let source = std::fs::read_to_string(&path).map_err(|e| format!("{name}: {e}"))?;
            let project = analyze_project(&clang, &path, &source, &self.args, &header_text)?;
            let a = project.main;
            if let Some(first) = a.errors.first() {
                // Do not instrument a program that does not compile: the real
                // compiler will report the user's error in its own words.
                warn!(file = %name, "analysis found errors; leaving the program uninstrumented");
                summary.skipped = Some(format!("not instrumented: {first}"));
                self.results.lock().unwrap().insert(
                    ws.root.clone(),
                    Observation { timeline: Timeline::new(), issues: vec![], events_read: 0, summary: summary.clone() },
                );
                return Ok(BuildPlan::default());
            }
            summary.files += 1;
            tally(&mut summary, &a);
            rewritten.push((path, instrument(&source, &name, &a)));
            for (key, ha) in project.headers {
                if !headers_done.insert(key.clone()) {
                    continue;
                }
                let Some((hpath, htext)) = headers.get(&key) else { continue };
                let hname = hpath.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                summary.headers += 1;
                tally(&mut summary, &ha);
                rewritten.push((hpath.clone(), instrument(htext, hname, &ha)));
            }
        }
        info!(?summary, libclang = %clang.version, "instrumentation planned");

        // Commit only after every file analysed cleanly.
        for (path, text) in &rewritten {
            std::fs::write(path, text).map_err(|e| e.to_string())?;
        }
        let rt_dir = ws.build_dir.join("lattice-runtime");
        std::fs::create_dir_all(&rt_dir).map_err(|e| e.to_string())?;
        let header = rt_dir.join("lattice_runtime.h");
        let runtime_cpp = rt_dir.join("lattice_runtime.cpp");
        std::fs::write(&header, RUNTIME_HEADER).map_err(|e| e.to_string())?;
        std::fs::write(&runtime_cpp, RUNTIME_SOURCE).map_err(|e| e.to_string())?;

        let applied = Arc::new(AtomicUsize::new(0));
        let ingest = Arc::new(Mutex::new(Ingest::new(applied.clone())));
        let (sink, source) = match self.transport {
            Transport::Pipe => {
                // One pipe per session; the workspace directory name is unique.
                let id = ws.root.file_name().and_then(|n| n.to_str()).unwrap_or("session");
                let name = format!("\\\\.\\pipe\\lattice-{id}-{}", std::process::id());
                let server = PipeServer::start(name.clone(), ingest.clone())
                    .map_err(|e| format!("could not create the event pipe: {e}"))?;
                (name, Source::Pipe(server))
            }
            Transport::File => {
                let path = ws.output_dir.join("events.jsonl");
                (path.to_string_lossy().into_owned(), Source::File(path))
            }
        };
        self.progress.lock().unwrap().insert(ws.root.clone(), applied);

        let mut plan = BuildPlan::default();
        plan.extra_compile_args = vec![
            "-I".into(),
            rt_dir.to_string_lossy().into_owned(),
            "-include".into(),
            header.to_string_lossy().into_owned(),
            runtime_cpp.to_string_lossy().into_owned(),
        ];
        plan.run_env.insert(SINK_ENV.into(), sink);
        if self.event_limit > 0 {
            plan.run_env.insert("LATTICE_EVENT_LIMIT".into(), self.event_limit.to_string());
        }
        plan.event_stream = Some(Box::new(EventStream {
            source,
            ingest,
            root: ws.root.clone(),
            summary,
            results: self.results.clone(),
        }));
        Ok(plan)
    }
}

enum Source {
    Pipe(PipeServer),
    File(PathBuf),
}

/// The receiving end for one run. The manager closes it after the program exits;
/// that is when the stream is finalized and the observation published. (With the
/// pipe transport the events have been arriving all along.)
struct EventStream {
    source: Source,
    ingest: Arc<Mutex<Ingest>>,
    root: PathBuf,
    summary: InstrumentationSummary,
    results: Results,
}

impl RuntimeEventStream for EventStream {
    fn close(&mut self) {
        match &mut self.source {
            Source::Pipe(server) => {
                if !server.finish() {
                    self.ingest.lock().unwrap().issues.push(
                        "the event stream did not close after the program exited; later events may be missing".into(),
                    );
                }
            }
            Source::File(path) => {
                let mut ingest = self.ingest.lock().unwrap();
                match std::fs::read(&*path) {
                    Ok(bytes) => ingest.feed(&bytes),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => ingest.issues.push(format!("could not read the event stream: {e}")),
                }
            }
        }
        let mut ingest = self.ingest.lock().unwrap();
        ingest.finish();
        if !ingest.issues.is_empty() {
            warn!(issues = ?ingest.issues, "event stream problems");
        }
        info!(events = ingest.timeline.len(), "runtime events received");
        // Move the finished timeline out; the stream is closed and will not be fed again.
        let applied = Arc::new(AtomicUsize::new(ingest.timeline.len()));
        let done = std::mem::replace(&mut *ingest, Ingest::new(applied));
        drop(ingest);
        self.results.lock().unwrap().insert(
            self.root.clone(),
            Observation {
                events_read: done.lines,
                timeline: done.timeline,
                issues: done.issues,
                summary: self.summary.clone(),
            },
        );
    }
}
