use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use crate::config;
use crate::runtime::{ExecutionManager, RunRequest, RuntimeSession, SourceFile};
use crate::toolchain::ToolchainStatus;

/// Emitted with a [`RuntimeSession`] snapshot on every session state change.
pub const SESSION_EVENT: &str = "lattice://session-state";

pub struct AppState {
    pub execution: Arc<ExecutionManager>,
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
