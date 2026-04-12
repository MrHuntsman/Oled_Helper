// tab_crush.rs — Black Crush logic, profiles, and UI state.

#![allow(non_snake_case, unused_variables, unused_mut, unused_assignments,
         unused_must_use)]

use std::mem;

use windows::{
    core::*,
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::{
            LibraryLoader::GetModuleHandleW,
            Registry::*,
        },
        UI::{
            Controls::*,
            HiDpi::GetDpiForWindow,
            WindowsAndMessaging::*,
        },
        Devices::Display::*,
    },
};

use crate::{
    constants::*,
    controls::ControlBuilder,
    gamma_ramp,
    hdr_panel::HdrPanel,
    profile_manager::ProfileManager,
    ui_drawing::{
        combo_selected_text,
        get_slider_val, make_font,
        set_window_text,
        slider_subclass_proc, compare_btn_subclass_proc, combo_subclass_proc,
        SetWindowSubclass,
    },
    win32::{set_text, set_text_fmt, ControlGroup},
};

// ── Tab state ─────────────────────────────────────────────────────────────────

/// All state and control handles owned by the Black Crush Tweak tab (tab 0).
pub struct CrushTab {
    // ── Controls ──────────────────────────────────────────────────────────────
    pub h_lbl_title:     HWND,
    pub h_lbl_sub1:      HWND,
    pub h_lbl_sub2:      HWND,
    pub h_lbl_bl_sect:   HWND,
    pub h_sld_black:     HWND,
    pub h_lbl_black_val: HWND,
    pub h_lbl_sl_hint:   HWND,
    pub h_lbl_gamma_warn:HWND,
    pub h_lbl_hdr_sect:  HWND,
    pub h_sld_squares:   HWND,
    pub h_lbl_range_val: HWND,
    pub h_lbl_hdr_note:  HWND,
    pub h_lbl_zoom_out_icon: HWND, // zoom_out icon left of the squares slider
    pub h_lbl_zoom_icon: HWND,    // zoom icon right of the squares slider
    pub h_hdr_panel:     HWND,
    pub h_btn_toggle:    HWND,
    pub h_lbl_ref_sect:  HWND,
    pub h_lbl_hz_profile: HWND,
    pub h_lbl_hz_icon:   HWND,   // ℹ info glyph, right of the dropdown
    pub h_ddl_refresh:   HWND,

    // ── Group for atomic show/hide ────────────────────────────────────────────
    /// Every control exclusive to this tab — toggled as a unit when switching tabs.
    pub group: ControlGroup,

    // ── Runtime state ─────────────────────────────────────────────────────────
    pub hdr_panel:         Box<HdrPanel>,
    pub previewing:        bool,
    pub suppress_load:     bool,
    pub btn_toggle_active: bool,
    pub hdr_note_color:    COLORREF,
}

impl CrushTab {
    /// Construct all controls and load persisted state from `ini`.
    ///
    /// # Safety
    /// Must be called on the same thread that owns `parent`.
    pub unsafe fn new(
        parent: HWND,
        hinstance: HINSTANCE,
        dpi: u32,
        font_normal: HFONT,
        font_title:  HFONT,
        font_bold:   HFONT,
        ini: &mut ProfileManager,
        h_sep_h: &[HWND],         // separators owned by AppState, included in this tab's group
    ) -> Self {
        let cb = ControlBuilder { parent, hinstance, dpi, font: font_normal };

        let h_lbl_title      = cb.static_text(w!("Black Crush Tweak"), 0);
        let h_lbl_sub1       = cb.static_text(
            w!("Adjust black levels using a Reinhard gamma curve. Preserves pure black."), 0);
        let h_lbl_sub2       = cb.static_text(w!("You can use a different value for each refresh rate."), 0);
        let h_lbl_bl_sect    = cb.static_text(w!("Black Level"), SS_NOPREFIX);
        let h_sld_black      = cb.slider(IDC_SLD_BLACK, 0, MAX_BLACK, DEFAULT_BLACK);
        let h_lbl_black_val  = cb.static_text(w!("OFF"), SS_CENTERIMAGE);
        let h_lbl_sl_hint    = cb.static_text(w!(""), 0);
        let h_lbl_gamma_warn = cb.static_text(w!(""), 0);
        let h_lbl_hdr_sect   = cb.static_text(w!("HDR-Black Calibration"), SS_NOPREFIX);
        let h_sld_squares    = cb.slider(IDC_SLD_SQUARES, 9, 24, 9);
        let h_lbl_range_val  = cb.static_text(w!(""), SS_CENTERIMAGE);
        let h_lbl_hdr_note   = cb.static_text(
            w!("⚠  HDR colour space not detected — enable Windows HDR for accurate results"),
            SS_NOPREFIX);

        let h_lbl_zoom_out_icon = cb.static_text(w!(""), SS_NOPREFIX);
        let h_lbl_zoom_icon = cb.static_text(w!(""), SS_NOPREFIX);

        let h_hdr_panel = CreateWindowExW(
            WS_EX_LEFT, w!("STATIC"), w!(""),
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_BLACKRECT),
            0, 0, 1, 1, parent, HMENU(IDC_HDR_PANEL as *mut _), hinstance, None,
        ).unwrap_or_default();

        let h_btn_toggle = cb.button(w!("Click-hold to compare"), IDC_BTN_TOGGLE);
        {
            SetWindowSubclass(h_btn_toggle, Some(compare_btn_subclass_proc), 1, 0);
        }

        let h_lbl_ref_sect = cb.static_text(w!("Refresh Rate"), SS_NOPREFIX);
        let h_lbl_hz_profile = cb.static_text(w!(""), SS_NOPREFIX);
        let h_lbl_hz_icon  = cb.static_text(w!(""), SS_NOPREFIX);
        let h_ddl_refresh  = cb.combobox(IDC_DDL_REFRESH);
        {
            SetWindowSubclass(h_ddl_refresh, Some(combo_subclass_proc), 1, 0);
            // CBS_OWNERDRAWFIXED item height must be set explicitly for dynamically
            // created comboboxes — WM_MEASUREITEM is not reliably sent to the parent
            // for non-dialog windows.  Use the same s(26) formula as the layout so
            // every list row matches the closed-face height exactly.
            // Index -1 sets the height of the selection field; index 0 sets all list items.
            let item_h = (20 * dpi / 96) as isize;
            SendMessageW(h_ddl_refresh, CB_SETITEMHEIGHT, WPARAM(usize::MAX), LPARAM(item_h));
            SendMessageW(h_ddl_refresh, CB_SETITEMHEIGHT, WPARAM(0),          LPARAM(item_h));
        }

        SendMessageW(h_lbl_title,     WM_SETFONT, WPARAM(font_title.0 as usize), LPARAM(1));
        SendMessageW(h_lbl_black_val, WM_SETFONT, WPARAM(font_bold.0  as usize), LPARAM(1));

        // Section headings: 11pt bold — matches the style used in the hotkeys tab.
        let font_sect = crate::ui_drawing::make_font_cached(w!("Segoe UI"), 11, dpi, true);
        SendMessageW(h_lbl_bl_sect,  WM_SETFONT, WPARAM(font_sect.0 as usize), LPARAM(1));
        SendMessageW(h_lbl_hdr_sect, WM_SETFONT, WPARAM(font_sect.0 as usize), LPARAM(1));
        SendMessageW(h_lbl_ref_sect, WM_SETFONT, WPARAM(font_sect.0 as usize), LPARAM(1));
        // font_sect is cached and reused across DPI changes.

        // Build the group — separators h_sep_h[0..=2] are shared with AppState
        // but logically belong to this tab's visibility.
        let mut group_handles = vec![
            h_lbl_title, h_lbl_sub1, h_lbl_sub2,
            h_lbl_bl_sect, h_sld_black, h_lbl_black_val,
            h_lbl_sl_hint, h_lbl_gamma_warn,
            h_lbl_hdr_sect, h_sld_squares, h_lbl_range_val,
            h_hdr_panel, h_lbl_hdr_note, h_lbl_zoom_out_icon, h_lbl_zoom_icon,
            h_btn_toggle,
            h_lbl_ref_sect, h_lbl_hz_profile, h_lbl_hz_icon, h_ddl_refresh,
        ];
        group_handles.extend_from_slice(&h_sep_h[0..4]);
        let group = ControlGroup::new(group_handles);

        let s = Self {
            h_lbl_title, h_lbl_sub1, h_lbl_sub2,
            h_lbl_bl_sect, h_sld_black, h_lbl_black_val,
            h_lbl_sl_hint, h_lbl_gamma_warn,
            h_lbl_hdr_sect, h_sld_squares, h_lbl_range_val,
            h_lbl_hdr_note, h_lbl_zoom_out_icon, h_lbl_zoom_icon, h_hdr_panel,
            h_btn_toggle,
            h_lbl_ref_sect, h_lbl_hz_profile, h_lbl_hz_icon, h_ddl_refresh,
            group,
            hdr_panel: Box::default(),
            previewing: false,
            suppress_load: false,
            btn_toggle_active: false,
            hdr_note_color: COLORREF(0x00888888),
        };
        s.update_sl_hint();
        s
    }

    // ── HDR labels ────────────────────────────────────────────────────────────

    pub unsafe fn update_range_label(&self) {
        // No-op: label removed in favor of icons.
        let _ = get_slider_val(self.h_sld_squares);
    }

    pub unsafe fn update_hdr_sect_label(&self) {
        let hdr = self.hdr_panel.hdr_active;
        set_window_text(self.h_lbl_hdr_sect,
            if hdr { "HDR Black Calibration (PQ)" }
            else   { "SDR Black Calibration (RGB)" });
    }

    pub unsafe fn update_sl_hint(&self) {
        let hdr = self.hdr_panel.hdr_active;
        self.update_hdr_sect_label();
        set_window_text(self.h_lbl_sl_hint,
            if hdr {
                "Raise black level until you can almost barely start to see the PQ 68 column."
            } else {
                "Raise black level until you can almost barely start to see the RGB 1 column."
            });
    }

    // ── Sliders ───────────────────────────────────────────────────────────────

    /// Real-time visual updates during black-level drag.
    pub unsafe fn on_black_slider_visual(&mut self) {
        let v = get_slider_val(self.h_sld_black);
        let text = if v == 0 { "OFF".to_string() } else { format!("{v}") };
        set_text(self.h_lbl_black_val, &text);
        self.hdr_panel.update(v);
        let hz = get_current_hz();
        self.refresh_dropdown_label(hz as u32, v);
    }

    /// Commits black-level changes to ramp and INI.
    pub unsafe fn on_black_slider_changed(&mut self, ini: &mut ProfileManager) {
        let v = get_slider_val(self.h_sld_black);
        let text = if v == 0 { "OFF".to_string() } else { format!("{v}") };
        set_text(self.h_lbl_black_val, &text);
        self.apply_ramp_internal(v);
        self.hdr_panel.update(v);
        let hz = get_current_hz();
        ini.write_int(&hz_section(hz), "Black", v);
        self.refresh_dropdown_label(hz as u32, v);
    }

    /// Updates Hz bracket label in dropdown without flicker.
    pub unsafe fn refresh_dropdown_label(&self, hz: u32, v: i32) {
        let count = SendMessageW(self.h_ddl_refresh, CB_GETCOUNT, WPARAM(0), LPARAM(0)).0 as i32;
        for i in 0..count {
            let mut buf = [0u16; 64];
            let len = SendMessageW(self.h_ddl_refresh, CB_GETLBTEXT,
                WPARAM(i as usize), LPARAM(buf.as_mut_ptr() as isize)).0 as usize;
            if len == 0 { continue; }

            // Parse the Hz value from the start of the item string.
            let end = buf[..len].iter().position(|&c| c == b' ' as u16).unwrap_or(len);
            let item_hz: u32 = String::from_utf16_lossy(&buf[..end])
                .parse().unwrap_or(0);
            if item_hz != hz { continue; }

            let cur_sel = SendMessageW(self.h_ddl_refresh, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 as i32;
            let new_label = if v > 0 {
                format!("{hz} Hz [{v}]\0")
            } else {
                format!("{hz} Hz\0")
            };
            let new_w: Vec<u16> = new_label.encode_utf16().collect();
            SendMessageW(self.h_ddl_refresh, WM_SETREDRAW, WPARAM(0), LPARAM(0));
            SendMessageW(self.h_ddl_refresh, CB_DELETESTRING, WPARAM(i as usize), LPARAM(0));
            SendMessageW(self.h_ddl_refresh, CB_INSERTSTRING,
                WPARAM(i as usize), LPARAM(new_w.as_ptr() as isize));
            SendMessageW(self.h_ddl_refresh, CB_SETCURSEL, WPARAM(cur_sel as usize), LPARAM(0));
            SendMessageW(self.h_ddl_refresh, WM_SETREDRAW, WPARAM(1), LPARAM(0));
            InvalidateRect(self.h_ddl_refresh, None, false);
            break;
        }
    }

    pub unsafe fn on_squares_changed(&mut self) {
        let v = get_slider_val(self.h_sld_squares) as usize;
        self.hdr_panel.set_square_count(v);
        self.update_range_label();
    }

    // ── Gamma ramp ────────────────────────────────────────────────────────────

    /// Applies current black-level ramp.
    pub unsafe fn apply_ramp(&self) -> (bool, i32) {
        let v    = get_slider_val(self.h_sld_black);
        let ramp = gamma_ramp::build_ramp(v);
        let null = HWND(std::ptr::null_mut());
        let hdc  = GetDC(null);
        let ok = if hdc.0.is_null() { 0 } else {
            let r = gamma_ramp::SetDeviceGammaRamp(hdc, &ramp);
            ReleaseDC(null, hdc);
            r
        };
        (ok != 0, v)
    }

    /// Applies neutral ramp for comparison.
    pub unsafe fn apply_linear_ramp(&self) -> bool {
        let null = HWND(std::ptr::null_mut());
        let ramp = gamma_ramp::build_linear_ramp();
        let hdc  = GetDC(null);
        if hdc.0.is_null() { return false; }
        let r = gamma_ramp::SetDeviceGammaRamp(hdc, &ramp);
        ReleaseDC(null, hdc);
        r != 0
    }

    /// Build the ramp that should currently be active for the current mode.
    pub unsafe fn desired_ramp(&self) -> gamma_ramp::GammaRamp {
        if self.previewing {
            gamma_ramp::build_linear_ramp()
        } else {
            let v = get_slider_val(self.h_sld_black);
            gamma_ramp::build_ramp(v)
        }
    }

    /// Restore the desired ramp if the system ramp has drifted.
    pub unsafe fn maybe_restore_desired_ramp(&self) -> bool {
        if let Some(actual) = gamma_ramp::get_display_ramp() {
            let desired = self.desired_ramp();
            if actual == desired {
                return true;
            }
        } else {
            return false;
        }

        if self.previewing {
            self.apply_linear_ramp()
        } else {
            self.apply_ramp().0
        }
    }

    // Private: apply without updating status (used internally so callers
    // always go through `apply_ramp` which returns status info).
    unsafe fn apply_ramp_internal(&self, v: i32) {
        let ramp = gamma_ramp::build_ramp(v);
        let null = HWND(std::ptr::null_mut());
        let hdc  = GetDC(null);
        if !hdc.0.is_null() {
            gamma_ramp::SetDeviceGammaRamp(hdc, &ramp);
            ReleaseDC(null, hdc);
        }
    }

    // ── NVIDIA CAM detection ──────────────────────────────────────────────────

    /// Check for NVIDIA "Color Accuracy Mode" via registry.
    pub unsafe fn is_nvidia_cam_enabled() -> bool {
        const PATH_W: &[u16] = &[
            b'S' as u16, b'O' as u16, b'F' as u16, b'T' as u16, b'W' as u16,
            b'A' as u16, b'R' as u16, b'E' as u16, b'\\' as u16, b'N' as u16,
            b'V' as u16, b'I' as u16, b'D' as u16, b'I' as u16, b'A' as u16,
            b' ' as u16, b'C' as u16, b'o' as u16, b'r' as u16, b'p' as u16,
            b'o' as u16, b'r' as u16, b'a' as u16, b't' as u16, b'i' as u16,
            b'o' as u16, b'n' as u16, b'\\' as u16, b'G' as u16, b'l' as u16,
            b'o' as u16, b'b' as u16, b'a' as u16, b'l' as u16, b'\\' as u16,
            b'N' as u16, b'V' as u16, b'T' as u16, b'w' as u16, b'e' as u16,
            b'a' as u16, b'k' as u16, b'\\' as u16, b'D' as u16, b'e' as u16,
            b'v' as u16, b'i' as u16, b'c' as u16, b'e' as u16, b's' as u16,
            0u16, // NUL terminator
        ];

        const COLOR_SUFFIX: &[u16] = &[
            b'\\' as u16, b'C' as u16, b'o' as u16, b'l' as u16,
            b'o' as u16,  b'r' as u16, 0u16, // NUL terminator
        ];

        const VNAME: &[u16] = &[
            b'3' as u16, b'5' as u16, b'3' as u16, b'8' as u16,
            b'9' as u16, b'7' as u16, b'0' as u16, 0u16, // NUL terminator
        ];

        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(PATH_W.as_ptr()),
            0, KEY_READ, &mut key) != ERROR_SUCCESS { return false; }

        let mut idx = 0u32;
        loop {
            let mut name_buf = [0u16; 256];
            let mut name_len = 256u32;
            if RegEnumKeyExW(key, idx, PWSTR(name_buf.as_mut_ptr()), &mut name_len,
                None, PWSTR::null(), None, None) != ERROR_SUCCESS { break; }

            let name_len = name_len as usize;
            let suffix_len = COLOR_SUFFIX.len();
            let sub_total = name_len + suffix_len;
            let mut sub_buf = [0u16; 512];
            sub_buf[..name_len].copy_from_slice(&name_buf[..name_len]);
            sub_buf[name_len..sub_total].copy_from_slice(COLOR_SUFFIX);

            let mut ckey = HKEY::default();
            if RegOpenKeyExW(key, PCWSTR(sub_buf.as_ptr()), 0, KEY_READ, &mut ckey)
                == ERROR_SUCCESS
            {
                let mut data = 0u32;
                let mut sz   = 4u32;
                if RegQueryValueExW(ckey, PCWSTR(VNAME.as_ptr()), None, None,
                    Some(&mut data as *mut _ as _), Some(&mut sz))
                    == ERROR_SUCCESS && data == 2
                {
                    RegCloseKey(ckey); RegCloseKey(key); return true;
                }
                RegCloseKey(ckey);
            }
            idx += 1;
        }
        RegCloseKey(key);
        false
    }

    // ── Hz-keyed profile store ────────────────────────────────────────────────

    /// On Hz switch: always load the saved black level for this Hz (defaulting
    /// to OFF=0 if no value has been saved yet).
    /// Returns `Some((v, status_text))` when a non-zero value was applied.
    pub unsafe fn try_auto_load_profile_for_hz(
        &mut self,
        hz: i32,
        ini: &mut ProfileManager,
    ) -> Option<(i32, String)> {
        let sec = hz_section(hz);
        let v = ini.read_int(&sec, "Black", DEFAULT_BLACK).clamp(0, MAX_BLACK);
        SendMessageW(self.h_sld_black, TBM_SETPOS, WPARAM(1), LPARAM(v as isize));
        InvalidateRect(self.h_sld_black, None, true);
        UpdateWindow(self.h_sld_black);
        let v_text = if v == 0 { "OFF".to_string() } else { format!("{v}") };
        set_text(self.h_lbl_black_val, &v_text);
        self.apply_ramp_internal(v);
        self.hdr_panel.update(v);
        if v != 0 {
            Some((v, format!("Auto-applied [{v_text}] at {hz} Hz")))
        } else {
            None
        }
    }

    // ── Refresh rate ──────────────────────────────────────────────────────────

    pub unsafe fn populate_refresh_rates(&mut self, ini: &mut ProfileManager) {
        let mut rates: Vec<u32> = Vec::new();
        let mut i: u32 = 0;
        loop {
            let mut dm = DEVMODEW {
                dmSize: mem::size_of::<DEVMODEW>() as u16, ..Default::default()
            };
            if !EnumDisplaySettingsW(None, ENUM_DISPLAY_SETTINGS_MODE(i), &mut dm).as_bool() {
                break;
            }
            if dm.dmDisplayFrequency > 0 && !rates.contains(&dm.dmDisplayFrequency) {
                rates.push(dm.dmDisplayFrequency);
            }
            i += 1;
        }

        let mut cur_dm = DEVMODEW {
            dmSize: mem::size_of::<DEVMODEW>() as u16, ..Default::default()
        };
        EnumDisplaySettingsW(None, ENUM_CURRENT_SETTINGS, &mut cur_dm);
        let cur = cur_dm.dmDisplayFrequency;

        if rates.is_empty() { rates.push(cur); }
        rates.sort_unstable_by(|a, b| b.cmp(a));

        self.suppress_load = true;
        SendMessageW(self.h_ddl_refresh, CB_RESETCONTENT, WPARAM(0), LPARAM(0));
        let mut cur_idx = 0usize;
        for (idx, &r) in rates.iter().enumerate() {
            let sec = hz_section(r as i32);
            let black = ini.read_int(&sec, "Black", DEFAULT_BLACK).clamp(0, MAX_BLACK);
            let label = if black > 0 {
                format!("{r} Hz [{black}]\0")
            } else {
                format!("{r} Hz\0")
            };
            let text: Vec<u16> = label.encode_utf16().collect();
            SendMessageW(self.h_ddl_refresh, CB_ADDSTRING,
                WPARAM(0), LPARAM(text.as_ptr() as isize));
            if r == cur { cur_idx = idx; }
        }
        SendMessageW(self.h_ddl_refresh, CB_SETMINVISIBLE,
            WPARAM(rates.len().min(20)), LPARAM(0));
        SendMessageW(self.h_ddl_refresh, CB_SETCURSEL, WPARAM(cur_idx), LPARAM(0));
        SendMessageW(self.h_ddl_refresh, CB_SETTOPINDEX, WPARAM(0), LPARAM(0));
        self.suppress_load = false;
    }

    /// Apply refresh rate selected in the dropdown.
    /// Returns `Ok(hz)` on success, `Err(code)` on failure.
    pub unsafe fn apply_refresh_rate(&self) -> std::result::Result<u32, i32> {
        let Some(sel) = combo_selected_text(self.h_ddl_refresh) else {
            return Err(-1);
        };
        let Some(hz) = sel.splitn(2, ' ').next()
            .and_then(|s| s.trim().parse::<u32>().ok()) else {
            return Err(-1);
        };
        let mut dm = DEVMODEW {
            dmSize: mem::size_of::<DEVMODEW>() as u16, ..Default::default()
        };
        EnumDisplaySettingsW(None, ENUM_CURRENT_SETTINGS, &mut dm);
        dm.dmDisplayFrequency = hz;
        dm.dmFields = DEVMODE_FIELD_FLAGS(0x40_0000);
        let result = ChangeDisplaySettingsW(Some(&dm), CDS_TYPE(0));
        if result.0 == 0 { Ok(hz) } else { Err(result.0) }
    }
}

// ── Free helpers (module-private; exposed as `pub` where app.rs needs them) ───

/// INI section key for a given refresh rate.
pub fn hz_section(hz: i32) -> String { format!("hz_{hz}") }

/// Current display refresh rate in Hz.
pub unsafe fn get_current_hz() -> i32 {
    let mut dm = DEVMODEW {
        dmSize: mem::size_of::<DEVMODEW>() as u16, ..Default::default()
    };
    EnumDisplaySettingsW(None, ENUM_CURRENT_SETTINGS, &mut dm);
    dm.dmDisplayFrequency as i32
}

/// Render-timer interval in ms.
///
/// The HDR panel only calls Present when render_dirty is true — set on
/// discrete events (slider move, resize, HDR toggle), never per-frame.
/// 100 ms is zero-cost visually while avoiding sub-frame jitter that a
/// per-Hz interval injects into the VRR compositor.
///
/// The old per-Hz formula with Present(1,0) blocked the UI timer callback
/// on vblank, producing irregular cadences VRR interprets as flicker.
pub fn render_interval_ms() -> u32 { 100 }