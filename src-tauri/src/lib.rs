//! Lattice native core.
//!
//! Module boundaries (most are intentionally empty for now):
//! - [`app`]       window/application shell and IPC command surface
//! - [`config`]    application configuration
//! - [`model`]     shared data model (nothing defined yet)
//! - [`runtime`]   future: running user programs and observing their state
//! - [`toolchain`] future: locating and driving C++ compilers

pub mod app;
pub mod config;
pub mod model;
pub mod runtime;
pub mod toolchain;

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![app::commands::app_info])
        .run(tauri::generate_context!())
        .expect("failed to run Lattice");
}
