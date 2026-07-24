# RAMOpt

## Download and run

[Download latest release](https://github.com/thnonl/RAMOpt/releases)

1. Open link above and download RAMOpt release archive from **Assets**.
2. Extract downloaded archive to folder where you want to keep app.
3. Open extracted `RAMOpt` folder and run `RAMOpt.exe`.

RAMOpt is native Windows memory-maintenance app. Written in Rust with Slint. No browser runtime or WebView.

## What it does

During cleanup, RAMOpt:

- Requests Windows to trim working sets for processes current user can access.
- Optionally removes files and folders from current user's `%TEMP%` and `C:\Windows\Temp`.
- Optionally force-closes selected background apps.
- Reports estimated working-set reduction in MB.

Cleanup runs on demand, on configured schedule, or from global hotkey. Tray menu can show app, run cleanup, toggle scheduled cleanup, temp cleanup, background-app cleanup, Windows startup, or exit.

## App interface and usage

![RAMOpt main window](docs/ramopt-main-window.png)

1. **Enable scheduled cleanup** controls scheduled cleanup. Set **Interval (minutes)** from 1 to 1440. Changes save immediately.
2. Choose global hotkey. Default **Ctrl + Alt + R** works while RAMOpt is open or minimized to tray.
3. **Clean user temp files** also attempts `C:\Windows\Temp`; files RAMOpt cannot access are skipped.
4. **Close selected background apps** force-closes listed processes during cleanup. Disable it when those apps must keep running.
5. **Start with Windows** launches RAMOpt after sign-in. **Close to tray icon** hides window instead of exiting when closed.
6. Click **Clean RAM now** for immediate cleanup. Status area shows latest result and up to five cleanup log entries.
7. RAMOpt checks GitHub Releases at startup and every hour. When newer version exists, **Update now** appears beside theme switch. Hover button to see version, then click it to download, replace app files, and restart RAMOpt.
8. Toggle light/dark theme. Click **Default** to restore default settings.

## What it does not do

- Does not overclock RAM, create physical memory, or guarantee free-RAM increase.
- Does not disable, stop, configure, or modify Windows Update.
- Does not bypass Windows protection or access controls.

Windows decides when trimmed memory becomes available. Free RAM may not rise immediately because Windows uses standby cache to improve performance.

## Requirements

- Windows 10 or Windows 11.
- [Rust toolchain](https://www.rust-lang.org/tools/install) with MSVC target.
- Visual Studio Build Tools with **Desktop development with C++** workload, if Rust setup did not install MSVC linker.
- PowerShell, included with Windows.

## Build from source

1. Clone repository:

   ```powershell
   git clone https://github.com/thnonl/RAMOpt.git
   cd RAMOpt
   ```

2. Confirm Rust installation:

   ```powershell
   rustc --version
   cargo --version
   ```

3. Build optimized executable and create the release package:

   ```powershell
   .\package-release.ps1
   ```

   Output: `release\RAMOpt\` containing `RAMOpt.exe`, `LICENSE`, and `README.md`. Raw binary also appears at `target\release\ramopt.exe`.

## Notes

- RAMOpt allows one running instance. Starting it again restores existing window.
- Some protected processes cannot be trimmed without elevated privileges. RAMOpt skips them.
- Startup toggle writes `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\RAMOpt`.
- Settings and `ramopt.log` are stored beside `RAMOpt.exe`.
