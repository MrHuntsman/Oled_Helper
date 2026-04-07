// tab_hotkeys.rs — Hotkeys settings tab.
//
// Each hotkey row consists of:
//   • A label describing the action
//   • An owner-drawn "pill" button that captures key combos on focus
//   • A small "×" clear button to reset a single binding
//
// The pill button is BS_OWNERDRAW. When focused it shows an accent-coloured
// border and intercepts WM_KEYDOWN/WM_SYSKEYDOWN via a subclass proc to
// record key combos. WM_DRAWITEM in app.rs calls draw_hotkey_pill() here.
//
// Auto-save: every key-capture and every clear fires EN_CHANGE-style
// notification so app.rs can persist + re-register immediately.

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
    ui_drawing::get_accent_color,
    win32::ControlGroup,
};

// Re-export so app.rs can call it from WM_DRAWITEM.
pub use self::draw::draw_hotkey_pill;

// ── Mouse-button sentinel codes ────────────────────────────────────────────────
//
// Synthetic VK-like values stored in the pill text and INI.
// Live above the real VK range (0x00–0xFF) so they never collide.
// Left/right click and mousewheel are intentionally excluded.
//
// Windows raw-input and WH_MOUSE_LL report extra buttons as XBUTTON events
// whose HIWORD(mouseData) is a 16-bit button-index bitmask.  Mice with many
// programmable buttons (e.g. Razer Naga, Logitech G600) can report indices
// 1–16.  We reserve one sentinel per index so every button gets a stable
// round-trip through the INI.
pub const MB_MIDDLE:    u32 = 0x10000; // Middle click (WM_MBUTTONDOWN)
// XBUTTON indices 1–16 (HIWORD of mouseData in WM_XBUTTONDOWN / MSLLHOOKSTRUCT)
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
/// Returns None for index 0 or anything above 16.
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

// ── Key-name table ─────────────────────────────────────────────────────────────

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
        // Mouse button sentinels (not real VKs)
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
    // Mouse button sentinels carry no keyboard modifiers.
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

// ── Property names ─────────────────────────────────────────────────────────────

const PROP_ORIG_PROC: PCWSTR = w!("BCT_HkOrigProc");
/// Stored as 1 when the pill button has keyboard focus.
pub const PROP_HK_FOCUSED: PCWSTR = w!("BCT_HkFocused");
/// Stored as 1 while the user is "recording" (focus + waiting for a key).
pub const PROP_HK_RECORDING: PCWSTR = w!("BCT_HkRecording");
/// Heap-allocated UTF-16 string snapshot of the text at focus-gain, used to
/// restore on cancel (click-away without pressing a key).
const PROP_HK_SAVED_TEXT: PCWSTR = w!("BCT_HkSavedText");

// ── Pill-button subclass proc ──────────────────────────────────────────────────
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
            // Free any pre-existing snapshot to avoid a memory leak on unexpected
            // re-focus (e.g. SetFocus called twice without an intervening KillFocus).
            let old_ptr = GetPropW(hwnd, PROP_HK_SAVED_TEXT).0 as *mut Vec<u16>;
            if !old_ptr.is_null() {
                drop(Box::from_raw(old_ptr));
            }
            // Snapshot current text so we can restore it if the user clicks away.
            let mut buf = [0u16; 256];
            let len = GetWindowTextW(hwnd, &mut buf) as usize;
            let snapshot: Box<Vec<u16>> = Box::new(buf[..len+1].to_vec()); // includes NUL
            SetPropW(hwnd, PROP_HK_SAVED_TEXT, HANDLE(Box::into_raw(snapshot) as *mut _));
            // Capture the mouse so we receive WM_LBUTTONDOWN even when the user
            // clicks on non-focusable areas (background, static labels, separators).
            // Without capture, clicks on those targets move no focus and the pill
            // stays stuck in recording mode indefinitely.
            SetCapture(hwnd);
            InvalidateRect(hwnd, None, false);
            call_orig()
        }

        WM_KILLFOCUS => {
            // If still recording (no key was confirmed), restore the snapshot.
            let was_recording = !GetPropW(hwnd, PROP_HK_RECORDING).0.is_null();
            let saved_ptr = GetPropW(hwnd, PROP_HK_SAVED_TEXT).0 as *mut Vec<u16>;
            if was_recording && !saved_ptr.is_null() {
                let saved = &*saved_ptr;
                SetWindowTextW(hwnd, PCWSTR(saved.as_ptr()));
            }
            // Free the snapshot regardless.
            if !saved_ptr.is_null() {
                drop(Box::from_raw(saved_ptr));
                RemovePropW(hwnd, PROP_HK_SAVED_TEXT);
            }
            RemovePropW(hwnd, PROP_HK_FOCUSED);
            RemovePropW(hwnd, PROP_HK_RECORDING);
            // Release capture if we still own it (focus may have moved via keyboard
            // or programmatically rather than through our WM_LBUTTONDOWN handler).
            if GetCapture() == hwnd {
                ReleaseCapture();
            }
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
                // Release capture then move focus so WM_KILLFOCUS fires cleanly.
                if GetCapture() == hwnd { ReleaseCapture(); }
                let parent = GetParent(hwnd).unwrap_or_default();
                SetFocus(parent);
                return LRESULT(0);
            }
            if let Some(s) = format_hotkey(vk) {
                // Before committing, clear the same binding on any sibling pill
                // so each hotkey can only be assigned to one action at a time.
                clear_duplicate_siblings(hwnd, &s);
                let ws: Vec<u16> = s.encode_utf16().chain([0]).collect();
                SetWindowTextW(hwnd, PCWSTR(ws.as_ptr()));
                RemovePropW(hwnd, PROP_HK_RECORDING);
                InvalidateRect(hwnd, None, false);
                notify_parent_changed(hwnd);
                // Release capture then move focus so WM_KILLFOCUS fires cleanly.
                if GetCapture() == hwnd { ReleaseCapture(); }
                let parent = GetParent(hwnd).unwrap_or_default();
                SetFocus(parent);
            }
            LRESULT(0)
        }

        WM_CHAR | WM_SYSCHAR | WM_DEADCHAR | WM_SYSDEADCHAR => LRESULT(0),

        // Middle-click and extra buttons (Mouse4…Mouse19) can be bound while recording.
        // Left/right click and wheel are intentionally excluded.
        WM_MBUTTONDOWN | WM_XBUTTONDOWN => {
            if GetPropW(hwnd, PROP_HK_RECORDING).0.is_null() {
                return call_orig();
            }
            let vk = if msg == WM_MBUTTONDOWN {
                MB_MIDDLE
            } else {
                // XBUTTON id is in the high word of wp (1-based index)
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

        // While recording we own mouse capture (set in WM_SETFOCUS), so every
        // WM_LBUTTONDOWN in the whole window arrives here.  If the click is
        // outside the pill, cancel recording.  We release capture and move focus
        // to the parent; WM_KILLFOCUS restores the saved text.  The subsequent
        // WM_LBUTTONUP (still captured) is eaten harmlessly, and the user can
        // click again normally on whatever they want.
        WM_LBUTTONDOWN => {
            if !GetPropW(hwnd, PROP_HK_RECORDING).0.is_null() {
                let x = (lp.0 & 0xFFFF) as i16 as i32;
                let y = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
                let mut rc = RECT::default();
                GetClientRect(hwnd, &mut rc);
                let inside = x >= rc.left && x < rc.right && y >= rc.top && y < rc.bottom;
                if !inside {
                    // Release capture first so focus transfer works correctly,
                    // then move focus to parent — WM_KILLFOCUS will restore text.
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

        // WM_CAPTURECHANGED fires when something else steals capture (e.g. a
        // drag starts elsewhere).  Treat it the same as clicking away.
        WM_CAPTURECHANGED => {
            if !GetPropW(hwnd, PROP_HK_RECORDING).0.is_null() {
                // Don't call ReleaseCapture — we no longer own it.
                let parent = GetParent(hwnd).unwrap_or_default();
                SetFocus(parent);
            }
            call_orig()
        }

        // Left-click release → enter recording mode, but only when:
        //   • the release is over the button (not a drag-off), AND
        //   • the pill is not already recording.
        // If we are already recording, WM_LBUTTONDOWN already fired the cancel
        // path (ReleaseCapture + SetFocus(parent) + WM_KILLFOCUS), so by the
        // time WM_LBUTTONUP arrives here the pill is no longer focused.  Calling
        // SetFocus again would immediately re-enter recording mode.
        // call_orig() is always called so the default button proc repaints the
        // depressed state correctly.
        WM_LBUTTONUP => {
            let already_recording = !GetPropW(hwnd, PROP_HK_RECORDING).0.is_null();
            let x = (lp.0 & 0xFFFF) as i16 as i32;
            let y = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
            let mut rc = RECT::default();
            GetClientRect(hwnd, &mut rc);
            let over_button = x >= rc.left && x < rc.right && y >= rc.top && y < rc.bottom;
            if over_button && !already_recording {
                if GetFocus() != hwnd {
                    SetFocus(hwnd);
                }
                return LRESULT(0);
            }
            call_orig()
        }

        // Safety net: free snapshot and clear all props if window is destroyed
        // while still focused or recording.
        WM_NCDESTROY => {
            let saved_ptr = GetPropW(hwnd, PROP_HK_SAVED_TEXT).0 as *mut Vec<u16>;
            if !saved_ptr.is_null() {
                drop(Box::from_raw(saved_ptr));
                RemovePropW(hwnd, PROP_HK_SAVED_TEXT);
            }
            RemovePropW(hwnd, PROP_HK_FOCUSED);
            RemovePropW(hwnd, PROP_HK_RECORDING);
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

/// Walk all sibling pill buttons (identified by PROP_HK_RECORDING presence or
/// the BCT_HkOrigProc property) and reset any that already show `combo` to
/// "None", then notify the parent so the change is auto-saved.
unsafe fn clear_duplicate_siblings(hwnd: HWND, combo: &str) {
    let parent = match GetParent(hwnd) {
        Ok(p) => p,
        Err(_) => return,
    };
    // Enumerate direct children of the parent looking for sibling pills.
    // A pill is identified by having the PROP_ORIG_PROC property installed.
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

// ── Drawing ────────────────────────────────────────────────────────────────────

mod draw {
    use super::*;
    use windows::Win32::Graphics::Gdi::*;

    /// Paint one hotkey pill from WM_DRAWITEM.
    /// Called by app.rs's WM_DRAWITEM handler.
    pub unsafe fn draw_hotkey_pill(di: &DRAWITEMSTRUCT) {
        let hdc = di.hDC;
        let rc  = di.rcItem;

        let focused   = !GetPropW(di.hwndItem, PROP_HK_FOCUSED).0.is_null();
        let recording = !GetPropW(di.hwndItem, PROP_HK_RECORDING).0.is_null();
        let pressed   = di.itemState.0 & ODS_SELECTED.0 != 0;

        let dpi = GetDeviceCaps(hdc, LOGPIXELSX).max(96);
        let s   = |px: i32| (px * dpi / 96).max(1);

        let accent = get_accent_color();

        // ── Background ────────────────────────────────────────────────────────
        // Slightly lighter when pressed to give tactile feedback.
        let bg_color = if pressed { COLORREF(0x00323232) } else { COLORREF(0x002A2A2A) };
        let bg_br    = CreateSolidBrush(bg_color);
        FillRect(hdc, &rc, bg_br);
        DeleteObject(bg_br);

        // ── Rounded-rect border ───────────────────────────────────────────────
        // Always draw a subtle 1px grey border; the pill is compact, not
        // full-radius — use a small fixed corner radius (~6 dp) instead.
        let radius = s(6);
        let border_color = COLORREF(0x00484848);
        let pen    = CreatePen(PS_SOLID, 1, border_color);
        let old_p  = SelectObject(hdc, pen);
        let old_br = SelectObject(hdc, GetStockObject(NULL_BRUSH));
        RoundRect(hdc, rc.left, rc.top, rc.right - 1, rc.bottom - 1, radius, radius);
        SelectObject(hdc, old_p);
        SelectObject(hdc, old_br);
        DeleteObject(pen);

        // ── Accent underline (focused only) ───────────────────────────────────
        // Draw a 2dp accent-coloured bar along the bottom edge, inset from the
        // side corners so it sits cleanly inside the rounded rect.
        if focused {
            let bar_h    = s(2);
            let bar_inset = s(4);
            let bar_rc   = RECT {
                left:   rc.left  + bar_inset,
                top:    rc.bottom - bar_h,
                right:  rc.right - bar_inset,
                bottom: rc.bottom,
            };
            let accent_br = CreateSolidBrush(accent);
            FillRect(hdc, &bar_rc, accent_br);
            DeleteObject(accent_br);
        }

        // ── Text ──────────────────────────────────────────────────────────────
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
            // Bound key combo — always light grey; accent tint when focused
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
    /// Owner-drawn pill button (replaces the old edit control).
    pub h_edit:  HWND,
    pub h_clear: HWND,
    pub ini_key: &'static str,
    pub clr_id:  usize,
    /// Control ID of the pill (needed for EN_CHANGE matching in app.rs).
    #[allow(dead_code)]
    pub edit_id: usize,
}

// ── HotkeysTab ─────────────────────────────────────────────────────────────────

pub struct HotkeysTab {
    pub h_lbl_title:       HWND,
    pub h_lbl_desc:        HWND,
    pub h_lbl_sect_dimmer: HWND,
    pub h_lbl_sect_crush:  HWND,
    pub h_sep_sect: [HWND; 2],
    pub rows: [HotkeyRow; 7],
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

        // Section headings: 11pt bold — smaller than the 16pt tab title but
        // still distinct from the 10pt normal row labels.
        let font_sect = crate::ui_drawing::make_font_cached(w!("Segoe UI"), 11, dpi, true);
        SendMessageW(h_lbl_sect_dimmer, WM_SETFONT, WPARAM(font_sect.0 as usize), LPARAM(1));
        SendMessageW(h_lbl_sect_crush,  WM_SETFONT, WPARAM(font_sect.0 as usize), LPARAM(1));
        // font_sect is cached and reused across DPI changes.


        let make_row = |lbl_text: PCWSTR,
                        id_edit: usize, id_clear: usize,
                        ini_key: &'static str| -> HotkeyRow {
            let h_lbl   = cb.static_text(lbl_text, SS_NOPREFIX);
            let initial = ini.read(sec, ini_key, "None");
            let h_edit  = Self::make_pill_button(&cb, id_edit, &initial);
            let h_clear = cb.button(w!("×"), id_clear);
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
        let row5 = make_row(w!("Decrease Dim Level"),
            IDC_HK_EDT_DIM_DECREASE,  IDC_HK_CLR_DIM_DECREASE,  "DecreaseDimLevel");
        let row6 = make_row(w!("Increase Dim Level"),
            IDC_HK_EDT_DIM_INCREASE,  IDC_HK_CLR_DIM_INCREASE,  "IncreaseDimLevel");

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
            row5.h_lbl, row5.h_edit, row5.h_clear,
            row6.h_lbl, row6.h_edit, row6.h_clear,
            h_sep_bottom,
        ]);

        Self {
            h_lbl_title, h_lbl_desc,
            h_lbl_sect_dimmer, h_lbl_sect_crush,
            h_sep_sect: [h_sep_sect0, h_sep_sect1],
            rows: [row0, row1, row2, row3, row4, row5, row6],
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
                // Notify parent so the cleared binding is auto-saved immediately,
                // consistent with key-capture behaviour.
                notify_parent_changed(row.h_edit);
                break;
            }
        }
    }

    /// Return true if `hwnd` is one of the hotkey pill buttons.
    pub fn is_pill(&self, hwnd: HWND) -> bool {
        self.rows.iter().any(|r| r.h_edit == hwnd)
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    unsafe fn make_pill_button(cb: &ControlBuilder, id: usize, initial: &str) -> HWND {
        // Create as owner-drawn button so WM_DRAWITEM paints it.
        let h = cb.button(PCWSTR(std::ptr::null()), id); // text set below via SetWindowText

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