// app_helpers.rs — Shared helpers used by both app.rs (WndProc) and app_init.rs.
//
// Extracted from app.rs to keep the message-dispatch file focused on routing.
// All functions here require &mut AppState and an HWND, making them unsuitable
// for individual tab files (which don't own AppState).

#![allow(non_snake_case, unused_must_use)]

use std::sync::atomic::Ordering;

use windows::{
    Win32::{
        Foundation::{COLORREF, HWND},
        Graphics::Gdi::InvalidateRect,
        UI::WindowsAndMessaging::{KillTimer, SetTimer},
    },
};

use crate::{
    app::AppState,
    constants::*,
    tab_crush::render_interval_ms,
    ui_drawing::{self, set_window_text},
    win32::set_visible,
};

// ── Debug mode flag ───────────────────────────────────────────────────────────

/// Set to true when launched with --debug. Readable from any thread/module.
static DEBUG_MODE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Set to true when the tray Restart command is used; triggers a re-spawn
/// at the end of WM_DESTROY, after the single-instance mutex is released.
pub static RESTART_ON_EXIT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn is_debug_mode() -> bool {
    DEBUG_MODE.load(Ordering::Relaxed)
}

pub fn set_debug_mode(enabled: bool) {
    DEBUG_MODE.store(enabled, Ordering::Relaxed);
}

// ── Status / error labels ─────────────────────────────────────────────────────

/// Show a transient message in the normal status label.
/// Automatically hides after 4 seconds via TIMER_STATUS_CLEAR.
pub unsafe fn set_status(st: &mut AppState, text: &str, color: COLORREF) {
    st.status_color = color;
    set_window_text(st.h_lbl_status, text);
    set_visible(st.h_lbl_status, !text.is_empty());
    crate::win32::redraw_now(st.h_lbl_status);

    let hwnd = st.hwnd;
    // Only auto-clear transient accent confirmations.
    // Warnings (C_WARN) and errors (C_ERR) persist until explicitly cleared.
    if text.is_empty() || color != C_ACCENT {
        KillTimer(hwnd, TIMER_STATUS_CLEAR);
    } else {
        SetTimer(hwnd, TIMER_STATUS_CLEAR, 4000, None);
    }
}

/// Show or clear the persistent error label (shown below the status label).
/// Pass an empty string to hide it.
pub unsafe fn set_error(st: &mut AppState, text: &str) {
    set_window_text(st.h_lbl_error, text);
    set_visible(st.h_lbl_error, !text.is_empty());
    crate::win32::redraw_now(st.h_lbl_error);
}

// ── Gamma ramp ────────────────────────────────────────────────────────────────

pub unsafe fn apply_ramp(st: &mut AppState, _hwnd: HWND) {
    if st.crush.previewing { return; }

    // NVIDIA CAM blocks gamma — show warning, skip the write entirely.
    if st.nvidia_cam_enabled {
        set_error(st, "⚠  Gamma blocked — disable NVIDIA Override / Colour Accuracy Mode");
        return;
    }

    let (ok, _v) = st.crush.apply_ramp();
    if !ok {
        set_error(st, "⚠  Gamma blocked by GPU driver");
    }
    // Never call set_error("") here — only check_gamma_blocked() may clear
    // the error, after confirming via probe that the driver is unblocked.
}

pub unsafe fn maybe_restore_gamma(st: &mut AppState, _hwnd: HWND) {
    if st.nvidia_cam_enabled { return; }
    // Attempt restore; if the write is accepted, confirm via readback and
    // clear any stale "gamma blocked" error. This is the only place that
    // may call set_error("").
    let (ok, _) = st.crush.apply_ramp();
    if ok {
        set_error(st, "");
    }
}

// ── Render timer ──────────────────────────────────────────────────────────────

pub unsafe fn rearm_render_timer(hwnd: HWND) {
    SetTimer(hwnd, TIMER_RENDER, render_interval_ms(), None);
}

// ── Slider animation interval ─────────────────────────────────────────────────

/// Recompute and store the slider animation timer interval from the current
/// display refresh rate. Call whenever the Hz changes.
pub fn update_slider_anim_interval(hz: i32) {
    let hz = hz.max(60) as u32;
    let ms = (1000 / hz).clamp(1, 16);
    ui_drawing::SLIDER_ANIM_INTERVAL_MS.store(ms, Ordering::Relaxed);
}

// ── Tab visibility ────────────────────────────────────────────────────────────

pub unsafe fn show_tab(st: &mut AppState, hwnd: HWND) {
    let tab = st.active_tab;

    st.crush.group.set_visible(tab == 0);

    set_visible(st.dimmer.h_lbl_dim_title,   tab == 1);
    set_visible(st.dimmer.h_lbl_dim_sub,     tab == 1);
    set_visible(st.dimmer.h_chk_taskbar_dim, tab == 1);

    let show_dim_controls = tab == 1 && st.dimmer.enabled;
    st.dimmer.grp_dim_controls.set_visible(show_dim_controls);

    st.system.group.set_visible(tab == 2);
    st.hotkeys.group.set_visible(tab == 3);
    st.debug.group.set_visible(tab == 4);
    st.about.group.set_visible(tab == 5);

    if is_debug_mode() {
        if tab == 4 {
            SetTimer(hwnd, TIMER_DEBUG_REFRESH, 500, None);
            crate::tab_debug::install_mouse_hook(hwnd);
            crate::tab_debug::install_debug_hotkeys(hwnd);
        } else {
            KillTimer(hwnd, TIMER_DEBUG_REFRESH);
            crate::tab_debug::uninstall_mouse_hook();
            crate::tab_debug::uninstall_debug_hotkeys(hwnd);
        }
    }
}

// ── Nav button → tab index ────────────────────────────────────────────────────

pub fn nav_btn_to_tab(id: usize) -> Option<usize> {
    match id {
        IDC_NAV_BTN_0 => Some(0),
        IDC_NAV_BTN_1 => Some(1),
        IDC_NAV_BTN_5 => Some(2), // System
        IDC_NAV_BTN_2 => Some(3), // Hotkeys
        IDC_NAV_BTN_3 => Some(4), // Debug
        IDC_NAV_BTN_4 => Some(5), // About
        _ => None,
    }
}