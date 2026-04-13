use windows::core::*;
use windows::Win32::{
    Foundation::{HWND, RECT},
    UI::{
        HiDpi::GetDpiForWindow,
        WindowsAndMessaging::{
            BeginDeferWindowPos, DeferWindowPos, EndDeferWindowPos,
            GetClientRect, GetWindowRect, SetWindowPos,
            SWP_NOZORDER, SWP_NOMOVE, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOCOPYBITS,
        },
    },
};

use crate::constants::*;

// ── Public entry point ────────────────────────────────────────────────────────

pub unsafe fn apply(st: &mut crate::app::AppState, hwnd: HWND) {
    const CTRL_COUNT: i32 = 92; // over-estimate is fine; OS clamps it
    if let Some(mut grid) = LayoutGrid::new(hwnd, CTRL_COUNT) {
        grid.place_side_panel(st, hwnd);
        let _ = grid.place_black_crush_tab(st, hwnd);
        grid.place_taskbar_tab(st, hwnd);
        grid.place_system_tab(st, hwnd);
        grid.place_hotkeys_tab(st, hwnd);
        grid.place_debug_tab(st, hwnd);
        grid.place_about_tab(st, hwnd);
        grid.flush();
    }
}

// ── LayoutGrid ────────────────────────────────────────────────────────────────

struct LayoutGrid {
    dpi: f32,
    ch: i32,
    side_w: i32,
    main_x: i32,
    main_w: i32,
    hdwp: windows::Win32::UI::WindowsAndMessaging::HDWP,
}

impl LayoutGrid {
    unsafe fn new(hwnd: HWND, count: i32) -> Option<Self> {
        let dpi = GetDpiForWindow(hwnd).max(96) as f32;
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);

        let cw = rc.right;
        let ch = rc.bottom;
        let side_w = Self::scale(dpi, 220);
        let main_x = side_w + Self::scale(dpi, 1) + Self::scale(dpi, 12);
        let main_w = cw - main_x - Self::scale(dpi, 12);
        if main_w < Self::scale(dpi, 100) {
            return None;
        }

        let hdwp = BeginDeferWindowPos(count).unwrap_or_default();
        Some(Self { dpi, ch, side_w, main_x, main_w, hdwp })
    }

    /// Commit all deferred moves in one atomic call.
    unsafe fn flush(&mut self) {
        if !self.hdwp.0.is_null() {
            let _ = EndDeferWindowPos(self.hdwp);
            self.hdwp = windows::Win32::UI::WindowsAndMessaging::HDWP::default();
        }
    }

    fn scale(dpi: f32, px: i32) -> i32 {
        (px as f32 * dpi / 96.0).round() as i32
    }

    fn s(&self, px: i32) -> i32 {
        Self::scale(self.dpi, px)
    }

    fn set(&mut self, control: HWND, x: i32, y: i32, w: i32, h: i32) {
        if control.0.is_null() || self.hdwp.0.is_null() { return; }
        unsafe {
            self.hdwp = DeferWindowPos(
                self.hdwp, control, None,
                x, y, w.max(1), h.max(1),
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOCOPYBITS,
            ).unwrap_or(self.hdwp);
        }
    }

    /// Move control off-screen so it doesn't paint outside its tab.
    fn hide(&mut self, control: HWND) {
        if control.0.is_null() || self.hdwp.0.is_null() { return; }
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::SWP_HIDEWINDOW;
            self.hdwp = DeferWindowPos(
                self.hdwp, control, None,
                -32000, -32000, 1, 1,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOCOPYBITS | SWP_HIDEWINDOW,
            ).unwrap_or(self.hdwp);
        }
    }

    fn place_with_gap(&mut self, sy: &mut i32, x: i32, width: i32, height: i32, control: HWND, gap: i32) {
        self.set(control, x, *sy, width, height);
        *sy += height + gap;
    }

    fn place_separator(&mut self, sy: &mut i32, x: i32, width: i32, control: HWND) {
        self.set(control, x, *sy, width, 1);
        *sy += self.s(10);
    }

    // ── Tabs ──────────────────────────────────────────────────────────────────

    fn place_side_panel(&mut self, st: &mut crate::app::AppState, _hwnd: HWND) {
        self.set(st.h_sep_vert, self.side_w, 0, 1, self.ch);

        let nav_h = self.s(38);
        let mut sy = self.s(14);

        // Nav order: 0=Black Crush, 1=Taskbar, 5=System, 2=Hotkeys, 4=About, 3=Debug
        for idx in [0usize, 1, 5, 2, 4] {
            self.set(st.h_nav_btn[idx], 0, sy, self.side_w, nav_h);
            sy += nav_h;
        }

        // Debug button: visible only in debug mode, otherwise parked off-screen
        if crate::app::is_debug_mode() {
            self.set(st.h_nav_btn[3], 0, sy, self.side_w, nav_h);
            sy += nav_h;
        } else {
            self.set(st.h_nav_btn[3], -self.side_w - 1, 0, self.side_w, nav_h);
        }

        let col_w  = self.side_w - self.s(12) - self.s(8);
        let tog_h  = self.s(28);
        let tog1_y = sy + self.s(10);
        let tog2_y = tog1_y + tog_h + self.s(4);
        self.set(st.h_chk_startup,    self.s(12), tog1_y, col_w, tog_h);
        self.set(st.h_btn_hdr_toggle, self.s(12), tog2_y, col_w, tog_h);
    }

    fn place_black_crush_tab(&mut self, st: &mut crate::app::AppState, _hwnd: HWND) -> i32 {
        let mut y = self.s(8) + self.s(6);

        // Title + subtitles
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(36), st.crush.h_lbl_title, self.s(2));
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20), st.crush.h_lbl_sub1,  self.s(4));
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20), st.crush.h_lbl_sub2,  self.s(14));

        // ── Black Level ───────────────────────────────────────────────────────
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20), st.crush.h_lbl_bl_sect, self.s(4));
        self.place_separator(&mut y, self.main_x, self.main_w, st.h_sep_h[0]);

        let slider_h = self.s(40);
        self.set(st.crush.h_sld_black,     self.main_x, y, self.main_w - self.s(56) - self.s(4), slider_h);
        self.set(st.crush.h_lbl_black_val, self.main_x + self.main_w - self.s(56), y, self.s(56), slider_h);
        y += self.s(44);
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(22), st.crush.h_lbl_sl_hint, self.s(6));
        self.set(st.crush.h_lbl_gamma_warn, self.main_x, y, self.main_w, 0); // zero-size; reserved

        // ── Refresh Rate ──────────────────────────────────────────────────────
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20), st.crush.h_lbl_ref_sect, self.s(4));
        self.place_separator(&mut y, self.main_x, self.main_w, st.h_sep_h[3]);

        // Dropdown left; profile note to its right, vertically centred on closed height
        let ddl_w   = self.s(110);
        let ddl_h   = self.s(26);
        let txt_gap = self.s(8);
        let note_x  = self.main_x + ddl_w + txt_gap;
        let note_w  = self.main_w - ddl_w - txt_gap;

        self.set(st.crush.h_ddl_refresh,    self.main_x, y, ddl_w, ddl_h + self.s(220)); // extra height for dropdown list
        self.set(st.crush.h_lbl_hz_icon,    0, 0, 0, 0);                                  // hidden — unused
        self.set(st.crush.h_lbl_hz_profile, note_x, y, note_w, ddl_h);
        y += ddl_h + self.s(14);

        // ── Near-Black Calibration ────────────────────────────────────────────
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20), st.crush.h_lbl_hdr_sect, self.s(4));
        self.place_separator(&mut y, self.main_x, self.main_w, st.h_sep_h[1]);

        let note_h = self.s(20);

        // Panel grows with window height; reserve space for the row below + toggle + margin
        let bottom_reserved = self.s(20)             // note/slider row
            + self.s(14)                             // gap after row
            + self.s(1) + self.s(10)                 // separator
            + self.s(30) + self.s(10)                // toggle button + gap
            + self.s(36)                             // status/error label
            + self.s(14);                            // bottom margin
        let panel_h = (self.ch - y - bottom_reserved).max(self.s(280));
        self.place_with_gap(&mut y, self.main_x, self.main_w, panel_h, st.crush.h_hdr_panel, 0);

        // Bottom row: [zoom_out] [squares slider] [zoom_in] — icon slots removed
        let zoom_icon_w = self.s(18);
        let zoom_gap    = self.s(6);
        let sld_w       = self.main_w - zoom_icon_w * 2 - zoom_gap * 2;
        let rsl_x       = self.main_x + zoom_icon_w + zoom_gap;
        self.set(st.crush.h_lbl_hdr_note,  0, 0, 0, 0); // removed
        self.set(st.crush.h_lbl_range_val, 0, 0, 0, 0); // removed
        self.set(st.crush.h_sld_squares,   rsl_x, y, sld_w, note_h);
        y += note_h + self.s(14);

        self.place_separator(&mut y, self.main_x, self.main_w, st.h_sep_h[2]);

        // Toggle button, centred horizontally
        let btn_ab = self.s(200);
        self.set(st.crush.h_btn_toggle,
            self.main_x + (self.main_w - btn_ab) / 2, y, btn_ab, self.s(30));
        y += self.s(30) + self.s(6);

        // Status and error labels share the same slot; shown mutually exclusively
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(36), st.h_lbl_status, 0);
        self.set(st.h_lbl_error, self.main_x, y - self.s(36), self.main_w, self.s(36));

        y
    }

    fn place_taskbar_tab(&mut self, st: &mut crate::app::AppState, _hwnd: HWND) {
        let mut dy = self.s(8) + self.s(6);

        // Title, description, toggle
        self.place_with_gap(&mut dy, self.main_x, self.main_w, self.s(36), st.dimmer.h_lbl_dim_title,   self.s(4));
        self.place_with_gap(&mut dy, self.main_x, self.main_w, self.s(40), st.dimmer.h_lbl_dim_sub,     self.s(4));
        self.place_with_gap(&mut dy, self.main_x, self.main_w, self.s(28), st.dimmer.h_chk_taskbar_dim, self.s(10));

        // ── Dim Level ─────────────────────────────────────────────────────────
        self.place_with_gap(&mut dy, self.main_x, self.main_w, self.s(20), st.dimmer.h_lbl_dim_sect, self.s(4));
        self.place_separator(&mut dy, self.main_x, self.main_w, st.dimmer.h_sep_dim_sect);

        let pct_w = self.s(56);
        self.set(st.dimmer.h_sld_taskbar_dim,
            self.main_x, dy, self.main_w - pct_w - self.s(4), self.s(40));
        self.set(st.dimmer.h_lbl_dim_pct,
            self.main_x + self.main_w - pct_w, dy, pct_w, self.s(40));
        dy += self.s(40) + self.s(14);

        // ── Fade Timings ──────────────────────────────────────────────────────
        self.place_with_gap(&mut dy, self.main_x, self.main_w, self.s(20), st.dimmer.h_lbl_fade_sect, self.s(4));
        self.place_separator(&mut dy, self.main_x, self.main_w, st.dimmer.h_sep_fade_sect);

        let fade_lbl_w = self.s(68);
        let fade_ms_w  = self.s(72);
        let fade_sld_w = self.main_w - fade_lbl_w - fade_ms_w - self.s(8);
        let fade_sld_x = self.main_x + fade_lbl_w + self.s(4);
        let fade_val_x = self.main_x + self.main_w - fade_ms_w;
        let fade_row_h = self.s(32);

        self.set(st.dimmer.h_lbl_fade_in_title,  self.main_x,  dy, fade_lbl_w, fade_row_h);
        self.set(st.dimmer.h_sld_fade_in,        fade_sld_x,   dy, fade_sld_w, fade_row_h);
        self.set(st.dimmer.h_lbl_fade_in_val,    fade_val_x,   dy, fade_ms_w,  fade_row_h);
        dy += fade_row_h + self.s(6);

        self.set(st.dimmer.h_lbl_fade_out_title, self.main_x,  dy, fade_lbl_w, fade_row_h);
        self.set(st.dimmer.h_sld_fade_out,       fade_sld_x,   dy, fade_sld_w, fade_row_h);
        self.set(st.dimmer.h_lbl_fade_out_val,   fade_val_x,   dy, fade_ms_w,  fade_row_h);
        dy += fade_row_h + self.s(14);

        let def_btn_w = self.s(160);
        self.set(st.dimmer.h_btn_dim_defaults,
            self.main_x + (self.main_w - def_btn_w) / 2, dy, def_btn_w, self.s(26));
    }

    fn place_system_tab(&mut self, st: &mut crate::app::AppState, _hwnd: HWND) {
        let mut y = self.s(8) + self.s(6);

        // Title + description
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(36), st.system.h_lbl_title, self.s(2));
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20), st.system.h_lbl_desc,  self.s(10));

        // Shared column metrics (reused for toggles and dropdowns)
        let pw_lbl_w       = self.s(160);
        let pw_ddl_w       = self.s(110);
        let pw_ddl_x       = self.main_x + pw_lbl_w + self.s(8);
        let pw_row_h       = self.s(28);
        let pw_ddl_popup_h = pw_row_h + self.s(220);

        let pill_w   = self.s(28) + self.s(2) * 2;
        let tog_w    = (pw_ddl_x - self.main_x) + pill_w;
        let tog_st_x = self.main_x + tog_w + self.s(10);
        let tog_st_w = self.main_x + self.main_w - tog_st_x;

        // ── Taskbar ───────────────────────────────────────────────────────────
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20), st.system.h_lbl_sect_display, self.s(4));
        self.place_separator(&mut y, self.main_x, self.main_w, st.system.h_sep_display);

        self.set(st.system.h_btn_taskbar_autohide,    self.main_x, y, tog_w,    pw_row_h);
        self.set(st.system.h_lbl_taskbar_autohide_st, tog_st_x,    y, tog_st_w, pw_row_h);
        y += pw_row_h + self.s(14);

        // ── Power ─────────────────────────────────────────────────────────────
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20), st.system.h_lbl_sect_power, self.s(4));
        self.place_separator(&mut y, self.main_x, self.main_w, st.system.h_sep_power);

        self.set(st.system.h_lbl_screen_timeout, self.main_x, y, pw_lbl_w, pw_row_h);
        self.set(st.system.h_ddl_screen_timeout, pw_ddl_x,    y, pw_ddl_w, pw_ddl_popup_h);
        y += pw_row_h + self.s(10);

        self.set(st.system.h_lbl_sleep_timeout,  self.main_x, y, pw_lbl_w, pw_row_h);
        self.set(st.system.h_ddl_sleep_timeout,  pw_ddl_x,    y, pw_ddl_w, pw_ddl_popup_h);
        y += pw_row_h + self.s(14);

        // ── Screensaver ───────────────────────────────────────────────────────
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20), st.system.h_lbl_sect_screensaver, self.s(4));
        self.place_separator(&mut y, self.main_x, self.main_w, st.system.h_sep_screensaver);

        self.set(st.system.h_lbl_screensaver, self.main_x, y, pw_lbl_w, pw_row_h);
        self.set(st.system.h_ddl_screensaver, pw_ddl_x,    y, pw_ddl_w, pw_ddl_popup_h);
        y += pw_row_h + self.s(10);

        // Wait row: [label] [edit] [spin] [minutes]
        let ss_edit_w = self.s(38);
        let ss_spin_w = self.s(18);
        let ss_spin_x = pw_ddl_x + ss_edit_w;
        let ss_mins_x = ss_spin_x + ss_spin_w + self.s(6);
        let ss_mins_w = self.main_x + self.main_w - ss_mins_x;
        self.set(st.system.h_lbl_ss_timeout, self.main_x, y, pw_lbl_w,  pw_row_h);
        self.set(st.system.h_edt_ss_timeout, pw_ddl_x,    y, ss_edit_w, self.s(22));
        self.set(st.system.h_spin_ss,        ss_spin_x,   y, ss_spin_w, self.s(22));
        self.set(st.system.h_lbl_ss_minutes, ss_mins_x,   y, ss_mins_w, pw_row_h);
        // y not advanced — last row
    }

    fn place_hotkeys_tab(&mut self, st: &mut crate::app::AppState, _hwnd: HWND) {
        let mut y = self.s(8) + self.s(6);

        // Title + description
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(36), st.hotkeys.h_lbl_title, self.s(2));
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(48), st.hotkeys.h_lbl_desc,  self.s(12));

        // Column metrics
        let clear_w = self.s(28);
        let gap_c   = self.s(4);
        let gap_le  = self.s(8);
        let label_w = self.s(200);
        let edit_w  = self.s(140);
        let edit_x  = self.main_x + label_w + gap_le;
        let clear_x = edit_x + edit_w + gap_c;
        let row_h   = self.s(28);
        let row_gap = self.s(8);

        // ── Black Crush Tweak ─────────────────────────────────────────────────
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20), st.hotkeys.h_lbl_sect_crush, self.s(4));
        self.place_separator(&mut y, self.main_x, self.main_w, st.hotkeys.h_sep_sect[0]);

        for i in 0..4 { // rows 0–3
            let r = &st.hotkeys.rows[i];
            self.set(r.h_lbl,   self.main_x, y, label_w, row_h);
            self.set(r.h_edit,  edit_x,      y, edit_w,  row_h);
            self.set(r.h_clear, clear_x,     y, clear_w, row_h);
            y += row_h + row_gap;
        }
        y += self.s(6);

        // ── Taskbar Dimmer ────────────────────────────────────────────────────
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20), st.hotkeys.h_lbl_sect_dimmer, self.s(4));
        self.place_separator(&mut y, self.main_x, self.main_w, st.hotkeys.h_sep_sect[1]);

        { // row 4: Toggle Taskbar Dimmer
            let r = &st.hotkeys.rows[4];
            self.set(r.h_lbl,   self.main_x, y, label_w, row_h);
            self.set(r.h_edit,  edit_x,      y, edit_w,  row_h);
            self.set(r.h_clear, clear_x,     y, clear_w, row_h);
            y += row_h + row_gap;
        }
        y += self.s(6);

        // ── System ────────────────────────────────────────────────────────────
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20), st.hotkeys.h_lbl_sect_system, self.s(4));
        self.place_separator(&mut y, self.main_x, self.main_w, st.hotkeys.h_sep_sect[2]);

        { // row 5: Toggle HDR/SDR
            let r = &st.hotkeys.rows[5];
            self.set(r.h_lbl,   self.main_x, y, label_w, row_h);
            self.set(r.h_edit,  edit_x,      y, edit_w,  row_h);
            self.set(r.h_clear, clear_x,     y, clear_w, row_h);
            y += row_h + row_gap;
        }
        y += self.s(8);

        self.place_separator(&mut y, self.main_x, self.main_w, st.hotkeys.h_sep_bottom);
    }

    fn place_debug_tab(&mut self, st: &mut crate::app::AppState, _hwnd: HWND) {
        let mut dy = self.s(8) + self.s(6);

        // Title
        self.place_with_gap(&mut dy, self.main_x, self.main_w, self.s(36), st.debug.h_lbl_title, self.s(10));

        // ── Dimmer State ──────────────────────────────────────────────────────
        self.place_with_gap(&mut dy, self.main_x, self.main_w, self.s(20), st.debug.h_lbl_sect_state, self.s(4));
        self.place_separator(&mut dy, self.main_x, self.main_w, st.debug.h_sep_state);

        let key_w = self.s(180);
        let val_x = self.main_x + key_w + self.s(8);
        let val_w = self.main_w - key_w - self.s(8);
        let row_h = self.s(22);
        let gap   = self.s(6);

        let rows = [
            (st.debug.h_lbl_fs_key,           st.debug.h_lbl_fs_val),
            (st.debug.h_lbl_ah_key,           st.debug.h_lbl_ah_val),
            (st.debug.h_lbl_alpha_key,        st.debug.h_lbl_alpha_val),
            (st.debug.h_lbl_target_key,       st.debug.h_lbl_target_val),
            (st.debug.h_lbl_overlays_key,     st.debug.h_lbl_overlays_val),
            (st.debug.h_lbl_zpos_key,         st.debug.h_lbl_zpos_val),
            (st.debug.h_lbl_taskbar_zpos_key, st.debug.h_lbl_taskbar_zpos_val),
        ];
        for (key, val) in rows {
            self.set(key, self.main_x, dy, key_w, row_h);
            self.set(val, val_x,       dy, val_w, row_h);
            dy += row_h + gap;
        }

        // ── Suppression ───────────────────────────────────────────────────────
        dy += self.s(6);
        self.place_with_gap(&mut dy, self.main_x, self.main_w, self.s(20), st.debug.h_lbl_sect_suppress, self.s(4));
        self.place_separator(&mut dy, self.main_x, self.main_w, st.debug.h_sep_suppress);

        let chk_h = self.s(24);
        self.place_with_gap(&mut dy, self.main_x, self.main_w, chk_h, st.debug.h_chk_suppress_fs, self.s(4));
        self.place_with_gap(&mut dy, self.main_x, self.main_w, chk_h, st.debug.h_chk_suppress_ah, self.s(10));

        // ── Event Log ─────────────────────────────────────────────────────────
        // "Clear" button right-aligned on the same row as the heading
        let btn_h   = self.s(22);
        let clear_w = self.s(68);
        let gap_bc  = self.s(8);
        let head_w  = self.main_w - clear_w - gap_bc;

        self.set(st.debug.h_lbl_sect_log,  self.main_x, dy, head_w, self.s(20));
        self.set(st.debug.h_btn_log_clear, self.main_x + head_w + gap_bc, dy, clear_w, btn_h);
        dy += self.s(20) + self.s(4);
        self.place_separator(&mut dy, self.main_x, self.main_w, st.debug.h_sep_log);

        // Listbox fills remaining vertical space
        let bottom = self.ch - self.s(14);
        let log_h  = (bottom - dy).max(self.s(60));
        self.set(st.debug.h_lst_zlog, self.main_x, dy, self.main_w, log_h);
    }

    fn place_about_tab(&mut self, st: &mut crate::app::AppState, _hwnd: HWND) {
        let mut y = self.s(8) + self.s(6);

        // Title
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(36), st.about.h_lbl_title, self.s(10));

        // ── Application ───────────────────────────────────────────────────────
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20), st.about.h_lbl_sect_about, self.s(4));
        self.place_separator(&mut y, self.main_x, self.main_w, st.about.h_sep_about);
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20), st.about.h_lbl_version, self.s(6));
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20), st.about.h_lbl_link,    self.s(14));

        // ── Updates ───────────────────────────────────────────────────────────
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20), st.about.h_lbl_sect_update, self.s(4));
        self.place_separator(&mut y, self.main_x, self.main_w, st.about.h_sep_update);

        // Status: "Checking…" / "Up to date." / "vX.Y available"
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20), st.about.h_lbl_check_info, self.s(8));

        // "Update Now" + download progress (hidden until update found)
        let btn_w = self.s(140);
        self.set(st.about.h_btn_update,    self.main_x, y, btn_w, self.s(28));
        self.set(st.about.h_lbl_dl_status, self.main_x + btn_w + self.s(10), y,
            self.main_w - btn_w - self.s(10), self.s(28));
        y += self.s(28) + self.s(14);

        // ── Changelog (only when update available) ────────────────────────────
        if st.update_available {
            self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20), st.about.h_lbl_sect_changelog, self.s(4));
            self.place_separator(&mut y, self.main_x, self.main_w, st.about.h_sep_changelog);
            self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(200), st.about.h_lbl_changelog, 0);
        } else {
            // Hide so controls don't bleed through on resize
            self.hide(st.about.h_lbl_sect_changelog);
            self.hide(st.about.h_sep_changelog);
            self.hide(st.about.h_lbl_changelog);
        }
    }
}