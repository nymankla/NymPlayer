//! Platform integration. All OS-specific code lives under `system/`; the rest of the app
//! only uses this module's API, which every platform file provides:
//! - `single_instance() -> Option<ShowRequests>`, `ShowRequests::watch`
//! - `autostart()`, `set_autostart(on)`, `AUTOSTART_LABEL` (`None` hides the option)
//! - `tray_icon()`, `config_dir()`, `DEFAULT_DEVICE_LABEL`

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
