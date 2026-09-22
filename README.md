# RAMOpt

## Download and run

[Download latest release](https://github.com/thnonl/RAMOpt/releases)

1. Open link above and download RAMOpt release archive from **Assets**.
2. Extract downloaded archive to folder where you want to keep app.
3. Open extracted `RAMOpt` folder and run `RAMOpt.exe`.

RAMOpt is native Windows memory-maintenance app. Written in Rust with Slint. No browser runtime or WebView. Cleanup runs at most one cleanup operation at a time.

## What it does

During cleanup, RAMOpt runs three backend optimization phases without changing the existing UI:

- **Phase 1:** measures physical memory and requests a system working-set trim; Windows may include RAMOpt and other eligible processes.
- **Phase 2:** trims system file cache, flushes modified pages, and purges the standby list.
- **Phase 3:** combines physical pages, reconciles the registry cache, and flushes modified file cache on fixed volumes.
- Each memory area runs independently; failures are logged and later areas still run.
- Optionally removes stale regular files older than 24 hours from current user's `%TEMP%`; directories and symbolic links are skipped.
- Optionally closes selected user applications gracefully and force-cleans only narrowly identified orphan helper processes. Hardware drivers, security processes, network processes, active application trees, and running service-owned processes are excluded.
- Reports estimated physical-memory delta, successful/skipped area counts, and orphan-process cleanup count. The delta is system-wide and can include normal Windows workload changes.

Cleanup runs on demand, on configured schedule, or from global hotkey. Tray menu can show app, run cleanup, toggle scheduled cleanup, temp cleanup, user-app cleanup, Windows startup, or exit.

## App interface and usage

![RAMOpt main window](docs/ramopt-main-window.png)

1. **Enable scheduled cleanup** controls scheduled cleanup and is disabled by default. Set **Interval (minutes)** from 1 to 1440. Changes save immediately.
2. Choose global hotkey. Default **Ctrl + Alt + R** works while RAMOpt is open or minimized to tray.
3. **Clean user temp files** also attempts `C:\Windows\Temp`; files RAMOpt cannot access are skipped.
4. **Close selected user apps (safe)** is disabled by default. It sends graceful close requests to matching processes in the current user session, including OneDrive, Teams, and Adobe helper apps. It force-cleans only stale, parentless, childless, network-idle helper processes that match exact allowlisted paths and command lines. It skips active application trees, running service-owned processes, hardware drivers, security/network processes, and ambiguous processes.
5. **Start with Windows** launches RAMOpt after sign-in. **Close to tray icon** hides window instead of exiting when closed.
6. Click **Clean RAM now** for immediate cleanup. Status area shows latest result and up to five cleanup log entries.
7. RAMOpt checks GitHub Releases at startup and every hour. When newer version exists, **Update now** appears beside theme switch. Hover button to see version, then click it to download, replace app files, and restart RAMOpt.
8. Toggle light/dark theme. Click **Default** to restore default settings.

## What it does not do

- Does not overclock RAM, create physical memory, or guarantee free-RAM increase.
- Does not disable, stop, configure, or modify Windows Update.
- Does not bypass Windows protection or access controls.

Windows decides when trimmed memory becomes available. Free RAM may not rise immediately because Windows uses standby cache to improve performance. Native cache and page-list operations can increase page faults or disk I/O; RAMOpt logs each area separately and skips only the failed area.

## Requirements

- Windows 10 or Windows 11.
- RAMOpt requests administrator privileges because phases 1–3 use system memory APIs and volume cache operations. Launching RAMOpt or its Windows startup entry may show a UAC prompt.
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

   Output: `release\RAMOpt\` containing `RAMOpt.exe`, `RAMOpt-updater.bat`, `LICENSE`, and `README.md`; plus `release\RAMOpt-Windows-x64.zip` and `release\SHA256SUMS.txt`. Raw binary also appears at `target\release\ramopt.exe`.

## Notes

- RAMOpt allows one running instance. Starting it again restores existing window.
- Some protected processes cannot be trimmed even with elevated privileges. RAMOpt skips failed areas/processes and records native error codes in `%LOCALAPPDATA%\RAMOpt\ramopt.log`.
- Startup toggle writes `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\RAMOpt`; because RAMOpt now requests administrator privileges, Windows may require interactive elevation at startup. Use manual launch if unattended startup is required.
- Settings and `ramopt.log` are stored in `%LOCALAPPDATA%\RAMOpt`. An existing `settings.json` beside `RAMOpt.exe` is migrated on first launch.
- The native working-set operation is system-wide; Windows may include RAMOpt and other eligible processes. RAMOpt does not explicitly open or target its own process.
- `OneDrive.Sync.Service.exe` is eligible only under strict orphan checks: exact OneDrive path, current user session, no running OneDrive companion, no parent or child, and no service ownership. Otherwise it is skipped.
- The updater verifies a `SHA256SUMS.txt` checksum before replacing files, and restarts the previous install if the update fails.
