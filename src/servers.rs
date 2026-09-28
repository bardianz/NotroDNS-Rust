//! The DNS server list the UI shows: built-in defaults + the (optional)
//! online API list + the user's own custom entries.
//!
//! The API is only one *source* among three. The app is fully functional
//! with just the built-in and custom entries.

use crate::api::DnsServer;
use crate::config::{custom_file_path, write_atomic};
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::IpAddr;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Builtin,
    Api,
    Custom,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Builtin => "Built-in",
            Source::Api => "Online",
            Source::Custom => "Custom",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerEntry {
    pub server: DnsServer,
    pub source: Source,
}

fn s(name: &str, preferred: &str, alternate: &str) -> DnsServer {
    DnsServer {
        name: name.to_string(),
        preferred_ip: preferred.to_string(),
        alternate_ip: alternate.to_string(),
    }
}

/// A small, well-known starter list so the app is useful on first launch,
/// offline, before any API update.
pub fn builtin_servers() -> Vec<DnsServer> {
    vec![
        s("Cloudflare", "1.1.1.1", "1.0.0.1"),
        s("Google", "8.8.8.8", "8.8.4.4"),
        s("Quad9", "9.9.9.9", "149.112.112.112"),
        s("OpenDNS", "208.67.222.222", "208.67.220.220"),
        s("AdGuard", "94.140.14.14", "94.140.15.15"),
        s("CleanBrowsing", "185.228.168.9", "185.228.169.9"),
        s(
            "Cloudflare (IPv6)",
            "2606:4700:4700::1111",
            "2606:4700:4700::1001",
        ),
        s("Google (IPv6)", "2001:4860:4860::8888", "2001:4860:4860::8844"),
    ]
}

/// Combines the three sources. When two entries share the same preferred IP,
/// the first wins, in priority order: custom, then API, then built-in.
pub fn merge(builtin: &[DnsServer], api: &[DnsServer], custom: &[DnsServer]) -> Vec<ServerEntry> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let groups: [(&[DnsServer], Source); 3] = [
        (custom, Source::Custom),
        (api, Source::Api),
        (builtin, Source::Builtin),
    ];
    for (list, source) in groups {
        for server in list {
            let key = server
                .preferred_ip
                .trim()
                .parse::<IpAddr>()
                .map(|ip| ip.to_string())
                .unwrap_or_else(|_| server.preferred_ip.clone());
            if seen.insert(key) {
                out.push(ServerEntry {
                    server: server.clone(),
                    source,
                });
            }
        }
    }
    out
}

/// True if the server's preferred address is IPv6.
pub fn is_ipv6(server: &DnsServer) -> bool {
    matches!(server.preferred_ip.trim().parse::<IpAddr>(), Ok(IpAddr::V6(_)))
}

/// Validates and normalizes a user-typed custom entry.
pub fn validate_custom(name: &str, preferred: &str, alternate: &str) -> AppResult<DnsServer> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::Validation("Name must not be empty.".into()));
    }
    if name.chars().count() > 64 {
        return Err(AppError::Validation(
            "Name must be at most 64 characters.".into(),
        ));
    }
    let preferred = preferred.trim();
    let p: IpAddr = preferred
        .parse()
        .map_err(|_| AppError::Validation(format!("Invalid preferred IP: \"{preferred}\".")))?;
    if p.is_unspecified() || p.is_multicast() {
        return Err(AppError::Validation(
            "Preferred IP is not a usable unicast address.".into(),
        ));
    }
    let alternate = alternate.trim();
    let alternate_ip = if alternate.is_empty() {
        String::new()
    } else {
        let a: IpAddr = alternate.parse().map_err(|_| {
            AppError::Validation(format!("Invalid alternate IP: \"{alternate}\"."))
        })?;
        if a.is_unspecified() || a.is_multicast() {
            return Err(AppError::Validation(
                "Alternate IP is not a usable unicast address.".into(),
            ));
        }
        if a.is_ipv4() != p.is_ipv4() {
            return Err(AppError::Validation(
                "Preferred and alternate must both be IPv4 or both IPv6.".into(),
            ));
        }
        a.to_string()
    };
    Ok(DnsServer {
        name: name.to_string(),
        preferred_ip: p.to_string(),
        alternate_ip,
    })
}

/// The user's own DNS entries, persisted as JSON.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CustomServers {
    pub servers: Vec<DnsServer>,
}

impl CustomServers {
    pub fn load() -> Self {
        Self::load_from(&custom_file_path())
    }

    pub fn save(&self) -> AppResult<()> {
        self.save_to(&custom_file_path())
    }

    pub fn load_from(path: &Path) -> Self {
        let mut loaded: CustomServers = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        // The file is user-editable: never trust it blindly.
        loaded.servers = loaded
            .servers
            .into_iter()
            .filter_map(|x| validate_custom(&x.name, &x.preferred_ip, &x.alternate_ip).ok())
            .collect();
        loaded
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
    fn builtin_list_is_valid() {
        for server in builtin_servers() {
            assert!(
                validate_custom(&server.name, &server.preferred_ip, &server.alternate_ip).is_ok(),
                "invalid built-in entry: {}",
                server.name
            );
        }
    }

    #[test]
    fn merge_prefers_custom_then_api_then_builtin() {
        let builtin = vec![s("Built", "1.1.1.1", "1.0.0.1"), s("Only builtin", "9.9.9.9", "")];
        let api = vec![s("Api", "1.1.1.1", "1.0.0.1"), s("Only api", "8.8.8.8", "")];
        let custom = vec![s("Mine", "1.1.1.1", "")];
        let merged = merge(&builtin, &api, &custom);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].server.name, "Mine");
        assert_eq!(merged[0].source, Source::Custom);
        assert_eq!(merged[1].server.name, "Only api");
        assert_eq!(merged[1].source, Source::Api);
        assert_eq!(merged[2].server.name, "Only builtin");
        assert_eq!(merged[2].source, Source::Builtin);
    }

    #[test]
    fn validates_custom_entries() {
        assert!(validate_custom("", "1.1.1.1", "").is_err());
        assert!(validate_custom("x", "nope", "").is_err());
        assert!(validate_custom("x", "1.1.1.1", "nope").is_err());
        assert!(validate_custom("x", "1.1.1.1", "2606:4700:4700::1111").is_err());
        assert!(validate_custom("x", "0.0.0.0", "").is_err());
        let ok = validate_custom("  My DNS ", " 1.1.1.1 ", "").unwrap();
        assert_eq!(ok.name, "My DNS");
        assert_eq!(ok.preferred_ip, "1.1.1.1");
        assert!(ok.alternate_ip.is_empty());
    }

    #[test]
    fn detects_ipv6() {
        assert!(is_ipv6(&s("a", "2606:4700:4700::1111", "")));
        assert!(!is_ipv6(&s("a", "1.1.1.1", "")));
    }

    #[test]
    fn custom_store_round_trips_and_drops_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("custom.json");
        let store = CustomServers {
            servers: vec![s("Mine", "1.1.1.1", "1.0.0.1")],
        };
        store.save_to(&path).unwrap();
        assert_eq!(CustomServers::load_from(&path), store);

        std::fs::write(
            &path,
            r#"{"servers":[{"name":"ok","preferred_ip":"8.8.8.8","alternate_ip":""},
                           {"name":"bad","preferred_ip":"zzz","alternate_ip":""}]}"#,
        )
        .unwrap();
        let loaded = CustomServers::load_from(&path);
        assert_eq!(loaded.servers.len(), 1);
        assert_eq!(loaded.servers[0].name, "ok");
    }
}
