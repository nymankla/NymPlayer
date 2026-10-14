//! Fallbacks for macOS and Linux, so the app builds and runs there: one copy per start
//! (no single-instance check), no autostart and no tray icon.
// shortcut: no single instance, autostart or tray on macOS/Linux; port them when a release for those systems is planned.

use std::path::PathBuf;

pub const DEFAULT_DEVICE_LABEL: &str = "System default";
// shortcut: no loopback inputs off Windows; cpal supports it on macOS 14.6+, enable once tested there.
pub const LOOPBACK: bool = false;
pub const AUTOSTART_LABEL: Option<&str> = None;

pub struct ShowRequests;

pub fn single_instance() -> Option<ShowRequests> {
    Some(ShowRequests)
}

impl ShowRequests {
    pub fn watch(self, _on_show: impl Fn() + Send + 'static) {}
}

pub fn autostart() -> bool {
    false
}

pub fn set_autostart(_on: bool) -> Result<(), String> {
    Err("autostart is not supported on this system yet".into())
}

pub fn tray_icon() -> Result<tray_icon::Icon, String> {
    Err("the tray icon is not supported on this system yet".into())
}

/// `~/Library/Application Support` on macOS, `$XDG_CONFIG_HOME` or `~/.config` elsewhere.
pub fn config_dir() -> Option<PathBuf> {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        return Some(home()?.join("Library/Application Support"));
    }
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| Some(home()?.join(".config")))
}
