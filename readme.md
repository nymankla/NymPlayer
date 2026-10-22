# NymPlayer

[![CI](https://github.com/nymankla/NymPlayer/actions/workflows/ci.yml/badge.svg)](https://github.com/nymankla/NymPlayer/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/nymankla/NymPlayer)](https://github.com/nymankla/NymPlayer/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

A small Windows app that plays live audio from an input device, such as a USB audio interface or line-in, through any output device, such as speakers, Bluetooth headphones or Cast to Google Home speakers.

**[⬇ Download the latest version](https://github.com/nymankla/NymPlayer/releases/latest)**: a single `.exe`, no installer.

<img src="assets/screenshot.png" alt="NymPlayer window: cast icon, input and output dropdowns with level meters, volume slider, Play and record buttons and the Start with Windows option" width="440">

## Install

NymPlayer is a single program file. There's no installer and no extra software to install.

1. Open the [latest release](https://github.com/nymankla/NymPlayer/releases/latest) and download `NymPlayer-vX.Y.Z-windows-x64.exe` under **Assets**.
2. Make a folder for it, for example `%LOCALAPPDATA%\Programs\NymPlayer` (paste that into the File Explorer address bar), and move the file there. Keeping it out of Downloads matters if you use **Start with Windows**, because Windows starts the app from wherever the file is.
3. Optionally rename it to `NymPlayer.exe`, then right-click it and choose **Show more options → Send to → Desktop (create shortcut)** or **Pin to Start**.
4. Double-click to run. The first time, Windows SmartScreen may say *"Windows protected your PC"*, because the app isn't code-signed. Click **More info → Run anyway**. Windows remembers this, so it only asks once.

Requires 64-bit Windows 11. Windows 10 should also work but hasn't been tested.

### First use

1. Plug in your USB audio interface (or other input) and turn on your headphones or speakers.
2. Choose the **Input** and **Output** devices, or leave either on **Windows default**.
3. Press **Play**. The level meters show audio coming in and going out.
4. Optional: tick **Start with Windows and keep in the tray** to have NymPlayer start hidden in the system tray when you sign in.

### Update

Quit NymPlayer (right-click the tray icon → **Quit**, or close the window), then replace the `.exe` with the new version. Your settings are kept.

### Uninstall

1. If **Start with Windows** is ticked, untick it first. This removes the autostart entry.
2. Quit NymPlayer and delete the `.exe`.
3. To also remove your settings, delete the folder `%APPDATA%\NymPlayer`.

## Features

- **Pick input and output** from the Windows device lists, or choose "Windows default" to follow the system default device.
- **Play / Stop** to start or stop the audio passthrough.
- **Level meters** for input and output.
- **Volume and mute.**
- **Start with Windows and keep in the tray** (optional). With this on, the app starts hidden in the tray and minimizing hides it there. Click the tray icon to show the window, or right-click for Play/Stop and Quit. Turning it off removes the tray icon and the autostart entry, and the app minimizes normally.
- **Remembers your settings.** Devices, volume and mute are saved. If the app was playing when you closed it, it starts playing again on the next launch.
- **One copy at a time.** Starting NymPlayer again brings the running copy to the front.
- **Automatic device switching**, also while minimized. If the chosen device is unplugged, playback moves to the Windows default and switches back when the device returns. "Windows default" follows changes to the system default.
- **High-quality sample-rate conversion.** Input and output can run at different sample rates. The conversion also continuously adjusts for the small speed difference between the two devices' clocks, so audio doesn't click or drift over long sessions.
- **Record to a file.** The round red button next to Play records the input to a WAV or FLAC file. See [Recording](#recording).
- **All PC sound.** The Input list offers *"All PC sound (except NymPlayer)"*: everything other programs play, such as music, videos or games. It's a clean digital copy at full level, unaffected by the Windows volume and sound effects like Dolby. Combine it with casting to send the PC's sound to your Google speakers, or record it. NymPlayer's own sound is left out, so there's no echo. Requires Windows 11; it isn't available on Windows 10.
- **Cast to Google speakers** (experimental). Click the cast icon at the top right to find Google Home, Nest and Chromecast speakers and speaker groups on your network, and pick one to play there instead of on this PC. See [Casting](#casting-experimental).
- **Mono and integer input.** Mono input is played on both channels, and 16- or 32-bit integer input is accepted as well as float.

## Recording

1. Choose the **Input** to record. This can be your USB interface, a microphone, or *"All PC sound"* to record what the PC plays.
2. Press the round **record button** next to Play, and choose a file name, folder and type in the save dialog:
   - **WAV**: uncompressed, opens everywhere.
   - **FLAC**: lossless at about half the size.
3. While recording, the status shows the elapsed time. **Nothing else plays**: no speakers and no casting, only the file.
4. Press the button again (now a square) to stop. The file is finished and ready to open, and NymPlayer goes back to what it was doing before.

Good to know:
- **Recorded as it comes in:** files are 16-bit, at the input's own sample rate and channel count. Volume and Mute don't affect them.
- **Silence stays in the timeline:** quiet stretches are recorded as silence, so the file matches real time even with *"All PC sound"*, which sends nothing while the PC is quiet.
- **Remembered:** the folder and file type you used last.
- **Never overwritten:** if the input device disappears or fails mid-recording, NymPlayer stops and keeps the file rather than starting over. A write error such as a full disk also keeps what was recorded and is shown in the status line.
- **Length limit:** WAV files are limited to 4 GB, about 6 hours of 48 kHz stereo. Use FLAC for longer recordings.

## Casting (experimental)

1. Click the **cast icon** at the top right. NymPlayer searches your network for Google Cast speakers (Google Home, Nest Mini, Nest Audio, Nest Hub, Chromecast, speaker groups); they appear within a second or two.
2. Pick a speaker. The audio now plays there instead of on this PC, and the Output row shows **Casting to …**. Volume and Mute still work.
3. To go back, open the cast menu and choose **Stop casting**. If the speaker stops on its own (for example "Hey Google, stop"), NymPlayer stops playing.

Things to know:

- **The speaker must be able to connect to this PC.** A Cast speaker fetches the audio from NymPlayer over your network, so the first time you cast, Windows asks whether to allow NymPlayer through the firewall: allow it on **private networks**. If your Wi-Fi is set to *Public*, change it to *Private* in Windows settings (Network & internet → Wi-Fi → your network). On a company-managed PC, the firewall may block this entirely; NymPlayer then shows *"… didn't connect to NymPlayer"* after 10 seconds.
- **Delay:** speakers buffer a few seconds of audio, so casting suits music, not live monitoring.
- The audio is sent uncompressed (about 1.5 Mbit/s), which any home Wi-Fi handles.
- Casting has been confirmed working with a Google speaker on a home network, but it's still new and only a few speaker models have been tried. Reports of what works with your speakers are welcome.

## macOS and Linux

Each release also has downloads for **macOS (Apple Silicon)** and **Linux (x64)**. They're built and tested automatically, but **they haven't been run with real audio hardware**, because I don't have a Mac or a Linux machine to test on. On those systems there's currently no single-instance check, no autostart, no tray icon and no "All PC sound" input.

**macOS:** download `NymPlayer-vX.Y.Z-macos-arm64.tar.gz` and double-click it to unpack `nymplayer`. The app isn't signed, so macOS blocks it at first. To allow it, open Terminal in that folder and run:

```sh
xattr -d com.apple.quarantine nymplayer
./nymplayer
```

macOS asks for microphone access the first time NymPlayer records from an input. When it's started from Terminal, the request is made for Terminal.

**Linux:** download `NymPlayer-vX.Y.Z-linux-x64.tar.gz`, then unpack and run it:

```sh
tar -xzf NymPlayer-*-linux-x64.tar.gz
./nymplayer
```

It needs ALSA, GTK 3 and libappindicator, which most desktop distributions already have. On Debian or Ubuntu: `sudo apt install libasound2 libgtk-3-0 libxdo3 libayatana-appindicator3-1`. Audio goes through ALSA, so on PipeWire or PulseAudio systems, choose the "default" device.

**Help is very welcome!** If you have a Mac or a Linux machine with an audio interface, please try it, report what works and what doesn't, or implement the missing parts in `src/system/other.rs`. See [CONTRIBUTING.md](CONTRIBUTING.md).

## Build from source

Requires the [Rust toolchain](https://rustup.rs) (on Windows, the MSVC toolchain). On Linux, install the ALSA and GTK development packages first, e.g. on Debian/Ubuntu: `sudo apt install libasound2-dev libgtk-3-dev libxdo-dev libayatana-appindicator3-dev libxkbcommon-dev`.

```sh
cargo build --release
target/release/nymplayer.exe
```

Run the tests with `cargo test`.

## Notes

- Settings are stored in `%APPDATA%\NymPlayer\settings.txt`.
- Latency through the app is about 40–50 ms. Bluetooth headphones add their own delay, typically 150–250 ms.
- Devices are remembered by their Windows device ID, so identical devices are told apart; the dropdown numbers them, e.g. "USB Codec (2)". If you plug a USB device into a different port, Windows may give it a new ID, and you'll need to select it again.
- The icon is generated by `assets/make_icon.py` (Python standard library only).

Built with [cpal](https://github.com/RustAudio/cpal) for audio, [hound](https://github.com/ruuda/hound) and [flacenc](https://github.com/yotarok/flacenc-rs) for recording, [rfd](https://github.com/PolyMeilex/rfd) for the save dialog, [rustls](https://github.com/rustls/rustls) for the Cast connection, [wasapi](https://github.com/HEnquist/wasapi-rs) for capturing the PC sound, [egui/eframe](https://github.com/emilk/egui) for the GUI, [rubato](https://github.com/HEnquist/rubato) for resampling, and [ringbuf](https://github.com/agerasev/ringbuf).

## License

[MIT](LICENSE). Contributions are welcome; see [CONTRIBUTING.md](CONTRIBUTING.md) and the [Code of Conduct](CODE_OF_CONDUCT.md).
