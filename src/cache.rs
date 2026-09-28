//! Local JSON cache of the DNS server list, so the app keeps working when
//! the remote API is unreachable.

use crate::api::DnsServer;
use crate::config::cache_file_path;
use crate::error::AppResult;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct DnsCache {
    pub servers: Vec<DnsServer>,
    pub fetched_at_unix: u64,
}

impl DnsCache {
    pub fn new(servers: Vec<DnsServer>) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self { servers, fetched_at_unix: now }
    }

    /// Age of this cache entry, in seconds, relative to "now". Returns `0`
    /// on clock skew rather than panicking.
    pub fn age_secs(&self) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        now.saturating_sub(self.fetched_at_unix)
    }

    pub fn load() -> Option<Self> {
        Self::load_from(&cache_file_path())
    }

    pub fn save(&self) -> AppResult<()> {
        self.save_to(&cache_file_path())
    }

    pub fn load_from(path: &Path) -> Option<Self> {
        let data = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&data).ok()
    }

    pub fn save_to(&self, path: &Path) -> AppResult<()> {
        let data = serde_json::to_string_pretty(self)?;
        std::fs::write(path, data)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");

        let servers = vec![DnsServer {
            name: "Test".into(),
            preferred_ip: "1.1.1.1".into(),
            alternate_ip: "1.0.0.1".into(),
        }];
        let cache = DnsCache::new(servers.clone());
        cache.save_to(&path).unwrap();

        let loaded = DnsCache::load_from(&path).expect("cache should load");
        assert_eq!(loaded.servers, servers);
    }

    #[test]
    fn missing_file_loads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does_not_exist.json");
        assert!(DnsCache::load_from(&path).is_none());
    }
}
