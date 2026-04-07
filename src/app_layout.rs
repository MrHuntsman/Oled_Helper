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

pub unsafe fn apply(st: &mut crate::app::AppState, hwnd: HWND) {
    // Count of controls to reposition — over-estimate is fine, the OS clamps it.
    const CTRL_COUNT: i32 = 82;
    if let Some(mut grid) = LayoutGrid::new(hwnd, CTRL_COUNT) {
        grid.place_side_panel(st, hwnd);
        let _ = grid.place_black_crush_tab(st, hwnd);
        grid.place_taskbar_tab(st, hwnd);
        grid.place_hotkeys_tab(st, hwnd);
        grid.place_debug_tab(st, hwnd);
        grid.place_about_tab(st, hwnd);
        grid.flush(); // single atomic reposition of all controls
        // Removed: grid.resize_window_to_content(hwnd, needed_height);
    }
}

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

    /// Flush all deferred moves in one atomic system call.
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

    fn place_with_gap(&mut self, sy: &mut i32, x: i32, width: i32, height: i32, control: HWND, gap: i32) {
        self.set(control, x, *sy, width, height);
        *sy += height + gap;
    }

    fn place_separator(&mut self, sy: &mut i32, x: i32, width: i32, control: HWND) {
        self.set(control, x, *sy, width, 1);
        *sy += self.s(10);
    }

    fn place_side_panel(&mut self, st: &mut crate::app::AppState, _hwnd: HWND) {
        self.set(st.h_sep_vert, self.side_w, 0, 1, self.ch);

        let nav_h = self.s(38);
        let mut sy = self.s(14);

        self.set(st.h_nav_btn[0], 0, sy, self.side_w, nav_h);
        sy += nav_h;
        self.set(st.h_nav_btn[1], 0, sy, self.side_w, nav_h);
        sy += nav_h;
        self.set(st.h_nav_btn[2], 0, sy, self.side_w, nav_h);
        sy += nav_h;
        self.set(st.h_nav_btn[4], 0, sy, self.side_w, nav_h);
        sy += nav_h;
        self.set(st.h_nav_btn[3], 0, sy, self.side_w, nav_h);

        let col_w = self.side_w - self.s(12) - self.s(8);
        let tog_h = self.s(28);
        let tog_gap = self.s(4);
        // Both toggles stacked directly below the last nav button.
        let tog1_y = sy + self.s(10);
        let tog2_y = tog1_y + tog_h + tog_gap;
        self.set(st.h_chk_startup,    self.s(12), tog1_y, col_w, tog_h);
        self.set(st.h_btn_hdr_toggle, self.s(12), tog2_y, col_w, tog_h);
        let quit_y = self.ch - self.s(14) - self.s(24);
        let foot_hw = col_w / 2 - self.s(2);
        self.set(st.h_btn_minimize, self.s(12), quit_y, foot_hw, self.s(24));
        self.set(st.h_btn_quit,     self.s(12) + foot_hw + self.s(4), quit_y, foot_hw, self.s(24));
    }

    fn place_black_crush_tab(&mut self, st: &mut crate::app::AppState, _hwnd: HWND) -> i32 {
        let mut y = self.s(8) + self.s(6);

        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(36), st.crush.h_lbl_title,  self.s(2));
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(36), st.crush.h_lbl_sub1,   0);
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(36), st.crush.h_lbl_sub2,   self.s(14));

        // ── Black Level section ───────────────────────────────────────────────
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20), st.crush.h_lbl_bl_sect, self.s(4));
        self.place_separator(&mut y, self.main_x, self.main_w, st.h_sep_h[0]);

        let slider_h = self.s(40);
        self.set(st.crush.h_sld_black,
            self.main_x, y, self.main_w - self.s(56) - self.s(4), slider_h);
        self.set(st.crush.h_lbl_black_val,
            self.main_x + self.main_w - self.s(56), y, self.s(56), slider_h);
        y += self.s(44);
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(22), st.crush.h_lbl_sl_hint,    self.s(6));
        // h_lbl_gamma_warn is parked at zero size — it exists but takes no space.
        self.set(st.crush.h_lbl_gamma_warn, self.main_x, y, self.main_w, 0);

        // ── Refresh Rate section ──────────────────────────────────────────────
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20), st.crush.h_lbl_ref_sect, self.s(4));
        self.place_separator(&mut y, self.main_x, self.main_w, st.h_sep_h[3]);

        // Dropdown on the left; info icon + note to its right, vertically centred
        // on the closed-height of the dropdown.
        let ddl_w    = self.s(110);
        let ddl_h    = self.s(26);           // closed height only (for row sizing)
        let txt_gap  = self.s(8);            // gap between dropdown and text (no icon)
        let note_x   = self.main_x + ddl_w + txt_gap;
        let note_w   = self.main_w - ddl_w - txt_gap;

        // Dropdown: full height = closed + list popup so the list can paint below.
        self.set(st.crush.h_ddl_refresh,    self.main_x, y, ddl_w, ddl_h + self.s(220));
        // Icon hidden — no longer used.
        self.set(st.crush.h_lbl_hz_icon,    0, 0, 0, 0);
        self.set(st.crush.h_lbl_hz_profile, note_x, y, note_w, ddl_h);
        y += ddl_h + self.s(14);

        // ── Near-Black Calibration section ────────────────────────────────────
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20), st.crush.h_lbl_hdr_sect, self.s(4));
        self.place_separator(&mut y, self.main_x, self.main_w, st.h_sep_h[1]);

        let note_h = self.s(20);

        // HDR panel: base height 280 px, but grow with extra window height so the
        // calibration squares fill the available space rather than leaving dead
        // whitespace below them when the window is maximised or stretched tall.
        // We leave room below for the bottom row (note + zoom slider) + separator
        // + toggle button + status label + a comfortable margin.
        let bottom_reserved = self.s(20)   // note/slider row
            + self.s(14)                   // gap after note row
            + self.s(1) + self.s(10)       // separator
            + self.s(30) + self.s(10)      // toggle button + gap
            + self.s(36)                   // status/error label
            + self.s(14);                  // bottom margin
        let panel_h = (self.ch - y - bottom_reserved).max(self.s(280));
        self.place_with_gap(&mut y, self.main_x, self.main_w, panel_h, st.crush.h_hdr_panel, 0);

        // Bottom row: zoom_out icon | squares slider | zoom icon.
        // The HDR/SDR note label and value label are hidden (zero size).
        let zoom_icon_w = self.s(18);
        let zoom_gap    = self.s(6);
        let sld_w       = self.main_w - zoom_icon_w * 2 - zoom_gap * 2;
        let zoom_out_x  = self.main_x;
        let rsl_x       = zoom_out_x + zoom_icon_w + zoom_gap;
        self.set(st.crush.h_lbl_hdr_note,    0, 0, 0, 0);   // removed
        self.set(st.crush.h_lbl_range_val,   0, 0, 0, 0);   // removed
       //self.set(st.crush.h_lbl_zoom_out_icon, zoom_out_x, y, zoom_icon_w, note_h);
        self.set(st.crush.h_sld_squares,       rsl_x,      y, sld_w,       note_h);
       //self.set(st.crush.h_lbl_zoom_icon,     zoom_in_x,  y, zoom_icon_w, note_h);
        y += note_h + self.s(14);

        self.place_separator(&mut y, self.main_x, self.main_w, st.h_sep_h[2]);

        let btn_ab = self.s(200);
        self.set(st.crush.h_btn_toggle,
            self.main_x + (self.main_w - btn_ab) / 2, y, btn_ab, self.s(30));
        y += self.s(30) + self.s(6);
        // h_lbl_status (transient) and h_lbl_error (persistent) occupy the same
        // vertical slot and are shown mutually exclusively.  Both must be placed
        // here because they were previously left at position (0,0) / size (1×1),
        // making them invisible or causing overlap with other controls.
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(36), st.h_lbl_status, 0);
        self.set(st.h_lbl_error, self.main_x, y - self.s(36), self.main_w, self.s(36));

        y
    }

    fn place_taskbar_tab(&mut self, st: &mut crate::app::AppState, _hwnd: HWND) {
        let mut dy = self.s(8) + self.s(6);

        // Title then immediately the toggle — so the pill sits right next to the heading.
        self.place_with_gap(&mut dy, self.main_x, self.main_w, self.s(36), st.dimmer.h_lbl_dim_title,    self.s(4));
        self.place_with_gap(&mut dy, self.main_x, self.main_w, self.s(28), st.dimmer.h_chk_taskbar_dim,  self.s(4));
        // Sub-description as a small note below the toggle.
        self.place_with_gap(&mut dy, self.main_x, self.main_w, self.s(40), st.dimmer.h_lbl_dim_sub,      self.s(10));

        // ── "Dim Level" section ───────────────────────────────────────────────
        self.place_with_gap(&mut dy, self.main_x, self.main_w, self.s(20), st.dimmer.h_lbl_dim_sect, self.s(4));
        self.place_separator(&mut dy, self.main_x, self.main_w, st.dimmer.h_sep_dim_sect);

        let pct_w = self.s(56);
        self.set(st.dimmer.h_sld_taskbar_dim,
            self.main_x, dy, self.main_w - pct_w - self.s(4), self.s(40));
        self.set(st.dimmer.h_lbl_dim_pct,
            self.main_x + self.main_w - pct_w, dy, pct_w, self.s(40));
        dy += self.s(40) + self.s(14);

        // ── "Fade Timings" section ────────────────────────────────────────────
        self.place_with_gap(&mut dy, self.main_x, self.main_w, self.s(20), st.dimmer.h_lbl_fade_sect, self.s(4));
        self.place_separator(&mut dy, self.main_x, self.main_w, st.dimmer.h_sep_fade_sect);

        let fade_lbl_w = self.s(68);
        let fade_ms_w  = self.s(72);
        let fade_sld_w = self.main_w - fade_lbl_w - fade_ms_w - self.s(8);
        let fade_sld_x = self.main_x + fade_lbl_w + self.s(4);
        let fade_val_x = self.main_x + self.main_w - fade_ms_w;
        let fade_row_h = self.s(32);

        self.set(st.dimmer.h_lbl_fade_in_title,  self.main_x,   dy, fade_lbl_w, fade_row_h);
        self.set(st.dimmer.h_sld_fade_in,         fade_sld_x,   dy, fade_sld_w, fade_row_h);
        self.set(st.dimmer.h_lbl_fade_in_val,     fade_val_x,   dy, fade_ms_w,  fade_row_h);
        dy += fade_row_h + self.s(6);

        self.set(st.dimmer.h_lbl_fade_out_title,  self.main_x,  dy, fade_lbl_w, fade_row_h);
        self.set(st.dimmer.h_sld_fade_out,        fade_sld_x,   dy, fade_sld_w, fade_row_h);
        self.set(st.dimmer.h_lbl_fade_out_val,    fade_val_x,   dy, fade_ms_w,  fade_row_h);
        dy += fade_row_h + self.s(14);

        let def_btn_w = self.s(160);
        self.set(st.dimmer.h_btn_dim_defaults,
            self.main_x + (self.main_w - def_btn_w) / 2, dy, def_btn_w, self.s(26));
    }

    fn place_hotkeys_tab(&mut self, st: &mut crate::app::AppState, _hwnd: HWND) {
        let mut y = self.s(8) + self.s(6);

        // ── Title + description ───────────────────────────────────────────────
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(36),
            st.hotkeys.h_lbl_title, self.s(2));
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(48),
            st.hotkeys.h_lbl_desc, self.s(12));

        // ── Column metrics ────────────────────────────────────────────────────
        let clear_w = self.s(28);
        let gap_c   = self.s(4);   // gap between edit and clear button
        let gap_le  = self.s(8);   // gap between label and edit
        // Fixed label width; edit field capped so the pair stays close
        // together regardless of window width.
        let label_w = self.s(200);
        let edit_w  = self.s(140);
        let edit_x  = self.main_x + label_w + gap_le;
        let clear_x = edit_x + edit_w + gap_c;
        let row_h   = self.s(28);
        let row_gap = self.s(8);

        // ── Section helper closure ────────────────────────────────────────────
        // (inlined manually below — Rust closures can't borrow self + st together)

        // ── Black Crush Tweak section ─────────────────────────────────────────
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20),
            st.hotkeys.h_lbl_sect_crush, self.s(4));
        self.place_separator(&mut y, self.main_x, self.main_w,
            st.hotkeys.h_sep_sect[0]);

        // Rows 0–3: Black Crush hotkeys
        for i in 0..4 {
            let r = &st.hotkeys.rows[i];
            self.set(r.h_lbl,   self.main_x, y, label_w, row_h);
            self.set(r.h_edit,  edit_x,      y, edit_w,  row_h);
            self.set(r.h_clear, clear_x,     y, clear_w, row_h);
            y += row_h + row_gap;
        }

        y += self.s(6); // extra breathing room before next section

        // ── Taskbar Dimmer section ────────────────────────────────────────────
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20),
            st.hotkeys.h_lbl_sect_dimmer, self.s(4));
        self.place_separator(&mut y, self.main_x, self.main_w,
            st.hotkeys.h_sep_sect[1]);

        // Rows 4–6: Toggle, Decrease, Increase Dim Level
        for i in 4..7 {
            let r = &st.hotkeys.rows[i];
            self.set(r.h_lbl,   self.main_x, y, label_w, row_h);
            self.set(r.h_edit,  edit_x,      y, edit_w,  row_h);
            self.set(r.h_clear, clear_x,     y, clear_w, row_h);
            y += row_h + row_gap;
        }

        y += self.s(8);

        // ── Bottom separator ──────────────────────────────────────────────────
        self.place_separator(&mut y, self.main_x, self.main_w,
            st.hotkeys.h_sep_bottom);
    }

    fn place_debug_tab(&mut self, st: &mut crate::app::AppState, _hwnd: HWND) {
        let mut dy = self.s(8) + self.s(6);

        // ── Title ─────────────────────────────────────────────────────────────
        self.place_with_gap(&mut dy, self.main_x, self.main_w, self.s(36),
            st.debug.h_lbl_title, self.s(10));

        // ── "Dimmer State" section ────────────────────────────────────────────
        self.place_with_gap(&mut dy, self.main_x, self.main_w, self.s(20),
            st.debug.h_lbl_sect_state, self.s(4));
        self.place_separator(&mut dy, self.main_x, self.main_w, st.debug.h_sep_state);

        let key_w = self.s(180);
        let val_x = self.main_x + key_w + self.s(8);
        let val_w = self.main_w - key_w - self.s(8);
        let row_h = self.s(22);
        let gap   = self.s(6);

        // Five original state rows + overlay Z-order + taskbar Z-order.
        let rows = [
            (st.debug.h_lbl_fs_key,            st.debug.h_lbl_fs_val),
            (st.debug.h_lbl_ah_key,            st.debug.h_lbl_ah_val),
            (st.debug.h_lbl_alpha_key,         st.debug.h_lbl_alpha_val),
            (st.debug.h_lbl_target_key,        st.debug.h_lbl_target_val),
            (st.debug.h_lbl_overlays_key,      st.debug.h_lbl_overlays_val),
            (st.debug.h_lbl_zpos_key,          st.debug.h_lbl_zpos_val),
            (st.debug.h_lbl_taskbar_zpos_key,  st.debug.h_lbl_taskbar_zpos_val),
        ];
        for (key, val) in rows {
            self.set(key, self.main_x, dy, key_w, row_h);
            self.set(val, val_x,       dy, val_w, row_h);
            dy += row_h + gap;
        }

        // ── "Suppression" section ─────────────────────────────────────────────
        dy += self.s(6);
        self.place_with_gap(&mut dy, self.main_x, self.main_w, self.s(20),
            st.debug.h_lbl_sect_suppress, self.s(4));
        self.place_separator(&mut dy, self.main_x, self.main_w, st.debug.h_sep_suppress);

        let chk_h = self.s(24);
        self.place_with_gap(&mut dy, self.main_x, self.main_w, chk_h,
            st.debug.h_chk_suppress_fs, self.s(4));
        self.place_with_gap(&mut dy, self.main_x, self.main_w, chk_h,
            st.debug.h_chk_suppress_ah, self.s(10));

        // ── "Event Log" section ───────────────────────────────────────────────
        // "Clear" sits right-aligned on the same row as the heading so it is
        // always visible regardless of window height.
        let btn_h   = self.s(22);
        let clear_w = self.s(68);
        let gap_bc  = self.s(8);
        let head_w  = self.main_w - clear_w - gap_bc;

        self.set(st.debug.h_lbl_sect_log,
            self.main_x, dy, head_w, self.s(20));
        self.set(st.debug.h_btn_log_clear,
            self.main_x + head_w + gap_bc, dy, clear_w, btn_h);
        dy += self.s(20) + self.s(4);
        self.place_separator(&mut dy, self.main_x, self.main_w, st.debug.h_sep_log);

        // Log listbox: fill all remaining vertical space down to the bottom margin.
        let bottom = self.ch - self.s(14);
        let log_h  = (bottom - dy).max(self.s(60));
        self.set(st.debug.h_lst_zlog, self.main_x, dy, self.main_w, log_h);
    }


    fn place_about_tab(&mut self, st: &mut crate::app::AppState, _hwnd: HWND) {
        let mut y = self.s(8) + self.s(6);

        // ── Title ─────────────────────────────────────────────────────────────
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(36),
            st.about.h_lbl_title, self.s(10));

        // ── "Application" section ─────────────────────────────────────────────
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20),
            st.about.h_lbl_sect_about, self.s(4));
        self.place_separator(&mut y, self.main_x, self.main_w, st.about.h_sep_about);

        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20),
            st.about.h_lbl_version, self.s(6));
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20),
            st.about.h_lbl_link, self.s(14));

        // ── "Updates" section ─────────────────────────────────────────────────
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20),
            st.about.h_lbl_sect_update, self.s(4));
        self.place_separator(&mut y, self.main_x, self.main_w, st.about.h_sep_update);

        let btn_w = self.s(260);
        self.set(st.about.h_btn_check,
            self.main_x, y, btn_w, self.s(32));
        y += self.s(32) + self.s(8);

        // Info label below the button (empty until a check is triggered).
        self.place_with_gap(&mut y, self.main_x, self.main_w, self.s(20),
            st.about.h_lbl_check_info, 0);
    }

}