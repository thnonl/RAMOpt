use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::{ComponentHandle, Weak};
use std::{fs, os::windows::process::CommandExt, path::PathBuf, process::Command, sync::{atomic::{AtomicBool, Ordering}, mpsc::{self, Receiver, Sender}, Arc, Mutex}, thread, time::Duration};
use tray_icon::{menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem}, Icon, TrayIconBuilder, TrayIconEvent};
use winreg::{enums::HKEY_CURRENT_USER, RegKey};
use windows_sys::Win32::{Foundation::{HANDLE, HWND, RECT, WAIT_OBJECT_0}, System::{LibraryLoader::GetModuleHandleW, Threading::WaitForSingleObject}, UI::WindowsAndMessaging::{DispatchMessageW, GetSystemMetrics, GetWindowRect, LoadImageW, MSG, PeekMessageW, PM_REMOVE, SendMessageW, SetWindowPos, SWP_NOSIZE, SWP_NOZORDER, SM_CXSCREEN, SM_CYSCREEN, TranslateMessage, IMAGE_ICON, LR_DEFAULTSIZE, WM_SETICON, ICON_SMALL, ICON_BIG}};

slint::include_modules!();

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_VALUE: &str = "RAMOpt";
const RELEASE_API_URL: &str = "https://api.github.com/repos/thnonl/RAMOpt/releases/latest";
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Settings { pub auto_clean: bool, pub interval_minutes: u32, pub hotkey: String, pub clean_temp: bool, pub trim_background_apps: bool, pub start_with_windows: bool, #[serde(default = "default_close_to_tray")] pub close_to_tray: bool, #[serde(default)] pub dark_mode: bool }
fn default_close_to_tray() -> bool { true }
impl Default for Settings { fn default() -> Self { Self { auto_clean: true, interval_minutes: 15, hotkey: "ctrl+alt+KeyR".into(), clean_temp: true, trim_background_apps: true, start_with_windows: false, close_to_tray: default_close_to_tray(), dark_mode: false } } }

fn app_directory() -> PathBuf { std::env::current_exe().ok().and_then(|path| path.parent().map(PathBuf::from)).unwrap_or_else(|| PathBuf::from(".")) }
fn settings_path() -> PathBuf { app_directory().join("settings.json") }
fn updater_path() -> PathBuf { app_directory().join("RAMOpt-updater.bat") }
fn log_path() -> PathBuf { settings_path().with_file_name("ramopt.log") }
fn log(message: impl std::fmt::Display) {
    let path = log_path();
    if let Some(folder) = path.parent() { let _ = fs::create_dir_all(folder); }
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        use std::io::Write;
        let _ = writeln!(file, "{} | {message}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|time| time.as_secs()).unwrap_or_default());
    }
}
pub fn load_settings() -> Settings { fs::read_to_string(settings_path()).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default() }
pub fn save_settings(settings: &Settings) { let path = settings_path(); if let Some(folder) = path.parent() { let _ = fs::create_dir_all(folder); } if let Ok(json) = serde_json::to_string_pretty(settings) { let _ = fs::write(path, json); } }
fn set_startup(enabled: bool) -> Result<(), String> { let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_KEY, winreg::enums::KEY_WRITE).map_err(|e| e.to_string())?; if enabled { let exe = std::env::current_exe().map_err(|e| e.to_string())?; key.set_value(RUN_VALUE, &format!("\"{}\"", exe.display())).map_err(|e| e.to_string())?; } else { let _ = key.delete_value(RUN_VALUE); } Ok(()) }
fn clear_temp_folder(folder: PathBuf) -> u64 { fs::read_dir(folder).map(|entries| entries.flatten().filter(|entry| { let path = entry.path(); if path.is_dir() { fs::remove_dir_all(path).is_ok() } else { fs::remove_file(path).is_ok() } }).count() as u64).unwrap_or(0) }
fn clear_temp() -> u64 { clear_temp_folder(std::env::temp_dir()) + clear_temp_folder(PathBuf::from(r"C:\Windows\Temp")) }
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
fn trim_working_sets() -> f64 { let script = "$ErrorActionPreference='SilentlyContinue'; $before=(Get-Process | Measure-Object -Property WorkingSet64 -Sum).Sum; Get-Process | ForEach-Object { try { $_.MinWorkingSet=$_.MinWorkingSet } catch {} }; $after=(Get-Process | Measure-Object -Property WorkingSet64 -Sum).Sum; [math]::Max(0, ($before-$after)/1MB)"; Command::new("powershell.exe").creation_flags(CREATE_NO_WINDOW).args(["-NoProfile", "-NonInteractive", "-Command", script]).output().ok().and_then(|o| String::from_utf8(o.stdout).ok()).and_then(|text| text.trim().replace(',', ".").parse().ok()).unwrap_or(0.0) }
fn close_background_apps() -> u32 { let names = ["OneDrive.exe", "Teams.exe", "AdobeIPCBroker.exe", "AdobeCollabSync.exe", "ArmouryCrate.UserSessionHelper.exe", "GameSDK.exe", "NahimicService.exe", "AuraService.exe"]; names.iter().filter(|name| Command::new("taskkill.exe").creation_flags(CREATE_NO_WINDOW).args(["/F", "/IM", name]).output().map(|o| o.status.success()).unwrap_or(false)).count() as u32 }
pub fn clean_memory(settings: &Settings) -> String { let freed_mb = trim_working_sets(); if settings.clean_temp { clear_temp(); } if settings.trim_background_apps { close_background_apps(); } format!("Memory cleaned: {freed_mb:.1} MB") }

fn version_is_newer(tag: &str) -> bool {
    fn parts(version: &str) -> Option<Vec<u32>> { version.trim_start_matches('v').split('.').map(str::parse).collect::<Result<_, _>>().ok() }
    match (parts(tag), parts(env!("CARGO_PKG_VERSION"))) { (Some(latest), Some(current)) => latest > current, _ => false }
}
fn latest_release_version() -> Result<String, String> {
    let script = format!("$ErrorActionPreference='Stop'; (Invoke-RestMethod -Headers @{{'User-Agent'='RAMOpt'}} -Uri '{RELEASE_API_URL}').tag_name");
    let output = Command::new("powershell.exe").creation_flags(CREATE_NO_WINDOW).args(["-NoProfile", "-NonInteractive", "-Command", &script]).output().map_err(|error| error.to_string())?;
    if !output.status.success() { return Err(String::from_utf8_lossy(&output.stderr).trim().to_string()); }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
fn spawn_update_checks(ui: Weak<MainWindow>, update_available: Arc<AtomicBool>) {
    thread::spawn(move || loop {
        match latest_release_version() {
            Ok(version) if version_is_newer(&version) => { update_available.store(true, Ordering::Release); let _ = slint::invoke_from_event_loop({ let ui = ui.clone(); move || if let Some(ui) = ui.upgrade() { ui.set_update_version(version.into()); } }); }
            Ok(_) => update_available.store(false, Ordering::Release),
            Err(error) => log(format!("Update check failed: {error}")),
        }
        thread::sleep(UPDATE_CHECK_INTERVAL);
    });
}
fn start_update() -> Result<(), String> {
    let updater = updater_path();
    if !updater.is_file() { return Err(format!("Updater not found: {}", updater.display())); }
    Command::new("cmd.exe").args(["/C", "start", "", &updater.to_string_lossy()]).spawn().map_err(|error| error.to_string())?;
    Ok(())
}

#[derive(Clone)]
struct TrayMenu { update: MenuItem, update_separator: PredefinedMenuItem, show: MenuItem, clean: MenuItem, auto: CheckMenuItem, temp: CheckMenuItem, apps: CheckMenuItem, startup: CheckMenuItem, exit: MenuItem }
impl TrayMenu {
    fn new(settings: &Settings) -> (Menu, Self) { let menu = Menu::new(); let items = Self { update: MenuItem::new("★ Update now", true, None), update_separator: PredefinedMenuItem::separator(), show: MenuItem::new("Show RAMOpt", true, None), clean: MenuItem::new("Clean RAM now", true, None), auto: CheckMenuItem::new("Scheduled cleanup", true, settings.auto_clean, None), temp: CheckMenuItem::new("Clean temp files", true, settings.clean_temp, None), apps: CheckMenuItem::new("Close background apps", true, settings.trim_background_apps, None), startup: CheckMenuItem::new("Start with Windows", true, settings.start_with_windows, None), exit: MenuItem::new("Exit RAMOpt", true, None) }; let cleanup_separator = PredefinedMenuItem::separator(); let exit_separator = PredefinedMenuItem::separator(); for item in [&items.show, &items.clean] { menu.append(item).unwrap(); } menu.append(&cleanup_separator).unwrap(); menu.append(&items.auto).unwrap(); menu.append(&items.temp).unwrap(); menu.append(&items.apps).unwrap(); menu.append(&items.startup).unwrap(); menu.append(&exit_separator).unwrap(); menu.append(&items.exit).unwrap(); (menu, items) }
    fn sync_checks(&self, settings: &Settings) { self.auto.set_checked(settings.auto_clean); self.temp.set_checked(settings.clean_temp); self.apps.set_checked(settings.trim_background_apps); self.startup.set_checked(settings.start_with_windows); }
}
fn icon() -> Icon { let image = image::load_from_memory(include_bytes!("../assets/ramopt.ico")).expect("invalid tray icon").into_rgba8(); let (width, height) = (image.width(), image.height()); Icon::from_rgba(image.into_raw(), width, height).expect("invalid tray icon") }
fn set_window_icon(ui: &MainWindow) {
    let handle = ui.window().window_handle();
    let Ok(handle) = handle.window_handle() else { return; };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else { return; };
    let hwnd = handle.hwnd.get() as HWND;
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let icon = LoadImageW(instance, 1usize as *const u16, IMAGE_ICON, 0, 0, LR_DEFAULTSIZE);
        if !icon.is_null() {
            SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, icon as isize);
            SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, icon as isize);
        }
    }
}
fn center_window(ui: &MainWindow) {
    let handle = ui.window().window_handle();
    let Ok(handle) = handle.window_handle() else { return; };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else { return; };
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
            SetWindowPos(hwnd, std::ptr::null_mut(), x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER);
        }
    }
}
fn sync_ui(ui: &MainWindow, s: &Settings) { ui.set_auto_clean(s.auto_clean); ui.set_interval_minutes(s.interval_minutes as i32); ui.set_hotkey(s.hotkey.clone().into()); ui.set_clean_temp(s.clean_temp); ui.set_close_apps(s.trim_background_apps); ui.set_startup(s.start_with_windows); ui.set_close_to_tray(s.close_to_tray); ui.set_dark_mode(s.dark_mode); }
fn read_ui(ui: &MainWindow) -> Settings { Settings { auto_clean: ui.get_auto_clean(), interval_minutes: ui.get_interval_minutes().clamp(1, 1440) as u32, hotkey: ui.get_hotkey().to_string(), clean_temp: ui.get_clean_temp(), trim_background_apps: ui.get_close_apps(), start_with_windows: ui.get_startup(), close_to_tray: ui.get_close_to_tray(), dark_mode: ui.get_dark_mode() } }
fn persist(ui: &MainWindow, state: &Arc<Mutex<Settings>>, hotkey_updates: &Sender<String>) { let settings = read_ui(ui); if let Err(error) = set_startup(settings.start_with_windows) { ui.set_status(format!("Startup setting failed: {error}").into()); return; } let previous_hotkey = state.lock().unwrap().hotkey.clone(); if settings.hotkey != previous_hotkey { let _ = hotkey_updates.send(settings.hotkey.clone()); } save_settings(&settings); *state.lock().unwrap() = settings; }
fn push_log(ui: &MainWindow, message: &str) { let logs = ui.get_logs(); let lines: Vec<String> = logs.lines().chain(std::iter::once(message)).filter(|line| !line.is_empty()).map(|line| format!("• {}", line.trim_start_matches("• "))).rev().take(5).collect(); ui.set_logs(lines.into_iter().rev().collect::<Vec<_>>().join("\n").into()); }

pub fn run(show_event: HANDLE) -> Result<(), slint::PlatformError> {
    let state = Arc::new(Mutex::new(load_settings())); let ui = MainWindow::new()?; sync_ui(&ui, &state.lock().unwrap()); ui.set_current_version(format!("v{}", env!("CARGO_PKG_VERSION")).into());
    let (menu, tray) = TrayMenu::new(&state.lock().unwrap()); let update_menu = menu.clone(); let _tray = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("RAMOpt").with_icon(icon()).build().expect("failed to create tray icon");
    let hotkey_updates = spawn_hotkey(ui.as_weak(), state.clone());
    spawn_show_event_listener(ui.as_weak(), show_event);
    let weak = ui.as_weak(); let save_state = state.clone(); let save_hotkey_updates = hotkey_updates.clone(); let save_tray = tray.clone(); ui.on_save_settings(move || if let Some(ui) = weak.upgrade() { persist(&ui, &save_state, &save_hotkey_updates); save_tray.sync_checks(&save_state.lock().unwrap()); });
    let weak = ui.as_weak(); let default_state = state.clone(); let default_hotkey_updates = hotkey_updates.clone(); let default_tray = tray.clone(); ui.on_restore_defaults(move || if let Some(ui) = weak.upgrade() { let settings = Settings::default(); if let Err(error) = set_startup(false) { ui.set_status(format!("Startup setting failed: {error}").into()); return; } let _ = default_hotkey_updates.send(settings.hotkey.clone()); save_settings(&settings); *default_state.lock().unwrap() = settings.clone(); default_tray.sync_checks(&settings); sync_ui(&ui, &settings); });
    let weak = ui.as_weak(); let clean_state = state.clone(); ui.on_clean_now(move || { if let Some(ui) = weak.upgrade() { let settings = read_ui(&ui); *clean_state.lock().unwrap() = settings.clone(); ui.set_status("Cleaning RAM...".into()); let weak = ui.as_weak(); thread::spawn(move || { let status = clean_memory(&settings); log(&status); let _ = slint::invoke_from_event_loop(move || if let Some(ui) = weak.upgrade() { ui.set_status(status.clone().into()); push_log(&ui, &status); }); }); } });
    let weak = ui.as_weak(); ui.on_update_now(move || if let Some(ui) = weak.upgrade() { match start_update() { Ok(()) => { ui.set_status("Downloading update... RAMOpt will restart automatically.".into()); let _ = slint::quit_event_loop(); } Err(error) => ui.set_status(format!("Update failed to start: {error}").into()), } });
    let weak = ui.as_weak(); ui.on_hide_window(move || if let Some(ui) = weak.upgrade() { ui.hide().unwrap(); });
    let close_state = state.clone();
    ui.window().on_close_requested(move || {
        if close_state.lock().unwrap().close_to_tray {
            slint::CloseRequestResponse::HideWindow
        } else {
            let _ = slint::quit_event_loop();
            slint::CloseRequestResponse::HideWindow
        }
    });
    ui.show()?;
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
    let update_menu_item = tray.update.clone();
    let update_menu_separator = tray.update_separator.clone();
    let update_menu_available = update_available.clone();
    let update_menu_inserted = update_inserted.clone();
    update_menu_timer.start(slint::TimerMode::Repeated, Duration::from_millis(250), move || {
        if update_menu_available.load(Ordering::Acquire) && !update_menu_inserted.swap(true, Ordering::AcqRel) {
            let _ = update_menu.insert(&update_menu_item, 0);
            let _ = update_menu.insert(&update_menu_separator, 1);
        }
    });
    let ids = (tray.update.id().clone(), tray.show.id().clone(), tray.clean.id().clone(), tray.auto.id().clone(), tray.temp.id().clone(), tray.apps.id().clone(), tray.startup.id().clone(), tray.exit.id().clone());
    spawn_tray_events(ui.as_weak(), state.clone(), ids);
    spawn_timer(ui.as_weak(), state.clone());
    spawn_update_checks(ui.as_weak(), update_available);
    slint::run_event_loop_until_quit()
}
fn spawn_tray_events(ui: Weak<MainWindow>, state: Arc<Mutex<Settings>>, ids: (tray_icon::menu::MenuId, tray_icon::menu::MenuId, tray_icon::menu::MenuId, tray_icon::menu::MenuId, tray_icon::menu::MenuId, tray_icon::menu::MenuId, tray_icon::menu::MenuId, tray_icon::menu::MenuId)) { thread::spawn(move || loop { if let Ok(event) = MenuEvent::receiver().recv_timeout(Duration::from_millis(200)) { let mut settings = state.lock().unwrap(); if event.id == ids.0 { drop(settings); match start_update() { Ok(()) => { let _ = slint::invoke_from_event_loop({ let ui = ui.clone(); move || if let Some(ui) = ui.upgrade() { ui.set_status("Downloading update... RAMOpt will restart automatically.".into()); } }); let _ = slint::quit_event_loop(); return; } Err(error) => log(format!("Update failed to start: {error}")), } } else if event.id == ids.1 { drop(settings); let _ = slint::invoke_from_event_loop({ let ui = ui.clone(); move || if let Some(ui) = ui.upgrade() { ui.show().unwrap(); set_window_icon(&ui); } }); } else if event.id == ids.2 { let copy = settings.clone(); drop(settings); let weak = ui.clone(); thread::spawn(move || { let status = clean_memory(&copy); let _ = slint::invoke_from_event_loop(move || if let Some(ui) = weak.upgrade() { ui.set_status(status.into()); }); }); } else { if event.id == ids.3 { settings.auto_clean = !settings.auto_clean; } if event.id == ids.4 { settings.clean_temp = !settings.clean_temp; } if event.id == ids.5 { settings.trim_background_apps = !settings.trim_background_apps; } if event.id == ids.6 { settings.start_with_windows = !settings.start_with_windows; let _ = set_startup(settings.start_with_windows); } if event.id == ids.7 { let _ = slint::quit_event_loop(); return; } save_settings(&settings); let copy = settings.clone(); drop(settings); let _ = slint::invoke_from_event_loop({ let ui = ui.clone(); move || if let Some(ui) = ui.upgrade() { sync_ui(&ui, &copy); } }); } } let _ = TrayIconEvent::receiver().try_recv(); }); }
fn spawn_show_event_listener(ui: Weak<MainWindow>, show_event: HANDLE) { let show_event = show_event as usize; thread::spawn(move || loop { if unsafe { WaitForSingleObject(show_event as HANDLE, 200) } == WAIT_OBJECT_0 { let _ = slint::invoke_from_event_loop({ let ui = ui.clone(); move || if let Some(ui) = ui.upgrade() { ui.show().unwrap(); set_window_icon(&ui); } }); } }); }
fn spawn_timer(ui: Weak<MainWindow>, state: Arc<Mutex<Settings>>) { thread::spawn(move || loop { let minutes = { state.lock().unwrap().interval_minutes.max(1) }; thread::sleep(Duration::from_secs(u64::from(minutes) * 60)); let settings = state.lock().unwrap().clone(); if settings.auto_clean { let status = clean_memory(&settings); log(&status); let _ = slint::invoke_from_event_loop({ let ui = ui.clone(); move || if let Some(ui) = ui.upgrade() { ui.set_status(status.clone().into()); push_log(&ui, &status); } }); } }); }
fn pump_messages() {
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
fn spawn_hotkey(ui: Weak<MainWindow>, state: Arc<Mutex<Settings>>) -> Sender<String> {
    let hotkey = state.lock().unwrap().hotkey.clone();
    let (sender, updates): (Sender<String>, Receiver<String>) = mpsc::channel();
    thread::spawn(move || {
        let Ok(manager) = GlobalHotKeyManager::new() else { log("Hotkey manager creation failed."); return; };
        let mut registered = hotkey.parse().ok();
        if let Some(hotkey) = registered {
            if let Err(error) = manager.register(hotkey) { log(format!("Hotkey registration failed for {hotkey:?}: {error}")); registered = None; } else { log(format!("Hotkey registered: {hotkey:?}")); }
        } else { log(format!("Hotkey parse failed: {hotkey}")); }
        loop {
            pump_messages();
            if let Ok(next) = updates.recv_timeout(Duration::from_millis(50)) {
                if let Some(hotkey) = registered { let _ = manager.unregister(hotkey); }
                registered = next.parse().ok();
                if let Some(hotkey) = registered {
                    if let Err(error) = manager.register(hotkey) { log(format!("Hotkey registration failed for {next}: {error}")); registered = None; } else { log(format!("Hotkey registered: {next}")); }
                } else { log(format!("Hotkey parse failed: {next}")); }
            }
            while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
                log(format!("Hotkey event received: id={:?}, state={:?}", event.id, event.state));
                if event.state == HotKeyState::Pressed {
                    log("Hotkey pressed. Starting manual cleanup.");
                    let settings = state.lock().unwrap().clone();
                    let status = clean_memory(&settings);
                    log(format!("Hotkey cleanup completed: {status}"));
                    let _ = slint::invoke_from_event_loop({ let ui = ui.clone(); move || if let Some(ui) = ui.upgrade() { ui.set_status(status.clone().into()); push_log(&ui, &status); } });
                }
            }
        }
    });
    sender
}
