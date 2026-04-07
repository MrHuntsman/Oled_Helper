// gamma_ramp.rs
//
// Builds 256-entry 16-bit gamma ramps using a Reinhard-based shadow-recovery curve.
// Pure black (index 0) is always left untouched.

use windows::Win32::Graphics::Gdi::{GetDC, HDC, ReleaseDC};
use windows::Win32::Foundation::HWND;
use std::ptr;

/// 256-entry 16-bit RGB gamma ramp, matching the GDI RAMP layout.
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

// Declare SetDeviceGammaRamp directly — windows-rs exposes it as taking *mut c_void
// which requires a manual extern block to use our typed GammaRamp struct.
#[link(name = "gdi32")]
extern "system" {
    pub fn SetDeviceGammaRamp(hdc: HDC, lpramp: *const GammaRamp) -> i32;
    pub fn GetDeviceGammaRamp(hdc: HDC, lpramp: *mut GammaRamp) -> i32;
}

/// Reads the current system gamma ramp for the primary display.
pub unsafe fn get_display_ramp() -> Option<GammaRamp> {
    let ramp = GammaRamp::zeroed();
    let null_hwnd = HWND(ptr::null_mut());
    let hdc = GetDC(null_hwnd);
    if hdc.0.is_null() {
        return None;
    }
    let mut ramp = ramp;
    let ok = GetDeviceGammaRamp(hdc, &mut ramp);
    ReleaseDC(null_hwnd, hdc);
    if ok != 0 {
        Some(ramp)
    } else {
        None
    }
}

/// Returns a corrected ramp that lifts near-black tones by `black_lift` (0–20).
pub fn build_ramp(black_lift: i32) -> GammaRamp {
    let mut ramp = GammaRamp::zeroed();
    for i in 0usize..256 {
        let lifted: f64 = if i > 0 {
            let t     = i as f64 / 255.0;
            let blend = (1.0 - t) / (1.0 + t * 2.0);
            let floor = (black_lift as f64 / 255.0) * blend;
            (floor + t * (1.0 - floor)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let v = (lifted * 65535.0).round() as u16;
        ramp.red[i] = v; ramp.green[i] = v; ramp.blue[i] = v;
    }
    ramp
}

/// Neutral linear ramp — no correction applied.
pub fn build_linear_ramp() -> GammaRamp {
    let mut ramp = GammaRamp::zeroed();
    for i in 0usize..256 {
        let v = (i as f64 / 255.0 * 65535.0).round() as u16;
        ramp.red[i] = v; ramp.green[i] = v; ramp.blue[i] = v;
    }
    ramp
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
