---
name: run-nymplayer
description: Build, run, launch, drive, click, screenshot or smoke-test the NymPlayer desktop app on Windows (mute, record to a file, cast menu, read the audio trace). Use when asked to run NymPlayer, check a UI change in the real window, or take a screenshot of it.
---

# Run NymPlayer

NymPlayer is a native egui/eframe Windows app. Drive it with
`.claude/skills/run-nymplayer/driver.ps1`. The driver works through
**UI Automation**: eframe's `accesskit` feature exposes every egui widget, so
the driver needs no mouse and no window focus. It also takes `PrintWindow`
screenshots and reads the `NYMPLAYER_TRACE` log. Paths are relative to the
repo root.

## Build

```powershell
cargo build --release
```

If the build fails with `failed to remove file ... nymplayer.exe`, a copy is
still running. Run `driver.ps1 quit` first.

## Run (agent path)

Run one command per call from the repo root.

```powershell
$d = '.claude/skills/run-nymplayer/driver.ps1'
powershell -ExecutionPolicy Bypass -File $d launch      # sandboxed settings, starts MUTED
powershell -ExecutionPolicy Bypass -File $d tree        # widgets: #index, type, name, actions, value/state
powershell -ExecutionPolicy Bypass -File $d ss          # -> $env:TEMP\nymplayer-run\window.png; Read it
powershell -ExecutionPolicy Bypass -File $d trace 3     # unix time, fill ms, underruns, dropped, xruns, failed, status
powershell -ExecutionPolicy Bypass -File $d act Muted   # Toggle by name (label changes: Muted <-> Mute)
powershell -ExecutionPolicy Bypass -File $d act '#1'    # cast icon (unnamed): opens the speaker menu
powershell -ExecutionPolicy Bypass -File $d act Record  # opens the save dialog...
powershell -ExecutionPolicy Bypass -File $d save        # ...answers it (default $env:TEMP\nymplayer-run\test.wav)
powershell -ExecutionPolicy Bypass -File $d act Stop    # finishes the file
powershell -ExecutionPolicy Bypass -File $d settings    # the sandboxed settings.txt
powershell -ExecutionPolicy Bypass -File $d quit
```

- `launch -Input pcsound -Volume 0.84` seeds the input and volume. Use
  `pcsound` (All PC sound) rather than the mic whenever you need `-Unmuted`:
  there's no feedback.
- `ss <name> -Frame` captures the whole window, title bar included, the way
  `assets/screenshot.png` is framed. The `release` skill has the exact readme
  recipe.
- `act` uses Toggle if the widget supports it, otherwise Invoke.
- `set <name|#i> <value>` sets the volume slider (`#7`, 0..1) or a combo box.
- The `tree` output from a launch, for reference:

  ```
  #1  Custom    (cast icon)   #3/#5 ComboBox  Input / Output   #7 Slider (volume)
  #9  Button    Muted         #10   Button    Record           #11 Text (status)
  #12 CheckBox  Start with Windows and keep in the tray
  ```

All state goes to `$env:TEMP\nymplayer-run`: the sandboxed `%APPDATA%`, the
trace file, screenshots and test recordings.

## Test

```powershell
cargo clippy --all-targets   # keep it warning-free
cargo test
```

## Gotchas

- **It plays on launch.** There's no Play button: the app starts playing from
  the default input to the default output immediately. `launch` seeds the
  settings with `muted=1`, so the mic doesn't reach the user's speakers.
  Unmute (`-Unmuted` or `act Muted`) only when the sound matters.
- **Settings are sandboxed.** `config_dir()` is just `%APPDATA%`, so `launch`
  points it at the scratch folder. The user's real settings are never
  touched.
- **Autostart is not sandboxed.** The "Start with Windows" checkbox writes the
  real `HKCU\...\Run` key. Don't toggle it.
- **Single instance.** A second launch only signals the first copy and exits.
  `launch` refuses to start if `nymplayer.exe` is already running.
- **Clicking with window messages doesn't work.** A `WM_MOUSEMOVE` posted or
  sent to the window makes winit call `TrackMouseEvent`. Windows then posts
  `WM_MOUSELEAVE` at once, because the real cursor is elsewhere, and egui
  drops the click. That's why the driver uses UI Automation.
- **`SetForegroundWindow` fails** because of the foreground lock, so
  `CopyFromScreen` captures whatever is on top (usually VS Code). Use `ss`
  (PrintWindow with `PW_RENDERFULLCONTENT`), which works behind other windows.
- **DPI scaling.** Without `SetProcessDPIAware`, window rectangles are in
  scaled pixels and screenshots come out cropped. The driver sets it.
- **The save dialog is opaque to UI Automation.** Its file-name box and Save
  button don't appear in the tree, so `save` uses Win32 instead: it walks
  `DUIViewWndClassName > DirectUIHWND > FloatNotifySink > ComboBox > Edit`,
  sends `WM_SETTEXT`, then `BM_CLICK` on control ID 1.
- **The volume slider is disabled while muted.** `set '#7' 0.5` fails with
  "operation is not allowed on a nonenabled element" until you unmute.
- **Unnamed widgets.** The cast icon is custom-painted and has no name; address
  it as `#1`. If you add a widget above it, the indexes shift, so check `tree`.

## Troubleshooting

- **`no widget named 'X'`:** the label changed (Muted/Mute, Record/Stop).
  Run `tree`.
- **`NymPlayer is not running (or its window is hidden in the tray)`:** in tray
  mode, minimizing hides the window. Start it again, or show it from the tray.
