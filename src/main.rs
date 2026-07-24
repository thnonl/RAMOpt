#![cfg_attr(windows, windows_subsystem = "windows")]

mod app;

fn main() {
    app::run().expect("failed to initialize RAMOpt UI");
}

/* Obsolete native-windows-gui implementation retained only until next source cleanup.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Settings {
    auto_clean: bool,
    interval_minutes: u32,
    hotkey: String,
    clean_temp: bool,
    trim_background_apps: bool,
    start_with_windows: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { auto_clean: true, interval_minutes: 15, hotkey: "ctrl+alt+KeyR".into(), clean_temp: true, trim_background_apps: false, start_with_windows: false }
    }
}

fn settings_path() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join(APP_NAME).join("settings.json")
}
fn load_settings() -> Settings { fs::read_to_string(settings_path()).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default() }
fn save_settings(settings: &Settings) {
    let path = settings_path();
    if let Some(folder) = path.parent() { let _ = fs::create_dir_all(folder); }
    if let Ok(json) = serde_json::to_string_pretty(settings) { let _ = fs::write(path, json); }
}
fn set_startup(enabled: bool) -> Result<(), String> {
    let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_KEY, winreg::enums::KEY_WRITE).map_err(|e| e.to_string())?;
    if enabled {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        key.set_value(RUN_VALUE, &format!("\"{}\"", exe.display())).map_err(|e| e.to_string())?;
    } else { let _ = key.delete_value(RUN_VALUE); }
    Ok(())
}
fn clear_temp_folder(folder: PathBuf) -> u64 {
    let mut removed = 0;
    if let Ok(entries) = fs::read_dir(folder) { for entry in entries.flatten() { let path = entry.path(); let result = if path.is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) }; if result.is_ok() { removed += 1; } } }
    removed
}
fn clear_temp() -> u64 {
    clear_temp_folder(std::env::temp_dir()) + clear_temp_folder(PathBuf::from(r"C:\\Windows\\Temp"))
}
fn trim_working_sets() -> u32 {
    let script = "$ErrorActionPreference='SilentlyContinue'; $n=0; Get-Process | ForEach-Object { try { $_.MinWorkingSet=$_.MinWorkingSet; $n++ } catch {} }; $n";
    Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", script]).output().ok().and_then(|o| String::from_utf8(o.stdout).ok()).and_then(|text| text.trim().parse().ok()).unwrap_or(0)
}
fn close_background_apps() -> u32 {
    let names = ["OneDrive.exe", "Teams.exe", "AdobeIPCBroker.exe", "AdobeCollabSync.exe", "ArmouryCrate.UserSessionHelper.exe", "GameSDK.exe", "NahimicService.exe", "AuraService.exe"];
    names.iter().filter(|name| Command::new("taskkill.exe").args(["/F", "/IM", name]).output().map(|o| o.status.success()).unwrap_or(false)).count() as u32
}
fn clean_memory(settings: &Settings) -> String {
    let trimmed = trim_working_sets();
    let temp = if settings.clean_temp { clear_temp() } else { 0 };
    let closed = if settings.trim_background_apps { close_background_apps() } else { 0 };
    format!("Cleanup complete: trimmed {trimmed} process working sets; removed {temp} temp items; closed {closed} selected background apps.")
}

#[derive(Default, NwgUi)]
pub struct App {
    #[nwg_control(size: (520, 365), position: (300, 300), title: "RAMOpt", flags: "WINDOW|VISIBLE")]
    #[nwg_events(OnWindowClose: [App::hide_window])]
    window: nwg::Window,
    #[nwg_control(text: "RAMOpt", size: (150, 32), position: (22, 18))] title: nwg::Label,
    #[nwg_control(text: "Native RAM maintenance. Closing this window keeps RAMOpt in tray.", size: (455, 28), position: (22, 50))] subtitle: nwg::Label,
    #[nwg_control(text: "Enable scheduled cleanup", size: (220, 25), position: (22, 98), check_state: nwg::CheckBoxState::Checked)] #[nwg_events(OnButtonClick: [App::save_from_ui])] auto_clean: nwg::CheckBox,
    #[nwg_control(text: "Interval (minutes)", size: (120, 25), position: (22, 132))] interval_label: nwg::Label,
    #[nwg_control(size: (80, 26), position: (155, 128), limit: 4)] interval: nwg::TextInput,
    #[nwg_control(text: "Global hotkey", size: (100, 25), position: (260, 132))] hotkey_label: nwg::Label,
    #[nwg_control(size: (155, 26), position: (350, 128), limit: 32)] hotkey: nwg::TextInput,
    #[nwg_control(text: "Clean user temp files", size: (200, 25), position: (22, 175), check_state: nwg::CheckBoxState::Checked)] #[nwg_events(OnButtonClick: [App::save_from_ui])] clean_temp: nwg::CheckBox,
    #[nwg_control(text: "Close selected background apps", size: (245, 25), position: (260, 175))] #[nwg_events(OnButtonClick: [App::save_from_ui])] close_apps: nwg::CheckBox,
    #[nwg_control(text: "Start with Windows", size: (190, 25), position: (22, 209))] #[nwg_events(OnButtonClick: [App::save_from_ui])] startup: nwg::CheckBox,
    #[nwg_control(text: "Save settings", size: (130, 36), position: (22, 255))] #[nwg_events(OnButtonClick: [App::save_from_ui])] save_button: nwg::Button,
    #[nwg_control(text: "Clean RAM now", size: (150, 36), position: (170, 255))] #[nwg_events(OnButtonClick: [App::clean_now])] clean_button: nwg::Button,
    #[nwg_control(text: "Ready.", size: (470, 42), position: (22, 307))] status: nwg::Label,
    #[nwg_control] message_window: nwg::MessageWindow,
    #[nwg_resource(source_file: Some("assets/ramopt.ico"))] icon: nwg::Icon,
    #[nwg_control(parent: message_window, icon: Some(&data.icon), tip: Some("RAMOpt"))] tray: nwg::TrayNotification,
    #[nwg_control(popup: true, parent: message_window)] tray_menu: nwg::Menu,
    #[nwg_control(text: "Show RAMOpt", parent: tray_menu)] tray_show: nwg::MenuItem,
    #[nwg_control(text: "Clean RAM now", parent: tray_menu)] tray_clean: nwg::MenuItem,
    #[nwg_control(text: "Toggle scheduled cleanup", parent: tray_menu)] tray_auto: nwg::MenuItem,
    #[nwg_control(text: "Toggle temp cleanup", parent: tray_menu)] tray_temp: nwg::MenuItem,
    #[nwg_control(text: "Toggle background-app cleanup", parent: tray_menu)] tray_apps: nwg::MenuItem,
    #[nwg_control(text: "Toggle startup", parent: tray_menu)] tray_startup: nwg::MenuItem,
    #[nwg_control(text: "Exit RAMOpt", parent: tray_menu)] tray_exit: nwg::MenuItem,
    #[nwg_control(parent: window)] hotkey_notice: nwg::Notice,
    #[nwg_control(parent: window, interval: Duration::from_secs(60), active: true)] #[nwg_events(OnTimerTick: [App::scheduled_clean])] timer: nwg::AnimationTimer,
    settings: RefCell<Settings>,
    hotkey_manager: RefCell<Option<GlobalHotKeyManager>>,
}

impl App {
    fn checked(box_: &nwg::CheckBox) -> bool { box_.check_state() == nwg::CheckBoxState::Checked }
    fn read_ui(&self) -> Settings {
        let mut settings = self.settings.borrow().clone();
        settings.auto_clean = Self::checked(&self.auto_clean); settings.clean_temp = Self::checked(&self.clean_temp); settings.trim_background_apps = Self::checked(&self.close_apps); settings.start_with_windows = Self::checked(&self.startup);
        settings.interval_minutes = self.interval.text().trim().parse().unwrap_or(15).clamp(1, 1440); settings.hotkey = self.hotkey.text().trim().to_string(); settings
    }
    fn apply_timer(&self, settings: &Settings) { self.timer.set_interval(Duration::from_secs(u64::from(settings.interval_minutes) * 60)); if settings.auto_clean { self.timer.start(); } else { self.timer.stop(); } }
    fn update_hotkey(&self, old_hotkey: &str, new_hotkey: &str) -> Result<(), String> {
        if old_hotkey.eq_ignore_ascii_case(new_hotkey) { return Ok(()); }
        let new_hotkey = new_hotkey.parse().map_err(|_| "Use format ctrl+alt+KeyR, shift+KeyF, or alt+KeyM.".to_string())?;
        let mut manager_slot = self.hotkey_manager.borrow_mut();
        if let Some(manager) = manager_slot.as_ref() {
            if let Ok(old_hotkey) = old_hotkey.parse() { let _ = manager.unregister(old_hotkey); }
            manager.register(new_hotkey).map_err(|e| e.to_string())?;
            Ok(())
        } else {
            let new_manager = GlobalHotKeyManager::new().map_err(|e| e.to_string())?;
            new_manager.register(new_hotkey).map_err(|e| e.to_string())?;
            *manager_slot = Some(new_manager);
            Ok(())
        }
    }
    fn save_from_ui(&self) {
        let settings = self.read_ui();
        if let Err(error) = self.update_hotkey(&self.settings.borrow().hotkey, &settings.hotkey) { self.status.set_text(&format!("Hotkey failed: {error}")); return; }
        if let Err(error) = set_startup(settings.start_with_windows) { self.status.set_text(&format!("Startup setting failed: {error}")); return; }
        self.apply_timer(&settings); save_settings(&settings); *self.settings.borrow_mut() = settings; self.status.set_text("Settings saved.");
    }
    fn clean_now(&self) { self.save_from_ui(); let result = clean_memory(&self.settings.borrow()); self.status.set_text(&result); }
    fn scheduled_clean(&self) { if self.settings.borrow().auto_clean { let result = clean_memory(&self.settings.borrow()); self.status.set_text(&result); } }
    fn hide_window(&self) { self.window.set_visible(false); }
    fn show_window(&self) { self.window.set_visible(true); self.window.set_focus(); }
    fn show_tray_menu(&self) { let (x, y) = nwg::GlobalCursor::position(); self.tray_menu.popup(x, y); }
    fn toggle_auto(&self) { let next = !Self::checked(&self.auto_clean); self.auto_clean.set_check_state(if next { nwg::CheckBoxState::Checked } else { nwg::CheckBoxState::Unchecked }); self.save_from_ui(); }
    fn toggle_temp(&self) { let next = !Self::checked(&self.clean_temp); self.clean_temp.set_check_state(if next { nwg::CheckBoxState::Checked } else { nwg::CheckBoxState::Unchecked }); self.save_from_ui(); }
    fn toggle_apps(&self) { let next = !Self::checked(&self.close_apps); self.close_apps.set_check_state(if next { nwg::CheckBoxState::Checked } else { nwg::CheckBoxState::Unchecked }); self.save_from_ui(); }
    fn toggle_startup(&self) { let next = !Self::checked(&self.startup); self.startup.set_check_state(if next { nwg::CheckBoxState::Checked } else { nwg::CheckBoxState::Unchecked }); self.save_from_ui(); }
    fn exit(&self) { nwg::stop_thread_dispatch(); }
    fn on_hotkey(&self) { self.clean_now(); }
}

fn main() {
    nwg::init().expect("failed to initialize native Windows GUI"); nwg::Font::set_global_family("Segoe UI").expect("failed to set UI font");
    let settings = load_settings(); let app = App { settings: RefCell::new(settings.clone()), ..Default::default() }; let ui = App::build_ui(app).expect("failed to create RAMOpt window");
    ui.auto_clean.set_check_state(if settings.auto_clean { nwg::CheckBoxState::Checked } else { nwg::CheckBoxState::Unchecked }); ui.clean_temp.set_check_state(if settings.clean_temp { nwg::CheckBoxState::Checked } else { nwg::CheckBoxState::Unchecked }); ui.close_apps.set_check_state(if settings.trim_background_apps { nwg::CheckBoxState::Checked } else { nwg::CheckBoxState::Unchecked }); ui.startup.set_check_state(if settings.start_with_windows { nwg::CheckBoxState::Checked } else { nwg::CheckBoxState::Unchecked }); ui.interval.set_text(&settings.interval_minutes.to_string()); ui.hotkey.set_text(&settings.hotkey); ui.apply_timer(&settings);
    if let Ok(hotkey) = settings.hotkey.parse() { if let Ok(manager) = GlobalHotKeyManager::new() { if manager.register(hotkey).is_ok() { *ui.hotkey_manager.borrow_mut() = Some(manager); let sender = ui.hotkey_notice.sender(); std::thread::spawn(move || loop { if let Ok(event) = GlobalHotKeyEvent::receiver().recv() { if event.state == HotKeyState::Pressed { sender.notice(); } } }); } } }
    let ui_ref = ui;
    let event_parent = ui_ref.message_window.handle.clone();
    let events = move |event, _, handle| match event { nwg::Event::OnNotice if &handle == &ui_ref.hotkey_notice => ui_ref.on_hotkey(), nwg::Event::OnContextMenu if &handle == &ui_ref.tray => ui_ref.show_tray_menu(), nwg::Event::OnMenuItemSelected if &handle == &ui_ref.tray_show => ui_ref.show_window(), nwg::Event::OnMenuItemSelected if &handle == &ui_ref.tray_clean => ui_ref.clean_now(), nwg::Event::OnMenuItemSelected if &handle == &ui_ref.tray_auto => ui_ref.toggle_auto(), nwg::Event::OnMenuItemSelected if &handle == &ui_ref.tray_temp => ui_ref.toggle_temp(), nwg::Event::OnMenuItemSelected if &handle == &ui_ref.tray_apps => ui_ref.toggle_apps(), nwg::Event::OnMenuItemSelected if &handle == &ui_ref.tray_startup => ui_ref.toggle_startup(), nwg::Event::OnMenuItemSelected if &handle == &ui_ref.tray_exit => ui_ref.exit(), _ => {} };
    let _handler = nwg::full_bind_event_handler(&event_parent, events); nwg::dispatch_thread_events();
}
*/
