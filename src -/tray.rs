// tray.rs — System-tray icon and popup menu helpers.

use std::mem;

use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{HINSTANCE, HWND},
        UI::{
            Shell::{Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW},
            WindowsAndMessaging::{
                AppendMenuW, CreatePopupMenu, DestroyMenu,
                LoadIconW, IDI_APPLICATION,
                HMENU, MF_SEPARATOR, MF_STRING,
            },
        },
    },
};

use crate::constants::WM_TRAY_CALLBACK;

/// Build the tray right-click popup menu.
/// Returns the HMENU — caller owns it and must pass it to `remove_tray_icon`
/// for cleanup via `DestroyMenu`.
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

/// Add the app icon to the system tray and set `tray_added` to `true`.
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

/// Remove the tray icon (if present) and destroy the popup menu.
#[allow(unused_must_use)]
pub unsafe fn remove_tray_icon(hwnd: HWND, tray_menu: HMENU, tray_added: &mut bool) {
    if *tray_added {
        let nid = NOTIFYICONDATAW {
            cbSize: mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            ..Default::default()
        };
        Shell_NotifyIconW(NIM_DELETE, &nid);
        *tray_added = false;
    }
    DestroyMenu(tray_menu);
}