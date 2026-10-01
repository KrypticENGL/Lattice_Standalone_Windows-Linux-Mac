use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::config;
use crate::lsp::{self, discovery::ClangdStatus, LspManager, LspSession};
use crate::observe::{self, Observation, ObservationService, ObservationStatus, StepView};
use crate::runtime::{ExecutionManager, RunRequest, RuntimeSession, SourceFile};
use crate::toolchain::ToolchainStatus;

/// Emitted with a [`RuntimeSession`] snapshot on every session state change.
pub const SESSION_EVENT: &str = "lattice://session-state";

pub struct AppState {
    pub execution: Arc<ExecutionManager>,
    pub lsp: Arc<LspManager>,
    pub observation: Arc<ObservationService>,
    /// The most recent observed run (session id, its recording). Only one is kept:
    /// a recording holds every event and object of its run in memory.
    pub last_observation: Mutex<Option<(String, Arc<Observation>)>>,
}

#[derive(Serialize)]
pub struct AppInfo {
    pub name: &'static str,
    pub version: &'static str,
}

#[tauri::command]
pub fn app_info() -> AppInfo {
    AppInfo {
        name: config::APP_NAME,
        version: env!("CARGO_PKG_VERSION"),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceFileDto {
    pub name: String,
    pub contents: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunProgramRequest {
    pub project: String,
    pub files: Vec<SourceFileDto>,
    /// Record the run's runtime state (see `observation_status` for availability).
    #[serde(default)]
    pub observe: bool,
    /// The folder of the open solution; the built executable is kept in its `target` folder.
    #[serde(default)]
    pub solution: Option<String>,
}

/// Compile and run the given sources. Resolves when the session has ended; state
/// changes along the way are emitted as [`SESSION_EVENT`].
#[tauri::command]
pub async fn run_program(
    app: AppHandle,
    state: State<'_, AppState>,
    request: RunProgramRequest,
) -> Result<RuntimeSession, String> {
    let manager = state.execution.clone();
    let service = state.observation.clone();
    let observe = request.observe;
    let target = request
        .solution
        .as_deref()
        .map(std::path::Path::new)
        .filter(|root| crate::solution::exists(root))
        .map(crate::solution::target_dir);
    let request = RunRequest {
        project: request.project,
        files: request
            .files
            .into_iter()
            .map(|f| SourceFile { name: f.name, contents: f.contents })
            .collect(),
        observe,
    };
    let (session, recording) = tauri::async_runtime::spawn_blocking(move || {
        let mut session = manager.run_into(request, target.as_deref(), &|s: &RuntimeSession| {
            let _ = app.emit(SESSION_EVENT, s);
        });
        // The recording of an observed run is ready once the run has ended.
        let recording = if observe {
            session.workspace_path.as_deref().and_then(|root| service.take(root)).map(|obs| {
                session.observation = Some(service.summarize(&obs));
                Arc::new(obs)
            })
        } else {
            None
        };
        (session, recording)
    })
    .await
    .map_err(|e| format!("execution worker failed: {e}"))?;
    // Replace the previous recording (or clear it: it belongs to an older run).
    *state.last_observation.lock().unwrap() = recording.map(|r| (session.id.as_str().to_string(), r));
    Ok(session)
}

/// Whether runs can be observed on this machine, and if not, why and what to do.
#[tauri::command]
pub async fn observation_status(state: State<'_, AppState>) -> Result<ObservationStatus, String> {
    let service = state.observation.clone();
    tauri::async_runtime::spawn_blocking(move || service.status()).await.map_err(|e| e.to_string())
}

/// The recorded run of `session_id` after `step` events, as data for the
/// visualization: frames, variables, heap objects and their pointers. `focus` names
/// objects an inspector is looking at; they are described in `GraphView::focus`.
#[tauri::command]
pub async fn observation_graph(
    state: State<'_, AppState>,
    session_id: String,
    step: u64,
    focus: Option<Vec<u64>>,
) -> Result<crate::viz::GraphView, String> {
    let kept = state.last_observation.lock().unwrap().clone();
    let focus = focus.unwrap_or_default();
    match kept {
        Some((id, recording)) if id == session_id => {
            tauri::async_runtime::spawn_blocking(move || observe::graph_at(&recording, step, &focus))
                .await
                .map_err(|e| e.to_string())
        }
        _ => Err("The recording of that run is no longer available (only the latest observed run is kept).".into()),
    }
}

/// The user-facing timeline of `session_id`: the raw events grouped into steps a
/// reader would recognise (statement, call, return). Presentation only; every
/// step's `start`/`end` are raw step numbers `observation_graph` understands.
#[tauri::command]
pub async fn observation_timeline(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<observe::TimelineStep>, String> {
    let kept = state.last_observation.lock().unwrap().clone();
    match kept {
        Some((id, recording)) if id == session_id => {
            tauri::async_runtime::spawn_blocking(move || observe::aggregate(recording.timeline.events()))
                .await
                .map_err(|e| e.to_string())
        }
        _ => Err("The recording of that run is no longer available (only the latest observed run is kept).".into()),
    }
}

/// The recorded run of `session_id` after `step` events, as text. Fails if that
/// run was not observed or a newer observed run has replaced it.
#[tauri::command]
pub async fn observation_step(
    state: State<'_, AppState>,
    session_id: String,
    step: u64,
) -> Result<StepView, String> {
    let kept = state.last_observation.lock().unwrap().clone();
    match kept {
        Some((id, recording)) if id == session_id => {
            tauri::async_runtime::spawn_blocking(move || observe::view_at(&recording, step))
                .await
                .map_err(|e| e.to_string())
        }
        _ => Err("The recording of that run is no longer available (only the latest observed run is kept).".into()),
    }
}

#[tauri::command]
pub fn stop_program(state: State<'_, AppState>, session_id: String) -> bool {
    state.execution.stop(&session_id)
}

#[tauri::command]
pub fn toolchain_status(state: State<'_, AppState>) -> ToolchainStatus {
    state.execution.toolchain_status()
}

#[tauri::command]
pub async fn rescan_toolchain(state: State<'_, AppState>) -> Result<ToolchainStatus, String> {
    let manager = state.execution.clone();
    tauri::async_runtime::spawn_blocking(move || manager.rescan_toolchain())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn clangd_status(state: State<'_, AppState>) -> Result<ClangdStatus, String> {
    let lsp = state.lsp.clone();
    tauri::async_runtime::spawn_blocking(move || lsp.status()).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn clangd_rescan(state: State<'_, AppState>) -> Result<ClangdStatus, String> {
    let lsp = state.lsp.clone();
    tauri::async_runtime::spawn_blocking(move || lsp.rescan()).await.map_err(|e| e.to_string())
}

/// Save the clangd path (`None` or blank restores auto-discovery). Rejects paths that are not clangd.
#[tauri::command]
pub async fn clangd_set_path(state: State<'_, AppState>, path: Option<String>) -> Result<ClangdStatus, String> {
    let lsp = state.lsp.clone();
    tauri::async_runtime::spawn_blocking(move || lsp.set_path(path)).await.map_err(|e| e.to_string())?
}

/// Start (or restart) clangd. LSP traffic then flows through `clangd_send` and
/// the `lattice://lsp-message` event.
#[tauri::command]
pub async fn clangd_start(app: AppHandle, state: State<'_, AppState>, file_name: String) -> Result<LspSession, String> {
    let lsp = state.lsp.clone();
    let execution = state.execution.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let compiler = execution.compiler_info();
        lsp.start(app, &file_name, compiler.as_ref(), &execution.config().cxx_standard)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn clangd_send(state: State<'_, AppState>, message: String) -> Result<(), String> {
    state.lsp.send(&message)
}

#[tauri::command]
pub fn clangd_stop(state: State<'_, AppState>) {
    state.lsp.stop();
}

#[tauri::command]
pub fn read_source_file(uri: String) -> Result<String, String> {
    lsp::read_source_file(&uri)
}

// ---- solutions (lattice.sln folders) ----

/// The part of a solution the UI owns: what is on screen. The URR state is not sent from
/// the UI; the backend attaches the recording it already holds.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SolutionDto {
    pub name: String,
    pub files: Vec<crate::solution::SolutionSource>,
    pub active_file: Option<String>,
    pub observe: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedSolution {
    /// The solution folder (the one holding `lattice.sln`).
    pub path: String,
    pub name: String,
    /// Events of the recorded run stored with it (0 = no runtime state saved).
    pub urr_events: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedSolution {
    /// The solution folder.
    pub path: String,
    pub name: String,
    pub files: Vec<crate::solution::SolutionSource>,
    pub active_file: Option<String>,
    pub observe: bool,
    /// A session carrying the saved recording, ready for the timeline and visualization.
    pub session: Option<RuntimeSession>,
    /// The saved runtime state was recorded from different sources than the files now hold.
    pub urr_stale: bool,
    /// Why the saved runtime state was not loaded, if the solution had one.
    pub warning: Option<String>,
}

/// Write the current solution into the folder `path` (created if needed), with the recording
/// of `session_id` (the run on screen) as its URR state when that recording is still held.
/// `create` refuses to write over an existing solution (used for a first save).
#[tauri::command]
pub async fn solution_save(
    state: State<'_, AppState>,
    path: String,
    solution: SolutionDto,
    session_id: Option<String>,
    create: bool,
) -> Result<SavedSolution, String> {
    use crate::solution::{self, Solution, SolutionSettings, UrrState, URR_SCHEMA};
    let kept = state.last_observation.lock().unwrap().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let root = std::path::PathBuf::from(&path);
        if create && solution::exists(&root) {
            return Err(format!("{} already contains a Lattice solution.", root.display()));
        }
        let name = solution.name.clone();
        let sln = Solution::new(
            name.clone(),
            solution.files,
            solution.active_file,
            SolutionSettings { observe: solution.observe },
        );
        let urr = match (session_id, kept) {
            (Some(id), Some((kept_id, recording))) if id == kept_id => Some(UrrState {
                schema: URR_SCHEMA,
                events: recording.timeline.events().to_vec(),
                source_hash: solution::hash_sources(&sln.files),
            }),
            _ => None,
        };
        let urr_events = urr.as_ref().map_or(0, |u| u.events.len());
        solution::write(&root, &sln, urr.as_ref())?;
        Ok(SavedSolution { path: root.display().to_string(), name, urr_events })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Open a solution: `path` is its folder or its `lattice.sln`. Its URR state, if any,
/// becomes the current recording (replacing the previous one), exactly as if the program
/// had just been run and observed.
#[tauri::command]
pub async fn solution_open(state: State<'_, AppState>, path: String) -> Result<OpenedSolution, String> {
    use crate::runtime::{SessionId, SessionState};
    let service = state.observation.clone();
    let timeout_ms = config::ExecutionConfig::default().run_timeout.as_millis() as u64;
    let (opened, recording) = tauri::async_runtime::spawn_blocking(move || {
        let root = crate::solution::root_of(std::path::Path::new(&path));
        let (sln, urr) = crate::solution::read(&root)?;
        let name = sln.manifest.name.clone();
        let mut warning = None;
        let mut session = None;
        let mut recording = None;
        let mut urr_stale = false;
        if sln.manifest.has_urr && urr.is_none() {
            warning = Some("the runtime state file is missing or damaged".to_string());
        }
        if let Some(urr) = &urr {
            urr_stale = urr.source_hash != crate::solution::hash_sources(&sln.files);
            match urr.timeline() {
                Ok(timeline) => {
                    let obs = Observation {
                        events_read: timeline.len(),
                        timeline,
                        issues: Vec::new(),
                        summary: Default::default(),
                    };
                    let mut s = RuntimeSession::new(
                        SessionId::new(),
                        name.clone(),
                        sln.files.iter().map(|f| f.name.clone()).collect(),
                        timeout_ms,
                    );
                    s.state = SessionState::Completed;
                    s.observation = Some(service.summarize(&obs));
                    recording = Some((s.id.as_str().to_string(), Arc::new(obs)));
                    session = Some(s);
                }
                Err(e) => warning = Some(e),
            }
        }
        let opened = OpenedSolution {
            path: root.display().to_string(),
            name,
            active_file: sln.manifest.active_file,
            observe: sln.manifest.settings.observe,
            files: sln.files,
            session,
            urr_stale,
            warning,
        };
        Ok::<_, String>((opened, recording))
    })
    .await
    .map_err(|e| e.to_string())??;
    if recording.is_some() {
        *state.last_observation.lock().unwrap() = recording;
    }
    Ok(opened)
}

// ---- file explorer (the in-app open/browse dialogs) ----

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FsEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    /// A folder that holds a `lattice.sln` (or the `lattice.sln` file itself).
    pub is_solution: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FsListing {
    /// The folder listed, normalized.
    pub path: String,
    /// Its parent, if it has one.
    pub parent: Option<String>,
    pub entries: Vec<FsEntry>,
}

#[cfg(windows)]
fn is_hidden(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes() & 0x2 != 0 || meta.file_attributes() & 0x4 != 0
}
#[cfg(not(windows))]
fn is_hidden(_: &std::fs::Metadata) -> bool {
    false
}

/// Folders (and `lattice.sln` files; every file with `all_files`) inside `path`, folders first. Hidden and unreadable
/// entries are skipped; a folder that cannot be read is an error the dialog shows.
#[tauri::command]
pub async fn fs_list_dir(path: String, all_files: Option<bool>) -> Result<FsListing, String> {
    let all_files = all_files.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        let dir = std::fs::canonicalize(&path).map_err(|e| format!("Cannot open {path}: {e}"))?;
        // `canonicalize` yields \?\ paths on Windows; show the ordinary form.
        let shown = |p: &std::path::Path| {
            let s = p.display().to_string();
            s.strip_prefix(r"\?\").map(str::to_string).unwrap_or(s)
        };
        let rd = std::fs::read_dir(&dir).map_err(|e| format!("Cannot read {}: {e}", shown(&dir)))?;
        let mut entries: Vec<FsEntry> = rd
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let meta = e.metadata().ok()?;
                let name = e.file_name().into_string().ok()?;
                if name.starts_with('.') || is_hidden(&meta) {
                    return None;
                }
                let p = e.path();
                if meta.is_dir() {
                    let is_solution = crate::solution::exists(&p);
                    Some(FsEntry { name, path: shown(&p), is_dir: true, is_solution })
                } else if all_files {
                    Some(FsEntry { name, path: shown(&p), is_dir: false, is_solution: false })
                } else if name.eq_ignore_ascii_case(crate::solution::MANIFEST) {
                    Some(FsEntry { name, path: shown(&p), is_dir: false, is_solution: true })
                } else {
                    None
                }
            })
            .collect();
        entries.sort_by(|a, b| {
            b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(FsListing { path: shown(&dir), parent: dir.parent().map(shown), entries })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Drive roots on Windows; `/` elsewhere.
#[tauri::command]
pub fn fs_roots() -> Vec<String> {
    #[cfg(windows)]
    {
        ('A'..='Z')
            .map(|c| format!("{c}:\\"))
            .filter(|d| std::path::Path::new(d).exists())
            .collect()
    }
    #[cfg(not(windows))]
    {
        vec!["/".to_string()]
    }
}

/// Create a folder (used by the explorer's "New folder").
#[tauri::command]
pub fn fs_create_dir(path: String) -> Result<(), String> {
    std::fs::create_dir(&path).map_err(|e| format!("Cannot create {path}: {e}"))
}
