// win32.rs — Thin safe wrappers around the Win32 patterns used throughout app.rs.
//
// Goals:
//   · Keep all raw pointer manipulation in one place.
//   · Let business-logic functions stay free of `unsafe` blocks for the common cases.
//   · Every function is either safe or has a clearly-documented safety contract.

#![allow(unused_must_use)]
use windows::Win32::{
    Foundation::HWND,
    Graphics::Gdi::{InvalidateRect, RedrawWindow, RDW_INVALIDATE, RDW_UPDATENOW, RDW_ERASE},
    UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW,
        ShowWindow, SetWindowTextW,
        GWLP_USERDATA, SW_SHOW, SW_HIDE,
    },
    UI::Input::KeyboardAndMouse::{
        EnableWindow,
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
        KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
        SendInput, VIRTUAL_KEY, VK_LWIN, VK_MENU,
    },
};
use windows::core::PCWSTR;

// ── ControlGroup ──────────────────────────────────────────────────────────────
//
// Owns a set of logically related HWNDs (e.g. a slider + its label + its value
// display) and lets callers act on all of them in one call.  This eliminates
// the scattered per-handle `set_visible` loops that were the source of
// mismatched-visibility bugs when toggling whole UI sections.
//
// Usage:
//   let g = ControlGroup::new(vec![h_slider, h_label, h_value]);
//   g.show();
//   g.hide();
//   g.set_enabled(false);

pub struct ControlGroup {
    handles: Vec<HWND>,
}

impl ControlGroup {
    /// Create a group from an explicit list of handles.
    pub fn new(handles: Vec<HWND>) -> Self {
        Self { handles }
    }

    /// Make every handle in the group visible.
    pub fn show(&self) {
        for &h in &self.handles {
            set_visible(h, true);
        }
    }

    /// Hide every handle in the group.
    pub fn hide(&self) {
        for &h in &self.handles {
            set_visible(h, false);
        }
    }

    /// Show or hide based on `visible`.
    pub fn set_visible(&self, visible: bool) {
        if visible { self.show() } else { self.hide() }
    }

    /// Enable or disable every handle in the group.
    #[allow(dead_code)]
    pub fn set_enabled(&self, enabled: bool) {
        for &h in &self.handles {
            unsafe { EnableWindow(h, enabled); }
        }
    }

    /// Invalidate (schedule repaint) every handle in the group.
    #[allow(dead_code)]
    pub fn invalidate_all(&self) {
        for &h in &self.handles {
            invalidate(h);
        }
    }
}

// ── State-pointer helpers ─────────────────────────────────────────────────────
//
// The main window stores its `AppState` as a raw pointer in GWLP_USERDATA so
// that the `extern "system"` WndProc can reach it.  These two functions are the
// only places in the codebase that touch that raw pointer.

/// Store `state` in `hwnd`'s GWLP_USERDATA slot, taking ownership of the Box.
///
/// # Safety
/// `hwnd` must be a valid window handle created by this process.
pub unsafe fn attach_state<T>(hwnd: HWND, state: Box<T>) {
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
}

/// Borrow the `AppState` stored in GWLP_USERDATA, if any.
///
/// Returns `None` when the slot is zero (before `attach_state` or after
/// `detach_state`).
///
/// # Safety
/// The pointer in GWLP_USERDATA must have been placed there by `attach_state<T>`
/// and must not yet have been freed by `detach_state<T>`.
pub unsafe fn borrow_state<T>(hwnd: HWND) -> Option<&'static mut T> {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut T;
    if ptr.is_null() { None } else { Some(&mut *ptr) }
}

/// Reclaim and drop the `AppState` stored in GWLP_USERDATA, then zero the slot.
///
/// # Safety
/// Same contract as `borrow_state`.  Must be called at most once per window.
pub unsafe fn detach_state<T>(hwnd: HWND) {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut T;
    if !ptr.is_null() {
        drop(Box::from_raw(ptr));
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
    }
}

// ── Text helpers ──────────────────────────────────────────────────────────────

/// Set the text of a Win32 control from a Rust `&str`.
///
/// Encodes to UTF-16 and calls `SetWindowTextW`.  This is safe to call from
/// any thread that owns the window (same thread that created it).
///
/// # Safety
/// `hwnd` must be a valid child window.
pub unsafe fn set_text(hwnd: HWND, text: &str) {
    let w: Vec<u16> = text.encode_utf16().chain([0]).collect();
    SetWindowTextW(hwnd, PCWSTR(w.as_ptr()));
}

/// Format a value and set it as the text of a Win32 control.
///
/// Convenience wrapper around `set_text` for numeric/formatted labels.
///
/// # Safety
/// Same as `set_text`.
pub unsafe fn set_text_fmt(hwnd: HWND, args: std::fmt::Arguments<'_>) {
    set_text(hwnd, &args.to_string());
}

// ── Visibility / enable helpers ───────────────────────────────────────────────

/// Show or hide a control.
pub fn set_visible(hwnd: HWND, visible: bool) {
    // ShowWindow is safe for any valid HWND (it does not dereference user pointers).
    unsafe { ShowWindow(hwnd, if visible { SW_SHOW } else { SW_HIDE }); }
}

/// Invalidate a control so it repaints on the next WM_PAINT.
pub fn invalidate(hwnd: HWND) {
    unsafe { InvalidateRect(hwnd, None, false); }
}

/// Invalidate and immediately force a repaint (equivalent to `RedrawWindow`
/// with `RDW_INVALIDATE | RDW_UPDATENOW | RDW_ERASE`).
pub fn redraw_now(hwnd: HWND) {
    unsafe { RedrawWindow(hwnd, None, None, RDW_INVALIDATE | RDW_UPDATENOW | RDW_ERASE); }
}
// ── HDR toggle via Win+Alt+B shortcut ────────────────────────────────────────

/// Simulates Win+Alt+B to toggle Windows HDR on the current display.
pub unsafe fn toggle_hdr_via_shortcut() {
    let inputs: [INPUT; 6] = [
        INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT {
            wVk: VK_LWIN,  wScan: 0, dwFlags: KEYEVENTF_EXTENDEDKEY, time: 0, dwExtraInfo: 0 } } },
        INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT {
            wVk: VK_MENU,  wScan: 0, dwFlags: KEYEVENTF_EXTENDEDKEY, time: 0, dwExtraInfo: 0 } } },
        INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT {
            wVk: VIRTUAL_KEY(b'B' as u16), wScan: 0, dwFlags: KEYEVENTF_EXTENDEDKEY, time: 0, dwExtraInfo: 0 } } },
        INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT {
            wVk: VIRTUAL_KEY(b'B' as u16), wScan: 0, dwFlags: KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP, time: 0, dwExtraInfo: 0 } } },
        INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT {
            wVk: VK_MENU,  wScan: 0, dwFlags: KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP, time: 0, dwExtraInfo: 0 } } },
        INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT {
            wVk: VK_LWIN,  wScan: 0, dwFlags: KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP, time: 0, dwExtraInfo: 0 } } },
    ];
    SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
}