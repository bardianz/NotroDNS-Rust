//! Client for the NotroDNS server-list API.
//!
//! Talks to `https://dnschanger.pythonanywhere.com/api/`, which returns a
//! JSON array of `{ "name", "preferred_ip", "alternate_ip" }` objects (the
//! "legacy" schema this app targets). Every entry is validated before it is
//! trusted anywhere else in the app.

use crate::config::{API_RETRIES, API_TIMEOUT_SECS, API_URL};
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DnsServer {
    pub name: String,
    pub preferred_ip: String,
    #[serde(default)]
    pub alternate_ip: String,
}

/// True if `ip` parses as a valid IPv4 or IPv6 address.
pub fn validate_ip(ip: &str) -> bool {
    ip.trim().parse::<std::net::IpAddr>().is_ok()
}

/// Drops entries with a missing name, an invalid `preferred_ip`, or a
/// non-empty `alternate_ip` that fails to parse. The API is untrusted input,
/// so nothing downstream (benchmarking, `netsh` invocations) should ever see
/// a malformed IP string.
pub fn sanitize_servers(servers: Vec<DnsServer>) -> Vec<DnsServer> {
    servers
        .into_iter()
        .filter(|s| {
            !s.name.trim().is_empty()
                && validate_ip(&s.preferred_ip)
                && (s.alternate_ip.trim().is_empty() || validate_ip(&s.alternate_ip))
        })
        .collect()
}

/// Fetches and sanitizes the server list, retrying with a short backoff on
/// transient failures. Callers are expected to fall back to the local cache
/// (see [`crate::cache`]) when this returns `Err`.
pub async fn fetch_dns_servers(client: &reqwest::Client) -> AppResult<Vec<DnsServer>> {
    let mut last_err: Option<AppError> = None;
    for attempt in 0..API_RETRIES {
        match try_fetch(client).await {
            Ok(servers) => return Ok(sanitize_servers(servers)),
            Err(e) => {
                log::warn!("DNS server list fetch attempt {attempt} failed: {e}");
                last_err = Some(e);
                let backoff_ms = 300u64.saturating_mul(u64::from(attempt) + 1);
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            }
        }
    }
    Err(last_err.unwrap_or_else(|| AppError::Network("unknown error".into())))
}

async fn try_fetch(client: &reqwest::Client) -> AppResult<Vec<DnsServer>> {
    let resp = client
        .get(API_URL)
        .timeout(Duration::from_secs(API_TIMEOUT_SECS))
        .send()
        .await?;
    let resp = resp.error_for_status()?;
    let servers: Vec<DnsServer> = resp.json().await?;
    Ok(servers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_ipv4_and_ipv6() {
        assert!(validate_ip("1.1.1.1"));
        assert!(validate_ip("2606:4700:4700::1111"));
        assert!(!validate_ip("not-an-ip"));
        assert!(!validate_ip("999.999.999.999"));
        assert!(!validate_ip(""));
    }

    #[test]
    fn sanitize_drops_invalid_entries() {
        let input = vec![
            DnsServer {
                name: "Good".into(),
                preferred_ip: "1.1.1.1".into(),
                alternate_ip: "1.0.0.1".into(),
            },
            DnsServer {
                name: "".into(),
                preferred_ip: "8.8.8.8".into(),
                alternate_ip: "".into(),
            },
            DnsServer {
                name: "Bad IP".into(),
                preferred_ip: "nope".into(),
                alternate_ip: "".into(),
            },
            DnsServer {
                name: "Bad alt".into(),
                preferred_ip: "8.8.8.8".into(),
                alternate_ip: "nope".into(),
            },
            DnsServer {
                name: "No alt is fine".into(),
                preferred_ip: "9.9.9.9".into(),
                alternate_ip: "".into(),
            },
        ];
        let out = sanitize_servers(input);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].name, "Good");
        assert_eq!(out[1].name, "No alt is fine");
    }
}
