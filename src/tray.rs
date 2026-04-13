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
                HMENU, MF_SEPARATOR, MF_STRING,
            },
        },
    },
};

use crate::constants::WM_TRAY_CALLBACK;

/// Build the right-click popup menu. Caller owns the HMENU and must pass it
/// to `remove_tray_icon` for cleanup.
#[allow(unused_must_use)]
pub unsafe fn build_tray_menu() -> HMENU {
    let menu = CreatePopupMenu().unwrap_or_default();
    let show_w:    Vec<u16> = "Show\0"   .encode_utf16().collect();
    let restart_w: Vec<u16> = "Restart\0".encode_utf16().collect();
    let exit_w:    Vec<u16> = "Exit\0"   .encode_utf16().collect();
    AppendMenuW(menu, MF_STRING,    200, PCWSTR(show_w.as_ptr()));
    AppendMenuW(menu, MF_STRING,    201, PCWSTR(restart_w.as_ptr()));
    AppendMenuW(menu, MF_SEPARATOR, 0,   PCWSTR::null());
    AppendMenuW(menu, MF_STRING,    202, PCWSTR(exit_w.as_ptr()));
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