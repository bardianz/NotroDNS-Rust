//! Sets, and restores, an adapter's DNS servers via `netsh`.
//!
//! Every IP address and adapter name is validated *before* it ever reaches
//! a [`std::process::Command`] argument vector: addresses must parse as
//! [`std::net::IpAddr`] of the expected family, and adapter names may not
//! contain quote or newline characters. Commands are never built as a
//! shell string, so there is no command-injection surface.

use super::adapters::{netsh_name_arg, validate_adapter_name};
use crate::error::{AppError, AppResult};
use std::net::IpAddr;
use std::process::Command;

fn validate_ipv4(ip: &str) -> AppResult<()> {
    match ip.trim().parse::<IpAddr>() {
        Ok(IpAddr::V4(_)) => Ok(()),
        Ok(IpAddr::V6(_)) => Err(AppError::Validation(format!(
            "{ip} is an IPv6 address, expected IPv4"
        ))),
        Err(_) => Err(AppError::Validation(format!("invalid IPv4 address: {ip}"))),
    }
}

fn validate_ipv6(ip: &str) -> AppResult<()> {
    match ip.trim().parse::<IpAddr>() {
        Ok(IpAddr::V6(_)) => Ok(()),
        Ok(IpAddr::V4(_)) => Err(AppError::Validation(format!(
            "{ip} is an IPv4 address, expected IPv6"
        ))),
        Err(_) => Err(AppError::Validation(format!("invalid IPv6 address: {ip}"))),
    }
}

fn run_netsh(args: &[String]) -> AppResult<()> {
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = Command::new("netsh")
        .args(&arg_refs)
        .output()
        .map_err(|e| AppError::Windows(format!("failed to run netsh: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Windows(format!(
            "netsh {arg_refs:?} failed: {stderr}"
        )));
    }
    Ok(())
}

/// Sets a static IPv4 primary DNS server, and optionally a secondary one,
/// on `adapter_name`. Requires administrator privileges; callers should
/// check [`super::elevation::is_elevated`] first and prompt for elevation
/// rather than let this fail with an opaque `netsh` error.
pub fn set_ipv4_dns(adapter_name: &str, primary: &str, secondary: Option<&str>) -> AppResult<()> {
    validate_adapter_name(adapter_name)?;
    validate_ipv4(primary)?;
    let name_arg = netsh_name_arg(adapter_name);

    run_netsh(&[
        "interface".into(),
        "ip".into(),
        "set".into(),
        "dns".into(),
        name_arg.clone(),
        "static".into(),
        primary.trim().into(),
        "primary".into(),
    ])?;

    if let Some(sec) = secondary {
        let sec = sec.trim();
        if !sec.is_empty() {
            validate_ipv4(sec)?;
            run_netsh(&[
                "interface".into(),
                "ip".into(),
                "add".into(),
                "dns".into(),
                name_arg,
                sec.into(),
                "index=2".into(),
            ])?;
        }
    }
    Ok(())
}

/// Sets a static IPv6 primary DNS server, and optionally a secondary one,
/// on `adapter_name`.
pub fn set_ipv6_dns(adapter_name: &str, primary: &str, secondary: Option<&str>) -> AppResult<()> {
    validate_adapter_name(adapter_name)?;
    validate_ipv6(primary)?;
    let name_arg = netsh_name_arg(adapter_name);

    run_netsh(&[
        "interface".into(),
        "ipv6".into(),
        "set".into(),
        "dns".into(),
        name_arg.clone(),
        "static".into(),
        primary.trim().into(),
        "primary".into(),
    ])?;

    if let Some(sec) = secondary {
        let sec = sec.trim();
        if !sec.is_empty() {
            validate_ipv6(sec)?;
            run_netsh(&[
                "interface".into(),
                "ipv6".into(),
                "add".into(),
                "dns".into(),
                name_arg,
                sec.into(),
                "index=2".into(),
            ])?;
        }
    }
    Ok(())
}

/// Restores automatic (DHCP) IPv4 DNS on `adapter_name`.
pub fn restore_dhcp_ipv4(adapter_name: &str) -> AppResult<()> {
    validate_adapter_name(adapter_name)?;
    let name_arg = netsh_name_arg(adapter_name);
    run_netsh(&[
        "interface".into(),
        "ip".into(),
        "set".into(),
        "dns".into(),
        name_arg,
        "source=dhcp".into(),
    ])
}

/// Restores automatic (DHCP) IPv6 DNS on `adapter_name`.
pub fn restore_dhcp_ipv6(adapter_name: &str) -> AppResult<()> {
    validate_adapter_name(adapter_name)?;
    let name_arg = netsh_name_arg(adapter_name);
    run_netsh(&[
        "interface".into(),
        "ipv6".into(),
        "set".into(),
        "dns".into(),
        name_arg,
        "source=dhcp".into(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_ipv6_as_ipv4() {
        assert!(validate_ipv4("2606:4700:4700::1111").is_err());
    }

    #[test]
    fn rejects_ipv4_as_ipv6() {
        assert!(validate_ipv6("1.1.1.1").is_err());
    }

    #[test]
    fn rejects_garbage_ip() {
        assert!(validate_ipv4("not-an-ip").is_err());
        assert!(validate_ipv6("not-an-ip").is_err());
    }

    #[test]
    fn accepts_valid_addresses() {
        assert!(validate_ipv4("1.1.1.1").is_ok());
        assert!(validate_ipv6("2606:4700:4700::1111").is_ok());
    }
}
