// controls.rs — Win32 child-window factory.
//
// `ControlBuilder` wraps per-window hinstance/font/dpi context and exposes
// typed constructors for each control class. All unsafe Win32 calls live here
// so that `create_state` in app.rs reads as plain configuration.

#![allow(unused_must_use)]
#![allow(non_snake_case)]

use std::ptr;

use windows::{
    core::{PCWSTR, w},
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, WPARAM},
        UI::{
            Controls::{TBM_SETPOS, TBM_SETRANGEMAX, TBM_SETRANGEMIN, TBM_SETTHUMBLENGTH,
                        TBS_FIXEDLENGTH, TBS_HORZ, TBS_NOTICKS},
            WindowsAndMessaging::*,
        },
    },
};

use crate::ui_drawing::{slider_subclass_proc, combo_subclass_proc, SetWindowSubclass};
use crate::constants::SS_LEFT;

// CBS_* constants live in WindowsAndMessaging in windows-rs 0.58, not UI::Controls.
const CBS_DROPDOWNLIST:   u32 = 0x0003;
const CBS_HASSTRINGS:     u32 = 0x0200;
const CBS_OWNERDRAWFIXED: u32 = 0x0010;

// ── Builder ───────────────────────────────────────────────────────────────────

/// Creation context shared by every child control.
pub struct ControlBuilder {
    pub parent:    HWND,
    pub hinstance: HINSTANCE,
    pub dpi:       u32,
    pub font:      windows::Win32::Graphics::Gdi::HFONT,
}

impl ControlBuilder {
    /// Static text label. `extra_style` is OR-ed onto `SS_LEFT | WS_CHILD | WS_VISIBLE`.
    pub unsafe fn static_text(&self, text: PCWSTR, extra_style: u32) -> HWND {
        let h = CreateWindowExW(
            WS_EX_TRANSPARENT,
            w!("STATIC"), text,
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_LEFT | extra_style),
            0, 0, 1, 1,
            self.parent, HMENU(ptr::null_mut()), self.hinstance, None,
        ).unwrap_or_default();
        self.set_font(h);
        h
    }

    /// Owner-drawn push button.
    pub unsafe fn button(&self, text: PCWSTR, id: usize) -> HWND {
        let h = CreateWindowExW(
            WS_EX_LEFT,
            w!("BUTTON"), text,
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_OWNERDRAW as u32),
            0, 0, 1, 1,
            self.parent, HMENU(id as *mut _), self.hinstance, None,
        ).unwrap_or_default();
        self.set_font(h);
        h
    }

    /// Owner-drawn checkbox. Toggle state is managed manually in `on_command`.
    #[inline]
    pub unsafe fn checkbox(&self, text: PCWSTR, id: usize) -> HWND {
        self.button(text, id)
    }

    /// Horizontal trackbar with custom subclass proc installed.
    pub unsafe fn slider(&self, id: usize, min: i32, max: i32, val: i32) -> HWND {
        let h = CreateWindowExW(
            WS_EX_LEFT,
            w!("msctls_trackbar32"), w!(""),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(TBS_HORZ | TBS_FIXEDLENGTH | TBS_NOTICKS),
            0, 0, 1, 1,
            self.parent, HMENU(id as *mut _), self.hinstance, None,
        ).unwrap_or_default();

        SendMessageW(h, TBM_SETRANGEMIN, WPARAM(0), LPARAM(min as isize));
        SendMessageW(h, TBM_SETRANGEMAX, WPARAM(0), LPARAM(max as isize));
        SendMessageW(h, TBM_SETPOS,      WPARAM(1), LPARAM(val as isize));

        let thumb_px = (8u32 * self.dpi / 96).max(4); // DPI-scaled thumb, min 4 px
        SendMessageW(h, TBM_SETTHUMBLENGTH, WPARAM(thumb_px as usize), LPARAM(0));

        SetWindowSubclass(h, Some(slider_subclass_proc), 1, 0); // subclass ID 1, unique per HWND
        h
    }

    /// Single-line edit for hotkey text input.
    #[allow(dead_code)]
    pub unsafe fn edit(&self, id: usize, text: &str) -> HWND {
        let wtext: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        let h = CreateWindowExW(
            WS_EX_CLIENTEDGE,
            w!("EDIT"), PCWSTR(wtext.as_ptr()),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE((ES_LEFT | ES_AUTOHSCROLL) as u32),
            0, 0, 1, 1,
            self.parent, HMENU(id as *mut _), self.hinstance, None,
        ).unwrap_or_default();
        self.set_font(h);
        h
    }

    /// CBS_DROPDOWNLIST combobox with custom scroll subclass installed.
    pub unsafe fn combobox(&self, id: usize) -> HWND {
        let h = CreateWindowExW(
            WS_EX_LEFT,
            w!("COMBOBOX"), w!(""),
            WS_CHILD | WS_VISIBLE | WS_VSCROLL
                | WINDOW_STYLE((CBS_DROPDOWNLIST | CBS_HASSTRINGS | CBS_OWNERDRAWFIXED) as u32),
            0, 0, 200, 200,
            self.parent, HMENU(id as *mut _), self.hinstance, None,
        ).unwrap_or_default();
        self.set_font(h);
        SetWindowSubclass(h, Some(combo_subclass_proc), 1, 0);
        h
    }

    /// Send `WM_SETFONT` with the builder's font.
    pub unsafe fn set_font(&self, hwnd: HWND) {
        SendMessageW(hwnd, WM_SETFONT, WPARAM(self.font.0 as usize), LPARAM(1));
    }
}