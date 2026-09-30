use serde::Serialize;

use crate::config;

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
