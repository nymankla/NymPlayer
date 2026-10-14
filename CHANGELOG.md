# Changelog

All notable changes to NymPlayer are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- Cast to Google speakers (experimental): a cast icon finds Google Home, Nest and Chromecast speakers and groups on the network (mDNS) and streams the input to the chosen one. The speaker must be allowed to connect to the PC through the firewall.

## [0.1.2] - 2026-10-14

### Changed
- Much faster start-up: the window appears in about 0.25 s instead of 2–3 s. NymPlayer now draws with OpenGL instead of wgpu, and opens the audio devices just after the window is shown. The `.exe` is also smaller (8 MB instead of 15 MB).
- Windows-specific code is isolated in `src/system/`, so the app builds on macOS and Linux (untested with real audio there; no single instance, autostart or tray icon yet). CI now builds and tests on all three.

## [0.1.1] - 2026-10-14

### Added
- Install, update and uninstall instructions in the readme, with CI, release and license badges and a download link.
- Screenshot in the readme.
- `NYMPLAYER_TRACE=<file>` writes a once-per-second diagnostic log (buffer fill, underruns, dropped input, xruns).

### Fixed
- Up to 70 ms of extra delay for the first ~40 seconds after playback started, when the output (typically Bluetooth) started late.
- Clicks roughly every 30 seconds with Bluetooth output: the buffer target is raised from 20 to 40 ms.
- Repeated dropouts after about 5 minutes of playback: a brief input glitch (xrun) restarted both streams. Glitches the stream recovers from on its own no longer cause a restart.

## [0.1.0] - 2026-10-13

First release.

### Added
- Live audio passthrough from an input device (such as a USB audio interface) to any output device (speakers, Bluetooth headphones).
- Input and output dropdowns with a "Windows default" option that follows the system default.
- Play / Stop, input and output level meters, volume and mute.
- Automatic device switching, also while minimized: unplugged devices fall back to the Windows default and are picked up again when they return.
- High-quality sample-rate conversion (rubato), with drift correction that keeps latency steady when the two device clocks differ.
- Mono input is played on both channels; 16- and 32-bit integer input is accepted as well as float.
- Settings are saved: devices, volume, mute, and whether it was playing (playback resumes on the next launch).
- Devices are remembered by Windows device ID, so identical devices are told apart. Settings from earlier builds, which stored names, are migrated automatically.
- Optional "Start with Windows and keep in the tray" mode.
- Only one copy runs at a time; starting it again brings the running copy to the front.
- Record-player icon.

[Unreleased]: https://github.com/nymankla/NymPlayer/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/nymankla/NymPlayer/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/nymankla/NymPlayer/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/nymankla/NymPlayer/releases/tag/v0.1.0
