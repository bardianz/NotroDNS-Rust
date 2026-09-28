//! App-wide constants and filesystem locations.

use directories::ProjectDirs;
use std::path::PathBuf;

pub const API_URL: &str = "https://dnschanger.pythonanywhere.com/api/";
pub const API_TIMEOUT_SECS: u64 = 8;
pub const API_RETRIES: u32 = 3;

pub const BENCH_TIMEOUT_MS: u64 = 1500;
pub const BENCH_CONCURRENCY: usize = 8;
/// FQDN used for the benchmarking DNS query. A widely cached, stable name is
/// used so the measurement reflects resolver latency rather than lookup
/// failures. The trailing dot marks it as already-fully-qualified.
pub const DNS_QUERY_DOMAIN: &str = "example.com.";

fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("com", "NotroDNS", "NotroDNS")
}

/// Where the last successfully fetched DNS server list is cached, so the
/// app keeps working when `API_URL` is unreachable.
pub fn cache_file_path() -> PathBuf {
    match project_dirs() {
        Some(dirs) => {
            let dir = dirs.cache_dir();
            let _ = std::fs::create_dir_all(dir);
            dir.join("dns_cache.json")
        }
        None => PathBuf::from("notrodns_cache.json"),
    }
}

/// Where per-adapter "DNS settings before NotroDNS touched them" snapshots
/// are stored, so "Restore previous DNS" survives an app restart.
pub fn state_file_path() -> PathBuf {
    match project_dirs() {
        Some(dirs) => {
            let dir = dirs.data_dir();
            let _ = std::fs::create_dir_all(dir);
            dir.join("dns_state.json")
        }
        None => PathBuf::from("notrodns_state.json"),
    }
}
