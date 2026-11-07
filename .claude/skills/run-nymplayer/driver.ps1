<#
Drives a running NymPlayer on Windows through UI Automation (eframe's "accesskit" feature
exposes every egui widget), plus PrintWindow screenshots and the NYMPLAYER_TRACE log.
One command per invocation; state lives in $env:TEMP\nymplayer-run (sandboxed %APPDATA%,
trace file, screenshots). Run from the repo root:

  powershell -ExecutionPolicy Bypass -File .claude/skills/run-nymplayer/driver.ps1 <command> [args]

  launch [-Unmuted] [-Input pcsound|<device id>] [-Volume 0..1] [-Exe <path>]
                                   start target\release\nymplayer.exe with sandboxed settings, muted unless -Unmuted
  tree                             list widgets: index, type, name, supported actions
  act <name|#index>                Invoke (buttons, unnamed icons) or Toggle (Mute, checkboxes) a widget
  set <name|#index> <value>        set a slider (RangeValue) or a combo box (Value)
  save [path]                      answer the "Record to file" dialog (default <state>\test.wav; .wav or .flac)
  ss [name] [-Frame]               screenshot the client area (or with -Frame the whole window, title bar included,
                                   as in assets/screenshot.png) to <state>\<name>.png (default: window)
  trace [n]                        last n trace lines (default 5): unix time, fill ms, underruns, dropped, xruns, failed, status
  settings                         print the sandboxed settings.txt
  quit                             close the window (like clicking X) and wait for exit
#>
param(
    [Parameter(Position = 0, Mandatory = $true)][string]$Command,
    [Parameter(Position = 1)][string]$A1,
    [Parameter(Position = 2)][string]$A2,
    [switch]$Unmuted,
    [Alias('Input')][string]$InputId = '',  # $Input itself is a PowerShell automatic variable
    [double]$Volume = 1,
    [switch]$Frame,
    [string]$Exe
)
$ErrorActionPreference = 'Stop'
$state = Join-Path $env:TEMP 'nymplayer-run'
$appdata = Join-Path $state 'appdata'
$trace = Join-Path $state 'trace.txt'

Add-Type -AssemblyName System.Drawing, UIAutomationClient, UIAutomationTypes
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class NymWin {
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
    [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int attr, out RECT r, int size);
    public struct RECT { public int L, T, R, B; }
    public struct POINT { public int X, Y; }
}
'@
[NymWin]::SetProcessDPIAware() | Out-Null  # physical pixels, else rects are scaled and crops are wrong
$UIA = [System.Windows.Automation.AutomationElement]

function Get-Window {
    $p = Get-Process nymplayer -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
    if (-not $p) { throw 'NymPlayer is not running (or its window is hidden in the tray)' }
    $p
}

function Get-Widgets {
    $root = $UIA::FromHandle((Get-Window).MainWindowHandle)
    # The first query wakes AccessKit up; the tree can be empty until it has.
    for ($i = 0; $i -lt 10; $i++) {
        $all = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
        if ($all.Count -gt 0) { return @($all) }
        Start-Sleep -Milliseconds 200
    }
    throw 'no UI Automation elements: was the app built without eframe''s "accesskit" feature?'
}

function Find-Widget([string]$key) {
    $w = Get-Widgets
    if ($key -match '^#(\d+)$') { return $w[[int]$Matches[1]] }
    $hit = $w | Where-Object { $_.Current.Name -eq $key } | Select-Object -First 1
    if (-not $hit) { throw "no widget named '$key'; run 'tree'" }
    $hit
}

function Get-Pattern($e, $pattern) {
    $o = $null
    if ($e.TryGetCurrentPattern($pattern, [ref]$o)) { $o } else { $null }
}

switch ($Command) {
    'launch' {
        if (Get-Process nymplayer -ErrorAction SilentlyContinue) {
            throw 'NymPlayer is already running; a second copy would only signal it and exit. Run "quit" (or close it) first.'
        }
        if (-not $Exe) { $Exe = Join-Path (Get-Location) 'target\release\nymplayer.exe' }
        $settings = Join-Path $appdata 'NymPlayer\settings.txt'
        New-Item -ItemType Directory -Force (Split-Path $settings) | Out-Null
        # input, output (empty = Windows defaults), volume, muted, (unused "playing"), record dir, record format
        $muted = if ($Unmuted) { '0' } else { '1' }
        $vol = $Volume.ToString([Globalization.CultureInfo]::InvariantCulture)
        [IO.File]::WriteAllText($settings, "$InputId`n`n$vol`n$muted`n1`n`n`n")
        Remove-Item $trace -ErrorAction SilentlyContinue
        $env:APPDATA = $appdata
        $env:NYMPLAYER_TRACE = $trace
        $p = Start-Process -FilePath $Exe -PassThru
        for ($i = 0; $i -lt 50 -and $p.MainWindowHandle -eq 0; $i++) { Start-Sleep -Milliseconds 100; $p.Refresh() }
        if ($p.MainWindowHandle -eq 0) { throw "no window after 5 s (exited: $($p.HasExited))" }
        Start-Sleep -Seconds 2  # the first sync opens the streams after the first frame
        "pid $($p.Id); state in $state"
    }
    'tree' {
        $i = 0
        foreach ($e in Get-Widgets) {
            $c = $e.Current
            $acts = ($e.GetSupportedPatterns() | ForEach-Object { $_.ProgrammaticName -replace 'PatternIdentifiers.Pattern', '' }) -join ','
            $val = Get-Pattern $e ([System.Windows.Automation.ValuePattern]::Pattern)
            $rng = Get-Pattern $e ([System.Windows.Automation.RangeValuePattern]::Pattern)
            $tog = Get-Pattern $e ([System.Windows.Automation.TogglePattern]::Pattern)
            $extra = if ($rng) { " value=$($rng.Current.Value)" } elseif ($val) { " value='$($val.Current.Value)'" } else { '' }
            if ($tog) { $extra += " state=$($tog.Current.ToggleState)" }
            '#{0,-2} {1,-9} {2,-40} [{3}]{4}' -f $i, ($c.ControlType.ProgrammaticName -replace 'ControlType.', ''), $c.Name, $acts, $extra
            $i++
        }
    }
    'act' {
        $e = Find-Widget $A1
        $inv = Get-Pattern $e ([System.Windows.Automation.InvokePattern]::Pattern)
        $tog = Get-Pattern $e ([System.Windows.Automation.TogglePattern]::Pattern)
        if ($tog) { $tog.Toggle(); 'toggled' } elseif ($inv) { $inv.Invoke(); 'invoked' } else { throw 'widget supports neither Invoke nor Toggle' }
        Start-Sleep -Milliseconds 300
    }
    'set' {
        $e = Find-Widget $A1
        $rng = Get-Pattern $e ([System.Windows.Automation.RangeValuePattern]::Pattern)
        $val = Get-Pattern $e ([System.Windows.Automation.ValuePattern]::Pattern)
        if ($rng) { $rng.SetValue([double]$A2) } elseif ($val) { $val.SetValue($A2) } else { throw 'widget has no settable value' }
        Start-Sleep -Milliseconds 300
        'set'
    }
    'save' {
        # The save dialog (after "act Record") hides its file-name box and Save button from UI
        # Automation, so this goes through Win32: WM_SETTEXT on the Edit, BM_CLICK on control 1.
        Add-Type @'
using System; using System.Runtime.InteropServices;
public static class NymDlg {
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string c, string t);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowEx(IntPtr p, IntPtr after, string c, string t);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, string l);
    [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
}
'@
        $path = if ($A1) { $A1 } else { Join-Path $state 'test.wav' }
        $dlg = [IntPtr]::Zero
        for ($i = 0; $i -lt 30 -and $dlg -eq [IntPtr]::Zero; $i++) { $dlg = [NymDlg]::FindWindow('#32770', 'Record to file'); if ($dlg -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 100 } }
        if ($dlg -eq [IntPtr]::Zero) { throw 'no "Record to file" dialog; run "act Record" first' }
        $h = $dlg
        foreach ($cls in 'DUIViewWndClassName', 'DirectUIHWND', 'FloatNotifySink', 'ComboBox', 'Edit') { $h = [NymDlg]::FindWindowEx($h, [IntPtr]::Zero, $cls, $null) }
        if ($h -eq [IntPtr]::Zero) { throw 'file-name box not found in the dialog' }
        [NymDlg]::SendMessage($h, 0x000C, [IntPtr]::Zero, $path) | Out-Null  # WM_SETTEXT
        [NymDlg]::SendMessage([NymDlg]::GetDlgItem($dlg, 1), 0x00F5, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null  # BM_CLICK Save
        Start-Sleep -Seconds 1
        "recording to $path"
    }
    'ss' {
        $p = Get-Window
        $name = if ($A1) { $A1 } else { 'window' }
        $r = New-Object NymWin+RECT
        [NymWin]::GetWindowRect($p.MainWindowHandle, [ref]$r) | Out-Null
        $bmp = New-Object System.Drawing.Bitmap ($r.R - $r.L), ($r.B - $r.T)
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        $dc = $g.GetHdc()
        # PW_RENDERFULLCONTENT (2): captures the OpenGL surface even when the window is behind others.
        [NymWin]::PrintWindow($p.MainWindowHandle, $dc, 2) | Out-Null
        $g.ReleaseHdc($dc); $g.Dispose()
        if ($Frame) {
            # The visible frame: GetWindowRect also covers the invisible resize borders.
            $v = New-Object NymWin+RECT
            [NymWin]::DwmGetWindowAttribute($p.MainWindowHandle, 9, [ref]$v, 16) | Out-Null  # DWMWA_EXTENDED_FRAME_BOUNDS
            $box = New-Object System.Drawing.Rectangle ($v.L - $r.L), ($v.T - $r.T), ($v.R - $v.L), ($v.B - $v.T)
        } else {
            $c = New-Object NymWin+RECT; [NymWin]::GetClientRect($p.MainWindowHandle, [ref]$c) | Out-Null
            $o = New-Object NymWin+POINT; [NymWin]::ClientToScreen($p.MainWindowHandle, [ref]$o) | Out-Null
            $box = New-Object System.Drawing.Rectangle ($o.X - $r.L), ($o.Y - $r.T), $c.R, $c.B
        }
        $crop = $bmp.Clone($box, $bmp.PixelFormat)
        $out = Join-Path $state "$name.png"
        $crop.Save($out); $crop.Dispose(); $bmp.Dispose()
        $out
    }
    'trace' {
        $n = if ($A1) { [int]$A1 } else { 5 }
        Get-Content $trace -Tail $n -Encoding UTF8
    }
    'settings' { Get-Content (Join-Path $appdata 'NymPlayer\settings.txt') -Encoding UTF8 }
    'quit' {
        $p = Get-Window
        $p.CloseMainWindow() | Out-Null
        if (-not $p.WaitForExit(5000)) { $p.Kill(); 'killed after 5 s' } else { 'exited' }
    }
    default { throw "unknown command: $Command" }
}
