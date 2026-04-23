// constants.rs — compile-time constants, colour palette, control IDs,
//               and Win32 constants not exported by windows-rs.

use windows::Win32::Foundation::COLORREF;

// ── Colours ───────────────────────────────────────────────────────────────────
// COLORREF format: 0x00BBGGRR (Win32 BGR, little-endian).

pub const C_BG:     COLORREF = COLORREF(0x001A1A1A);
pub const C_BG3:    COLORREF = COLORREF(0x00303030);
pub const C_SEP:    COLORREF = COLORREF(0x00383838);
pub const C_FG:     COLORREF = COLORREF(0x00DDDDDD);
pub const C_LABEL:  COLORREF = COLORREF(0x00AAAAAA); // #AAAAAA → BGR 0x00AAAAAA
pub const C_ACCENT: COLORREF = COLORREF(0x0000AAFF); // orange  → BGR 0x0000AAFF
pub const C_WARN:   COLORREF = COLORREF(0x000066FF); // #FF6600 → BGR 0x000066FF
pub const C_ERR:    COLORREF = COLORREF(0x004444FF); // #FF4444 → BGR 0x004444FF

// Button colours
pub const C_BTN_NORMAL:        COLORREF = COLORREF(0x002A2A2A);
pub const C_BTN_PRESS:         COLORREF = COLORREF(0x00222222);
pub const C_BTN_BORDER:        COLORREF = COLORREF(0x00555555);
pub const C_BTN_TEXT:          COLORREF = COLORREF(0x00DDDDDD);
pub const C_BTN_ACTIVE:        COLORREF = COLORREF(0x00363636); // slightly lighter bg
pub const C_BTN_ACTIVE_PRESS:  COLORREF = COLORREF(0x002E2E2E); // pressed → back toward normal
pub const C_BTN_ACTIVE_BORDER: COLORREF = COLORREF(0x00707070); // brighter border, still grey
pub const C_BTN_ACTIVE_TEXT:   COLORREF = COLORREF(0x00DDDDDD);

pub const C_SLIDER_TRACK: COLORREF = COLORREF(0x00555555);


pub const C_DDL_ARROW: COLORREF = COLORREF(0x00DDDDDD);

// ── Static control styles (not in windows-rs) ─────────────────────────────────
pub const SS_LEFT:        u32 = 0x0000_0000;
pub const SS_CENTER:      u32 = 0x0000_0001;
pub const SS_CENTERIMAGE: u32 = 0x0000_0200; // vertically centres text
pub const SS_NOPREFIX: u32 = 0x0000_0080;
pub const SS_BLACKRECT: u32 = 0x0000_0004;
pub const SS_NOTIFY:   u32 = 0x0000_0100;

// ── TrackBar messages ─────────────────────────────────────────────────────────
// TBM_GETPOS is not exported by windows-rs (only PBM_GETPOS exists there).
pub const TBM_GETPOS: u32 = 0x0400;

// ── Window messages ───────────────────────────────────────────────────────────
use windows::Win32::UI::WindowsAndMessaging::WM_USER;
pub const WM_TRAY_CALLBACK:    u32 = WM_USER + 1;
pub const WM_COMPARE_START:    u32 = WM_USER + 2;  // btn subclass: mouse-down
pub const WM_COMPARE_END:      u32 = WM_USER + 3;  // btn subclass: mouse-up / leave
pub const WM_MOUSE_CLICK_LOG:  u32 = WM_USER + 10; // WH_MOUSE_LL → UI thread; wparam=btn, lparam=POINT
pub const WM_DOWNLOAD_PROGRESS: u32 = WM_USER + 21; // wparam=bytes_received, lparam=total_bytes
pub const WM_DOWNLOAD_DONE:     u32 = WM_USER + 22; // wparam=1 success / 0 failure

// ── Timer IDs ─────────────────────────────────────────────────────────────────
pub const TIMER_HDR:                  usize = 1;
pub const TIMER_RENDER:               usize = 2;
pub const TIMER_DEBUG_REFRESH:        usize = 6;
pub const TIMER_STATUS_CLEAR:         usize = 7;  // one-shot: hide status label
pub const TIMER_FULLSCREEN_RECHECK:   usize = 8;  // ~1 s after foreground event
pub const TIMER_OVERLAY_REPOSITION:   usize = 9;  // 5 s window after display change
pub const TIMER_RAMP_APPLY:           usize = 10; // ~50 ms debounce after slider settles
pub const TIMER_SCROLL_REFRESH:       usize = 4;  // one-shot: scroll refresh dropdown to top
pub const TIMER_OVERLAY_FADE:         usize = 5;  // ~16 ms tick for overlay hover-fade
pub const TIMER_CRUSH_REPEAT:         usize = 11; // repeating tick while increase/decrease key is held

// ── Business logic limits ─────────────────────────────────────────────────────
pub const MIN_BLACK:     i32 = -150;
pub const MAX_BLACK:     i32 = 150;
pub const DEFAULT_BLACK: i32 = 0;

pub const MIN_WIN_W: i32 = 720; // logical px at 96 DPI
pub const MIN_WIN_H: i32 = 730;

pub const Z_ORDER_BACKUP_INTERVAL_MS: u32 = 100;

// ── Control IDs ──────────────────────────────────────────────────────────────

pub const IDC_SLD_BLACK:   usize = 101;
pub const IDC_SLD_SQUARES: usize = 102;
pub const IDC_BTN_TOGGLE:  usize = 103;
pub const IDC_CHK_STARTUP: usize = 110;
pub const IDC_BTN_QUIT:    usize = 111;
pub const IDC_DDL_REFRESH: usize = 112;
pub const IDC_HDR_PANEL:   usize = 113;
pub const IDC_BTN_MINIMIZE: usize = 114;
pub const IDC_CHK_TASKBAR_DIM: usize = 115;
pub const IDC_SLD_TASKBAR_DIM: usize = 116;
pub const IDC_SLD_FADE_IN:      usize = 118;
pub const IDC_SLD_FADE_OUT:     usize = 119;
pub const IDC_BTN_DIM_DEFAULTS: usize = 120;

// Nav buttons (one per tab)
pub const IDC_NAV_BTN_0: usize = 121;
pub const IDC_NAV_BTN_1: usize = 122;
pub const IDC_NAV_BTN_2: usize = 1030;
pub const IDC_NAV_BTN_3: usize = 1031; // debug (hidden in release mode)
pub const IDC_NAV_BTN_4: usize = 1032; // about
pub const IDC_NAV_BTN_5: usize = 1033; // system

pub const IDC_SYS_BTN_TASKBAR_AUTOHIDE: usize = 245;

pub const IDC_CHK_SUPPRESS_FS: u16 = 210;
pub const IDC_CHK_SUPPRESS_AH: u16 = 211;

pub const IDC_LST_ZLOG:      usize = 213;
pub const IDC_BTN_LOG_CLEAR: usize = 214;

// Hotkey edits
pub const IDC_HK_EDT_TOGGLE_CRUSH:   usize = 218;
pub const IDC_HK_EDT_HOLD_COMPARE:   usize = 219;
pub const IDC_HK_EDT_DECREASE:       usize = 220;
pub const IDC_HK_EDT_INCREASE:       usize = 221;
pub const IDC_HK_EDT_TOGGLE_DIMMER:  usize = 217;
pub const IDC_HK_EDT_TOGGLE_HDR:     usize = 232;

// Hotkey clear buttons
pub const IDC_HK_CLR_TOGGLE_DIMMER: usize = 222;
pub const IDC_HK_CLR_TOGGLE_CRUSH:  usize = 223;
pub const IDC_HK_CLR_HOLD_COMPARE:  usize = 224;
pub const IDC_HK_CLR_DECREASE:      usize = 225;
pub const IDC_HK_CLR_INCREASE:      usize = 226;
pub const IDC_HK_CLR_TOGGLE_HDR:    usize = 233;

pub const IDC_ABOUT_BTN_UPDATE: usize = 250;

/// Hotkey ID for debug-mode force-raise overlay (key '1'); registered only while debug tab is active.
pub const HK_DEBUG_FORCE_RAISE: i32 = 50;
