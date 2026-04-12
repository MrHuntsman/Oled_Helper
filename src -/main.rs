// main.rs — single-instance guard, panic hook, entry point.
#![windows_subsystem = "windows"]
#![allow(unused_imports)]

mod app;
mod app_helpers;
mod app_init;
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

// CreateMutexW is not re-exported by windows-rs 0.58's Threading bindings,
// so we declare it directly — same pattern used for SetDeviceGammaRamp.
#[link(name = "kernel32")]
extern "system" {
    fn CreateMutexW(
        lp_mutex_attributes: *mut std::ffi::c_void,
        b_initial_owner:     i32,
        lp_name:             *const u16,
    ) -> HANDLE;
}

fn ini_path() -> PathBuf {
    let mut dir = PathBuf::from(
        env::var("APPDATA").unwrap_or_else(|_| ".".into()),
    );
    dir.push("OledHelper");
    std::fs::create_dir_all(&dir).ok();
    dir.push("OledHelper.ini");
    dir
}

fn main() {
    unsafe { run_app(); }
}

unsafe fn run_app() {
    // ── DPI awareness — belt-and-suspenders alongside the manifest ────────────
    // The manifest embedded by build.rs declares PerMonitorV2, but some execution
    // contexts (debuggers, certain launchers) don't honour it.  Calling this API
    // first ensures we always get crisp per-monitor scaling regardless.
    // It is a no-op if the manifest already set PerMonitorV2.
    let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

    // ── Single-instance guard (named kernel mutex) ────────────────────────────
    // CreateMutexW with bInitialOwner=TRUE succeeds for the first instance and
    // sets ERROR_ALREADY_EXISTS for every subsequent one.  The handle is held
    // in `_mutex` for the lifetime of run_app; Windows releases it automatically
    // when the process exits or the handle drops.
    let mutex_name: Vec<u16> = "Global\\OledHelperSingleInstanceMutex\0"
        .encode_utf16().collect();
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
    // Hold the handle for the process lifetime — dropped (and released) at end of run_app.
    let _mutex = mutex_handle;

    // ── Panic hook: reset gamma ramp on unexpected crash ──────────────────────
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        gamma_ramp::reset_display_ramp();
        default_hook(info);
    }));

    // ── Run the main window ───────────────────────────────────────────────────
    if let Err(e) = app::run(ini_path()) {
        gamma_ramp::reset_display_ramp();
        let msg: Vec<u16> = format!("Fatal error: {e}\0").encode_utf16().collect();
        MessageBoxW(
            HWND(ptr::null_mut()),
            PCWSTR(msg.as_ptr()),
            w!("OledHelper Error"),
            MB_OK | MB_ICONERROR,
        );
    }

    gamma_ramp::reset_display_ramp();

    // Check restart flag before _mutex drops so we know it was set intentionally,
    // but spawn AFTER the drop below so the new instance can acquire the mutex.
    let should_restart = app_helpers::RESTART_ON_EXIT.load(std::sync::atomic::Ordering::SeqCst);

    // _mutex drops here, releasing the kernel mutex

    if should_restart {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe).spawn();
        }
    }
}