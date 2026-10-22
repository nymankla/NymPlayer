//! Platform integration. All OS-specific code lives under `system/`; the rest of the app
//! only uses this module's API, which every platform file provides:
//! - `single_instance() -> Option<ShowRequests>`, `ShowRequests::watch`
//! - `autostart()`, `set_autostart(on)`, `AUTOSTART_LABEL` (`None` hides the option)
//! - `tray_icon()`, `config_dir()`, `DEFAULT_DEVICE_LABEL`
//! - `PC_SOUND` (input label, `None` if unsupported), `PC_SOUND_RATE`, `PcSound::start`:
//!   capture everything the PC plays except NymPlayer
//! - `local_timestamp()`: local date and time for file names, "2026-10-22 14-05"

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(not(windows))]
mod other;
#[cfg(not(windows))]
pub use other::*;

/// Command-line flag used by the autostart entry: start hidden in the tray.
pub const TRAY_FLAG: &str = "--tray";
