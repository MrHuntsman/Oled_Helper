// tab_about.rs — About tab (tab 4).
//
// Displays app name, version, a GitHub link, and a "Check for Updates" button
// whose functionality will be added later.  No persistent state, no INI writes.
//
// Style follows the conventions established in tab_crush.rs / tab_hotkeys.rs:
//   • 16pt bold Segoe UI  — tab title  (font_title)
//   • 11pt bold Segoe UI  — section headings  (font_sect, leaked like the other tabs)
//   • 10pt Segoe UI        — body labels / info  (font_normal, default)
//   • SS_BLACKRECT separators under every section heading
//   • SS_NOPREFIX on every static label that might contain '&'

#![allow(non_snake_case, unused_must_use)]

use windows::{
    core::*,
    Win32::{
        Foundation::*,
        Graphics::Gdi::HFONT,
        UI::WindowsAndMessaging::*,
    },
};

use crate::{
    constants::*,
    controls::ControlBuilder,
    ui_drawing::make_font,
    win32::ControlGroup,
};

pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GITHUB_URL:  &str = "https://github.com/placeholder/OledHelper";

// ── Tab state ──────────────────────────────────────────────────────────────────

pub struct AboutTab {
    // ── Title ─────────────────────────────────────────────────────────────────
    pub h_lbl_title:       HWND,

    // ── "About" section ───────────────────────────────────────────────────────
    pub h_lbl_sect_about:  HWND,
    pub h_sep_about:       HWND,
    pub h_lbl_version:     HWND,
    pub h_lbl_link:        HWND,

    // ── "Updates" section ─────────────────────────────────────────────────────
    pub h_lbl_sect_update: HWND,
    pub h_sep_update:      HWND,
    pub h_btn_check:       HWND,
    pub h_lbl_check_info:  HWND,

    pub group: ControlGroup,
}

impl AboutTab {
    /// # Safety
    /// Must be called on the same thread that owns `parent`.
    pub unsafe fn new(
        parent:      HWND,
        hinstance:   HINSTANCE,
        dpi:         u32,
        font_normal: HFONT,
        font_title:  HFONT,
    ) -> Self {
        let cb = ControlBuilder { parent, hinstance, dpi, font: font_normal };

        // ── Tab title (16pt bold) ─────────────────────────────────────────────
        let h_lbl_title = cb.static_text(w!("About"), 0);
        SendMessageW(h_lbl_title, WM_SETFONT,
            WPARAM(font_title.0 as usize), LPARAM(1));

        // ── Section headings: 11pt bold — matches tab_crush / tab_hotkeys ─────
        let font_sect = crate::ui_drawing::make_font_cached(w!("Segoe UI"), 11, dpi, true);
        // font_sect is cached and reused across DPI changes.

        // ── "About" section ───────────────────────────────────────────────────
        let h_lbl_sect_about = cb.static_text(w!("Application"), SS_NOPREFIX);
        SendMessageW(h_lbl_sect_about, WM_SETFONT,
            WPARAM(font_sect.0 as usize), LPARAM(1));
        let h_sep_about = cb.static_text(w!(""), SS_BLACKRECT);

        let ver_w: Vec<u16> = format!("Version {APP_VERSION}\0")
            .encode_utf16().collect();
        let h_lbl_version = cb.static_text(PCWSTR(ver_w.as_ptr()), SS_NOPREFIX);

        // GitHub link — styled as a URL via WM_CTLCOLORSTATIC (accent colour).
        let link_w: Vec<u16> = format!("{GITHUB_URL}\0")
            .encode_utf16().collect();
        let h_lbl_link = cb.static_text(PCWSTR(link_w.as_ptr()), SS_NOPREFIX);

        // ── "Updates" section ─────────────────────────────────────────────────
        let h_lbl_sect_update = cb.static_text(w!("Updates"), SS_NOPREFIX);
        SendMessageW(h_lbl_sect_update, WM_SETFONT,
            WPARAM(font_sect.0 as usize), LPARAM(1));
        let h_sep_update = cb.static_text(w!(""), SS_BLACKRECT);

        let h_btn_check = cb.button(w!("Check for available updates"), IDC_ABOUT_BTN_CHECK);

        // Small info label below the button (shows result once implemented).
        let h_lbl_check_info = cb.static_text(w!(""), SS_NOPREFIX);

        let group = ControlGroup::new(vec![
            h_lbl_title,
            h_lbl_sect_about, h_sep_about,
            h_lbl_version,
            h_lbl_link,
            h_lbl_sect_update, h_sep_update,
            h_btn_check,
            h_lbl_check_info,
        ]);

        // Hidden by default — only shown when tab 4 is active.
        group.set_visible(false);

        Self {
            h_lbl_title,
            h_lbl_sect_about, h_sep_about,
            h_lbl_version,
            h_lbl_link,
            h_lbl_sect_update, h_sep_update,
            h_btn_check,
            h_lbl_check_info,
            group,
        }
    }

    /// Called when the "Check for updates" button is pressed.
    /// Placeholder — real network logic goes here later.
    pub unsafe fn on_check_updates(&mut self) {
        // TODO: fetch latest release from GitHub API and compare with APP_VERSION.
        // For now, open the GitHub releases page in the default browser so the
        // user can check manually.
        let url: Vec<u16> = format!("{GITHUB_URL}/releases\0")
            .encode_utf16().collect();
        windows::Win32::UI::Shell::ShellExecuteW(
            HWND(std::ptr::null_mut()),
            w!("open"),
            PCWSTR(url.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
        );
    }
}