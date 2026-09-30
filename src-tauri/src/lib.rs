//! Lattice native core.
//!
//! Module boundaries:
//! - [`app`]       window/application shell and IPC command surface
//! - [`config`]    application configuration and execution limits
//! - [`lsp`]       clangd process bridge for editor language features
//! - [`model`]     shared data model (nothing defined yet)
//! - [`process`]   OS process execution with capture, timeout and tree-kill
//! - [`toolchain`] compiler abstraction and discovery
//! - [`runtime`]   sessions: workspace, compile, run; future instrumentation seam

pub mod app;
pub mod config;
pub mod lsp;
pub mod model;
pub mod process;
pub mod runtime;
pub mod toolchain;

use std::sync::Arc;

use app::commands::AppState;
use config::ExecutionConfig;
use lsp::LspManager;
use runtime::ExecutionManager;

/// Development logging. `LATTICE_LOG` takes tracing filter syntax (default `info`).
fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_env("LATTICE_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("lattice_lib=info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).with_target(true).try_init();
}

pub fn run() {
    init_logging();
    tauri::Builder::default()
        .setup(|app| {
            use tauri::Manager;
            let execution = Arc::new(ExecutionManager::new(ExecutionConfig::default()));
            let sweeper = execution.clone();
            std::thread::spawn(move || {
                sweeper.sweep_stale_workspaces();
            });
            let lsp = Arc::new(LspManager::new(lsp::default_settings_path()));
            app.manage(AppState { execution, lsp });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app::commands::app_info,
            app::commands::run_program,
            app::commands::stop_program,
            app::commands::toolchain_status,
            app::commands::rescan_toolchain,
            app::commands::clangd_status,
            app::commands::clangd_rescan,
            app::commands::clangd_set_path,
            app::commands::clangd_start,
            app::commands::clangd_send,
            app::commands::clangd_stop,
            app::commands::read_source_file,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Lattice");
}
