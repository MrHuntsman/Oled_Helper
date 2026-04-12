// ui_drawing.rs — GDI/GDI+ custom painting and subclassing.

#![allow(non_snake_case, clippy::too_many_lines, unused_variables,
         unused_mut, unused_assignments, unused_must_use)]

use std::{collections::HashMap, mem, ptr};
use std::result::Result::Ok;
use std::sync::{Mutex, OnceLock};

use windows::{
    core::*,
    Win32::{
        Foundation::*,
        Graphics::{Dwm::*, Gdi::*, GdiPlus::*},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::*,
            HiDpi::GetDpiForWindow,
            Input::KeyboardAndMouse::*,
            WindowsAndMessaging::*,
        },
    },
};

type SubclassProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM, usize, usize) -> LRESULT;

#[link(name = "comctl32")]
extern "system" {
    pub fn SetWindowSubclass(hwnd: HWND, pfn_subclass: Option<SubclassProc>,
                             uid_subclass: usize, dw_ref_data: usize) -> BOOL;
    pub fn RemoveWindowSubclass(hwnd: HWND, pfn_subclass: Option<SubclassProc>,
                                uid_subclass: usize) -> BOOL;
    pub fn DefSubclassProc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT;
}

// AlphaBlend lives in Msimg32 — link it explicitly.
#[link(name = "Msimg32")]
extern "system" {
    fn AlphaBlend(
        hdcdest:     HDC,
        xorigdest:   i32, yorigdest:   i32,
        wdest:       i32, hdest:       i32,
        hdcsrc:      HDC,
        xorigsrc:    i32, yorigsrc:    i32,
        wsrc:        i32, hsrcsrc:     i32,
        ftn:         BLENDFUNCTION,
    ) -> windows::Win32::Foundation::BOOL;
}

use crate::constants::*;

// ── Accent colour ─────────────────────────────────────────────────────────────
//
// DwmGetColorizationColor returns the Windows accent colour as 0xAARRGGBB.
// We convert it to a GDI COLORREF (0x00BBGGRR), falling back to the system
// highlight colour if DWM is unavailable.

pub unsafe fn get_accent_color() -> COLORREF {
    let mut color: u32 = 0;
    let mut opaque: windows::Win32::Foundation::BOOL = windows::Win32::Foundation::BOOL(0);
    if DwmGetColorizationColor(&mut color, &mut opaque).is_ok() {
        let r = ((color >> 16) & 0xFF) as u32;
        let g = ((color >>  8) & 0xFF) as u32;
        let b = ( color        & 0xFF) as u32;
        COLORREF((b << 16) | (g << 8) | r)
    } else {
        COLORREF(GetSysColor(COLOR_HIGHLIGHT))
    }
}

// ── Shared GDI+ helpers ───────────────────────────────────────────────────────
//
// Call gdip_init() once before any GDI+ draw call (cheap after first call).
// These live here so every painter in this file can share them without
// duplicating the startup boilerplate.

/// Ensure GDI+ is started for this process.  Safe to call multiple times.
pub unsafe fn gdip_init() {
    static GDIP_TOKEN: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    GDIP_TOKEN.get_or_init(|| {
        let mut token: usize = 0;
        let si = GdiplusStartupInput {
            GdiplusVersion:           1,
            DebugEventCallback:       0,
            SuppressBackgroundThread: false.into(),
            SuppressExternalCodecs:   false.into(),
        };
        GdiplusStartup(&mut token, &si, ptr::null_mut());
        token
    });
}

/// Lighten a COLORREF (0x00BBGGRR) by adding `amount` to each channel (clamped).
#[inline]
fn lighten_colorref(cr: COLORREF, amount: u32) -> COLORREF {
    let r = ((cr.0        & 0xFF) + amount).min(0xFF);
    let g = (((cr.0 >> 8) & 0xFF) + amount).min(0xFF);
    let b = (((cr.0 >>16) & 0xFF) + amount).min(0xFF);
    COLORREF(r | (g << 8) | (b << 16))
}

/// COLORREF (0x00BBGGRR) → GDI+ ARGB (0xAARRGGBB).
#[inline]
pub fn colorref_to_argb(cr: COLORREF, a: u8) -> u32 {
    let r = (cr.0        & 0xFF) as u8;
    let g = ((cr.0 >> 8) & 0xFF) as u8;
    let b = ((cr.0 >>16) & 0xFF) as u8;
    ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

/// Build a rounded-rectangle GDI+ path. Caller must call GdipDeletePath.
unsafe fn build_round_rect_path(x: i32, y: i32, w: i32, h: i32, radius: i32) -> *mut GpPath {
    let mut path: *mut GpPath = ptr::null_mut();
    GdipCreatePath(FillModeAlternate, &mut path);
    let d   = radius.min(w / 2).min(h / 2).max(1) * 2;
    let d_f = d as f32;
    let x_f = x as f32; let y_f = y as f32;
    let w_f = w as f32; let h_f = h as f32;
    GdipAddPathArc(path, x_f,             y_f,             d_f, d_f, 180.0, 90.0);
    GdipAddPathArc(path, x_f + w_f - d_f, y_f,             d_f, d_f, 270.0, 90.0);
    GdipAddPathArc(path, x_f + w_f - d_f, y_f + h_f - d_f, d_f, d_f,   0.0, 90.0);
    GdipAddPathArc(path, x_f,             y_f + h_f - d_f, d_f, d_f,  90.0, 90.0);
    GdipClosePathFigure(path);
    path
}

/// Fill a rounded rectangle using GDI+ (antialiased).
pub unsafe fn fill_round_rect(
    graphics: *mut GpGraphics, color: COLORREF, alpha: u8,
    x: i32, y: i32, w: i32, h: i32, radius: i32,
) {
    if w <= 0 || h <= 0 { return; }
    let mut brush: *mut GpSolidFill = ptr::null_mut();
    GdipCreateSolidFill(colorref_to_argb(color, alpha), &mut brush);
    let path = build_round_rect_path(x, y, w, h, radius);
    GdipFillPath(graphics, brush as _, path);
    GdipDeletePath(path);
    GdipDeleteBrush(brush as _);
}

pub unsafe fn draw_round_rect(
    graphics: *mut GpGraphics, color: COLORREF, alpha: u8, pen_width: f32,
    x: i32, y: i32, w: i32, h: i32, radius: i32,
) {
    if w <= 0 || h <= 0 { return; }
    let mut pen: *mut GpPen = ptr::null_mut();
    GdipCreatePen1(colorref_to_argb(color, alpha), pen_width, UnitPixel, &mut pen);
    let half = (pen_width / 2.0).round() as i32;
    let path = build_round_rect_path(x + half, y + half, w - half * 2, h - half * 2, radius);
    GdipDrawPath(graphics, pen, path);
    GdipDeletePath(path);
    GdipDeletePen(pen);
}

// ── Win32 utility helpers (shared with app.rs via pub) ────────────────────────

#[allow(dead_code)]
pub unsafe fn set_bounds(hwnd: HWND, x: i32, y: i32, w: i32, h: i32) {
    MoveWindow(hwnd, x, y, w.max(1), h.max(1), true);
}

#[derive(Hash, PartialEq, Eq)]
struct FontCacheKey {
    family: String,
    pt: i32,
    dpi: u32,
    bold: bool,
}

pub unsafe fn make_font_cached(face: PCWSTR, pt: i32, dpi: u32, bold: bool) -> HFONT {
    static FONT_CACHE: OnceLock<Mutex<HashMap<FontCacheKey, usize>>> = OnceLock::new();
    let cache = FONT_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = FontCacheKey {
        family: face.to_string().unwrap_or_default(),
        pt,
        dpi,
        bold,
    };
    let mut cache = cache.lock().unwrap();
    if let Some(&handle) = cache.get(&key) {
        return HFONT(handle as _);
    }
    let font = make_font(face, pt, dpi, bold);
    cache.insert(key, font.0 as usize);
    font
}

pub unsafe fn make_font(face: PCWSTR, pt: i32, dpi: u32, bold: bool) -> HFONT {
    let height = -(pt * dpi as i32) / 72;
    let weight = if bold { FW_BOLD.0 as i32 } else { FW_NORMAL.0 as i32 };
    CreateFontW(height, 0, 0, 0, weight, 0, 0, 0,
        DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32, CLEARTYPE_QUALITY.0 as u32,
        (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32, face)
}

pub unsafe fn get_slider_val(hwnd: HWND) -> i32 {
    SendMessageW(hwnd, TBM_GETPOS, WPARAM(0), LPARAM(0)).0 as i32
}

pub unsafe fn combo_selected_text(hwnd: HWND) -> Option<String> {
    let idx = SendMessageW(hwnd, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 as isize;
    if idx < 0 { return None; }
    let len = SendMessageW(hwnd, CB_GETLBTEXTLEN, WPARAM(idx as usize), LPARAM(0)).0;
    if len <= 0 { return None; }
    let mut buf = vec![0u16; (len + 1) as usize];
    SendMessageW(hwnd, CB_GETLBTEXT, WPARAM(idx as usize), LPARAM(buf.as_mut_ptr() as isize));
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..end]).to_string())
}


pub unsafe fn set_window_text(hwnd: HWND, text: &str) {
    crate::win32::set_text(hwnd, text);
}

pub unsafe fn client_size_of(hwnd: HWND) -> (i32, i32) {
    let mut rc = RECT::default();
    GetClientRect(hwnd, &mut rc);
    (rc.right, rc.bottom)
}

// ── Shared mouse-tracking helper ──────────────────────────────────────────────
//
unsafe fn begin_mouse_track(hwnd: HWND, prop: PCWSTR) -> bool {
    if !GetPropW(hwnd, prop).0.is_null() { return false; }
    let mut tme = TRACKMOUSEEVENT {
        cbSize:      mem::size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags:     TME_LEAVE,
        hwndTrack:   hwnd,
        dwHoverTime: 0,
    };
    TrackMouseEvent(&mut tme);
    SetPropW(hwnd, prop, HANDLE(1 as *mut _));
    InvalidateRect(hwnd, None, false);
    true
}

// ── Custom slider WndProc ────────────────────────────────────────────────────
//
// We subclass each native trackbar and intercept WM_PAINT to draw our own
// track+thumb. The native control still handles all input (drag, click, keys)
// and fires WM_HSCROLL — we only replace the visual output.

const SLIDER_ORIG_PROC_PROP:  PCWSTR = w!("BCT_SliderOrigProc");
const SLIDER_HOVER_PROP:      PCWSTR = w!("BCT_SliderHover");
const SLIDER_TRACK_HOVER_PROP: PCWSTR = w!("BCT_SliderTrackHover");
const SLIDER_DRAG_PROP:       PCWSTR = w!("BCT_SliderDrag");
const SLIDER_FILL_PROP:       PCWSTR = w!("BCT_SliderFill");
const SLIDER_ANIM_START_PROP:    PCWSTR = w!("BCT_SliderAnimStart");
const SLIDER_ANIM_STARTVAL_PROP: PCWSTR = w!("BCT_SliderAnimStartVal");

const SLIDER_ANIM_TIMER: usize = 0xBC7A;

const ANIM_DURATION_MS: f32 = 120.0;

pub static SLIDER_ANIM_INTERVAL_MS: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(8);

// Thumb is a fixed size — only the inner fill dot animates on hover.
const THUMB_D: i32 = 20;  // logical px, scaled by DPI

// timeGetTime: high-resolution ms tick counter from winmm.lib.
// GetTickCount has 10-15 ms granularity on some machines; timeGetTime is
// guaranteed 1 ms resolution after timeBeginPeriod(1), and even without
// that call it is typically ≤4 ms — far better than GetTickCount.
#[link(name = "winmm")]
extern "system" {
    fn timeGetTime() -> u32;
}

pub unsafe extern "system" fn slider_subclass_proc(
    hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM,
    _subclass_id: usize, _ref_data: usize,
) -> LRESULT {
    let call_orig = || -> LRESULT {
        DefSubclassProc(hwnd, msg, wp, lp)
    };

    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            if !hdc.0.is_null() {
                paint_slider(hwnd, hdc);
                EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),

        WM_MOUSEMOVE => {
            // Geometry must exactly match paint_slider.
            let hdc_tmp = GetDC(hwnd);
            let dpi = GetDeviceCaps(hdc_tmp, LOGPIXELSX).max(96);
            ReleaseDC(hwnd, hdc_tmp);
            let thumb_d  = (THUMB_D * dpi / 96).max(1);
            let thumb_hl = thumb_d / 2;
            let thumb_hr = thumb_d - thumb_hl;
            let mut rc  = RECT::default();
            GetClientRect(hwnd, &mut rc);
            let val = SendMessageW(hwnd, TBM_GETPOS,      WPARAM(0), LPARAM(0)).0 as i32;
            let min = SendMessageW(hwnd, TBM_GETRANGEMIN, WPARAM(0), LPARAM(0)).0 as i32;
            let max = SendMessageW(hwnd, TBM_GETRANGEMAX, WPARAM(0), LPARAM(0)).0 as i32;
            let range = (max - min).max(1);
            let track_l = thumb_hl + (1 * dpi / 96).max(1);
            let track_r = rc.right - thumb_hr - (1 * dpi / 96).max(1);
            let track_span = (track_r - track_l).max(1);
            let thumb_cx = track_l + ((val - min) * track_span + range / 2) / range;
            let mx = (lp.0 & 0xFFFF) as i16 as i32;
            let on_thumb = (mx - thumb_cx).abs() <= thumb_hl + (2 * dpi / 96).max(1);

            // Register WM_MOUSELEAVE once while cursor is anywhere inside the slider.
            begin_mouse_track(hwnd, w!("BCT_SliderTracking"));

            // Track-level hover — set as soon as mouse is anywhere on the slider.
            begin_mouse_track(hwnd, SLIDER_TRACK_HOVER_PROP);

            // SLIDER_HOVER_PROP tracks thumb-specific hover — drives the animation.
            let was_on_thumb = !GetPropW(hwnd, SLIDER_HOVER_PROP).0.is_null();
            if on_thumb != was_on_thumb {
                if on_thumb {
                    SetPropW(hwnd, SLIDER_HOVER_PROP, HANDLE(1 as *mut _));
                } else {
                    RemovePropW(hwnd, SLIDER_HOVER_PROP);
                }
                // Snapshot the current fill value and wall-clock time so the
                // delta-time interpolator has a well-defined start point.
                let cur_fp = GetPropW(hwnd, SLIDER_FILL_PROP).0 as isize;
                if cur_fp != 0 {
                    SetPropW(hwnd, SLIDER_ANIM_STARTVAL_PROP, HANDLE(cur_fp as *mut _));
                }
                SetPropW(hwnd, SLIDER_ANIM_START_PROP,
                    HANDLE(timeGetTime() as usize as *mut _));
                let anim_ms = SLIDER_ANIM_INTERVAL_MS.load(std::sync::atomic::Ordering::Relaxed);
                SetTimer(hwnd, SLIDER_ANIM_TIMER, anim_ms, None);
                InvalidateRect(hwnd, None, false);
            }

            // Drag.
            // slider_set_from_x sends WM_HSCROLL to the parent, which calls
            // InvalidateRect on this control.  Don't call it again here —
            // a second invalidate in the same message pump cycle causes a
            // duplicate repaint on every mouse-move pixel during drag.
            if GetCapture() == hwnd {
                slider_set_from_x(hwnd, mx);
            }
            LRESULT(0)
        }

        WM_MOUSELEAVE => {
            RemovePropW(hwnd, SLIDER_HOVER_PROP);
            RemovePropW(hwnd, SLIDER_TRACK_HOVER_PROP);
            RemovePropW(hwnd, w!("BCT_SliderTracking"));
            // Snapshot current fill and time so the shrink-back animation starts
            // from wherever the dot currently is, not from a stale stored value.
            let cur_fp = GetPropW(hwnd, SLIDER_FILL_PROP).0 as isize;
            if cur_fp != 0 {
                SetPropW(hwnd, SLIDER_ANIM_STARTVAL_PROP, HANDLE(cur_fp as *mut _));
            }
            SetPropW(hwnd, SLIDER_ANIM_START_PROP,
                HANDLE(timeGetTime() as usize as *mut _));
            // Keep animation timer running so thumb shrinks back smoothly.
            let anim_ms = SLIDER_ANIM_INTERVAL_MS.load(std::sync::atomic::Ordering::Relaxed);
            SetTimer(hwnd, SLIDER_ANIM_TIMER, anim_ms, None);
            InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            SetCapture(hwnd);
            SetPropW(hwnd, SLIDER_DRAG_PROP, HANDLE(1 as *mut _));
            let mx = (lp.0 & 0xFFFF) as i16 as i32;
            slider_set_from_x(hwnd, mx);
            InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let was_dragging = !GetPropW(hwnd, SLIDER_DRAG_PROP).0.is_null();
            if GetCapture() == hwnd { ReleaseCapture(); }
            RemovePropW(hwnd, SLIDER_DRAG_PROP);
            if was_dragging {
                let val = SendMessageW(hwnd, TBM_GETPOS, WPARAM(0), LPARAM(0)).0 as i32;
                if let Ok(parent) = GetParent(hwnd) {
                    if !parent.0.is_null() {
                        let wparam = WPARAM((TB_THUMBPOSITION as usize) | ((val as usize) << 16));
                        SendMessageW(parent, WM_HSCROLL, wparam, LPARAM(hwnd.0 as isize));
                    }
                }
            }
            InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        WM_TIMER if wp.0 == SLIDER_ANIM_TIMER => {
            let hdc_tmp = GetDC(hwnd);
            let dpi = GetDeviceCaps(hdc_tmp, LOGPIXELSX).max(96);
            ReleaseDC(hwnd, hdc_tmp);

            let is_hovering = !GetPropW(hwnd, SLIDER_HOVER_PROP).0.is_null();
            let is_dragging = !GetPropW(hwnd, SLIDER_DRAG_PROP).0.is_null();

            // Thumb is always THUMB_D — compute the inner area available for the fill dot.
            let border_w  = (2 * dpi / 96).max(1);
            let thumb_d   = (THUMB_D * dpi / 96).max(1);
            let inner_d   = (thumb_d - border_w * 2).max(2);

            // Idle: medium dot (6px logical). Hover/drag: expand to full inner area.
            let idle_dot  = (10 * dpi / 96).max(3);
            let hover_dot = inner_d;
            let target_fp = if is_hovering || is_dragging {
                hover_dot * 16
            } else {
                idle_dot * 16
            };

            // Bootstrap: if SLIDER_FILL_PROP has never been set, start at idle.
            let cur_fp_raw = GetPropW(hwnd, SLIDER_FILL_PROP).0 as isize;
            let cur_fp = if cur_fp_raw == 0 { idle_dot * 16 } else { cur_fp_raw as i32 };

            // Delta-time eased interpolation.

            let now_ms   = timeGetTime();
            let start_ms = GetPropW(hwnd, SLIDER_ANIM_START_PROP).0 as usize as u32;
            let elapsed  = now_ms.wrapping_sub(start_ms) as f32;
            let t        = (elapsed / ANIM_DURATION_MS).clamp(0.0, 1.0);
            // Ease-out cubic: fast at start, decelerates to target.
            let eased    = 1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t);

            // The start value for this transition is stored in SLIDER_ANIM_STARTVAL_PROP.
            // Fall back to cur_fp if it was never set (e.g. first hover before any tick).
            let start_fp_raw = GetPropW(hwnd, SLIDER_ANIM_STARTVAL_PROP).0 as isize;
            let start_fp = if start_fp_raw == 0 { cur_fp } else { start_fp_raw as i32 };

            let new_fp = if t >= 1.0 {
                target_fp
            } else {
                (start_fp as f32 + (target_fp - start_fp) as f32 * eased).round() as i32
            };

            SetPropW(hwnd, SLIDER_FILL_PROP, HANDLE(new_fp as *mut _));
            InvalidateRect(hwnd, None, false);

            if new_fp == target_fp {
                KillTimer(hwnd, SLIDER_ANIM_TIMER);
                RemovePropW(hwnd, SLIDER_ANIM_START_PROP);
                RemovePropW(hwnd, SLIDER_ANIM_STARTVAL_PROP);
            }
            LRESULT(0)
        }
        WM_NCDESTROY => {
            RemovePropW(hwnd, SLIDER_ORIG_PROC_PROP);
            RemovePropW(hwnd, SLIDER_HOVER_PROP);
            RemovePropW(hwnd, SLIDER_TRACK_HOVER_PROP);
            RemovePropW(hwnd, SLIDER_DRAG_PROP);
            RemovePropW(hwnd, SLIDER_FILL_PROP);
            RemovePropW(hwnd, SLIDER_ANIM_START_PROP);
            RemovePropW(hwnd, SLIDER_ANIM_STARTVAL_PROP);
            RemovePropW(hwnd, w!("BCT_SliderTracking"));
            RemoveWindowSubclass(hwnd, Some(slider_subclass_proc), 1);
            call_orig()
        }
        _ => call_orig(),
    }
}

/// Thumb diameter is fixed — returns the scaled physical pixel size.
unsafe fn slider_thumb_d(dpi: i32) -> i32 {
    (THUMB_D * dpi / 96).max(1)
}

/// Set slider position based on client coordinate.
pub unsafe fn slider_set_from_x(hwnd: HWND, mx: i32) {
    let mut rc = RECT::default();
    GetClientRect(hwnd, &mut rc);
    let w = rc.right;
    let hdc = GetDC(hwnd);
    let dpi = GetDeviceCaps(hdc, LOGPIXELSX).max(96);
    ReleaseDC(hwnd, hdc);

    // Use the thumb radius as the fixed margin.
    let thumb_r     = (THUMB_D * dpi / 96 / 2).max(1);
    let track_left  = thumb_r + (1 * dpi / 96).max(1);
    let track_right = w - thumb_r - (1 * dpi / 96).max(1);
    let track_span   = (track_right - track_left).max(1);

    let min = SendMessageW(hwnd, TBM_GETRANGEMIN, WPARAM(0), LPARAM(0)).0 as i32;
    let max = SendMessageW(hwnd, TBM_GETRANGEMAX, WPARAM(0), LPARAM(0)).0 as i32;
    let range = (max - min).max(1);

    let clamped = (mx - track_left).clamp(0, track_span);
    let mut new_val = min + (clamped * range + track_span / 2) / track_span;

    // Snap fade sliders to 25-ms steps.
    let id = GetDlgCtrlID(hwnd) as usize;
    if id == IDC_SLD_FADE_IN || id == IDC_SLD_FADE_OUT {
        new_val = ((new_val + 12) / 25) * 25;
    }

    SendMessageW(hwnd, TBM_SETPOS, WPARAM(1), LPARAM(new_val as isize));

    if let Ok(parent) = GetParent(hwnd) {
        if !parent.0.is_null() {
            let _ = PostMessageW(parent, WM_HSCROLL,
                WPARAM((TB_THUMBTRACK as usize) | ((new_val as usize) << 16)),
                LPARAM(hwnd.0 as isize));
        }
    }
}

pub unsafe fn paint_slider(hwnd: HWND, hdc: HDC) {
    let mut rc = RECT::default();
    GetClientRect(hwnd, &mut rc);
    let w = rc.right;
    let h = rc.bottom;

    let mem_dc  = CreateCompatibleDC(hdc);
    let mem_bmp = CreateCompatibleBitmap(hdc, w, h);
    let old_bmp = SelectObject(mem_dc, mem_bmp);

    let val = SendMessageW(hwnd, TBM_GETPOS,      WPARAM(0), LPARAM(0)).0 as i32;
    let min = SendMessageW(hwnd, TBM_GETRANGEMIN, WPARAM(0), LPARAM(0)).0 as i32;
    let max = SendMessageW(hwnd, TBM_GETRANGEMAX, WPARAM(0), LPARAM(0)).0 as i32;
    let range = (max - min).max(1);

    let dpi = GetDeviceCaps(hdc, LOGPIXELSX).max(96);
    let s = |px: i32| (px * dpi / 96).max(1);

    let is_thumb_hovering = !GetPropW(hwnd, SLIDER_HOVER_PROP).0.is_null();
    let is_track_hovering = !GetPropW(hwnd, SLIDER_TRACK_HOVER_PROP).0.is_null();
    let is_dragging       = !GetPropW(hwnd, SLIDER_DRAG_PROP).0.is_null();

    // Clear background.
    let bg_br = CreateSolidBrush(C_BG);
    FillRect(mem_dc, &rc, bg_br);
    DeleteObject(bg_br);

    let accent = get_accent_color();

    // ── Geometry ─────────────────────────────────────────────────────────────
    let thumb_d  = slider_thumb_d(dpi);
    // Split asymmetrically so left_half + right_half == thumb_d for any value,
    // eliminating the 1-px gap that occurs when thumb_d is odd at non-100% DPI.
    let thumb_hl = thumb_d / 2;
    let thumb_hr = thumb_d - thumb_hl;
    let track_h  = if is_track_hovering || is_dragging { s(6) } else { s(5) };
    let track_cy  = h / 2;
    let track_top = track_cy - track_h / 2;
    let track_l   = thumb_hl + s(1);
    let track_r_x = w - thumb_hr - s(1);
    let track_span = (track_r_x - track_l).max(1);

    // Round to nearest pixel so thumb never drifts ±1 px from its track position.
    let thumb_cx = track_l + ((val - min) * track_span + range / 2) / range;
    let thumb_x  = thumb_cx - thumb_hl;
    let thumb_y  = track_cy - thumb_hl;

    let r = track_h / 2;

    // Paint layers in Z-order to hide AA sub-pixel artifacts at track terminations.
    gdip_init();
    let mut g: *mut GpGraphics = ptr::null_mut();
    GdipCreateFromHDC(mem_dc, &mut g);
    GdipSetSmoothingMode(g, SmoothingModeAntiAlias);

    // Step 1: full grey track.
    let track_w = track_r_x - track_l;
    let empty_color = if is_track_hovering || is_dragging {
        COLORREF(0x00666666)
    } else {
        C_SLIDER_TRACK
    };
    if track_w > 0 {
        fill_round_rect(g, empty_color, 0xFF, track_l, track_top, track_w, track_h, r);
    }

    // Step 2: accent fill.
    let accent_w = thumb_cx - track_l;
    if accent_w > 0 {
        fill_round_rect(g, accent, 0xFF, track_l, track_top, accent_w, track_h, r);
    }

    let pad     = s(3);
    let inner_d = (thumb_d - pad * 2).max(2);
    let inner_x = thumb_x + pad;
    let inner_y = thumb_y + pad;

    // ── Cached GDI+ brushes ───────────────────────────────────────────────────
    // SAFETY: only ever accessed from the single UI thread.
    struct CachedBrush {
        argb:  u32,
        brush: *mut GpSolidFill,
    }
    impl CachedBrush {
        const fn empty() -> Self { CachedBrush { argb: 0, brush: ptr::null_mut() } }
        unsafe fn get(&mut self, argb: u32) -> *mut GpSolidFill {
            if self.brush.is_null() || self.argb != argb {
                if !self.brush.is_null() { GdipDeleteBrush(self.brush as _); }
                GdipCreateSolidFill(argb, &mut self.brush);
                self.argb = argb;
            }
            self.brush
        }
    }
    static mut CACHE_INNER:  CachedBrush = CachedBrush::empty();
    static mut CACHE_ACCENT: CachedBrush = CachedBrush::empty();

    // Step 3: thumb disc.
    let inner_argb  = colorref_to_argb(C_BG3, 0xFF);
    let inner_brush = (*std::ptr::addr_of_mut!(CACHE_INNER)).get(inner_argb);
    GdipFillEllipseI(g, inner_brush as _, thumb_x, thumb_y, thumb_d, thumb_d);

    // Step 4: animated accent dot.
    let idle_dot  = (10 * dpi / 96).max(3);
    let hover_dot = inner_d;
    let fill_raw  = GetPropW(hwnd, SLIDER_FILL_PROP).0 as isize;
    let fill_d = if fill_raw == 0 {
        idle_dot
    } else {
        ((fill_raw / 16) as i32).clamp(idle_dot, hover_dot)
    };
    let fill_cx = inner_x + inner_d / 2;
    let fill_cy = inner_y + inner_d / 2;
    let fill_x  = fill_cx - fill_d / 2;
    let fill_y  = fill_cy - fill_d / 2;
    let accent_argb  = colorref_to_argb(accent, 0xFF);
    let accent_brush = (*std::ptr::addr_of_mut!(CACHE_ACCENT)).get(accent_argb);
    GdipFillEllipseI(g, accent_brush as _, fill_x, fill_y, fill_d, fill_d);

    GdipDeleteGraphics(g);

    BitBlt(hdc, 0, 0, w, h, mem_dc, 0, 0, SRCCOPY);

    SelectObject(mem_dc, old_bmp);
    DeleteObject(mem_bmp);
    DeleteDC(mem_dc);
}

// ── Hold-to-compare button subclass ──────────────────────────────────────────
//
// Intercepts mouse down/up on the A/B button to implement hold-to-compare.
// SetCapture ensures WM_LBUTTONUP is received even if the mouse leaves the button.

const BTN_ORIG_PROC_PROP: PCWSTR = w!("BCT_BtnOrigProc");

pub unsafe extern "system" fn compare_btn_subclass_proc(
    hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM,
    _subclass_id: usize, _ref_data: usize,
) -> LRESULT {
    let call_orig = || -> LRESULT {
        DefSubclassProc(hwnd, msg, wp, lp)
    };

    match msg {
        WM_LBUTTONDOWN => {
            SetCapture(hwnd);
            InvalidateRect(hwnd, None, false);
            if let Ok(parent) = GetParent(hwnd) {
                PostMessageW(parent, WM_COMPARE_START, WPARAM(0), LPARAM(0));
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            if GetCapture() == hwnd { ReleaseCapture(); }
            InvalidateRect(hwnd, None, false);
            if let Ok(parent) = GetParent(hwnd) {
                PostMessageW(parent, WM_COMPARE_END, WPARAM(0), LPARAM(0));
            }
            LRESULT(0)
        }
        WM_CAPTURECHANGED => {
            if let Ok(parent) = GetParent(hwnd) {
                PostMessageW(parent, WM_COMPARE_END, WPARAM(0), LPARAM(0));
            }
            call_orig()
        }
        WM_NCDESTROY => {
            RemovePropW(hwnd, BTN_ORIG_PROC_PROP);
            RemoveWindowSubclass(hwnd, Some(compare_btn_subclass_proc), 1);
            call_orig()
        }
        _ => call_orig(),
    }
}

// ── Action button hover subclass ─────────────────────────────────────────────
//
// Tracks mouse hover for regular push buttons (Minimize, Exit, Restore Defaults,
// and the click-hold Compare button) so draw_dark_button_full can paint a
// distinct hover background.  Uses the same begin_mouse_track / WM_MOUSELEAVE
// pattern as the nav and slider subclasses.

/// Window property set to non-null while the mouse is over the button.
pub const ACTION_BTN_HOVER_PROP: PCWSTR = w!("BCT_ActionBtnHover");

/// Attach the hover-tracking subclass to an action button.
/// Call once after creating each push button (Minimize, Exit, Restore Defaults,
/// Compare).
pub unsafe fn install_action_btn_hover(hwnd: HWND) {
    SetWindowSubclass(hwnd, Some(action_btn_subclass_proc), 4, 0);
}

pub unsafe extern "system" fn action_btn_subclass_proc(
    hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM,
    _subclass_id: usize, _ref_data: usize,
) -> LRESULT {
    let call_orig = || DefSubclassProc(hwnd, msg, wp, lp);

    match msg {
        WM_MOUSEMOVE => {
            begin_mouse_track(hwnd, ACTION_BTN_HOVER_PROP);
            call_orig()
        }
        WM_MOUSELEAVE => {
            RemovePropW(hwnd, ACTION_BTN_HOVER_PROP);
            InvalidateRect(hwnd, None, false);
            call_orig()
        }
        WM_LBUTTONDOWN | WM_LBUTTONUP => {
            // Repaint immediately on press/release so the pressed state is snappy.
            InvalidateRect(hwnd, None, false);
            call_orig()
        }
        WM_NCDESTROY => {
            RemovePropW(hwnd, ACTION_BTN_HOVER_PROP);
            RemoveWindowSubclass(hwnd, Some(action_btn_subclass_proc), 4);
            call_orig()
        }
        _ => call_orig(),
    }
}

// ── Navigation button subclass ───────────────────────────────────────────────
//
// Intercepts WM_SETFOCUS and WM_KILLFOCUS so the owner-draw path in
// draw_nav_item can paint a keyboard-focus ring.  The focus state is stored as
// a Win32 window property (BCT_NavFocused) so draw_nav_item can read it without
// needing access to AppState.

const NAV_BTN_ORIG_PROC_PROP: PCWSTR = w!("BCT_NavBtnOrigProc");
pub const NAV_BTN_FOCUSED_PROP: PCWSTR = w!("BCT_NavFocused");
pub const NAV_BTN_HOVER_PROP:   PCWSTR = w!("BCT_NavHovered");

pub unsafe extern "system" fn nav_btn_subclass_proc(
    hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM,
    _subclass_id: usize, _ref_data: usize,
) -> LRESULT {
    let call_orig = || -> LRESULT {
        DefSubclassProc(hwnd, msg, wp, lp)
    };

    match msg {
        WM_SETFOCUS => {
            SetPropW(hwnd, NAV_BTN_FOCUSED_PROP, HANDLE(1 as *mut _));
            InvalidateRect(hwnd, None, false);
            call_orig()
        }
        WM_KILLFOCUS => {
            RemovePropW(hwnd, NAV_BTN_FOCUSED_PROP);
            InvalidateRect(hwnd, None, false);
            call_orig()
        }
        WM_MOUSEMOVE => {
            // Set hover flag and request WM_MOUSELEAVE tracking if not already doing so.
            begin_mouse_track(hwnd, NAV_BTN_HOVER_PROP);
            call_orig()
        }
        WM_MOUSELEAVE => {
            RemovePropW(hwnd, NAV_BTN_HOVER_PROP);
            InvalidateRect(hwnd, None, false);
            call_orig()
        }
        WM_NCDESTROY => {
            RemovePropW(hwnd, NAV_BTN_ORIG_PROC_PROP);
            RemovePropW(hwnd, NAV_BTN_FOCUSED_PROP);
            RemovePropW(hwnd, NAV_BTN_HOVER_PROP);
            RemoveWindowSubclass(hwnd, Some(nav_btn_subclass_proc), 1);
            call_orig()
        }
        _ => call_orig(),
    }
}

// ── Combobox subclass ─────────────────────────────────────────────────────────
//
// CBS_DROPDOWNLIST combobox — the selected-text area is drawn by the combobox
// itself (no edit child).  The native painter calls RedrawWindow with
// RDW_INVALIDATE | RDW_ERASE on selection changes, queuing WM_ERASEBKGND then
// WM_PAINT as separate messages.  The window is visible between them, so even a
// fast background fill in WM_ERASEBKGND produces a blank frame before the text
// reappears — visible as flicker.
//
// Solution: suppress WM_ERASEBKGND completely (return 1, draw nothing) and do
// all painting in WM_PAINT via a memory DC that is BitBlt'd in one atomic
// operation.  No intermediate state ever reaches the screen.
//
// The listbox popup (COMBOLBOX) is a separate HWND — subclassed via
// WM_CTLCOLORLISTBOX to suppress its white erase on fast mouse moves.
// WM_CTLCOLORLISTBOX passes through so native item colours are untouched.

const COMBO_ORIG_PROC_PROP: PCWSTR = w!("BCT_ComboOrigProc");

/// Paint the non-client border of a window in our dark style: 1px C_SEP rect.
unsafe fn combo_paint_border(hwnd: HWND) {
    let hdc = GetWindowDC(hwnd);
    if hdc.0.is_null() { return; }
    let mut rc = RECT::default();
    GetWindowRect(hwnd, &mut rc);
    let w = rc.right  - rc.left;
    let h = rc.bottom - rc.top;
    let hovered = !GetPropW(hwnd, w!("BCT_ComboHover")).0.is_null();
    let border_color = if hovered { COLORREF(0x00888888) } else { C_SEP };
    let pen = CreatePen(PS_SOLID, 1, border_color);
    let old_pen = SelectObject(hdc, pen);
    let old_br  = SelectObject(hdc, GetStockObject(NULL_BRUSH));
    Rectangle(hdc, 0, 0, w, h);
    SelectObject(hdc, old_pen);
    SelectObject(hdc, old_br);
    DeleteObject(pen);
    ReleaseDC(hwnd, hdc);
}

/// Render the combobox client area into `hdc` using a memory DC back-buffer
/// so the blit is atomic — no intermediate blank frame reaches the screen.
unsafe fn combo_paint_buffered(hwnd: HWND, hdc: HDC) {
    let mut rc = RECT::default();
    GetClientRect(hwnd, &mut rc);
    let w = rc.right;
    let h = rc.bottom;

    // ── Back-buffer ───────────────────────────────────────────────────────────
    let mem_dc  = CreateCompatibleDC(hdc);
    let mem_bmp = CreateCompatibleBitmap(hdc, w, h);
    let old_bmp = SelectObject(mem_dc, mem_bmp);

    let btn_w = GetSystemMetrics(SM_CXVSCROLL).max(16);
    let hovered  = !GetPropW(hwnd, w!("BCT_ComboHover")).0.is_null();
    let bg_color = if hovered { COLORREF(0x00363636) } else { C_BG3 };
    let bg_br = CreateSolidBrush(bg_color);

    // Text-field background.
    let txt_rc = RECT { left: 0, top: 0, right: w - btn_w, bottom: h };
    FillRect(mem_dc, &txt_rc, bg_br);

    // Selected item text.
    // The collapsed selected-item area: draw left-aligned matching list item style.
    // When the list is open, WM_DRAWITEM paints each row (including the selected one)
    // via draw_combo_item, so the split/right-align logic lives there.
    let sel = SendMessageW(hwnd, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0 as i32;
    if sel >= 0 {
        let mut buf = [0u16; 128];
        let len = SendMessageW(hwnd, CB_GETLBTEXT,
            WPARAM(sel as usize), LPARAM(buf.as_mut_ptr() as isize)).0 as usize;
        if len > 0 && len < buf.len() {
            SetBkMode(mem_dc, TRANSPARENT);
            let font = SendMessageW(hwnd, WM_GETFONT, WPARAM(0), LPARAM(0));
            let old_font = SelectObject(mem_dc, HGDIOBJ(font.0 as *mut _));

            // Find the " [" separator — Hz part left, suffix right in dimmer colour.
            let split = buf[..len].windows(2).position(|w| w[0] == b' ' as u16 && w[1] == b'[' as u16);
            let mut label_rc = RECT {
                left: txt_rc.left + 4, top: txt_rc.top,
                right: txt_rc.right - 2, bottom: txt_rc.bottom,
            };
            if let Some(sep) = split {
                SetTextColor(mem_dc, C_FG);
                DrawTextW(mem_dc, &mut buf[..sep], &mut label_rc,
                    DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOPREFIX);
                let suffix_start = sep + 1;
                SetTextColor(mem_dc, C_LABEL);
                DrawTextW(mem_dc, &mut buf[suffix_start..len], &mut label_rc,
                    DT_SINGLELINE | DT_VCENTER | DT_RIGHT | DT_NOPREFIX);
            } else {
                SetTextColor(mem_dc, C_FG);
                DrawTextW(mem_dc, &mut buf[..len], &mut label_rc,
                    DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOPREFIX);
            }
            SelectObject(mem_dc, old_font);
        }
    }

    // Drop-button background.
    let btn_rc = RECT { left: w - btn_w, top: 0, right: w, bottom: h };
    FillRect(mem_dc, &btn_rc, bg_br);
    DeleteObject(bg_br);

    // Downward triangle — GDI+ for smooth edges.
    let tw = ((btn_w * 4 / 10) | 1).max(5);
    let th = (tw / 2).max(2);
    let cx = btn_rc.left + btn_w / 2;
    let cy = h / 2;

    gdip_init();
    let mut gp: *mut GpGraphics = ptr::null_mut();
    GdipCreateFromHDC(mem_dc, &mut gp);
    GdipSetSmoothingMode(gp, SmoothingModeAntiAlias);
    let pts_f: [PointF; 3] = [
        PointF { X: (cx - tw / 2)     as f32, Y: (cy - th / 2) as f32 },
        PointF { X: (cx + tw / 2 + 1) as f32, Y: (cy - th / 2) as f32 },
        PointF { X: cx                as f32, Y: (cy + th / 2) as f32  },
    ];
    let arrow_argb = colorref_to_argb(C_DDL_ARROW, 0xFF);
    let mut arrow_br: *mut GpSolidFill = ptr::null_mut();
    GdipCreateSolidFill(arrow_argb, &mut arrow_br);
    GdipFillPolygon(gp, arrow_br as _, pts_f.as_ptr(), 3, FillModeAlternate);
    GdipDeleteBrush(arrow_br as _);
    GdipDeleteGraphics(gp);

    // ── Single atomic blit ────────────────────────────────────────────────────
    BitBlt(hdc, 0, 0, w, h, mem_dc, 0, 0, SRCCOPY);

    SelectObject(mem_dc, old_bmp);
    DeleteObject(mem_bmp);
    DeleteDC(mem_dc);
}

// ── Owner-draw combobox list item painter ─────────────────────────────────────
//
// Called from WM_DRAWITEM in app.rs for every item the combobox needs to paint,
// including both the collapsed selected-item area (ODA_SELECT with itemID == -1
// does not exist for CBS_OWNERDRAWFIXED — the collapsed face is a separate draw)
// and each row in the open dropdown list.
//
// Layout: "144 Hz" drawn left-aligned in C_FG, "[7]" drawn right-aligned in
// C_LABEL.  Items with no suffix draw the full text left-aligned in C_FG.
// The selected/highlighted row gets a solid accent background.

pub unsafe fn draw_combo_item(di: &DRAWITEMSTRUCT) {
    // ODA_DRAWENTIRE is 0x0001; ODA_SELECT is 0x0002; ODA_FOCUS is 0x0004.
    // Skip focus-only redraws — we don't draw a focus rectangle.
    if di.itemAction == ODA_FOCUS { return; }
    // itemID == u32::MAX means "no item" (empty combo or collapsed-face sentinel).
    if di.itemID == u32::MAX { return; }

    let hdc = di.hDC;
    let rc  = di.rcItem;

    let is_selected = (di.itemState.0 & ODS_SELECTED.0) != 0;

    // Background.
    let bg = if is_selected { COLORREF(0x00CC6600) } else { C_BG3 };
    let bg_br = CreateSolidBrush(bg);
    FillRect(hdc, &rc, bg_br);
    DeleteObject(bg_br);

    // Fetch item text.
    let parent = GetParent(di.hwndItem).unwrap_or_default();
    let mut buf = [0u16; 128];
    let len = SendMessageW(di.hwndItem, CB_GETLBTEXT,
        WPARAM(di.itemID as usize), LPARAM(buf.as_mut_ptr() as isize)).0 as usize;
    if len == 0 || len >= buf.len() { return; }

    SetBkMode(hdc, TRANSPARENT);
    let font = SendMessageW(di.hwndItem, WM_GETFONT, WPARAM(0), LPARAM(0));
    let old_font = SelectObject(hdc, HGDIOBJ(font.0 as *mut _));

    // Inset rect with 4px left pad, 2px right pad.
    let mut label_rc = RECT {
        left:   rc.left + 4,
        top:    rc.top,
        right:  rc.right - 2,
        bottom: rc.bottom,
    };

    // Find " [" separator.
    let split = buf[..len].windows(2)
        .position(|w| w[0] == b' ' as u16 && w[1] == b'[' as u16);

    let fg    = if is_selected { C_FG } else { C_FG };
    let label = if is_selected { C_FG } else { C_LABEL };

    if let Some(sep) = split {
        SetTextColor(hdc, fg);
        DrawTextW(hdc, &mut buf[..sep], &mut label_rc,
            DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOPREFIX);
        let suffix_start = sep + 1; // skip the space before '['
        SetTextColor(hdc, label);
        DrawTextW(hdc, &mut buf[suffix_start..len], &mut label_rc,
            DT_SINGLELINE | DT_VCENTER | DT_RIGHT | DT_NOPREFIX);
    } else {
        SetTextColor(hdc, fg);
        DrawTextW(hdc, &mut buf[..len], &mut label_rc,
            DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOPREFIX);
    }

    SelectObject(hdc, old_font);
}

// ── Listbox popup subclass ─────────────────────────────────────────────────────
//
// Suppresses white erase on the COMBOLBOX popup on fast mouse moves.
// All item-draw / scroll / mouse messages pass through unchanged.

unsafe extern "system" fn combo_listbox_subclass_proc(
    hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM,
    _subclass_id: usize, _ref_data: usize,
) -> LRESULT {
    let call_orig = || DefSubclassProc(hwnd, msg, wp, lp);
    match msg {
        WM_ERASEBKGND => {
            let hdc = HDC(wp.0 as *mut _);
            let mut rc = RECT::default();
            GetClientRect(hwnd, &mut rc);
            let br = CreateSolidBrush(C_BG3);
            FillRect(hdc, &rc, br);
            DeleteObject(br);
            LRESULT(1)
        }
        WM_NCPAINT => {
            combo_paint_border(hwnd);
            LRESULT(0)
        }
        WM_NCDESTROY => {
            RemovePropW(hwnd, w!("BCT_LBSubclassed"));
            RemoveWindowSubclass(hwnd, Some(combo_listbox_subclass_proc), 2);
            call_orig()
        }
        _ => call_orig(),
    }
}

pub unsafe extern "system" fn combo_subclass_proc(
    hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM,
    _subclass_id: usize, _ref_data: usize,
) -> LRESULT {
    let call_orig = || -> LRESULT { DefSubclassProc(hwnd, msg, wp, lp) };

    match msg {
        // Suppress erase entirely — WM_PAINT does a back-buffered atomic blit
        // so no intermediate blank frame ever reaches the screen.
        WM_ERASEBKGND => LRESULT(1),

        // Suppress the native themed border; stamp our own flat 1px one.
        WM_NCPAINT => {
            combo_paint_border(hwnd);
            LRESULT(0)
        }

        // Re-stamp our border after any hover-driven NC repaint.
        WM_MOUSEMOVE => {
            begin_mouse_track(hwnd, w!("BCT_ComboHover"));
            let r = call_orig();
            combo_paint_border(hwnd);
            r
        }
        WM_MOUSELEAVE => {
            RemovePropW(hwnd, w!("BCT_ComboHover"));
            let r = call_orig();
            combo_paint_border(hwnd);
            r
        }

        // Own the full paint cycle. Back-buffered blit = no flicker.
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            if !hdc.0.is_null() {
                combo_paint_buffered(hwnd, hdc);
            }
            EndPaint(hwnd, &ps);
            combo_paint_border(hwnd);
            LRESULT(0)
        }

        // Install the listbox subclass before the popup erases itself.
        // Pass through so native item colours are completely unaffected.
        WM_CTLCOLORLISTBOX => {
            let h_listbox = HWND(lp.0 as *mut _);
            if !h_listbox.0.is_null() {
                let already = !GetPropW(h_listbox, w!("BCT_LBSubclassed")).0.is_null();
                if !already {
                    SetPropW(h_listbox, w!("BCT_LBSubclassed"), HANDLE(1 as *mut _));
                    SetWindowSubclass(h_listbox, Some(combo_listbox_subclass_proc), 2, 0);
                }
            }
            call_orig()
        }

        WM_NCDESTROY => {
            RemovePropW(hwnd, COMBO_ORIG_PROC_PROP);
            RemovePropW(hwnd, w!("BCT_ComboHover"));
            RemoveWindowSubclass(hwnd, Some(combo_subclass_proc), 1);
            call_orig()
        }

        _ => call_orig(),
    }
}

// ── Bitmap static label subclass ─────────────────────────────────────────────
//
// Paints an HBITMAP (pre-multiplied BGRA, same format as NavIcons) centred
// inside a plain STATIC control using AlphaBlend.  The bitmap handle is stored
// in the subclass ref-data parameter so no extra allocation is needed.
//
// Usage:
//   install_bitmap_static(hwnd, hbmp);   // call once after control creation

pub unsafe fn install_bitmap_static(hwnd: HWND, hbmp: Option<HBITMAP>) {
    if let Some(bmp) = hbmp {
        SetWindowSubclass(hwnd, Some(bitmap_static_subclass_proc), 3, bmp.0 as usize);
    }
}

pub unsafe extern "system" fn bitmap_static_subclass_proc(
    hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM,
    _id: usize, ref_data: usize,
) -> LRESULT {
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);

            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let w = rc.right  - rc.left;
            let h = rc.bottom - rc.top;

            // Fill background with the app background colour.
            let bg_brush = CreateSolidBrush(crate::constants::C_BG);
            FillRect(hdc, &rc, bg_brush);
            DeleteObject(bg_brush);

            if ref_data != 0 && w > 0 && h > 0 {
                let hbmp   = HBITMAP(ref_data as *mut _);

                // Query actual bitmap dimensions — AlphaBlend requires the
                // source rect to match the real bitmap size or it silently fails.
                let mut bm = BITMAP::default();
                GetObjectW(hbmp, std::mem::size_of::<BITMAP>() as i32,
                           Some(&mut bm as *mut _ as *mut _));
                let bw = bm.bmWidth.max(1);
                let bh = bm.bmHeight.abs().max(1);

                let sz     = w.min(h);          // dest size: keep square, centred
                let bx     = rc.left + (w - sz) / 2;
                let by_pos = rc.top  + (h - sz) / 2;

                let hdc_mem = CreateCompatibleDC(hdc);
                let old     = SelectObject(hdc_mem, hbmp);
                let bf = BLENDFUNCTION {
                    BlendOp:             0,  // AC_SRC_OVER
                    BlendFlags:          0,
                    SourceConstantAlpha: 200,
                    AlphaFormat:         1,  // AC_SRC_ALPHA
                };
                AlphaBlend(hdc, bx, by_pos, sz, sz, hdc_mem, 0, 0, bw, bh, bf);
                SelectObject(hdc_mem, old);
                DeleteDC(hdc_mem);
            }

            EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1), // prevent flicker
        WM_DESTROY => {
            RemoveWindowSubclass(hwnd, Some(bitmap_static_subclass_proc), 3);
            DefSubclassProc(hwnd, msg, wparam, lparam)
        }
        _ => DefSubclassProc(hwnd, msg, wparam, lparam),
    }
}

// ── Owner-drawn dark tab control (kept for reference; replaced by draw_nav_item) ─
#[allow(dead_code)]
pub unsafe fn draw_dark_tab(di: &DRAWITEMSTRUCT, accent: COLORREF,
                             fg: COLORREF, label: COLORREF, bg3: COLORREF) {
    let hdc = di.hDC;
    let rc  = di.rcItem;
    let is_sel = di.itemState.0 & ODS_SELECTED.0 != 0;

    // Selected: same bg as the main panel so the tab "opens into" it.
    // Unselected: slightly darker so they recede visually without disappearing.
    let bg_color = if is_sel { C_BG } else { COLORREF(0x00202020) };
    let br = CreateSolidBrush(bg_color);
    FillRect(hdc, &rc, br);
    DeleteObject(br);

    if is_sel {
        // Bold accent bar across the top edge of the selected tab.
        let bar_h = 2i32;
        let bar = RECT { left: rc.left + 1, top: rc.top, right: rc.right - 1, bottom: rc.top + bar_h };
        let bar_br = CreateSolidBrush(accent);
        FillRect(hdc, &bar, bar_br);
        DeleteObject(bar_br);
        // Bottom separator: erase the tab control's bottom border so it blends into the panel.
        let bot = RECT { left: rc.left, top: rc.bottom - 1, right: rc.right, bottom: rc.bottom };
        let bg_br = CreateSolidBrush(C_BG);
        FillRect(hdc, &bot, bg_br);
        DeleteObject(bg_br);
    } else {
        // Subtle bottom border line on unselected tabs to ground them.
        let pen = CreatePen(PS_SOLID, 1, COLORREF(0x00383838));
        let old = SelectObject(hdc, pen);
        MoveToEx(hdc, rc.left,      rc.bottom - 1, None);
        LineTo(hdc,   rc.right,     rc.bottom - 1);
        SelectObject(hdc, old);
        DeleteObject(pen);
    }

    SetBkMode(hdc, TRANSPARENT);
    // Selected tab gets full-brightness fg; unselected gets a readable mid-tone.
    SetTextColor(hdc, if is_sel { fg } else { label });

    const TCIF_TEXT_GET: u32 = 0x0001;
    let mut buf = [0u16; 64];
    #[repr(C)]
    struct TCITEMW_GET { mask: u32, dw_state: u32, dw_state_mask: u32,
        psz_text: *mut u16, cch_text_max: i32, i_image: i32, l_param: isize }
    let mut item = TCITEMW_GET {
        mask: TCIF_TEXT_GET, dw_state: 0, dw_state_mask: 0,
        psz_text: buf.as_mut_ptr(), cch_text_max: buf.len() as i32,
        i_image: -1, l_param: 0,
    };
    const TCM_GETITEMW: u32 = 0x133C;
    SendMessageW(di.hwndItem, TCM_GETITEMW,
        WPARAM(di.itemID as usize), LPARAM(&mut item as *mut _ as isize));

    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let mut rc_text = rc;
    DrawTextW(hdc, &mut buf[..len], &mut rc_text, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
}

// ── Left-panel vertical navigation item painter ───────────────────────────────
//
// `icon_hicon`  — if non-null, the app HICON is drawn scaled to 16×16 left of the label.
// `icon_glyph`  — if non-empty, a Unicode glyph string drawn left of the label instead.
// Only one of the two should be supplied; icon_hicon takes priority.

pub unsafe fn draw_nav_item(
    di: &DRAWITEMSTRUCT,
    is_active:   bool,
    is_focused:  bool,
    is_hovered:  bool,
    icon_hicon:  HICON,
    icon_glyph:  &str,
    icon_bitmap: Option<HBITMAP>,
    badge: bool,
) {
    let hdc = di.hDC;
    let rc  = di.rcItem;
    let is_pressed = di.itemState.0 & ODS_SELECTED.0 != 0;

    let row_h = rc.bottom - rc.top;
    // Use GetDpiForWindow rather than GetDeviceCaps(hdc, LOGPIXELSX): the HDC
    // passed to WM_DRAWITEM can still report the old DPI for a brief window
    // after WM_DPICHANGED, which would cause the icon to be drawn at the wrong
    // size and force AlphaBlend to stretch/shrink the already-correct bitmap.
    let dpi   = GetDpiForWindow(di.hwndItem) as i32;
    let dpi   = if dpi < 96 { 96 } else { dpi };
    let s     = |px: i32| (px * dpi / 96).max(1);

    // Background.
    let bg = if is_active {
        COLORREF(0x00252525)
    } else if is_pressed {
        COLORREF(0x00222222)
    } else if is_hovered {
        COLORREF(0x00222222)
    } else {
        C_BG
    };
    let br = CreateSolidBrush(bg);
    FillRect(hdc, &rc, br);
    DeleteObject(br);

    // Left accent bar for the active item.
    if is_active {
        let bar = RECT { left: rc.left, top: rc.top, right: rc.left + s(3), bottom: rc.bottom };
        let bar_br = CreateSolidBrush(C_ACCENT);
        FillRect(hdc, &bar, bar_br);
        DeleteObject(bar_br);
    }

    // Subtle bottom separator line.
    let sep_pen = CreatePen(PS_SOLID, 1, C_SEP);
    let old_pen = SelectObject(hdc, sep_pen);
    MoveToEx(hdc, rc.left, rc.bottom - 1, None);
    LineTo(hdc,   rc.right, rc.bottom - 1);
    SelectObject(hdc, old_pen);
    DeleteObject(sep_pen);

    SetBkMode(hdc, TRANSPARENT);
    let text_color = if is_active { C_FG } else { C_LABEL };

    // ── Icon area ─────────────────────────────────────────────────────────────
    let accent_gap = s(7);
    let icon_size  = s(16);
    let icon_gap   = s(6);
    let icon_x     = rc.left + s(3) + accent_gap;
    let icon_cy    = rc.top + (row_h - icon_size) / 2;

    if let Some(hbmp) = icon_bitmap {
        // Draw the PNG bitmap with per-pixel alpha via AlphaBlend.
        // Query actual bitmap dimensions — source rect must match the real
        // bitmap size or AlphaBlend clips/stretches incorrectly.
        let mut bm = BITMAP::default();
        GetObjectW(hbmp, std::mem::size_of::<BITMAP>() as i32,
                   Some(&mut bm as *mut _ as *mut _));
        let bw = bm.bmWidth.max(1);
        let bh = bm.bmHeight.abs().max(1);
        let hdc_mem = CreateCompatibleDC(hdc);
        let old_bmp = SelectObject(hdc_mem, hbmp);
        let bf = BLENDFUNCTION {
            BlendOp:             0,    // AC_SRC_OVER
            BlendFlags:          0,
            SourceConstantAlpha: 255,
            AlphaFormat:         1,    // AC_SRC_ALPHA (pre-multiplied)
        };
        AlphaBlend(
            hdc,
            icon_x, icon_cy, icon_size, icon_size,
            hdc_mem,
            0, 0, bw, bh,
            bf,
        );
        SelectObject(hdc_mem, old_bmp);
        DeleteDC(hdc_mem);
    } else if !icon_hicon.0.is_null() {
        DrawIconEx(
            hdc,
            icon_x, icon_cy,
            icon_hicon,
            icon_size, icon_size,
            0, HBRUSH(ptr::null_mut()),
            DI_NORMAL,
        );
    } else if !icon_glyph.is_empty() {
        // ── Option C: taskbar bar with 3 icon squares + dim overlay ──────────
        // Pure GDI — no font required, sharp at any DPI.
        //
        // Layout within icon_size × icon_size:
        //   bar sits at ~40% down, height ~40% of icon_size.
        //   3 small squares inside the bar.
        //   Dim overlay covers the bar interior at reduced opacity.

        let r = (text_color.0       & 0xFF) as u8;
        let g = ((text_color.0 >> 8)  & 0xFF) as u8;
        let b = ((text_color.0 >> 16) & 0xFF) as u8;

        // Blend colour toward C_BG (0x1A,0x1A,0x1A) at given alpha 0-255.
        let blend = |fr: u8, fg_: u8, fb: u8, alpha: u8| -> COLORREF {
            let a  = alpha as u32;
            let na = 255 - a;
            COLORREF(
                ((((fb as u32 * a + 0x1A * na) / 255)) << 16) |
                ((((fg_ as u32 * a + 0x1A * na) / 255)) << 8)  |
                 (((fr as u32 * a + 0x1A * na) / 255))
            )
        };

        let sz  = icon_size;
        let bx  = icon_x;
        let bh  = (sz * 2 / 5).max(4);
        let by  = icon_cy + (sz - bh) / 2;  // center bar vertically in icon cell

        // Bar outline.
        let border_pen = CreatePen(PS_SOLID, 1, blend(r, g, b, 160));
        let old_pen    = SelectObject(hdc, border_pen);
        let old_br     = SelectObject(hdc, GetStockObject(NULL_BRUSH));
        Rectangle(hdc, bx, by, bx + sz, by + bh);
        SelectObject(hdc, old_pen);
        SelectObject(hdc, old_br);
        DeleteObject(border_pen);

        // Dim overlay fill inside bar.
        let fill_br = CreateSolidBrush(blend(r, g, b, 55));
        let inner   = RECT { left: bx + 1, top: by + 1, right: bx + sz - 1, bottom: by + bh - 1 };
        FillRect(hdc, &inner, fill_br);
        DeleteObject(fill_br);

        // 3 small icon squares evenly spaced inside the bar.
        // sq is sized relative to bh so they fit comfortably; gap is computed
        // against the bar interior width (sz - 2) to prevent left-edge clipping.
        let sq   = ((bh - 4) * 2 / 3).max(2);
        let interior_w = sz - 2;
        let gap  = ((interior_w - 3 * sq) / 4).max(1);
        let sq_y = by + (bh - sq) / 2;
        let icon_br = CreateSolidBrush(blend(r, g, b, 180));
        for i in 0..3i32 {
            let sq_x  = bx + 1 + gap + i * (sq + gap);
            let sq_rc = RECT { left: sq_x, top: sq_y, right: sq_x + sq, bottom: sq_y + sq };
            FillRect(hdc, &sq_rc, icon_br);
        }
        DeleteObject(icon_br);

        // Bright top-edge accent line (the "dim glow").
        let accent_pen = CreatePen(PS_SOLID, 1, blend(r, g, b, 220));
        let old_pen2   = SelectObject(hdc, accent_pen);
        MoveToEx(hdc, bx + 1,     by, None);
        LineTo(hdc,   bx + sz - 1, by);
        SelectObject(hdc, old_pen2);
        DeleteObject(accent_pen);
    }

    // ── Label text ────────────────────────────────────────────────────────────
    SetTextColor(hdc, text_color);
    let text_x = icon_x + icon_size + icon_gap;
    let mut buf = [0u16; 128];
    let len = GetWindowTextW(di.hwndItem, &mut buf) as usize;

    // When a badge is shown, pre-measure its width and shrink the label rect
    // so the label never overlaps the badge.
    let badge_text: Vec<u16> = "Update".encode_utf16().collect();
    let badge_reserve = if badge {
        let mut rc_b = RECT { left: 0, top: 0, right: 500, bottom: 40 };
        let mut bw = badge_text.clone();
        DrawTextW(hdc, &mut bw, &mut rc_b,
            DT_LEFT | DT_SINGLELINE | DT_CALCRECT);
        rc_b.right + s(2)
    } else { 0 };

    let mut rc_text = RECT {
        left:   text_x,
        top:    rc.top,
        right:  rc.right - s(4) - badge_reserve,
        bottom: rc.bottom,
    };
    DrawTextW(hdc, &mut buf[..len], &mut rc_text,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);

    // ── Badge (accent colour) ─────────────────────────────────────────────
    if badge {
        let badge_x = rc.right - s(4) - badge_reserve;
        let mut rc_dot = RECT {
            left: badge_x, top: rc.top,
            right: rc.right - s(2), bottom: rc.bottom,
        };
        let mut bw = badge_text.clone();
        SetTextColor(hdc, C_ACCENT);
        DrawTextW(hdc, &mut bw, &mut rc_dot,
            DT_LEFT | DT_VCENTER | DT_SINGLELINE);
    }
}

/// Full dark-button / checkbox painter used by the main window's WM_DRAWITEM.
/// `AppState`-specific state (which handle is which, checkbox flags, toggle-active)
/// is passed as individual arguments so this module stays independent of app.rs.
pub unsafe fn draw_dark_button_full(
    di: &DRAWITEMSTRUCT,
    h_chk_auto_hz: HWND, h_chk_startup: HWND, h_chk_taskbar_dim: HWND,
    h_btn_toggle: HWND,
    chk_auto_hz_state: bool, chk_startup_state: bool, chk_taskbar_dim_enabled: bool,
    btn_toggle_active: bool,
    is_hovered: bool,
) {
    let hdc    = di.hDC;
    let rc     = di.rcItem;
    let is_sel = di.itemState.0 & ODS_SELECTED.0 != 0;
    let is_dis = di.itemState.0 & ODS_DISABLED.0 != 0;

    // ── Checkbox controls ────────────────────────────────────────────────────
    let is_checkbox = di.hwndItem == h_chk_auto_hz
                   || di.hwndItem == h_chk_startup
                   || di.hwndItem == h_chk_taskbar_dim;
    if is_checkbox {
        let bg_br = CreateSolidBrush(C_BG);
        FillRect(hdc, &rc, bg_br);
        DeleteObject(bg_br);

        SetBkMode(hdc, TRANSPARENT);

        let text_color = if is_dis { COLORREF(0x00555555) } else { C_FG };
        let box_border = if is_dis { COLORREF(0x00444444) } else { COLORREF(0x00777777) };

        let mut buf = [0u16; 256];
        let len = GetWindowTextW(di.hwndItem, &mut buf) as usize;

        let dpi    = GetDeviceCaps(hdc, LOGPIXELSX).max(96);
        let box_sz = (14 * dpi / 96).max(11);
        let box_x  = rc.left;
        let radius  = (3 * dpi / 96).max(2);

        let text_left = box_x + box_sz + (6 * dpi / 96).max(4);
        let mut rc_measure = RECT {
            left: text_left, top: rc.top, right: rc.right, bottom: rc.bottom,
        };
        DrawTextW(hdc, &mut buf[..len], &mut rc_measure,
            DT_LEFT | DT_WORDBREAK | DT_CALCRECT);
        let text_h  = rc_measure.bottom - rc_measure.top;
        let ctrl_h  = rc.bottom - rc.top;
        let block_top = rc.top + (ctrl_h - text_h).max(0) / 2;
        let box_y   = block_top + (text_h - box_sz).max(0) / 2;

        let checked = if di.hwndItem == h_chk_auto_hz { chk_auto_hz_state }
                      else if di.hwndItem == h_chk_startup { chk_startup_state }
                      else { chk_taskbar_dim_enabled };

        let accent = get_accent_color();

        // ── Shared pill helper (closure-like macro pattern) ───────────────────
        // Draws track + thumb at the given position using GDI+ for smooth edges.
        let is_hovered = !GetPropW(di.hwndItem, TOGGLE_HOVER_PROP).0.is_null();
        let draw_pill = |tr_x: i32, tr_y: i32, tr_h: i32, tr_w: i32| {
            let th_pad = (2 * dpi / 96).max(1);
            let th_d   = tr_h - th_pad * 2;
            let th_y   = tr_y + th_pad;

            let track_color = if is_dis { COLORREF(0x00444444) }
                              else if checked && is_hovered { lighten_colorref(accent, 40) }
                              else if checked { accent }
                              else if is_hovered { COLORREF(0x00707070) }
                              else { COLORREF(0x00555555) };
            let thumb_color = if is_dis { COLORREF(0x00888888) }
                              else if checked { C_BG } else { C_FG };

            gdip_init();
            let mut gp: *mut GpGraphics = ptr::null_mut();
            GdipCreateFromHDC(hdc, &mut gp);
            GdipSetSmoothingMode(gp, SmoothingModeAntiAlias);

            // Track: fully-rounded pill (radius = half height).
            fill_round_rect(gp, track_color, 0xFF, tr_x, tr_y, tr_w, tr_h, tr_h / 2);

            // Thumb: antialiased filled ellipse.
            let th_x = if checked { tr_x + tr_w - th_pad - th_d }
                       else       { tr_x + th_pad };
            let thumb_argb = colorref_to_argb(thumb_color, 0xFF);
            let mut thumb_br: *mut GpSolidFill = ptr::null_mut();
            GdipCreateSolidFill(thumb_argb, &mut thumb_br);
            GdipFillEllipseI(gp, thumb_br as _, th_x, th_y, th_d, th_d);
            GdipDeleteBrush(thumb_br as _);

            GdipDeleteGraphics(gp);
        };

        // ── h_chk_startup (sidebar): pill RIGHT-aligned, label to its left ───
        if di.hwndItem == h_chk_startup {
            let pad    = (2 * dpi / 96).max(2);
            let tr_h   = box_sz;
            let tr_w   = (box_sz * 2).max(22);
            let ctrl_h = rc.bottom - rc.top;
            let tr_x   = rc.right - tr_w - pad * 2;
            let tr_y   = rc.top + (ctrl_h - tr_h) / 2;
            draw_pill(tr_x, tr_y, tr_h, tr_w);
            SetTextColor(hdc, text_color);
            let mut rc_text = RECT {
                left: rc.left, top: rc.top,
                right: tr_x - (4 * dpi / 96).max(4), bottom: rc.bottom,
            };
            DrawTextW(hdc, &mut buf[..len], &mut rc_text,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
            return;
        }

        // ── h_chk_taskbar_dim (content): pill LEFT-aligned, label to its right
        if di.hwndItem == h_chk_taskbar_dim {
            let tr_h   = box_sz;
            let tr_w   = (box_sz * 2).max(22);
            let ctrl_h = rc.bottom - rc.top;
            let tr_x   = rc.left;
            let tr_y   = rc.top + (ctrl_h - tr_h) / 2;
            draw_pill(tr_x, tr_y, tr_h, tr_w);
            let gap = (6 * dpi / 96).max(4);
            SetTextColor(hdc, text_color);
            let mut rc_text = RECT {
                left: tr_x + tr_w + gap, top: rc.top,
                right: rc.right, bottom: rc.bottom,
            };
            DrawTextW(hdc, &mut buf[..len], &mut rc_text,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
            return;
        }

        // ── Standard checkbox (auto-Hz, startup) ─────────────────────────────
        if checked {
            gdip_init();
            let mut gp: *mut GpGraphics = ptr::null_mut();
            GdipCreateFromHDC(hdc, &mut gp);
            GdipSetSmoothingMode(gp, SmoothingModeAntiAlias);
            let fill_col = if is_dis { COLORREF(0x00444444) } else { accent };
            fill_round_rect(gp, fill_col, 0xFF, box_x, box_y, box_sz, box_sz, radius);
            GdipDeleteGraphics(gp);

            // Checkmark tick — GDI+ for antialiased stroke.
            let pen_w_f = (2 * dpi / 96).max(2) as f32;
            let mut ck_pen: *mut GpPen = ptr::null_mut();
            GdipCreatePen1(0xFFFFFFFF_u32, pen_w_f, UnitPixel, &mut ck_pen);
            let mut gp_ck: *mut GpGraphics = ptr::null_mut();
            GdipCreateFromHDC(hdc, &mut gp_ck);
            GdipSetSmoothingMode(gp_ck, SmoothingModeAntiAlias);
            let ck_pts: [PointF; 3] = [
                PointF { X: (box_x + box_sz * 15 / 100) as f32, Y: (box_y + box_sz * 50 / 100) as f32 },
                PointF { X: (box_x + box_sz * 38 / 100) as f32, Y: (box_y + box_sz * 72 / 100) as f32 },
                PointF { X: (box_x + box_sz * 85 / 100) as f32, Y: (box_y + box_sz * 22 / 100) as f32 },
            ];
            GdipDrawLines(gp_ck, ck_pen, ck_pts.as_ptr(), ck_pts.len() as i32);
            GdipDeleteGraphics(gp_ck);
            GdipDeletePen(ck_pen);
        } else {
            let border_color = if is_dis { COLORREF(0x00888888) } else { COLORREF(0x00AAAAAA) };
            let fill_color   = if is_dis { COLORREF(0x00555555) } else { COLORREF(0x00C8C8C8) };
            gdip_init();
            let mut gp: *mut GpGraphics = ptr::null_mut();
            GdipCreateFromHDC(hdc, &mut gp);
            GdipSetSmoothingMode(gp, SmoothingModeAntiAlias);
            fill_round_rect(gp, fill_color,   0xFF, box_x, box_y, box_sz, box_sz, radius);
            draw_round_rect(gp, border_color, 0xFF, 1.0,   box_x, box_y, box_sz, box_sz, radius);
            GdipDeleteGraphics(gp);
        }

        SetTextColor(hdc, text_color);
        let mut rc_text = RECT {
            left:   text_left,
            top:    block_top,
            right:  rc.right,
            bottom: rc.bottom,
        };
        DrawTextW(hdc, &mut buf[..len], &mut rc_text, DT_LEFT | DT_WORDBREAK);
        return;
    }

    // ── Regular push buttons ─────────────────────────────────────────────────
    let is_active = di.hwndItem == h_btn_toggle && btn_toggle_active;

    // Read hover state from the window property set by action_btn_subclass_proc.
    // The caller may also pass `is_hovered` directly (e.g. when it already has
    // the flag from AppState), so OR both sources together.
    let btn_hovered = is_hovered
        || !GetPropW(di.hwndItem, ACTION_BTN_HOVER_PROP).0.is_null();

    let (bg, border, text_color) = if is_dis {
        (COLORREF(0x00222222), COLORREF(0x00333333), COLORREF(0x00555555))
    } else if is_active {
        let bg = if is_sel {
            C_BTN_ACTIVE_PRESS
        } else if btn_hovered {
            lighten_colorref(C_BTN_ACTIVE, 20)
        } else {
            C_BTN_ACTIVE
        };
        (bg, C_BTN_ACTIVE_BORDER, C_BTN_ACTIVE_TEXT)
    } else if is_sel {
        (C_BTN_PRESS, C_BTN_BORDER, C_BTN_TEXT)
    } else if btn_hovered {
        // Subtle hover: slightly lighter background and brighter border.
        (lighten_colorref(C_BTN_NORMAL, 18), lighten_colorref(C_BTN_BORDER, 40), C_BTN_TEXT)
    } else {
        (C_BTN_NORMAL, C_BTN_BORDER, C_BTN_TEXT)
    };

    let br = CreateSolidBrush(bg);
    FillRect(hdc, &rc, br);
    DeleteObject(br);

    let pen = CreatePen(PS_SOLID, 1, border);
    SelectObject(hdc, pen);
    let old_br = SelectObject(hdc, GetStockObject(NULL_BRUSH));
    Rectangle(hdc, rc.left, rc.top, rc.right - 1, rc.bottom - 1);
    SelectObject(hdc, old_br);
    DeleteObject(pen);

    SetTextColor(hdc, text_color);
    SetBkMode(hdc, TRANSPARENT);

    let mut buf = [0u16; 256];
    let len = GetWindowTextW(di.hwndItem, &mut buf) as usize;
    let mut rc_text = rc;
    DrawTextW(hdc, &mut buf[..len], &mut rc_text,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE);
}

// ── HDR toggle switch painter ─────────────────────────────────────────────────
//
// Draws the "Enable HDR" row: label on the left, pill toggle on the right.
// Identical geometry to the "Launch with Windows" pill so both rows match.
// Moved here from app.rs so all toggle painting lives in one place and shares
// the GDI+ helpers for antialiased edges.

pub unsafe fn draw_hdr_toggle_switch(
    di:      &DRAWITEMSTRUCT,
    checked: bool,
    _hdr_icon: Option<HBITMAP>,
) {
    let hdc = di.hDC;
    let rc  = di.rcItem;
    let h   = rc.bottom - rc.top;

    // Background.
    let bg_br = CreateSolidBrush(C_BG);
    FillRect(hdc, &rc, bg_br);
    DeleteObject(bg_br);
    SetBkMode(hdc, TRANSPARENT);

    let dpi    = GetDeviceCaps(hdc, LOGPIXELSX).max(96);
    let box_sz = (14 * dpi / 96).max(11);
    let pad    = (2 * dpi / 96).max(2);
    let tr_h   = box_sz;
    let tr_w   = (box_sz * 2).max(22);
    let tr_x   = rc.right - tr_w - pad * 2;
    let tr_y   = rc.top + (h - tr_h) / 2;

    // Label — read from the window text so any button using this painter
    // shows its own label rather than a hardcoded string.
    SetTextColor(hdc, C_FG);
    let mut lbl_buf = [0u16; 128];
    let lbl_len = GetWindowTextW(di.hwndItem, &mut lbl_buf) as usize;
    let mut lbl_rc = RECT {
        left:   rc.left,
        top:    rc.top,
        right:  tr_x - (4 * dpi / 96).max(4),
        bottom: rc.bottom,
    };
    DrawTextW(hdc, &mut lbl_buf[..lbl_len], &mut lbl_rc,
        DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOPREFIX);

    // Pill track + thumb — GDI+ for antialiased edges, matching draw_pill.
    let accent      = get_accent_color();
    let is_hovered  = !GetPropW(di.hwndItem, TOGGLE_HOVER_PROP).0.is_null();
    let track_color = if checked && is_hovered { lighten_colorref(accent, 40) }
                      else if checked { accent }
                      else if is_hovered { COLORREF(0x00707070) }
                      else { COLORREF(0x00555555) };
    let thumb_color = if checked { C_BG } else { C_FG };

    gdip_init();
    let mut gp: *mut GpGraphics = ptr::null_mut();
    GdipCreateFromHDC(hdc, &mut gp);
    GdipSetSmoothingMode(gp, SmoothingModeAntiAlias);

    fill_round_rect(gp, track_color, 0xFF, tr_x, tr_y, tr_w, tr_h, tr_h / 2);

    let th_pad = (2 * dpi / 96).max(1);
    let th_d   = tr_h - th_pad * 2;
    let th_y   = tr_y + th_pad;
    let th_x   = if checked { tr_x + tr_w - th_pad - th_d } else { tr_x + th_pad };
    let thumb_argb = colorref_to_argb(thumb_color, 0xFF);
    let mut thumb_br: *mut GpSolidFill = ptr::null_mut();
    GdipCreateSolidFill(thumb_argb, &mut thumb_br);
    GdipFillEllipseI(gp, thumb_br as _, th_x, th_y, th_d, th_d);
    GdipDeleteBrush(thumb_br as _);

    GdipDeleteGraphics(gp);
}

// ── Tab header painter (icon + title text) ────────────────────────────────────
//
// Call `subclass_tab_header(hwnd, hbitmap)` once after creating a title label.
// The subclass proc intercepts WM_PAINT and draws the PNG icon to the left of
// the title text, scaled to ~32 logical px (2× the nav icon size).

const TAB_HDR_ORIG_PROC:    PCWSTR = w!("BCT_TabHdrOrigProc");
pub const TAB_HDR_BITMAP:   PCWSTR = w!("BCT_TabHdrBitmap");

/// Attach the tab-header painter to a static title label.
/// `hbitmap` may be null — in that case only the text is drawn (same as before).
pub unsafe fn subclass_tab_header(hwnd: HWND, hbitmap: Option<HBITMAP>) {
    let bmp_raw = hbitmap.map(|b| b.0 as isize).unwrap_or(0);
    SetPropW(hwnd, TAB_HDR_BITMAP, HANDLE(bmp_raw as *mut _));
    SetWindowSubclass(hwnd, Some(tab_hdr_subclass_proc), 1, 0);
}

unsafe extern "system" fn tab_hdr_subclass_proc(
    hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM,
    _subclass_id: usize, _ref_data: usize,
) -> LRESULT {
    let call_orig = || DefSubclassProc(hwnd, msg, wp, lp);

    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            if !hdc.0.is_null() {
                paint_tab_header(hwnd, hdc);
                EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_NCDESTROY => {
            RemovePropW(hwnd, TAB_HDR_ORIG_PROC);
            RemovePropW(hwnd, TAB_HDR_BITMAP);
            RemoveWindowSubclass(hwnd, Some(tab_hdr_subclass_proc), 1);
            call_orig()
        }
        _ => call_orig(),
    }
}

unsafe fn paint_tab_header(hwnd: HWND, hdc: HDC) {
    let mut rc = RECT::default();
    GetClientRect(hwnd, &mut rc);

    // Clear background to match the parent dark theme.
    let bg_br = CreateSolidBrush(C_BG);
    FillRect(hdc, &rc, bg_br);
    DeleteObject(bg_br);

    SetBkMode(hdc, TRANSPARENT);
    SetTextColor(hdc, C_FG);

    let dpi    = GetDpiForWindow(hwnd) as i32;
    let dpi    = if dpi < 96 { 96 } else { dpi };
    let s      = |px: i32| (px * dpi / 96).max(1);
    // Icon drawn at 20 logical px — matches the baked TabHeaderIcons bitmap size.
    let icon_sz = s(20);
    let gap     = s(10);

    let bmp_raw = GetPropW(hwnd, TAB_HDR_BITMAP).0 as isize;
    let text_x = if bmp_raw != 0 {
        let hbmp   = HBITMAP(bmp_raw as *mut _);
        let hdc_mem = CreateCompatibleDC(hdc);
        let old     = SelectObject(hdc_mem, hbmp);

        // Vertically centre the icon within the control height.
        let ctrl_h = rc.bottom - rc.top;
        let icon_y = rc.top + (ctrl_h - icon_sz) / 2;

        let bf = BLENDFUNCTION {
            BlendOp:             0,    // AC_SRC_OVER
            BlendFlags:          0,
            SourceConstantAlpha: 255,
            AlphaFormat:         1,    // AC_SRC_ALPHA
        };
        let mut bm = BITMAP::default();
        GetObjectW(hbmp, std::mem::size_of::<BITMAP>() as i32,
                   Some(&mut bm as *mut _ as *mut _));
        let bw = bm.bmWidth.max(1);
        let bh = bm.bmHeight.abs().max(1);
        AlphaBlend(
            hdc,
            rc.left, icon_y, icon_sz, icon_sz,
            hdc_mem,
            0, 0, bw, bh,
            bf,
        );
        SelectObject(hdc_mem, old);
        DeleteDC(hdc_mem);
        rc.left + icon_sz + gap
    } else {
        rc.left
    };

    // Draw the window text (title) to the right of the icon.
    // Select a bold font sized to ~13pt so the title fills the 36px control height.
    let font = make_font(w!("Segoe UI"), 13, dpi as u32, true);
    let old_font = SelectObject(hdc, font);
    let mut buf = [0u16; 128];
    let len = GetWindowTextW(hwnd, &mut buf) as usize;
    let mut rc_text = RECT { left: text_x, ..rc };
    DrawTextW(hdc, &mut buf[..len], &mut rc_text,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
    SelectObject(hdc, old_font);
    DeleteObject(font);
}
// ── Toggle pill subclass (shared by HDR and Dimmer toggle buttons) ────────────
//
// ref_data == 0 → pill is RIGHT-aligned (HDR toggle, matches draw_hdr_toggle_switch)
// ref_data == 1 → pill is LEFT-aligned  (Dimmer toggle)
//
// The pill geometry mirrors draw_hdr_toggle_switch / draw_pill exactly:
//   box_sz = (14*dpi/96).max(11)   tr_w = (box_sz*2).max(22)
//   HDR:    pill_x = w - tr_w - pad*2
//   Dimmer: pill_x = 0

/// Window property used by toggle-pill subclass procs to record hover state.
/// Non-null = hovered; null = not hovered.
const TOGGLE_HOVER_PROP: PCWSTR = w!("BCT_ToggleHover");

unsafe fn toggle_pill_hit(hwnd: HWND, lp: LPARAM, right_aligned: bool) -> bool {
    let mut rc = RECT::default();
    GetClientRect(hwnd, &mut rc);
    let w = rc.right  - rc.left;
    let h = rc.bottom - rc.top;
    let dpi    = GetDpiForWindow(hwnd).max(96) as i32;
    let box_sz = (14 * dpi / 96).max(11);
    let pad    = (2  * dpi / 96).max(2);
    let tr_h   = box_sz;
    let tr_w   = (box_sz * 2).max(22);
    let pill_x = if right_aligned { w - tr_w - pad * 2 } else { 0 };
    let pill_y = (h - tr_h) / 2;
    let screen_pt = POINT { x: (lp.0 & 0xFFFF) as i16 as i32,
                             y: (lp.0 >> 16)    as i16 as i32 };
    let mut pt = screen_pt;
    ScreenToClient(hwnd, &mut pt);
    pt.x >= pill_x && pt.x < pill_x + tr_w && pt.y >= pill_y && pt.y < pill_y + tr_h
}

pub unsafe extern "system" fn hdr_toggle_subclass_proc(
    hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM,
    _subclass_id: usize, ref_data: usize,
) -> LRESULT {
    let right_aligned = ref_data == 0;
    match msg {
        WM_MOUSEMOVE => {
            begin_mouse_track(hwnd, TOGGLE_HOVER_PROP);
            DefSubclassProc(hwnd, msg, wp, lp)
        }
        WM_MOUSELEAVE => {
            RemovePropW(hwnd, TOGGLE_HOVER_PROP);
            InvalidateRect(hwnd, None, false);
            DefSubclassProc(hwnd, msg, wp, lp)
        }
        WM_NCHITTEST => {
            if toggle_pill_hit(hwnd, lp, right_aligned) { LRESULT(HTCLIENT as isize) }
            else { LRESULT(HTTRANSPARENT as isize) }
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        _ => DefSubclassProc(hwnd, msg, wp, lp),
    }
}

/// Alias used by the dimmer toggle — same proc, ref_data = 1 (left-aligned pill).
pub unsafe extern "system" fn dimmer_toggle_subclass_proc(
    hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM,
    subclass_id: usize, _ref_data: usize,
) -> LRESULT {
    hdr_toggle_subclass_proc(hwnd, msg, wp, lp, subclass_id, 1)
}