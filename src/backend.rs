use std::{
    ffi::c_void,
    fs::{self, OpenOptions},
    io::Write,
    mem::size_of,
    os::windows::{ffi::OsStrExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{Mutex, OnceLock, mpsc},
    thread,
    time::{Duration, Instant, SystemTime},
};
use windows_sys::Win32::{
    Foundation::HANDLE,
    Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MoveFileExW},
    System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX},
};

const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);
const VOLUME_FLUSH_TIMEOUT: Duration = Duration::from_secs(5);
const LOG_ROTATE_BYTES: u64 = 2 * 1024 * 1024;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
static LOG_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static SETTINGS_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

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
    #[serde(default)]
    pub auto_threshold: bool,
    #[serde(default = "default_threshold_percent")]
    pub threshold_percent: u32,
}

fn default_threshold_percent() -> u32 {
    75
}

fn default_close_to_tray() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_clean: false,
            interval_minutes: 15,
            hotkey: "ctrl+alt+KeyR".into(),
            clean_temp: true,
            trim_background_apps: false,
            start_with_windows: false,
            close_to_tray: default_close_to_tray(),
            dark_mode: false,
            auto_threshold: false,
            threshold_percent: 75,
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
        if let Ok(mut settings) = serde_json::from_str::<Settings>(&text) {
            settings.interval_minutes = settings.interval_minutes.clamp(1, 1440);
            return settings;
        }
        log("Settings file invalid; trying legacy settings file.");
    }
    if let Ok(text) = fs::read_to_string(legacy_settings_path())
        && let Ok(mut settings) = serde_json::from_str::<Settings>(&text)
    {
        settings.interval_minutes = settings.interval_minutes.clamp(1, 1440);
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

pub fn save_settings(settings: &Settings) -> Result<(), String> {
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
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return None;
            }
            let age = metadata.modified().ok()?.elapsed().ok()?;
            if age < Duration::from_secs(24 * 60 * 60) {
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
const NT_SYSTEM_FILE_CACHE_INFORMATION: u32 = 21;
const NT_SYSTEM_MEMORY_LIST_INFORMATION: u32 = 80;
const NT_SYSTEM_COMBINE_PHYSICAL_MEMORY_INFORMATION: u32 = 130;
const NT_SYSTEM_REGISTRY_RECONCILIATION_INFORMATION: u32 = 155;
const MEMORY_EMPTY_WORKING_SETS: i32 = 2;
const MEMORY_FLUSH_MODIFIED_LIST: i32 = 3;
const MEMORY_PURGE_STANDBY_LIST: i32 = 4;
const SE_PRIVILEGE_ENABLED: u32 = 0x0000_0002;
const TOKEN_QUERY: u32 = 0x0000_0008;
const TOKEN_ADJUST_PRIVILEGES: u32 = 0x0000_0020;
const ERROR_NOT_ALL_ASSIGNED: u32 = 1300;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_SHARE_READ: u32 = 0x0000_0001;
const FILE_SHARE_WRITE: u32 = 0x0000_0002;
const OPEN_EXISTING: u32 = 3;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x0000_0080;
const FILE_FLAG_NO_BUFFERING: u32 = 0x2000_0000;
const DRIVE_FIXED: u32 = 3;

#[repr(C)]
struct Luid {
    low_part: u32,
    high_part: i32,
}

#[repr(C)]
struct LuidAndAttributes {
    luid: Luid,
    attributes: u32,
}

#[repr(C)]
struct TokenPrivileges {
    privilege_count: u32,
    privileges: [LuidAndAttributes; 1],
}

#[repr(C)]
#[derive(Default)]
struct SystemFileCacheInformation {
    current_size: usize,
    peak_size: usize,
    page_fault_count: u32,
    minimum_working_set: usize,
    maximum_working_set: usize,
    current_size_including_transition_in_pages: usize,
    peak_size_including_transition_in_pages: usize,
    transition_repurpose_count: u32,
    flags: u32,
}

#[repr(C)]
struct MemoryCombineInformationEx {
    handle: HANDLE,
    pages_combined: usize,
    flags: u32,
}

#[link(name = "advapi32")]
unsafe extern "system" {
    #[link_name = "AdjustTokenPrivileges"]
    fn adjust_token_privileges(
        token_handle: HANDLE,
        disable_all_privileges: i32,
        new_state: *const TokenPrivileges,
        buffer_length: u32,
        previous_state: *mut c_void,
        return_length: *mut u32,
    ) -> i32;
    #[link_name = "LookupPrivilegeValueW"]
    fn lookup_privilege_value_w(system_name: *const u16, name: *const u16, luid: *mut Luid) -> i32;
    #[link_name = "OpenProcessToken"]
    fn open_process_token(
        process_handle: HANDLE,
        desired_access: u32,
        token_handle: *mut HANDLE,
    ) -> i32;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CloseHandle(handle: HANDLE) -> i32;
    fn CreateFileW(
        file_name: *const u16,
        desired_access: u32,
        share_mode: u32,
        security_attributes: *const c_void,
        creation_disposition: u32,
        flags_and_attributes: u32,
        template_file: HANDLE,
    ) -> HANDLE;
    fn FlushFileBuffers(file: HANDLE) -> i32;
    #[link_name = "GetCurrentProcess"]
    fn get_current_process() -> HANDLE;
    fn GetDriveTypeW(root_path: *const u16) -> u32;
    fn GetLastError() -> u32;
    fn GetLogicalDrives() -> u32;
    fn SetSystemFileCacheSize(
        minimum_file_cache_size: usize,
        maximum_file_cache_size: usize,
        flags: u32,
    ) -> i32;
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtSetSystemInformation(
        information_class: u32,
        information: *mut c_void,
        information_length: u32,
    ) -> i32;
}

struct OwnedWindowsHandle(HANDLE);

impl OwnedWindowsHandle {
    fn is_valid(handle: HANDLE) -> bool {
        !handle.is_null() && handle != (-1isize as HANDLE)
    }
}

impl Drop for OwnedWindowsHandle {
    fn drop(&mut self) {
        if Self::is_valid(self.0) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

fn win32_error(operation: &str) -> String {
    let code = unsafe { GetLastError() };
    format!("{operation} failed (Win32 error {code})")
}

fn enable_privilege(name: &str) -> Result<(), String> {
    let token_name = wide(Path::new(name));
    let mut luid = Luid {
        low_part: 0,
        high_part: 0,
    };
    let mut token: HANDLE = std::ptr::null_mut();
    let opened = unsafe {
        open_process_token(
            get_current_process(),
            TOKEN_QUERY | TOKEN_ADJUST_PRIVILEGES,
            &mut token,
        )
    };
    if opened == 0 || !OwnedWindowsHandle::is_valid(token) {
        return Err(win32_error("OpenProcessToken"));
    }
    let token = OwnedWindowsHandle(token);

    if unsafe { lookup_privilege_value_w(std::ptr::null(), token_name.as_ptr(), &mut luid) } == 0 {
        return Err(win32_error(&format!("LookupPrivilegeValueW({name})")));
    }
    let state = TokenPrivileges {
        privilege_count: 1,
        privileges: [LuidAndAttributes {
            luid,
            attributes: SE_PRIVILEGE_ENABLED,
        }],
    };
    if unsafe {
        adjust_token_privileges(
            token.0,
            0,
            &state,
            size_of::<TokenPrivileges>() as u32,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(win32_error(&format!("AdjustTokenPrivileges({name})")));
    }
    let error = unsafe { GetLastError() };
    if error == ERROR_NOT_ALL_ASSIGNED {
        return Err(format!(
            "AdjustTokenPrivileges({name}) failed: privilege not assigned"
        ));
    }
    Ok(())
}

fn nt_success(operation: &str, status: i32) -> Result<(), String> {
    if status >= 0 {
        Ok(())
    } else {
        Err(format!(
            "{operation} failed (NTSTATUS 0x{:08X})",
            status as u32
        ))
    }
}

fn nt_memory_command(operation: &str, command: i32) -> Result<(), String> {
    enable_privilege("SeProfileSingleProcessPrivilege")?;
    let mut command = command;
    let status = unsafe {
        NtSetSystemInformation(
            NT_SYSTEM_MEMORY_LIST_INFORMATION,
            (&mut command as *mut i32).cast::<c_void>(),
            size_of::<i32>() as u32,
        )
    };
    nt_success(operation, status)
}

fn optimize_working_set() -> Result<(), String> {
    nt_memory_command("working-set optimization", MEMORY_EMPTY_WORKING_SETS)
}

fn optimize_standby_list() -> Result<(), String> {
    nt_memory_command("standby-list purge", MEMORY_PURGE_STANDBY_LIST)
}

fn optimize_modified_page_list() -> Result<(), String> {
    nt_memory_command("modified-page-list flush", MEMORY_FLUSH_MODIFIED_LIST)
}

fn optimize_combined_page_list() -> Result<(), String> {
    enable_privilege("SeProfileSingleProcessPrivilege")?;
    let mut information = MemoryCombineInformationEx {
        handle: std::ptr::null_mut(),
        pages_combined: 0,
        flags: 0,
    };
    let status = unsafe {
        NtSetSystemInformation(
            NT_SYSTEM_COMBINE_PHYSICAL_MEMORY_INFORMATION,
            (&mut information as *mut MemoryCombineInformationEx).cast::<c_void>(),
            size_of::<MemoryCombineInformationEx>() as u32,
        )
    };
    nt_success("combined-page-list optimization", status)
}

fn optimize_registry_cache() -> Result<(), String> {
    let status = unsafe {
        NtSetSystemInformation(
            NT_SYSTEM_REGISTRY_RECONCILIATION_INFORMATION,
            std::ptr::null_mut(),
            0,
        )
    };
    nt_success("registry-cache reconciliation", status)
}

fn optimize_system_file_cache() -> Result<(), String> {
    enable_privilege("SeIncreaseQuotaPrivilege")?;
    let flush_sentinel = if cfg!(target_pointer_width = "64") {
        usize::MAX
    } else {
        i32::MAX as usize
    };
    let mut information = SystemFileCacheInformation {
        minimum_working_set: flush_sentinel,
        maximum_working_set: flush_sentinel,
        ..Default::default()
    };
    let status = unsafe {
        NtSetSystemInformation(
            NT_SYSTEM_FILE_CACHE_INFORMATION,
            (&mut information as *mut SystemFileCacheInformation).cast::<c_void>(),
            size_of::<SystemFileCacheInformation>() as u32,
        )
    };
    let mut errors = Vec::new();
    if let Err(error) = nt_success("system-file-cache trim", status) {
        errors.push(error);
    }
    let flushed = unsafe { SetSystemFileCacheSize(flush_sentinel, flush_sentinel, 0) };
    if flushed == 0 {
        errors.push(win32_error("SetSystemFileCacheSize"));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

fn flush_volume_cache(letter: u8) -> Result<(), String> {
    let volume = format!(r"\\.\{}:", letter as char);
    let root = format!("{}:\\", letter as char);
    let root_name = wide(Path::new(&root));
    if unsafe { GetDriveTypeW(root_name.as_ptr()) } != DRIVE_FIXED {
        return Ok(());
    }
    let volume_name = wide(Path::new(&volume));
    let handle = unsafe {
        CreateFileW(
            volume_name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_NO_BUFFERING,
            std::ptr::null_mut(),
        )
    };
    if !OwnedWindowsHandle::is_valid(handle) {
        return Err(win32_error(&format!("CreateFileW({volume})")));
    }

    let handle_value = handle as usize;
    std::mem::forget(OwnedWindowsHandle(handle));
    let (sender, receiver) = mpsc::channel();
    let operation = volume.clone();
    thread::spawn(move || {
        let result = if unsafe { FlushFileBuffers(handle_value as HANDLE) } == 0 {
            Err(win32_error(&format!("FlushFileBuffers({operation})")))
        } else {
            Ok(())
        };
        unsafe {
            let _ = CloseHandle(handle_value as HANDLE);
        }
        let _ = sender.send(result);
    });

    match receiver.recv_timeout(VOLUME_FLUSH_TIMEOUT) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(format!(
            "FlushFileBuffers({volume}) timed out after {} seconds",
            VOLUME_FLUSH_TIMEOUT.as_secs()
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err(format!("FlushFileBuffers({volume}) worker disconnected"))
        }
    }
}

fn optimize_modified_file_cache() -> Result<(), String> {
    let mask = unsafe { GetLogicalDrives() };
    if mask == 0 {
        return Err(win32_error("GetLogicalDrives"));
    }
    let mut errors = Vec::new();
    for index in 0..26u8 {
        if mask & (1u32 << index) == 0 {
            continue;
        }
        if let Err(error) = flush_volume_cache(b'A' + index) {
            errors.push(error);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

#[derive(Debug, Default)]
struct MemoryOptimizationReport {
    freed_mb: f64,
    measurement_available: bool,
    attempted_areas: u32,
    successful_areas: u32,
    failed_areas: Vec<String>,
}

fn run_memory_area<F>(report: &mut MemoryOptimizationReport, name: &str, optimize: F)
where
    F: FnOnce() -> Result<(), String>,
{
    report.attempted_areas += 1;
    let started = Instant::now();
    match optimize() {
        Ok(()) => {
            report.successful_areas += 1;
            log(format!(
                "Memory area {name}: completed in {:.0} ms",
                started.elapsed().as_secs_f64() * 1000.0
            ));
        }
        Err(error) => {
            report.failed_areas.push(format!("{name}: {error}"));
            log(format!(
                "Memory area {name}: skipped after {:.0} ms: {error}",
                started.elapsed().as_secs_f64() * 1000.0
            ));
        }
    }
}

fn optimize_memory() -> MemoryOptimizationReport {
    let before = memory_status();
    let mut report = MemoryOptimizationReport::default();
    if before.is_none() {
        log("Memory snapshot before optimization unavailable.");
    }

    // Phase 1: working-set optimization and physical-memory measurement.
    run_memory_area(&mut report, "working set", optimize_working_set);

    // Phase 2: cache and modified-page operations used by WinMemoryCleaner defaults.
    run_memory_area(&mut report, "system file cache", optimize_system_file_cache);
    run_memory_area(
        &mut report,
        "modified page list",
        optimize_modified_page_list,
    );
    run_memory_area(&mut report, "standby list", optimize_standby_list);

    // Phase 3: page combining, registry, and volume cache.
    run_memory_area(
        &mut report,
        "combined page list",
        optimize_combined_page_list,
    );
    run_memory_area(&mut report, "registry cache", optimize_registry_cache);
    run_memory_area(
        &mut report,
        "modified file cache",
        optimize_modified_file_cache,
    );

    if let (Some(before), Some(after)) = (before, memory_status()) {
        report.measurement_available = true;
        report.freed_mb = (before.0 as f64 - after.0 as f64) / (1024.0 * 1024.0);
        log(format!(
            "Memory snapshot: used {} -> {} MB; available {} -> {} MB",
            before.0 / (1024 * 1024),
            after.0 / (1024 * 1024),
            before.1.saturating_sub(before.0) / (1024 * 1024),
            after.1.saturating_sub(after.0) / (1024 * 1024)
        ));
    } else {
        log("Memory snapshot before or after optimization unavailable.");
    }
    report
}

/// Gracefully closes selected user apps and removes narrowly identified orphan helpers.
/// Service-owned processes, protected processes, and active application trees are skipped.
fn close_background_apps() -> Result<(u32, u32), String> {
    let script = r#"
$ErrorActionPreference = 'Stop'
$current = [Security.Principal.WindowsIdentity]::GetCurrent().Name
$currentSessionId = (Get-Process -Id $PID -ErrorAction SilentlyContinue).SessionId
if ($null -eq $currentSessionId) {
    Write-Output 'RAMOPT_COUNTS:0:0'
    exit 0
}
$currentSessionId = [int]$currentSessionId
$processes = @(Get-CimInstance -ClassName Win32_Process -ErrorAction Stop)
if ($processes.Count -eq 0) {
    Write-Output 'RAMOPT_COUNTS:0:0'
    exit 0
}
$byId = @{}
$byName = @{}
foreach ($process in $processes) {
    [void]($byId[[int]$process.ProcessId] = $process)
    [void]($byName[([string]$process.Name).ToLowerInvariant()] = $true)
}

$owners = @{}
$sessionIds = @{}
Get-Process -IncludeUserName | ForEach-Object {
    try {
        if ($_.UserName) {
            $owners[[int]$_.Id] = $_.UserName
            $sessionIds[[int]$_.Id] = [int]$_.SessionId
        }
    } catch {}
}

$servicePids = @{}
$services = @(Get-CimInstance -ClassName Win32_Service -ErrorAction Stop)
foreach ($service in $services | Where-Object { $_.State -eq 'Running' -and $_.ProcessId -ne 0 }) {
    [void]($servicePids[[int]$service.ProcessId] = $true)
}

$visibleRoots = @{}
Get-Process |
    Where-Object { $_.MainWindowHandle -ne 0 } |
    ForEach-Object { $visibleRoots[$_.ProcessName.ToLowerInvariant()] = $true }

function Normalize-Path {
    param([string]$Value)
    if ([string]::IsNullOrWhiteSpace($Value)) {
        return ''
    }
    try {
        return ([System.IO.Path]::GetFullPath($Value)).TrimEnd('\').ToLowerInvariant()
    } catch {
        return $Value.Trim().Trim('"').TrimEnd('\').ToLowerInvariant()
    }
}

function Get-CreationTime {
    param($Process)
    try {
        if ($Process.CreationDate -is [datetime]) {
            return [datetime]$Process.CreationDate
        }
        return [System.Management.ManagementDateTimeConverter]::ToDateTime([string]$Process.CreationDate)
    } catch {
        return $null
    }
}

function Is-CurrentUserProcess {
    param([int]$candidateId)
    return $owners.ContainsKey($candidateId) -and
        $owners[$candidateId] -eq $current -and
        $sessionIds.ContainsKey($candidateId) -and
        $sessionIds[$candidateId] -eq $currentSessionId
}

function Is-StaleOrphan {
    param($Process)
    $candidateId = [int]$Process.ProcessId
    if (-not (Is-CurrentUserProcess $candidateId)) {
        return $false
    }
    if ($servicePids.ContainsKey($candidateId)) {
        return $false
    }
    $parentPid = [int]$Process.ParentProcessId
    if ($parentPid -ne 0 -and $byId.ContainsKey($parentPid)) {
        return $false
    }
    $created = Get-CreationTime $Process
    if ($null -eq $created -or ((Get-Date) - $created).TotalSeconds -lt 120) {
        return $false
    }
    if (@($byId.Values | Where-Object { [int]$_.ParentProcessId -eq $candidateId }).Count -gt 0) {
        return $false
    }
    if (Has-NetworkActivity $candidateId) {
        return $false
    }
    return $true
}

function Has-NetworkActivity {
    param([int]$candidateId)
    try {
        return @(
            Get-NetTCPConnection -OwningProcess $candidateId -ErrorAction Stop
        ).Count -gt 0
    } catch {
        return $true
    }
}

function Has-VisibleRoot {
    param([string]$Name)
    return $visibleRoots.ContainsKey($Name)
}

function Has-ActiveRoot {
    param([string]$Name, [string]$Path, [int]$excludeId)
    return @($byId.Values | Where-Object {
        [int]$_.ProcessId -ne $excludeId -and
        ([string]$_.Name).ToLowerInvariant() -eq $Name.ToLowerInvariant() -and
        (Normalize-Path ([string]$_.ExecutablePath)) -eq $Path -and
        [string]$_.CommandLine -notmatch '--type='
    }).Count -gt 0
}

function Same-ProcessIdentity {
    param($Expected, $Actual)
    return [string]$Actual.Name -eq [string]$Expected.Name -and
        (Normalize-Path ([string]$Actual.ExecutablePath)) -eq (Normalize-Path ([string]$Expected.ExecutablePath)) -and
        [string]$Actual.CommandLine -eq [string]$Expected.CommandLine -and
        [int]$Actual.ParentProcessId -eq [int]$Expected.ParentProcessId -and
        $Actual.CreationDate -eq $Expected.CreationDate -and
        (Get-CreationTime $Actual) -eq (Get-CreationTime $Expected)
}

function Is-TrustedOneDrivePath {
    param([string]$Path)
    $roots = @(
        (Normalize-Path $env:ProgramFiles),
        (Normalize-Path ${env:ProgramFiles(x86)}),
        (Normalize-Path $env:LOCALAPPDATA)
    ) | Where-Object { $_ }
    foreach ($root in $roots) {
        if ($Path.StartsWith("$root\\microsoft onedrive\\")) {
            return $true
        }
    }
    return $false
}

function Is-OrphanCandidate {
    param($Process)
    $name = ([string]$Process.Name).ToLowerInvariant()
    $path = Normalize-Path ([string]$Process.ExecutablePath)
    $commandLine = [string]$Process.CommandLine
    $mcpOrDevWorkload = '(findskills-mcp|chrome-devtools-mcp|agentmemory-mcp|mcp-server-mobile|notebooklm-mcp|codegraph|vite|nuxt|next|esbuild)'

    switch ($name) {
        'onedrive.sync.service.exe' {
            return (Is-TrustedOneDrivePath $path) -and
                $path -match '\\microsoft onedrive\\[^\\]+\\onedrive\.sync\.service\.exe$' -and
                -not $byName.ContainsKey('onedrive.exe') -and
                -not $byName.ContainsKey('onedrivesetup.exe') -and
                -not $byName.ContainsKey('onedrivestandaloneupdater.exe')
        }
        'node.exe' {
            $knownNodePath = $path -match '^[a-z]:\\nvm4w\\nodejs\\node\.exe$' -or
                $path -match '\\appdata\\local\\nvm\\[^\\]+\\node_modules\\@colbymchenry\\codegraph\\node_modules\\@colbymchenry\\codegraph-win32-x64\\node\.exe$'
            return $knownNodePath -and $commandLine -match '(findskills-mcp|chrome-devtools-mcp|agentmemory-mcp|mcp-server-mobile|notebooklm-mcp|codegraph|vite|nuxt|next|esbuild)' -and
                -not (Has-ActiveRoot 'node.exe' $path ([int]$Process.ProcessId))
        }
        'cmd.exe' {
            return $path -match '^[a-z]:\\windows\\system32\\cmd\.exe$' -and
                $commandLine -match '(/c|/d\s+/s\s+/c)\s+.*(mcp|codegraph|vite|nuxt|next|esbuild)'
        }
        'esbuild.exe' {
            return $path -match '\\node_modules\\@esbuild\\win32-x64\\esbuild\.exe$' -and
                $commandLine -match '--service'
        }
        'workspace-mcp.exe' {
            return $path -match '\\appdata\\local\\uv\\cache\\archive-v0\\[^\\]+\\scripts\\workspace-mcp\.exe$' -and
                $commandLine -match 'workspace-mcp|--tools\s+docs\s+drive'
        }
        'uv.exe' {
            return $path -match '\\appdata\\local\\hermes\\bin\\uv\.exe$' -and
                $commandLine -match 'workspace-mcp|mcp|notebooklm|findskills|chrome-devtools|agentmemory|mobile'
        }
        'uvx.exe' {
            return $path -match '\\appdata\\local\\hermes\\bin\\uvx\.exe$' -and
                $commandLine -match 'workspace-mcp|mcp|notebooklm|findskills|chrome-devtools|agentmemory|mobile'
        }
        'python.exe' {
            $knownPythonPath = $path -match '\\appdata\\local\\uv\\cache\\archive-v0\\[^\\]+\\scripts\\python\.exe$' -or
                $path -match '\\appdata\\local\\programs\\python\\python313\\python\.exe$'
            return $knownPythonPath -and $commandLine -match 'workspace-mcp'
        }
        'msedge.exe' {
            return $path -match '\\microsoft edge\\application\\msedge\.exe$' -and
                $commandLine -match '--type=' -and -not (Has-VisibleRoot 'msedge')
        }
        'chrome.exe' {
            return $path -match '\\google\\chrome\\application\\chrome\.exe$' -and
                $commandLine -match '--type=' -and -not (Has-VisibleRoot 'chrome')
        }
        'code.exe' {
            return $path -match '\\microsoft vs code\\code\.exe$' -and
                $commandLine -match '--type=' -and -not (Has-VisibleRoot 'code')
        }
        'slack.exe' {
            return $false
        }
        'steam.exe' {
            return $false
        }
        'steamwebhelper.exe' {
            return $false
        }
        'sideloadly.exe' {
            return $false
        }
        default {
            return $false
        }
    }
}

$gracefulNames = @(
    'OneDrive.exe',
    'OneDrive.Sync.Service.exe',
    'Teams.exe',
    'AdobeIPCBroker.exe',
    'AdobeCollabSync.exe',
    'Slack.exe',
    'steam.exe',
    'sideloadly.exe'
)
$closed = 0
foreach ($process in $processes | Where-Object { $_.Name -in $gracefulNames }) {
    $candidateId = [int]$process.ProcessId
    if (-not (Is-CurrentUserProcess $candidateId) -or $servicePids.ContainsKey($candidateId)) {
        continue
    }
    $live = Get-CimInstance -ClassName Win32_Process -Filter "ProcessId = $candidateId"
    if ($null -eq $live -or -not (Same-ProcessIdentity $process $live)) {
        continue
    }
    try {
        $processHandle = Get-Process -Id $candidateId -ErrorAction Stop
        if ($processHandle.SessionId -ne $currentSessionId -or $processHandle.MainWindowHandle -eq 0) {
            continue
        }
        if ($processHandle.CloseMainWindow()) {
            [void]($closed++)
        }
    } catch {}
}

    $orphanCandidates = @($processes | Where-Object { (Is-StaleOrphan $_) -and (Is-OrphanCandidate $_) })
$liveProcesses = @(Get-CimInstance -ClassName Win32_Process -ErrorAction Stop)
$liveById = @{}
$liveByName = @{}
foreach ($process in $liveProcesses) {
    $liveById[[int]$process.ProcessId] = $process
    $liveByName[([string]$process.Name).ToLowerInvariant()] = $true
}
$liveServicePids = @{}
$liveServices = @(Get-CimInstance -ClassName Win32_Service -ErrorAction Stop)
foreach ($service in $liveServices | Where-Object { $_.State -eq 'Running' -and $_.ProcessId -ne 0 }) {
    [void]($liveServicePids[[int]$service.ProcessId] = $true)
}
$byId = $liveById
$byName = $liveByName
$servicePids = $liveServicePids
$terminated = 0
foreach ($candidate in $orphanCandidates) {
    $candidateId = [int]$candidate.ProcessId
    if (-not $liveById.ContainsKey($candidateId)) {
        continue
    }
    $live = $liveById[$candidateId]
    if ([string]$live.Name -ne [string]$candidate.Name -or
        (Normalize-Path ([string]$live.ExecutablePath)) -ne (Normalize-Path ([string]$candidate.ExecutablePath)) -or
        [string]$live.CommandLine -ne [string]$candidate.CommandLine -or
        [int]$live.ParentProcessId -ne [int]$candidate.ParentProcessId -or
        [string]$live.CreationDate -ne [string]$candidate.CreationDate) {
        continue
    }
    if (-not (Is-StaleOrphan $live) -or -not (Is-OrphanCandidate $live)) {
        continue
    }
    try {
        $processHandle = Get-Process -Id $candidateId -IncludeUserName -ErrorAction Stop
        if ($processHandle.UserName -ne $current -or $processHandle.SessionId -ne $currentSessionId) {
            continue
        }
        Stop-Process -InputObject $processHandle -Force -ErrorAction Stop
        [void]($terminated++)
    } catch {}
}

Write-Output "RAMOPT_COUNTS:$closed`:$terminated"
"#;
    let mut command = Command::new(system_binary(r"WindowsPowerShell\v1.0\powershell.exe"));
    command.creation_flags(CREATE_NO_WINDOW).args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        script,
    ]);
    let output = command_output_with_timeout(command, COMMAND_TIMEOUT)
        .map_err(|error| format!("background-app cleanup failed: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("background-app cleanup exited with {}", output.status)
        } else {
            format!(
                "background-app cleanup exited with {}: {stderr}",
                output.status
            )
        });
    }
    let output = String::from_utf8(output.stdout)
        .map_err(|error| format!("background-app cleanup output was not UTF-8: {error}"))?;
    let line = output
        .lines()
        .find(|line| line.starts_with("RAMOPT_COUNTS:"))
        .ok_or_else(|| "background-app cleanup returned no result".to_string())?;
    let mut values = line
        .trim_start_matches("RAMOPT_COUNTS:")
        .split(':')
        .map(str::parse::<u32>);
    Ok((
        values
            .next()
            .ok_or_else(|| "background-app cleanup missing closed count".to_string())?
            .map_err(|error| format!("invalid closed count: {error}"))?,
        values
            .next()
            .ok_or_else(|| "background-app cleanup missing orphan count".to_string())?
            .map_err(|error| format!("invalid orphan count: {error}"))?,
    ))
}

pub fn clean_memory(settings: &Settings) -> String {
    let report = optimize_memory();
    if settings.clean_temp {
        clear_temp();
    }
    let (closed, terminated, app_cleanup_error) = if settings.trim_background_apps {
        match close_background_apps() {
            Ok((closed, terminated)) => (closed, terminated, None),
            Err(error) => (0, 0, Some(error)),
        }
    } else {
        (0, 0, None)
    };
    let measurement = if report.measurement_available {
        format!("{:.1} MB estimated physical delta", report.freed_mb)
    } else {
        "measurement unavailable".to_string()
    };
    let mut status = format!(
        "Memory cleaned: {measurement}; areas {}/{}; closed {closed} user apps",
        report.successful_areas, report.attempted_areas
    );
    if terminated > 0 {
        status.push_str(&format!("; cleaned {terminated} orphan processes"));
    }
    if !report.failed_areas.is_empty() {
        status.push_str(&format!("; skipped {} areas", report.failed_areas.len()));
    }
    if let Some(error) = app_cleanup_error {
        log(&error);
        status.push_str("; app cleanup failed");
    }
    status
}

pub fn memory_status() -> Option<(u64, u64)> {
    unsafe {
        let mut status: MEMORYSTATUSEX = std::mem::zeroed();
        status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if GlobalMemoryStatusEx(&mut status) == 0 {
            log(format!("GlobalMemoryStatusEx failed: {}", GetLastError()));
            return None;
        }
        Some((
            status.ullTotalPhys.saturating_sub(status.ullAvailPhys),
            status.ullTotalPhys,
        ))
    }
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
        assert!(!settings.auto_threshold);
        assert_eq!(settings.threshold_percent, 75);
    }

    #[test]
    fn settings_default_is_safe() {
        let settings = Settings::default();
        assert_eq!(settings.interval_minutes, 15);
        assert!(!settings.auto_clean);
        assert!(!settings.trim_background_apps);
        assert!(!settings.start_with_windows);
        assert!(!settings.auto_threshold);
        assert!(settings.close_to_tray);
    }

    #[test]
    fn native_memory_structs_match_windows_layout() {
        if cfg!(target_pointer_width = "64") {
            assert_eq!(size_of::<SystemFileCacheInformation>(), 64);
            assert_eq!(size_of::<MemoryCombineInformationEx>(), 24);
        } else {
            assert_eq!(size_of::<SystemFileCacheInformation>(), 36);
            assert_eq!(size_of::<MemoryCombineInformationEx>(), 12);
        }
    }

    #[test]
    fn nt_success_accepts_success_and_information_statuses() {
        assert!(nt_success("test", 0).is_ok());
        assert!(nt_success("test", 1).is_ok());
        assert!(nt_success("test", i32::MIN).is_err());
    }
}
