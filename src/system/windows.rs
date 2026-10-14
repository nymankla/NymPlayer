//! Windows: single instance via a named event, autostart via the registry Run key.

use super::TRAY_FLAG;
use std::path::PathBuf;
use std::ptr::null;
use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, GetLastError, HANDLE};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForSingleObject};
use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};

pub const DEFAULT_DEVICE_LABEL: &str = "Windows default";
/// Output devices can be recorded (WASAPI loopback), so they're offered as inputs too.
pub const LOOPBACK: bool = true;
pub const AUTOSTART_LABEL: Option<&str> = Some("Start with Windows and keep in the tray");

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "NymPlayer";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// A named event that later copies of the app signal to ask this one to show itself.
pub struct ShowRequests(usize); // HANDLE as usize, so it can move to the waiting thread

/// Claims the single-instance slot. If another copy already runs, asks it to show its
/// window and returns `None`; this copy should then exit.
pub fn single_instance() -> Option<ShowRequests> {
    // SAFETY: plain Win32 calls with valid, NUL-terminated arguments.
    unsafe {
        let handle = CreateEventW(null(), 0, 0, wide(r"Local\NymPlayer.Show").as_ptr());
        if handle.is_null() {
            return Some(ShowRequests(0)); // can't tell; run rather than refuse to start
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            AllowSetForegroundWindow(ASFW_ANY); // let the other copy take focus
            SetEvent(handle);
            return None;
        }
        Some(ShowRequests(handle as usize))
    }
}

impl ShowRequests {
    /// Calls `on_show` on a background thread each time another copy is started.
    pub fn watch(self, on_show: impl Fn() + Send + 'static) {
        if self.0 == 0 {
            return;
        }
        std::thread::spawn(move || {
            loop {
                // SAFETY: the handle stays open for the life of the process.
                unsafe { WaitForSingleObject(self.0 as HANDLE, INFINITE) };
                on_show();
            }
        });
    }
}

/// Whether the app is registered to start with Windows.
pub fn autostart() -> bool {
    // SAFETY: only queries existence; no buffer is passed.
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            wide(RUN_KEY).as_ptr(),
            wide(RUN_VALUE).as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        ) == ERROR_SUCCESS
    }
}

/// Adds or removes the autostart entry. Adding again refreshes the path to this exe.
pub fn set_autostart(on: bool) -> Result<(), String> {
    let (key, value) = (wide(RUN_KEY), wide(RUN_VALUE));
    // SAFETY: valid NUL-terminated strings; `data` outlives the call.
    let err = unsafe {
        if on {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let data = wide(&format!("\"{}\" {TRAY_FLAG}", exe.display()));
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        } else {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr())
        }
    };
    // Removing an entry that isn't there is fine: it's already off.
    if err == ERROR_SUCCESS || (!on && err == ERROR_FILE_NOT_FOUND) {
        Ok(())
    } else {
        Err(format!("registry error {err}"))
    }
}

/// The tray icon: resource 1, the icon build.rs embeds in the exe.
pub fn tray_icon() -> Result<tray_icon::Icon, String> {
    tray_icon::Icon::from_resource(1, None).map_err(|e| e.to_string())
}

pub fn config_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Touches the real HKCU Run key (restored afterwards): `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn autostart_round_trip() {
        let before = autostart();
        set_autostart(true).unwrap();
        assert!(autostart());
        set_autostart(false).unwrap();
        assert!(!autostart());
        set_autostart(before).unwrap();
    }
}
