// tab_dimmer.rs — Dimmer state, overlays, and animation.
// Rendering: ui_drawing.rs  |  Constants/IDs: constants.rs

#![allow(non_snake_case, clippy::too_many_lines, unused_variables,
         unused_mut, unused_assignments, unused_must_use)]

use std::{mem, ptr};

use windows::{
    core::*,
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::{
            LibraryLoader::GetModuleHandleW,
            SystemInformation::GetTickCount64,
        },
        UI::{
            Accessibility::HWINEVENTHOOK,
            Controls::*,
            Shell::{SHAppBarMessage, APPBARDATA, ABM_GETSTATE},
            WindowsAndMessaging::*,
        },
    },
};

// Not re-exported by windows-rs Shell bindings.
const ABS_AUTOHIDE: u32 = 0x0000_0001;

// Prevents shell windows from re-asserting Z-order during SetWindowPos.
const SWP_NOSENDCHANGING: SET_WINDOW_POS_FLAGS = SET_WINDOW_POS_FLAGS(0x0400);

// Active timer interval for TIMER_OVERLAY_FADE in ms. Defaults to 16 (≈60 Hz).
// Updated by `set_fade_interval_from_hz` on display-rate changes.
// Atomic so app.rs can write it from WndProc without borrowing DimmerTab.
pub static OVERLAY_FADE_INTERVAL_MS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(16);

/// Updates fade timer interval based on Hz.
pub fn set_fade_interval_from_hz(hz: i32) {
    let interval = (1000u32 / (hz.max(30) as u32)).max(1);
    OVERLAY_FADE_INTERVAL_MS.store(interval, std::sync::atomic::Ordering::Relaxed);
}

use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS};

// Fires when shell panels (Quick Settings, etc.) displace our overlays.
const EVENT_OBJECT_SHOW: u32 = 0x8002;

use crate::{
    constants::*,
    profile_manager::ProfileManager,
    ui_drawing::{get_slider_val, make_font, install_action_btn_hover, slider_subclass_proc, SetWindowSubclass},
    win32::ControlGroup,
};

// UTF-16 class-name constants for zero-alloc slice comparisons.
// `GetClassNameW` returns a NUL-free u16 slice; comparing directly avoids
// the String::from_utf16_lossy allocation done at each call-site.
// `ascii_to_utf16` converts ASCII byte strings to u16 arrays at compile time.

const fn ascii_to_utf16<const N: usize>(s: &[u8; N]) -> [u16; N] {
    let mut out = [0u16; N];
    let mut i = 0;
    while i < N {
        out[i] = s[i] as u16;
        i += 1;
    }
    out
}

const CLS_TRAY_ARR:     [u16; 13] = ascii_to_utf16(b"Shell_TrayWnd");
const CLS_SEC_TRAY_ARR: [u16; 22] = ascii_to_utf16(b"Shell_SecondaryTrayWnd");
const CLS_PROGMAN_ARR:  [u16;  7] = ascii_to_utf16(b"Progman");
const CLS_WORKER_W_ARR: [u16;  7] = ascii_to_utf16(b"WorkerW");

const CLS_TRAY:     &[u16] = &CLS_TRAY_ARR;
const CLS_SEC_TRAY: &[u16] = &CLS_SEC_TRAY_ARR;
const CLS_PROGMAN:  &[u16] = &CLS_PROGMAN_ARR;
const CLS_WORKER_W: &[u16] = &CLS_WORKER_W_ARR;

// Shell panel classes that appear on top of our overlays (Quick Settings,
// Action Center, Start, search, toast hosts). On EVENT_OBJECT_SHOW we
// immediately re-raise the overlays.
const CLS_CTRL_CENTER_ARR:  [u16; 19] = ascii_to_utf16(b"ControlCenterWindow");
const CLS_XAML_HOST_ARR:    [u16; 28] = ascii_to_utf16(b"XamlExplorerHostIslandWindow");
const CLS_ACTION_CTR_ARR:   [u16; 22] = ascii_to_utf16(b"ActionCenterExperience");
const CLS_NOTIF_TOAST_ARR:  [u16; 26] = ascii_to_utf16(b"Windows.UI.Core.CoreWindow");
const CLS_START_HOST_ARR:   [u16; 23] = ascii_to_utf16(b"StartMenuExperienceHost");

const CLS_CTRL_CENTER:  &[u16] = &CLS_CTRL_CENTER_ARR;
const CLS_XAML_HOST:    &[u16] = &CLS_XAML_HOST_ARR;
const CLS_ACTION_CTR:   &[u16] = &CLS_ACTION_CTR_ARR;
const CLS_NOTIF_TOAST:  &[u16] = &CLS_NOTIF_TOAST_ARR;
const CLS_START_HOST:   &[u16] = &CLS_START_HOST_ARR;

/// True if the class matches any shell panel that displaces overlay windows.
/// Zero allocations — compares u16 slices against compile-time arrays.
#[inline]
fn is_displacing_shell_panel(cls: &[u16]) -> bool {
    cls == CLS_CTRL_CENTER
        || cls == CLS_XAML_HOST
        || cls == CLS_ACTION_CTR
        || cls == CLS_NOTIF_TOAST
        || cls == CLS_START_HOST
}

// ── Hook → UI-thread messages ─────────────────────────────────────────────────

/// Posted by `zorder_winevent_proc` to trigger fullscreen re-evaluation on the UI thread.
pub const WM_APP_FULLSCREEN_CHECK: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 1;

/// Posted by `object_show_proc` when a displacing shell panel appears.
/// UI thread responds by re-raising all overlays to HWND_TOPMOST.
pub const WM_APP_RAISE_OVERLAYS: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 2;





/// Main window HWND as an atomic isize so the WinEvent hook thread can post
/// messages back to the UI. Also read by the WH_MOUSE_LL hook in tab_debug.
pub static MAIN_HWND_FOR_HOOK: std::sync::atomic::AtomicIsize =
    std::sync::atomic::AtomicIsize::new(0);

/// Register the main HWND for use by the WinEvent hook proc. Call once after creation.
pub fn register_main_hwnd(hwnd: HWND) {
    MAIN_HWND_FOR_HOOK.store(hwnd.0 as isize, std::sync::atomic::Ordering::Relaxed);
}

/// Primary taskbar HWND cached for zero-syscall lookup in the hook proc.
pub static TRAY_HWND_FOR_HOOK: std::sync::atomic::AtomicIsize =
    std::sync::atomic::AtomicIsize::new(0);

/// Cache the primary taskbar HWND. Call once after overlays are created and after display changes.
pub unsafe fn cache_tray_hwnd() {
    if let Ok(h) = FindWindowW(w!("Shell_TrayWnd"), None) {
        TRAY_HWND_FOR_HOOK.store(h.0 as isize, std::sync::atomic::Ordering::Relaxed);
    }
}

// ── Z-order event log (shared with DebugTab) ──────────────────────────────────

/// Discriminant for a Z-order log entry.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ZLogKind {
    ForegroundChange,
    MinimizeEnd,
    OverlayRaised,
    /// Z-order position sampled after a HWND_TOPMOST call, logged only on change.
    /// payload = ((overlay_index as u32) << 32) | (1-based z-pos as u32)
    /// Position 1 = topmost; u32::MAX = overlay not found in chain.
    OverlayZPos,
    /// payload = ((x as u32) << 32) | (y as u32)
    MouseClick,
    /// Dimmer suppression state changed (hover / fullscreen / auto-hide).
    /// payload bits: 0=hovering 1=fullscreen 2=autohide 3=enabled
    DimmerStateChange,
    /// WM_DPICHANGED received. payload = new DPI.
    DpiChanged,
    /// WM_DISPLAYCHANGE received. payload = (width << 32) | height.
    DisplayChange,
    /// Phase inside WM_APP_DEFERRED_DPI_RESIZE.
    /// bits [63:48] = phase (0=entry, 1=after SetWindowPos, 2=after RedrawWindow)
    /// bits [47:0]  = ms since WM_DPICHANGED
    DeferredDpiResize,
    /// reposition_overlays_now() completed. payload = (num_rects << 32) | elapsed_ms.
    RepositionNow,
    /// tick_reposition() fired from the 500 ms safety-net timer. payload = num_rects.
    RepositionSafetyNet,
    Other,
}

impl ZLogKind {
    /// Sigil prefix for log display.
    pub fn label(self, payload: u64) -> String {
        match self {
            ZLogKind::ForegroundChange => {
                format!("[FG] Foreground changed  hwnd={:#010x}", payload)
            }
            ZLogKind::MinimizeEnd => {
                format!("[MN] Window un-minimised  hwnd={:#010x}", payload)
            }
            ZLogKind::OverlayRaised => {
                if payload == u64::MAX {
                    "[RZ] Overlays force-raised (manual/debug)".to_owned()
                } else {
                    format!("[RZ] Overlay raised  idx={}", payload)
                }
            }
            ZLogKind::OverlayZPos => {
                let idx = (payload >> 32) as u32;
                let pos = (payload & 0xFFFF_FFFF) as u32;
                if pos == u32::MAX {
                    format!("[ZP] Overlay[{}] Z-pos → NOT FOUND in chain", idx)
                } else if pos == 1 {
                    format!("[ZP] Overlay[{}] Z-pos → #1 (topmost ✓)", idx)
                } else {
                    format!("[ZP] Overlay[{}] Z-pos → #{} (displaced!)", idx, pos)
                }
            }
            ZLogKind::MouseClick => {
                let x = (payload >> 32) as i32;
                let y = (payload & 0xFFFF_FFFF) as i32;
                format!("[MB] Click  ({}, {})", x, y)
            }
            ZLogKind::DimmerStateChange => {
                "[DM] Dimmer state changed".to_owned()
            }
            ZLogKind::DpiChanged => {
                format!("[DP] WM_DPICHANGED  new_dpi={}", payload)
            }
            ZLogKind::DisplayChange => {
                let w = (payload >> 32) as u32;
                let h = (payload & 0xFFFF_FFFF) as u32;
                format!("[DC] WM_DISPLAYCHANGE  {}x{}", w, h)
            }
            ZLogKind::DeferredDpiResize => {
                // detail already contains the full formatted line.
                "[DR]".to_owned()
            }
            ZLogKind::RepositionNow => {
                let rects = (payload >> 32) as u32;
                let ms    = (payload & 0xFFFF_FFFF) as u32;
                format!("[RP] reposition_overlays_now  rects={}  took={}ms", rects, ms)
            }
            ZLogKind::RepositionSafetyNet => {
                format!("[SN] tick_reposition (safety-net)  rects={}", payload)
            }
            ZLogKind::Other => format!("[??] Unknown  data={:#x}", payload),
        }
    }
}

/// A single entry in the Z-order event ring.
#[derive(Clone)]
pub struct ZLogEntry {
    /// `GetTickCount64` ms at event time.
    pub tick:    u64,
    pub kind:    ZLogKind,
    /// Context-dependent payload (HWND, index, etc.).
    pub payload: u64,
    /// Optional rich description. When non-empty, used instead of `kind.label(payload)`.
    pub detail:  String,
}

/// Fixed-capacity ring buffer for Z-order events.
#[allow(dead_code)]
pub struct ZOrderLog {
    entries: std::collections::VecDeque<ZLogEntry>,
    /// Total pushes ever (monotonically increasing).
    pub count: usize,
    capacity: usize,
}

impl ZOrderLog {
    pub fn new() -> Self {
        Self { entries: std::collections::VecDeque::new(), count: 0, capacity: 256 }
    }

    #[allow(dead_code)]
    pub fn push(&mut self, tick: u64, kind: ZLogKind, payload: u64, detail: String) {
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(ZLogEntry { tick, kind, payload, detail });
        self.count += 1;
    }

    /// Returns up to `n` most recent entries, oldest first.
    pub fn recent(&self, n: usize) -> Vec<ZLogEntry> {
        let skip = self.entries.len().saturating_sub(n);
        self.entries.iter().skip(skip).cloned().collect()
    }
}

use std::sync::{Mutex, OnceLock};

static ZORDER_LOG_INNER: OnceLock<Mutex<ZOrderLog>> = OnceLock::new();

/// Global Z-order event log. Returns None when not in debug mode,
/// so callers can just do: `if let Some(log) = zorder_log() { ... }`
pub fn zorder_log() -> Option<&'static Mutex<ZOrderLog>> {
    if !crate::app::is_debug_mode() {
        return None;
    }
    Some(ZORDER_LOG_INNER.get_or_init(|| Mutex::new(ZOrderLog::new())))
}

// ── DimmerTab ─────────────────────────────────────────────────────────────────

pub struct DimmerTab {
    // ── Controls ──────────────────────────────────────────────────────────────
    pub h_lbl_dim_title:      HWND,
    pub h_lbl_dim_sub:        HWND,
    pub h_chk_taskbar_dim:    HWND,
    pub h_lbl_dim_sect:       HWND,
    pub h_sep_dim_sect:       HWND,
    pub h_lbl_dim_pct:        HWND,
    pub h_sld_taskbar_dim:    HWND,
    pub h_lbl_fade_sect:      HWND,
    pub h_sep_fade_sect:      HWND,
    pub h_lbl_fade_in_title:  HWND,
    pub h_sld_fade_in:        HWND,
    pub h_lbl_fade_in_val:    HWND,
    pub h_lbl_fade_out_title: HWND,
    pub h_sld_fade_out:       HWND,
    pub h_lbl_fade_out_val:   HWND,
    pub h_btn_dim_defaults:   HWND,

    /// Controls shown only when the dimmer is enabled; toggled via `grp_dim_controls.set_visible()`.
    pub grp_dim_controls: ControlGroup,

    // ── Runtime state ─────────────────────────────────────────────────────────
    pub enabled: bool,
    /// One overlay per taskbar (primary + secondaries).
    pub taskbar_overlays: Vec<HWND>,

    /// f32 for smooth interpolation (0–255).
    pub overlay_alpha_current:    f32,
    /// Fade target.
    pub overlay_alpha_target:     f32,
    /// Alpha derived from slider; used as the fade-in target.
    pub overlay_alpha_full:       f32,
    /// Tick (ms) when the current fade transition started.
    pub overlay_fade_start_time:  u64,
    pub overlay_fade_start_alpha: f32,
    /// Fade in: transparent → dim (cursor leaves taskbar).
    pub overlay_fade_in_ms:       f32,
    /// Fade out: dim → transparent (cursor hovers).
    pub overlay_fade_out_ms:      f32,

    // ── Suppression flags ─────────────────────────────────────────────────────
    pub suppress_fs_enabled: bool,  // hide overlay when fullscreen
    pub suppress_ah_enabled: bool,  // hide overlay when auto-hide active

    /// Needed to arm/kill the fade timer.
    main_hwnd: HWND,

    // ── Performance ───────────────────────────────────────────────────────────
    /// Refreshed only on display-change/overlay-create, not every tick.
    pub cached_taskbar_rects: Vec<RECT>,
    /// Last alpha sent to `SetLayeredWindowAttributes`; skip DWM call if unchanged.
    pub last_applied_alpha: u8,
    /// Updated via WM_APP from WinEvent hook, not re-queried every tick.
    pub fullscreen_active: bool,
    /// Updated each fade tick via SHAppBarMessage.
    pub taskbar_autohide: bool,

    // ── Heavy-poll throttling ─────────────────────────────────────────────────
    /// Wrapping counter; expensive Win32 calls only fire every N ticks (~500 ms idle).
    idle_heavy_tick: u32,
    /// Tick of the last HWND_TOPMOST call; throttles Z-order enforcement to Z_ORDER_BACKUP_INTERVAL_MS.
    last_zorder_enforce_tick: u64,
    /// Pre-raise Z-position per overlay (1-based; u32::MAX = not found). Logged only on change.
    last_logged_zpos: Vec<u32>,

    /// Tick deadline for taskbar reflow stabilisation.
    reposition_until_tick: u64,
    /// True while overlays are hidden waiting for the taskbar to settle.
    pub is_repositioning: bool,
}

/// WinEvent hook handles. Unregisters all hooks on drop.
pub struct ZOrderWinEventHooks {
    _hooks: Vec<HWINEVENTHOOK>,
}

impl Drop for ZOrderWinEventHooks {
    fn drop(&mut self) {
        unsafe {
            for &h in &self._hooks {
                if !h.is_invalid() {
                    UnhookWinEvent(h);
                }
            }
        }
    }
}

/// Install WinEvent hooks for foreground-change, minimize, and shell-panel-show events.
/// Uses WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS — no in-process DLL needed.
pub unsafe fn install_zorder_winevent_hooks() -> ZOrderWinEventHooks {
    let h_fg = SetWinEventHook(
        EVENT_SYSTEM_FOREGROUND,
        EVENT_SYSTEM_FOREGROUND,
        None,
        Some(zorder_winevent_proc),
        0, 0,
        WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
    );
    let h_min = SetWinEventHook(
        EVENT_SYSTEM_MINIMIZEEND,
        EVENT_SYSTEM_MINIMIZEEND,
        None,
        Some(zorder_winevent_proc),
        0, 0,
        WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
    );

    let h_show = SetWinEventHook(
        EVENT_OBJECT_SHOW,
        EVENT_OBJECT_SHOW,
        None,
        Some(object_show_proc),
        0, 0,
        WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
    );
    let mut hooks = Vec::new();
    if !h_fg.is_invalid()   { hooks.push(h_fg); }
    if !h_min.is_invalid()  { hooks.push(h_min); }
    if !h_show.is_invalid() { hooks.push(h_show); }
    ZOrderWinEventHooks { _hooks: hooks }
}

/// WinEvent proc for EVENT_OBJECT_SHOW: posts WM_APP_RAISE_OVERLAYS when a displacing shell panel appears.
unsafe extern "system" fn object_show_proc(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    id_object: i32,
    _id_child: i32,
    _id_event_thread: u32,
    _event_time: u32,
) {
    if id_object != 0 { return; }
    if hwnd.0.is_null() { return; }

    let mut cls_buf = [0u16; 32];
    let cls_len = GetClassNameW(hwnd, &mut cls_buf) as usize;
    if cls_len == 0 || !is_displacing_shell_panel(&cls_buf[..cls_len]) {
        return;
    }

    let main_raw = MAIN_HWND_FOR_HOOK.load(std::sync::atomic::Ordering::Relaxed);
    if main_raw != 0 {
        let _ = PostMessageW(
            HWND(main_raw as *mut _),
            WM_APP_RAISE_OVERLAYS,
            WPARAM(0),
            LPARAM(0),
        );
    }
}

/// WinEvent proc for foreground/minimize events: posts WM_APP_FULLSCREEN_CHECK to the UI thread.
unsafe extern "system" fn zorder_winevent_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _id_event_thread: u32,
    _event_time: u32,
) {
    let main_raw = MAIN_HWND_FOR_HOOK.load(std::sync::atomic::Ordering::Relaxed);
    if main_raw != 0 {
        let _ = PostMessageW(
            HWND(main_raw as *mut _),
            WM_APP_FULLSCREEN_CHECK,
            WPARAM(hwnd.0 as usize),
            LPARAM(event as isize),
        );
    }

    if let Some(Ok(mut log)) = zorder_log().map(|m| m.lock()) {
        let kind = match event {
            e if e == EVENT_SYSTEM_FOREGROUND  => ZLogKind::ForegroundChange,
            e if e == EVENT_SYSTEM_MINIMIZEEND => ZLogKind::MinimizeEnd,
            _                                  => ZLogKind::Other,
        };

        let detail = if !hwnd.0.is_null() {
            let mut cls_buf = [0u16; 64];
            let cls_len = GetClassNameW(hwnd, &mut cls_buf) as usize;
            let cls = String::from_utf16_lossy(&cls_buf[..cls_len]);

            let title_len = GetWindowTextLengthW(hwnd) as usize;
            let mut title_buf = vec![0u16; title_len.min(80) + 2];
            let got = GetWindowTextW(hwnd, &mut title_buf) as usize;
            let title_raw = String::from_utf16_lossy(&title_buf[..got]);
            let title: std::borrow::Cow<str> = if title_raw.chars().count() > 50 {
                format!("{}…", title_raw.chars().take(50).collect::<String>()).into()
            } else {
                title_raw.as_str().into()
            };

            let prefix = match kind {
                ZLogKind::ForegroundChange => "[FG]",
                ZLogKind::MinimizeEnd      => "[MN]",
                _                          => "[??]",
            };
            format!("{} hwnd={:#010x}  [{}]  \"{}\"",
                prefix, hwnd.0 as usize, cls, title)
        } else {
            String::new()
        };

        log.push(
            unsafe { GetTickCount64() },
            kind,
            hwnd.0 as u64,
            detail,
        );
    }
}

// ── DimmerTab impl ────────────────────────────────────────────────────────────

impl DimmerTab {
    /// Creates all Win32 controls and restores persisted settings from `ini`.
    /// `main_hwnd` is used to arm/kill `TIMER_OVERLAY_FADE`.
    pub unsafe fn new(
        parent:    HWND,
        hinstance: HINSTANCE,
        dpi:       u32,
        font_normal:   HFONT,
        font_title:    HFONT,
        font_bold_val: HFONT,
        ini:       &mut ProfileManager,
        main_hwnd: HWND,
    ) -> Self {
        let s = |px: i32| -> i32 { (px as f32 * dpi as f32 / 96.0).round() as i32 };

        // Helper closures
        let mk_static = |text: PCWSTR, extra_style: u32| -> HWND {
            let h = CreateWindowExW(
                WS_EX_LEFT, w!("STATIC"), text,
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(extra_style),
                0, 0, 1, 1, parent, HMENU(ptr::null_mut()), hinstance, None,
            ).unwrap_or_default();
            SendMessageW(h, WM_SETFONT, WPARAM(font_normal.0 as usize), LPARAM(1));
            h
        };

        let mk_check = |text: PCWSTR, id: usize| -> HWND {
            let h = CreateWindowExW(
                WS_EX_LEFT, w!("BUTTON"), text,
                WS_CHILD | WS_VISIBLE | WS_TABSTOP
                    | WINDOW_STYLE((BS_AUTOCHECKBOX | BS_OWNERDRAW) as u32),
                0, 0, 1, 1, parent, HMENU(id as *mut _), hinstance, None,
            ).unwrap_or_default();
            SendMessageW(h, WM_SETFONT, WPARAM(font_normal.0 as usize), LPARAM(1));
            h
        };

        let mk_slider = |id: usize, lo: i32, hi: i32, init: i32| -> HWND {
            let h = CreateWindowExW(
                WS_EX_LEFT,
                w!("msctls_trackbar32"),
                w!(""),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP
                    | WINDOW_STYLE((TBS_HORZ | TBS_NOTICKS | TBS_FIXEDLENGTH) as u32),
                0, 0, 1, 1, parent, HMENU(id as *mut _), hinstance, None,
            ).unwrap_or_default();
            SendMessageW(h, TBM_SETRANGE, WPARAM(0), LPARAM(((hi << 16) | lo) as isize));
            SendMessageW(h, TBM_SETPOS,   WPARAM(1), LPARAM(init as isize));
            let thumb_px = (8u32 * dpi / 96).max(4);
            SendMessageW(h, TBM_SETTHUMBLENGTH, WPARAM(thumb_px as usize), LPARAM(0));

            SetWindowSubclass(h, Some(slider_subclass_proc), 1, 0);
            h
        };

        let mk_btn = |text: PCWSTR, id: usize| -> HWND {
            let h = CreateWindowExW(
                WS_EX_LEFT, w!("BUTTON"), text,
                WS_CHILD | WS_VISIBLE | WS_TABSTOP
                    | WINDOW_STYLE(BS_OWNERDRAW as u32),
                0, 0, 1, 1, parent, HMENU(id as *mut _), hinstance, None,
            ).unwrap_or_default();
            SendMessageW(h, WM_SETFONT, WPARAM(font_normal.0 as usize), LPARAM(1));
            h
        };

        // Create controls
        let h_lbl_dim_title = mk_static(w!("Taskbar Dimmer"), 0);
        let h_lbl_dim_sub   = mk_static(
            w!("Creates a dark overlay over the taskbar to help with burn-in."), 0);
        let h_chk_taskbar_dim = mk_check(w!("Enable Taskbar Dimmer"), IDC_CHK_TASKBAR_DIM);

        // Section headings — 11pt bold, matches tab_crush / tab_hotkeys.
        let font_sect = crate::ui_drawing::make_font_cached(w!("Segoe UI"), 11, dpi, true);

        let h_lbl_dim_sect  = mk_static(w!("Dim Level"), SS_NOPREFIX);
        SendMessageW(h_lbl_dim_sect, WM_SETFONT, WPARAM(font_sect.0 as usize), LPARAM(1));
        let h_sep_dim_sect  = mk_static(w!(""), SS_BLACKRECT);

        let h_lbl_dim_pct   = mk_static(w!("90%"), SS_CENTERIMAGE);

        let h_lbl_fade_sect      = mk_static(w!("Fade Timings"), SS_NOPREFIX);
        SendMessageW(h_lbl_fade_sect, WM_SETFONT, WPARAM(font_sect.0 as usize), LPARAM(1));
        let h_sep_fade_sect      = mk_static(w!(""), SS_BLACKRECT);
        let h_lbl_fade_in_title  = mk_static(w!("Fade in"), SS_CENTERIMAGE);
        let h_sld_fade_in        = mk_slider(IDC_SLD_FADE_IN,  0, 2000, 400);
        let h_lbl_fade_in_val    = mk_static(w!("400 ms"), SS_CENTERIMAGE);
        let h_lbl_fade_out_title = mk_static(w!("Fade out"), SS_CENTERIMAGE);
        let h_sld_fade_out       = mk_slider(IDC_SLD_FADE_OUT, 0, 2000, 50);
        let h_lbl_fade_out_val   = mk_static(w!("50 ms"), SS_CENTERIMAGE);
        let h_btn_dim_defaults   = mk_btn(w!("↺  Restore Defaults"), IDC_BTN_DIM_DEFAULTS);
        install_action_btn_hover(h_btn_dim_defaults);
        let h_sld_taskbar_dim    = mk_slider(IDC_SLD_TASKBAR_DIM, 0, 100, 90);

        // Title and value labels get special fonts.
        SendMessageW(h_lbl_dim_title,    WM_SETFONT, WPARAM(font_title.0    as usize), LPARAM(1));
        SendMessageW(h_lbl_dim_pct,      WM_SETFONT, WPARAM(font_bold_val.0 as usize), LPARAM(1));
        SendMessageW(h_lbl_fade_in_val,  WM_SETFONT, WPARAM(font_bold_val.0 as usize), LPARAM(1));
        SendMessageW(h_lbl_fade_out_val, WM_SETFONT, WPARAM(font_bold_val.0 as usize), LPARAM(1));

        // Restore persisted state.
        let enabled = ini.read_int("_state", "TaskbarDimEnabled", 0) != 0;
        SendMessageW(h_chk_taskbar_dim, BM_SETCHECK, WPARAM(enabled as usize), LPARAM(0));

        let show = if enabled { SW_SHOW } else { SW_HIDE };
        ShowWindow(h_lbl_dim_sub, SW_SHOW);
        ShowWindow(h_lbl_dim_sect,       show);
        ShowWindow(h_sep_dim_sect,       show);
        ShowWindow(h_sld_taskbar_dim,    show);
        ShowWindow(h_lbl_dim_pct,        show);
        ShowWindow(h_lbl_fade_sect,      show);
        ShowWindow(h_sep_fade_sect,      show);
        ShowWindow(h_lbl_fade_in_title,  show);
        ShowWindow(h_lbl_fade_out_title, show);
        ShowWindow(h_btn_dim_defaults,   show);

        let saved_dim_level = ini.read_int("_state", "TaskbarDimPct", 90).clamp(0, 100);
        SendMessageW(h_sld_taskbar_dim, TBM_SETPOS, WPARAM(1), LPARAM(saved_dim_level as isize));
        let pct_text: Vec<u16> = format!("{}%\0", saved_dim_level).encode_utf16().collect();
        let _ = SetWindowTextW(h_lbl_dim_pct, PCWSTR(pct_text.as_ptr()));

        let saved_fade_in  = ini.read_int("_state", "FadeInMs",  400).clamp(0, 2000);
        let saved_fade_out = ini.read_int("_state", "FadeOutMs", 50).clamp(0, 2000);
        SendMessageW(h_sld_fade_in,  TBM_SETPOS, WPARAM(1), LPARAM(saved_fade_in  as isize));
        SendMessageW(h_sld_fade_out, TBM_SETPOS, WPARAM(1), LPARAM(saved_fade_out as isize));
        ShowWindow(h_sld_fade_in,      show);
        ShowWindow(h_sld_fade_out,     show);
        ShowWindow(h_lbl_fade_in_val,  show);
        ShowWindow(h_lbl_fade_out_val, show);
        let fi_text: Vec<u16> = format!("{} ms\0", saved_fade_in).encode_utf16().collect();
        let _ = SetWindowTextW(h_lbl_fade_in_val,  PCWSTR(fi_text.as_ptr()));
        let fo_text: Vec<u16> = format!("{} ms\0", saved_fade_out).encode_utf16().collect();
        let _ = SetWindowTextW(h_lbl_fade_out_val, PCWSTR(fo_text.as_ptr()));

        // Overlay initialisation.
        let mut taskbar_overlays: Vec<HWND> = Vec::new();
        let (overlay_alpha_full, overlay_alpha_current, overlay_alpha_target,
             overlay_fade_start_alpha, overlay_fade_start_time);

        if enabled && saved_dim_level > 0 {
            let alpha = dim_level_to_alpha(saved_dim_level);
            overlay_alpha_full         = alpha as f32;
            overlay_alpha_current      = alpha as f32;
            overlay_alpha_target       = alpha as f32;
            overlay_fade_start_alpha   = alpha as f32;
            overlay_fade_start_time    = GetTickCount64();
            dim_all_taskbars(&mut taskbar_overlays, alpha);
            if !taskbar_overlays.is_empty() {
                let frame_ms = OVERLAY_FADE_INTERVAL_MS.load(std::sync::atomic::Ordering::Relaxed);
                let _ = SetTimer(main_hwnd, TIMER_OVERLAY_FADE, frame_ms, None);
            }
        } else {
            overlay_alpha_full         = 0.0;
            overlay_alpha_current      = 0.0;
            overlay_alpha_target       = 0.0;
            overlay_fade_start_alpha   = 0.0;
            overlay_fade_start_time    = 0;
        }

        let cached_taskbar_rects = collect_taskbar_rects();
        let fullscreen_active = is_fullscreen_on_monitor(GetForegroundWindow());

        let grp_dim_controls = ControlGroup::new(vec![
            h_lbl_dim_sect, h_sep_dim_sect, h_lbl_dim_pct,
            h_sld_taskbar_dim,
            h_lbl_fade_sect, h_sep_fade_sect,
            h_lbl_fade_in_title,  h_sld_fade_in,  h_lbl_fade_in_val,
            h_lbl_fade_out_title, h_sld_fade_out, h_lbl_fade_out_val,
            h_btn_dim_defaults,
        ]);

        Self {
            h_lbl_dim_title,
            h_lbl_dim_sub,
            h_chk_taskbar_dim,
            h_lbl_dim_sect,
            h_sep_dim_sect,
            h_lbl_dim_pct,
            h_sld_taskbar_dim,
            h_lbl_fade_sect,
            h_sep_fade_sect,
            h_lbl_fade_in_title,
            h_sld_fade_in,
            h_lbl_fade_in_val,
            h_lbl_fade_out_title,
            h_sld_fade_out,
            h_lbl_fade_out_val,
            h_btn_dim_defaults,
            grp_dim_controls,
            enabled,
            taskbar_overlays,
            overlay_alpha_current,
            overlay_alpha_target,
            overlay_alpha_full,
            overlay_fade_start_time,
            overlay_fade_start_alpha,
            overlay_fade_in_ms:  saved_fade_in  as f32,
            overlay_fade_out_ms: saved_fade_out as f32,
            // Defaults: suppress (hide) overlay when fullscreen/auto-hide active.
            suppress_fs_enabled:   true,
            suppress_ah_enabled:   true,
            main_hwnd,
            cached_taskbar_rects,
            last_applied_alpha: 0,
            fullscreen_active,
            taskbar_autohide: false,
            idle_heavy_tick: 0,
            last_zorder_enforce_tick: 0,
            last_logged_zpos: Vec::new(),
            reposition_until_tick: 0,
            is_repositioning: false,
        }
    }

    // ── Command handlers ──────────────────────────────────────────────────────

    /// Handles "Enable Taskbar Dimmer" checkbox click. Returns `(status, is_ok)`.
    pub unsafe fn on_checkbox_toggled(
        &mut self,
        hwnd: HWND,
        ini: &mut ProfileManager,
    ) -> (&'static str, bool) {
        self.enabled = !self.enabled;
        SendMessageW(self.h_chk_taskbar_dim,
            BM_SETCHECK, WPARAM(self.enabled as usize), LPARAM(0));
        RedrawWindow(
            self.h_chk_taskbar_dim, None, None,
            RDW_INVALIDATE | RDW_UPDATENOW | RDW_ERASE);

        ini.write_int("_state", "TaskbarDimEnabled", if self.enabled { 1 } else { 0 });
        let _ = self.on_dim_slider_changed(hwnd, ini);
        InvalidateRect(self.h_sld_taskbar_dim, None, false);
        InvalidateRect(self.h_lbl_dim_pct,     None, false);
        ("", true)
    }

    /// Handles dim-level slider change (visuals + save). Used for programmatic calls.
    pub unsafe fn on_dim_slider_changed(
        &mut self,
        hwnd: HWND,
        ini: &mut ProfileManager,
    ) -> String {
        let msg = self.update_dim_visuals(hwnd);
        self.save_dim_slider(ini);
        msg
    }

    /// Updates overlay alpha and label from the slider. Does not write to disk.
    /// Returns a status string (empty = currently dimming).
    pub unsafe fn update_dim_visuals(&mut self, hwnd: HWND) -> String {
        let level = get_slider_val(self.h_sld_taskbar_dim).clamp(0, 100);
        let pct_text: Vec<u16> = format!("{}%\0", level).encode_utf16().collect();
        let _ = SetWindowTextW(self.h_lbl_dim_pct, PCWSTR(pct_text.as_ptr()));

        if self.enabled && level > 0 {
            let alpha = dim_level_to_alpha(level);
            self.overlay_alpha_full    = alpha as f32;
            self.overlay_alpha_current = alpha as f32;
            self.overlay_alpha_target  = alpha as f32;

            if self.taskbar_overlays.is_empty() {
                dim_all_taskbars(&mut self.taskbar_overlays, alpha);
                self.cached_taskbar_rects = collect_taskbar_rects();
                cache_tray_hwnd();
                self.start_fade_timer_if_needed();
            } else {
                if alpha != self.last_applied_alpha {
                    for &h in &self.taskbar_overlays {
                        if !h.0.is_null() {
                            let _ = SetLayeredWindowAttributes(h, COLORREF(0), alpha, LWA_ALPHA);
                        }
                    }
                }
            }

            self.last_applied_alpha = alpha;
            String::new()
        } else {
            destroy_all_overlays(&mut self.taskbar_overlays);
            self.cached_taskbar_rects.clear();
            KillTimer(hwnd, TIMER_OVERLAY_FADE);
            self.overlay_alpha_current = 0.0;
            self.overlay_alpha_target  = 0.0;
            self.overlay_alpha_full    = 0.0;
            self.last_applied_alpha    = 0;
            "Taskbar dim: off".to_owned()
        }
    }

    /// Persists dim-level to INI. Call once on TB_ENDTRACK.
    pub fn save_dim_slider(&mut self, ini: &mut ProfileManager) {
        let level = unsafe { get_slider_val(self.h_sld_taskbar_dim).clamp(0, 100) };
        ini.write_int("_state", "TaskbarDimPct", level);
    }

    /// Handles fade slider change (visuals + save). `is_in`: true = fade-in slider.
    pub unsafe fn on_fade_slider_changed(
        &mut self,
        is_in: bool,
        ini: &mut ProfileManager,
    ) {
        self.update_fade_visuals(is_in);
        self.save_fade_slider(is_in, ini);
    }

    /// Updates fade label and runtime ms from slider. Does not write to disk.
    pub unsafe fn update_fade_visuals(&mut self, is_in: bool) {
        let (h_sld, h_lbl) = if is_in {
            (self.h_sld_fade_in,  self.h_lbl_fade_in_val)
        } else {
            (self.h_sld_fade_out, self.h_lbl_fade_out_val)
        };
        let raw_ms = get_slider_val(h_sld).clamp(0, 2000);
        let ms = ((raw_ms + 12) / 25) * 25;
        if ms != raw_ms {
            SendMessageW(h_sld, TBM_SETPOS, WPARAM(1), LPARAM(ms as isize));
        }
        let text: Vec<u16> = format!("{} ms\0", ms).encode_utf16().collect();
        let _ = SetWindowTextW(h_lbl, PCWSTR(text.as_ptr()));
        if is_in {
            self.overlay_fade_in_ms  = ms as f32;
        } else {
            self.overlay_fade_out_ms = ms as f32;
        }
    }

    /// Persists fade slider to INI. Call once on TB_ENDTRACK.
    pub unsafe fn save_fade_slider(&mut self, is_in: bool, ini: &mut ProfileManager) {
        let h_sld = if is_in { self.h_sld_fade_in } else { self.h_sld_fade_out };
        let raw_ms = get_slider_val(h_sld).clamp(0, 2000);
        let ms = ((raw_ms + 12) / 25) * 25;
        if is_in {
            ini.write_int("_state", "FadeInMs",  ms);
        } else {
            ini.write_int("_state", "FadeOutMs", ms);
        }
    }

    /// Restore all dimmer controls to their default values.
    /// Returns a status string for the caller to display.
    pub unsafe fn restore_defaults(
        &mut self,
        hwnd: HWND,
        ini: &mut ProfileManager,
    ) -> String {
        const DEF_DIM:      i32 = 90;
        const DEF_FADE_IN:  i32 = 400;
        const DEF_FADE_OUT: i32 = 50;

        SendMessageW(self.h_sld_taskbar_dim,
            TBM_SETPOS, WPARAM(1), LPARAM(DEF_DIM as isize));
        let _ = self.on_dim_slider_changed(hwnd, ini);

        SendMessageW(self.h_sld_fade_in,
            TBM_SETPOS, WPARAM(1), LPARAM(DEF_FADE_IN as isize));
        self.on_fade_slider_changed(true, ini);

        SendMessageW(self.h_sld_fade_out,
            TBM_SETPOS, WPARAM(1), LPARAM(DEF_FADE_OUT as isize));
        self.on_fade_slider_changed(false, ini);

        InvalidateRect(self.h_sld_taskbar_dim, None, false);
        InvalidateRect(self.h_sld_fade_in,     None, false);
        InvalidateRect(self.h_sld_fade_out,    None, false);

        "Taskbar dimmer reset to defaults".to_owned()
    }

    // ── Fade timer tick ───────────────────────────────────────────────────────

    /// Drives overlay alpha transitions and state polling. Called every TIMER_OVERLAY_FADE tick.
    pub unsafe fn tick_fade(&mut self) {
        if self.taskbar_overlays.is_empty() { return; }

        if self.is_repositioning { return; }

        let is_animating = (self.overlay_alpha_current - self.overlay_alpha_target).abs() > 0.1;

        // Hover detection using cached rects — no EnumWindows on every tick.
        let hovering = if self.overlay_alpha_full > 0.5 {
            let mut cursor = POINT::default();
            GetCursorPos(&mut cursor);
            self.cached_taskbar_rects.iter().any(|r| {
                cursor.x >= r.left && cursor.x < r.right &&
                cursor.y >= r.top  && cursor.y < r.bottom
            })
        } else {
            false
        };

        // Heavy Win32 calls run every N ticks (~500 ms at idle).
        const HEAVY_POLL_EVERY_N_TICKS: u32 = 5;

        self.idle_heavy_tick = self.idle_heavy_tick.wrapping_add(1);
        let should_poll_heavy = is_animating || (self.idle_heavy_tick % HEAVY_POLL_EVERY_N_TICKS == 0);

        if should_poll_heavy {
            if self.suppress_ah_enabled {
                let mut abd = APPBARDATA {
                    cbSize: mem::size_of::<APPBARDATA>() as u32,
                    ..Default::default()
                };
                self.taskbar_autohide = (SHAppBarMessage(ABM_GETSTATE, &mut abd) as u32 & ABS_AUTOHIDE) != 0;
            } else {
                self.taskbar_autohide = false;
            }

            // Fullscreen state is maintained by WinEvent hook + one-shot recheck timer.
            // When suppression is disabled, force the cached value to false.
            if !self.suppress_fs_enabled {
                self.fullscreen_active = false;
            }
        }

        // Suppress conditions — read cached values only, no new Win32 calls.
        let fs_hiding = self.fullscreen_active && self.suppress_fs_enabled;
        let ah_hiding = self.taskbar_autohide  && self.suppress_ah_enabled;

        let new_target =
            if hovering || fs_hiding || ah_hiding { 0.0 }
            else { self.overlay_alpha_full };

        if (new_target - self.overlay_alpha_target).abs() > 0.5 {
            self.overlay_alpha_target     = new_target;
            self.overlay_fade_start_time  = GetTickCount64();
            self.overlay_fade_start_alpha = self.overlay_alpha_current;

            if let Some(Ok(mut log)) = zorder_log().map(|m| m.lock()) {
                let reason = match (hovering, fs_hiding, ah_hiding) {
                    (true,  _,     _    ) => "hover → fade out",
                    (_,     true,  _    ) => "fullscreen → suppress",
                    (_,     _,     true ) => "auto-hide → suppress",
                    _                     => "conditions cleared → fade in",
                };
                let payload: u64 =
                    (hovering   as u64)       |
                    (fs_hiding  as u64) << 1  |
                    (ah_hiding  as u64) << 2  |
                    (self.enabled as u64) << 3;
                let detail = format!(
                    "[DM] {} | target α={:.0} | hover={} fs={} ah={}",
                    reason,
                    new_target,
                    hovering, fs_hiding, ah_hiding,
                );
                log.push(GetTickCount64(), ZLogKind::DimmerStateChange, payload, detail);
            }
        }

        // Ease-out quadratic interpolation toward target.
        let fade_duration = if self.overlay_alpha_target >= self.overlay_fade_start_alpha {
            self.overlay_fade_in_ms.max(1.0)
        } else {
            self.overlay_fade_out_ms.max(1.0)
        };
        let elapsed  = GetTickCount64().wrapping_sub(self.overlay_fade_start_time) as f32;
        let progress = (elapsed / fade_duration).clamp(0.0, 1.0);
        let eased    = 1.0 - (1.0 - progress) * (1.0 - progress);
        self.overlay_alpha_current = (self.overlay_fade_start_alpha
            + (self.overlay_alpha_target - self.overlay_fade_start_alpha) * eased)
            .clamp(0.0, 255.0);
        if progress >= 1.0 {
            self.overlay_alpha_current = self.overlay_alpha_target;
        }

        let alpha = self.overlay_alpha_current.round() as u8;
        let alpha_changed = alpha != self.last_applied_alpha;

        // Periodic Z-order safety net.
        let now_ms = GetTickCount64();
        let zorder_due = alpha_changed || now_ms.wrapping_sub(self.last_zorder_enforce_tick)
            >= Z_ORDER_BACKUP_INTERVAL_MS as u64;

        if zorder_due && !self.fullscreen_active {
            raise_and_log_overlay_zpos(&self.taskbar_overlays, &mut self.last_logged_zpos);
        }

        for h in &self.taskbar_overlays {
            if h.0.is_null() { continue; }
            if alpha_changed {
                let _ = SetLayeredWindowAttributes(*h, COLORREF(0), alpha, LWA_ALPHA);
            }
        }

        if zorder_due {
            self.last_zorder_enforce_tick = now_ms;
        }
        if alpha_changed {
            self.last_applied_alpha = alpha;
        }

        // Dynamic timer: fast while animating/visible, slow at idle.
        let overlay_visible = self.overlay_alpha_current > 0.5
            || self.overlay_alpha_full > 0.5;  // dimmer enabled but hovering
        let frame_ms = OVERLAY_FADE_INTERVAL_MS.load(std::sync::atomic::Ordering::Relaxed);
        let next_ms: u32 = if is_animating || overlay_visible { frame_ms } else { 100 };
        let _ = SetTimer(self.main_hwnd, TIMER_OVERLAY_FADE, next_ms, None);
    }

    // ── Fullscreen cache update ───────────────────────────────────────────────

    /// Called on WM_APP_FULLSCREEN_CHECK (posted by `zorder_winevent_proc`).
    /// Runs an immediate fullscreen check and re-asserts overlay Z-order.
    /// Caller (app.rs) should also arm TIMER_FULLSCREEN_RECHECK for late-sizing games.
    pub unsafe fn on_fullscreen_check(&mut self, fg_hwnd: HWND) {
        self.fullscreen_active = is_fullscreen_on_monitor(fg_hwnd);

        if !self.fullscreen_active {
            raise_and_log_overlay_zpos(&self.taskbar_overlays, &mut self.last_logged_zpos);
            self.last_zorder_enforce_tick = GetTickCount64();
        }
    }

    /// Catch games that resize late after becoming foreground.
    pub unsafe fn on_fullscreen_recheck(&mut self) {
        if self.suppress_fs_enabled {
            self.fullscreen_active = is_fullscreen_on_monitor(GetForegroundWindow());
        }
    }

    // ── Debug helpers ─────────────────────────────────────────────────────────

    /// Force all overlays to Z-top immediately. Debug only — bound to '1' in the debug tab.
    pub unsafe fn force_raise_overlays(&mut self) {
        raise_and_log_overlay_zpos(&self.taskbar_overlays, &mut self.last_logged_zpos);
        self.last_zorder_enforce_tick = windows::Win32::System::SystemInformation::GetTickCount64();

        if let Some(Ok(mut log)) = zorder_log().map(|m| m.lock()) {
            log.push(
                windows::Win32::System::SystemInformation::GetTickCount64(),
                ZLogKind::OverlayRaised,
                u64::MAX, // u64::MAX = manual/debug trigger sentinel
                "ManualRaise (debug key '1')".to_owned(),
            );
        }
    }

    // ── Overlay reposition ────────────────────────────────────────────────────

    /// Hides overlays and starts polling for taskbar reflow stabilisation.
    pub unsafe fn start_reposition_overlays(&mut self) {
        if self.taskbar_overlays.is_empty() { return; }

        // Hide immediately to avoid showing the overlay at the old position.
        for h in &self.taskbar_overlays {
            if !h.0.is_null() {
                SetWindowPos(*h, HWND_TOPMOST, 0, 0, 0, 0,
                    SWP_HIDEWINDOW | SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }

        // Clear cache so the first tick_reposition sample is never treated as "settled".
        self.cached_taskbar_rects.clear();

        self.is_repositioning = true;

        // 5-second hard deadline; tick_reposition kills this timer early once stable.
        self.reposition_until_tick = GetTickCount64() + 5000;
        let _ = SetTimer(self.main_hwnd, TIMER_OVERLAY_REPOSITION, 100, None);
    }

    /// Polls until taskbar rect stabilises, then shows overlays at the new position.
    pub unsafe fn tick_reposition(&mut self) {
        if !self.enabled || self.taskbar_overlays.is_empty() {
            KillTimer(self.main_hwnd, TIMER_OVERLAY_REPOSITION);
            self.reposition_until_tick = 0;
            self.is_repositioning = false;
            return;
        }

        let new_rects = collect_taskbar_rects();

        // Settled = same count and every rect matches the previous sample.
        let settled = !new_rects.is_empty()
            && new_rects.len() == self.cached_taskbar_rects.len()
            && new_rects.iter().zip(self.cached_taskbar_rects.iter()).all(|(a, b)| {
                a.left == b.left && a.top == b.top
                    && a.right == b.right && a.bottom == b.bottom
            });

        // Always update the cache so the next tick compares against this sample.
        self.cached_taskbar_rects = new_rects;

        let now = GetTickCount64();
        let deadline_hit = self.reposition_until_tick != 0 && now >= self.reposition_until_tick;

        if settled || deadline_hit {
            let alpha = self.overlay_alpha_current.round() as u8;
            for (h, r) in self.taskbar_overlays.iter().zip(self.cached_taskbar_rects.iter()) {
                if h.0.is_null() { continue; }
                let w  = r.right  - r.left;
                let ht = r.bottom - r.top;
                let _ = SetWindowPos(*h, HWND_TOPMOST, r.left, r.top, w, ht,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOSENDCHANGING);
                let _ = SetLayeredWindowAttributes(*h, COLORREF(0), alpha, LWA_ALPHA);
            }

            raise_and_log_overlay_zpos(&self.taskbar_overlays, &mut self.last_logged_zpos);
            self.last_zorder_enforce_tick = now;

            KillTimer(self.main_hwnd, TIMER_OVERLAY_REPOSITION);
            self.reposition_until_tick = 0;
            self.is_repositioning = false;

            if let Some(Ok(mut log)) = zorder_log().map(|m| m.lock()) {
                let n = self.cached_taskbar_rects.len() as u64;
                let rects_str: Vec<String> = self.cached_taskbar_rects.iter().map(|r|
                    format!("({},{} {}x{})", r.left, r.top, r.right - r.left, r.bottom - r.top)
                ).collect();
                let reason = if deadline_hit { "deadline" } else { "settled" };
                let detail = format!(
                    "[SN] tick_reposition ({})  rects={}  positions=[{}]",
                    reason, n, rects_str.join(", ")
                );
                log.push(GetTickCount64(), ZLogKind::RepositionSafetyNet, n, detail);
            }
        } else {
            let _ = SetTimer(self.main_hwnd, TIMER_OVERLAY_REPOSITION, 100, None);
        }
    }

    /// Synchronously sync overlays to physical monitor rects.
    pub unsafe fn reposition_overlays_now(&mut self) {
        if !self.enabled || self.taskbar_overlays.is_empty() { return; }

        let t0 = GetTickCount64();

        self.cached_taskbar_rects = collect_taskbar_rects();

        let hinstance: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        while self.taskbar_overlays.len() < self.cached_taskbar_rects.len() {
            let cls = w!("BCT_TaskbarOverlay");
            let h = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                cls, PCWSTR(ptr::null()),
                WS_POPUP,
                0, 0, 1, 1,
                None, None, hinstance, None,
            ).unwrap_or_default();
            self.taskbar_overlays.push(h);
        }
        while self.taskbar_overlays.len() > self.cached_taskbar_rects.len() {
            if let Some(h) = self.taskbar_overlays.pop() {
                if !h.0.is_null() { DestroyWindow(h); }
            }
        }

        let alpha = self.overlay_alpha_current.round() as u8;
        for (h, r) in self.taskbar_overlays.iter().zip(self.cached_taskbar_rects.iter()) {
            if h.0.is_null() { continue; }
            let w  = r.right  - r.left;
            let ht = r.bottom - r.top;
            let _ = SetWindowPos(*h, HWND_TOPMOST, r.left, r.top, w, ht,
                SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOSENDCHANGING);
            let _ = SetLayeredWindowAttributes(*h, COLORREF(0), alpha, LWA_ALPHA);
        }

        self.last_applied_alpha = alpha;
        cache_tray_hwnd();

        if let Some(Ok(mut log)) = zorder_log().map(|m| m.lock()) {
            let elapsed = GetTickCount64().wrapping_sub(t0);
            let n = self.cached_taskbar_rects.len() as u64;
            let payload = (n << 32) | (elapsed & 0xFFFF_FFFF);
            let rects_str: Vec<String> = self.cached_taskbar_rects.iter().map(|r|
                format!("({},{} {}x{})", r.left, r.top, r.right - r.left, r.bottom - r.top)
            ).collect();
            let detail = format!(
                "[RP] reposition_overlays_now  rects={}  took={}ms  positions=[{}]",
                n, elapsed, rects_str.join(", ")
            );
            log.push(GetTickCount64(), ZLogKind::RepositionNow, payload, detail);
        }
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    unsafe fn start_fade_timer_if_needed(&self) {
        if !self.taskbar_overlays.is_empty() {
            let frame_ms = OVERLAY_FADE_INTERVAL_MS.load(std::sync::atomic::Ordering::Relaxed);
            let _ = SetTimer(self.main_hwnd, TIMER_OVERLAY_FADE, frame_ms, None);
        }
    }
}

// ── Z-order position logger ───────────────────────────────────────────────────

/// Returns 1-based Z-order positions for each overlay.
unsafe fn sample_overlay_zpos(overlays: &[HWND]) -> Vec<u32> {
    overlays.iter().map(|&ov| {
        if ov.0.is_null() { return u32::MAX; }
        let mut pos: u32 = 0;
        let mut cur = GetTopWindow(None).unwrap_or_default();
        loop {
            if cur.0.is_null() { break; }
            pos += 1;
            if cur == ov { return pos; }
            if pos > 8192 { break; }
            cur = GetWindow(cur, GW_HWNDNEXT).unwrap_or_default();
        }
        u32::MAX
    }).collect()
}

/// Samples Z-order *before* raising overlays, raises them, then logs only
/// overlays that weren't already at #1. Sampling pre-raise is essential —
/// post-raise every overlay reads #1 regardless of prior displacement.
/// `last` caches the last logged position per overlay to suppress repeat entries.
unsafe fn raise_and_log_overlay_zpos(overlays: &[HWND], last: &mut Vec<u32>) {
    if overlays.is_empty() { return; }

    // Grow cache; u32::MAX initial value ensures the first observation is always logged.
    if last.len() < overlays.len() {
        last.resize(overlays.len(), u32::MAX);
    }

    // 1. Sample before raise.
    let positions = sample_overlay_zpos(overlays);

    // 2. Raise all overlays.
    for &h in overlays {
        if h.0.is_null() { continue; }
        let _ = SetWindowPos(h, HWND_TOPMOST, 0, 0, 0, 0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOSENDCHANGING);
    }

    // 3. Log only changed, non-#1 positions (debug mode only).
    let Some(log_mutex) = zorder_log() else { return };
    let Ok(mut log) = log_mutex.lock() else { return };
    let tick = GetTickCount64();

    for (idx, &found) in positions.iter().enumerate() {
        if found == 1 || found == last[idx] { continue; }
        last[idx] = found;
        let payload = ((idx as u64) << 32) | (found as u64);
        log.push(tick, ZLogKind::OverlayZPos, payload, String::new());
    }
}

// ── Free functions (overlay management) ──────────────────────────────────────

/// Returns taskbar RECTs for all monitors. Call on startup, WM_DISPLAYCHANGE,
/// and overlay rebuild — not on every animation tick.
unsafe fn collect_taskbar_rects() -> Vec<RECT> {
    let mut rects = Vec::new();

    // EnumDisplayMonitors is faster than searching window handles per monitor.
    unsafe extern "system" fn monitor_enum_proc(
        hmonitor: HMONITOR,
        _: HDC,
        _: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        let rects = &mut *(lparam.0 as *mut Vec<RECT>);
        let mut mi = MONITORINFO::default();
        mi.cbSize = mem::size_of::<MONITORINFO>() as u32;

        if GetMonitorInfoW(hmonitor, &mut mi).as_bool() {
            let full = mi.rcMonitor;
            let work = mi.rcWork;

            if full.left != work.left || full.top != work.top || 
               full.right != work.right || full.bottom != work.bottom 
            {
                let mut tb_rect = RECT::default();
                
                if work.top > full.top { // Taskbar at TOP
                    tb_rect = RECT { left: full.left, top: full.top, right: full.right, bottom: work.top };
                } else if work.bottom < full.bottom { // Taskbar at BOTTOM
                    tb_rect = RECT { left: full.left, top: work.bottom, right: full.right, bottom: full.bottom };
                } else if work.left > full.left { // Taskbar at LEFT
                    tb_rect = RECT { left: full.left, top: full.top, right: work.left, bottom: full.bottom };
                } else if work.right < full.right { // Taskbar at RIGHT
                    tb_rect = RECT { left: work.right, top: full.top, right: full.right, bottom: full.bottom };
                }

                if tb_rect.left != tb_rect.right {
                    rects.push(tb_rect);
                }
            }
        }
        BOOL(1)
    }

    let _ = EnumDisplayMonitors(None, None, Some(monitor_enum_proc), LPARAM(&mut rects as *mut _ as isize));
    rects
}

fn dim_level_to_alpha(level: i32) -> u8 {
    // Piecewise: quadratic 0–60% for smooth low-end, linear 60–100% for step resolution.
    const SPLIT: f32 = 0.60;
    const ALPHA_AT_SPLIT: f32 = 255.0 * (1.0 - (1.0 - SPLIT) * (1.0 - SPLIT));

    let t = (level as f32 / 100.0).clamp(0.0, 1.0);
    let alpha = if t <= SPLIT {
        // Quadratic ease-in over [0, SPLIT].
        let t_norm = t / SPLIT;
        let inv = 1.0 - t_norm;
        ALPHA_AT_SPLIT * (1.0 - inv * inv)
    } else {
        // Linear ramp from ALPHA_AT_SPLIT → 255.
        let t_lin = (t - SPLIT) / (1.0 - SPLIT);
        ALPHA_AT_SPLIT + t_lin * (255.0 - ALPHA_AT_SPLIT)
    };
    alpha.round().clamp(0.0, 255.0) as u8
}

unsafe fn destroy_all_overlays(overlays: &mut Vec<HWND>) {
    for h in overlays.drain(..) {
        if !h.0.is_null() { DestroyWindow(h); }
    }
}

unsafe fn dim_all_taskbars(overlays: &mut Vec<HWND>, alpha: u8) {
    let rects = collect_taskbar_rects();

    let hinstance: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();

    while overlays.len() < rects.len() {
        let cls = w!("BCT_TaskbarOverlay");
        let wc = WNDCLASSEXW {
            cbSize:        mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc:   Some(overlay_wnd_proc),
            hInstance:     hinstance,
            hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
            lpszClassName: cls,
            ..Default::default()
        };
        RegisterClassExW(&wc);

        let h = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            cls, PCWSTR(ptr::null()),
            WS_POPUP,
            0, 0, 1, 1,
            None, None, hinstance, None,
        ).unwrap_or_default();
        overlays.push(h);
    }
    while overlays.len() > rects.len() {
        if let Some(h) = overlays.pop() {
            if !h.0.is_null() { DestroyWindow(h); }
        }
    }

    for (h, r) in overlays.iter().zip(rects.iter()) {
        if h.0.is_null() { continue; }
        let w  = r.right  - r.left;
        let ht = r.bottom - r.top;
        let _ = SetWindowPos(*h, HWND_TOPMOST, r.left, r.top, w, ht,
            SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOSENDCHANGING);
        let _ = SetLayeredWindowAttributes(*h, COLORREF(0), alpha, LWA_ALPHA);
    }
}

unsafe fn is_fullscreen_on_monitor(fg: HWND) -> bool {
    if fg.0.is_null() { return false; }

    // Cheap checks first — no string allocations. Most windowed apps fail here.
    // GetClassNameW is only reached for windows that actually cover the monitor.

    if !IsWindowVisible(fg).as_bool() { return false; }

    let hmon = MonitorFromWindow(fg, MONITOR_DEFAULTTONULL);
    if hmon.is_invalid() { return false; }

    let mut mi = MONITORINFO {
        cbSize: mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !GetMonitorInfoW(hmon, &mut mi).as_bool() { return false; }

    let mut wr = RECT::default();
    GetWindowRect(fg, &mut wr);

    let mr = mi.rcMonitor;
    let covers_monitor =
        (wr.left   - mr.left).abs()   <= 1 &&
        (wr.top    - mr.top).abs()    <= 1 &&
        (wr.right  - mr.right).abs()  <= 1 &&
        (wr.bottom - mr.bottom).abs() <= 1;

    if !covers_monitor { return false; }

    // Class-name check — filters shell/desktop windows that happen to be full-monitor-sized.
    let mut cls_buf = [0u16; 64];
    let cls_len = GetClassNameW(fg, &mut cls_buf) as usize;
    if cls_len > 0 {
        let cls = &cls_buf[..cls_len];
        if cls == CLS_TRAY
            || cls == CLS_SEC_TRAY
            || cls == CLS_PROGMAN
            || cls == CLS_WORKER_W
        {
            return false;
        }
    }

    true
}

// ── Overlay window proc ───────────────────────────────────────────────────────

unsafe extern "system" fn overlay_wnd_proc(
    hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut rc = RECT::default();
            GetClientRect(hwnd, &mut rc);
            FillRect(hdc, &rc, HBRUSH(GetStockObject(BLACK_BRUSH).0));
            EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}