// constants.rs — compile-time constants, colour palette, control IDs,
//               Win32 constants not exported by windows-rs, and shared types.

use windows::Win32::Foundation::COLORREF;

// ── Colours ──────────────────────────────────────────────────────────────────
// COLORREF is 0x00BBGGRR in Win32 (little-endian BGR).

pub const C_BG:     COLORREF = COLORREF(0x001A1A1A);
pub const C_BG3:    COLORREF = COLORREF(0x00303030);
pub const C_SEP:    COLORREF = COLORREF(0x00383838);
pub const C_FG:     COLORREF = COLORREF(0x00DDDDDD);
// C# C_LABEL = Color.FromArgb(0xAA,0xAA,0xAA) → BGR 0x00AAAAAA
pub const C_LABEL:  COLORREF = COLORREF(0x00AAAAAA);
// C# C_ACCENT = Color.FromArgb(0xFF,0xAA,0x00) orange → BGR 0x0000AAFF
pub const C_ACCENT: COLORREF = COLORREF(0x0000AAFF);
// C# C_WARN = Color.FromArgb(0xFF,0x66,0x00) → BGR 0x000066FF
pub const C_WARN:   COLORREF = COLORREF(0x000066FF);
// C# C_ERR = Color.FromArgb(0xFF,0x44,0x44) → BGR 0x004444FF
pub const C_ERR:    COLORREF = COLORREF(0x004444FF);

// ── Button colours (matching C# DarkButton) ───────────────────────────────────
pub const C_BTN_NORMAL:        COLORREF = COLORREF(0x002A2A2A);
// C_BTN_HOVER would be used here if WM_MOUSEMOVE tracking is added to owner-drawn buttons
pub const C_BTN_PRESS:         COLORREF = COLORREF(0x00222222);
pub const C_BTN_BORDER:        COLORREF = COLORREF(0x00555555);
pub const C_BTN_TEXT:          COLORREF = COLORREF(0x00DDDDDD);
// Active (compare held) — subtle warm-grey, just a step above the normal button.
// Background is a touch lighter than C_BTN_NORMAL (0x2A2A2A), border slightly
// brighter than C_BTN_BORDER (0x555555), text same as normal button text.
pub const C_BTN_ACTIVE:        COLORREF = COLORREF(0x00363636); // slightly lighter bg
pub const C_BTN_ACTIVE_PRESS:  COLORREF = COLORREF(0x002E2E2E); // pressed = back toward normal
pub const C_BTN_ACTIVE_BORDER: COLORREF = COLORREF(0x00707070); // brighter border, still grey
pub const C_BTN_ACTIVE_TEXT:   COLORREF = COLORREF(0x00DDDDDD); // same as C_BTN_TEXT

// Slider track groove colour
pub const C_SLIDER_TRACK: COLORREF = COLORREF(0x00555555);

// Combobox drop-button colours
#[allow(dead_code)] pub const C_DDL_BTN:   COLORREF = COLORREF(0x00585858);
pub const C_DDL_ARROW: COLORREF = COLORREF(0x00DDDDDD);

// ── Static control styles (not exported by windows-rs) ────────────────────────
#[allow(dead_code)] pub const SS_LEFT:        u32 = 0x0000_0000;
#[allow(dead_code)] pub const SS_CENTER:      u32 = 0x0000_0001;
#[allow(dead_code)] pub const SS_CENTERIMAGE: u32 = 0x0000_0200; // vertically centres text in static controls
pub const SS_NOPREFIX: u32 = 0x0000_0080;
pub const SS_BLACKRECT:u32 = 0x0000_0004;

// ── TrackBar messages ─────────────────────────────────────────────────────────
// TBM_GETPOS is NOT exported by windows-rs (only PBM_GETPOS exists there).
pub const TBM_GETPOS: u32 = 0x0400;
// TBM_GETRANGEMIN, TBM_GETRANGEMAX, TBM_SETRANGE, TBM_SETPOS, TBM_SETTHUMBLENGTH,
// TBS_HORZ, TBS_FIXEDLENGTH, TBS_NOTICKS are all available via Controls::*.

// ── Control IDs ──────────────────────────────────────────────────────────────

pub const IDC_SLD_BLACK:   usize = 101;
pub const IDC_SLD_SQUARES: usize = 102;
pub const IDC_BTN_TOGGLE:  usize = 103;
pub const IDC_CHK_STARTUP: usize = 110;
pub const IDC_BTN_QUIT:    usize = 111;
pub const IDC_DDL_REFRESH:  usize = 112;
pub const IDC_HDR_PANEL:       usize = 113;
pub const IDC_BTN_MINIMIZE:    usize = 114;
#[allow(dead_code)]
pub const IDC_CHK_TASKBAR_DIM: usize = 115;
pub const IDC_SLD_TASKBAR_DIM: usize = 116;
pub const IDC_SLD_FADE_IN:    usize = 118;
pub const IDC_SLD_FADE_OUT:   usize = 119;
pub const IDC_BTN_DIM_DEFAULTS: usize = 120;
/// Left-panel vertical navigation buttons (one per tab).
pub const IDC_NAV_BTN_0: usize = 121;
pub const IDC_NAV_BTN_1: usize = 122;

// ── Tab control messages / notifications — re-exported from windows-rs (Controls::*)
// TCM_INSERTITEMW, TCM_GETCURSEL, TCN_SELCHANGE, TCIF_TEXT, TCS_FLATBUTTONS
// are all available via `windows::Win32::UI::Controls::*` — do not redefine them here.

// ── Window messages ───────────────────────────────────────────────────────────
use windows::Win32::UI::WindowsAndMessaging::WM_USER;
pub const WM_TRAY_CALLBACK: u32  = WM_USER + 1;
// WM_DISPLAYCHANGE (0x007E) is exported by windows-rs WindowsAndMessaging::* — do not redefine.
pub const WM_COMPARE_START: u32  = WM_USER + 2; // sent by btn subclass on mouse-down
pub const WM_COMPARE_END:   u32  = WM_USER + 3; // sent by btn subclass on mouse-up / leave
// ── Timer IDs ─────────────────────────────────────────────────────────────────
pub const TIMER_HDR:          usize = 1;
pub const TIMER_RENDER:       usize = 2;
pub const TIMER_SCROLL_REFRESH: usize = 4; // one-shot: scroll refresh-rate dropdown to top after open
pub const TIMER_OVERLAY_FADE: usize = 5; // ~16 ms tick driving overlay hover-fade animation
pub const TIMER_OVERLAY_REPOSITION: usize = 9; // repositioning on display/refresh-rate change (5 sec window)
/// One-shot ~50 ms debounce timer — fires `apply_ramp` after the black-level
/// slider settles, instead of calling `SetDeviceGammaRamp` on every drag tick.
pub const TIMER_RAMP_APPLY: usize = 10;


// ── Business logic limits ─────────────────────────────────────────────────────
pub const MAX_BLACK:     i32 = 15;
pub const DEFAULT_BLACK: i32 = 0;

// Minimum window size in logical pixels (96 DPI baseline).
pub const MIN_WIN_W: i32 = 700;
pub const MIN_WIN_H: i32 = 740;

// ── Startup shortcut ──────────────────────────────────────────────────────────
// CSIDL_STARTUP is exported by windows-rs Shell::* — do not redefine it here.

// ── Shared Win32 layout types ─────────────────────────────────────────────────
// TCITEMW and NMHDR are exported by windows-rs Controls::* — do not redefine them here.

pub const IDC_NAV_BTN_2:        usize = 1030;
pub const IDC_NAV_BTN_3:        usize = 1031;
pub const TIMER_DEBUG_REFRESH:        usize = 6;  // or next free timer ID
pub const TIMER_STATUS_CLEAR:         usize = 7;  // one-shot: hide the normal status label after a delay
/// One-shot timer fired ~1 s after a foreground event to re-check fullscreen
/// for games that resize *after* EVENT_SYSTEM_FOREGROUND fires.
pub const TIMER_FULLSCREEN_RECHECK:   usize = 8;

/// If no foreground events, still realign overlays on this interval (ms).
#[allow(dead_code)]
pub const Z_ORDER_BACKUP_INTERVAL_MS: u32 = 100;

pub const IDC_CHK_SUPPRESS_FS:   u16 = 210; // pick free IDs
pub const IDC_CHK_SUPPRESS_AH:   u16 = 211;

pub const IDC_LST_ZLOG:      usize = 213;
pub const IDC_BTN_LOG_CLEAR: usize = 214;

pub const IDC_HK_EDT_TOGGLE_DIMMER: usize = 217;
pub const IDC_HK_EDT_TOGGLE_CRUSH:  usize = 218;
pub const IDC_HK_EDT_HOLD_COMPARE:  usize = 219;
pub const IDC_HK_EDT_DECREASE:      usize = 220;
pub const IDC_HK_EDT_INCREASE:      usize = 221;

/// "×" clear buttons — one per hotkey row.
pub const IDC_HK_CLR_TOGGLE_DIMMER: usize = 222;
pub const IDC_HK_CLR_TOGGLE_CRUSH:  usize = 223;
pub const IDC_HK_CLR_HOLD_COMPARE:  usize = 224;
pub const IDC_HK_CLR_DECREASE:      usize = 225;
pub const IDC_HK_CLR_INCREASE:      usize = 226;

pub const IDC_HK_EDT_DIM_DECREASE:  usize = 228;
pub const IDC_HK_EDT_DIM_INCREASE:  usize = 229;
pub const IDC_HK_CLR_DIM_DECREASE:  usize = 230;
pub const IDC_HK_CLR_DIM_INCREASE:  usize = 231;

/// About tab nav button.
pub const IDC_NAV_BTN_4: usize = 1032;

/// "Check for updates" button in the About tab.
pub const IDC_ABOUT_BTN_CHECK: usize = 240;

/// Hotkey ID for the debug-mode force-raise overlay action (key: '1').
/// Registered only while the debug tab is active; unregistered on tab switch.
pub const HK_DEBUG_FORCE_RAISE: i32 = 50;

// ── Debug tab: mouse click log ────────────────────────────────────────────────

/// Custom WM_USER message posted by the WH_MOUSE_LL hook proc to the UI thread.
/// wparam = button index (0 = LMB, 1 = RMB, 2 = MMB).
/// lparam = POINT packed as (x as u32 as isize) << 32 | (y as u32 as isize).
pub const WM_MOUSE_CLICK_LOG: u32 = WM_USER + 10;

/// Overlay Z-order position — key / value labels inside "Dimmer State".
// These IDs are defined for consistency but the debug tab creates Z-pos labels
// via its row() helper and tracks them by HWND, not by control ID.
#[allow(dead_code)] pub const IDC_DBG_ZPOS_KEY: usize = 241;
#[allow(dead_code)] pub const IDC_DBG_ZPOS_VAL: usize = 242;

/// Mouse click log IDs — kept for ABI stability but controls are now unified into h_lst_zlog.
#[allow(dead_code)] pub const IDC_LST_CLICK_LOG:   usize = 243;
#[allow(dead_code)] pub const IDC_BTN_CLICK_CLEAR: usize = 244;