//! Persists, per network adapter, the DNS configuration that was in effect
//! *before* NotroDNS changed it for the first time — so "Restore previous
//! DNS" works even across app restarts.

use crate::config::state_file_path;
use crate::error::AppResult;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct AdapterSnapshot {
    pub dhcp: bool,
    pub ipv4_servers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RestoreState {
    pub snapshots: HashMap<String, AdapterSnapshot>,
}

impl RestoreState {
    pub fn load() -> Self {
        Self::load_from(&state_file_path())
    }

    pub fn save(&self) -> AppResult<()> {
        self.save_to(&state_file_path())
    }

    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save_to(&self, path: &Path) -> AppResult<()> {
        let data = serde_json::to_string_pretty(self)?;
        std::fs::write(path, data)?;
        Ok(())
    }

    /// Records `snapshot` for `adapter` only if nothing is recorded yet, so
    /// repeated "Apply" clicks never overwrite the *original* pre-NotroDNS
    /// configuration.
    pub fn record_if_absent(&mut self, adapter: &str, snapshot: AdapterSnapshot) {
        self.snapshots.entry(adapter.to_string()).or_insert(snapshot);
    }

    pub fn take(&mut self, adapter: &str) -> Option<AdapterSnapshot> {
        self.snapshots.remove(adapter)
    }

    pub fn has(&self, adapter: &str) -> bool {
        self.snapshots.contains_key(adapter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_if_absent_keeps_first_snapshot() {
        let mut state = RestoreState::default();
        state.record_if_absent("Ethernet", AdapterSnapshot { dhcp: true, ipv4_servers: vec![] });
        state.record_if_absent(
            "Ethernet",
            AdapterSnapshot { dhcp: false, ipv4_servers: vec!["1.1.1.1".into()] },
        );
        let snap = state.snapshots.get("Ethernet").unwrap();
        assert!(snap.dhcp);
        assert!(snap.ipv4_servers.is_empty());
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");

        let mut state = RestoreState::default();
        state.record_if_absent(
            "Wi-Fi",
            AdapterSnapshot { dhcp: false, ipv4_servers: vec!["8.8.8.8".into()] },
        );
        state.save_to(&path).unwrap();

        let mut loaded = RestoreState::load_from(&path);
        assert!(loaded.has("Wi-Fi"));
        let snap = loaded.take("Wi-Fi").unwrap();
        assert_eq!(snap.ipv4_servers, vec!["8.8.8.8".to_string()]);
    }
}
