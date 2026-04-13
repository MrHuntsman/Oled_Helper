// tab_system.rs — System tab (tab index 2): taskbar, power, and screensaver settings.

#![allow(non_snake_case, unused_must_use, dead_code)]

use windows::{
    core::*,
    Win32::{
        Foundation::*,
        Graphics::Gdi::HFONT,
        System::{
            Power::*,
            Registry::*,
        },
        UI::{
            Controls::*,
            Shell::*,
            WindowsAndMessaging::*,
        },
    },
};

use std::ptr;


use crate::{
    constants::{SS_BLACKRECT, SS_NOPREFIX, IDC_SYS_BTN_TASKBAR_AUTOHIDE},
    controls::ControlBuilder,
    ui_drawing::{combo_subclass_proc, make_font_cached, DefSubclassProc, SetWindowSubclass},
    win32::ControlGroup,
};


// Intercepts VK_RETURN to commit the typed value via the same EN_KILLFOCUS
// path the parent uses for focus-loss commits.
unsafe extern "system" fn ss_timeout_edit_subclass_proc(
    hwnd:    HWND,
    msg:     u32,
    wparam:  WPARAM,
    lparam:  LPARAM,
    _uid:    usize,
    _data:   usize,
) -> LRESULT {
    if msg == WM_KEYDOWN && wparam.0 == 0x0D /* VK_RETURN */ {
        // Post EN_KILLFOCUS to parent so commit_ss_timeout fires.
        let id  = GetDlgCtrlID(hwnd) as usize;
        let parent = GetParent(hwnd).unwrap_or_default();
        PostMessageW(parent, WM_COMMAND,
            WPARAM((0x0200 << 16) | (id & 0xFFFF)),
            LPARAM(hwnd.0 as isize));
        return LRESULT(0);
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

// ── Taskbar auto-hide ─────────────────────────────────────────────────────────
// ABM_GETSTATE / ABM_SETSTATE read/write ABS_AUTOHIDE (0x01) system-wide.

pub fn read_taskbar_autohide() -> bool {
    unsafe {
        let mut abd = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            ..Default::default()
        };
        let state = SHAppBarMessage(ABM_GETSTATE, &mut abd) as u32;
        state & ABS_AUTOHIDE != 0
    }
}

pub unsafe fn write_taskbar_autohide(enable: bool) {
    let mut abd = APPBARDATA {
        cbSize: std::mem::size_of::<APPBARDATA>() as u32,
        lParam: LPARAM(if enable { ABS_AUTOHIDE as isize } else { 0 }),
        ..Default::default()
    };
    SHAppBarMessage(ABM_SETSTATE, &mut abd);
}

// ── Power timeout API ─────────────────────────────────────────────────────────
// Reads/writes AC (plugged-in) values on the active scheme. Timeouts in
// seconds; 0 = Never. GUIDs are stable across Windows 10/11.

#[link(name = "kernel32")]
extern "system" {
    fn LocalFree(hmem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
}

/// Control IDs for the power-timeout comboboxes (matched in `app.rs::on_command`).
pub const IDC_SYS_DDL_SCREEN_TIMEOUT:  usize = 0x0A10;
pub const IDC_SYS_DDL_SLEEP_TIMEOUT:   usize = 0x0A11;
pub const IDC_SYS_DDL_SCREENSAVER:     usize = 0x0A14;
pub const IDC_SYS_EDT_SS_TIMEOUT:      usize = 0x0A13;

pub const TIMEOUT_OPTIONS: &[(&str, u32)] = &[
    ("Never",      0),
    ("1 minute",   60),
    ("2 minutes",  120),
    ("3 minutes",  180),
    ("5 minutes",  300),
    ("10 minutes", 600),
    ("15 minutes", 900),
    ("20 minutes", 1200),
    ("25 minutes", 1500),
    ("30 minutes", 1800),
    ("45 minutes", 2700),
    ("1 hour",     3600),
    ("2 hours",    7200),
    ("3 hours",    10800),
    ("4 hours",    14400),
    ("5 hours",    18000),
];

/// Returns the index in `TIMEOUT_OPTIONS` matching `seconds`, or 0 ("Never").
pub fn timeout_to_index(seconds: u32) -> usize {
    TIMEOUT_OPTIONS
        .iter()
        .position(|&(_, s)| s == seconds)
        .unwrap_or(0)
}

// ── Power GUIDs ───────────────────────────────────────────────────────────────
pub const GUID_VIDEO_SUBGROUP: GUID = GUID {
    data1: 0x7516b95f,
    data2: 0xf776,
    data3: 0x4464,
    data4: [0x8c, 0x53, 0x06, 0x16, 0x7f, 0x40, 0xcc, 0x99],
};

pub const GUID_VIDEO_POWERDOWN_TIMEOUT: GUID = GUID {
    data1: 0x3c0bc021,
    data2: 0xc8a8,
    data3: 0x4e07,
    data4: [0xa9, 0x73, 0x6b, 0x14, 0xcb, 0xcb, 0x2b, 0x7e],
};

pub const GUID_SLEEP_SUBGROUP: GUID = GUID {
    data1: 0x238c9fa8,
    data2: 0x0aad,
    data3: 0x41ed,
    data4: [0x83, 0xf4, 0x97, 0xbe, 0x24, 0x2c, 0x8f, 0x20],
};

pub const GUID_STANDBY_TIMEOUT: GUID = GUID {
    data1: 0x29f6c1db,
    data2: 0x86da,
    data3: 0x48c5,
    data4: [0x9f, 0xdb, 0xf2, 0xb6, 0x7b, 0x1f, 0x44, 0xda],
};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Returns the active power scheme GUID, or None on failure.
unsafe fn active_scheme() -> Option<GUID> {
    let mut scheme_ptr: *mut GUID = std::ptr::null_mut();
    if PowerGetActiveScheme(None, &mut scheme_ptr) == WIN32_ERROR(0) && !scheme_ptr.is_null() {
        let guid = *scheme_ptr;
        LocalFree(scheme_ptr as *mut std::ffi::c_void);
        Some(guid)
    } else {
        None
    }
}

/// Read an AC power timeout (seconds). Returns None on failure.
pub unsafe fn read_power_timeout(subgroup: &GUID, setting: &GUID) -> Option<u32> {
    let scheme = active_scheme()?;
    let mut value: u32 = 0;
    if PowerReadACValueIndex(
        None,
        Some(&scheme as *const GUID),
        Some(subgroup as *const GUID),
        Some(setting as *const GUID),
        &mut value,
    ) == WIN32_ERROR(0) {
        Some(value)
    } else {
        None
    }
}

/// Write an AC power timeout (seconds). Returns `(write_err, activate_err)`; both 0 on success.
pub unsafe fn write_power_timeout(subgroup: &GUID, setting: &GUID, seconds: u32) -> (u32, u32) {
    let scheme = match active_scheme() { Some(g) => g, None => return (u32::MAX, u32::MAX) };
    let write_err = PowerWriteACValueIndex(
        None,
        &scheme as *const GUID,
        Some(subgroup as *const GUID),
        Some(setting as *const GUID),
        seconds,
    ).0;
    if write_err != 0 {
        return (write_err, 0);
    }
    let activate_err = PowerSetActiveScheme(None, Some(&scheme as *const GUID)).0;
    (write_err, activate_err)
}

// ── Public wrappers ───────────────────────────────────────────────────────────
pub unsafe fn read_screen_timeout() -> Option<u32> {
    read_power_timeout(&GUID_VIDEO_SUBGROUP, &GUID_VIDEO_POWERDOWN_TIMEOUT)
}

pub unsafe fn read_sleep_timeout() -> Option<u32> {
    read_power_timeout(&GUID_SLEEP_SUBGROUP, &GUID_STANDBY_TIMEOUT)
}

pub unsafe fn write_screen_timeout(seconds: u32) -> (u32, u32) {
    write_power_timeout(&GUID_VIDEO_SUBGROUP, &GUID_VIDEO_POWERDOWN_TIMEOUT, seconds)
}

pub unsafe fn write_sleep_timeout(seconds: u32) -> (u32, u32) {
    write_power_timeout(&GUID_SLEEP_SUBGROUP, &GUID_STANDBY_TIMEOUT, seconds)
}

// ── Screensaver API ───────────────────────────────────────────────────────────
// SPI_GETSCREENSAVE* / SPI_SETSCREENSAVE* — user-session scoped, no reboot.

pub fn read_screensaver_active() -> bool {
    // SPI_GETSCREENSAVEACTIVE returns its BOOL result in pvParam.
    let mut active: i32 = 0;
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::SystemParametersInfoW(
            windows::Win32::UI::WindowsAndMessaging::SPI_GETSCREENSAVEACTIVE,
            0,
            Some(&mut active as *mut i32 as *mut _),
            windows::Win32::UI::WindowsAndMessaging::SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    active != 0
}

pub unsafe fn write_screensaver_active(enabled: bool) -> bool {
    windows::Win32::UI::WindowsAndMessaging::SystemParametersInfoW(
        windows::Win32::UI::WindowsAndMessaging::SPI_SETSCREENSAVEACTIVE,
        enabled as u32,
        None,
        windows::Win32::UI::WindowsAndMessaging::SPIF_UPDATEINIFILE
            | windows::Win32::UI::WindowsAndMessaging::SPIF_SENDCHANGE,
    ).is_ok()
}

/// Returns the screensaver timeout in seconds, or 0 on failure.
pub fn read_screensaver_timeout() -> u32 {
    let mut secs: u32 = 0;
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::SystemParametersInfoW(
            windows::Win32::UI::WindowsAndMessaging::SPI_GETSCREENSAVETIMEOUT,
            0,
            Some(&mut secs as *mut u32 as *mut _),
            windows::Win32::UI::WindowsAndMessaging::SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    secs
}

pub unsafe fn write_screensaver_timeout(seconds: u32) -> bool {
    windows::Win32::UI::WindowsAndMessaging::SystemParametersInfoW(
        windows::Win32::UI::WindowsAndMessaging::SPI_SETSCREENSAVETIMEOUT,
        seconds,
        None,
        windows::Win32::UI::WindowsAndMessaging::SPIF_UPDATEINIFILE
            | windows::Win32::UI::WindowsAndMessaging::SPIF_SENDCHANGE,
    ).is_ok()
}



// ── Screensaver enumeration ───────────────────────────────────────────────────

/// Read `HKCU\Control Panel\Desktop\SCRNSAVE.EXE`. Returns "" if absent.
pub fn read_screensaver_exe() -> String {
    let key_w:  Vec<u16> = "Control Panel\\Desktop\0".encode_utf16().collect();
    let val_w:  Vec<u16> = "SCRNSAVE.EXE\0".encode_utf16().collect();
    let mut hk = HKEY::default();
    unsafe {
        if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(key_w.as_ptr()),
            0, KEY_READ, &mut hk) != ERROR_SUCCESS
        {
            return String::new();
        }
        let mut buf = vec![0u8; 520]; // MAX_PATH * 2
        let mut size = buf.len() as u32;
        let mut kind = REG_VALUE_TYPE::default();
        let ok = RegQueryValueExW(hk, PCWSTR(val_w.as_ptr()), None, Some(&mut kind),
            Some(buf.as_mut_ptr()), Some(&mut size)) == ERROR_SUCCESS;
        RegCloseKey(hk);
        if !ok || size < 2 { return String::new(); }
        // REG_SZ stored as UTF-16 LE bytes.
        let words: Vec<u16> = buf[..size as usize]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let len = words.iter().position(|&c| c == 0).unwrap_or(words.len());
        String::from_utf16_lossy(&words[..len])
    }
}

/// Write `HKCU\Control Panel\Desktop\SCRNSAVE.EXE`. Pass "" to remove (disable).
pub unsafe fn write_screensaver_exe(path: &str) -> bool {
    let key_w: Vec<u16> = "Control Panel\\Desktop\0".encode_utf16().collect();
    let val_w: Vec<u16> = "SCRNSAVE.EXE\0".encode_utf16().collect();
    let mut hk = HKEY::default();
    if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(key_w.as_ptr()),
        0, KEY_WRITE, &mut hk) != ERROR_SUCCESS
    {
        return false;
    }
    let ok = if path.is_empty() {
        // Delete value — Windows then shows "(None)".
        RegDeleteValueW(hk, PCWSTR(val_w.as_ptr())) == ERROR_SUCCESS
    } else {
        // Store as REG_SZ (UTF-16 LE, null-terminated).
        let data: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        let bytes = std::slice::from_raw_parts(
            data.as_ptr() as *const u8,
            data.len() * 2,
        );
        RegSetValueExW(hk, PCWSTR(val_w.as_ptr()), 0, REG_SZ, Some(bytes))
            == ERROR_SUCCESS
    };
    RegCloseKey(hk);
    // Notify the system so the change takes effect immediately.
    windows::Win32::UI::WindowsAndMessaging::SystemParametersInfoW(
        windows::Win32::UI::WindowsAndMessaging::SPI_SETSCREENSAVEACTIVE,
        (!path.is_empty()) as u32,
        None,
        windows::Win32::UI::WindowsAndMessaging::SPIF_UPDATEINIFILE
            | windows::Win32::UI::WindowsAndMessaging::SPIF_SENDCHANGE,
    ).ok();
    ok
}

/// Returns `(display_name, full_path)` for all .scr files in System32/SysWOW64,
/// sorted by name. Index 0 is always `("(None)", "")`.
pub fn enumerate_screensavers() -> Vec<(String, String)> {
    let mut list = vec![("(None)".to_string(), String::new())];

    let dirs = [
        std::path::PathBuf::from(r"C:\Windows\System32"),
        std::path::PathBuf::from(r"C:\Windows\SysWOW64"),
    ];

    let mut seen = std::collections::HashSet::new();

    for dir in &dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("scr"))
                .unwrap_or(false)
            {
                let path_str = path.to_string_lossy().to_string();
                let stem = path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                // Deduplicate by stem (System32 and SysWOW64 share many files).
                if seen.insert(stem.to_lowercase()) {
                    // Use version resource FileDescription as display name.
                    let name = scr_display_name(&path_str).unwrap_or(stem);
                    list.push((name, path_str));
                }
            }
        }
    }

    list[1..].sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase())); // sort after (None)
    list
}

/// Extract `FileDescription` from a .scr version resource, or None if absent.
fn scr_display_name(path: &str) -> Option<String> {
    use windows::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
    };

    let wpath: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let mut dummy = 0u32;
        let size = GetFileVersionInfoSizeW(
            windows::core::PCWSTR(wpath.as_ptr()), Some(&mut dummy));
        if size == 0 { return None; }

        let mut buf = vec![0u8; size as usize];
        if GetFileVersionInfoW(
            windows::core::PCWSTR(wpath.as_ptr()), 0, size, buf.as_mut_ptr() as _
        ).is_err() { return None; }

        // Try English/Unicode sub-block first, then fallback blocks.
        for lang_cp in &[r"\StringFileInfo\040904B0\FileDescription",
                         r"\StringFileInfo\040904E4\FileDescription",
                         r"\StringFileInfo\000004B0\FileDescription"] {
            let subblock: Vec<u16> = lang_cp.encode_utf16().chain(Some(0)).collect();
            let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
            let mut len: u32 = 0;
            if VerQueryValueW(buf.as_ptr() as _, windows::core::PCWSTR(subblock.as_ptr()),
                &mut ptr, &mut len).as_bool() && len > 0 && !ptr.is_null()
            {
                let slice = std::slice::from_raw_parts(ptr as *const u16, (len - 1) as usize);
                let s = String::from_utf16_lossy(slice);
                if !s.trim().is_empty() { return Some(s.trim().to_string()); }
            }
        }
        None
    }
}

pub struct SystemTab {
    pub h_lbl_title:          HWND,
    pub h_lbl_desc:           HWND,
    // ── Taskbar section ───────────────────────────────────────────────────────
    pub h_lbl_sect_display:   HWND,
    pub h_sep_display:        HWND,
    pub h_btn_taskbar_autohide:    HWND,
    pub h_lbl_taskbar_autohide_st: HWND,
    pub taskbar_autohide_state:    bool,
    // ── Power section ────────────────────────────────────────────────────────
    pub h_lbl_sect_power:     HWND,
    pub h_sep_power:          HWND,
    pub h_lbl_screen_timeout: HWND,
    pub h_ddl_screen_timeout: HWND,
    pub h_lbl_sleep_timeout:  HWND,
    pub h_ddl_sleep_timeout:  HWND,
    // ── Screensaver section ───────────────────────────────────────────────────
    pub h_lbl_sect_screensaver:  HWND,
    pub h_sep_screensaver:       HWND,
    pub h_lbl_screensaver:       HWND,
    pub h_ddl_screensaver:       HWND,
    pub h_lbl_ss_timeout:        HWND,
    pub h_edt_ss_timeout:        HWND,
    pub h_spin_ss:               HWND,
    pub h_lbl_ss_minutes:        HWND,
    // ─────────────────────────────────────────────────────────────────────────
    pub group:                ControlGroup,
    pub screen_timeout_idx:   usize,
    pub sleep_timeout_idx:    usize,
    /// Index into screensavers vec; 0 = None.
    pub screensaver_idx:      usize,
    /// `(display_name, exe_path)`; index 0 is always `("(None)", "")`.
    pub screensavers:         Vec<(String, String)>,
    /// Suppresses EN_CHANGE while we programmatically update the edit text.
    pub suppress_en_change:   bool,
}

impl SystemTab {
    pub unsafe fn new(
        parent:      HWND,
        hinstance:   HINSTANCE,
        dpi:         u32,
        font_normal: HFONT,
        font_title:  HFONT,
    ) -> Self {
        let cb = ControlBuilder { parent, hinstance, dpi, font: font_normal };

        let h_lbl_title = cb.static_text(w!("System"), SS_NOPREFIX);
        SendMessageW(h_lbl_title, WM_SETFONT,
            WPARAM(font_title.0 as usize), LPARAM(1));

        let h_lbl_desc = cb.static_text(
            w!("Adjust various Windows system settings."), SS_NOPREFIX);

        let font_sect = make_font_cached(w!("Segoe UI"), 11, dpi, true);

        // ── Taskbar section ───────────────────────────────────────────────────
        let h_lbl_sect_display = cb.static_text(w!("Taskbar"), SS_NOPREFIX);
        SendMessageW(h_lbl_sect_display, WM_SETFONT,
            WPARAM(font_sect.0 as usize), LPARAM(1));

        let h_sep_display = cb.static_text(w!(""), SS_BLACKRECT);

        let taskbar_autohide_state = read_taskbar_autohide();
        let h_btn_taskbar_autohide = CreateWindowExW(
            WS_EX_LEFT, w!("BUTTON"), w!("Taskbar Auto-hide"),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP
                | WINDOW_STYLE((BS_AUTOCHECKBOX | BS_OWNERDRAW) as u32),
            0, 0, 1, 1, parent,
            HMENU(IDC_SYS_BTN_TASKBAR_AUTOHIDE as *mut _), hinstance, None,
        ).unwrap_or_default();
        SendMessageW(h_btn_taskbar_autohide, WM_SETFONT,
            WPARAM(font_normal.0 as usize), LPARAM(1));
        SendMessageW(h_btn_taskbar_autohide, BM_SETCHECK,
            WPARAM(taskbar_autohide_state as usize), LPARAM(0));
        let h_lbl_taskbar_autohide_st = cb.static_text(w!(""), SS_NOPREFIX);
        Self::apply_autohide_label(h_lbl_taskbar_autohide_st, taskbar_autohide_state);

        // ── Power section ─────────────────────────────────────────────────────
        let h_lbl_sect_power = cb.static_text(w!("Power Options"), SS_NOPREFIX);
        SendMessageW(h_lbl_sect_power, WM_SETFONT,
            WPARAM(font_sect.0 as usize), LPARAM(1));

        let h_sep_power           = cb.static_text(w!(""), SS_BLACKRECT);
        let h_lbl_screen_timeout  = cb.static_text(w!("Turn off screen after"), SS_NOPREFIX);
        let h_lbl_sleep_timeout   = cb.static_text(w!("Sleep after"), SS_NOPREFIX);

        // CBS_DROPDOWNLIST | WS_VSCROLL (raw values — not in windows-rs consts).
        const CBS_DROPDOWNLIST: u32 = 0x0003;
        const WS_VSCROLL: u32       = 0x00200000;
        let ddl_style = WINDOW_STYLE(
            CBS_DROPDOWNLIST | WS_VSCROLL
            | WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0
        );
        let h_ddl_screen_timeout = CreateWindowExW(
            WINDOW_EX_STYLE(0), w!("COMBOBOX"), PCWSTR::null(),
            ddl_style, 0, 0, 0, 0,
            parent, HMENU(IDC_SYS_DDL_SCREEN_TIMEOUT as _), hinstance, None,
        ).unwrap_or_default();
        let h_ddl_sleep_timeout = CreateWindowExW(
            WINDOW_EX_STYLE(0), w!("COMBOBOX"), PCWSTR::null(),
            ddl_style, 0, 0, 0, 0,
            parent, HMENU(IDC_SYS_DDL_SLEEP_TIMEOUT as _), hinstance, None,
        ).unwrap_or_default();
        SendMessageW(h_ddl_screen_timeout, WM_SETFONT,
            WPARAM(font_normal.0 as usize), LPARAM(1));
        SendMessageW(h_ddl_sleep_timeout,  WM_SETFONT,
            WPARAM(font_normal.0 as usize), LPARAM(1));

        // Owner-draw subclass + item heights (matches other tab combo style).
        SetWindowSubclass(h_ddl_screen_timeout, Some(combo_subclass_proc), 1, 0);
        SetWindowSubclass(h_ddl_sleep_timeout,  Some(combo_subclass_proc), 1, 0);
        let item_h = (20 * dpi / 96) as isize;
        SendMessageW(h_ddl_screen_timeout, CB_SETITEMHEIGHT, WPARAM(usize::MAX), LPARAM(item_h));
        SendMessageW(h_ddl_screen_timeout, CB_SETITEMHEIGHT, WPARAM(0),          LPARAM(item_h));
        SendMessageW(h_ddl_sleep_timeout,  CB_SETITEMHEIGHT, WPARAM(usize::MAX), LPARAM(item_h));
        SendMessageW(h_ddl_sleep_timeout,  CB_SETITEMHEIGHT, WPARAM(0),          LPARAM(item_h));

        for &(label, _) in TIMEOUT_OPTIONS {
            let lw: Vec<u16> = label.encode_utf16().chain([0]).collect();
            SendMessageW(h_ddl_screen_timeout, CB_ADDSTRING, WPARAM(0),
                LPARAM(lw.as_ptr() as isize));
            SendMessageW(h_ddl_sleep_timeout,  CB_ADDSTRING, WPARAM(0),
                LPARAM(lw.as_ptr() as isize));
        }

        let screen_timeout_idx = read_screen_timeout()
            .map(timeout_to_index).unwrap_or(0);
        let sleep_timeout_idx  = read_sleep_timeout()
            .map(timeout_to_index).unwrap_or(0);

        SendMessageW(h_ddl_screen_timeout, CB_SETCURSEL,
            WPARAM(screen_timeout_idx), LPARAM(0));
        SendMessageW(h_ddl_sleep_timeout,  CB_SETCURSEL,
            WPARAM(sleep_timeout_idx),  LPARAM(0));

        // ── Screensaver section ───────────────────────────────────────────────
        let h_lbl_sect_screensaver = cb.static_text(w!("Screensaver Options"), SS_NOPREFIX);
        SendMessageW(h_lbl_sect_screensaver, WM_SETFONT,
            WPARAM(font_sect.0 as usize), LPARAM(1));

        let h_sep_screensaver  = cb.static_text(w!(""), SS_BLACKRECT);
        let h_lbl_screensaver  = cb.static_text(w!("Screen saver"), SS_NOPREFIX);
        let h_ddl_screensaver  = cb.combobox(IDC_SYS_DDL_SCREENSAVER);
        SetWindowSubclass(h_ddl_screensaver,    Some(combo_subclass_proc), 1, 0);
        SendMessageW(h_ddl_screensaver,    CB_SETITEMHEIGHT, WPARAM(usize::MAX), LPARAM(item_h));
        SendMessageW(h_ddl_screensaver,    CB_SETITEMHEIGHT, WPARAM(0),          LPARAM(item_h));
        let h_lbl_ss_timeout   = cb.static_text(w!("Wait"), SS_NOPREFIX);
        let h_lbl_ss_minutes   = cb.static_text(w!("minutes"), SS_NOPREFIX);

        // Numeric edit (ES_NUMBER) + updown spinner for screensaver timeout (1–999 min).
        let ss_secs = read_screensaver_timeout();
        let ss_mins = (ss_secs / 60).max(1);
        let h_edt_ss_timeout = {
            let init: Vec<u16> = format!("{}\0", ss_mins).encode_utf16().collect();
            CreateWindowExW(
                WS_EX_LEFT,
                w!("EDIT"), PCWSTR(init.as_ptr()),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP
                    | WINDOW_STYLE((ES_LEFT | ES_NUMBER | ES_AUTOHSCROLL) as u32),
                0, 0, 1, 1,
                parent, HMENU(IDC_SYS_EDT_SS_TIMEOUT as _), hinstance, None,
            ).unwrap_or_default()
        };
        SendMessageW(h_edt_ss_timeout, WM_SETFONT,
            WPARAM(font_normal.0 as usize), LPARAM(1));
        // Subclass: Enter commits value (same path as focus loss).
        SetWindowSubclass(h_edt_ss_timeout, Some(ss_timeout_edit_subclass_proc),
            IDC_SYS_EDT_SS_TIMEOUT, 0);

        const UDS_WRAP:         u32 = 0x0001;
        const UDS_SETBUDDYINT:  u32 = 0x0002;
        const UDS_ALIGNRIGHT:   u32 = 0x0004;
        const UDS_AUTOBUDDY:    u32 = 0x0008;
        const UDS_ARROWKEYS:    u32 = 0x0020;
        const UDS_NOTHOUSANDS:  u32 = 0x0080;
        let h_spin_ss = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("msctls_updown32"), w!(""),
            WS_CHILD | WS_VISIBLE
                | WINDOW_STYLE(UDS_AUTOBUDDY | UDS_ALIGNRIGHT
                               | UDS_ARROWKEYS | UDS_NOTHOUSANDS),
            0, 0, 0, 0,
            parent, HMENU(ptr::null_mut()), hinstance, None,
        ).unwrap_or_default();
        // UDM_SETRANGE32 avoids the 16-bit limit of UDM_SETRANGE.
        SendMessageW(h_spin_ss, windows::Win32::UI::Controls::UDM_SETRANGE32,
            WPARAM(1), LPARAM(999));
        SendMessageW(h_spin_ss, windows::Win32::UI::Controls::UDM_SETPOS32,
            WPARAM(0), LPARAM(ss_mins as isize));

        let screensavers = enumerate_screensavers();
        let current_exe  = read_screensaver_exe().to_lowercase();
        let screensaver_idx = screensavers.iter().position(|(_, p)| {
            // Match on filename only — paths can differ between System32/SysWOW64.
            let scr_file = std::path::Path::new(p)
                .file_name().and_then(|f| f.to_str()).unwrap_or("").to_lowercase();
            let cur_file = std::path::Path::new(&current_exe)
                .file_name().and_then(|f| f.to_str()).unwrap_or("").to_lowercase();
            !p.is_empty() && !cur_file.is_empty() && scr_file == cur_file
        }).unwrap_or(0);

        for (name, _) in &screensavers {
            let lw: Vec<u16> = format!("{}\0", name).encode_utf16().collect();
            SendMessageW(h_ddl_screensaver, CB_ADDSTRING, WPARAM(0), LPARAM(lw.as_ptr() as isize));
        }
        SendMessageW(h_ddl_screensaver, CB_SETCURSEL, WPARAM(screensaver_idx), LPARAM(0));

        // Disable wait controls when no screensaver selected; static labels stay enabled.
        let ss_enabled = screensaver_idx != 0;
        windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(h_edt_ss_timeout, ss_enabled);
        windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(h_spin_ss,        ss_enabled);

        let group = ControlGroup::new(vec![
            h_lbl_title,
            h_lbl_desc,
            h_lbl_sect_display, h_sep_display,
            h_btn_taskbar_autohide, h_lbl_taskbar_autohide_st,
            h_lbl_sect_power, h_sep_power,
            h_lbl_screen_timeout, h_ddl_screen_timeout,
            h_lbl_sleep_timeout,  h_ddl_sleep_timeout,
            h_lbl_sect_screensaver, h_sep_screensaver,
            h_lbl_screensaver, h_ddl_screensaver,
            h_lbl_ss_timeout, h_edt_ss_timeout, h_spin_ss, h_lbl_ss_minutes,
        ]);
        group.set_visible(false);

        Self {
            h_lbl_title,
            h_lbl_desc,
            h_lbl_sect_display, h_sep_display,
            h_btn_taskbar_autohide, h_lbl_taskbar_autohide_st,
            taskbar_autohide_state,
            h_lbl_sect_power, h_sep_power,
            h_lbl_screen_timeout, h_ddl_screen_timeout,
            h_lbl_sleep_timeout,  h_ddl_sleep_timeout,
            h_lbl_sect_screensaver, h_sep_screensaver,
            h_lbl_screensaver, h_ddl_screensaver,
            h_lbl_ss_timeout, h_edt_ss_timeout, h_spin_ss, h_lbl_ss_minutes,
            group,
            screen_timeout_idx,
            sleep_timeout_idx,
            screensaver_idx,
            screensavers,
            suppress_en_change: false,
        }
    }


    pub unsafe fn refresh(&mut self) {
        self.taskbar_autohide_state = read_taskbar_autohide();
        SendMessageW(self.h_btn_taskbar_autohide, BM_SETCHECK,
            WPARAM(self.taskbar_autohide_state as usize), LPARAM(0));
        Self::apply_autohide_label(self.h_lbl_taskbar_autohide_st, self.taskbar_autohide_state);

        self.screen_timeout_idx = read_screen_timeout()
            .map(timeout_to_index).unwrap_or(0);
        self.sleep_timeout_idx  = read_sleep_timeout()
            .map(timeout_to_index).unwrap_or(0);
        SendMessageW(self.h_ddl_screen_timeout, CB_SETCURSEL,
            WPARAM(self.screen_timeout_idx), LPARAM(0));
        SendMessageW(self.h_ddl_sleep_timeout,  CB_SETCURSEL,
            WPARAM(self.sleep_timeout_idx),  LPARAM(0));

        // Re-read from registry (more reliable than SPI_GETSCREENSAVER).
        let current_exe = read_screensaver_exe().to_lowercase();
        self.screensaver_idx = self.screensavers.iter().position(|(_, p)| {
            let scr_file = std::path::Path::new(p)
                .file_name().and_then(|f| f.to_str()).unwrap_or("").to_lowercase();
            let cur_file = std::path::Path::new(&current_exe)
                .file_name().and_then(|f| f.to_str()).unwrap_or("").to_lowercase();
            !p.is_empty() && !cur_file.is_empty() && scr_file == cur_file
        }).unwrap_or(0);
        SendMessageW(self.h_ddl_screensaver, CB_SETCURSEL,
            WPARAM(self.screensaver_idx), LPARAM(0));

        let ss_secs = read_screensaver_timeout();
        let ss_mins = (ss_secs / 60).max(1);
        let txt: Vec<u16> = format!("{}\0", ss_mins).encode_utf16().collect();
        SetWindowTextW(self.h_edt_ss_timeout, PCWSTR(txt.as_ptr()));
        SendMessageW(self.h_spin_ss, windows::Win32::UI::Controls::UDM_SETPOS32,
            WPARAM(0), LPARAM(ss_mins as isize));
        let ss_enabled = self.screensaver_idx != 0;
        windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(self.h_edt_ss_timeout, ss_enabled);
        windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(self.h_spin_ss,        ss_enabled);
    }


    /// Called from WM_COMMAND when `IDC_SYS_DDL_SCREEN_TIMEOUT` fires CBN_SELCHANGE.
    pub unsafe fn on_screen_timeout_changed(&mut self) -> String {
        let sel = SendMessageW(self.h_ddl_screen_timeout,
            CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 as usize;
        if sel >= TIMEOUT_OPTIONS.len() { return "Invalid selection".into(); }
        self.screen_timeout_idx = sel;
        let seconds = TIMEOUT_OPTIONS[sel].1;
        let (write_err, activate_err) = write_screen_timeout(seconds);
        if write_err == 0 && activate_err == 0 {
            "Screen timeout updated".into()
        } else {
            format!("Screen timeout failed: write={write_err} activate={activate_err}")
        }
    }

    /// Called from WM_COMMAND when `IDC_SYS_DDL_SLEEP_TIMEOUT` fires CBN_SELCHANGE.
    pub unsafe fn on_sleep_timeout_changed(&mut self) -> String {
        let sel = SendMessageW(self.h_ddl_sleep_timeout,
            CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 as usize;
        if sel >= TIMEOUT_OPTIONS.len() { return "Invalid selection".into(); }
        self.sleep_timeout_idx = sel;
        let seconds = TIMEOUT_OPTIONS[sel].1;
        let (write_err, activate_err) = write_sleep_timeout(seconds);
        if write_err == 0 && activate_err == 0 {
            "Sleep timeout updated".into()
        } else {
            format!("Sleep timeout failed: write={write_err} activate={activate_err}")
        }
    }

    /// Called from WM_COMMAND when `IDC_SYS_DDL_SCREENSAVER` fires CBN_SELCHANGE.
    pub unsafe fn on_screensaver_changed(&mut self) -> String {
        let sel = SendMessageW(self.h_ddl_screensaver,
            CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 as usize;
        if sel >= self.screensavers.len() { return "Invalid selection".into(); }
        
        let was_enabled = self.screensaver_idx != 0;
        self.screensaver_idx = sel;
        let path = &self.screensavers[sel].1.clone();
        let ok = write_screensaver_exe(path);
        // Only toggle if state changed — avoids flicker.
        let ss_enabled = sel != 0;
        if ss_enabled != was_enabled {
            windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(self.h_edt_ss_timeout, ss_enabled);
            windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(self.h_spin_ss,        ss_enabled);
        }
        if ok {
            if sel == 0 { "Screensaver disabled".into() }
            else        { format!("Screensaver set to {}", self.screensavers[sel].0) }
        } else {
            "Screensaver: registry write failed".into()
        }
    }

    /// Called from WM_NOTIFY / UDN_DELTAPOS with the pre-computed new value.
    /// Updates edit text immediately rather than waiting for UDS_SETBUDDYINT.
    pub unsafe fn on_ss_timeout_set(&mut self, mins: u32) -> String {
        let mins = mins.clamp(1, 999);
        // Update edit immediately — UDS_SETBUDDYINT fires after DefWindowProcW returns.
        let txt: Vec<u16> = format!("{}\0", mins).encode_utf16().collect();
        SetWindowTextW(self.h_edt_ss_timeout, PCWSTR(txt.as_ptr()));
        // Sync spinner iPos AFTER SetWindowTextW so it doesn't overwrite the edit.
        SendMessageW(self.h_spin_ss,
            windows::Win32::UI::Controls::UDM_SETPOS32,
            WPARAM(0), LPARAM(mins as isize));
        // Place caret at end after programmatic update.
        let end = mins.to_string().len();
        SendMessageW(self.h_edt_ss_timeout, EM_SETSEL, WPARAM(end), LPARAM(end as isize));
        let seconds = mins * 60;
        if write_screensaver_timeout(seconds) {
            format!("Screensaver timeout set to {} min", mins)
        } else {
            "Screensaver timeout: SystemParametersInfo failed".into()
        }
    }


    /// Commit the current edit field value to the system. Called on focus loss or Enter key.
    pub unsafe fn commit_ss_timeout(&mut self) -> String {
        let mut buf = [0u16; 8];
        let len = GetWindowTextW(self.h_edt_ss_timeout, &mut buf) as usize;
        let s = String::from_utf16_lossy(&buf[..len]);
        let mins: u32 = match s.trim().parse::<u32>() {
            Ok(v) if v >= 1 => v.min(999),
            // Invalid — restore last known good value from system.
            _ => {
                let secs = read_screensaver_timeout();
                let m = (secs / 60).max(1);
                let txt: Vec<u16> = format!("{}\0", m).encode_utf16().collect();
                SetWindowTextW(self.h_edt_ss_timeout, PCWSTR(txt.as_ptr()));
                SendMessageW(self.h_spin_ss,
                    windows::Win32::UI::Controls::UDM_SETPOS32,
                    WPARAM(0), LPARAM(m as isize));
                return String::new();
            }
        };
        // Normalise text (strip leading zeros) and sync spinner.
        let txt: Vec<u16> = format!("{}\0", mins).encode_utf16().collect();
        SetWindowTextW(self.h_edt_ss_timeout, PCWSTR(txt.as_ptr()));
        SendMessageW(self.h_spin_ss,
            windows::Win32::UI::Controls::UDM_SETPOS32,
            WPARAM(0), LPARAM(mins as isize));
        let seconds = mins * 60;
        if write_screensaver_timeout(seconds) {
            format!("Screensaver timeout set to {} min", mins)
        } else {
            "Screensaver timeout: SystemParametersInfo failed".into()
        }
    }

    pub unsafe fn on_toggle_taskbar_autohide(&mut self) -> &'static str {
        let new_state = !self.taskbar_autohide_state;
        write_taskbar_autohide(new_state);
        self.taskbar_autohide_state = new_state;
        Self::apply_autohide_label(self.h_lbl_taskbar_autohide_st, new_state);
        SendMessageW(self.h_btn_taskbar_autohide, BM_SETCHECK,
            WPARAM(new_state as usize), LPARAM(0));
        if new_state { "Taskbar auto-hide enabled" } else { "Taskbar auto-hide disabled" }
    }

    fn apply_autohide_label(hwnd: HWND, state: bool) {
        let text: Vec<u16> = if state { "On\0" } else { "Off\0" }
            .encode_utf16().collect();
        unsafe { SetWindowTextW(hwnd, PCWSTR(text.as_ptr())); }
    }

}