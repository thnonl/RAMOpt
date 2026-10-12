# RAMOpt UI-test build (opt-in, non-elevated, isolated)

Purpose: let automated UI testing (computer-use) run the native app without UAC and without colliding with an
installed RAMOpt. Normal and release builds are unchanged: everything is gated on the env var `RAMOPT_UI_TEST=1`
at compile time.

## What differs in the UI-test variant
- Manifest `asInvoker` (no UAC). Not elevated, so memory phases (system cache, standby list, ...) may fail per
  area and are logged as "N of M areas skipped"; `C:\Windows\Temp` files are skipped. Fine for UI testing.
- Identifier `com.ramopt.desktop.test` (agent/ui-test.tauri.conf.json): own single-instance mutex and own
  WebView2 profile, so it starts while the installed RAMOpt.exe is running.
- Settings, log: `%LOCALAPPDATA%\RAMOpt-uitest\` (installed app: `%LOCALAPPDATA%\RAMOpt\`).
- "Start with Windows" task / Run entry name: `RAMOpt-uitest` (installed: `RAMOpt`). Do not leave it enabled:
  toggle it off again after testing (or `schtasks /Delete /TN RAMOpt-uitest /F`).
- "Install update" / tray "Update now" are disabled (the updater script targets the installed RAMOpt.exe).
- Output: `E:\RAMOpt\target-uitest\debug\ramopt-tauri.exe` (git-ignored via `/target-uitest/`); the normal
  `target\debug\ramopt-tauri.exe` is not touched.
- Hotkey: the default Ctrl+Alt+R is normally owned by the installed app, so the variant logs "Shortcut unavailable"
  at startup. That is expected; pick another hotkey (e.g. Alt + M) in the UI.

## Build (PowerShell, from E:\RAMOpt)
```
$env:RAMOPT_UI_TEST='1'; $env:CARGO_TARGET_DIR='E:\RAMOpt\target-uitest'
npx tauri build --debug --no-bundle --config agent/ui-test.tauri.conf.json
$env:RAMOPT_UI_TEST=$null; $env:CARGO_TARGET_DIR=$null
```
(Always clear both env vars afterwards, or the next normal build would be a UI-test build.)

Normal debug build, for comparison: `npx tauri build --debug --no-bundle` -> `target\debug\ramopt-tauri.exe`
(requireAdministrator; launching raises UAC).

## Launch
```
Start-Process -FilePath 'E:\RAMOpt\target-uitest\debug\ramopt-tauri.exe' -WorkingDirectory 'E:\RAMOpt\target-uitest\debug' -PassThru
```
Close only that process: `Stop-Process -Id <PID returned above>` (never the installed `RAMOpt.exe`).
Identify it by path: `Get-Process ramopt-tauri | Select Id,Path`.

## Optional: seed a working non-default hotkey (to test conflict -> revert)
Close the variant, then:
```
New-Item -ItemType Directory -Force "$env:LOCALAPPDATA\RAMOpt-uitest" | Out-Null
Set-Content "$env:LOCALAPPDATA\RAMOpt-uitest\settings.json" '{"auto_clean":false,"interval_minutes":15,"hotkey":"alt+KeyM","clean_temp":true,"trim_background_apps":false,"start_with_windows":false}'
```
(the other fields take defaults). Launch, then in the UI pick "Ctrl + Alt + R" (owned by the installed app): expect
Recent activity "Shortcut unavailable: ... still using Alt+M" and the selector returning to Alt + M.
Reset: delete `%LOCALAPPDATA%\RAMOpt-uitest` when done.
