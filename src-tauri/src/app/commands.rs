use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::config;
use crate::lsp::{self, discovery::ClangdStatus, LspManager, LspSession};
use crate::runtime::{ExecutionManager, RunRequest, RuntimeSession, SourceFile};
use crate::toolchain::ToolchainStatus;

/// Emitted with a [`RuntimeSession`] snapshot on every session state change.
pub const SESSION_EVENT: &str = "lattice://session-state";

pub struct AppState {
    pub execution: Arc<ExecutionManager>,
    pub lsp: Arc<LspManager>,
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
    let request = RunRequest {
        project: request.project,
        files: request
            .files
            .into_iter()
            .map(|f| SourceFile { name: f.name, contents: f.contents })
            .collect(),
    };
    tauri::async_runtime::spawn_blocking(move || {
        manager.run(request, &|s: &RuntimeSession| {
            let _ = app.emit(SESSION_EVENT, s);
        })
    })
    .await
    .map_err(|e| format!("execution worker failed: {e}"))
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
