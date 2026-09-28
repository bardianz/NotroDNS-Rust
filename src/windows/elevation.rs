//! Detects whether the current process is running elevated, and relaunches
//! it with a UAC prompt on demand. Uses `windows-sys` (thin FFI bindings,
//! no runtime, small code size) rather than the higher-level `windows`
//! crate, matching this project's "small .exe, low dependencies" goal.
//!
//! NotroDNS does **not** require elevation to launch or to browse/benchmark
//! DNS servers — only [`is_elevated`]/[`relaunch_elevated`] are invoked, and
//! only right before an operation that actually needs it (setting or
//! restoring DNS), per the "require elevation only when needed" design
//! goal.

use crate::error::{AppError, AppResult};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// Returns `true` if the current process token is elevated (i.e. the app is
/// running "as administrator"). Fails closed: any error querying the token
/// is treated as "not elevated".
pub fn is_elevated() -> bool {
    unsafe {
        let mut token: HANDLE = std::mem::zeroed();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }

        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut returned_len: u32 = 0;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut TOKEN_ELEVATION as *mut core::ffi::c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned_len,
        );
        CloseHandle(token);

        ok != 0 && elevation.TokenIsElevated != 0
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Relaunches the current executable with the same CLI arguments, via the
/// `runas` shell verb, which triggers the standard UAC consent prompt.
///
/// On success this only means the elevation *request* was accepted by the
/// shell; the caller is expected to exit the current (non-elevated)
/// process immediately afterward, letting the new elevated instance take
/// over.
pub fn relaunch_elevated() -> AppResult<()> {
    let exe = std::env::current_exe()
        .map_err(|e| AppError::Windows(format!("could not resolve own executable path: {e}")))?;

    let exe_wide = to_wide(&exe.to_string_lossy());
    let verb_wide = to_wide("runas");

    let args: Vec<String> = std::env::args().skip(1).collect();
    let args_joined = args.join(" ");
    let args_wide = to_wide(&args_joined);

    let params_ptr = if args_joined.is_empty() {
        std::ptr::null()
    } else {
        args_wide.as_ptr()
    };

    let hwnd: HWND = unsafe { std::mem::zeroed() };
    let result = unsafe {
        ShellExecuteW(
            hwnd,
            verb_wide.as_ptr(),
            exe_wide.as_ptr(),
            params_ptr,
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };

    // Per the ShellExecuteW contract, a return value > 32 indicates success;
    // anything else (including the user declining the UAC prompt) is <= 32.
    if (result as isize) > 32 {
        Ok(())
    } else {
        Err(AppError::Windows(
            "elevation request was declined or could not be started".into(),
        ))
    }
}
