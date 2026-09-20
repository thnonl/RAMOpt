#![cfg_attr(windows, windows_subsystem = "windows")]

use std::{process, ptr};
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError},
    System::Threading::{CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, OpenEventW, SetEvent},
};

mod app;

const INSTANCE_MUTEX_NAME: &[u16] = &[
    'R' as u16, 'A' as u16, 'M' as u16, 'O' as u16, 'p' as u16, 't' as u16, '.' as u16, 'S' as u16,
    'i' as u16, 'n' as u16, 'g' as u16, 'l' as u16, 'e' as u16, 'I' as u16, 'n' as u16, 's' as u16,
    't' as u16, 'a' as u16, 'n' as u16, 'c' as u16, 'e' as u16, 0,
];
const SHOW_EVENT_NAME: &[u16] = &[
    'R' as u16, 'A' as u16, 'M' as u16, 'O' as u16, 'p' as u16, 't' as u16, '.' as u16, 'S' as u16,
    'h' as u16, 'o' as u16, 'w' as u16, 'E' as u16, 'v' as u16, 'e' as u16, 'n' as u16, 't' as u16,
    0,
];

fn main() {
    app::install_panic_hook();
    let mutex = unsafe { CreateMutexW(ptr::null(), 0, INSTANCE_MUTEX_NAME.as_ptr()) };
    if mutex.is_null() {
        app::log("failed to create RAMOpt instance mutex");
        process::exit(1);
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        let event = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, SHOW_EVENT_NAME.as_ptr()) };
        if !event.is_null() {
            unsafe {
                SetEvent(event);
                CloseHandle(event);
            }
        }
        unsafe {
            CloseHandle(mutex);
        }
        return;
    }

    let show_event = unsafe { CreateEventW(ptr::null(), 0, 0, SHOW_EVENT_NAME.as_ptr()) };
    if show_event.is_null() {
        app::log("failed to create RAMOpt show event");
        unsafe {
            CloseHandle(mutex);
        }
        process::exit(1);
    }
    let exit_code = match app::run(show_event) {
        Ok(()) => 0,
        Err(error) => {
            app::log(format!("RAMOpt stopped: {error}"));
            1
        }
    };
    unsafe {
        CloseHandle(show_event);
        CloseHandle(mutex);
    }
    if exit_code != 0 {
        process::exit(exit_code);
    }
}
