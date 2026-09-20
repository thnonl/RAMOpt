use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::{ComponentHandle, Weak};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::windows::{ffi::OsStrExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime},
};
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent,
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, HANDLE, HWND, RECT, WAIT_OBJECT_0},
    Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MoveFileExW},
    System::{
        LibraryLoader::GetModuleHandleW,
        ProcessStatus::{
            K32EmptyWorkingSet, K32EnumProcesses, K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        },
        SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX},
        Threading::{
            GetCurrentProcessId, OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_SET_QUOTA,
            WaitForSingleObject,
        },
    },
    UI::WindowsAndMessaging::{
        DispatchMessageW, GetSystemMetrics, GetWindowRect, ICON_BIG, ICON_SMALL, IMAGE_ICON,
        LR_DEFAULTSIZE, LR_SHARED, LoadImageW, MSG, PM_REMOVE, PeekMessageW, SM_CXSCREEN,
        SM_CYSCREEN, SW_RESTORE, SWP_NOSIZE, SWP_NOZORDER, SendMessageW, SetForegroundWindow,
        SetWindowPos, ShowWindow, TranslateMessage, WM_SETICON,
    },
};
use winreg::{RegKey, enums::HKEY_CURRENT_USER};

slint::include_modules!();

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_VALUE: &str = "RAMOpt";
const RELEASE_API_URL: &str = "https://api.github.com/repos/thnonl/RAMOpt/releases/latest";
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);
const TEMP_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);
const LOG_ROTATE_BYTES: u64 = 2 * 1024 * 1024;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

static LOG_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static SETTINGS_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static UPDATE_STARTED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    pub auto_clean: bool,
    pub interval_minutes: u32,
    pub hotkey: String,
    pub clean_temp: bool,
    pub trim_background_apps: bool,
    pub start_with_windows: bool,
    #[serde(default = "default_close_to_tray")]
    pub close_to_tray: bool,
    #[serde(default)]
    pub dark_mode: bool,
}

fn default_close_to_tray() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_clean: true,
            interval_minutes: 15,
            hotkey: "ctrl+alt+KeyR".into(),
            clean_temp: true,
            trim_background_apps: false,
            start_with_windows: false,
            close_to_tray: default_close_to_tray(),
            dark_mode: false,
        }
    }
}

fn app_directory() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn data_directory() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("RAMOpt"))
        .unwrap_or_else(app_directory)
}

fn settings_path() -> PathBuf {
    data_directory().join("settings.json")
}

fn legacy_settings_path() -> PathBuf {
    app_directory().join("settings.json")
}

fn updater_path() -> PathBuf {
    app_directory().join("RAMOpt-updater.bat")
}

fn log_path() -> PathBuf {
    data_directory().join("ramopt.log")
}

fn lock_or_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn log(message: impl std::fmt::Display) {
    let lock = LOG_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock_or_recover(lock);
    let path = log_path();
    if let Some(folder) = path.parent() {
        let _ = fs::create_dir_all(folder);
    }
    if fs::metadata(&path)
        .map(|metadata| metadata.len() >= LOG_ROTATE_BYTES)
        .unwrap_or(false)
    {
        let rotated = path.with_extension("log.1");
        let _ = fs::remove_file(&rotated);
        let _ = fs::rename(&path, rotated);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|time| time.as_secs())
            .unwrap_or_default();
        let _ = writeln!(file, "{timestamp} | {message}");
    }
}

pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|panic| {
        log(format!("panic: {panic}"));
    }));
}

pub fn load_settings() -> Settings {
    let primary = settings_path();
    if let Ok(text) = fs::read_to_string(&primary) {
        if let Ok(settings) = serde_json::from_str::<Settings>(&text) {
            return settings;
        }
        log("Settings file invalid; trying legacy settings file.");
    }
    if let Ok(text) = fs::read_to_string(legacy_settings_path())
        && let Ok(settings) = serde_json::from_str::<Settings>(&text)
    {
        if let Err(error) = save_settings(&settings) {
            log(format!("Settings migration failed: {error}"));
        } else {
            let _ = fs::remove_file(legacy_settings_path());
        }
        return settings;
    }
    Settings::default()
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn replace_file_atomically(temp: &Path, destination: &Path) -> Result<(), String> {
    let temp_wide = wide(temp);
    let destination_wide = wide(destination);
    let result = unsafe {
        MoveFileExW(
            temp_wide.as_ptr(),
            destination_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING,
        )
    };
    if result == 0 {
        let error = std::io::Error::last_os_error();
        let _ = fs::remove_file(temp);
        return Err(error.to_string());
    }
    Ok(())
}

fn save_settings(settings: &Settings) -> Result<(), String> {
    let lock = SETTINGS_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock_or_recover(lock);
    let path = settings_path();
    let folder = path
        .parent()
        .ok_or_else(|| "Settings path has no parent directory".to_string())?;
    fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    let json = serde_json::to_vec_pretty(settings).map_err(|error| error.to_string())?;
    let temp = path.with_extension("json.tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp)
        .map_err(|error| error.to_string())?;
    file.write_all(&json).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    drop(file);
    replace_file_atomically(&temp, &path)
}

fn set_startup(enabled: bool) -> Result<(), String> {
    let root = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = root
        .create_subkey(RUN_KEY)
        .map_err(|error| error.to_string())?;
    if enabled {
        let exe = std::env::current_exe().map_err(|error| error.to_string())?;
        key.set_value(RUN_VALUE, &format!("\"{}\"", exe.display()))
            .map_err(|error| error.to_string())?;
    } else if let Err(error) = key.delete_value(RUN_VALUE)
        && error.raw_os_error() != Some(ERROR_FILE_NOT_FOUND as i32)
    {
        return Err(error.to_string());
    }
    Ok(())
}

fn clear_temp_folder(folder: &Path) -> u64 {
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(_) => return 0,
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).ok()?;
            if metadata.file_type().is_symlink() {
                return None;
            }
            let age = metadata.modified().ok()?.elapsed().ok()?;
            if age < TEMP_RETENTION {
                return None;
            }
            // Do not recursively remove directories: a stale directory can
            // still contain files that another process created recently.
            if !metadata.is_file() {
                return None;
            }
            fs::remove_file(path).ok()
        })
        .count() as u64
}

fn clear_temp() -> u64 {
    let temp = std::env::temp_dir();
    clear_temp_folder(&temp)
}

fn system_binary(name: &str) -> PathBuf {
    std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("System32")
        .join(name)
}

fn command_output_with_timeout(mut command: Command, timeout: Duration) -> Result<Output, String> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().map_err(|error| error.to_string()),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "command timed out after {} seconds",
                    timeout.as_secs()
                ));
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.to_string());
            }
        }
    }
}

fn process_working_set(handle: HANDLE) -> Option<u64> {
    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    let success = unsafe {
        K32GetProcessMemoryInfo(
            handle,
            &mut counters,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
    };
    if success == 0 {
        None
    } else {
        Some(counters.WorkingSetSize as u64)
    }
}

fn process_ids() -> Vec<u32> {
    let mut capacity = 1024usize;
    loop {
        let mut ids = vec![0u32; capacity];
        let mut bytes = 0u32;
        let success = unsafe {
            K32EnumProcesses(
                ids.as_mut_ptr(),
                (ids.len() * std::mem::size_of::<u32>()) as u32,
                &mut bytes,
            )
        };
        if success == 0 {
            return Vec::new();
        }
        let count = bytes as usize / std::mem::size_of::<u32>();
        if count < ids.len() - 1 {
            ids.truncate(count);
            return ids;
        }
        capacity *= 2;
        if capacity > 65_536 {
            ids.truncate(count.min(ids.len()));
            return ids;
        }
    }
}

fn trim_working_sets() -> (f64, u32, u32) {
    let current_pid = unsafe { GetCurrentProcessId() };
    let mut trimmed_mb = 0.0;
    let mut trimmed_processes = 0;
    let mut skipped_processes = 0;
    for pid in process_ids() {
        if pid == current_pid || pid == 0 {
            continue;
        }
        let handle = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_SET_QUOTA, 0, pid) };
        if handle.is_null() {
            skipped_processes += 1;
            continue;
        }
        let before = process_working_set(handle);
        let emptied = unsafe { K32EmptyWorkingSet(handle) } != 0;
        let after = process_working_set(handle);
        unsafe {
            CloseHandle(handle);
        }
        if emptied {
            trimmed_processes += 1;
            if let (Some(before), Some(after)) = (before, after) {
                trimmed_mb += before.saturating_sub(after) as f64 / (1024.0 * 1024.0);
            }
        } else {
            skipped_processes += 1;
        }
    }
    (trimmed_mb, trimmed_processes, skipped_processes)
}

fn close_background_apps() -> u32 {
    let names = ["OneDrive", "Teams", "AdobeIPCBroker", "AdobeCollabSync"];
    names
        .iter()
        .map(|name| {
            let script = format!(
                "$ErrorActionPreference='SilentlyContinue'; $current=[Security.Principal.WindowsIdentity]::GetCurrent().Name; $closed=0; Get-Process -Name '{name}' -IncludeUserName | Where-Object {{ $_.UserName -eq $current }} | ForEach-Object {{ if ($_.CloseMainWindow()) {{ [void]($closed++) }} }}; Write-Output $closed"
            );
            let mut command = Command::new(system_binary(r"WindowsPowerShell\v1.0\powershell.exe"));
            command
                .creation_flags(CREATE_NO_WINDOW)
                .args(["-NoProfile", "-NonInteractive", "-Command", &script]);
            command_output_with_timeout(command, COMMAND_TIMEOUT)
                .ok()
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .and_then(|output| output.trim().parse::<u32>().ok())
                .unwrap_or(0)
        })
        .sum()
}

pub fn clean_memory(settings: &Settings) -> String {
    let (trimmed_mb, trimmed_processes, skipped_processes) = trim_working_sets();
    let removed_temp_items = if settings.clean_temp { clear_temp() } else { 0 };
    let closed_apps = if settings.trim_background_apps {
        close_background_apps()
    } else {
        0
    };
    format!(
        "Memory cleaned: {trimmed_mb:.1} MB; trimmed {trimmed_processes} processes; skipped {skipped_processes}; removed {removed_temp_items} temp items; closed {closed_apps} user apps"
    )
}

fn memory_status() -> (u64, u64) {
    unsafe {
        let mut status: MEMORYSTATUSEX = std::mem::zeroed();
        status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut status) == 0 {
            return (0, 0);
        }
        (
            status.ullTotalPhys.saturating_sub(status.ullAvailPhys),
            status.ullTotalPhys,
        )
    }
}

fn gb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
}

fn memory_tooltip(used: u64, total: u64) -> String {
    let percent = if total > 0 {
        used as f64 / total as f64
    } else {
        0.0
    };
    format!(
        "RAMOpt - RAM: {} / {} GB ({:.0}%)",
        gb(used),
        gb(total),
        percent * 100.0
    )
}

fn version_is_newer(tag: &str) -> bool {
    fn parts(version: &str) -> Option<Vec<u32>> {
        version
            .trim_start_matches('v')
            .split('.')
            .map(str::parse)
            .collect::<Result<_, _>>()
            .ok()
    }
    match (parts(tag), parts(env!("CARGO_PKG_VERSION"))) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

fn latest_release_version() -> Result<String, String> {
    let script = format!(
        "$ErrorActionPreference='Stop'; (Invoke-RestMethod -Headers @{{'User-Agent'='RAMOpt'}} -Uri '{RELEASE_API_URL}').tag_name"
    );
    let mut command = Command::new(system_binary(r"WindowsPowerShell\v1.0\powershell.exe"));
    command.creation_flags(CREATE_NO_WINDOW).args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        &script,
    ]);
    let output = command_output_with_timeout(command, COMMAND_TIMEOUT)?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn start_update() -> Result<(), String> {
    if UPDATE_STARTED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("Update is already starting.".to_string());
    }
    let updater = updater_path();
    if !updater.is_file() {
        UPDATE_STARTED.store(false, Ordering::Release);
        return Err(format!("Updater not found: {}", updater.display()));
    }
    let updater_arg = updater.to_string_lossy().into_owned();
    let result = Command::new(system_binary("cmd.exe"))
        .creation_flags(CREATE_NO_WINDOW)
        .current_dir(app_directory())
        .raw_arg(format!("/D /S /C \"\"{updater_arg}\"\""))
        .spawn()
        .map_err(|error| error.to_string());
    if result.is_err() {
        UPDATE_STARTED.store(false, Ordering::Release);
    }
    result.map(|_| ())
}

#[derive(Clone)]
struct TrayMenu {
    update: MenuItem,
    update_separator: PredefinedMenuItem,
    show: MenuItem,
    clean: MenuItem,
    auto: CheckMenuItem,
    temp: CheckMenuItem,
    apps: CheckMenuItem,
    startup: CheckMenuItem,
    exit: MenuItem,
}

impl TrayMenu {
    fn new(settings: &Settings) -> Result<(Menu, Self), String> {
        let menu = Menu::new();
        let items = Self {
            update: MenuItem::new("Update now", true, None),
            update_separator: PredefinedMenuItem::separator(),
            show: MenuItem::new("Show RAMOpt", true, None),
            clean: MenuItem::new("Clean RAM now", true, None),
            auto: CheckMenuItem::new("Scheduled cleanup", true, settings.auto_clean, None),
            temp: CheckMenuItem::new("Clean temp files", true, settings.clean_temp, None),
            apps: CheckMenuItem::new(
                "Close user apps (safe)",
                true,
                settings.trim_background_apps,
                None,
            ),
            startup: CheckMenuItem::new(
                "Start with Windows",
                true,
                settings.start_with_windows,
                None,
            ),
            exit: MenuItem::new("Exit RAMOpt", true, None),
        };
        let cleanup_separator = PredefinedMenuItem::separator();
        let exit_separator = PredefinedMenuItem::separator();
        for item in [&items.show, &items.clean] {
            menu.append(item).map_err(|error| format!("{error:?}"))?;
        }
        menu.append(&cleanup_separator)
            .map_err(|error| format!("{error:?}"))?;
        for item in [&items.auto, &items.temp, &items.apps, &items.startup] {
            menu.append(item).map_err(|error| format!("{error:?}"))?;
        }
        menu.append(&exit_separator)
            .map_err(|error| format!("{error:?}"))?;
        menu.append(&items.exit)
            .map_err(|error| format!("{error:?}"))?;
        Ok((menu, items))
    }

    fn sync_checks(&self, settings: &Settings) {
        self.auto.set_checked(settings.auto_clean);
        self.temp.set_checked(settings.clean_temp);
        self.apps.set_checked(settings.trim_background_apps);
        self.startup.set_checked(settings.start_with_windows);
    }
}

#[allow(clippy::manual_dangling_ptr)]
fn set_window_icon(ui: &MainWindow) {
    let window_handle = ui.window().window_handle();
    let Ok(handle) = window_handle.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let hwnd = handle.hwnd.get() as HWND;
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        // Resource ID 1 is represented by pointer value 1 for MAKEINTRESOURCEW.
        let icon = LoadImageW(
            instance,
            1usize as *const u16,
            IMAGE_ICON,
            0,
            0,
            LR_DEFAULTSIZE | LR_SHARED,
        );
        if !icon.is_null() {
            SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, icon as isize);
            SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, icon as isize);
        } else {
            log("Window icon resource could not be loaded.");
        }
    }
}

fn focus_window(ui: &MainWindow) {
    let window_handle = ui.window().window_handle();
    let Ok(handle) = window_handle.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let hwnd = handle.hwnd.get() as HWND;
    unsafe {
        ShowWindow(hwnd, SW_RESTORE);
        SetForegroundWindow(hwnd);
    }
}

fn show_window(ui: &MainWindow, source: &str) {
    match ui.show() {
        Ok(()) => {
            set_window_icon(ui);
            focus_window(ui);
        }
        Err(error) => {
            let message = format!("Show window failed from {source}: {error}");
            log(&message);
            ui.set_status(message.into());
        }
    }
}

fn center_window(ui: &MainWindow) {
    let window_handle = ui.window().window_handle();
    let Ok(handle) = window_handle.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let hwnd = handle.hwnd.get() as HWND;
    unsafe {
        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);
        let mut rc: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd, &mut rc) != 0 {
            let w = rc.right - rc.left;
            let h = rc.bottom - rc.top;
            let x = (screen_w - w) / 2;
            let y = (screen_h - h) / 2;
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER,
            );
        }
    }
}

fn sync_ui(ui: &MainWindow, settings: &Settings) {
    ui.set_auto_clean(settings.auto_clean);
    ui.set_interval_minutes(settings.interval_minutes as i32);
    ui.set_hotkey(settings.hotkey.clone().into());
    ui.set_clean_temp(settings.clean_temp);
    ui.set_close_apps(settings.trim_background_apps);
    ui.set_startup(settings.start_with_windows);
    ui.set_close_to_tray(settings.close_to_tray);
    ui.set_dark_mode(settings.dark_mode);
}

fn read_ui(ui: &MainWindow) -> Settings {
    Settings {
        auto_clean: ui.get_auto_clean(),
        interval_minutes: ui.get_interval_minutes().clamp(1, 1440) as u32,
        hotkey: ui.get_hotkey().to_string(),
        clean_temp: ui.get_clean_temp(),
        trim_background_apps: ui.get_close_apps(),
        start_with_windows: ui.get_startup(),
        close_to_tray: ui.get_close_to_tray(),
        dark_mode: ui.get_dark_mode(),
    }
}

fn persist(ui: &MainWindow, state: &Arc<Mutex<Settings>>, hotkey_updates: &Sender<String>) {
    let requested = read_ui(ui);
    let previous = lock_or_recover(state).clone();
    let mut settings = requested;
    let startup_changed = settings.start_with_windows != previous.start_with_windows;
    if startup_changed && let Err(error) = set_startup(settings.start_with_windows) {
        log(format!("Startup setting failed: {error}"));
        ui.set_status(format!("Startup setting failed: {error}").into());
        settings.start_with_windows = previous.start_with_windows;
        ui.set_startup(settings.start_with_windows);
    }
    if let Err(error) = save_settings(&settings) {
        if startup_changed && settings.start_with_windows != previous.start_with_windows {
            let _ = set_startup(previous.start_with_windows);
        }
        ui.set_status(format!("Settings save failed: {error}").into());
        log(format!("Settings save failed: {error}"));
        return;
    }
    if settings.hotkey != previous.hotkey {
        let _ = hotkey_updates.send(settings.hotkey.clone());
    }
    *lock_or_recover(state) = settings;
}

fn push_log(ui: &MainWindow, message: &str) {
    let logs = ui.get_logs();
    let lines: Vec<String> = logs
        .lines()
        .chain(std::iter::once(message))
        .filter(|line| !line.is_empty())
        .map(|line| format!("• {}", line.trim_start_matches("• ")))
        .rev()
        .take(5)
        .collect();
    ui.set_logs(
        lines
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n")
            .into(),
    );
}

struct Shutdown {
    stopped: AtomicBool,
    wake: Condvar,
    lock: Mutex<()>,
}

impl Shutdown {
    fn new() -> Self {
        Self {
            stopped: AtomicBool::new(false),
            wake: Condvar::new(),
            lock: Mutex::new(()),
        }
    }

    fn request(&self) {
        let _guard = lock_or_recover(&self.lock);
        self.stopped.store(true, Ordering::Release);
        self.wake.notify_all();
    }

    fn is_requested(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }

    fn wait(&self, duration: Duration) -> bool {
        let guard = lock_or_recover(&self.lock);
        if self.is_requested() {
            return true;
        }
        let _ = self.wake.wait_timeout(guard, duration);
        self.is_requested()
    }
}

fn invoke_ui<F>(ui: &Weak<MainWindow>, action: F)
where
    F: FnOnce(MainWindow) + Send + 'static,
{
    let weak = ui.clone();
    if let Err(error) = slint::invoke_from_event_loop(move || {
        if let Some(ui) = weak.upgrade() {
            action(ui);
        }
    }) {
        log(format!("UI event dispatch failed: {error}"));
    }
}

/// Runs one cleanup at a time. The single-flight flag is released by the worker,
/// so the cleanup thread is intentionally detached: shutdown must never block on
/// system-wide process enumeration or helper-process timeouts.
fn start_cleanup(ui: Weak<MainWindow>, settings: Settings, running: Arc<AtomicBool>) {
    if running
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        invoke_ui(&ui, |ui| ui.set_status("Cleanup already running.".into()));
        return;
    }
    let callback_ui = ui.clone();
    thread::spawn(move || {
        let status = clean_memory(&settings);
        log(&status);
        running.store(false, Ordering::Release);
        invoke_ui(&callback_ui, move |ui| {
            ui.set_status(status.clone().into());
            push_log(&ui, &status);
        });
    });
}

fn spawn_tray_events(
    ui: Weak<MainWindow>,
    state: Arc<Mutex<Settings>>,
    cleanup_running: Arc<AtomicBool>,
    shutdown: Arc<Shutdown>,
    ids: (
        tray_icon::menu::MenuId,
        tray_icon::menu::MenuId,
        tray_icon::menu::MenuId,
        tray_icon::menu::MenuId,
        tray_icon::menu::MenuId,
        tray_icon::menu::MenuId,
        tray_icon::menu::MenuId,
        tray_icon::menu::MenuId,
    ),
) -> JoinHandle<()> {
    thread::spawn(move || {
        while !shutdown.is_requested() {
            while let Ok(event) = TrayIconEvent::receiver().try_recv() {
                if matches!(
                    event,
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } | TrayIconEvent::DoubleClick {
                        button: MouseButton::Left,
                        ..
                    }
                ) {
                    invoke_ui(&ui, |ui| show_window(&ui, "tray click"));
                }
            }
            let event = match MenuEvent::receiver().try_recv() {
                Ok(event) => event,
                Err(_) => {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }
            };
            if event.id == ids.0 {
                match start_update() {
                    Ok(()) => {
                        invoke_ui(&ui, |ui| {
                            ui.set_status(
                                "Downloading update... RAMOpt will restart automatically.".into(),
                            )
                        });
                        shutdown.request();
                        let _ = slint::quit_event_loop();
                        break;
                    }
                    Err(error) => {
                        log(format!("Update failed to start: {error}"));
                        invoke_ui(&ui, move |ui| {
                            ui.set_status(format!("Update failed to start: {error}").into())
                        });
                    }
                }
            } else if event.id == ids.1 {
                invoke_ui(&ui, |ui| show_window(&ui, "tray menu"));
            } else if event.id == ids.2 {
                let settings = lock_or_recover(&state).clone();
                start_cleanup(ui.clone(), settings, cleanup_running.clone());
            } else if event.id == ids.7 {
                shutdown.request();
                let _ = slint::quit_event_loop();
                break;
            } else {
                let mut settings = lock_or_recover(&state);
                let changed = if event.id == ids.3 {
                    settings.auto_clean = !settings.auto_clean;
                    true
                } else if event.id == ids.4 {
                    settings.clean_temp = !settings.clean_temp;
                    true
                } else if event.id == ids.5 {
                    settings.trim_background_apps = !settings.trim_background_apps;
                    true
                } else if event.id == ids.6 {
                    settings.start_with_windows = !settings.start_with_windows;
                    if let Err(error) = set_startup(settings.start_with_windows) {
                        settings.start_with_windows = !settings.start_with_windows;
                        log(format!("Startup setting failed: {error}"));
                    }
                    true
                } else {
                    false
                };
                if changed {
                    let copy = settings.clone();
                    if let Err(error) = save_settings(&copy) {
                        if event.id == ids.6 {
                            let _ = set_startup(!copy.start_with_windows);
                        }
                        log(format!("Settings save failed from tray: {error}"));
                    } else {
                        drop(settings);
                        invoke_ui(&ui, move |ui| sync_ui(&ui, &copy));
                    }
                }
            }
        }
    })
}

fn spawn_show_event_listener(
    ui: Weak<MainWindow>,
    show_event: usize,
    shutdown: Arc<Shutdown>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        while !shutdown.is_requested() {
            let result = unsafe { WaitForSingleObject(show_event as HANDLE, 200) };
            if result == WAIT_OBJECT_0 && !shutdown.is_requested() {
                invoke_ui(&ui, |ui| show_window(&ui, "second instance"));
            }
        }
    })
}

fn spawn_timer(
    ui: Weak<MainWindow>,
    state: Arc<Mutex<Settings>>,
    cleanup_running: Arc<AtomicBool>,
    shutdown: Arc<Shutdown>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        while !shutdown.is_requested() {
            let minutes = lock_or_recover(&state).interval_minutes.clamp(1, 1440);
            if shutdown.wait(Duration::from_secs(u64::from(minutes) * 60)) {
                break;
            }
            let settings = lock_or_recover(&state).clone();
            if settings.auto_clean {
                start_cleanup(ui.clone(), settings, cleanup_running.clone());
            }
        }
    })
}

fn pump_messages() {
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn spawn_hotkey(
    ui: Weak<MainWindow>,
    state: Arc<Mutex<Settings>>,
    cleanup_running: Arc<AtomicBool>,
    shutdown: Arc<Shutdown>,
) -> (Sender<String>, JoinHandle<()>) {
    let hotkey = lock_or_recover(&state).hotkey.clone();
    let (sender, updates): (Sender<String>, Receiver<String>) = mpsc::channel();
    let handle = thread::spawn(move || {
        let Ok(manager) = GlobalHotKeyManager::new() else {
            log("Hotkey manager creation failed.");
            return;
        };
        let mut registered = hotkey.parse().ok();
        if let Some(hotkey) = registered {
            if let Err(error) = manager.register(hotkey) {
                log(format!(
                    "Hotkey registration failed for {hotkey:?}: {error}"
                ));
                registered = None;
            } else {
                log(format!("Hotkey registered: {hotkey:?}"));
            }
        } else {
            log(format!("Hotkey parse failed: {hotkey}"));
        }
        while !shutdown.is_requested() {
            pump_messages();
            if let Ok(next) = updates.recv_timeout(Duration::from_millis(50)) {
                if let Some(hotkey) = registered {
                    let _ = manager.unregister(hotkey);
                }
                registered = next.parse().ok();
                if let Some(hotkey) = registered {
                    if let Err(error) = manager.register(hotkey) {
                        log(format!("Hotkey registration failed for {next}: {error}"));
                        registered = None;
                    } else {
                        log(format!("Hotkey registered: {next}"));
                    }
                } else {
                    log(format!("Hotkey parse failed: {next}"));
                }
            }
            while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
                if event.state == HotKeyState::Pressed {
                    log("Hotkey pressed. Starting manual cleanup.");
                    let settings = lock_or_recover(&state).clone();
                    start_cleanup(ui.clone(), settings, cleanup_running.clone());
                }
            }
        }
        if let Some(hotkey) = registered {
            let _ = manager.unregister(hotkey);
        }
    });
    (sender, handle)
}

pub fn run(show_event: HANDLE) -> Result<(), String> {
    let state = Arc::new(Mutex::new(load_settings()));
    let ui = MainWindow::new().map_err(|error| error.to_string())?;
    let initial_settings = lock_or_recover(&state).clone();
    sync_ui(&ui, &initial_settings);
    ui.set_current_version(format!("v{}", env!("CARGO_PKG_VERSION")).into());

    let (menu, tray) = TrayMenu::new(&initial_settings)?;
    let tray_icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu.clone()))
        .with_tooltip("RAMOpt")
        .with_icon(icon()?)
        .build()
        .map_err(|error| format!("failed to create tray icon: {error:?}"))?;

    let (used, total) = memory_status();
    ui.set_memory_used(gb(used).into());
    ui.set_memory_total(gb(total).into());
    ui.set_memory_percent(if total > 0 {
        used as f32 / total as f32
    } else {
        0.0
    });
    ui.set_memory_available(true);
    let _ = tray_icon.set_tooltip(Some(memory_tooltip(used, total)));

    let shutdown = Arc::new(Shutdown::new());
    let cleanup_running = Arc::new(AtomicBool::new(false));
    let (hotkey_updates, hotkey_thread) = spawn_hotkey(
        ui.as_weak(),
        state.clone(),
        cleanup_running.clone(),
        shutdown.clone(),
    );
    let show_thread =
        spawn_show_event_listener(ui.as_weak(), show_event as usize, shutdown.clone());

    let memory_tray = tray_icon.clone();
    let memory_ui = ui.as_weak();
    let memory_timer = slint::Timer::default();
    memory_timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(5000),
        move || {
            let (used, total) = memory_status();
            let percent = if total > 0 {
                used as f32 / total as f32
            } else {
                0.0
            };
            if let Some(ui) = memory_ui.upgrade() {
                ui.set_memory_used(gb(used).into());
                ui.set_memory_total(gb(total).into());
                ui.set_memory_percent(percent);
            }
            let _ = memory_tray.set_tooltip(Some(memory_tooltip(used, total)));
        },
    );

    let weak = ui.as_weak();
    let save_state = state.clone();
    let save_hotkey_updates = hotkey_updates.clone();
    let save_tray = tray.clone();
    ui.on_save_settings(move || {
        if let Some(ui) = weak.upgrade() {
            persist(&ui, &save_state, &save_hotkey_updates);
            save_tray.sync_checks(&lock_or_recover(&save_state));
        }
    });

    let weak = ui.as_weak();
    let default_state = state.clone();
    let default_hotkey_updates = hotkey_updates.clone();
    let default_tray = tray.clone();
    ui.on_restore_defaults(move || {
        if let Some(ui) = weak.upgrade() {
            let settings = Settings::default();
            let previous = lock_or_recover(&default_state).clone();
            if let Err(error) = set_startup(false) {
                ui.set_status(format!("Startup setting failed: {error}").into());
                return;
            }
            if let Err(error) = save_settings(&settings) {
                let _ = set_startup(previous.start_with_windows);
                ui.set_status(format!("Settings save failed: {error}").into());
                log(format!("Settings save failed restoring defaults: {error}"));
                return;
            }
            let _ = default_hotkey_updates.send(settings.hotkey.clone());
            *lock_or_recover(&default_state) = settings.clone();
            default_tray.sync_checks(&settings);
            sync_ui(&ui, &settings);
        }
    });

    let weak = ui.as_weak();
    let clean_state = state.clone();
    let clean_running = cleanup_running.clone();
    ui.on_clean_now(move || {
        if let Some(ui) = weak.upgrade() {
            let settings = read_ui(&ui);
            *lock_or_recover(&clean_state) = settings.clone();
            ui.set_status("Cleaning RAM...".into());
            start_cleanup(ui.as_weak(), settings, clean_running.clone());
        }
    });

    let weak = ui.as_weak();
    ui.on_update_now(move || {
        if let Some(ui) = weak.upgrade() {
            match start_update() {
                Ok(()) => {
                    ui.set_status(
                        "Downloading update... RAMOpt will restart automatically.".into(),
                    );
                    let _ = slint::quit_event_loop();
                }
                Err(error) => ui.set_status(format!("Update failed to start: {error}").into()),
            }
        }
    });

    let weak = ui.as_weak();
    ui.on_hide_window(move || {
        if let Some(ui) = weak.upgrade()
            && let Err(error) = ui.hide()
        {
            log(format!("Hide window failed: {error}"));
        }
    });

    let close_state = state.clone();
    ui.window().on_close_requested(move || {
        if lock_or_recover(&close_state).close_to_tray {
            slint::CloseRequestResponse::HideWindow
        } else {
            let _ = slint::quit_event_loop();
            slint::CloseRequestResponse::HideWindow
        }
    });

    ui.show().map_err(|error| error.to_string())?;
    set_window_icon(&ui);
    center_window(&ui);
    let window_ui = ui.as_weak();
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        if let Some(ui) = window_ui.upgrade() {
            set_window_icon(&ui);
            center_window(&ui);
        }
    });

    let update_available = Arc::new(AtomicBool::new(false));
    let update_inserted = Arc::new(AtomicBool::new(false));
    let update_menu_timer = slint::Timer::default();
    let update_menu = menu;
    let update_menu_item = tray.update.clone();
    let update_menu_separator = tray.update_separator.clone();
    let update_menu_available = update_available.clone();
    let update_menu_inserted = update_inserted.clone();
    update_menu_timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(250),
        move || {
            if update_menu_available.load(Ordering::Acquire)
                && !update_menu_inserted.swap(true, Ordering::AcqRel)
            {
                let _ = update_menu.insert(&update_menu_item, 0);
                let _ = update_menu.insert(&update_menu_separator, 1);
            }
        },
    );

    let ids = (
        tray.update.id().clone(),
        tray.show.id().clone(),
        tray.clean.id().clone(),
        tray.auto.id().clone(),
        tray.temp.id().clone(),
        tray.apps.id().clone(),
        tray.startup.id().clone(),
        tray.exit.id().clone(),
    );
    let tray_thread = spawn_tray_events(
        ui.as_weak(),
        state.clone(),
        cleanup_running.clone(),
        shutdown.clone(),
        ids,
    );
    let timer_thread = spawn_timer(
        ui.as_weak(),
        state.clone(),
        cleanup_running,
        shutdown.clone(),
    );
    let update_thread = spawn_update_checks(ui.as_weak(), update_available, shutdown.clone());

    let event_loop_result = slint::run_event_loop_until_quit().map_err(|error| error.to_string());
    shutdown.request();
    unsafe {
        let _ = windows_sys::Win32::System::Threading::SetEvent(show_event);
    }
    for (name, handle) in [
        ("tray", tray_thread),
        ("show", show_thread),
        ("timer", timer_thread),
        ("hotkey", hotkey_thread),
        ("update", update_thread),
    ] {
        if handle.join().is_err() {
            log(format!("{name} worker panicked during shutdown."));
        }
    }
    drop(memory_timer);
    drop(update_menu_timer);
    drop(tray_icon);
    event_loop_result
}

fn icon() -> Result<Icon, String> {
    let image = image::load_from_memory(include_bytes!("../assets/ramopt.ico"))
        .map_err(|error| format!("invalid tray icon: {error}"))?
        .into_rgba8();
    let (width, height) = (image.width(), image.height());
    Icon::from_rgba(image.into_raw(), width, height)
        .map_err(|error| format!("invalid tray icon: {error:?}"))
}

fn spawn_update_checks(
    ui: Weak<MainWindow>,
    update_available: Arc<AtomicBool>,
    shutdown: Arc<Shutdown>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        while !shutdown.is_requested() {
            match latest_release_version() {
                Ok(version) if version_is_newer(&version) => {
                    update_available.store(true, Ordering::Release);
                    invoke_ui(&ui, move |ui| ui.set_update_version(version.into()));
                }
                Ok(_) => update_available.store(false, Ordering::Release),
                Err(error) => log(format!("Update check failed: {error}")),
            }
            if shutdown.wait(UPDATE_CHECK_INTERVAL) {
                break;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_deserialize_old_format_with_defaults() {
        let settings: Settings = serde_json::from_str(
            r#"{"auto_clean":true,"interval_minutes":10,"hotkey":"ctrl+alt+KeyR","clean_temp":false,"trim_background_apps":false,"start_with_windows":true}"#,
        )
        .expect("valid settings");
        assert!(settings.close_to_tray);
        assert!(!settings.dark_mode);
    }

    #[test]
    fn settings_default_is_safe() {
        let settings = Settings::default();
        assert_eq!(settings.interval_minutes, 15);
        assert!(!settings.trim_background_apps);
        assert!(!settings.start_with_windows);
        assert!(settings.close_to_tray);
    }

    #[test]
    fn version_comparison_accepts_v_prefix() {
        assert!(version_is_newer("v999.0.0"));
        assert!(!version_is_newer(env!("CARGO_PKG_VERSION")));
        assert!(!version_is_newer("not-a-version"));
    }
}
