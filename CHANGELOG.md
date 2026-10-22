# Changelog

All notable changes to NymPlayer are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.4.0] - 2026-10-22

### Added
- Record the input to a WAV or FLAC file ([#1](https://github.com/nymankla/NymPlayer/issues/1)): a record button next to Play opens a save dialog. While recording, nothing else plays, and the input is saved untouched by volume and mute. Quiet stretches are kept as silence, and the last folder and format are remembered. If the input changes mid-recording, the recording stops and the file is kept.

- Downloads for macOS (Apple Silicon) and Linux (x64) in every release, alongside Windows. They're untested with real audio hardware.

### Changed
- Smaller downloads: 7.7 MB instead of 10.2 MB. Published builds optimize dependencies for size (NymPlayer's own audio code and the resampler stay optimized for speed). NymPlayer now does its own speaker search and Cast control connection, replacing three libraries (mdns-sd, oxicast, tokio) and a second crypto library.
- The speaker search only asks on the network adapter used for internet traffic, so virtual adapters (Hyper-V, VPN) no longer get in the way.
- The CI workflow only runs when started by hand. Releases build and test on all three systems themselves.
- "What plays on …" inputs are replaced by **"All PC sound (except NymPlayer)"**, which captures other programs' sound before the Windows volume and the sound driver's effects (process loopback). The old inputs were quiet and dull on many laptops: they captured after the volume slider and effects such as Dolby. NymPlayer's own sound is left out, so the feedback guard is no longer needed. Saved settings move to the new input automatically.

## [0.3.0] - 2026-10-14

### Added
- Play what the PC plays (loopback): output devices are offered as inputs ("What plays on …"), e.g. to cast the PC's music to Google speakers. Looping a device back into itself is refused, as it would echo endlessly. While casting, gaps in the input are filled with silence, so the speaker keeps playing when the PC goes quiet.

### Changed
- Long device names are cut short with "…", so the dropdowns stay inside the window.
- The cast menu lists speakers first, then speaker groups after a divider, and scrolls when it doesn't fit the window.
- Published downloads are built with link-time optimization: about 20% smaller.

### Fixed
- Casting: the stream address now contains a random secret, so other devices on the network can't listen in or take the stream over.
- Casting: a speaker that stops reading (e.g. a Wi-Fi dropout) can no longer freeze the stream; it times out after 5 seconds.
- Speaker groups (and the speaker hosting a group) were often missing from the cast menu: a group is announced at one of its speakers' addresses, and only the first answer from each address was kept.

## [0.2.0] - 2026-10-14

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

[Unreleased]: https://github.com/nymankla/NymPlayer/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/nymankla/NymPlayer/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/nymankla/NymPlayer/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/nymankla/NymPlayer/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/nymankla/NymPlayer/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/nymankla/NymPlayer/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/nymankla/NymPlayer/releases/tag/v0.1.0
