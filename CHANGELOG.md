# Changelog

All notable changes to NymPlayer are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- Install, update and uninstall instructions in the readme, with CI, release and license badges and a download link.

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

[Unreleased]: https://github.com/nymankla/NymPlayer/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/nymankla/NymPlayer/releases/tag/v0.1.0
