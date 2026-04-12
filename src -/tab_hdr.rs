// tab_hdr.rs — HDR tab (tab index 2).
//
// Contains: title label, Enable HDR toggle pill, and SDR Content Brightness
// slider (mirrors the Windows HDR display settings page).

#![allow(non_snake_case, dead_code)]

use std::ptr;

use windows::{
    core::*,
    Win32::{
        Foundation::{HWND, WPARAM, LPARAM},
        Graphics::Gdi::HFONT,
        UI::{
            Controls::*,
            WindowsAndMessaging::*,
        },
    },
};

use crate::{
    constants::{SS_NOPREFIX, SS_BLACKRECT, SS_CENTERIMAGE},
    controls::ControlBuilder,
    ui_drawing::{make_font_cached, slider_subclass_proc, SetWindowSubclass},
    win32::ControlGroup,
};

/// Control ID for the SDR brightness trackbar.
pub const IDC_SLD_SDR_BRIGHTNESS: usize = 151;

pub struct HdrTab {
    // ── Title ─────────────────────────────────────────────────────────────────
    pub h_lbl_title: HWND,

    // ── Toggle pill (Enable HDR) ───────────────────────────────────────────────
    /// The pill-style toggle button (owner-drawn).  Kept from the dimmer tab.
    pub h_btn_toggle: HWND,
    /// Whether HDR is currently enabled.
    pub enabled: bool,

    // ── SDR Content Brightness section ────────────────────────────────────────
    pub h_lbl_sdr_sect:       HWND,
    pub h_sep_sdr_sect:       HWND,
    pub h_sld_sdr_brightness: HWND,
    pub h_lbl_sdr_val:        HWND,
    /// Current SDR brightness value (0–100).
    pub sdr_brightness: i32,

    /// All controls in this tab — used by show_tab to batch-show/hide.
    pub group: ControlGroup,
}

impl HdrTab {
    pub unsafe fn new(
        parent:      HWND,
        hinstance:   windows::Win32::Foundation::HINSTANCE,
        dpi:         u32,
        font_normal: HFONT,
        font_title:  HFONT,
    ) -> Self {
        let cb_normal = ControlBuilder { parent, hinstance, dpi, font: font_normal };
        let cb_title  = ControlBuilder { parent, hinstance, dpi, font: font_title  };

        // ── Title ─────────────────────────────────────────────────────────────
        let h_lbl_title = cb_title.static_text(w!("HDR"), SS_NOPREFIX);

        // ── Enable HDR toggle ─────────────────────────────────────────────────
        // Re-uses IDC_BTN_HDR_TOGGLE (150) so the existing on_command handler
        // fires toggle_hdr_via_shortcut() without any extra wiring.
        const IDC_BTN_HDR_TOGGLE: usize = 150;
        let h_btn_toggle = cb_normal.button(w!("Enable HDR"), IDC_BTN_HDR_TOGGLE);

        // ── Section heading (11pt bold — matches dimmer / crush tabs) ─────────
        let font_sect = make_font_cached(w!("Segoe UI"), 11, dpi, true);
        let h_lbl_sdr_sect = CreateWindowExW(
            WS_EX_LEFT, w!("STATIC"), w!("SDR Content Brightness"),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_NOPREFIX),
            0, 0, 1, 1, parent, HMENU(ptr::null_mut()), hinstance, None,
        ).unwrap_or_default();
        SendMessageW(h_lbl_sdr_sect, WM_SETFONT, WPARAM(font_sect.0 as usize), LPARAM(1));

        let h_sep_sdr_sect = CreateWindowExW(
            WS_EX_LEFT, w!("STATIC"), w!(""),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_BLACKRECT),
            0, 0, 1, 1, parent, HMENU(ptr::null_mut()), hinstance, None,
        ).unwrap_or_default();

        // ── Slider (0–100, initial 50) ────────────────────────────────────────
        let h_sld_sdr_brightness = CreateWindowExW(
            WS_EX_LEFT,
            w!("msctls_trackbar32"),
            w!(""),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP
                | WINDOW_STYLE((TBS_HORZ | TBS_NOTICKS | TBS_FIXEDLENGTH) as u32),
            0, 0, 1, 1, parent,
            HMENU(IDC_SLD_SDR_BRIGHTNESS as *mut _),
            hinstance, None,
        ).unwrap_or_default();
        SendMessageW(h_sld_sdr_brightness, TBM_SETRANGE, WPARAM(0), LPARAM((100 << 16) | 0));
        SendMessageW(h_sld_sdr_brightness, TBM_SETPOS,   WPARAM(1), LPARAM(50));
        let thumb_px = (8u32 * dpi / 96).max(4);
        SendMessageW(h_sld_sdr_brightness, TBM_SETTHUMBLENGTH, WPARAM(thumb_px as usize), LPARAM(0));
        let _ = SetWindowSubclass(h_sld_sdr_brightness, Some(slider_subclass_proc), 1, 0);

        // ── Value label (bold, right of slider — matches h_lbl_dim_pct) ───────
        let font_bold_val = make_font_cached(w!("Segoe UI"), 10, dpi, true);
        let h_lbl_sdr_val = CreateWindowExW(
            WS_EX_LEFT, w!("STATIC"), w!("50"),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_CENTERIMAGE),
            0, 0, 1, 1, parent, HMENU(ptr::null_mut()), hinstance, None,
        ).unwrap_or_default();
        SendMessageW(h_lbl_sdr_val, WM_SETFONT, WPARAM(font_bold_val.0 as usize), LPARAM(1));

        let group = ControlGroup::new(vec![
            h_lbl_title,
            h_btn_toggle,
            h_lbl_sdr_sect,
            h_sep_sdr_sect,
            h_sld_sdr_brightness,
            h_lbl_sdr_val,
        ]);

        Self {
            h_lbl_title,
            h_btn_toggle,
            enabled: false,
            h_lbl_sdr_sect,
            h_sep_sdr_sect,
            h_sld_sdr_brightness,
            h_lbl_sdr_val,
            sdr_brightness: 50,
            group,
        }
    }
}