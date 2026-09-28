//! Enumerates Windows network adapters and reads their current DNS
//! configuration.
//!
//! Deliberately implemented on top of `netsh` (always present on Windows,
//! no admin rights needed to *read* config) rather than the IP Helper /
//! WMI APIs, to keep the dependency surface — and the resulting binary —
//! small. All commands are invoked as argument vectors via
//! [`std::process::Command`], never through a shell, so there is no command
//! injection surface even though adapter names are attacker-influenceable
//! in principle (they come from the local machine, not the network).
//!
//! Parsing assumes the English-language output of `netsh`. On non-English
//! Windows installs, DHCP-vs-static detection may be unreliable; the
//! adapter list itself is unaffected.

use crate::error::{AppError, AppResult};
use std::net::IpAddr;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adapter {
    pub name: String,
    pub state: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdapterDnsConfig {
    pub dhcp: bool,
    pub ipv4_servers: Vec<String>,
}

pub(crate) fn validate_adapter_name(name: &str) -> AppResult<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::Validation("adapter name must not be empty".into()));
    }
    // netsh's own argument syntax uses `name="..."`; reject characters that
    // would break that syntax or that have no business in an adapter name.
    if name.contains(['"', '\n', '\r']) {
        return Err(AppError::Validation("adapter name contains invalid characters".into()));
    }
    Ok(())
}

/// Wraps `name` the way `netsh`'s own parser expects: as a single argv
/// element containing literal quote characters, e.g. `name="Ethernet 2"`.
/// `std::process::Command` escapes this correctly for `CreateProcess` on
/// Windows, so the child process receives the quotes intact.
pub(crate) fn netsh_name_arg(adapter_name: &str) -> String {
    format!("name=\"{adapter_name}\"")
}

fn run_netsh(args: &[&str]) -> AppResult<String> {
    let output = Command::new("netsh")
        .args(args)
        .output()
        .map_err(|e| AppError::Windows(format!("failed to run netsh: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Windows(format!("netsh {args:?} failed: {stderr}")));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Lists network adapters via `netsh interface ipv4 show interfaces`.
pub fn list_adapters() -> AppResult<Vec<Adapter>> {
    let out = run_netsh(&["interface", "ipv4", "show", "interfaces"])?;
    Ok(parse_adapter_list(&out))
}

fn parse_adapter_list(out: &str) -> Vec<Adapter> {
    // Header looks like:
    // Idx     Met         MTU          State                Name
    // ---  ----------  ----------  ------------  ---------------------------
    let mut adapters = Vec::new();
    let mut header_seen = false;
    for line in out.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        if line.trim_start().starts_with("Idx") {
            header_seen = true;
            continue;
        }
        if !header_seen {
            continue;
        }
        if line.trim_start().starts_with("---") {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        // Idx  Met  MTU  State  Name...  (Name may contain spaces)
        if parts.len() < 5 {
            continue;
        }
        let state = parts[3].to_string();
        let name = parts[4..].join(" ");
        if name.is_empty() {
            continue;
        }
        adapters.push(Adapter { name, state });
    }
    adapters
}

/// Reads the current IPv4 DNS configuration for `adapter_name` via
/// `netsh interface ip show config`.
pub fn get_current_dns(adapter_name: &str) -> AppResult<AdapterDnsConfig> {
    validate_adapter_name(adapter_name)?;
    let name_arg = netsh_name_arg(adapter_name);
    let out = run_netsh(&["interface", "ip", "show", "config", &name_arg])?;
    Ok(parse_dns_config(&out))
}

/// If `line` starts with `label` (case-insensitively), returns the
/// trimmed remainder of the line. Used instead of a single big substring
/// match so this tolerates the variable column-padding `netsh` uses after
/// the colon.
fn line_after_label<'a>(line: &'a str, label: &str) -> Option<&'a str> {
    if line.len() >= label.len() && line[..label.len()].eq_ignore_ascii_case(label) {
        Some(line[label.len()..].trim())
    } else {
        None
    }
}

fn parse_dns_config(out: &str) -> AdapterDnsConfig {
    let mut dhcp = false;
    let mut servers = Vec::new();
    let mut in_dns_block = false;

    for raw_line in out.lines() {
        let line = raw_line.trim();

        if let Some(rest) = line_after_label(line, "DHCP enabled:") {
            dhcp = rest.eq_ignore_ascii_case("yes");
            in_dns_block = false;
            continue;
        }

        if let Some(rest) = line_after_label(line, "Statically Configured DNS Servers:") {
            in_dns_block = true;
            push_if_ip(&mut servers, rest);
            continue;
        }

        if in_dns_block {
            // Continuation lines under the same heading are bare IPs,
            // indented further than the heading itself.
            if line.parse::<IpAddr>().is_ok() {
                push_if_ip(&mut servers, line);
                continue;
            }
            in_dns_block = false;
        }
    }
    AdapterDnsConfig { dhcp, ipv4_servers: servers }
}

fn push_if_ip(servers: &mut Vec<String>, candidate: &str) {
    let candidate = candidate.trim();
    if !candidate.is_empty() && candidate.parse::<IpAddr>().is_ok() {
        servers.push(candidate.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_interface_list() {
        let sample = "\
Idx     Met         MTU          State                Name
---  ----------  ----------  ------------  ---------------------------
  1          75  4294967295  connected     Loopback Pseudo-Interface 1
 12          25        1500  connected     Ethernet
 21          35        1500  disconnected  Wi-Fi
";
        let adapters = parse_adapter_list(sample);
        assert_eq!(adapters.len(), 3);
        assert_eq!(adapters[1].name, "Ethernet");
        assert_eq!(adapters[1].state, "connected");
        assert_eq!(adapters[2].name, "Wi-Fi");
        assert_eq!(adapters[2].state, "disconnected");
    }

    #[test]
    fn parses_static_dns_config() {
        let sample = "\
Configuration for interface \"Ethernet\"
    DHCP enabled:                         No
    IP Address:                           192.168.1.20
    Statically Configured DNS Servers:    1.1.1.1
                                           1.0.0.1
    Register with which suffix:           Primary only
";
        let cfg = parse_dns_config(sample);
        assert!(!cfg.dhcp);
        assert_eq!(cfg.ipv4_servers, vec!["1.1.1.1".to_string(), "1.0.0.1".to_string()]);
    }

    #[test]
    fn parses_dhcp_dns_config() {
        let sample = "\
Configuration for interface \"Wi-Fi\"
    DHCP enabled:                         Yes
    IP Address:                           192.168.1.55
    Statically Configured DNS Servers:    None
";
        let cfg = parse_dns_config(sample);
        assert!(cfg.dhcp);
        assert!(cfg.ipv4_servers.is_empty());
    }

    #[test]
    fn rejects_adapter_names_with_quotes() {
        assert!(validate_adapter_name("Ethernet\"; rm -rf /").is_err());
        assert!(validate_adapter_name("").is_err());
        assert!(validate_adapter_name("Ethernet 2").is_ok());
    }

    #[test]
    fn builds_netsh_name_argument() {
        assert_eq!(netsh_name_arg("Ethernet 2"), "name=\"Ethernet 2\"");
    }
}
