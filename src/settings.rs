//! Small persisted user settings.

use crate::config::{settings_file_path, write_atomic};
use crate::error::AppResult;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Fetch the online (API) list every time the app starts. Off by default:
    /// the app is fully usable without the API.
    pub auto_update_on_start: bool,
    pub with_ping: bool,
    pub last_adapter: Option<String>,
}

impl Settings {
    pub fn load() -> Self {
        Self::load_from(&settings_file_path())
    }

    pub fn save(&self) -> AppResult<()> {
        self.save_to(&settings_file_path())
    }

    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save_to(&self, path: &Path) -> AppResult<()> {
        let data = serde_json::to_string_pretty(self)?;
        write_atomic(path, &data)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_file_missing_or_partial() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        assert_eq!(Settings::load_from(&path), Settings::default());

        std::fs::write(&path, r#"{ "with_ping": true }"#).unwrap();
        let s = Settings::load_from(&path);
        assert!(s.with_ping);
        assert!(!s.auto_update_on_start);
        assert!(s.last_adapter.is_none());
    }

    #[test]
    fn round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let s = Settings {
            auto_update_on_start: true,
            with_ping: false,
            last_adapter: Some("Ethernet".into()),
        };
        s.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path), s);
    }
}
