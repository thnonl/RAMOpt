use ramopt::backend::{self, Settings};
use serde::Serialize;
use std::{
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{
    AppHandle, Emitter, Manager, State,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tauri_plugin_autostart::ManagerExt as AutostartManagerExt;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

const RELEASE_API_URL: &str = "https://api.github.com/repos/thnonl/RAMOpt/releases/latest";
const THRESHOLD_INTERVAL: Duration = Duration::from_secs(60);
const HOTKEY_RETRY_INTERVAL: Duration = Duration::from_secs(10);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
static UPDATE_STARTED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Serialize)]
struct MemorySnapshot {
    used_gb: f64,
    total_gb: f64,
    percent: f64,
    available: bool,
}
#[derive(Clone, Serialize)]
struct LogEntry {
    time_ms: u64,
    source: String,
    level: &'static str,
    message: String,
}
#[derive(Clone, Serialize)]
struct AppSnapshot {
    settings: Settings,
    memory: MemorySnapshot,
    status: String,
    logs: Vec<LogEntry>,
    update_version: Option<String>,
    current_version: String,
    cleaning: bool,
}
struct RuntimeState {
    settings: Mutex<Settings>,
    status: Mutex<String>,
    logs: Mutex<Vec<LogEntry>>,
    update: Mutex<Option<String>>,
    used: Mutex<u64>,
    total: Mutex<u64>,
    cleaning: AtomicBool,
    last_cleanup: Mutex<Option<Instant>>,
    shutdown: AtomicBool,
}
impl RuntimeState {
    fn new(mut settings: Settings) -> Self {
        settings.interval_minutes = settings.interval_minutes.clamp(1, 1440);
        settings.threshold_percent = settings.threshold_percent.clamp(1, 100);
        if settings.auto_clean {
            settings.auto_threshold = false;
        }
        Self {
            settings: Mutex::new(settings),
            status: Mutex::new("Ready".into()),
            logs: Mutex::new(Vec::new()),
            update: Mutex::new(None),
            used: Mutex::new(0),
            total: Mutex::new(0),
            cleaning: AtomicBool::new(false),
            last_cleanup: Mutex::new(None),
            shutdown: AtomicBool::new(false),
        }
    }
}
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}
fn app_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}
fn startup(a: &AppHandle, on: bool) -> Result<(), String> {
    if on {
        a.autolaunch().enable()
    } else {
        a.autolaunch().disable()
    }
    .map_err(|e| e.to_string())
}
fn sync_startup(a: &AppHandle, s: &Settings) {
    if let Ok(on) = a.autolaunch().is_enabled()
        && on != s.start_with_windows
        && let Err(e) = startup(a, s.start_with_windows)
    {
        backend::log(format!("Startup sync failed: {e}"));
    }
}
fn winbin(n: &str) -> PathBuf {
    std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("System32")
        .join(n)
}
fn run_cmd(mut c: Command, d: Duration) -> Result<Output, String> {
    let mut ch = c
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let end = Instant::now() + d;
    loop {
        match ch.try_wait() {
            Ok(Some(_)) => return ch.wait_with_output().map_err(|e| e.to_string()),
            Ok(None) if Instant::now() >= end => {
                let _ = ch.kill();
                let _ = ch.wait();
                return Err("Command timed out".into());
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(e) => {
                let _ = ch.kill();
                let _ = ch.wait();
                return Err(e.to_string());
            }
        }
    }
}
fn memory() -> Option<(u64, u64)> {
    backend::memory_status()
}
fn snapshot(s: &RuntimeState) -> AppSnapshot {
    let used = *lock(&s.used);
    let total = *lock(&s.total);
    AppSnapshot {
        settings: lock(&s.settings).clone(),
        memory: MemorySnapshot {
            used_gb: used as f64 / 1_073_741_824.0,
            total_gb: total as f64 / 1_073_741_824.0,
            percent: if total > 0 {
                used as f64 / total as f64 * 100.0
            } else {
                0.0
            },
            available: total > 0,
        },
        status: lock(&s.status).clone(),
        logs: lock(&s.logs).clone(),
        update_version: lock(&s.update).clone(),
        current_version: format!("v{}", env!("CARGO_PKG_VERSION")),
        cleaning: s.cleaning.load(Ordering::Acquire),
    }
}
fn emit(a: &AppHandle) {
    if let Some(s) = a.try_state::<Arc<RuntimeState>>() {
        let _ = a.emit("app-state-changed", snapshot(&s));
    }
}
const LOG_CAPACITY: usize = 20;
fn set_status(s: &RuntimeState, msg: impl Into<String>) {
    *lock(&s.status) = msg.into();
}
/// Record one activity entry for the UI and the on-disk log.
fn push_log(s: &RuntimeState, source: &str, level: &'static str, msg: impl Into<String>) {
    let message = msg.into();
    backend::log(format!("[{source}] {message}"));
    let time_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default();
    let mut logs = lock(&s.logs);
    logs.push(LogEntry {
        time_ms,
        source: source.to_string(),
        level,
        message,
    });
    if logs.len() > LOG_CAPACITY {
        let n = logs.len() - LOG_CAPACITY;
        logs.drain(0..n);
    }
}
fn cleanup(s: &Settings) -> backend::CleanSummary {
    backend::clean_memory(s)
}
fn begin(
    a: AppHandle,
    s: Arc<RuntimeState>,
    settings: Settings,
    source: &'static str,
) -> Result<(), String> {
    if s.cleaning
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("Cleanup already running.".into());
    }
    set_status(&s, "Cleaning RAM…");
    emit(&a);
    thread::spawn(move || {
        match std::panic::catch_unwind(|| cleanup(&settings)) {
            Ok(summary) => push_log(
                &s,
                source,
                if summary.warning { "warn" } else { "ok" },
                summary.message,
            ),
            Err(_) => push_log(&s, source, "error", "Cleanup failed unexpectedly"),
        }
        // Sample right away so the meter matches the "Freed" message instead of waiting for the poll.
        if let Some((used, total)) = memory() {
            *lock(&s.used) = used;
            *lock(&s.total) = total;
        }
        set_status(&s, "Ready");
        *lock(&s.last_cleanup) = Some(Instant::now());
        s.cleaning.store(false, Ordering::Release);
        emit(&a);
    });
    Ok(())
}
#[tauri::command]
fn get_app_state(s: State<'_, Arc<RuntimeState>>) -> AppSnapshot {
    snapshot(&s)
}
#[tauri::command]
fn save_settings(
    a: AppHandle,
    s: State<'_, Arc<RuntimeState>>,
    mut settings: Settings,
) -> Result<Settings, String> {
    settings.interval_minutes = settings.interval_minutes.clamp(1, 1440);
    settings.threshold_percent = settings.threshold_percent.clamp(1, 100);
    if settings.auto_clean {
        settings.auto_threshold = false;
    }
    let old = lock(&s.settings).clone();
    if old.start_with_windows != settings.start_with_windows {
        startup(&a, settings.start_with_windows)
            .map_err(|e| format!("Startup setting failed: {e}"))?;
    }
    if let Err(error) = backend::save_settings(&settings) {
        if old.start_with_windows != settings.start_with_windows {
            let _ = startup(&a, old.start_with_windows);
        }
        return Err(error);
    }
    *lock(&s.settings) = settings.clone();
    sync_tray(&a, &settings);
    emit(&a);
    Ok(settings)
}
#[tauri::command]
fn restore_defaults(a: AppHandle, s: State<'_, Arc<RuntimeState>>) -> Result<Settings, String> {
    let old = lock(&s.settings).clone();
    let settings = Settings::default();
    if old.start_with_windows {
        startup(&a, false)?;
    }
    if let Err(e) = backend::save_settings(&settings) {
        if old.start_with_windows {
            let _ = startup(&a, true);
        }
        return Err(e);
    }
    *lock(&s.settings) = settings.clone();
    sync_tray(&a, &settings);
    push_log(&s, "Settings", "info", "Settings restored to defaults");
    emit(&a);
    Ok(settings)
}
#[tauri::command]
fn clean_now(a: AppHandle, s: State<'_, Arc<RuntimeState>>) -> Result<(), String> {
    let settings = lock(&s.settings).clone();
    let shared = a.state::<Arc<RuntimeState>>().inner().clone();
    begin(a, shared, settings, "Manual")
}
#[tauri::command]
fn hide_window(w: tauri::WebviewWindow) -> Result<(), String> {
    w.hide().map_err(|e| e.to_string())
}
#[tauri::command]
fn install_update(a: AppHandle, s: State<'_, Arc<RuntimeState>>) -> Result<(), String> {
    if lock(&s.update).is_none() {
        return Err("No update available.".into());
    }
    start_update(&a)?;
    a.exit(0);
    Ok(())
}
fn start_update(a: &AppHandle) -> Result<(), String> {
    if UPDATE_STARTED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("Update already starting.".into());
    }
    let dir = app_dir();
    let path = dir.join("RAMOpt-updater.bat");
    if !path.exists() {
        let resource = a
            .path()
            .resource_dir()
            .map_err(|e| e.to_string())?
            .join("RAMOpt-updater.bat");
        if !resource.exists() {
            UPDATE_STARTED.store(false, Ordering::Release);
            return Err("Updater missing".into());
        }
        if let Err(e) = std::fs::copy(resource, &path) {
            UPDATE_STARTED.store(false, Ordering::Release);
            return Err(e.to_string());
        }
    }
    let raw = format!("/D /S /C \"\"{}\"\"", path.display());
    let result = Command::new(winbin("cmd.exe"))
        .creation_flags(CREATE_NO_WINDOW)
        .current_dir(dir)
        .raw_arg(raw)
        .spawn()
        .map_err(|e| e.to_string());
    if result.is_err() {
        UPDATE_STARTED.store(false, Ordering::Release);
    }
    result.map(|_| ())
}
fn latest() -> Result<String, String> {
    let query = format!(
        "$ErrorActionPreference='Stop';(Invoke-RestMethod -Headers @{{'User-Agent'='RAMOpt'}} -Uri '{RELEASE_API_URL}').tag_name"
    );
    let mut cmd = Command::new(winbin(r"WindowsPowerShell\v1.0\powershell.exe"));
    cmd.creation_flags(CREATE_NO_WINDOW).args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        &query,
    ]);
    let output = run_cmd(cmd, COMMAND_TIMEOUT)?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().into())
}
fn version_newer(tag: &str) -> bool {
    fn parse(v: &str) -> Option<Vec<u32>> {
        v.trim_start_matches('v')
            .split('.')
            .map(str::parse)
            .collect::<Result<_, _>>()
            .ok()
    }
    matches!((parse(tag), parse(env!("CARGO_PKG_VERSION"))), (Some(a), Some(b)) if a > b)
}
fn show(a: &AppHandle) {
    if let Some(w) = a.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}
#[derive(Clone)]
struct TrayItems {
    schedule: CheckMenuItem<tauri::Wry>,
    temp: CheckMenuItem<tauri::Wry>,
    apps: CheckMenuItem<tauri::Wry>,
    startup: CheckMenuItem<tauri::Wry>,
    menu: Menu<tauri::Wry>,
    update: MenuItem<tauri::Wry>,
}
impl TrayItems {
    /// A tray menu item cannot be hidden, so "Update now" is inserted only while an update exists.
    fn show_update(&self, available: bool) {
        let present = self.menu.get("update").is_some();
        if available && !present {
            let _ = self.menu.insert(&self.update, 0);
        } else if !available && present {
            let _ = self.menu.remove(&self.update);
        }
    }
}
fn sync_tray(a: &AppHandle, s: &Settings) {
    if let Some(items) = a.try_state::<TrayItems>() {
        let _ = items.schedule.set_checked(s.auto_clean);
        let _ = items.temp.set_checked(s.clean_temp);
        let _ = items.apps.set_checked(s.trim_background_apps);
        let _ = items.startup.set_checked(s.start_with_windows);
    }
}
fn tray_toggle(a: &AppHandle, s: &RuntimeState, key: &str) {
    let old = lock(&s.settings).clone();
    let mut next = old.clone();
    match key {
        "schedule" => {
            next.auto_clean = !next.auto_clean;
            if next.auto_clean {
                next.auto_threshold = false;
            }
        }
        "temp" => next.clean_temp = !next.clean_temp,
        "apps" => next.trim_background_apps = !next.trim_background_apps,
        "startup" => {
            next.start_with_windows = !next.start_with_windows;
            if let Err(e) = startup(a, next.start_with_windows) {
                push_log(
                    s,
                    "Settings",
                    "error",
                    format!("Startup setting failed: {e}"),
                );
                return;
            }
        }
        _ => return,
    }
    if let Err(e) = backend::save_settings(&next) {
        if key == "startup" {
            let _ = startup(a, old.start_with_windows);
        }
        push_log(s, "Settings", "error", format!("Settings save failed: {e}"));
        return;
    }
    *lock(&s.settings) = next.clone();
    sync_tray(a, &next);
    emit(a);
}
fn setup_tray(a: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let handle = a.handle();
    let settings = lock(&a.state::<Arc<RuntimeState>>().settings).clone();
    let update = MenuItem::with_id(handle, "update", "Update now", true, None::<&str>)?;
    let show_item = MenuItem::with_id(handle, "show", "Show RAMOpt", true, None::<&str>)?;
    let settings_item = MenuItem::with_id(handle, "settings", "App settings…", true, None::<&str>)?;
    let clean = MenuItem::with_id(handle, "clean", "Clean RAM now", true, None::<&str>)?;
    let schedule = CheckMenuItem::with_id(
        handle,
        "schedule",
        "Scheduled cleanup",
        true,
        settings.auto_clean,
        None::<&str>,
    )?;
    let temp = CheckMenuItem::with_id(
        handle,
        "temp",
        "Clean temp files",
        true,
        settings.clean_temp,
        None::<&str>,
    )?;
    let apps = CheckMenuItem::with_id(
        handle,
        "apps",
        "Close user apps (safe)",
        true,
        settings.trim_background_apps,
        None::<&str>,
    )?;
    let startup_item = CheckMenuItem::with_id(
        handle,
        "startup",
        "Start with Windows",
        true,
        settings.start_with_windows,
        None::<&str>,
    )?;
    let exit = MenuItem::with_id(handle, "exit", "Exit RAMOpt", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(handle)?;
    let sep2 = PredefinedMenuItem::separator(handle)?;
    let menu = Menu::with_items(
        handle,
        &[
            &show_item,
            &clean,
            &sep,
            &schedule,
            &temp,
            &apps,
            &startup_item,
            &sep2,
            &settings_item,
            &exit,
        ],
    )?;
    a.manage(TrayItems {
        schedule,
        temp,
        apps,
        startup: startup_item,
        menu: menu.clone(),
        update,
    });
    let state = Arc::clone(a.state::<Arc<RuntimeState>>().inner());
    TrayIconBuilder::with_id("ramopt-tray")
        .menu(&menu)
        .tooltip("RAMOpt")
        .icon(tauri::image::Image::from_bytes(include_bytes!(
            "../../assets/ramopt.ico"
        ))?)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "show" => show(app),
            "settings" => {
                show(app);
                let _ = app.emit("open-settings", ());
            }
            "clean" => {
                let settings = lock(&state.settings).clone();
                let _ = begin(app.clone(), state.clone(), settings, "Tray");
            }
            "exit" => app.exit(0),
            "update" => {
                if lock(&state.update).is_none() {
                    push_log(&state, "Update", "info", "No update available");
                    emit(app);
                } else if let Err(e) = start_update(app) {
                    push_log(&state, "Update", "error", format!("Update failed: {e}"));
                    emit(app);
                } else {
                    app.exit(0);
                }
            }
            key @ ("schedule" | "temp" | "apps" | "startup") => tray_toggle(app, &state, key),
            _ => (),
        })
        .on_tray_icon_event(|tray, event| {
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
                show(tray.app_handle());
            }
        })
        .build(a)?;
    Ok(())
}
fn workers(a: AppHandle, s: Arc<RuntimeState>) {
    let memory_app = a.clone();
    let memory_state = s.clone();
    thread::spawn(move || {
        while !memory_state.shutdown.load(Ordering::Acquire) {
            if let Some((used, total)) = memory() {
                *lock(&memory_state.used) = used;
                *lock(&memory_state.total) = total;
                if let Some(tray) = memory_app.tray_by_id("ramopt-tray") {
                    let pct = if total > 0 {
                        used as f64 / total as f64 * 100.0
                    } else {
                        0.0
                    };
                    let tip = format!(
                        "RAMOpt - RAM: {:.1}/{:.1} GB ({pct:.0}%)",
                        used as f64 / 1_073_741_824.0,
                        total as f64 / 1_073_741_824.0
                    );
                    let _ = tray.set_tooltip(Some(&tip));
                }
            } else {
                *lock(&memory_state.used) = 0;
                *lock(&memory_state.total) = 0;
            }
            emit(&memory_app);
            for _ in 0..5 {
                if memory_state.shutdown.load(Ordering::Acquire) {
                    break;
                }
                thread::sleep(Duration::from_secs(1));
            }
        }
    });
    let schedule_app = a.clone();
    let schedule_state = s.clone();
    thread::spawn(move || {
        while !schedule_state.shutdown.load(Ordering::Acquire) {
            let original = lock(&schedule_state.settings).clone();
            let mut delay =
                Duration::from_secs(u64::from(original.interval_minutes.clamp(1, 1440)) * 60);
            while !delay.is_zero() && !schedule_state.shutdown.load(Ordering::Acquire) {
                let slice = delay.min(Duration::from_millis(500));
                thread::sleep(slice);
                delay = delay.saturating_sub(slice);
                let now = lock(&schedule_state.settings).clone();
                if now.interval_minutes != original.interval_minutes || !now.auto_clean {
                    delay = Duration::ZERO;
                }
            }
            if schedule_state.shutdown.load(Ordering::Acquire) {
                break;
            }
            let current = lock(&schedule_state.settings).clone();
            if current.auto_clean && !current.auto_threshold {
                let _ = begin(
                    schedule_app.clone(),
                    schedule_state.clone(),
                    current,
                    "Scheduled",
                );
            }
        }
    });
    let threshold_app = a.clone();
    let threshold_state = s.clone();
    thread::spawn(move || {
        while !threshold_state.shutdown.load(Ordering::Acquire) {
            for _ in 0..60 {
                if threshold_state.shutdown.load(Ordering::Acquire) {
                    break;
                }
                thread::sleep(Duration::from_secs(1));
            }
            if threshold_state.shutdown.load(Ordering::Acquire) {
                break;
            }
            let settings = lock(&threshold_state.settings).clone();
            if !settings.auto_threshold
                || settings.auto_clean
                || threshold_state.cleaning.load(Ordering::Acquire)
            {
                continue;
            }
            if lock(&threshold_state.last_cleanup)
                .is_some_and(|last| last.elapsed() < THRESHOLD_INTERVAL)
            {
                continue;
            }
            if let Some((used, total)) = memory()
                && total > 0
                && used as f64 / total as f64 * 100.0
                    >= settings.threshold_percent.clamp(1, 100) as f64
            {
                let _ = begin(
                    threshold_app.clone(),
                    threshold_state.clone(),
                    settings,
                    "RAM high",
                );
            }
        }
    });
    let update_app = a;
    let update_state = s;
    thread::spawn(move || {
        while !update_state.shutdown.load(Ordering::Acquire) {
            if let Ok(tag) = latest() {
                let available = version_newer(&tag);
                *lock(&update_state.update) = if available { Some(tag) } else { None };
                if let Some(items) = update_app.try_state::<TrayItems>() {
                    items.show_update(available);
                }
                emit(&update_app);
            }
            for _ in 0..900 {
                if update_state.shutdown.load(Ordering::Acquire) {
                    break;
                }
                thread::sleep(Duration::from_secs(1));
            }
        }
    });
}
fn show_and_clean(a: &AppHandle) {
    show(a);
    if let Some(s) = a.try_state::<Arc<RuntimeState>>() {
        let settings = lock(&s.settings).clone();
        let shared = Arc::clone(s.inner());
        let _ = begin(a.clone(), shared, settings, "Hotkey");
    }
}
fn register_hotkey(a: &AppHandle, key: &str) -> Result<(), String> {
    let accelerator = match key {
        "ctrl+alt+KeyR" => "Ctrl+Alt+R",
        "ctrl+alt+KeyM" => "Ctrl+Alt+M",
        "ctrl+shift+KeyR" => "Ctrl+Shift+R",
        "ctrl+shift+KeyM" => "Ctrl+Shift+M",
        "alt+KeyR" => "Alt+R",
        "alt+KeyM" => "Alt+M",
        _ => return Err("Unsupported hotkey".into()),
    };
    a.global_shortcut()
        .unregister_all()
        .map_err(|e| e.to_string())?;
    let app = a.clone();
    a.global_shortcut()
        .on_shortcut(accelerator, move |_, _, event| {
            if event.state == ShortcutState::Pressed {
                show_and_clean(&app);
            }
        })
        .map_err(|e| e.to_string())
}
fn hotkey_worker(a: &AppHandle) {
    let state = Arc::clone(a.state::<Arc<RuntimeState>>().inner());
    let app = a.clone();
    thread::spawn(move || {
        let mut previous = String::new();
        let mut last_error = String::new();
        let mut failed_for = String::new();
        let mut retry_after: Option<Instant> = None;
        while !state.shutdown.load(Ordering::Acquire) {
            let desired = lock(&state.settings).hotkey.clone();
            // A changed hotkey is tried immediately; the same failing one is retried slowly.
            let retry_due =
                failed_for != desired || retry_after.is_none_or(|at| Instant::now() >= at);
            if previous != desired && retry_due {
                match register_hotkey(&app, &desired) {
                    Ok(()) => {
                        previous = desired;
                        last_error.clear();
                        failed_for.clear();
                        retry_after = None;
                    }
                    Err(e) => {
                        previous.clear();
                        failed_for = desired;
                        retry_after = Some(Instant::now() + HOTKEY_RETRY_INTERVAL);
                        // Another process may own the hotkey; report it once instead of every retry.
                        if e != last_error {
                            push_log(
                                &state,
                                "Hotkey",
                                "error",
                                format!("Shortcut unavailable: {e}"),
                            );
                            emit(&app);
                            last_error = e;
                        }
                    }
                }
            }
            thread::sleep(Duration::from_millis(300));
        }
        let _ = app.global_shortcut().unregister_all();
    });
}
pub fn run() {
    let state = Arc::new(RuntimeState::new(backend::load_settings()));
    let autostart = tauri_plugin_autostart::Builder::new()
        .app_name("RAMOpt")
        .build();
    let shortcuts = tauri_plugin_global_shortcut::Builder::new().build();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| show(app)))
        .plugin(shortcuts)
        .plugin(autostart)
        .setup(move |app| {
            app.manage(state.clone());
            sync_startup(app.handle(), &lock(&state.settings));
            setup_tray(app)?;
            let handle = app.handle().clone();
            workers(handle.clone(), state.clone());
            hotkey_worker(&handle);
            let local = state.clone();
            let window_app = handle.clone();
            app.get_webview_window("main")
                .expect("main configured")
                .on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event
                        && lock(&local.settings).close_to_tray
                    {
                        api.prevent_close();
                        if let Some(w) = window_app.get_webview_window("main") {
                            let _ = w.hide();
                        }
                    }
                });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_app_state,
            save_settings,
            restore_defaults,
            clean_now,
            hide_window,
            install_update
        ])
        .build(tauri::generate_context!())
        .expect("Tauri application initialization failed");
    app.run(|app, event| {
        if let tauri::RunEvent::Exit = event
            && let Some(state) = app.try_state::<Arc<RuntimeState>>()
        {
            state.shutdown.store(true, Ordering::Release);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::version_newer;

    #[test]
    fn version_comparison_accepts_v_prefix() {
        assert!(version_newer("v999.0.0"));
        assert!(!version_newer(env!("CARGO_PKG_VERSION")));
        assert!(!version_newer("not-a-version"));
    }
}
