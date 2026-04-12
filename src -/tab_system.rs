// tab_system.rs — System tab (tab index 2).

#![allow(non_snake_case, unused_must_use, dead_code)]

use windows::{
    core::*,
    Win32::{
        Foundation::*,
        Graphics::Gdi::HFONT,
        System::{
            Power::*,
            Registry::*,
            SystemInformation::GetTickCount64,
        },
        UI::{
            Controls::*,
            Shell::*,
            WindowsAndMessaging::*,
        },
    },
};

use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{
    constants::{SS_BLACKRECT, SS_NOPREFIX, IDC_SYS_BTN_TASKBAR_AUTOHIDE,
                IDC_SYS_DDL_CURSOR_HIDE, TIMER_CURSOR_HIDE},
    controls::ControlBuilder,
    ui_drawing::{combo_subclass_proc, make_font_cached, DefSubclassProc, SetWindowSubclass},
    win32::ControlGroup,
};


// ── Cursor-hide idle tracking ─────────────────────────────────────────────────
//
// The WH_MOUSE_LL hook proc (hotkeys.rs) writes to this on every WM_MOUSEMOVE.
// TIMER_CURSOR_HIDE reads it; no lock needed — single UI thread reads, hook
// thread writes.  Relaxed ordering is fine: we only care about the value being
// "recent enough", not about happens-before with any other memory.
//
// Stored as milliseconds from GetTickCount64 (wraps every ~49 days; the delta
// comparison is safe across the wrap because we use wrapping subtraction).
// CURSOR_LAST_MOVE_MS / cursor_touch() are kept for potential future use;
// idle detection now uses GetLastInputInfo (no hook dependency).
pub static CURSOR_LAST_MOVE_MS: AtomicU64 = AtomicU64::new(u64::MAX);

/// Set to true by on_cursor_hide_tick when cursors are blanked.
/// Read by the WH_MOUSE_LL hook proc to restore immediately on movement
/// without waiting for the next 1-second timer tick.
pub static CURSOR_HIDDEN: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Call from the LL mouse hook proc on every WM_MOUSEMOVE (and optionally
/// WM_LBUTTONDOWN etc. if you want clicks to also reset the idle clock).
#[inline(always)]
pub fn cursor_touch() {
    CURSOR_LAST_MOVE_MS.store(
        unsafe { GetTickCount64() },
        Ordering::Relaxed,
    );
}

/// Create a 1×1 fully-transparent cursor at runtime.
/// The system takes ownership of the HCURSOR returned by SetSystemCursor, so
/// we must hand it a *fresh* copy each time — never re-use the same handle.
unsafe fn make_blank_cursor() -> HCURSOR {
    // AND mask all 1s (transparent), XOR mask all 0s (no colour) → invisible.
    let and_mask = [0xFFu8; 4]; // 1×1 at 1 bpp = 1 byte; pad to DWORD
    let xor_mask = [0x00u8; 4];
    CreateCursor(
        None,          // not tied to any module
        0, 0,          // hotspot
        1, 1,          // 1×1
        and_mask.as_ptr() as _,
        xor_mask.as_ptr() as _,
    ).unwrap_or_default()
}

/// Restore all system cursors from the current Windows theme/registry.
/// Safe to call even if the cursor was never hidden.
pub unsafe fn restore_system_cursors() {
    windows::Win32::UI::WindowsAndMessaging::SystemParametersInfoW(
        windows::Win32::UI::WindowsAndMessaging::SPI_SETCURSORS,
        0,
        None,
        windows::Win32::UI::WindowsAndMessaging::SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
    ).ok();
}


//
// Intercepts VK_RETURN so pressing Enter commits the typed value via the same
// EN_KILLFOCUS (0x0200) WM_COMMAND path the parent uses for focus-loss commits.
// DefSubclassProc handles everything else (digits, backspace, arrow keys, etc.).
unsafe extern "system" fn ss_timeout_edit_subclass_proc(
    hwnd:    HWND,
    msg:     u32,
    wparam:  WPARAM,
    lparam:  LPARAM,
    _uid:    usize,
    _data:   usize,
) -> LRESULT {
    if msg == WM_KEYDOWN && wparam.0 == 0x0D /* VK_RETURN */ {
        // Post EN_KILLFOCUS (0x0200) to the parent so commit_ss_timeout fires.
        let id  = GetDlgCtrlID(hwnd) as usize;
        let parent = GetParent(hwnd).unwrap_or_default();
        PostMessageW(parent, WM_COMMAND,
            WPARAM((0x0200 << 16) | (id & 0xFFFF)),
            LPARAM(hwnd.0 as isize));
        return LRESULT(0);
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

// ── Taskbar auto-hide API ─────────────────────────────────────────────────────
//
// SHAppBarMessage with ABM_GETSTATE / ABM_SETSTATE reads and writes the
// taskbar auto-hide flag (ABS_AUTOHIDE = 0x01) system-wide.
// The APPBARDATA struct must be zero-initialised with cbSize set; all other
// fields are unused for these two messages.

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
//
// We read/write AC (plugged-in) power plan values only, using the active scheme.
// Both timeouts are in seconds; 0 means "Never".
//
// GUIDs are stable across all Windows 10/11 versions:
//   GUID_VIDEO_SUBGROUP          = 7516b95f-f776-4464-8c53-06167f40cc99
//   GUID_VIDEO_POWERDOWN_TIMEOUT = 3c0bc021-c8a8-4e07-a973-6b14cbcb2b7e
//   GUID_STANDBY_SUBGROUP        = 238c9fa8-0aad-41ed-83f4-97be242c8f20  (= GUID_SLEEP_SUBGROUP)
//   GUID_STANDBY_TIMEOUT         = 29f6c1db-86da-48c5-9fdb-f2b67b1f44da  (= GUID_STANDBY_TIMEOUT)

#[link(name = "kernel32")]
extern "system" {
    fn LocalFree(hmem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
}

/// `(label, seconds)` — 0 = Never.
/// Control IDs for the power-timeout comboboxes.
/// Declared here so `app.rs` can match them in `on_command`.
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

/// `(label, seconds)` pairs for the cursor-hide dropdown.
/// Index 0 = "Never" (feature disabled — timer not armed).
pub const CURSOR_HIDE_OPTIONS: &[(&str, u32)] = &[
    ("Never",      0),
    ("5 seconds",  5),
    ("15 seconds", 15),
    ("30 seconds", 30),
    ("1 minute",   60),
    ("2 minutes",  120),
    ("5 minutes",  300),
];

pub fn cursor_hide_to_index(seconds: u32) -> usize {
    CURSOR_HIDE_OPTIONS
        .iter()
        .position(|&(_, s)| s == seconds)
        .unwrap_or(0)
}

/// Returns the index into `TIMEOUT_OPTIONS` whose seconds value is the
/// closest match to `seconds`, or 0 ("Never") if nothing matches.
pub fn timeout_to_index(seconds: u32) -> usize {
    TIMEOUT_OPTIONS
        .iter()
        .position(|&(_, s)| s == seconds)
        .unwrap_or(0)
}

// GUIDs -----------------------------------------------------------------------

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

// Helpers ---------------------------------------------------------------------

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

/// Read an AC power index (seconds). Returns None if the call fails.
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

/// Write an AC power index (seconds).
/// Returns `(write_err, activate_err)` — both 0 on full success.
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

// Public wrappers -------------------------------------------------------------

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
//
// SystemParametersInfo with SPI_GETSCREENSAVEACTIVE / SPI_SETSCREENSAVEACTIVE
// reads/writes whether the screensaver is enabled at all.
// SPI_GETSCREENSAVETIMEOUT / SPI_SETSCREENSAVETIMEOUT reads/writes the idle
// delay in seconds.  Both calls are user-session scoped and take effect
// immediately without a reboot.

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
/// SPI_GETSCREENSAVETIMEOUT returns its result in pvParam (a *mut u32).
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

/// Read the current screensaver exe path from the registry.
/// `HKCU\Control Panel\Desktop` → `SCRNSAVE.EXE`
/// Returns empty string if none is set or the value is absent.
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
        // The value is REG_SZ stored as UTF-16 LE bytes.
        let words: Vec<u16> = buf[..size as usize]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let len = words.iter().position(|&c| c == 0).unwrap_or(words.len());
        String::from_utf16_lossy(&words[..len])
    }
}

/// Write the screensaver exe path to the registry.
/// `HKCU\Control Panel\Desktop` → `SCRNSAVE.EXE`
/// Pass empty string to remove the value (disable screensaver).
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
        // No screensaver — delete the value so Windows shows "(None)".
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

/// Enumerate all .scr files in System32 (and SysWOW64 on 64-bit).
/// Returns vec of (display_name, full_path), sorted by display_name.
/// Index 0 is always ("(None)", "").
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
                // Deduplicate by stem (System32 and SysWOW64 often have the same files).
                if seen.insert(stem.to_lowercase()) {
                    // Try to get the friendly name from the version resource description.
                    let name = scr_display_name(&path_str).unwrap_or(stem);
                    list.push((name, path_str));
                }
            }
        }
    }

    // Sort everything after index 0 alphabetically.
    list[1..].sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    list
}

/// Extract a friendly display name from a .scr file's version resource.
/// Falls back to None if the resource is absent or can't be read.
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

        // Try the English (0409) / Unicode (04B0) sub-block first, then any block.
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
    // ── Display section ───────────────────────────────────────────────────────
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
    // ── Mouse section ─────────────────────────────────────────────────────────
    pub h_lbl_sect_mouse:        HWND,
    pub h_sep_mouse:             HWND,
    pub h_lbl_cursor_hide:       HWND,
    pub h_ddl_cursor_hide:       HWND,
    // ─────────────────────────────────────────────────────────────────────────
    pub group:                ControlGroup,
    pub screen_timeout_idx:   usize,
    pub sleep_timeout_idx:    usize,
    /// Index into screensavers vec (0 = None).
    pub screensaver_idx:      usize,
    /// (display_name, exe_path); index 0 is always ("(None)", "").
    pub screensavers:         Vec<(String, String)>,
    /// Index into CURSOR_HIDE_OPTIONS (0 = Never / disabled).
    pub cursor_hide_idx:      usize,
    /// True while the cursor is currently hidden via SetSystemCursor.
    /// Used to avoid redundant API calls on every timer tick.
    pub cursor_hidden:        bool,
    /// Guard: set while we programmatically update the edit text to suppress
    /// the resulting EN_CHANGE from re-entering. (Kept for future use if EN_CHANGE returns.)
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

        // ── Display section ───────────────────────────────────────────────────
        let h_lbl_sect_display = cb.static_text(w!("Display"), SS_NOPREFIX);
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

        // CBS_DROPDOWNLIST | WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_VSCROLL
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

        // Apply owner-draw subclass + item heights (matches tab_crush combo style).
        SetWindowSubclass(h_ddl_screen_timeout, Some(combo_subclass_proc), 1, 0);
        SetWindowSubclass(h_ddl_sleep_timeout,  Some(combo_subclass_proc), 1, 0);
        let item_h = (20 * dpi / 96) as isize;
        SendMessageW(h_ddl_screen_timeout, CB_SETITEMHEIGHT, WPARAM(usize::MAX), LPARAM(item_h));
        SendMessageW(h_ddl_screen_timeout, CB_SETITEMHEIGHT, WPARAM(0),          LPARAM(item_h));
        SendMessageW(h_ddl_sleep_timeout,  CB_SETITEMHEIGHT, WPARAM(usize::MAX), LPARAM(item_h));
        SendMessageW(h_ddl_sleep_timeout,  CB_SETITEMHEIGHT, WPARAM(0),          LPARAM(item_h));

        // Populate both dropdowns with the shared TIMEOUT_OPTIONS list.
        for &(label, _) in TIMEOUT_OPTIONS {
            let lw: Vec<u16> = label.encode_utf16().chain([0]).collect();
            SendMessageW(h_ddl_screen_timeout, CB_ADDSTRING, WPARAM(0),
                LPARAM(lw.as_ptr() as isize));
            SendMessageW(h_ddl_sleep_timeout,  CB_ADDSTRING, WPARAM(0),
                LPARAM(lw.as_ptr() as isize));
        }

        // Read current AC values and select the matching entry.
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

        // Numeric edit + updown spinner for screensaver timeout (minutes, 1–999).
        // ES_NUMBER restricts input to digits only.
        // The updown (msctls_updown32) attaches itself to the right edge of the
        // buddy edit via UDS_AUTOBUDDY | UDS_ALIGNRIGHT | UDS_SETBUDDYINT.
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
        // Subclass: pressing Enter commits the value (same path as focus loss).
        SetWindowSubclass(h_edt_ss_timeout, Some(ss_timeout_edit_subclass_proc),
            IDC_SYS_EDT_SS_TIMEOUT, 0);

        // UDS_AUTOBUDDY    — adopt the previous control (the edit) as buddy
        // UDS_ALIGNRIGHT   — dock arrows to the right edge of the buddy
        // UDS_SETBUDDYINT  — keep buddy text in sync when arrows are clicked
        // UDS_ARROWKEYS    — respond to Up/Down keyboard keys
        // UDS_NOTHOUSANDS  — no thousands separator in the buddy text
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
        // Range: 1 – 999 minutes.  UDM_SETRANGE32 avoids the 16-bit limit.
        SendMessageW(h_spin_ss, windows::Win32::UI::Controls::UDM_SETRANGE32,
            WPARAM(1), LPARAM(999));
        SendMessageW(h_spin_ss, windows::Win32::UI::Controls::UDM_SETPOS32,
            WPARAM(0), LPARAM(ss_mins as isize));

        // Populate screensaver dropdown.
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

        // Disable the interactive wait controls when no screensaver is selected.
        // Static labels ("Wait" / "minutes") are left always-enabled to avoid the greyed shadow.
        let ss_enabled = screensaver_idx != 0;
        windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(h_edt_ss_timeout, ss_enabled);
        windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(h_spin_ss,        ss_enabled);

        // ── Mouse section ─────────────────────────────────────────────────────
        let h_lbl_sect_mouse = cb.static_text(w!("Mouse"), SS_NOPREFIX);
        SendMessageW(h_lbl_sect_mouse, WM_SETFONT,
            WPARAM(font_sect.0 as usize), LPARAM(1));

        let h_sep_mouse       = cb.static_text(w!(""), SS_BLACKRECT);
        let h_lbl_cursor_hide = cb.static_text(w!("Hide cursor on inactivity"), SS_NOPREFIX);

        let h_ddl_cursor_hide = CreateWindowExW(
            WINDOW_EX_STYLE(0), w!("COMBOBOX"), PCWSTR::null(),
            ddl_style, 0, 0, 0, 0,
            parent, HMENU(IDC_SYS_DDL_CURSOR_HIDE as _), hinstance, None,
        ).unwrap_or_default();
        SendMessageW(h_ddl_cursor_hide, WM_SETFONT,
            WPARAM(font_normal.0 as usize), LPARAM(1));
        SetWindowSubclass(h_ddl_cursor_hide, Some(combo_subclass_proc), 1, 0);
        SendMessageW(h_ddl_cursor_hide, CB_SETITEMHEIGHT, WPARAM(usize::MAX), LPARAM(item_h));
        SendMessageW(h_ddl_cursor_hide, CB_SETITEMHEIGHT, WPARAM(0),          LPARAM(item_h));

        for &(label, _) in CURSOR_HIDE_OPTIONS {
            let lw: Vec<u16> = label.encode_utf16().chain([0]).collect();
            SendMessageW(h_ddl_cursor_hide, CB_ADDSTRING, WPARAM(0),
                LPARAM(lw.as_ptr() as isize));
        }
        // Default to "Never" (index 0) — overridden by INI load in app.rs.
        let cursor_hide_idx = 0usize;
        SendMessageW(h_ddl_cursor_hide, CB_SETCURSEL, WPARAM(cursor_hide_idx), LPARAM(0));

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
            h_lbl_sect_mouse, h_sep_mouse,
            h_lbl_cursor_hide, h_ddl_cursor_hide,
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
            h_lbl_sect_mouse, h_sep_mouse,
            h_lbl_cursor_hide, h_ddl_cursor_hide,
            group,
            screen_timeout_idx,
            sleep_timeout_idx,
            screensaver_idx,
            screensavers,
            cursor_hide_idx,
            cursor_hidden: false,
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

        // Re-read the current screensaver from the registry on every refresh.
        // Registry reads are reliable; the old SPI_GETSCREENSAVER was not.
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
        // Only toggle enable/disable if the state actually changed to avoid flickering.
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

    /// Called from WM_NOTIFY / UDN_DELTAPOS with the already-computed new value
    /// (iPos + iDelta), since the edit text has not been updated yet at that point.
    /// Also updates the edit control text immediately so the field reflects the
    /// new value without waiting for UDS_SETBUDDYINT to fire.
    pub unsafe fn on_ss_timeout_set(&mut self, mins: u32) -> String {
        let mins = mins.clamp(1, 999);
        // Update edit text immediately — UDS_SETBUDDYINT will do this too, but
        // only after DefWindowProcW returns, so the field would lag one click.
        // Guard suppresses the EN_CHANGE this triggers, avoiding infinite recursion.
        let txt: Vec<u16> = format!("{}\0", mins).encode_utf16().collect();
        SetWindowTextW(self.h_edt_ss_timeout, PCWSTR(txt.as_ptr()));
        // Keep the spinner's internal iPos in sync so successive arrow clicks
        // use the correct base value. Must be done AFTER SetWindowTextW so the
        // updown does not overwrite the edit (UDS_SETBUDDYINT is not set).
        SendMessageW(self.h_spin_ss,
            windows::Win32::UI::Controls::UDM_SETPOS32,
            WPARAM(0), LPARAM(mins as isize));
        // Place caret at end of text after programmatic update.
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
            // Empty or invalid — restore the last known good value from the system.
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
        // Normalise the edit text (e.g. strip leading zeros) and sync spinner.
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

    /// Called from WM_COMMAND when `IDC_SYS_DDL_CURSOR_HIDE` fires CBN_SELCHANGE.
    /// Arms or kills TIMER_CURSOR_HIDE, saves the new index, and resets the idle
    /// clock so the cursor isn't hidden immediately after the user interacts with
    /// the dropdown.
    pub unsafe fn on_cursor_hide_changed(&mut self, hwnd: HWND) -> &'static str {
        let sel = SendMessageW(self.h_ddl_cursor_hide,
            CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 as usize;
        let sel = sel.min(CURSOR_HIDE_OPTIONS.len() - 1);
        self.cursor_hide_idx = sel;

        // Reset idle clock so the timer doesn't immediately fire.
        cursor_touch();

        if sel == 0 {
            // "Never" — disarm timer and ensure cursor is visible.
            KillTimer(hwnd, TIMER_CURSOR_HIDE);
            restore_system_cursors();
            "Cursor hide: disabled"
        } else {
            // Arm a 1-second tick; cheap — fires ~once/sec, one atomic read per tick.
            SetTimer(hwnd, TIMER_CURSOR_HIDE, 1000, None);
            "Cursor hide: enabled"
        }
    }

    /// Called from WM_TIMER for `TIMER_CURSOR_HIDE` (~1 s tick).
    ///
    /// Uses `GetLastInputInfo` — OS-maintained, covers keyboard + mouse + touch,
    /// requires no hook. One cheap struct read per tick; only calls
    /// `SetSystemCursor` / `restore_system_cursors` on state change.
    pub unsafe fn on_cursor_hide_tick(&mut self, hwnd: HWND) {
        let threshold_secs = match CURSOR_HIDE_OPTIONS.get(self.cursor_hide_idx) {
            Some(&(_, s)) if s > 0 => s as u64,
            _ => {
                KillTimer(hwnd, TIMER_CURSOR_HIDE);
                return;
            }
        };

        // GetLastInputInfo returns dwTime in the same GetTickCount() domain.
        // GetTickCount64 gives the current time; the subtraction is safe across
        // the 32-bit wrap of dwTime because we cast to u64 after delta.
        let mut lii = windows::Win32::UI::Input::KeyboardAndMouse::LASTINPUTINFO {
            cbSize: std::mem::size_of::<windows::Win32::UI::Input::KeyboardAndMouse::LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        windows::Win32::UI::Input::KeyboardAndMouse::GetLastInputInfo(&mut lii);
        let now_ms   = GetTickCount64();
        let last_ms  = lii.dwTime as u64
            | (now_ms & 0xFFFF_FFFF_0000_0000); // extend to 64-bit in same epoch
        // If last_ms ended up in the future (32-bit wrapped), subtract one epoch.
        let last_ms = if last_ms > now_ms { last_ms.wrapping_sub(0x1_0000_0000) } else { last_ms };
        let idle_ms  = now_ms.saturating_sub(last_ms);
        let should_hide = idle_ms >= threshold_secs * 1000;

        // The hook proc may have already restored via CURSOR_HIDDEN atomic —
        // re-read it so our local state stays in sync.
        let hook_restored = !CURSOR_HIDDEN.load(Ordering::Relaxed);
        if hook_restored && self.cursor_hidden {
            self.cursor_hidden = false;
        }

        if should_hide != self.cursor_hidden {
            self.cursor_hidden = should_hide;
            CURSOR_HIDDEN.store(should_hide, Ordering::Relaxed);
            if should_hide {
                // Switch to 100ms ticks while hidden so restore is near-instant.
                SetTimer(hwnd, TIMER_CURSOR_HIDE, 100, None);
                // Replace ALL standard cursors so nothing leaks through.
                // SetSystemCursor takes ownership — fresh handle required each call.
                for &id in &[
                    32512u32, // OCR_NORMAL  — arrow
                    32513,    // OCR_IBEAM   — text beam
                    32514,    // OCR_WAIT    — hourglass
                    32515,    // OCR_CROSS
                    32516,    // OCR_UP
                    32642,    // OCR_SIZENWSE
                    32643,    // OCR_SIZENESW
                    32644,    // OCR_SIZEWE
                    32645,    // OCR_SIZENS
                    32646,    // OCR_SIZEALL
                    32648,    // OCR_NO
                    32649,    // OCR_HAND
                    32650,    // OCR_APPSTARTING
                ] {
                    let blank = make_blank_cursor();
                    if !blank.is_invalid() {
                        SetSystemCursor(blank, SYSTEM_CURSOR_ID(id));
                    }
                }
            } else {
                // Back to 1s ticks — no need to poll fast while visible.
                SetTimer(hwnd, TIMER_CURSOR_HIDE, 1000, None);
                restore_system_cursors();
            }
        }
    }

    /// Expose whether the cursor is currently hidden (used by app.rs on exit /
    /// tab-switch to guarantee restoration).
    pub fn cursor_is_hidden(&self) -> bool {
        CURSOR_HIDDEN.load(Ordering::Relaxed)
    }

    /// Re-read screensaver state from the system and update the dropdown + spinner.
/// Call from WM_SETTINGCHANGE so external changes (Windows Settings, screensaver dialog) are reflected.
pub unsafe fn reload_screensaver_state(&mut self) {
    let current_exe = read_screensaver_exe().to_lowercase();
    let idx = self.screensavers.iter().position(|(_, p)| {
        let scr_file = std::path::Path::new(p)
            .file_name().and_then(|f| f.to_str()).unwrap_or("").to_lowercase();
        let cur_file = std::path::Path::new(&current_exe)
            .file_name().and_then(|f| f.to_str()).unwrap_or("").to_lowercase();
        !p.is_empty() && !cur_file.is_empty() && scr_file == cur_file
    }).unwrap_or(0);

    self.screensaver_idx = idx;
    SendMessageW(self.h_ddl_screensaver, CB_SETCURSEL, WPARAM(idx), LPARAM(0));

    let ss_enabled = idx != 0;
    windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(self.h_edt_ss_timeout, ss_enabled);
    windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(self.h_spin_ss,        ss_enabled);

    let secs = read_screensaver_timeout();
    let mins = (secs / 60).max(1);
    let txt: Vec<u16> = format!("{}\0", mins).encode_utf16().collect();
    SetWindowTextW(self.h_edt_ss_timeout, PCWSTR(txt.as_ptr()));
    SendMessageW(self.h_spin_ss,
        windows::Win32::UI::Controls::UDM_SETPOS32,
        WPARAM(0), LPARAM(mins as isize));
}

}