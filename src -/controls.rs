// controls.rs — Safe Win32 child-window factory.
//
// `ControlBuilder` wraps the per-window hinstance/font/dpi context and
// exposes typed constructors for each control class used by the app.
// All unsafe Win32 calls are confined here so that `create_state` in
// app.rs can read as straightforward configuration rather than raw FFI.

#![allow(unused_must_use)]
#![allow(non_snake_case)]

use std::ptr;

use windows::{
    core::{PCWSTR, w},
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, WPARAM},
        UI::{
            Controls::{TBM_SETPOS, TBM_SETRANGE, TBM_SETTHUMBLENGTH,
                        TBS_FIXEDLENGTH, TBS_HORZ, TBS_NOTICKS},
            WindowsAndMessaging::*,
        },
    },
};

use crate::ui_drawing::{slider_subclass_proc, combo_subclass_proc, SetWindowSubclass};
use crate::constants::SS_LEFT;

// CBS_DROPDOWNLIST / CBS_HASSTRINGS / CBS_OWNERDRAWFIXED live in WindowsAndMessaging
// in windows-rs 0.58, not in UI::Controls. Define them locally.
const CBS_DROPDOWNLIST:   u32 = 0x0003;
const CBS_HASSTRINGS:     u32 = 0x0200;
const CBS_OWNERDRAWFIXED: u32 = 0x0010;

// ── Builder ───────────────────────────────────────────────────────────────────

/// Holds creation context that is identical for every child control.
pub struct ControlBuilder {
    pub parent:    HWND,
    pub hinstance: HINSTANCE,
    pub dpi:       u32,
    pub font:      windows::Win32::Graphics::Gdi::HFONT,
}

impl ControlBuilder {
    /// Create a static text label.
    ///
    /// `extra_style` is OR-ed onto `SS_LEFT | WS_CHILD | WS_VISIBLE | WS_EX_TRANSPARENT`.
    pub unsafe fn static_text(&self, text: PCWSTR, extra_style: u32) -> HWND {
        let h = CreateWindowExW(
            WS_EX_TRANSPARENT,
            w!("STATIC"), text,
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_LEFT | extra_style),
            0, 0, 1, 1,
            self.parent,
            HMENU(ptr::null_mut()),
            self.hinstance,
            None,
        ).unwrap_or_default();
        self.set_font(h);
        h
    }

    /// Create an owner-drawn push button.
    pub unsafe fn button(&self, text: PCWSTR, id: usize) -> HWND {
        let h = CreateWindowExW(
            WS_EX_LEFT,
            w!("BUTTON"), text,
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(BS_OWNERDRAW as u32),
            0, 0, 1, 1,
            self.parent,
            HMENU(id as *mut _),
            self.hinstance,
            None,
        ).unwrap_or_default();
        self.set_font(h);
        h
    }

    /// Create an owner-drawn checkbox (visually identical to a button — the
    /// app handles the toggle state manually in `on_command`).
    #[inline]
    pub unsafe fn checkbox(&self, text: PCWSTR, id: usize) -> HWND {
        // Owner-drawn checkboxes share the same Win32 control class as buttons.
        self.button(text, id)
    }

    /// Create a horizontal trackbar (slider) and install the custom subclass proc.
    pub unsafe fn slider(&self, id: usize, min: i32, max: i32, val: i32) -> HWND {
        let h = CreateWindowExW(
            WS_EX_LEFT,
            w!("msctls_trackbar32"),
            w!(""),
            WS_CHILD | WS_VISIBLE
                | WINDOW_STYLE(TBS_HORZ | TBS_FIXEDLENGTH | TBS_NOTICKS),
            0, 0, 1, 1,
            self.parent,
            HMENU(id as *mut _),
            self.hinstance,
            None,
        ).unwrap_or_default();

        SendMessageW(h, TBM_SETRANGE, WPARAM(0), LPARAM(((max << 16) | min) as isize));
        SendMessageW(h, TBM_SETPOS,   WPARAM(1), LPARAM(val as isize));

        // Scale thumb size with DPI (minimum 4 px).
        let thumb_px = (8u32 * self.dpi / 96).max(4);
        SendMessageW(h, TBM_SETTHUMBLENGTH, WPARAM(thumb_px as usize), LPARAM(0));

        // Install custom paint subclass via the safe comctl32 API.
        // The subclass ID (1) is arbitrary but must be unique per-HWND per-proc.
        SetWindowSubclass(h, Some(slider_subclass_proc), 1, 0);

        h
    }

    /// Create a single-line edit control for hotkey text input.
    #[allow(dead_code)]
    pub unsafe fn edit(&self, id: usize, text: &str) -> HWND {
        let wtext: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        let h = CreateWindowExW(
            WS_EX_CLIENTEDGE,
            w!("EDIT"), PCWSTR(wtext.as_ptr()),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP
                | WINDOW_STYLE((ES_LEFT | ES_AUTOHSCROLL) as u32),
            0, 0, 1, 1,
            self.parent,
            HMENU(id as *mut _),
            self.hinstance,
            None,
        ).unwrap_or_default();
        self.set_font(h);
        h
    }

    /// Create a CBS_DROPDOWNLIST combobox and install the custom subclass proc.
    pub unsafe fn combobox(&self, id: usize) -> HWND {
        let h = CreateWindowExW(
            WS_EX_LEFT,
            w!("COMBOBOX"),
            w!(""),
            WS_CHILD | WS_VISIBLE | WS_VSCROLL
                | WINDOW_STYLE((CBS_DROPDOWNLIST | CBS_HASSTRINGS | CBS_OWNERDRAWFIXED) as u32),
            0, 0, 200, 200,
            self.parent,
            HMENU(id as *mut _),
            self.hinstance,
            None,
        ).unwrap_or_default();
        self.set_font(h);

        // Install custom scroll subclass via the safe comctl32 API.
        SetWindowSubclass(h, Some(combo_subclass_proc), 1, 0);

        h
    }

    /// Send `WM_SETFONT` to `hwnd` using the builder's normal font.
    pub unsafe fn set_font(&self, hwnd: HWND) {
        SendMessageW(
            hwnd,
            WM_SETFONT,
            WPARAM(self.font.0 as usize),
            LPARAM(1),
        );
    }
}