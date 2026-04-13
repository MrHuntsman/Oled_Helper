// hotkeys.rs — hotkey IDs, parse_hotkey, RegisterHotKey wrappers,
//              hold-to-compare keyboard hook, and WH_MOUSE_LL mouse-button hook.

#![allow(non_snake_case, unused_must_use)]

use std::sync::atomic::{AtomicU32, AtomicIsize, Ordering};

use windows::Win32::{
    Foundation::{HWND, LRESULT, LPARAM, WPARAM},
    UI::{
        Input::KeyboardAndMouse::{
            HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN,
            RegisterHotKey, UnregisterHotKey,
        },
        WindowsAndMessaging::{
            CallNextHookEx, PostMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
            HHOOK, KBDLLHOOKSTRUCT, MSLLHOOKSTRUCT,
            WH_KEYBOARD_LL, WH_MOUSE_LL,
            WM_HOTKEY, WM_KEYUP, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_MOUSEMOVE,
            WM_RBUTTONDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN,
        },
    },
};

use crate::constants::WM_COMPARE_END;

// ── Hotkey IDs (WM_HOTKEY wparam) ────────────────────────────────────────────

pub const HK_TOGGLE_DIM:   usize = 1;
pub const HK_TOGGLE_CRUSH: usize = 2;
pub const HK_HOLD_COMPARE: usize = 3;
pub const HK_DECREASE:     usize = 4;
pub const HK_INCREASE:     usize = 5;
pub const HK_TOGGLE_HDR:   usize = 8;

// ── parse_hotkey ──────────────────────────────────────────────────────────────

/// Parses "Ctrl+Alt+F9" / "Mouse4" → (modifiers, vk_or_sentinel).
/// Returns None for "None", empty, or unrecognised keys.
/// Mouse sentinels are returned with mods=0; route them to WH_MOUSE_LL, not RegisterHotKey.
pub fn parse_hotkey(s: &str) -> Option<(u32, u32)> {
    use crate::tab_hotkeys::{
        MB_MIDDLE,
        MB_XBUTTON1,  MB_XBUTTON2,  MB_XBUTTON3,  MB_XBUTTON4,
        MB_XBUTTON5,  MB_XBUTTON6,  MB_XBUTTON7,  MB_XBUTTON8,
        MB_XBUTTON9,  MB_XBUTTON10, MB_XBUTTON11, MB_XBUTTON12,
        MB_XBUTTON13, MB_XBUTTON14, MB_XBUTTON15, MB_XBUTTON16,
    };
    if s.is_empty() || s.eq_ignore_ascii_case("none") { return None; }

    // Mouse buttons carry no modifiers — check before stripping any prefix.
    let mouse_sentinel: Option<u32> = match s {
        "MButton" => Some(MB_MIDDLE),
        "Mouse4"  => Some(MB_XBUTTON1),  "Mouse5"  => Some(MB_XBUTTON2),
        "Mouse6"  => Some(MB_XBUTTON3),  "Mouse7"  => Some(MB_XBUTTON4),
        "Mouse8"  => Some(MB_XBUTTON5),  "Mouse9"  => Some(MB_XBUTTON6),
        "Mouse10" => Some(MB_XBUTTON7),  "Mouse11" => Some(MB_XBUTTON8),
        "Mouse12" => Some(MB_XBUTTON9),  "Mouse13" => Some(MB_XBUTTON10),
        "Mouse14" => Some(MB_XBUTTON11), "Mouse15" => Some(MB_XBUTTON12),
        "Mouse16" => Some(MB_XBUTTON13), "Mouse17" => Some(MB_XBUTTON14),
        "Mouse18" => Some(MB_XBUTTON15), "Mouse19" => Some(MB_XBUTTON16),
        _ => None,
    };
    if let Some(s) = mouse_sentinel { return Some((0, s)); }

    let mut mods: u32 = 0;
    let mut key_part = s;
    loop {
        if      let Some(r) = key_part.strip_prefix("Win+")   { mods |= MOD_WIN.0;     key_part = r; }
        else if let Some(r) = key_part.strip_prefix("Ctrl+")  { mods |= MOD_CONTROL.0; key_part = r; }
        else if let Some(r) = key_part.strip_prefix("Alt+")   { mods |= MOD_ALT.0;     key_part = r; }
        else if let Some(r) = key_part.strip_prefix("Shift+") { mods |= MOD_SHIFT.0;   key_part = r; }
        else { break; }
    }

    // Sorted by lexicographic byte order for binary_search_by_key.
    static KEY_TABLE: &[(&str, u32)] = &[
        ("'",         0xDE),
        (",",         0xBC),
        ("-",         0xBD),
        (".",         0xBE),
        ("/",         0xBF),
        ("0",         0x30), ("1", 0x31), ("2", 0x32), ("3", 0x33), ("4", 0x34),
        ("5",         0x35), ("6", 0x36), ("7", 0x37), ("8", 0x38), ("9", 0x39),
        (";",         0xBA),
        ("=",         0xBB),
        ("A",         0x41), ("B", 0x42),
        ("Backspace", 0x08),
        ("C",         0x43),
        ("CapsLk",    0x14),
        ("D",         0x44),
        ("Delete",    0x2E),
        ("Down",      0x28),
        ("E",         0x45),
        ("End",       0x23),
        ("Enter",     0x0D),
        ("Esc",       0x1B),
        ("F",         0x46),
        ("F1",        0x70), ("F10", 0x79), ("F11", 0x7A), ("F12", 0x7B),
        ("F13",       0x7C), ("F14", 0x7D), ("F15", 0x7E), ("F16", 0x7F),
        ("F17",       0x80), ("F18", 0x81), ("F19", 0x82),
        ("F2",        0x71),
        ("F20",       0x83), ("F21", 0x84), ("F22", 0x85), ("F23", 0x86), ("F24", 0x87),
        ("F3",        0x72), ("F4",  0x73), ("F5",  0x74),
        ("F6",        0x75), ("F7",  0x76), ("F8",  0x77), ("F9",  0x78),
        ("G",         0x47), ("H",   0x48),
        ("Home",      0x24),
        ("I",         0x49),
        ("Insert",    0x2D),
        ("J",         0x4A), ("K",   0x4B), ("L",   0x4C),
        ("Left",      0x25),
        ("M",         0x4D), ("N",   0x4E),
        ("Num*",      0x6A), ("Num+", 0x6B), ("Num-", 0x6D),
        ("Num.",      0x6E), ("Num/", 0x6F),
        ("Num0",      0x60), ("Num1", 0x61), ("Num2", 0x62), ("Num3", 0x63),
        ("Num4",      0x64), ("Num5", 0x65), ("Num6", 0x66), ("Num7", 0x67),
        ("Num8",      0x68), ("Num9", 0x69),
        ("O",         0x4F), ("P",   0x50),
        ("Pause",     0x13),
        ("PgDn",      0x22),
        ("PgUp",      0x21),
        ("Print",     0x2C),
        ("Q",         0x51), ("R",   0x52),
        ("Right",     0x27),
        ("S",         0x53),
        ("Space",     0x20),
        ("T",         0x54),
        ("Tab",       0x09),
        ("U",         0x55),
        ("Up",        0x26),
        ("V",         0x56), ("W",   0x57), ("X",   0x58), ("Y",   0x59), ("Z", 0x5A),
        ("[",         0xDB),
        ("\\",        0xDC),
        ("]",         0xDD),
        ("`",         0xC0),
    ];

    let vk = KEY_TABLE
        .binary_search_by_key(&key_part, |&(name, _)| name)
        .ok()
        .map(|i| KEY_TABLE[i].1)?;

    Some((mods, vk))
}

// ── register_hotkeys ──────────────────────────────────────────────────────────

/// Unregisters all hotkeys then re-registers from INI values.
/// Keyboard bindings → RegisterHotKey; mouse bindings → WH_MOUSE_LL hook.
pub unsafe fn register_hotkeys(
    ini: &crate::profile_manager::ProfileManager,
    mouse_hotkeys: &mut [u32; 9],
    hwnd: HWND,
) {
    use crate::tab_hotkeys::is_mouse_sentinel;

    for id in [HK_TOGGLE_DIM, HK_TOGGLE_CRUSH, HK_HOLD_COMPARE,
               HK_DECREASE,   HK_INCREASE,      HK_TOGGLE_HDR] {
        UnregisterHotKey(hwnd, id as i32);
    }
    for slot in &MOUSE_HK_SLOTS { slot.store(0, Ordering::SeqCst); }
    *mouse_hotkeys = [0u32; 9];

    let sec = "Hotkeys";
    let bindings = [
        (HK_TOGGLE_DIM,   ini.read(sec, "ToggleTaskbarDimmer", "None")),
        (HK_TOGGLE_CRUSH, ini.read(sec, "ToggleBlackCrush",    "None")),
        (HK_HOLD_COMPARE, ini.read(sec, "HoldCompare",         "None")),
        (HK_DECREASE,     ini.read(sec, "DecreaseBlackCrush",  "None")),
        (HK_INCREASE,     ini.read(sec, "IncreaseBlackCrush",  "None")),
        (HK_TOGGLE_HDR,   ini.read(sec, "ToggleHDR",           "None")),
    ];

    let mut any_mouse = false;
    for (id, s) in &bindings {
        if let Some((mods, vk)) = parse_hotkey(s) {
            if is_mouse_sentinel(vk) {
                mouse_hotkeys[*id] = vk;
                set_mouse_hk_slot(*id, vk);
                any_mouse = true;
            } else {
                // MOD_NOREPEAT prevents action flooding on held key.
                RegisterHotKey(hwnd, *id as i32, HOT_KEY_MODIFIERS(mods | MOD_NOREPEAT.0), vk);
            }
        }
    }

    if any_mouse { ensure_mouse_hook_installed(hwnd); } else { uninstall_mouse_hook(); }
}

// ── Hold-to-compare keyboard hook (WH_KEYBOARD_LL) ───────────────────────────
// RegisterHotKey only fires on key-down. We install a low-level keyboard hook
// for the hold duration and remove it immediately on key-up.

static COMPARE_VK:   AtomicU32   = AtomicU32::new(0);   // VK being held (0 = none)
static COMPARE_HWND: AtomicIsize = AtomicIsize::new(0);  // main window HWND
static COMPARE_HOOK: AtomicIsize = AtomicIsize::new(0);  // hook handle (0 = not installed)

pub unsafe fn install_compare_hook(hwnd: HWND, vk: u32) {
    COMPARE_VK.store(vk, Ordering::SeqCst);
    COMPARE_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
    let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(compare_ll_hook_proc), None, 0)
        .unwrap_or(HHOOK(std::ptr::null_mut()));
    COMPARE_HOOK.store(hook.0 as isize, Ordering::SeqCst);
}

pub unsafe fn uninstall_compare_hook() {
    let raw = COMPARE_HOOK.swap(0, Ordering::SeqCst);
    if raw != 0 { UnhookWindowsHookEx(HHOOK(raw as *mut _)); }
    COMPARE_VK.store(0, Ordering::SeqCst);
}

// ── Hold-to-repeat keyboard hook (WH_KEYBOARD_LL) ────────────────────────────
// Installed when HK_INCREASE or HK_DECREASE fires. Posts WM_CRUSH_REPEAT_END
// to the main window on key-up so the repeat timer can be killed cleanly.

pub const WM_CRUSH_REPEAT_END: u32 = windows::Win32::UI::WindowsAndMessaging::WM_USER + 30;

static REPEAT_VK:   AtomicU32   = AtomicU32::new(0);
static REPEAT_HWND: AtomicIsize = AtomicIsize::new(0);
static REPEAT_HOOK: AtomicIsize = AtomicIsize::new(0);

pub unsafe fn install_repeat_hook(hwnd: HWND, vk: u32) {
    REPEAT_VK.store(vk, Ordering::SeqCst);
    REPEAT_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
    let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(repeat_ll_hook_proc), None, 0)
        .unwrap_or(HHOOK(std::ptr::null_mut()));
    REPEAT_HOOK.store(hook.0 as isize, Ordering::SeqCst);
}

pub unsafe fn uninstall_repeat_hook() {
    let raw = REPEAT_HOOK.swap(0, Ordering::SeqCst);
    if raw != 0 { UnhookWindowsHookEx(HHOOK(raw as *mut _)); }
    REPEAT_VK.store(0, Ordering::SeqCst);
}

unsafe extern "system" fn repeat_ll_hook_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 {
        let info = &*(lp.0 as *const KBDLLHOOKSTRUCT);
        let vk   = REPEAT_VK.load(Ordering::SeqCst);
        if info.vkCode == vk && (wp.0 as u32 == WM_KEYUP || wp.0 as u32 == WM_SYSKEYUP) {
            let hwnd_raw = REPEAT_HWND.load(Ordering::SeqCst);
            if hwnd_raw != 0 {
                PostMessageW(HWND(hwnd_raw as *mut _), WM_CRUSH_REPEAT_END, WPARAM(0), LPARAM(0));
            }
            uninstall_repeat_hook();
        }
    }
    CallNextHookEx(HHOOK(std::ptr::null_mut()), code, wp, lp)
}

unsafe extern "system" fn compare_ll_hook_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 {
        let info = &*(lp.0 as *const KBDLLHOOKSTRUCT);
        let vk   = COMPARE_VK.load(Ordering::SeqCst);
        if info.vkCode == vk && (wp.0 as u32 == WM_KEYUP || wp.0 as u32 == WM_SYSKEYUP) {
            let hwnd_raw = COMPARE_HWND.load(Ordering::SeqCst);
            if hwnd_raw != 0 {
                PostMessageW(HWND(hwnd_raw as *mut _), WM_COMPARE_END, WPARAM(0), LPARAM(0));
            }
            uninstall_compare_hook();
        }
    }
    CallNextHookEx(HHOOK(std::ptr::null_mut()), code, wp, lp)
}

// ── Mouse-button hotkeys (WH_MOUSE_LL) ───────────────────────────────────────
// RegisterHotKey only accepts VK codes; mouse buttons must use a low-level hook.
// When a bound button fires, we post WM_HOTKEY to the main window directly.
// The hook runs on the installing thread (same as the message pump), so PostMessageW is safe.

static MOUSE_HOOK:    AtomicIsize = AtomicIsize::new(0); // hook handle (0 = not installed)
static MOUSE_HK_HWND: AtomicIsize = AtomicIsize::new(0); // main window HWND

/// Packed bindings: index = HK_* id (1–7), value = (id << 32) | MB_* sentinel (0 = unbound).
pub static MOUSE_HK_SLOTS: [std::sync::atomic::AtomicU64; 9] = [
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
    std::sync::atomic::AtomicU64::new(0),
];

pub fn set_mouse_hk_slot(hk_id: usize, sentinel: u32) {
    if hk_id < MOUSE_HK_SLOTS.len() {
        MOUSE_HK_SLOTS[hk_id].store(((hk_id as u64) << 32) | sentinel as u64, Ordering::SeqCst);
    }
}

pub unsafe fn ensure_mouse_hook_installed(hwnd: HWND) {
    if MOUSE_HOOK.load(Ordering::SeqCst) != 0 { return; }
    MOUSE_HK_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
    let hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_ll_hook_proc), None, 0)
        .unwrap_or(HHOOK(std::ptr::null_mut()));
    MOUSE_HOOK.store(hook.0 as isize, Ordering::SeqCst);
}

pub unsafe fn uninstall_mouse_hook() {
    let raw = MOUSE_HOOK.swap(0, Ordering::SeqCst);
    if raw != 0 { UnhookWindowsHookEx(HHOOK(raw as *mut _)); }
    for slot in &MOUSE_HK_SLOTS { slot.store(0, Ordering::SeqCst); }
    MOUSE_HK_HWND.store(0, Ordering::SeqCst);
}

unsafe extern "system" fn mouse_ll_hook_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    use crate::tab_hotkeys::{MB_MIDDLE, xbutton_index_to_sentinel};

    if code >= 0 {
        let sentinel: Option<u32> = match wp.0 as u32 {
            WM_MBUTTONDOWN => Some(MB_MIDDLE),
            WM_XBUTTONDOWN => {
                let info = &*(lp.0 as *const MSLLHOOKSTRUCT);
                let idx  = ((info.mouseData >> 16) & 0xFFFF) as u16; // HIWORD = 1-based XBUTTON index
                xbutton_index_to_sentinel(idx)
            }
            _ => None,
        };

        if let Some(s) = sentinel {
            let hwnd_raw = MOUSE_HK_HWND.load(Ordering::SeqCst);
            if hwnd_raw != 0 {
                for slot in &MOUSE_HK_SLOTS {
                    let v               = slot.load(Ordering::SeqCst);
                    let stored_sentinel = (v & 0xFFFF_FFFF) as u32;
                    let hk_id           = (v >> 32) as usize;
                    if stored_sentinel != 0 && stored_sentinel == s {
                        PostMessageW(HWND(hwnd_raw as *mut _), WM_HOTKEY, WPARAM(hk_id), LPARAM(0));
                        // No break — two actions can share the same button.
                    }
                }
            }
        }
    }
    CallNextHookEx(HHOOK(std::ptr::null_mut()), code, wp, lp)
}