// nav_icons.rs — compile-time PNG embedding for nav and tab header icons.
//
// Drop PNGs into assets/icons/ next to Cargo.toml.
// Set any include_bytes!(...) to b"" to fall back to the built-in glyph.
// Call NavIcons::load(dpi) once at startup; pass each HBITMAP to draw_nav_item.

#![allow(non_snake_case)]

use std::{mem, ptr};

use windows::Win32::{
    Foundation::HWND,
    Graphics::Gdi::*,
};

// ── Embedded PNG bytes ────────────────────────────────────────────────────────

const BYTES_CRUSH:    &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/tab_crush.png"));
const BYTES_DIMMER:   &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/tab_dimmer.png"));
const BYTES_SYSTEM:   &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/tab_system.png"));
const BYTES_HOTKEYS:  &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/tab_hotkeys.png"));
const BYTES_DEBUG:    &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/tab_debug.png"));
const BYTES_ABOUT:    &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/tab_about.png"));
const BYTES_ZOOM:     &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/zoom.png"));
const BYTES_ZOOM_OUT: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/zoom_out.png"));

// ── NavIcons — sidebar nav buttons ───────────────────────────────────────────

pub struct NavIcons {
    pub crush:    Option<HBITMAP>,
    pub dimmer:   Option<HBITMAP>,
    pub system:   Option<HBITMAP>,
    pub hotkeys:  Option<HBITMAP>,
    pub debug:    Option<HBITMAP>,
    pub about:    Option<HBITMAP>,
    pub zoom:     Option<HBITMAP>,
    pub zoom_out: Option<HBITMAP>,
}

impl NavIcons {
    /// Decode all embedded PNGs scaled to the DPI-correct icon size.
    /// Call once after the main window is created.
    pub unsafe fn load(dpi: u32) -> Self {
        let icon_px = (16 * dpi / 96).max(16);
        // Zoom icons live in an 18×20 logical-px cell; draw at 18 so
        // AlphaBlend never has to stretch them.
        let zoom_px = (18 * dpi / 96).max(18);
        Self {
            crush:    decode_png(BYTES_CRUSH,    icon_px),
            dimmer:   decode_png(BYTES_DIMMER,   icon_px),
            system:   decode_png(BYTES_SYSTEM,   icon_px),
            hotkeys:  decode_png(BYTES_HOTKEYS,  icon_px),
            debug:    decode_png(BYTES_DEBUG,    icon_px),
            about:    decode_png(BYTES_ABOUT,    icon_px),
            zoom:     decode_png(BYTES_ZOOM,     zoom_px),
            zoom_out: decode_png(BYTES_ZOOM_OUT, zoom_px),
        }
    }

    /// Delete all GDI bitmaps. Call on WM_DESTROY.
    pub unsafe fn destroy(&self) {
        for bmp in [self.crush, self.dimmer, self.system, self.hotkeys,
                    self.debug, self.about, self.zoom, self.zoom_out]
            .iter().flatten()
        {
            let _ = DeleteObject(*bmp);
        }
    }
}

// ── TabHeaderIcons — 20 px icons beside each tab title ───────────────────────

pub struct TabHeaderIcons {
    pub crush:   Option<HBITMAP>,
    pub dimmer:  Option<HBITMAP>,
    pub system:  Option<HBITMAP>,
    pub hotkeys: Option<HBITMAP>,
    pub debug:   Option<HBITMAP>,
    pub about:   Option<HBITMAP>,
}

impl TabHeaderIcons {
    pub unsafe fn load(dpi: u32) -> Self {
        let icon_px = (20 * dpi / 96).max(20);
        Self {
            crush:   decode_png(BYTES_CRUSH,   icon_px),
            dimmer:  decode_png(BYTES_DIMMER,  icon_px),
            system:  decode_png(BYTES_SYSTEM,  icon_px),
            hotkeys: decode_png(BYTES_HOTKEYS, icon_px),
            debug:   decode_png(BYTES_DEBUG,   icon_px),
            about:   decode_png(BYTES_ABOUT,   icon_px),
        }
    }

    pub unsafe fn destroy(&self) {
        for bmp in [self.crush, self.dimmer, self.system,
                    self.hotkeys, self.debug, self.about]
            .iter().flatten()
        {
            let _ = DeleteObject(*bmp);
        }
    }
}

// ── PNG → pre-multiplied HBITMAP ─────────────────────────────────────────────

/// Decode `bytes`, Lanczos-3 downscale to `size×size`, return a 32-bpp
/// pre-multiplied-alpha HBITMAP ready for `AlphaBlend`.
/// Returns `None` if `bytes` is empty or decoding fails.
unsafe fn decode_png(bytes: &[u8], size: u32) -> Option<HBITMAP> {
    if bytes.is_empty() { return None; }

    let cursor  = std::io::Cursor::new(bytes);
    let decoder = png::Decoder::new(cursor);
    let mut reader = decoder.read_info().ok()?;
    let mut raw = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut raw).ok()?;

    let sw = info.width  as usize;
    let sh = info.height as usize;

    // Normalise to RGBA8.
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => raw[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb  => raw[..info.buffer_size()]
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255u8])
            .collect(),
        png::ColorType::GrayscaleAlpha => raw[..info.buffer_size()]
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => raw[..info.buffer_size()]
            .iter()
            .flat_map(|&v| [v, v, v, 255u8])
            .collect(),
        _ => return None,
    };

    // Lanczos-3 downscale — separable horizontal then vertical pass.
    fn sinc(x: f32) -> f32 {
        if x.abs() < 1e-6 { return 1.0; }
        let px = std::f32::consts::PI * x;
        px.sin() / px
    }
    fn lanczos3(x: f32) -> f32 {
        let ax = x.abs();
        if ax >= 3.0 { 0.0 } else { sinc(ax) * sinc(ax / 3.0) }
    }

    let dw = size as usize;
    let dh = size as usize;

    // Horizontal pass: rgba (sw×sh) → tmp (dw×sh).
    let scale_x = dw as f32 / sw as f32;
    let filter_r_x = (3.0 / scale_x).ceil() as isize;
    let mut tmp = vec![0f32; dw * sh * 4];
    for iy in 0..sh {
        for dx in 0..dw {
            let cx = (dx as f32 + 0.5) / scale_x - 0.5;
            let ix0 = (cx - filter_r_x as f32).ceil() as isize;
            let ix1 = (cx + filter_r_x as f32).floor() as isize;
            let mut acc = [0f32; 4];
            let mut wsum = 0f32;
            for ix in ix0..=ix1 {
                let sx = ix.clamp(0, sw as isize - 1) as usize;
                let w  = lanczos3((ix as f32 - cx) * scale_x);
                let src = (iy * sw + sx) * 4;
                for c in 0..4 { acc[c] += rgba[src + c] as f32 * w; }
                wsum += w;
            }
            let dst = (iy * dw + dx) * 4;
            if wsum.abs() > 1e-6 {
                for c in 0..4 { tmp[dst + c] = acc[c] / wsum; }
            }
        }
    }

    // Vertical pass: tmp (dw×sh) → scaled (dw×dh).
    let scale_y = dh as f32 / sh as f32;
    let filter_r_y = (3.0 / scale_y).ceil() as isize;
    let mut scaled_f = vec![0f32; dw * dh * 4];
    for dy in 0..dh {
        let cy = (dy as f32 + 0.5) / scale_y - 0.5;
        let iy0 = (cy - filter_r_y as f32).ceil() as isize;
        let iy1 = (cy + filter_r_y as f32).floor() as isize;
        for dx in 0..dw {
            let mut acc = [0f32; 4];
            let mut wsum = 0f32;
            for iy in iy0..=iy1 {
                let sy = iy.clamp(0, sh as isize - 1) as usize;
                let w  = lanczos3((iy as f32 - cy) * scale_y);
                let src = (sy * dw + dx) * 4;
                for c in 0..4 { acc[c] += tmp[src + c] * w; }
                wsum += w;
            }
            let dst = (dy * dw + dx) * 4;
            if wsum.abs() > 1e-6 {
                for c in 0..4 { scaled_f[dst + c] = acc[c] / wsum; }
            }
        }
    }

    let scaled: Vec<u8> = scaled_f.iter().map(|&v| v.round().clamp(0.0, 255.0) as u8).collect();

    // Pre-multiply alpha and convert RGBA → BGRA (GDI / AlphaBlend format).
    let mut bgra = vec![0u8; dw * dh * 4];
    for i in 0..dw * dh {
        let r = scaled[i * 4    ] as u32;
        let g = scaled[i * 4 + 1] as u32;
        let b = scaled[i * 4 + 2] as u32;
        let a = scaled[i * 4 + 3] as u32;
        bgra[i * 4    ] = (b * a / 255) as u8;
        bgra[i * 4 + 1] = (g * a / 255) as u8;
        bgra[i * 4 + 2] = (r * a / 255) as u8;
        bgra[i * 4 + 3] = a as u8;
    }

    // Build a top-down 32-bpp DIB and copy pixels in.
    let hdc_screen = GetDC(HWND(ptr::null_mut()));
    let hdc_mem    = CreateCompatibleDC(hdc_screen);
    ReleaseDC(HWND(ptr::null_mut()), hdc_screen);

    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize:        mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth:       dw as i32,
            biHeight:      -(dh as i32), // negative = top-down rows
            biPlanes:      1,
            biBitCount:    32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };

    let mut bits: *mut std::ffi::c_void = ptr::null_mut();
    let hbmp = CreateDIBSection(hdc_mem, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
    ptr::copy_nonoverlapping(bgra.as_ptr(), bits as *mut u8, bgra.len());
    let _ = DeleteDC(hdc_mem);

    Some(hbmp)
}