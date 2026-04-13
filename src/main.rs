// main.rs — single-instance guard, panic hook, entry point.
#![windows_subsystem = "windows"]
#![allow(unused_imports)]

mod app;
mod app_layout;
mod constants;
mod controls;
mod hotkeys;
mod tab_dimmer;
mod gamma_ramp;
mod hdr_panel;
mod nav_icons;
mod profile_manager;
mod tab_crush;
mod tab_about;
mod tab_system;
mod tab_debug;
mod tab_hotkeys;
mod ui_drawing;
mod win32;
mod startup;
mod tray;

use std::{env, path::PathBuf, ptr};

use windows::{
    core::*,
    Win32::{
        Foundation::*,
        UI::{
            HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},
            WindowsAndMessaging::*,
        },
    },
};

// CreateMutexW is not re-exported by windows-rs 0.58 Threading bindings.
#[link(name = "kernel32")]
extern "system" {
    fn CreateMutexW(
        lp_mutex_attributes: *mut std::ffi::c_void,
        b_initial_owner:     i32,
        lp_name:             *const u16,
    ) -> HANDLE;
}

fn ini_path() -> PathBuf {
    let mut dir = PathBuf::from(env::var("APPDATA").unwrap_or_else(|_| ".".into()));
    dir.push("OledHelper");
    std::fs::create_dir_all(&dir).ok();
    dir.push("OledHelper.ini");
    dir
}

/// Strips the `\\?\` prefix added by `current_exe()` — CreateProcess doesn't accept it.
fn strip_unc_prefix(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(stripped) = s.strip_prefix(r"\\?\") { PathBuf::from(stripped) } else { path }
}

fn main() {
    unsafe { run_app(); }
}

unsafe fn run_app() {
    // Belt-and-suspenders alongside the manifest: ensures PerMonitorV2 even in
    // debuggers/launchers that don't honour it. No-op if manifest already set it.
    let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

    // ── Single-instance guard ─────────────────────────────────────────────────
    // bInitialOwner=1: first instance succeeds, subsequent ones get ERROR_ALREADY_EXISTS.
    let mutex_name: Vec<u16> = "Global\\OledHelperSingleInstanceMutex\0".encode_utf16().collect();
    let mutex_handle = CreateMutexW(ptr::null_mut(), 1, mutex_name.as_ptr());
    if mutex_handle.0.is_null() || GetLastError() == ERROR_ALREADY_EXISTS {
        MessageBoxW(
            HWND(ptr::null_mut()),
            w!("Oled Helper is already running.\nCheck the system tray."),
            w!("Already Running"),
            MB_OK | MB_ICONINFORMATION,
        );
        return;
    }
    // OwnedHandle ensures CloseHandle on drop (bare HANDLE is Copy, so drop is a no-op).
    struct OwnedHandle(HANDLE);
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            if !self.0.0.is_null() {
                let _ = unsafe { windows::Win32::Foundation::CloseHandle(self.0) };
            }
        }
    }
    let _mutex = OwnedHandle(mutex_handle);

    // Reset gamma on panic
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        gamma_ramp::reset_display_ramp();
        default_hook(info);
    }));

    // ── Run ───────────────────────────────────────────────────────────────────
    if let Err(e) = app::run(ini_path()) {
        gamma_ramp::reset_display_ramp();
        let msg: Vec<u16> = format!("Fatal error: {e}\0").encode_utf16().collect();
        MessageBoxW(HWND(ptr::null_mut()), PCWSTR(msg.as_ptr()), w!("OledHelper Error"), MB_OK | MB_ICONERROR);
    }

    gamma_ramp::reset_display_ramp();

    // Capture flags before _mutex drops; all spawning happens after the drop
    // so the new instance can acquire the mutex.
    let should_restart = app::RESTART_ON_EXIT.load(std::sync::atomic::Ordering::SeqCst);
    let update_path    = app::UPDATE_RELAUNCH_PATH.lock().ok().and_then(|mut g| g.take());
    let old_exe_path   = app::OLD_EXE_PATH.lock().ok().and_then(|mut g| g.take());

    // Explicit drop before spawn — without this, the new process sees ERROR_ALREADY_EXISTS.
    drop(_mutex);

    if should_restart {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(strip_unc_prefix(exe)).spawn();
        }
    } else if let Some(path) = update_path {
        let _ = std::process::Command::new(&path).spawn();

        // Delete OledHelper_old.exe after releasing the exe image section handle.
        // Retry briefly in a background thread; sleep before exit to let it finish.
        if let Some(old_path) = old_exe_path {
            std::thread::spawn(move || {
                let p = std::path::Path::new(&old_path);
                for _ in 0..10 {
                    if std::fs::remove_file(p).is_ok() { return; }
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                let _ = std::fs::remove_file(p);
            });
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    }
}