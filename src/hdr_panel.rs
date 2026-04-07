// hdr_panel.rs
//
// D3D11 calibration panel — fixed for windows-rs 0.58 API:
//   • All Create* methods use output-param pattern: device.CreateFoo(&desc, init, Some(&mut out))?
//   • D3DResources.rtv is Option<> (reset during resize)
//   • ResizeBuffers takes typed DXGI_SWAP_CHAIN_FLAG
//   • Map takes output *mut D3D11_MAPPED_SUBRESOURCE
//   • ClearRenderTargetView takes &[f32; 4]
//   • Present takes u32 flags directly

#![allow(clippy::too_many_arguments, unused_must_use)]

use std::{mem, ptr};

use windows::{
    core::*,
    Win32::{
        Foundation::*,
        Graphics::{
            Direct3D::*,
            Direct3D11::*,
            Direct3D::Fxc::*,
            Dxgi::*,
            Dxgi::Common::*,
            Gdi::*,
        },
        UI::WindowsAndMessaging::*,
    },
};

// ── Square definitions ────────────────────────────────────────────────────────

const MAX_SQUARES: usize = 24;
const GAP_FRACTION: f32  = 0.008;

static ALL_PQ_CODES: [i32; MAX_SQUARES] = [
     64,  68,  72,  76,  80,  84,  88,  92,
     96, 100, 104, 108, 112, 116, 120, 124,
    128, 132, 136, 140, 144, 148, 152, 156,
];
static ALL_SDR_CODES: [i32; MAX_SQUARES] = [
    0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,
];

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055_f32).powf(2.4) }
}

fn pq_code_to_scrgb(code: i32) -> f32 {
    if code <= 64 { return 0.0; }
    const M1: f64 = 0.1593017578125;
    const M2: f64 = 78.84375;
    const C1: f64 = 0.8359375;
    const C2: f64 = 18.8515625;
    const C3: f64 = 18.6875;
    let v   = ((code as f64 - 64.0) / (940.0 - 64.0)).clamp(0.0, 1.0);
    let vm2 = v.powf(1.0 / M2);
    let num = (vm2 - C1).max(0.0);
    let den = C2 - C3 * vm2;
    (10000.0 * (num / den).powf(1.0 / M1) / 80.0) as f32
}

// ── Constant buffer (112 bytes = 7 × float4) ─────────────────────────────────

#[repr(C)]
struct CbData {
    sq:       [f32; 24],
    sq_count: i32,
    sq_cols:  i32,
    sq_rows:  i32,
    gap_frac: f32,
}

// ── HLSL ─────────────────────────────────────────────────────────────────────

const HLSL: &str = r#"
struct VS_OUT { float4 pos : SV_Position; float2 uv : UV; };
VS_OUT VS(uint id : SV_VertexID)
{
    float2 pos[3] = { float2(-1,1), float2(3,1), float2(-1,-3) };
    VS_OUT o; o.pos = float4(pos[id],0,1);
    o.uv = pos[id] * float2(0.5,-0.5) + 0.5; return o;
}
cbuffer CB : register(b0) {
    float4 sqVals0,sqVals1,sqVals2,sqVals3,sqVals4,sqVals5;
    int sqCount,sqCols,sqRows; float gapFrac;
}
Texture2D digitTex : register(t0);
SamplerState samp  : register(s0);
float GetVal(int i) {
    if(i<4) return sqVals0[i]; if(i<8) return sqVals1[i-4];
    if(i<12)return sqVals2[i-8];if(i<16)return sqVals3[i-12];
    if(i<20)return sqVals4[i-16]; return sqVals5[i-20];
}
float4 PS(VS_OUT i) : SV_Target {
    float sqW=(1.0-gapFrac*(sqCols-1))/(float)sqCols;
    float sqH=(1.0-gapFrac*(sqRows-1))/(float)sqRows;
    float cellW=sqW+gapFrac; float cellH=sqH+gapFrac;
    int col=(int)floor(i.uv.x/cellW); int row=(int)floor(i.uv.y/cellH);
    float xF=frac(i.uv.x/cellW)*cellW; float yF=frac(i.uv.y/cellH)*cellH;
    int idx=row*sqCols+col;
    if(xF>sqW||yF>sqH||col>=sqCols||row>=sqRows||idx>=sqCount)return float4(0,0,0,1);
    float sv=GetVal(idx);
    float2 duv; duv.x=(idx+(xF/sqW))/(float)sqCount; duv.y=yF/sqH;
    float4 t=digitTex.Sample(samp,duv);
    float v=lerp(sv,t.r,t.a); return float4(v,v,v,1);
}
"#;

// ── D3D resources ─────────────────────────────────────────────────────────────

struct D3DResources {
    device:    ID3D11Device,
    ctx:       ID3D11DeviceContext,
    swap3:     IDXGISwapChain3,
    rtv:       Option<ID3D11RenderTargetView>,
    vs:        ID3D11VertexShader,
    ps:        ID3D11PixelShader,
    cb:        ID3D11Buffer,
    sampler:   ID3D11SamplerState,
    digit_tex: Option<ID3D11Texture2D>,
    digit_srv: Option<ID3D11ShaderResourceView>,
}

impl D3DResources {
    unsafe fn create_rtv(&mut self) -> Result<()> {
        let bb: ID3D11Texture2D = self.swap3.GetBuffer(0)?;
        let mut rtv_out: Option<ID3D11RenderTargetView> = None;
        self.device.CreateRenderTargetView(&bb, None, Some(&mut rtv_out))?;
        self.rtv = rtv_out;
        Ok(())
    }
}

// ── Public HdrPanel ───────────────────────────────────────────────────────────

pub struct HdrPanel {
    pub hwnd:         HWND,
    pub hdr_active:   bool,
    pub square_count: usize,
    sq_values_hdr:    [f32; MAX_SQUARES],
    sq_values_sdr:    [f32; MAX_SQUARES],
    sq_values:        [f32; MAX_SQUARES],
    render_dirty:          bool,
    /// Set when `square_count` changes; causes `rebuild_digit_texture` to be
    /// called once at the start of the next `render_tick`, not on every drag tick.
    digit_texture_dirty:   bool,
    d3d:              Option<D3DResources>,
    width:            u32,
    height:           u32,
}

unsafe impl Send for HdrPanel {}
unsafe impl Sync for HdrPanel {}

impl HdrPanel {
    pub fn new() -> Self {
        let mut sq_values_hdr = [0f32; MAX_SQUARES];
        let mut sq_values_sdr = [0f32; MAX_SQUARES];
        for i in 0..MAX_SQUARES {
            sq_values_hdr[i] = pq_code_to_scrgb(ALL_PQ_CODES[i]);
            sq_values_sdr[i] = srgb_to_linear(ALL_SDR_CODES[i] as f32 / 255.0);
        }
        Self {
            hwnd: HWND(ptr::null_mut()),
            hdr_active: false, square_count: 9,
            sq_values_hdr, sq_values_sdr, sq_values: sq_values_sdr,
            render_dirty: true,
            digit_texture_dirty: false,
            d3d: None, width: 0, height: 0,
        }
    }

    pub unsafe fn init_d3d(&mut self, hwnd: HWND, w: u32, h: u32) {
        self.hwnd = hwnd; self.width = w; self.height = h;
        if w < 1 || h < 1 { return; }
        if let Err(e) = self.init_d3d_inner(hwnd, w, h) {
            eprintln!("[HdrPanel] init_d3d: {e}");
        }
    }

    unsafe fn init_d3d_inner(&mut self, hwnd: HWND, w: u32, h: u32) -> Result<()> {
        let feat = [D3D_FEATURE_LEVEL_11_0];
        let mut dev_out: Option<ID3D11Device>        = None;
        let mut ctx_out: Option<ID3D11DeviceContext>  = None;
        let mut feat_out = D3D_FEATURE_LEVEL_11_0;

        D3D11CreateDevice(
            None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(),
            D3D11_CREATE_DEVICE_FLAG::default(), Some(&feat), D3D11_SDK_VERSION,
            Some(&mut dev_out), Some(&mut feat_out), Some(&mut ctx_out),
        )?;
        let device: ID3D11Device       = dev_out.ok_or(Error::from(E_FAIL))?;
        let ctx:    ID3D11DeviceContext = ctx_out.ok_or(Error::from(E_FAIL))?;

        // Compile shaders
        let src      = HLSL.as_bytes();
        let src_name = s!("HdrPanel.hlsl");
        let mut vs_blob: Option<ID3DBlob> = None;
        let mut ps_blob: Option<ID3DBlob> = None;
        D3DCompile(src.as_ptr() as *const _, src.len(), src_name, None, None,
            s!("VS"), s!("vs_5_0"), 0, 0, &mut vs_blob, None)?;
        D3DCompile(src.as_ptr() as *const _, src.len(), src_name, None, None,
            s!("PS"), s!("ps_5_0"), 0, 0, &mut ps_blob, None)?;
        let vsb = vs_blob.ok_or(Error::from(E_FAIL))?;
        let psb = ps_blob.ok_or(Error::from(E_FAIL))?;
        let vs_bytes = std::slice::from_raw_parts(
            vsb.GetBufferPointer() as *const u8, vsb.GetBufferSize());
        let ps_bytes = std::slice::from_raw_parts(
            psb.GetBufferPointer() as *const u8, psb.GetBufferSize());

        let mut vs_out: Option<ID3D11VertexShader> = None;
        device.CreateVertexShader(vs_bytes, None, Some(&mut vs_out))?;
        let vs = vs_out.ok_or(Error::from(E_FAIL))?;

        let mut ps_out: Option<ID3D11PixelShader> = None;
        device.CreatePixelShader(ps_bytes, None, Some(&mut ps_out))?;
        let ps = ps_out.ok_or(Error::from(E_FAIL))?;

        // Constant buffer (112 bytes = 7 × float4)
        let cb_desc = D3D11_BUFFER_DESC {
            ByteWidth:      112,
            Usage:          D3D11_USAGE_DYNAMIC,
            BindFlags:      D3D11_BIND_CONSTANT_BUFFER.0 as u32,
            CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
            ..Default::default()
        };
        let mut cb_out: Option<ID3D11Buffer> = None;
        device.CreateBuffer(&cb_desc, None, Some(&mut cb_out))?;
        let cb = cb_out.ok_or(Error::from(E_FAIL))?;

        // Sampler
        let samp_desc = D3D11_SAMPLER_DESC {
            Filter:         D3D11_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU:       D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressV:       D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressW:       D3D11_TEXTURE_ADDRESS_CLAMP,
            MaxLOD:         f32::MAX,
            MaxAnisotropy:  1,
            ComparisonFunc: D3D11_COMPARISON_NEVER,
            ..Default::default()
        };
        let mut samp_out: Option<ID3D11SamplerState> = None;
        device.CreateSamplerState(&samp_desc, Some(&mut samp_out))?;
        let sampler = samp_out.ok_or(Error::from(E_FAIL))?;

        // Swap chain
        let dxgi_device: IDXGIDevice  = device.cast()?;
        let adapter:     IDXGIAdapter = dxgi_device.GetAdapter()?;
        let factory2:    IDXGIFactory2 = adapter.GetParent()?;

        let sc_desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: w, Height: h,
            Format:      DXGI_FORMAT_R16G16B16A16_FLOAT,
            BufferCount: 2,
            SwapEffect:  DXGI_SWAP_EFFECT_FLIP_DISCARD,
            SampleDesc:  DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            Scaling:     DXGI_SCALING_STRETCH,
            AlphaMode:   DXGI_ALPHA_MODE_IGNORE,
            Flags:       0,
            ..Default::default()
        };
        let sc1: IDXGISwapChain1 = factory2.CreateSwapChainForHwnd(
            &device, hwnd, &sc_desc, None, None)?;
        factory2.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER)?;
        let swap3: IDXGISwapChain3 = sc1.cast()?;
        let _ = swap3.SetColorSpace1(DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709);
        self.hdr_active = is_any_monitor_hdr();

        let mut res = D3DResources {
            device, ctx, swap3, rtv: None,
            vs, ps, cb, sampler, digit_tex: None, digit_srv: None,
        };
        res.create_rtv()?;
        self.d3d = Some(res);

        self.recompute_square_values();
        self.rebuild_digit_texture();
        self.render_dirty = true;
        Ok(())
    }

    // ── Public update / resize ────────────────────────────────────────────────

    pub unsafe fn update(&mut self, black_lift: i32) {
        let _ = black_lift; // lift is applied via SetDeviceGammaRamp; panel uses raw values
        self.recompute_square_values();
        self.render_dirty = true;
    }

    pub unsafe fn set_square_count(&mut self, count: usize) {
        let count = count.clamp(1, MAX_SQUARES);
        if self.square_count == count { return; }
        self.square_count = count;
        self.recompute_square_values();
        // Do NOT call rebuild_digit_texture() here — that does a full GDI rasterise
        // + CreateTexture2D + CreateShaderResourceView on every drag tick, which is
        // expensive enough to make the slider feel like low-FPS.  Instead, mark the
        // texture stale and let render_tick rebuild it once when the slider settles.
        self.digit_texture_dirty = true;
        self.render_dirty = true;
    }

    pub unsafe fn resize(&mut self, w: u32, h: u32) {
        if w < 1 || h < 1 { return; }
        self.width = w; self.height = h;
        if let Some(ref mut res) = self.d3d {
            res.rtv = None; // release before resize
            let _ = res.swap3.ResizeBuffers(
                0, w, h, DXGI_FORMAT_UNKNOWN,
                DXGI_SWAP_CHAIN_FLAG(0),
            );
            let _ = res.create_rtv();
        }
        self.rebuild_digit_texture();
        self.render_dirty = true;
    }

    pub unsafe fn refresh_hdr_status(&mut self) -> bool {
        let hdr     = is_any_monitor_hdr();
        let changed = hdr != self.hdr_active;
        self.hdr_active = hdr;
        if let Some(ref res) = self.d3d {
            let _ = res.swap3.SetColorSpace1(DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709);
        }
        self.recompute_square_values();
        if changed { self.rebuild_digit_texture(); self.render_dirty = true; }
        changed
    }

    pub unsafe fn render_tick(&mut self) {
        // Proactively detect device loss even when not dirty, so a driver
        // restart while the panel is hidden still gets caught on the next tick.
        if self.is_device_lost() {
            self.recover_device();
            return;
        }
        // Rebuild the digit texture lazily — only once the slider has settled
        // (i.e. on the first render_tick after set_square_count was called),
        // instead of on every drag tick where the GPU upload would block the UI.
        if self.digit_texture_dirty {
            self.digit_texture_dirty = false;
            self.rebuild_digit_texture();
        }
        if self.render_dirty {
            self.render_dirty = false;
            let lost = self.render();
            if lost { self.recover_device(); }
        }
    }

    /// Returns true if the D3D device has been removed (driver restart / GPU reset).
    unsafe fn is_device_lost(&self) -> bool {
        match &self.d3d {
            None => false,
            Some(res) => res.device.GetDeviceRemovedReason().is_err(),
        }
    }

    /// Drop the lost device and rebuild it from scratch.
    unsafe fn recover_device(&mut self) {
        self.d3d = None;
        self.render_dirty = true;
        if !self.hwnd.0.is_null() && self.width > 0 && self.height > 0 {
            self.init_d3d(self.hwnd, self.width, self.height);
        }
    }

    /// Release all GPU resources while the window is hidden (minimized to tray).
    ///
    /// Drops the D3D device, swap chain, and all associated VRAM allocations
    /// (~40 MB) so they are not held while the UI is invisible.  `render_dirty`
    /// is set so the first `render_tick` after `init_d3d` repaints immediately.
    /// The context is flushed before dropping to ensure the GPU has no
    /// in-flight work referencing any of the released resources.
    pub unsafe fn suspend_d3d(&mut self) {
        if let Some(ref res) = self.d3d {
            // Flush any pending GPU commands before releasing the device so the
            // driver doesn't hold references past the drop.
            res.ctx.Flush();
        }
        self.d3d = None;
        self.render_dirty = true;
    }

    // ── Internal ─────────────────────────────────────────────────────────────

    fn recompute_square_values(&mut self) {
        if self.hdr_active { self.sq_values = self.sq_values_hdr; return; }
        // Use raw sRGB values — the GPU gamma ramp lifts them the same as any
        // other SDR content, so the panel matches what the browser renders.
        self.sq_values = self.sq_values_sdr;
    }

    unsafe fn rebuild_digit_texture(&mut self) {
        let res = match &mut self.d3d { Some(r) => r, None => return };
        let w = self.width  as i32;
        let h = self.height as i32;
        if w < 1 || h < 1 { return; }

        let count  = self.square_count as i32;
        let gap_px = GAP_FRACTION * w as f32;
        let cell_w = ((w as f32 - gap_px * (count - 1) as f32) / count as f32).max(1.0) as i32;
        let cell_h = h;
        let tex_w  = cell_w * count;
        let tex_h  = cell_h;
        let stride = (tex_w * 4) as usize;
        let mut pixels = vec![0u8; stride * tex_h as usize];

        // GDI rasterise labels into BGRA8 buffer
        let screen_dc = GetDC(HWND(ptr::null_mut()));
        let mem_dc    = CreateCompatibleDC(screen_dc);
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize:     mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth:    tex_w,
                biHeight:   -tex_h, // top-down
                biPlanes:   1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits_ptr: *mut std::ffi::c_void = ptr::null_mut();
        let hbmp = CreateDIBSection(mem_dc, &bmi, DIB_RGB_COLORS,
            &mut bits_ptr, HANDLE::default(), 0).expect("CreateDIBSection");
        SelectObject(mem_dc, hbmp);

        // Cap at 22 px so labels don't balloon when the panel is tall (e.g. maximised).
        let label_px = ((cell_w.min(cell_h) as f32) * 0.36).clamp(5.0, 50.0) as i32;
        let hfont = CreateFontW(
            label_px, 0, 0, 0, FW_REGULAR.0 as i32, 0, 0, 0,
            DEFAULT_CHARSET.0 as u32, OUT_DEFAULT_PRECIS.0 as u32,
            CLIP_DEFAULT_PRECIS.0 as u32, ANTIALIASED_QUALITY.0 as u32,
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32, w!("Segoe UI"),
        );
        SelectObject(mem_dc, hfont);
        SetTextColor(mem_dc, COLORREF(0x00606060)); // mid-gray: readable above near-black, not glaring
        SetBkMode(mem_dc, TRANSPARENT);

        let is_hdr = self.hdr_active;
        for i in 0..count as usize {
            let ox  = i as i32 * cell_w;
            let dig: String = if i == 0 {
                if is_hdr { "64".into() } else { "0".into() }
            } else if is_hdr { ALL_PQ_CODES[i].to_string() }
              else { ALL_SDR_CODES[i].to_string() };
            let lbl = if i == 0 { "Black" } else if is_hdr { "PQ" } else { "RGB" };

            let dig_w: Vec<u16> = dig.encode_utf16().collect();
            let lbl_w: Vec<u16> = lbl.encode_utf16().collect();
            let mut dig_sz = SIZE::default();
            let mut lbl_sz = SIZE::default();
            GetTextExtentPoint32W(mem_dc, &dig_w, &mut dig_sz);
            GetTextExtentPoint32W(mem_dc, &lbl_w, &mut lbl_sz);

            let line_gap  = (cell_h as f32 * 0.02) as i32;
            let block_h   = dig_sz.cy + line_gap + lbl_sz.cy;
            let block_top = cell_h - block_h - (cell_h as f32 * 0.05) as i32;

            let dx = ox + (cell_w - dig_sz.cx) / 2;
            let lx = ox + (cell_w - lbl_sz.cx) / 2;
            TextOutW(mem_dc, dx, block_top, &dig_w);
            TextOutW(mem_dc, lx, block_top + dig_sz.cy + line_gap, &lbl_w);
        }

        // Copy GDI bits and fix alpha channel.
        // GDI renders into BGRA: bytes are [B, G, R, A] = indices [0, 1, 2, 3].
        // Text pixels are gray (B=G=R=0x60); background pixels are 0,0,0,0.
        // Use blue channel as alpha: 0x60 (96) for text, 0 for background.
        // texSample.r in shader = R channel = 0x60/255 ≈ 0.376 → readable mid-gray overlay.
        let gdi = std::slice::from_raw_parts(bits_ptr as *const u8, stride * tex_h as usize);
        pixels.copy_from_slice(gdi);
        for px in pixels.chunks_exact_mut(4) { px[3] = px[0]; } // alpha = B channel

        DeleteObject(hfont);
        DeleteObject(hbmp);
        DeleteDC(mem_dc);
        ReleaseDC(HWND(ptr::null_mut()), screen_dc);

        // Upload texture
        res.digit_srv = None; res.digit_tex = None;
        let tex_desc = D3D11_TEXTURE2D_DESC {
            Width: tex_w as u32, Height: tex_h as u32,
            MipLevels: 1, ArraySize: 1,
            Format:     DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage:      D3D11_USAGE_IMMUTABLE,
            BindFlags:  D3D11_BIND_SHADER_RESOURCE.0 as u32,
            ..Default::default()
        };
        let init = D3D11_SUBRESOURCE_DATA {
            pSysMem:     pixels.as_ptr() as *const _,
            SysMemPitch: stride as u32,
            ..Default::default()
        };
        let mut tex_out: Option<ID3D11Texture2D> = None;
        if res.device.CreateTexture2D(&tex_desc, Some(&init), Some(&mut tex_out)).is_ok() {
            if let Some(tex) = tex_out {
                let mut srv_out: Option<ID3D11ShaderResourceView> = None;
                if res.device.CreateShaderResourceView(&tex, None, Some(&mut srv_out)).is_ok() {
                    res.digit_srv = srv_out;
                }
                res.digit_tex = Some(tex);
            }
        }
    }

    /// Renders one frame. Returns true if the device was lost and the caller
    /// should call `recover_device()`.
    unsafe fn render(&mut self) -> bool {
        let res = match &mut self.d3d { Some(r) => r, None => return false };
        let rtv = match &res.rtv { Some(r) => r.clone(), None => return false };

        let count = self.square_count;
        let vals  = &self.sq_values;
        let v     = |i: usize| if i < count { vals[i] } else { 0.0 };

        let cb_data = CbData {
            sq: [ v(0),v(1),v(2),v(3),v(4),v(5),v(6),v(7),
                  v(8),v(9),v(10),v(11),v(12),v(13),v(14),v(15),
                  v(16),v(17),v(18),v(19),v(20),v(21),v(22),v(23) ],
            sq_count: count as i32, sq_cols: count as i32,
            sq_rows: 1, gap_frac: GAP_FRACTION,
        };

        // Upload constant buffer
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        if res.ctx.Map(&res.cb, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped)).is_ok() {
            ptr::copy_nonoverlapping(
                &cb_data as *const CbData as *const u8,
                mapped.pData as *mut u8,
                mem::size_of::<CbData>(),
            );
            res.ctx.Unmap(&res.cb, 0);
        }

        res.ctx.ClearRenderTargetView(&rtv, &[0f32, 0.0, 0.0, 1.0]);
        res.ctx.OMSetRenderTargets(Some(&[Some(rtv)]), None);
        let vp = D3D11_VIEWPORT {
            Width: self.width as f32, Height: self.height as f32, MaxDepth: 1.0,
            ..Default::default()
        };
        res.ctx.RSSetViewports(Some(&[vp]));
        res.ctx.IASetPrimitiveTopology(D3D11_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        res.ctx.VSSetShader(&res.vs, None);
        res.ctx.PSSetShader(&res.ps, None);
        res.ctx.PSSetConstantBuffers(0, Some(&[Some(res.cb.clone())]));

        if let (Some(srv), _) = (&res.digit_srv, &res.sampler) {
            res.ctx.PSSetShaderResources(0, Some(&[Some(srv.clone())]));
            res.ctx.PSSetSamplers(0, Some(&[Some(res.sampler.clone())]));
        }

        res.ctx.Draw(3, 0);

        // Present without V-sync blocking (SyncInterval=0, DXGI_PRESENT_DO_NOT_WAIT).
        //
        // The panel only renders when `render_dirty` is set — on discrete user
        // actions (slider move, resize, HDR toggle), never on a continuous loop.
        // SyncInterval=1 would block the UI-thread timer callback until the next
        // vblank, injecting exactly the irregular cadence that makes VRR stutter.
        //
        // DXGI_PRESENT_DO_NOT_WAIT (0x08): if the previous frame hasn't been
        // consumed yet, DXGI returns DXGI_ERROR_WAS_STILL_DRAWING.  We re-set
        // render_dirty so the next render_tick retries — the in-flight frame
        // already shows correct content so no visual glitch occurs.
        //
        // Device-removal errors (DEVICE_REMOVED 0x887A0005 / DEVICE_RESET
        // 0x887A0007) are still caught and forwarded to the caller.
        const DXGI_PRESENT_DO_NOT_WAIT: u32 = 0x08;
        const DXGI_ERROR_WAS_STILL_DRAWING: i32 = 0x887A000Au32 as i32;
        let present_result = res.swap3.Present(0, DXGI_PRESENT(DXGI_PRESENT_DO_NOT_WAIT));
        if present_result.is_err() {
            if present_result == windows::core::HRESULT(DXGI_ERROR_WAS_STILL_DRAWING).into() {
                self.render_dirty = true; // retry on next tick
                return false;
            }
            if res.device.GetDeviceRemovedReason().is_err() {
                return true; // signal device loss to caller
            }
        }
        false
    }
}

impl Default for HdrPanel {
    fn default() -> Self { Self::new() }
}

// ── HDR detection ─────────────────────────────────────────────────────────────

unsafe fn is_any_monitor_hdr() -> bool {
    let factory: IDXGIFactory1 = match CreateDXGIFactory1() {
        Ok(f)  => f,
        Err(_) => return false,
    };
    let mut ai = 0u32;
    loop {
        let adapter: IDXGIAdapter = match factory.EnumAdapters(ai) {
            Ok(a) => a, Err(_) => break,
        };
        let mut oi = 0u32;
        loop {
            let output: IDXGIOutput = match adapter.EnumOutputs(oi) {
                Ok(o) => o, Err(_) => break,
            };
            if let Ok(out6) = output.cast::<IDXGIOutput6>() {
                if let Ok(desc) = out6.GetDesc1() {
                    if desc.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020 {
                        return true;
                    }
                }
            }
            oi += 1;
        }
        ai += 1;
    }
    false
}