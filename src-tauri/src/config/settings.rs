//! User-editable settings persisted as JSON under `%LOCALAPPDATA%\Lattice`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Explicit path to the `clangd` executable. `None` means auto-discover.
    pub clangd_path: Option<String>,
}

impl Settings {
    pub fn default_path() -> PathBuf {
        super::app_data_dir().join("settings.json")
    }

    /// A missing or unreadable file yields defaults; settings must never block startup.
    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        fs::write(path, json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_garbage() {
        let dir = std::env::temp_dir().join(format!("lattice-settings-{}", std::process::id()));
        let file = dir.join("settings.json");
        let s = Settings { clangd_path: Some(r"C:\x\clangd.exe".into()) };
        s.save(&file).unwrap();
        assert_eq!(Settings::load(&file), s);
        fs::write(&file, "{ not json").unwrap();
        assert_eq!(Settings::load(&file), Settings::default());
        let _ = fs::remove_dir_all(dir);
    }
}
