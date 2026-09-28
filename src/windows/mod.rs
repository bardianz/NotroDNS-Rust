//! Windows-specific abstraction layer. Everything that talks to the OS
//! (adapter enumeration, DNS get/set/restore, UAC elevation) lives here,
//! behind a small, testable API — the GUI and business logic in
//! [`crate::app`] never invoke `netsh` or Win32 APIs directly.

#[cfg(windows)]
mod adapters;
#[cfg(windows)]
mod dns_set;
#[cfg(windows)]
mod elevation;

#[cfg(windows)]
pub use adapters::{get_current_dns, list_adapters, Adapter, AdapterDnsConfig};
#[cfg(windows)]
pub use dns_set::{restore_dhcp_ipv4, restore_dhcp_ipv6, set_ipv4_dns, set_ipv6_dns};
#[cfg(windows)]
pub use elevation::{is_elevated, relaunch_elevated};

/// Non-Windows fallback so `cargo check`/`cargo test` can still run the
/// platform-independent parts of this crate (api, cache, bench, restore
/// state) during development on macOS/Linux. NotroDNS is a Windows-only
/// product; this stub is never compiled into the shipped binary.
#[cfg(not(windows))]
mod stub {
    use crate::error::{AppError, AppResult};

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

    pub fn list_adapters() -> AppResult<Vec<Adapter>> {
        Err(AppError::Windows(
            "adapter management is only supported on Windows".into(),
        ))
    }

    pub fn get_current_dns(_adapter_name: &str) -> AppResult<AdapterDnsConfig> {
        Err(AppError::Windows(
            "adapter management is only supported on Windows".into(),
        ))
    }

    pub fn set_ipv4_dns(
        _adapter_name: &str,
        _primary: &str,
        _secondary: Option<&str>,
    ) -> AppResult<()> {
        Err(AppError::Windows(
            "DNS configuration is only supported on Windows".into(),
        ))
    }

    pub fn set_ipv6_dns(
        _adapter_name: &str,
        _primary: &str,
        _secondary: Option<&str>,
    ) -> AppResult<()> {
        Err(AppError::Windows(
            "DNS configuration is only supported on Windows".into(),
        ))
    }

    pub fn restore_dhcp_ipv4(_adapter_name: &str) -> AppResult<()> {
        Err(AppError::Windows(
            "DNS configuration is only supported on Windows".into(),
        ))
    }

    pub fn restore_dhcp_ipv6(_adapter_name: &str) -> AppResult<()> {
        Err(AppError::Windows(
            "DNS configuration is only supported on Windows".into(),
        ))
    }

    pub fn is_elevated() -> bool {
        false
    }

    pub fn relaunch_elevated() -> AppResult<()> {
        Err(AppError::Windows(
            "elevation is only supported on Windows".into(),
        ))
    }
}
#[cfg(not(windows))]
pub use stub::*;
