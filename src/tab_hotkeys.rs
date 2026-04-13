// tab_hotkeys.rs — Hotkeys settings tab.
//
// Each hotkey row: action label, owner-drawn pill button that captures key
// combos on focus, and a "×" clear button.
//
// The pill is BS_OWNERDRAW. When focused it shows an accent border and
// intercepts WM_KEYDOWN/WM_SYSKEYDOWN via subclass to record combos.
// WM_DRAWITEM in app.rs calls draw_hotkey_pill().
//
// Every capture and clear fires an EN_CHANGE-style notify so app.rs can
// persist and re-register immediately.

#![allow(unused_must_use, non_snake_case)]
use std::mem;

use windows::{
    core::*,
    Win32::{
        Foundation::*,
        Graphics::Gdi::{HFONT, InvalidateRect},
        UI::{
            Controls::*,
            Input::KeyboardAndMouse::*,
            WindowsAndMessaging::*,
        },
    },
};

use crate::{
    constants::*,
    controls::ControlBuilder,
    profile_manager::ProfileManager,
    ui_drawing::{get_accent_color, install_action_btn_hover},
    win32::ControlGroup,
};

// Re-export so app.rs can call it from WM_DRAWITEM.
pub use self::draw::draw_hotkey_pill;

// ── Mouse-button sentinel codes ───────────────────────────────────────────────
//
// Synthetic VK-like values stored in the pill text and INI.
// Live above the real VK range (0x00–0xFF) — no collisions.
// Left/right click and wheel are intentionally excluded.
//
// WH_MOUSE_LL reports extra buttons as XBUTTON events whose HIWORD(mouseData)
// is a 1-based button-index. We reserve one sentinel per index (1–16) for a
// stable round-trip through the INI.
pub const MB_MIDDLE:    u32 = 0x10000; // Middle click (WM_MBUTTONDOWN)
pub const MB_XBUTTON1:  u32 = 0x10001; // Mouse4  – back
pub const MB_XBUTTON2:  u32 = 0x10002; // Mouse5  – forward
pub const MB_XBUTTON3:  u32 = 0x10003; // Mouse6
pub const MB_XBUTTON4:  u32 = 0x10004; // Mouse7
pub const MB_XBUTTON5:  u32 = 0x10005; // Mouse8
pub const MB_XBUTTON6:  u32 = 0x10006; // Mouse9
pub const MB_XBUTTON7:  u32 = 0x10007; // Mouse10
pub const MB_XBUTTON8:  u32 = 0x10008; // Mouse11
pub const MB_XBUTTON9:  u32 = 0x10009; // Mouse12
pub const MB_XBUTTON10: u32 = 0x1000A; // Mouse13
pub const MB_XBUTTON11: u32 = 0x1000B; // Mouse14
pub const MB_XBUTTON12: u32 = 0x1000C; // Mouse15
pub const MB_XBUTTON13: u32 = 0x1000D; // Mouse16
pub const MB_XBUTTON14: u32 = 0x1000E; // Mouse17
pub const MB_XBUTTON15: u32 = 0x1000F; // Mouse18
pub const MB_XBUTTON16: u32 = 0x10010; // Mouse19

/// Convert a 1-based XBUTTON index (HIWORD of mouseData) to its sentinel.
/// Returns `None` for index 0 or above 16.
#[inline]
pub fn xbutton_index_to_sentinel(idx: u16) -> Option<u32> {
    match idx {
        1  => Some(MB_XBUTTON1),  2  => Some(MB_XBUTTON2),
        3  => Some(MB_XBUTTON3),  4  => Some(MB_XBUTTON4),
        5  => Some(MB_XBUTTON5),  6  => Some(MB_XBUTTON6),
        7  => Some(MB_XBUTTON7),  8  => Some(MB_XBUTTON8),
        9  => Some(MB_XBUTTON9),  10 => Some(MB_XBUTTON10),
        11 => Some(MB_XBUTTON11), 12 => Some(MB_XBUTTON12),
        13 => Some(MB_XBUTTON13), 14 => Some(MB_XBUTTON14),
        15 => Some(MB_XBUTTON15), 16 => Some(MB_XBUTTON16),
        _  => None,
    }
}

/// True if `vk` is any of the mouse-button sentinels.
#[inline]
pub fn is_mouse_sentinel(vk: u32) -> bool {
    vk >= MB_MIDDLE && vk <= MB_XBUTTON16
}

// ── Key-name table ────────────────────────────────────────────────────────────

fn vk_name(vk: u32) -> Option<&'static str> {
    Some(match vk {
        0x08 => "Backspace", 0x09 => "Tab",   0x0D => "Enter",
        0x13 => "Pause",     0x14 => "CapsLk", 0x1B => "Esc",
        0x20 => "Space",
        0x21 => "PgUp",  0x22 => "PgDn",  0x23 => "End",  0x24 => "Home",
        0x25 => "Left",  0x26 => "Up",    0x27 => "Right", 0x28 => "Down",
        0x2C => "Print", 0x2D => "Insert", 0x2E => "Delete",
        0x30 => "0", 0x31 => "1", 0x32 => "2", 0x33 => "3", 0x34 => "4",
        0x35 => "5", 0x36 => "6", 0x37 => "7", 0x38 => "8", 0x39 => "9",
        0x41 => "A", 0x42 => "B", 0x43 => "C", 0x44 => "D", 0x45 => "E",
        0x46 => "F", 0x47 => "G", 0x48 => "H", 0x49 => "I", 0x4A => "J",
        0x4B => "K", 0x4C => "L", 0x4D => "M", 0x4E => "N", 0x4F => "O",
        0x50 => "P", 0x51 => "Q", 0x52 => "R", 0x53 => "S", 0x54 => "T",
        0x55 => "U", 0x56 => "V", 0x57 => "W", 0x58 => "X", 0x59 => "Y",
        0x5A => "Z",
        0x60 => "Num0", 0x61 => "Num1", 0x62 => "Num2", 0x63 => "Num3",
        0x64 => "Num4", 0x65 => "Num5", 0x66 => "Num6", 0x67 => "Num7",
        0x68 => "Num8", 0x69 => "Num9",
        0x6A => "Num*", 0x6B => "Num+", 0x6D => "Num-",
        0x6E => "Num.", 0x6F => "Num/",
        0x70 => "F1",  0x71 => "F2",  0x72 => "F3",  0x73 => "F4",
        0x74 => "F5",  0x75 => "F6",  0x76 => "F7",  0x77 => "F8",
        0x78 => "F9",  0x79 => "F10", 0x7A => "F11", 0x7B => "F12",
        0x7C => "F13", 0x7D => "F14", 0x7E => "F15", 0x7F => "F16",
        0x80 => "F17", 0x81 => "F18", 0x82 => "F19", 0x83 => "F20",
        0x84 => "F21", 0x85 => "F22", 0x86 => "F23", 0x87 => "F24",
        0xBA => ";",  0xBB => "=",  0xBC => ",",  0xBD => "-",
        0xBE => ".",  0xBF => "/",  0xC0 => "`",
        0xDB => "[",  0xDC => "\\", 0xDD => "]",  0xDE => "'",
        // Mouse sentinels (not real VKs)
        x if x == MB_MIDDLE    => "MButton",
        x if x == MB_XBUTTON1  => "Mouse4",
        x if x == MB_XBUTTON2  => "Mouse5",
        x if x == MB_XBUTTON3  => "Mouse6",
        x if x == MB_XBUTTON4  => "Mouse7",
        x if x == MB_XBUTTON5  => "Mouse8",
        x if x == MB_XBUTTON6  => "Mouse9",
        x if x == MB_XBUTTON7  => "Mouse10",
        x if x == MB_XBUTTON8  => "Mouse11",
        x if x == MB_XBUTTON9  => "Mouse12",
        x if x == MB_XBUTTON10 => "Mouse13",
        x if x == MB_XBUTTON11 => "Mouse14",
        x if x == MB_XBUTTON12 => "Mouse15",
        x if x == MB_XBUTTON13 => "Mouse16",
        x if x == MB_XBUTTON14 => "Mouse17",
        x if x == MB_XBUTTON15 => "Mouse18",
        x if x == MB_XBUTTON16 => "Mouse19",
        _ => return None,
    })
}

pub unsafe fn format_hotkey(vk: u32) -> Option<String> {
    match vk {
        x if x == VK_SHIFT.0 as u32    || x == VK_LSHIFT.0 as u32   || x == VK_RSHIFT.0 as u32   ||
             x == VK_CONTROL.0 as u32   || x == VK_LCONTROL.0 as u32 || x == VK_RCONTROL.0 as u32 ||
             x == VK_MENU.0 as u32      || x == VK_LMENU.0 as u32    || x == VK_RMENU.0 as u32    ||
             x == VK_LWIN.0 as u32      || x == VK_RWIN.0 as u32 => return None,
        _ => {}
    }
    let key_name = vk_name(vk)?;
    // Mouse sentinels carry no keyboard modifiers.
    let is_mouse = is_mouse_sentinel(vk);
    let ctrl  = !is_mouse && (GetAsyncKeyState(VK_CONTROL.0 as i32) as u16) & 0x8000 != 0;
    let shift = !is_mouse && (GetAsyncKeyState(VK_SHIFT.0   as i32) as u16) & 0x8000 != 0;
    let alt   = !is_mouse && (GetAsyncKeyState(VK_MENU.0    as i32) as u16) & 0x8000 != 0;
    let win   = !is_mouse && ((GetAsyncKeyState(VK_LWIN.0   as i32) as u16) & 0x8000 != 0
                           || (GetAsyncKeyState(VK_RWIN.0   as i32) as u16) & 0x8000 != 0);
    let mut s = String::with_capacity(24);
    if win   { s.push_str("Win+");  }
    if ctrl  { s.push_str("Ctrl+"); }
    if alt   { s.push_str("Alt+");  }
    if shift { s.push_str("Shift+");}
    s.push_str(key_name);
    Some(s)
}

// ── Property names ────────────────────────────────────────────────────────────

const PROP_ORIG_PROC: PCWSTR = w!("BCT_HkOrigProc");
/// Set to 1 when the pill has keyboard focus.
pub const PROP_HK_FOCUSED: PCWSTR = w!("BCT_HkFocused");
/// Set to 1 while waiting for a key (focus + no key yet confirmed).
pub const PROP_HK_RECORDING: PCWSTR = w!("BCT_HkRecording");
/// Heap-allocated UTF-16 snapshot of text at focus-gain; restored on cancel.
const PROP_HK_SAVED_TEXT: PCWSTR = w!("BCT_HkSavedText");
/// Non-null while the cursor is hovering (read by draw_hotkey_pill).
const PROP_HK_HOVERED: PCWSTR = w!("BCT_HkHovered");

// ── Pill-button subclass proc ─────────────────────────────────────────────────
//
// Installed on every owner-drawn hotkey pill button.
// • WM_SETFOCUS / WM_KILLFOCUS  — track focus state and repaint
// • WM_GETDLGCODE               — claim all keys when focused
// • WM_KEYDOWN / WM_SYSKEYDOWN  — record key combo
// • WM_CHAR etc.                — swallowed to prevent beeps

pub unsafe extern "system" fn hotkey_pill_subclass_proc(
    hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM,
) -> LRESULT {
    let orig_ptr = GetPropW(hwnd, PROP_ORIG_PROC).0 as isize;
    let call_orig = || -> LRESULT {
        if orig_ptr != 0 {
            let f: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT =
                mem::transmute(orig_ptr);
            f(hwnd, msg, wp, lp)
        } else {
            DefWindowProcW(hwnd, msg, wp, lp)
        }
    };

    match msg {
        WM_SETFOCUS => {
            SetPropW(hwnd, PROP_HK_FOCUSED,   HANDLE(1 as *mut _));
            SetPropW(hwnd, PROP_HK_RECORDING, HANDLE(1 as *mut _));
            // Free any pre-existing snapshot to avoid a leak on unexpected re-focus.
            let old_ptr = GetPropW(hwnd, PROP_HK_SAVED_TEXT).0 as *mut Vec<u16>;
            if !old_ptr.is_null() { drop(Box::from_raw(old_ptr)); }
            // Snapshot current text so we can restore it on click-away.
            let mut buf = [0u16; 256];
            let len = GetWindowTextW(hwnd, &mut buf) as usize;
            let snapshot: Box<Vec<u16>> = Box::new(buf[..len+1].to_vec()); // includes NUL
            SetPropW(hwnd, PROP_HK_SAVED_TEXT, HANDLE(Box::into_raw(snapshot) as *mut _));
            // Capture the mouse so WM_LBUTTONDOWN fires even on non-focusable areas
            // (background, labels, separators). Without capture the pill can get stuck
            // in recording mode if the user clicks away from any button.
            SetCapture(hwnd);
            InvalidateRect(hwnd, None, false);
            call_orig()
        }

        WM_KILLFOCUS => {
            // Restore snapshot if recording was cancelled (no key confirmed).
            let was_recording = !GetPropW(hwnd, PROP_HK_RECORDING).0.is_null();
            let saved_ptr = GetPropW(hwnd, PROP_HK_SAVED_TEXT).0 as *mut Vec<u16>;
            if was_recording && !saved_ptr.is_null() {
                let saved = &*saved_ptr;
                SetWindowTextW(hwnd, PCWSTR(saved.as_ptr()));
            }
            if !saved_ptr.is_null() {
                drop(Box::from_raw(saved_ptr));
                RemovePropW(hwnd, PROP_HK_SAVED_TEXT);
            }
            RemovePropW(hwnd, PROP_HK_FOCUSED);
            RemovePropW(hwnd, PROP_HK_RECORDING);
            // Release capture if we still own it (focus may have moved via keyboard
            // or programmatically rather than through our WM_LBUTTONDOWN handler).
            if GetCapture() == hwnd { ReleaseCapture(); }
            InvalidateRect(hwnd, None, false);
            call_orig()
        }

        WM_GETDLGCODE => {
            // Claim all keys — Tab, Enter, arrows — so nothing is stolen.
            let base = call_orig();
            LRESULT(base.0 | DLGC_WANTALLKEYS as isize)
        }

        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = wp.0 as u32;
            // Esc / Delete → clear
            if vk == VK_ESCAPE.0 as u32 || vk == VK_DELETE.0 as u32 {
                let ws: Vec<u16> = "None".encode_utf16().chain([0]).collect();
                SetWindowTextW(hwnd, PCWSTR(ws.as_ptr()));
                RemovePropW(hwnd, PROP_HK_RECORDING);
                InvalidateRect(hwnd, None, false);
                notify_parent_changed(hwnd);
                if GetCapture() == hwnd { ReleaseCapture(); }
                let parent = GetParent(hwnd).unwrap_or_default();
                SetFocus(parent);
                return LRESULT(0);
            }
            if let Some(s) = format_hotkey(vk) {
                // Clear the same binding on any sibling pill — one action per hotkey.
                clear_duplicate_siblings(hwnd, &s);
                let ws: Vec<u16> = s.encode_utf16().chain([0]).collect();
                SetWindowTextW(hwnd, PCWSTR(ws.as_ptr()));
                RemovePropW(hwnd, PROP_HK_RECORDING);
                InvalidateRect(hwnd, None, false);
                notify_parent_changed(hwnd);
                if GetCapture() == hwnd { ReleaseCapture(); }
                let parent = GetParent(hwnd).unwrap_or_default();
                SetFocus(parent);
            }
            LRESULT(0)
        }

        WM_MOUSEMOVE => {
            // Start tracking so we get WM_MOUSELEAVE when the cursor exits.
            if GetPropW(hwnd, PROP_HK_HOVERED).0.is_null() {
                let mut tme = TRACKMOUSEEVENT {
                    cbSize:      std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags:     TME_LEAVE,
                    hwndTrack:   hwnd,
                    dwHoverTime: 0,
                };
                TrackMouseEvent(&mut tme);
                SetPropW(hwnd, PROP_HK_HOVERED, HANDLE(1 as *mut _));
                InvalidateRect(hwnd, None, false);
            }
            call_orig()
        }

        WM_MOUSELEAVE => {
            RemovePropW(hwnd, PROP_HK_HOVERED);
            InvalidateRect(hwnd, None, false);
            call_orig()
        }

        WM_CHAR | WM_SYSCHAR | WM_DEADCHAR | WM_SYSDEADCHAR => LRESULT(0),

        // Middle-click and Mouse4–Mouse19 can be bound while recording.
        // Left/right click and wheel are intentionally excluded.
        WM_MBUTTONDOWN | WM_XBUTTONDOWN => {
            if GetPropW(hwnd, PROP_HK_RECORDING).0.is_null() {
                return call_orig();
            }
            let vk = if msg == WM_MBUTTONDOWN {
                MB_MIDDLE
            } else {
                // XBUTTON id is in the high word of wp (1-based index).
                let idx = ((wp.0 >> 16) & 0xFFFF) as u16;
                match xbutton_index_to_sentinel(idx) {
                    Some(s) => s,
                    None    => return call_orig(), // unknown index — ignore
                }
            };
            if let Some(s) = format_hotkey(vk) {
                clear_duplicate_siblings(hwnd, &s);
                let ws: Vec<u16> = s.encode_utf16().chain([0]).collect();
                SetWindowTextW(hwnd, PCWSTR(ws.as_ptr()));
                RemovePropW(hwnd, PROP_HK_RECORDING);
                InvalidateRect(hwnd, None, false);
                notify_parent_changed(hwnd);
                if GetCapture() == hwnd { ReleaseCapture(); }
                let parent = GetParent(hwnd).unwrap_or_default();
                SetFocus(parent);
            }
            LRESULT(0)
        }

        // We own mouse capture while recording, so every WM_LBUTTONDOWN in the
        // whole window arrives here. If the click is outside the pill, cancel
        // recording — release capture and move focus; WM_KILLFOCUS restores text.
        // The subsequent WM_LBUTTONUP (still captured) is eaten harmlessly.
        WM_LBUTTONDOWN => {
            if !GetPropW(hwnd, PROP_HK_RECORDING).0.is_null() {
                let x = (lp.0 & 0xFFFF) as i16 as i32;
                let y = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
                let mut rc = RECT::default();
                GetClientRect(hwnd, &mut rc);
                let inside = x >= rc.left && x < rc.right && y >= rc.top && y < rc.bottom;
                if !inside {
                    ReleaseCapture();
                    let parent = GetParent(hwnd).unwrap_or_default();
                    SetFocus(parent);
                    return LRESULT(0);
                }
            } else {
                return LRESULT(0);
            }
            call_orig()
        }

        // WM_CAPTURECHANGED fires when something else steals capture.
        // Treat it the same as clicking away.
        WM_CAPTURECHANGED => {
            if !GetPropW(hwnd, PROP_HK_RECORDING).0.is_null() {
                // Don't call ReleaseCapture — we no longer own it.
                let parent = GetParent(hwnd).unwrap_or_default();
                SetFocus(parent);
            }
            call_orig()
        }

        // Enter recording mode on left-click release, but only when:
        //   • the release is over the button (not a drag-off), AND
        //   • the pill is not already recording.
        // If already recording, WM_LBUTTONDOWN already cancelled (ReleaseCapture
        // + SetFocus(parent)), so the pill is no longer focused by now. Calling
        // SetFocus again would immediately re-enter recording — avoid that.
        WM_LBUTTONUP => {
            let already_recording = !GetPropW(hwnd, PROP_HK_RECORDING).0.is_null();
            let x = (lp.0 & 0xFFFF) as i16 as i32;
            let y = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut rc = RECT::default();
            GetClientRect(hwnd, &mut rc);
            let over_button = x >= rc.left && x < rc.right && y >= rc.top && y < rc.bottom;
            if over_button && !already_recording {
                if GetFocus() != hwnd { SetFocus(hwnd); }
                return LRESULT(0);
            }
            call_orig()
        }

        // Free snapshot and clear all props if destroyed while focused/recording.
        WM_NCDESTROY => {
            let saved_ptr = GetPropW(hwnd, PROP_HK_SAVED_TEXT).0 as *mut Vec<u16>;
            if !saved_ptr.is_null() {
                drop(Box::from_raw(saved_ptr));
                RemovePropW(hwnd, PROP_HK_SAVED_TEXT);
            }
            RemovePropW(hwnd, PROP_HK_FOCUSED);
            RemovePropW(hwnd, PROP_HK_RECORDING);
            RemovePropW(hwnd, PROP_HK_HOVERED);
            call_orig()
        }

        _ => call_orig(),
    }
}

/// Send EN_CHANGE-style WM_COMMAND to the parent so app.rs auto-saves.
unsafe fn notify_parent_changed(hwnd: HWND) {
    let id     = GetWindowLongPtrW(hwnd, GWLP_ID) as usize;
    let parent = GetParent(hwnd).unwrap_or_default();
    SendMessageW(
        parent,
        WM_COMMAND,
        WPARAM(((EN_CHANGE as usize) << 16) | (id & 0xFFFF)),
        LPARAM(hwnd.0 as isize),
    );
}

/// Reset any sibling pill showing `combo` to "None" and notify the parent.
/// Ensures each hotkey is assigned to at most one action.
unsafe fn clear_duplicate_siblings(hwnd: HWND, combo: &str) {
    let parent = match GetParent(hwnd) {
        Ok(p) => p,
        Err(_) => return,
    };
    // A pill is identified by having PROP_ORIG_PROC installed.
    let mut child = GetWindow(parent, GW_CHILD).unwrap_or_default();
    while !child.0.is_null() {
        if child != hwnd && !GetPropW(child, PROP_ORIG_PROC).0.is_null() {
            let mut buf = [0u16; 256];
            let len = GetWindowTextW(child, &mut buf) as usize;
            let existing = String::from_utf16_lossy(&buf[..len]);
            if existing == combo {
                let none_ws: Vec<u16> = "None".encode_utf16().chain([0]).collect();
                SetWindowTextW(child, PCWSTR(none_ws.as_ptr()));
                InvalidateRect(child, None, false);
                notify_parent_changed(child);
            }
        }
        child = GetWindow(child, GW_HWNDNEXT).unwrap_or_default();
        if child.0.is_null() { break; }
    }
}

// ── Drawing ───────────────────────────────────────────────────────────────────

mod draw {
    use super::*;
    use windows::Win32::Graphics::Gdi::*;

    /// Paint one hotkey pill from WM_DRAWITEM.
    pub unsafe fn draw_hotkey_pill(di: &DRAWITEMSTRUCT) {
        let hdc = di.hDC;
        let rc  = di.rcItem;

        let focused   = !GetPropW(di.hwndItem, PROP_HK_FOCUSED).0.is_null();
        let recording = !GetPropW(di.hwndItem, PROP_HK_RECORDING).0.is_null();
        let pressed   = di.itemState.0 & ODS_SELECTED.0 != 0;
        let hovered   = !GetPropW(di.hwndItem, PROP_HK_HOVERED).0.is_null();

        let dpi = GetDeviceCaps(hdc, LOGPIXELSX).max(96);
        let s   = |px: i32| (px * dpi / 96).max(1);

        let accent = get_accent_color();

        // Background: lighten on hover, lighter still when pressed.
        let bg_color = if pressed       { COLORREF(0x00404040) }
                       else if hovered  { COLORREF(0x00363636) }
                       else             { COLORREF(0x002A2A2A) };
        let bg_br    = CreateSolidBrush(bg_color);
        FillRect(hdc, &rc, bg_br);
        DeleteObject(bg_br);

        // Rounded-rect border: subtle 1px, brightened on hover.
        let radius = s(6);
        let border_color = if hovered { COLORREF(0x00666666) } else { COLORREF(0x00484848) };
        let pen    = CreatePen(PS_SOLID, 1, border_color);
        let old_p  = SelectObject(hdc, pen);
        let old_br = SelectObject(hdc, GetStockObject(NULL_BRUSH));
        RoundRect(hdc, rc.left, rc.top, rc.right - 1, rc.bottom - 1, radius, radius);
        SelectObject(hdc, old_p);
        SelectObject(hdc, old_br);
        DeleteObject(pen);

        // Accent underline (focused only): 2dp bar along the bottom, inset from corners.
        if focused {
            let bar_h     = s(2);
            let bar_inset = s(4);
            let bar_rc    = RECT {
                left:   rc.left  + bar_inset,
                top:    rc.bottom - bar_h,
                right:  rc.right - bar_inset,
                bottom: rc.bottom,
            };
            let accent_br = CreateSolidBrush(accent);
            FillRect(hdc, &bar_rc, accent_br);
            DeleteObject(accent_br);
        }

        // Text
        let mut buf = [0u16; 256];
        let len = GetWindowTextW(di.hwndItem, &mut buf) as usize;
        let is_none = len == 0 || String::from_utf16_lossy(&buf[..len]) == "None";

        SetBkMode(hdc, TRANSPARENT);
        let pad_l = s(10);
        let pad_r = s(6);

        if recording && is_none {
            SetTextColor(hdc, COLORREF(0x00666666));
            let prompt: Vec<u16> = "press a key or mouse button…".encode_utf16().chain([0]).collect();
            let mut rc_text = RECT { left: rc.left + pad_l, top: rc.top, right: rc.right - pad_r, bottom: rc.bottom };
            DrawTextW(hdc, &mut prompt[..prompt.len()-1].to_vec(), &mut rc_text,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        } else if is_none {
            SetTextColor(hdc, COLORREF(0x00505050));
            let none_text: Vec<u16> = "None".encode_utf16().chain([0]).collect();
            let mut rc_text = RECT { left: rc.left + pad_l, top: rc.top, right: rc.right - pad_r, bottom: rc.bottom };
            DrawTextW(hdc, &mut none_text[..none_text.len()-1].to_vec(), &mut rc_text,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        } else {
            // Bound combo: light grey normally, accent tint when focused.
            let text_color = if focused { accent } else { COLORREF(0x00CCCCCC) };
            SetTextColor(hdc, text_color);
            let mut rc_text = RECT { left: rc.left + pad_l, top: rc.top, right: rc.right - pad_r, bottom: rc.bottom };
            DrawTextW(hdc, &mut buf[..len], &mut rc_text,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        }
    }
}

// ── HotkeyRow ─────────────────────────────────────────────────────────────────

pub struct HotkeyRow {
    pub h_lbl:   HWND,
    /// Owner-drawn pill button.
    pub h_edit:  HWND,
    pub h_clear: HWND,
    pub ini_key: &'static str,
    pub clr_id:  usize,
    /// Control ID of the pill (for EN_CHANGE matching in app.rs).
    #[allow(dead_code)]
    pub edit_id: usize,
}

// ── HotkeysTab ────────────────────────────────────────────────────────────────

pub struct HotkeysTab {
    pub h_lbl_title:       HWND,
    pub h_lbl_desc:        HWND,
    pub h_lbl_sect_dimmer: HWND,
    pub h_lbl_sect_crush:  HWND,
    #[allow(dead_code)]
    pub h_lbl_sect_system: HWND,
    pub h_sep_sect: [HWND; 3],
    pub rows: [HotkeyRow; 6],
    pub h_sep_bottom: HWND,
    pub group: ControlGroup,
}

impl HotkeysTab {
    pub unsafe fn new(
        parent:      HWND,
        hinstance:   HINSTANCE,
        dpi:         u32,
        font_normal: HFONT,
        font_title:  HFONT,
        ini:         &mut ProfileManager,
    ) -> Self {
        let cb  = ControlBuilder { parent, hinstance, dpi, font: font_normal };
        let sec = "Hotkeys";

        let h_lbl_title = cb.static_text(w!("Hotkeys"), 0);
        SendMessageW(h_lbl_title, WM_SETFONT, WPARAM(font_title.0 as usize), LPARAM(1));

        let h_lbl_desc = cb.static_text(
            w!(
                "Click a field then press a key combination. Esc or Delete to clear.\r\n\
                 Supported modifiers: Win, Ctrl, Alt, Shift."
            ),
            SS_NOPREFIX,
        );

        let h_lbl_sect_dimmer = cb.static_text(w!("Taskbar Dimmer"), SS_NOPREFIX);
        let h_sep_sect0       = cb.static_text(w!(""), SS_BLACKRECT);
        let h_lbl_sect_crush  = cb.static_text(w!("Black Crush Tweak"), SS_NOPREFIX);
        let h_sep_sect1       = cb.static_text(w!(""), SS_BLACKRECT);
        let h_lbl_sect_system = cb.static_text(w!("System"), SS_NOPREFIX);
        let h_sep_sect2       = cb.static_text(w!(""), SS_BLACKRECT);

        // Section headings: 11pt bold
        let font_sect = crate::ui_drawing::make_font_cached(w!("Segoe UI"), 11, dpi, true);
        SendMessageW(h_lbl_sect_dimmer, WM_SETFONT, WPARAM(font_sect.0 as usize), LPARAM(1));
        SendMessageW(h_lbl_sect_crush,  WM_SETFONT, WPARAM(font_sect.0 as usize), LPARAM(1));
        SendMessageW(h_lbl_sect_system, WM_SETFONT, WPARAM(font_sect.0 as usize), LPARAM(1));

        let make_row = |lbl_text: PCWSTR,
                        id_edit: usize, id_clear: usize,
                        ini_key: &'static str| -> HotkeyRow {
            let h_lbl   = cb.static_text(lbl_text, SS_NOPREFIX);
            let initial = ini.read(sec, ini_key, "None");
            let h_edit  = Self::make_pill_button(&cb, id_edit, &initial);
            let h_clear = cb.button(w!("×"), id_clear);
            install_action_btn_hover(h_clear);
            HotkeyRow { h_lbl, h_edit, h_clear, ini_key, clr_id: id_clear, edit_id: id_edit }
        };

        let row0 = make_row(w!("Toggle Black Crush"),
            IDC_HK_EDT_TOGGLE_CRUSH,  IDC_HK_CLR_TOGGLE_CRUSH,  "ToggleBlackCrush");
        let row1 = make_row(w!("Hold to Compare"),
            IDC_HK_EDT_HOLD_COMPARE,  IDC_HK_CLR_HOLD_COMPARE,  "HoldCompare");
        let row2 = make_row(w!("Decrease Black Crush"),
            IDC_HK_EDT_DECREASE,      IDC_HK_CLR_DECREASE,       "DecreaseBlackCrush");
        let row3 = make_row(w!("Increase Black Crush"),
            IDC_HK_EDT_INCREASE,      IDC_HK_CLR_INCREASE,       "IncreaseBlackCrush");
        let row4 = make_row(w!("Toggle Taskbar Dimmer"),
            IDC_HK_EDT_TOGGLE_DIMMER, IDC_HK_CLR_TOGGLE_DIMMER, "ToggleTaskbarDimmer");
        let row7 = make_row(w!("Toggle HDR/SDR"),
            IDC_HK_EDT_TOGGLE_HDR,    IDC_HK_CLR_TOGGLE_HDR,    "ToggleHDR");

        let h_sep_bottom = cb.static_text(w!(""), SS_BLACKRECT);

        let group = ControlGroup::new(vec![
            h_lbl_title, h_lbl_desc,
            h_lbl_sect_crush, h_sep_sect0,
            row0.h_lbl, row0.h_edit, row0.h_clear,
            row1.h_lbl, row1.h_edit, row1.h_clear,
            row2.h_lbl, row2.h_edit, row2.h_clear,
            row3.h_lbl, row3.h_edit, row3.h_clear,
            h_lbl_sect_dimmer, h_sep_sect1,
            row4.h_lbl, row4.h_edit, row4.h_clear,
            h_lbl_sect_system, h_sep_sect2,
            row7.h_lbl, row7.h_edit, row7.h_clear,
            h_sep_bottom,
        ]);

        Self {
            h_lbl_title, h_lbl_desc,
            h_lbl_sect_dimmer, h_lbl_sect_crush, h_lbl_sect_system,
            h_sep_sect: [h_sep_sect0, h_sep_sect1, h_sep_sect2],
            rows: [row0, row1, row2, row3, row4, row7],
            h_sep_bottom,
            group,
        }
    }

    // ── Persistence ───────────────────────────────────────────────────────────

    pub unsafe fn save(&self, ini: &mut ProfileManager) {
        for row in &self.rows {
            let mut buf = [0u16; 256];
            let len = GetWindowTextW(row.h_edit, &mut buf) as usize;
            let s   = String::from_utf16_lossy(&buf[..len]);
            ini.write("Hotkeys", row.ini_key, &s);
        }
    }

    pub unsafe fn clear_row_by_id(&self, id: usize) {
        for row in &self.rows {
            if row.clr_id == id {
                let ws: Vec<u16> = "None".encode_utf16().chain([0]).collect();
                SetWindowTextW(row.h_edit, PCWSTR(ws.as_ptr()));
                InvalidateRect(row.h_edit, None, false);
                // Auto-save immediately, consistent with key-capture behaviour.
                notify_parent_changed(row.h_edit);
                break;
            }
        }
    }

    /// Returns true if `hwnd` is one of the hotkey pill buttons.
    pub fn is_pill(&self, hwnd: HWND) -> bool {
        self.rows.iter().any(|r| r.h_edit == hwnd)
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    unsafe fn make_pill_button(cb: &ControlBuilder, id: usize, initial: &str) -> HWND {
        // Owner-drawn button — WM_DRAWITEM paints it; text set below.
        let h = cb.button(PCWSTR(std::ptr::null()), id);
        let ws: Vec<u16> = initial.encode_utf16().chain([0]).collect();
        SetWindowTextW(h, PCWSTR(ws.as_ptr()));
        // Install key-capture subclass.
        let proc_ptr: isize = mem::transmute(
            hotkey_pill_subclass_proc
                as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
        );
        let orig = SetWindowLongPtrW(h, GWLP_WNDPROC, proc_ptr);
        SetPropW(h, PROP_ORIG_PROC, HANDLE(orig as *mut _));
        h
    }
}