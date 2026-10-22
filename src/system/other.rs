//! Fallbacks for macOS and Linux, so the app builds and runs there: one copy per start
//! (no single-instance check), no autostart and no tray icon.
// shortcut: no single instance, autostart or tray on macOS/Linux; port them when a release for those systems is planned.

use std::path::PathBuf;

pub const DEFAULT_DEVICE_LABEL: &str = "System default";
// shortcut: no PC sound input off Windows; macOS 14.6+ has a system audio tap (cpal loopback), add it once tested there.
pub const PC_SOUND: Option<&str> = None;
pub const PC_SOUND_RATE: u32 = 48000;

pub struct PcSound;

impl PcSound {
    pub fn start(
        _data: impl FnMut(&[f32]) + Send + 'static,
        _failed: impl Fn() + Send + 'static,
    ) -> Result<Self, String> {
        Err("capturing the PC sound is not supported on this system yet".into())
    }
}
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

/// Date and time for file names, e.g. "2026-10-22 14-05".
// shortcut: UTC, not local time; use the platform's local time when this ships on macOS/Linux.
pub fn local_timestamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (year, month, day) = civil(secs / 86400);
    let rem = secs % 86400;
    format!("{year}-{month:02}-{day:02} {:02}-{:02}", rem / 3600, rem % 3600 / 60)
}

/// Year, month, day from days since 1970-01-01 (Howard Hinnant's algorithm).
fn civil(days: u64) -> (i64, i64, i64) {
    let z = days as i64 + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// `~/Library/Application Support` on macOS, `$XDG_CONFIG_HOME` or `~/.config` elsewhere.
pub fn config_dir() -> Option<PathBuf> {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    if cfg!(target_os = "macos") {
        return Some(home()?.join("Library/Application Support"));
    }
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| Some(home()?.join(".config")))
}

#[cfg(test)]
mod tests {
    #[test]
    fn civil_dates() {
        assert_eq!(super::civil(0), (1970, 1, 1));
        assert_eq!(super::civil(10_956), (1999, 12, 31));
        assert_eq!(super::civil(11_016), (2000, 2, 29));
        assert_eq!(super::civil(20_748), (2026, 10, 22));
    }
}
