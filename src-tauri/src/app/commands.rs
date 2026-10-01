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
        let mut session = manager.run(request, &|s: &RuntimeSession| {
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
/// visualization: frames, variables, heap objects and their pointers.
#[tauri::command]
pub async fn observation_graph(
    state: State<'_, AppState>,
    session_id: String,
    step: u64,
) -> Result<crate::viz::GraphView, String> {
    let kept = state.last_observation.lock().unwrap().clone();
    match kept {
        Some((id, recording)) if id == session_id => {
            tauri::async_runtime::spawn_blocking(move || observe::graph_at(&recording, step))
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
