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

/// Strips the `\\?\` extended-length path prefix added by `current_exe()` on
/// Windows.  `std::process::Command` (CreateProcess) does not accept `\\?\`
/// paths, so the prefix must be removed before spawning a child process.
fn strip_unc_prefix(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        PathBuf::from(stripped)
    } else {
        path
    }
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
    // Wrap the raw HANDLE in a newtype whose Drop calls CloseHandle.
    // HANDLE is Copy, so a bare `let _mutex = mutex_handle` followed by
    // `drop(_mutex)` is a no-op — the kernel object would never be released
    // until the process exits.  OwnedHandle guarantees release on drop.
    struct OwnedHandle(HANDLE);
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            if !self.0.0.is_null() {
                let _ = unsafe { windows::Win32::Foundation::CloseHandle(self.0) };
            }
        }
    }
    let _mutex = OwnedHandle(mutex_handle);

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

    // Capture flags before _mutex drops (reads are safe here), but all
    // spawning happens after the drop so the new instance can acquire the mutex.
    let should_restart = app::RESTART_ON_EXIT.load(std::sync::atomic::Ordering::SeqCst);
    let update_path = app::UPDATE_RELAUNCH_PATH.lock().ok()
        .and_then(|mut g| g.take());
    let old_exe_path = app::OLD_EXE_PATH.lock().ok()
        .and_then(|mut g| g.take());

    // Drop the mutex explicitly here — before any spawn — so the new instance
    // can acquire it successfully.  Without this explicit drop, _mutex lives
    // until the closing brace of run_app(), which is after the spawn() calls,
    // causing the new process to see ERROR_ALREADY_EXISTS and show the
    // "already running" message box.
    drop(_mutex);

    if should_restart {
        // Tray "Restart" — relaunch the current exe.
        // current_exe() returns a \\?\ path on Windows; strip it so CreateProcess works.
        if let Ok(exe) = std::env::current_exe() {
            let exe = strip_unc_prefix(exe);
            let _ = std::process::Command::new(exe).spawn();
        }
    } else if let Some(path) = update_path {
        // Self-update — launch the newly-installed exe.
        let _ = std::process::Command::new(&path).spawn();

        // Delete OledHelper_old.exe now that the mutex is released and the
        // new process has been spawned.  This process is about to exit, so
        // Windows will release the exe image section handle momentarily,
        // allowing the delete to succeed.  We retry briefly in a background
        // thread and sleep to let it finish before the process exits.
        if let Some(old_path) = old_exe_path {
            std::thread::spawn(move || {
                let p = std::path::Path::new(&old_path);
                for _ in 0..10 {
                    if std::fs::remove_file(p).is_ok() { return; }
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                let _ = std::fs::remove_file(p);
            });
            // Give the background thread a moment to finish before we exit.
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    }
}