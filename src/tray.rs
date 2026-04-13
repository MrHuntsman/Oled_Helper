// tray.rs — System tray icon and popup menu.

use std::mem;

use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{HINSTANCE, HWND},
        UI::{
            Shell::{
                Shell_NotifyIconW,
                NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP,
                NIM_ADD, NIM_DELETE, NIM_MODIFY,
                NIIF_INFO,
                NOTIFYICONDATAW,
            },
            WindowsAndMessaging::{
                AppendMenuW, CreatePopupMenu, DestroyMenu,
                LoadIconW, IDI_APPLICATION,
                HMENU, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING,
                MENU_ITEM_FLAGS,
            },
        },
    },
};

use crate::constants::WM_TRAY_CALLBACK;

// ── Tray menu command IDs ─────────────────────────────────────────────────────
// 200–202 are the existing Show / Restart / Exit items handled in app.rs.
pub const TRAY_CMD_SHOW:          u32 = 200;
pub const TRAY_CMD_RESTART:       u32 = 201;
pub const TRAY_CMD_EXIT:          u32 = 202;
pub const TRAY_CMD_TOGGLE_DIMMER: u32 = 203;
pub const TRAY_CMD_TOGGLE_HDR:    u32 = 205;
/// Profile submenu items start at this ID.
/// Profile index = id - TRAY_CMD_PROFILE_BASE.
pub const TRAY_CMD_PROFILE_BASE:  u32 = 300;

// ── State snapshot ────────────────────────────────────────────────────────────

pub struct TrayMenuState<'a> {
    pub dimmer_on:      bool,
    pub hdr_on:         bool,
    pub hdr_avail:      bool,
    /// (section_name, display_label) pairs in display order.
    pub profiles:       &'a [(String, String)],
    pub active_profile: Option<usize>,
}

/// Build the right-click popup menu with current state baked in.
/// Call `DestroyMenu` on the old handle before replacing it, or pass the result
/// of the previous call to `remove_tray_icon` for cleanup on exit.
#[allow(unused_must_use)]
pub unsafe fn build_tray_menu(state: &TrayMenuState) -> HMENU {
    let menu = CreatePopupMenu().unwrap_or_default();

    // Show / management
    append_str(menu, MF_STRING, TRAY_CMD_SHOW,    "Show");
    append_str(menu, MF_STRING, TRAY_CMD_RESTART, "Restart");
    AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());

    // Quick-action toggles with checkmarks
    append_str(
        menu,
        MF_STRING | if state.dimmer_on { MF_CHECKED } else { MENU_ITEM_FLAGS(0) },
        TRAY_CMD_TOGGLE_DIMMER,
        "Taskbar Dimmer",
    );
    append_str(
        menu,
        MF_STRING
            | if state.hdr_on    { MF_CHECKED } else { MENU_ITEM_FLAGS(0) }
            | if !state.hdr_avail { MF_GRAYED  } else { MENU_ITEM_FLAGS(0) },
        TRAY_CMD_TOGGLE_HDR,
        "HDR",
    );

    // Profile submenu (only shown when profiles exist)
    if !state.profiles.is_empty() {
        AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let sub = CreatePopupMenu().unwrap_or_default();
        for (i, (_sec, label)) in state.profiles.iter().enumerate() {
            let id = TRAY_CMD_PROFILE_BASE + i as u32;
            append_str(
                sub,
                MF_STRING
                    | if state.active_profile == Some(i) { MF_CHECKED } else { MENU_ITEM_FLAGS(0) },
                id,
                label,
            );
        }
        let lbl: Vec<u16> = "Profile\0".encode_utf16().collect();
        AppendMenuW(menu, MF_STRING | MF_POPUP, sub.0 as usize, PCWSTR(lbl.as_ptr()));
    }

    AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    append_str(menu, MF_STRING, TRAY_CMD_EXIT, "Exit");

    menu
}

/// Register the app icon in the system tray and set `tray_added = true`.
#[allow(unused_must_use)]
pub unsafe fn add_tray_icon(hwnd: HWND, hinstance: HINSTANCE, tray_added: &mut bool) {
    let icon = LoadIconW(hinstance, w!("MAINICON"))
        .unwrap_or_else(|_| LoadIconW(None, IDI_APPLICATION).unwrap_or_default());

    let mut nid = NOTIFYICONDATAW {
        cbSize:           mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd:             hwnd,
        uID:              1,
        uFlags:           NIF_ICON | NIF_TIP | NIF_MESSAGE,
        uCallbackMessage: WM_TRAY_CALLBACK,
        hIcon:            icon,
        ..Default::default()
    };
    let tip: Vec<u16> = "Oled Helper".encode_utf16().collect();
    let copy_len = tip.len().min(127);
    nid.szTip[..copy_len].copy_from_slice(&tip[..copy_len]);
    Shell_NotifyIconW(NIM_ADD, &nid);
    *tray_added = true;
}

/// Update the tray tooltip to reflect current state.
/// E.g. "Oled Helper — Dimmer ON | Crush: +3 | HDR ON"
#[allow(unused_must_use)]
pub unsafe fn update_tray_tooltip(hwnd: HWND, dimmer_on: bool, crush_val: i32, hdr_on: bool) {
    let crush_str;
    let mut parts: Vec<&str> = Vec::new();
    if dimmer_on               { parts.push("Dimmer ON"); }
    if crush_val != 0          { crush_str = format!("Crush: {:+}", crush_val); parts.push(&crush_str); }
    if hdr_on                  { parts.push("HDR ON"); }

    let tip = if parts.is_empty() {
        "Oled Helper".to_string()
    } else {
        format!("Oled Helper — {}", parts.join(" | "))
    };

    let mut nid = NOTIFYICONDATAW {
        cbSize: mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd:   hwnd,
        uID:    1,
        uFlags: NIF_TIP,
        ..Default::default()
    };
    let tip_w: Vec<u16> = tip.encode_utf16().collect();
    let copy_len = tip_w.len().min(127);
    nid.szTip[..copy_len].copy_from_slice(&tip_w[..copy_len]);
    Shell_NotifyIconW(NIM_MODIFY, &nid);
}

/// Show a balloon notification that the app has minimized to tray.
/// Only call after `add_tray_icon` has succeeded.
#[allow(unused_must_use)]
pub unsafe fn show_tray_balloon(hwnd: HWND) {
    let mut nid = NOTIFYICONDATAW {
        cbSize:      mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd:        hwnd,
        uID:         1,
        uFlags:      NIF_INFO,
        dwInfoFlags: NIIF_INFO,
        ..Default::default()
    };

    let title: Vec<u16> = "Oled Helper\0".encode_utf16().collect();
    let msg:   Vec<u16> = "Oled Helper is still running in the system tray.\0"
        .encode_utf16().collect();

    let t_len = title.len().min(63);
    let m_len = msg.len().min(255);
    nid.szInfoTitle[..t_len].copy_from_slice(&title[..t_len]);
    nid.szInfo[..m_len].copy_from_slice(&msg[..m_len]);
    nid.Anonymous.uTimeout = 3000; // ignored on Vista+, kept for XP compat

    Shell_NotifyIconW(NIM_MODIFY, &nid);
}

/// Remove the tray icon and destroy the popup menu.
#[allow(unused_must_use)]
pub unsafe fn remove_tray_icon(hwnd: HWND, tray_menu: HMENU, tray_added: &mut bool) {
    if *tray_added {
        let nid = NOTIFYICONDATAW {
            cbSize: mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd:   hwnd,
            uID:    1,
            ..Default::default()
        }; 
        Shell_NotifyIconW(NIM_DELETE, &nid);
        *tray_added = false;
    }
    DestroyMenu(tray_menu);
}

// ── Internal helper ───────────────────────────────────────────────────────────

#[allow(unused_must_use)]
unsafe fn append_str(menu: HMENU, flags: MENU_ITEM_FLAGS, id: u32, text: &str) {
    let w: Vec<u16> = format!("{text}\0").encode_utf16().collect();
    AppendMenuW(menu, flags, id as usize, PCWSTR(w.as_ptr()));
}