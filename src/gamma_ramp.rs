// gamma_ramp.rs — 256-entry 16-bit gamma ramps using a Reinhard-based shadow-recovery curve.
// Pure black (index 0) is always left untouched.

use windows::Win32::Graphics::Gdi::{GetDC, HDC, ReleaseDC};
use windows::Win32::Foundation::HWND;
use std::ptr;

// ── Types ─────────────────────────────────────────────────────────────────────

/// 256-entry 16-bit RGB gamma ramp matching the GDI RAMP layout.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct GammaRamp {
    pub red:   [u16; 256],
    pub green: [u16; 256],
    pub blue:  [u16; 256],
}

impl GammaRamp {
    fn zeroed() -> Self {
        Self { red: [0u16; 256], green: [0u16; 256], blue: [0u16; 256] }
    }
}

// Manual extern block — windows-rs exposes SetDeviceGammaRamp as *mut c_void,
// so we redeclare it with our typed struct.
#[link(name = "gdi32")]
extern "system" {
    pub fn SetDeviceGammaRamp(hdc: HDC, lpramp: *const GammaRamp) -> i32;
    pub fn GetDeviceGammaRamp(hdc: HDC, lpramp: *mut GammaRamp) -> i32;
}

// ── Ramp builders ─────────────────────────────────────────────────────────────

/// Lifted or crushed ramp using the same Reinhard curve in both directions.
/// - Positive `black_lift` (1–15): raises near-black tones above linear.
/// - Zero: linear passthrough.
/// - Negative `black_lift` (-1 to -15): pulls near-black tones the same distance below linear.
/// Pure black (index 0) is always left untouched.
pub fn build_ramp(black_lift: i32) -> GammaRamp {
    let mut ramp = GammaRamp::zeroed();
    for i in 0usize..256 {
        let out: f64 = if i == 0 {
            0.0
        } else {
            let t      = i as f64 / 255.0;
            let abs_bl = black_lift.unsigned_abs() as i32;
            let blend  = (1.0 - t) / (1.0 + t * 2.0);
            let floor  = (abs_bl as f64 / 255.0) * blend;
            let lifted = floor + t * (1.0 - floor);
            if black_lift >= 0 {
                lifted.clamp(0.0, 1.0)
            } else {
                // Mirror: reflect the lift delta below linear
                (2.0 * t - lifted).clamp(0.0, 1.0)
            }
        };
        let v = (out * 65535.0).round() as u16;
        ramp.red[i] = v; ramp.green[i] = v; ramp.blue[i] = v;
    }
    ramp
}

/// Linear ramp — no correction.
pub fn build_linear_ramp() -> GammaRamp {
    let mut ramp = GammaRamp::zeroed();
    for i in 0usize..256 {
        let v = (i as f64 / 255.0 * 65535.0).round() as u16;
        ramp.red[i] = v; ramp.green[i] = v; ramp.blue[i] = v;
    }
    ramp
}

// ── Display I/O ───────────────────────────────────────────────────────────────

/// Reads the current system gamma ramp from the primary display.
pub unsafe fn get_display_ramp() -> Option<GammaRamp> {
    let null_hwnd = HWND(ptr::null_mut());
    let hdc = GetDC(null_hwnd);
    if hdc.0.is_null() { return None; }
    let mut ramp = GammaRamp::zeroed();
    let ok = GetDeviceGammaRamp(hdc, &mut ramp);
    ReleaseDC(null_hwnd, hdc);
    if ok != 0 { Some(ramp) } else { None }
}

/// Applies a linear ramp to the primary display — used on exit/crash.
pub unsafe fn reset_display_ramp() {
    let ramp = build_linear_ramp();
    let null_hwnd = HWND(ptr::null_mut());
    let hdc = GetDC(null_hwnd);
    debug_assert!(!hdc.0.is_null(), "GetDC(null) failed — gamma ramp not reset");
    if !hdc.0.is_null() {
        SetDeviceGammaRamp(hdc, &ramp);
        ReleaseDC(null_hwnd, hdc);
    }
}