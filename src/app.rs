use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::{ComponentHandle, Weak};
use std::{fs, os::windows::process::CommandExt, path::PathBuf, process::Command, sync::{mpsc::{self, Receiver, Sender}, Arc, Mutex}, thread, time::Duration};
use tray_icon::{menu::{CheckMenuItem, Menu, MenuEvent, MenuItem}, Icon, TrayIconBuilder, TrayIconEvent};
use winreg::{enums::HKEY_CURRENT_USER, RegKey};
use windows_sys::Win32::{Foundation::HWND, System::LibraryLoader::GetModuleHandleW, UI::WindowsAndMessaging::{LoadImageW, SendMessageW, IMAGE_ICON, LR_DEFAULTSIZE, WM_SETICON, ICON_SMALL, ICON_BIG}};

slint::include_modules!();

const APP_NAME: &str = "RAMOpt";
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_VALUE: &str = "RAMOpt";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Settings { pub auto_clean: bool, pub interval_minutes: u32, pub hotkey: String, pub clean_temp: bool, pub trim_background_apps: bool, pub start_with_windows: bool, #[serde(default = "default_close_to_tray")] pub close_to_tray: bool }
fn default_close_to_tray() -> bool { true }
impl Default for Settings { fn default() -> Self { Self { auto_clean: true, interval_minutes: 15, hotkey: "ctrl+alt+KeyR".into(), clean_temp: true, trim_background_apps: false, start_with_windows: false, close_to_tray: default_close_to_tray() } } }

fn settings_path() -> PathBuf { std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join(APP_NAME).join("settings.json") }
pub fn load_settings() -> Settings { fs::read_to_string(settings_path()).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default() }
pub fn save_settings(settings: &Settings) { let path = settings_path(); if let Some(folder) = path.parent() { let _ = fs::create_dir_all(folder); } if let Ok(json) = serde_json::to_string_pretty(settings) { let _ = fs::write(path, json); } }
fn set_startup(enabled: bool) -> Result<(), String> { let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_KEY, winreg::enums::KEY_WRITE).map_err(|e| e.to_string())?; if enabled { let exe = std::env::current_exe().map_err(|e| e.to_string())?; key.set_value(RUN_VALUE, &format!("\"{}\"", exe.display())).map_err(|e| e.to_string())?; } else { let _ = key.delete_value(RUN_VALUE); } Ok(()) }
fn clear_temp_folder(folder: PathBuf) -> u64 { fs::read_dir(folder).map(|entries| entries.flatten().filter(|entry| { let path = entry.path(); if path.is_dir() { fs::remove_dir_all(path).is_ok() } else { fs::remove_file(path).is_ok() } }).count() as u64).unwrap_or(0) }
fn clear_temp() -> u64 { clear_temp_folder(std::env::temp_dir()) + clear_temp_folder(PathBuf::from(r"C:\Windows\Temp")) }
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
fn trim_working_sets() -> f64 { let script = "$ErrorActionPreference='SilentlyContinue'; $before=(Get-Process | Measure-Object -Property WorkingSet64 -Sum).Sum; Get-Process | ForEach-Object { try { $_.MinWorkingSet=$_.MinWorkingSet } catch {} }; $after=(Get-Process | Measure-Object -Property WorkingSet64 -Sum).Sum; [math]::Max(0, ($before-$after)/1MB)"; Command::new("powershell.exe").creation_flags(CREATE_NO_WINDOW).args(["-NoProfile", "-NonInteractive", "-Command", script]).output().ok().and_then(|o| String::from_utf8(o.stdout).ok()).and_then(|text| text.trim().replace(',', ".").parse().ok()).unwrap_or(0.0) }
fn close_background_apps() -> u32 { let names = ["OneDrive.exe", "Teams.exe", "AdobeIPCBroker.exe", "AdobeCollabSync.exe", "ArmouryCrate.UserSessionHelper.exe", "GameSDK.exe", "NahimicService.exe", "AuraService.exe"]; names.iter().filter(|name| Command::new("taskkill.exe").creation_flags(CREATE_NO_WINDOW).args(["/F", "/IM", name]).output().map(|o| o.status.success()).unwrap_or(false)).count() as u32 }
pub fn clean_memory(settings: &Settings) -> String { let freed_mb = trim_working_sets(); if settings.clean_temp { clear_temp(); } if settings.trim_background_apps { close_background_apps(); } format!("Memory cleaned: {freed_mb:.1} MB") }

struct TrayMenu { show: MenuItem, clean: MenuItem, auto: CheckMenuItem, temp: CheckMenuItem, apps: CheckMenuItem, startup: CheckMenuItem, exit: MenuItem }
impl TrayMenu {
    fn new(settings: &Settings) -> (Menu, Self) { let menu = Menu::new(); let items = Self { show: MenuItem::new("Show RAMOpt", true, None), clean: MenuItem::new("Clean RAM now", true, None), auto: CheckMenuItem::new("Scheduled cleanup", true, settings.auto_clean, None), temp: CheckMenuItem::new("Clean temp files", true, settings.clean_temp, None), apps: CheckMenuItem::new("Close background apps", true, settings.trim_background_apps, None), startup: CheckMenuItem::new("Start with Windows", true, settings.start_with_windows, None), exit: MenuItem::new("Exit RAMOpt", true, None) }; for item in [&items.show, &items.clean] { menu.append(item).unwrap(); } menu.append(&items.auto).unwrap(); menu.append(&items.temp).unwrap(); menu.append(&items.apps).unwrap(); menu.append(&items.startup).unwrap(); menu.append(&items.exit).unwrap(); (menu, items) }
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
fn sync_ui(ui: &MainWindow, s: &Settings) { ui.set_auto_clean(s.auto_clean); ui.set_interval_minutes(s.interval_minutes as i32); ui.set_hotkey(s.hotkey.clone().into()); ui.set_clean_temp(s.clean_temp); ui.set_close_apps(s.trim_background_apps); ui.set_startup(s.start_with_windows); ui.set_close_to_tray(s.close_to_tray); }
fn read_ui(ui: &MainWindow) -> Settings { Settings { auto_clean: ui.get_auto_clean(), interval_minutes: ui.get_interval_minutes().clamp(1, 1440) as u32, hotkey: ui.get_hotkey().to_string(), clean_temp: ui.get_clean_temp(), trim_background_apps: ui.get_close_apps(), start_with_windows: ui.get_startup(), close_to_tray: ui.get_close_to_tray() } }
fn persist(ui: &MainWindow, state: &Arc<Mutex<Settings>>, hotkey_updates: &Sender<String>) { let settings = read_ui(ui); if let Err(error) = set_startup(settings.start_with_windows) { ui.set_status(format!("Startup setting failed: {error}").into()); return; } let previous_hotkey = state.lock().unwrap().hotkey.clone(); if settings.hotkey != previous_hotkey { let _ = hotkey_updates.send(settings.hotkey.clone()); } save_settings(&settings); *state.lock().unwrap() = settings; ui.set_status("Settings saved.".into()); }

pub fn run() -> Result<(), slint::PlatformError> {
    let state = Arc::new(Mutex::new(load_settings())); let ui = MainWindow::new()?; sync_ui(&ui, &state.lock().unwrap());
    let (menu, tray) = TrayMenu::new(&state.lock().unwrap()); let _tray = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("RAMOpt").with_icon(icon()).build().expect("failed to create tray icon");
    let hotkey_updates = spawn_hotkey(ui.as_weak(), state.clone());
    let weak = ui.as_weak(); let save_state = state.clone(); ui.on_save_settings(move || if let Some(ui) = weak.upgrade() { persist(&ui, &save_state, &hotkey_updates); });
    let weak = ui.as_weak(); let clean_state = state.clone(); ui.on_clean_now(move || { if let Some(ui) = weak.upgrade() { let settings = read_ui(&ui); *clean_state.lock().unwrap() = settings.clone(); ui.set_status("Cleaning RAM...".into()); let weak = ui.as_weak(); thread::spawn(move || { let status = clean_memory(&settings); let _ = slint::invoke_from_event_loop(move || if let Some(ui) = weak.upgrade() { ui.set_status(status.into()); }); }); } });
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
    let icon_ui = ui.as_weak();
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        if let Some(ui) = icon_ui.upgrade() {
            set_window_icon(&ui);
        }
    });
    let ids = (tray.show.id().clone(), tray.clean.id().clone(), tray.auto.id().clone(), tray.temp.id().clone(), tray.apps.id().clone(), tray.startup.id().clone(), tray.exit.id().clone());
    spawn_tray_events(ui.as_weak(), state.clone(), ids);
    spawn_timer(ui.as_weak(), state.clone());
    slint::run_event_loop_until_quit()
}
fn spawn_tray_events(ui: Weak<MainWindow>, state: Arc<Mutex<Settings>>, ids: (tray_icon::menu::MenuId, tray_icon::menu::MenuId, tray_icon::menu::MenuId, tray_icon::menu::MenuId, tray_icon::menu::MenuId, tray_icon::menu::MenuId, tray_icon::menu::MenuId)) { thread::spawn(move || loop { if let Ok(event) = MenuEvent::receiver().recv_timeout(Duration::from_millis(200)) { let mut settings = state.lock().unwrap(); if event.id == ids.0 { drop(settings); let _ = slint::invoke_from_event_loop({ let ui = ui.clone(); move || if let Some(ui) = ui.upgrade() { ui.show().unwrap(); set_window_icon(&ui); } }); } else if event.id == ids.1 { let copy = settings.clone(); drop(settings); let weak = ui.clone(); thread::spawn(move || { let status = clean_memory(&copy); let _ = slint::invoke_from_event_loop(move || if let Some(ui) = weak.upgrade() { ui.set_status(status.into()); }); }); } else { if event.id == ids.2 { settings.auto_clean = !settings.auto_clean; } if event.id == ids.3 { settings.clean_temp = !settings.clean_temp; } if event.id == ids.4 { settings.trim_background_apps = !settings.trim_background_apps; } if event.id == ids.5 { settings.start_with_windows = !settings.start_with_windows; let _ = set_startup(settings.start_with_windows); } if event.id == ids.6 { let _ = slint::quit_event_loop(); return; } save_settings(&settings); let copy = settings.clone(); drop(settings); let _ = slint::invoke_from_event_loop({ let ui = ui.clone(); move || if let Some(ui) = ui.upgrade() { sync_ui(&ui, &copy); } }); } } let _ = TrayIconEvent::receiver().try_recv(); }); }
fn spawn_timer(ui: Weak<MainWindow>, state: Arc<Mutex<Settings>>) { thread::spawn(move || loop { let minutes = { state.lock().unwrap().interval_minutes.max(1) }; thread::sleep(Duration::from_secs(u64::from(minutes) * 60)); let settings = state.lock().unwrap().clone(); if settings.auto_clean { let status = clean_memory(&settings); let _ = slint::invoke_from_event_loop({ let ui = ui.clone(); move || if let Some(ui) = ui.upgrade() { ui.set_status(status.into()); } }); } }); }
fn spawn_hotkey(ui: Weak<MainWindow>, state: Arc<Mutex<Settings>>) -> Sender<String> { let hotkey = state.lock().unwrap().hotkey.clone(); let (sender, updates): (Sender<String>, Receiver<String>) = mpsc::channel(); thread::spawn(move || { let Ok(manager) = GlobalHotKeyManager::new() else { return; }; let mut registered = hotkey.parse().ok(); if let Some(hotkey) = registered { if manager.register(hotkey).is_err() { registered = None; } } loop { if let Ok(next) = updates.recv_timeout(Duration::from_millis(100)) { if let Some(hotkey) = registered { let _ = manager.unregister(hotkey); } registered = next.parse().ok(); if let Some(hotkey) = registered { if manager.register(hotkey).is_err() { registered = None; } } } while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() { if event.state == HotKeyState::Pressed { let settings = state.lock().unwrap().clone(); let status = clean_memory(&settings); let _ = slint::invoke_from_event_loop({ let ui = ui.clone(); move || if let Some(ui) = ui.upgrade() { ui.set_status(status.into()); } }); } } } }); sender }
