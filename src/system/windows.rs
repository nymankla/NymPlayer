//! Windows: single instance via a named event, autostart via the registry Run key.

use super::TRAY_FLAG;
use std::path::PathBuf;
use std::ptr::null;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, GetLastError, HANDLE};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForSingleObject};
use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};

pub const DEFAULT_DEVICE_LABEL: &str = "Windows default";
pub const PC_SOUND: Option<&str> = Some("All PC sound (except NymPlayer)");
/// PC sound comes as 48 kHz float stereo; Windows converts to it.
pub const PC_SOUND_RATE: u32 = 48000;
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

/// Local date and time for file names, e.g. "2026-10-22 14-05".
pub fn local_timestamp() -> String {
    // SAFETY: GetLocalTime only fills in the struct.
    let t = unsafe {
        let mut t = std::mem::zeroed::<windows_sys::Win32::Foundation::SYSTEMTIME>();
        windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t);
        t
    };
    format!("{}-{:02}-{:02} {:02}-{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute)
}

pub fn config_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?))
}

/// Everything the PC plays except NymPlayer itself, captured with process loopback:
/// before the volume control and the sound driver's effects (Dolby, Realtek, ...), so it's a
/// clean digital copy at full level. Plain loopback of an output device gets the sound after
/// those, which on some PCs is quiet and heavily filtered. Needs Windows 10 build 20348+.
pub struct PcSound {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl PcSound {
    /// Starts capturing on its own thread, which needs COM in multithreaded mode. `data`
    /// gets interleaved stereo samples; `failed` is called if the capture breaks.
    pub fn start(
        mut data: impl FnMut(&[f32]) + Send + 'static,
        failed: impl Fn() + Send + 'static,
    ) -> Result<Self, String> {
        use wasapi::{AudioClient, Direction, SampleType, StreamMode, WaveFormat};
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let s = stop.clone();
        let thread = std::thread::spawn(move || {
            let open = || -> Result<_, String> {
                let e = |e: wasapi::WasapiError| e.to_string();
                wasapi::initialize_mta().ok().map_err(|e| e.to_string())?;
                let format = WaveFormat::new(32, 32, &SampleType::Float, PC_SOUND_RATE as usize, 2, None);
                // Exclude mode with our own process ID: everything except NymPlayer, so what
                // NymPlayer plays is never captured again (no echo).
                let mut client = AudioClient::new_application_loopback_client(std::process::id(), false).map_err(e)?;
                let mode = StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: 0 };
                client.initialize_client(&format, &Direction::Capture, &mode).map_err(e)?;
                let event = client.set_get_eventhandle().map_err(e)?;
                let capture = client.get_audiocaptureclient().map_err(e)?;
                client.start_stream().map_err(e)?;
                Ok((client, capture, event))
            };
            let (client, capture, event) = match open() {
                Ok(c) => {
                    let _ = tx.send(Ok(()));
                    c
                }
                Err(e) => {
                    let _ = tx.send(Err(format!("can't capture the PC sound: {e}")));
                    return;
                }
            };
            let (mut bytes, mut samples) = (vec![], vec![]);
            while !s.load(Ordering::Relaxed) {
                // Times out while nothing plays; that only means checking `stop` again.
                if event.wait_for_event(100).is_err() {
                    continue;
                }
                loop {
                    let frames = match capture.get_next_packet_size() {
                        Ok(Some(n)) if n > 0 => n as usize,
                        Ok(_) => break,
                        Err(_) => return failed(),
                    };
                    bytes.resize(frames * 8, 0); // stereo f32
                    let Ok((n, info)) = capture.read_from_device(&mut bytes) else { return failed() };
                    let n = n as usize * 8;
                    samples.clear();
                    if info.flags.silent {
                        samples.resize(n / 4, 0.0);
                    } else {
                        samples.extend(bytes[..n].as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)));
                    }
                    data(&samples);
                }
            }
            let _ = client.stop_stream();
        });
        rx.recv().map_err(|_| "the PC sound capture stopped unexpectedly".to_string())??;
        Ok(Self { stop, thread: Some(thread) })
    }
}

impl Drop for PcSound {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
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
