// app.rs — AppState definition, main window entry point, WndProc, layout,
//          and cross-tab shared business logic (tray, startup, status bar).
//
// Per-tab logic has been extracted into dedicated modules:
//   tab_crush.rs  — Black Crush Tweak tab (gamma ramp, Hz profiles, compare button)
//   tab_dimmer.rs — Taskbar Dimmer tab (overlay windows, fade animation, auto-hide detection)
//
// Rendering helpers (subclass procs, paint routines, modal dialogs) live in
// ui_drawing.rs. Compile-time constants and IDs live in constants.rs.

#![allow(non_snake_case, clippy::too_many_lines, unused_variables,
         unused_mut, unused_assignments, unused_must_use)]

use std::{mem, path::{Path, PathBuf}, ptr};

use windows::{
    core::*,
    Win32::{
        Devices::Display::*,
        Foundation::*,
        Graphics::{Dwm::*, Gdi::*},
        System::{
            LibraryLoader::GetModuleHandleW,
            Power::*,
            RemoteDesktop::*,
        },
        UI::{
            Controls::*,
            HiDpi::GetDpiForWindow,
            Input::KeyboardAndMouse::*,
            Shell::*,
            WindowsAndMessaging::*,
        },
    },
};

use crate::{
    app_layout,
    constants::*,
    hotkeys::{
        HK_TOGGLE_DIM, HK_TOGGLE_CRUSH, HK_HOLD_COMPARE,
        HK_DECREASE, HK_INCREASE, HK_DIM_DECREASE, HK_DIM_INCREASE,
        MOUSE_HK_SLOTS,
        install_compare_hook, uninstall_compare_hook,
        ensure_mouse_hook_installed, uninstall_mouse_hook,
        set_mouse_hk_slot,
        parse_hotkey, register_hotkeys,
    },
    startup,
    tray,
    controls::ControlBuilder,
    gamma_ramp,
    profile_manager::ProfileManager,
    tab_crush::{CrushTab, get_current_hz, hz_section, render_interval_ms},
    tab_debug::DebugTab,
    tab_about::AboutTab,
    tab_hotkeys::{HotkeysTab, draw_hotkey_pill},
    tab_dimmer::{DimmerTab, ZOrderWinEventHooks, ZLogKind, zorder_log},
    ui_drawing::{
        self,
        client_size_of,
        draw_dark_button_full, draw_hdr_toggle_switch, draw_nav_item, NAV_BTN_HOVER_PROP,
        get_slider_val, make_font,
        set_bounds, set_window_text,
        slider_subclass_proc, nav_btn_subclass_proc,
        hdr_toggle_subclass_proc, dimmer_toggle_subclass_proc,
        SetWindowSubclass,
    },
    win32::{
        attach_state, borrow_state, detach_state,
        set_text, set_text_fmt,
        set_visible, redraw_now,
        ControlGroup,
        toggle_hdr_via_shortcut,
    },
};

// ── Deferred DPI resize ───────────────────────────────────────────────────────
//
// WM_DPICHANGED posts this message to itself instead of doing SetWindowPos
// synchronously, so WM_DISPLAYCHANGE can arrive (and reposition overlays)
// before the layout reflow happens.  See the WM_DPICHANGED handler for details.
use windows::Win32::UI::WindowsAndMessaging::WM_APP;
const WM_APP_DEFERRED_DPI_RESIZE: u32 = WM_APP + 10;

use std::cell::Cell;
thread_local! {
    /// Stores the suggested RECT from WM_DPICHANGED so the deferred resize
    /// handler (WM_APP_DEFERRED_DPI_RESIZE) can read it without extra allocations.
    static DEFERRED_DPI_RECT: Cell<RECT> = Cell::new(RECT::default());
}

// ── Per-window state ──────────────────────────────────────────────────────────

const IDC_BTN_HDR_TOGGLE: usize = 150;

pub struct AppState {
    #[allow(dead_code)]
    hwnd: HWND,

    // ── Tab sub-states ────────────────────────────────────────────────────────
    /// Black Crush Tweak tab (tab 0).
    pub crush:  CrushTab,
    /// Taskbar Dimmer tab (tab 1).
    pub dimmer: DimmerTab,
    /// Hotkeys tab (tab 2).
    pub hotkeys: HotkeysTab,
    /// Debug tab (tab 3).
    pub debug: DebugTab,
    /// About tab (tab 4).
    pub about: AboutTab,

    // ── Shared controls ───────────────────────────────────────────────────────
    pub h_chk_startup:    HWND,
    pub h_btn_quit:       HWND,
    pub h_btn_minimize:   HWND,
    pub h_btn_hdr_toggle: HWND,
    pub h_lbl_status:     HWND,
    /// Persistent error label shown below h_lbl_status (gamma blocked, etc.).
    pub h_lbl_error:      HWND,
    pub h_sep_vert:       HWND,
    pub h_sep_h:          [HWND; 4],
    /// Left-panel vertical navigation buttons (one per tab).
    pub h_nav_btn:        [HWND; 5],

    /// App icon used in the nav button for tab 0.
    h_app_icon:   HICON,
    /// Decoded PNG icons for nav buttons (None = use built-in glyph).
    nav_icons:    crate::nav_icons::NavIcons,
    /// Decoded PNG icons at 32px for tab header titles.
    tab_header_icons: crate::nav_icons::TabHeaderIcons,
    /// 0 = Black Crush Tweak, 1 = Taskbar Dimmer, 2 = Debug
    active_tab:   usize,

    // ── Shared brushes / fonts (kept alive for WM_PAINT lifetime) ────────────
    bg_brush:    HBRUSH,
    bg3_brush:   HBRUSH,
    sep_brush:   HBRUSH,

    _font_normal:   HFONT,
    _font_title:    HFONT,
    _font_bold_val: HFONT,

    // ── Tray / misc ───────────────────────────────────────────────────────────
    tray_menu:   HMENU,
    tray_added:  bool,

    ini:               ProfileManager,
    status_color:      COLORREF,
    chk_startup_state: bool,
    layout_initialized: bool,

    /// WinEvent hooks for deferred taskbar overlay Z-order (foreground + minimize).
    zorder_winevent_hooks: Option<ZOrderWinEventHooks>,

    /// Mouse-button bindings that can't use RegisterHotKey.
    /// Indexed by HK_* constant; value is a MB_* sentinel (0 = unbound).
    mouse_hotkeys: [u32; 8],

    /// Cached result of `CrushTab::is_nvidia_cam_enabled()`.
    /// Refreshed at startup and on every TIMER_HDR tick (~2 s).
    /// Avoids a registry enumeration loop inside the hot `apply_ramp` path.
    nvidia_cam_enabled: bool,

    /// Set to `true` when the black-level slider moves during a drag.
    /// `TIMER_RAMP_APPLY` fires ~50 ms later and calls `apply_ramp`, then
    /// clears this flag.  Keeps `SetDeviceGammaRamp` off the hot drag path.
    ramp_dirty: bool,
}

impl AppState {
    /// Recreate all DPI-sensitive fonts and push them to every child control.
    /// Call this inside WM_APP_DEFERRED_DPI_RESIZE before SetWindowPos.
    pub unsafe fn rebuild_fonts(&mut self, hwnd: HWND) {
        let dpi = GetDpiForWindow(hwnd).max(96);

        // ── Delete old owned fonts ────────────────────────────────────────────
        DeleteObject(HGDIOBJ(self._font_normal.0));
        DeleteObject(HGDIOBJ(self._font_title.0));
        DeleteObject(HGDIOBJ(self._font_bold_val.0));

        // ── Create new DPI-correct fonts ──────────────────────────────────────
        self._font_normal   = make_font(w!("Segoe UI"), 10, dpi, false);
        self._font_title    = make_font(w!("Segoe UI"), 16, dpi, true);
        self._font_bold_val = make_font(w!("Consolas"), 14, dpi, true);

        // ── Broadcast normal font to all child controls ───────────────────────
        // EnumChildWindows visits every descendant; we use it as a cheap way to
        // reset the default font on controls that don't need a special override.
        EnumChildWindows(
            hwnd,
            Some(set_font_enum_proc),
            LPARAM(self._font_normal.0 as isize),
        );

        // ── Override controls that use title / bold-val fonts ─────────────────
        for &h in &[
            self.crush.h_lbl_title,
            self.dimmer.h_lbl_dim_title,
            self.hotkeys.h_lbl_title,
            self.debug.h_lbl_title,
            self.about.h_lbl_title,
        ] {
            SendMessageW(h, WM_SETFONT, WPARAM(self._font_title.0 as usize), LPARAM(1));
        }
        SendMessageW(
            self.crush.h_lbl_black_val,
            WM_SETFONT,
            WPARAM(self._font_bold_val.0 as usize),
            LPARAM(1),
        );

        // ── Recreate and reapply 11pt-bold section-heading font ───────────────
        // Use a shared cache so repeated DPI rebuilds reuse the same font.
        let font_sect = crate::ui_drawing::make_font_cached(w!("Segoe UI"), 11, dpi, true);
        for &h in &[
            self.crush.h_lbl_bl_sect,
            self.crush.h_lbl_hdr_sect,
            self.crush.h_lbl_ref_sect,
            self.dimmer.h_lbl_dim_sect,
            self.dimmer.h_lbl_fade_sect,
            self.hotkeys.h_lbl_sect_crush,
            self.hotkeys.h_lbl_sect_dimmer,
            self.debug.h_lbl_sect_state,
            self.debug.h_lbl_sect_suppress,
            self.debug.h_lbl_sect_log,
            self.about.h_lbl_sect_about,
            self.about.h_lbl_sect_update,
        ] {
            SendMessageW(h, WM_SETFONT, WPARAM(font_sect.0 as usize), LPARAM(1));
        }
        // font_sect is leaked intentionally, same pattern as ::new()

        // ── Reload nav / tab-header icons at new DPI ──────────────────────────
        self.nav_icons.destroy();
        self.tab_header_icons.destroy();
        self.nav_icons        = crate::nav_icons::NavIcons::load(dpi);
        self.tab_header_icons = crate::nav_icons::TabHeaderIcons::load(dpi);

        // ── Patch zoom-icon statics: overwrite subclass ref_data ─────────────
        // bitmap_static_subclass_proc stores the HBITMAP as the SetWindowSubclass
        // ref_data (UID 3).  The old handle was just deleted above; calling
        // SetWindowSubclass again with the same proc + UID overwrites ref_data
        // in-place.  Without this the subclass still holds the freed handle,
        // GetObjectW returns 0, and AlphaBlend renders nothing.
        crate::ui_drawing::SetWindowSubclass(
            self.crush.h_lbl_zoom_out_icon,
            Some(crate::ui_drawing::bitmap_static_subclass_proc),
            3,
            self.nav_icons.zoom_out.map(|b| b.0 as usize).unwrap_or(0),
        );
        InvalidateRect(self.crush.h_lbl_zoom_out_icon, None, false);
        crate::ui_drawing::SetWindowSubclass(
            self.crush.h_lbl_zoom_icon,
            Some(crate::ui_drawing::bitmap_static_subclass_proc),
            3,
            self.nav_icons.zoom.map(|b| b.0 as usize).unwrap_or(0),
        );
        InvalidateRect(self.crush.h_lbl_zoom_icon, None, false);

        // ── Patch tab-header title labels: update TAB_HDR_BITMAP property ─────
        // subclass_tab_header stores the HBITMAP via SetPropW(TAB_HDR_BITMAP).
        // The old handle was just deleted; stamp the new DPI-correct one onto
        // every title label so paint_tab_header picks it up on the next WM_PAINT.
        let hdr_pairs: &[(HWND, Option<HBITMAP>)] = &[
            (self.crush.h_lbl_title,      self.tab_header_icons.crush),
            (self.dimmer.h_lbl_dim_title, self.tab_header_icons.dimmer),
            (self.hotkeys.h_lbl_title,    self.tab_header_icons.hotkeys),
            (self.debug.h_lbl_title,      self.tab_header_icons.debug),
            (self.about.h_lbl_title,      self.tab_header_icons.about),
        ];
        for &(h, bmp) in hdr_pairs {
            let raw = bmp.map(|b| b.0 as isize).unwrap_or(0);
            SetPropW(h, crate::ui_drawing::TAB_HDR_BITMAP, HANDLE(raw as *mut _));
            InvalidateRect(h, None, false);
        }
    }
}

/// EnumChildWindows callback: sends WM_SETFONT (lparam = HFONT handle) to every child.
unsafe extern "system" fn set_font_enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    SendMessageW(hwnd, WM_SETFONT, WPARAM(lparam.0 as usize), LPARAM(1));
    BOOL(1)
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub unsafe fn run(ini_path: PathBuf) -> Result<()> {
    use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
    let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    let hinstance: HINSTANCE = GetModuleHandleW(None)?.into();

    let icc = INITCOMMONCONTROLSEX {
        dwSize: mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC:  ICC_BAR_CLASSES | ICC_STANDARD_CLASSES,
    };
    InitCommonControlsEx(&icc);

    let class_name = w!("OledHelper_Main");
    let wc = WNDCLASSEXW {
        cbSize:        mem::size_of::<WNDCLASSEXW>() as u32,
        style:         WNDCLASS_STYLES(0),
        lpfnWndProc:   Some(wnd_proc),
        hInstance:     hinstance,
        hbrBackground: HBRUSH((COLOR_WINDOW.0 as usize + 1) as *mut _),
        lpszClassName: class_name,
        hIcon:         LoadIconW(hinstance, w!("MAINICON")).unwrap_or_default(),
        hCursor:       LoadCursorW(None, IDC_ARROW)?,
        ..Default::default()
    };
    RegisterClassExW(&wc);

    let ini_boxed    = Box::new(ini_path);
    let create_param = Box::into_raw(ini_boxed) as *mut _;

    let sys_dpi = windows::Win32::UI::HiDpi::GetDpiForSystem().max(96);
    let scale   = |px: i32| -> i32 { (px as f32 * sys_dpi as f32 / 96.0).round() as i32 };

    let mut window_rect = RECT {
        left: 0,
        top: 0,
        right: scale(MIN_WIN_W),
        bottom: scale(MIN_WIN_H),
    };
    let _ = AdjustWindowRectEx(
        &mut window_rect,
        WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
        false,
        WS_EX_APPWINDOW | WS_EX_COMPOSITED,
    );
    let width = window_rect.right - window_rect.left;
    let height = window_rect.bottom - window_rect.top;
    let x = (GetSystemMetrics(SM_CXSCREEN) - width) / 2;
    let y = (GetSystemMetrics(SM_CYSCREEN) - height) / 2;

    let window_title: Vec<u16> = format!("Oled Helper v{}\0", crate::tab_about::APP_VERSION)
        .encode_utf16().collect();
    let hwnd = CreateWindowExW(
        WS_EX_APPWINDOW | WS_EX_COMPOSITED,
        class_name,
        PCWSTR(window_title.as_ptr()),
        WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
        x, y, width, height,
        None, None, hinstance, Some(create_param),
    )?;

    ShowWindow(hwnd, SW_SHOW);
    UpdateWindow(hwnd);

    let mut msg = MSG::default();
    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
        if !IsDialogMessageW(hwnd, &msg).as_bool() {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    // Message loop has fully exited — WM_DESTROY has been processed and
    // detach_state has run.  The mutex is still held here (it drops when
    // run_app() returns), so we must NOT spawn yet.  Signal main.rs instead.
    Ok(())
}

// ── WndProc ───────────────────────────────────────────────────────────────────

unsafe extern "system" fn wnd_proc(
    hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM,
) -> LRESULT {

    match msg {
        
        WM_CREATE => {
            let cs   = &*(lparam.0 as *const CREATESTRUCTW);
            let path = Box::from_raw(cs.lpCreateParams as *mut PathBuf);
            attach_state(hwnd, Box::new(create_state(hwnd, *path)));

            

            // Register the main HWND so the WinEvent hook proc (which runs on
            // a system thread) can PostMessageW back to us.  Must be set before
            // any WM_APP_FULLSCREEN_CHECK can arrive.
            crate::tab_dimmer::register_main_hwnd(hwnd);

            let state = borrow_state::<AppState>(hwnd).unwrap();

            let dark: i32 = 1;
            let _ = DwmSetWindowAttribute(
                hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE,
                &dark as *const _ as _, 4,
            );
            WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION);
            // Note: timeBeginPeriod(1) is NOT called here.  The 1 ms system
            // timer period was needed when TIMER_RENDER fired every ~7-16 ms
            // to pace the D3D Present call at VRR cadence.  Now that Present
            // uses SyncInterval=0 and TIMER_RENDER fires at 100 ms (only
            // doing work when render_dirty is set), the global timer resolution
            // change is unnecessary and was a known VRR flicker trigger.
            // timeBeginPeriod(1) remains available for slider animations via
            // the slider subclass, but is not set for the whole app lifetime.
            SetTimer(hwnd, TIMER_HDR,          2000, None);
            // TIMER_DEBUG_REFRESH is NOT started here.  It is armed by show_tab
            // when the user switches to the debug tab (tab 3) and killed when
            // they switch away — so it never runs in production (non-debug) builds
            // and never wakes the CPU while the debug tab is not visible.
            SetTimer(hwnd, TIMER_RENDER, render_interval_ms(), None);
            LRESULT(0)
        }
        
       // 1. DPI Change: the app's effective DPI changed, but the display geometry
       //    has NOT changed yet — the taskbar is still in its old position.
       //
       //    IMPORTANT: Do NOT call SetWindowPos/RedrawWindow synchronously here.
       //    WM_DPICHANGED is sent via SendMessage (not posted), which means it
       //    arrives while the display change broadcast is in progress.  Any
       //    synchronous layout work (SetWindowPos + WM_SIZE reflow + child
       //    control repaints) blocks the message loop for the entire duration,
       //    preventing WM_DISPLAYCHANGE from arriving.  This was causing the
       //    ~2.4 s delay between [DP] and [DC] in the debug log.
       //
       //    Fix: stash the suggested rect in a thread-local and post
       //    WM_APP_DEFERRED_DPI_RESIZE to ourselves.  The posted message is
       //    processed after SendMessage returns and the broadcast finishes, so
       //    WM_DISPLAYCHANGE can arrive in the very next pump cycle.
       WM_DPICHANGED => {
            let prc = lparam.0 as *const RECT;
            let new_rect = unsafe { *prc };

            // Log immediately — this timestamp anchors the whole sequence.
            if is_debug_mode() {
                let dpi = unsafe { GetDpiForWindow(hwnd) } as u64;
                let detail = format!(
                    "[DP] WM_DPICHANGED  new_dpi={}  suggested=({},{} {}x{})",
                    dpi,
                    new_rect.left, new_rect.top,
                    new_rect.right - new_rect.left,
                    new_rect.bottom - new_rect.top,
                );
                if let Some(Ok(mut log)) = zorder_log().map(|m| m.lock()) {
                    log.push(unsafe { windows::Win32::System::SystemInformation::GetTickCount64() },
                        ZLogKind::DpiChanged, dpi, detail);
                }
            }

            // Stash the suggested rect so the deferred handler can read it.
            DEFERRED_DPI_RECT.with(|cell| cell.set(new_rect));

            // Post (not send) the deferred resize.  This returns immediately,
            // allowing WM_DISPLAYCHANGE to be dispatched before the layout work.
            unsafe {
                PostMessageW(hwnd, WM_APP_DEFERRED_DPI_RESIZE, WPARAM(0), LPARAM(0));
            }

            // Even though display geometry hasn't fully switched yet, we can call
// reposition_overlays_now() safely: collect_taskbar_rects() uses
// GetMonitorInfoW which returns physical-pixel rects regardless of the
// current DPI state.  The overlay will be at or very near the correct
// position immediately, and tick_reposition will correct any remaining
// offset once WM_DISPLAYCHANGE arrives.
if let Some(st) = borrow_state::<AppState>(hwnd) {
    if st.dimmer.enabled {
        st.dimmer.start_reposition_overlays(); // hides + starts polling
    }
}
            LRESULT(0)
        }

        // Deferred handler: performs the SetWindowPos + layout reflow that was
        // previously done synchronously inside WM_DPICHANGED.  By the time this
        // fires, WM_DISPLAYCHANGE has already been dispatched, so the new display
        // geometry is known and the taskbar rect is stable.
        //
        // If start_reposition_overlays() was called from WM_DISPLAYCHANGE and the
        // polling loop is still running, we skip the remaining ticks and reposition
        // immediately — the taskbar has settled now that the DPI reflow is done.
        // This eliminates the ~2.4 s delay seen when opening from the taskbar
        // (where WM_DPICHANGED blocked for the full first-paint layout pass before
        // WM_DISPLAYCHANGE could arrive, leaving tick_reposition polling long after
        // the taskbar had already moved to its final position).
        WM_APP_DEFERRED_DPI_RESIZE => {
            let new_rect = DEFERRED_DPI_RECT.with(|cell| cell.get());

            // ── Instrumentation: log entry, post-SetWindowPos, post-RedrawWindow ──
            // Three [DR] lines bracket each expensive call so the debug log shows
            // exactly where the 2+ s delay is being spent.
            //
            // Phase 0 — handler entered (posted message dequeued by the pump).
            //   Delta from [DP] = time WM_DPICHANGED spent blocked before the
            //   posted message could be processed.  Should be near-zero if the
            //   deferred-post trick is working; large if something else is holding
            //   the message loop (e.g. DWM, shell broadcast, or a synchronous
            //   SendMessage from another window during the DPI transition).
            //
            // Phase 1 — after SetWindowPos.
            //   Delta from phase 0 = time spent inside SetWindowPos + the WM_SIZE
            //   reflow it triggers (layout pass, child control reposition).
            //   This is expected to be the dominant cost on first show.
            //
            // Phase 2 — after RedrawWindow.
            //   Delta from phase 1 = time spent queueing + processing all the
            //   WM_PAINT messages for every child control.  Usually small, but
            //   can be large when the D3D HDR panel is initialising.
            macro_rules! log_dr {
                ($phase:expr, $label:expr) => {
                    if is_debug_mode() {
                        if let Some(Ok(mut log)) = zorder_log().map(|m| m.lock()) {
                            let tick = unsafe { windows::Win32::System::SystemInformation::GetTickCount64() };
                            let detail = format!(
                                "[DR] deferred_dpi_resize  phase={}  {}",
                                $phase, $label,
                            );
                            let payload = (($phase as u64) << 48) | (tick & 0x0000_FFFF_FFFF_FFFF);
                            log.push(tick,
                                crate::tab_dimmer::ZLogKind::DeferredDpiResize,
                                payload, detail);
                        }
                    }
                };
            }

            log_dr!(0u64, "entry");

            // Rebuild all DPI-sensitive fonts and push them to every child
            // control BEFORE SetWindowPos triggers the layout reflow (WM_SIZE →
            // app_layout::apply).  This ensures controls paint with the correct
            // font size immediately on their first post-resize WM_PAINT.
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                st.rebuild_fonts(hwnd);
            }

            unsafe {
                SetWindowPos(
                    hwnd, HWND::default(),
                    new_rect.left, new_rect.top,
                    new_rect.right - new_rect.left,
                    new_rect.bottom - new_rect.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }

            log_dr!(1u64, "after SetWindowPos");

            unsafe {
                // RDW_UPDATENOW flushes the WS_EX_COMPOSITED backbuffer
                // synchronously — same fix as WM_SIZE and WM_DISPLAYCHANGE.
                RedrawWindow(hwnd, None, None,
                    RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_ERASE | RDW_FRAME | RDW_UPDATENOW);
            }

            log_dr!(2u64, "after RedrawWindow");

            // If a reposition poll is in flight, short-circuit it now.
            // The taskbar rect is stable at this point — no need to keep waiting.
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                if st.dimmer.enabled && st.dimmer.is_repositioning {
                    unsafe {
                        KillTimer(hwnd, TIMER_OVERLAY_REPOSITION);
                        st.dimmer.reposition_overlays_now();
                        st.dimmer.is_repositioning = false;
                    }
                }
            }

            LRESULT(0)
        }

        // 2. Resolution Change: the display has actually switched.
        //    WM_DPICHANGED (if any) was sent via SendMessage and is fully
        //    processed before this arrives, so the main window is already
        //    at its new size. Reposition overlays immediately — no timer needed.
        WM_DISPLAYCHANGE => {
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                // Log immediately — delta from WM_DPICHANGED shows how long the
                // main window resize took before the display actually switched.
                if is_debug_mode() {
                    let lp = lparam.0 as u32;
                    let width  = lp & 0xFFFF;
                    let height = (lp >> 16) & 0xFFFF;
                    let bpp    = wparam.0 as u32;
                    let payload = ((width as u64) << 32) | (height as u64);
                    let detail = format!(
                        "[DC] WM_DISPLAYCHANGE  {}x{}  {}bpp",
                        width, height, bpp,
                    );
                    if let Some(Ok(mut log)) = zorder_log().map(|m| m.lock()) {
                        log.push(unsafe { windows::Win32::System::SystemInformation::GetTickCount64() },
                            ZLogKind::DisplayChange, payload, detail);
                    }
                }

                // Hide the overlays immediately and start the polling loop.
                //
                // We used to call reposition_overlays_now() here, but that
                // moves the overlay to the new coords before the shell taskbar
                // has physically moved — causing a ~2 s window where the overlay
                // is covering empty space and the taskbar is uncovered.
                //
                // start_reposition_overlays() hides the overlays instantly (no
                // visual artifact) and arms TIMER_OVERLAY_REPOSITION to poll
                // every 100 ms.  tick_reposition() keeps firing until two
                // consecutive samples return the same taskbar rect, at which
                // point the taskbar has settled and we show the overlay there.
                if st.dimmer.enabled {
                    st.dimmer.start_reposition_overlays();
                }
                // Same fix as WM_SIZE: RDW_UPDATENOW forces the composited
                // backbuffer to flush synchronously.  Without it, WS_EX_COMPOSITED
                // leaves the stale frame visible until the next input event.
                RedrawWindow(hwnd, None, None,
                    RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_ERASE | RDW_FRAME | RDW_UPDATENOW);
            }
            LRESULT(0)
        }

        WM_SHOWWINDOW if wparam.0 != 0 => {
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                if !st.layout_initialized {
                        app_layout::apply(st, hwnd);
                        // Install zoom icons — deferred from WM_CREATE to avoid
                        // borrow conflict while create_state still holds a mutable ref.
                        crate::ui_drawing::install_bitmap_static(
                            st.crush.h_lbl_zoom_out_icon, st.nav_icons.zoom_out);
                        crate::ui_drawing::install_bitmap_static(
                            st.crush.h_lbl_zoom_icon, st.nav_icons.zoom);
                    }
                let (pw, ph) = client_size_of(st.crush.h_hdr_panel);
                st.crush.hdr_panel.init_d3d(st.crush.h_hdr_panel, pw as u32, ph as u32);
                st.crush.hdr_panel.update(get_slider_val(st.crush.h_sld_black));
                st.crush.update_sl_hint();
                st.crush.update_range_label();
                st.crush.populate_refresh_rates(&mut st.ini);
                // Initialise slider animation interval from the real display Hz
                // now that the window is visible and the display is known.
                update_slider_anim_interval(get_current_hz());
                if !st.layout_initialized {
                    st.layout_initialized = true;
                }
                maybe_restore_gamma(st, hwnd);
                let args: Vec<String> = std::env::args().collect();
                if args.iter().any(|a| a.eq_ignore_ascii_case("--minimized")) {
                    st.crush.hdr_panel.suspend_d3d();
                    ShowWindow(hwnd, SW_HIDE);
                }
                let is_debug = args.iter().any(|a| a.eq_ignore_ascii_case("--debug"));
                DEBUG_MODE.store(is_debug, Ordering::Relaxed);
                if !is_debug_mode() {
                    ShowWindow(st.h_nav_btn[3], SW_HIDE);
                    KillTimer(hwnd, TIMER_DEBUG_REFRESH);
                }
            }
            LRESULT(0)
        }

        WM_SIZE => {
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                app_layout::apply(st, hwnd);
                let (pw, ph) = client_size_of(st.crush.h_hdr_panel);
                st.crush.hdr_panel.resize(pw as u32, ph as u32);
                // Force a full synchronous repaint.  RDW_UPDATENOW is required
                // with WS_EX_COMPOSITED: without it the composited backbuffer is
                // merely queued for invalidation, and the stale frame stays
                // visible until the next input event or focus change.
                RedrawWindow(hwnd, None, None,
                    RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_ERASE | RDW_FRAME | RDW_UPDATENOW);
            }
            LRESULT(0)
        }

        WM_DWMCOLORIZATIONCOLORCHANGED => {
            RedrawWindow(hwnd, None, None,
                RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_ERASE | RDW_UPDATENOW);
            LRESULT(0)
        }

        WM_TIMER => {
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                let ui_visible = IsWindowVisible(hwnd).as_bool();

                match wparam.0 {
                    TIMER_HDR => {
                        if ui_visible {
                            // Only trigger UI repaints and updates if the HDR status ACTUALLY changed
                            if st.crush.hdr_panel.refresh_hdr_status() {
                                st.crush.update_sl_hint();
                                st.crush.update_range_label();
                                if !st.crush.previewing { apply_ramp(st, hwnd); }
                                
                                // Force the toggle button to repaint with the updated state
                                InvalidateRect(st.h_btn_hdr_toggle, None, true);
                                UpdateWindow(st.h_btn_hdr_toggle);
                            }
                        }
                        // Refresh NVIDIA CAM cache so apply_ramp doesn't do registry I/O.
                        st.nvidia_cam_enabled = CrushTab::is_nvidia_cam_enabled();
                        maybe_restore_gamma(st, hwnd);
                        
                        // Note: We also want to reset the timer to 2000ms just in case 
                        // the user clicked the toggle button (which temporarily sets it to 300ms)
                        SetTimer(hwnd, TIMER_HDR, 2000, None);
                    }
                    TIMER_RENDER if ui_visible => { st.crush.hdr_panel.render_tick(); }
                    TIMER_OVERLAY_FADE  => { st.dimmer.tick_fade(); }
                    TIMER_DEBUG_REFRESH => {
                        // Timer only runs while tab 3 is active (armed/killed in
                        // show_tab), so active_tab == 3 is always true here.
                        // Still guard on ui_visible so we skip the refresh while
                        // the window is hidden to the tray.
                        if ui_visible {
                            st.debug.refresh(&st.dimmer);
                        }
                    }
                    TIMER_SCROLL_REFRESH => {
                        KillTimer(hwnd, TIMER_SCROLL_REFRESH);
                        SendMessageW(st.crush.h_ddl_refresh, CB_SETTOPINDEX,
                            WPARAM(0), LPARAM(0));
                    }
                    TIMER_STATUS_CLEAR => {
                        KillTimer(hwnd, TIMER_STATUS_CLEAR);
                        set_status(st, "", C_ACCENT);
                    }
                    TIMER_FULLSCREEN_RECHECK => {
                        // One-shot: fire once ~1 s after a foreground event to catch
                        // late-sizing games, then disarm.
                        KillTimer(hwnd, TIMER_FULLSCREEN_RECHECK);
                        st.dimmer.on_fullscreen_recheck();
                    }
                    TIMER_OVERLAY_REPOSITION => {
                        st.dimmer.tick_reposition();
                    }
                    TIMER_RAMP_APPLY => {
                        // Debounce: black-level slider moved recently.
                        // Apply the ramp once at ~20 Hz instead of on every drag tick.
                        KillTimer(hwnd, TIMER_RAMP_APPLY);
                        if st.ramp_dirty {
                            st.ramp_dirty = false;
                            apply_ramp(st, hwnd);
                        }
                    }
                    _ => {}
                }
            }
            LRESULT(0)
        }

        WM_COMMAND => {
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                let id     = (wparam.0 & 0xFFFF) as usize;
                let notify = ((wparam.0 >> 16) & 0xFFFF) as u32;
                let ctrl   = HWND(lparam.0 as *mut _);
                on_command(st, hwnd, id, notify, ctrl);
            }
            LRESULT(0)
        }

        WM_HSCROLL | WM_VSCROLL => {
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                let ctrl        = HWND(lparam.0 as *mut _);
                // TB_THUMBPOSITION (0x4) or TB_ENDTRACK (0x8) indicate a final
                // thumb release/commit on the trackbar.  TB_THUMBPOSITION occurs on
                // direct thumb positioning as well as keyboard/drag commit.
                let notify_code = (wparam.0 & 0xFFFF) as u32;
                let is_end_track = matches!(notify_code, 0x4 | 0x8); // TB_THUMBPOSITION | TB_ENDTRACK

                if ctrl == st.crush.h_sld_black {
                    if is_end_track {
                        // Drag finished — full commit: ramp + INI + dropdown.
                        KillTimer(hwnd, TIMER_RAMP_APPLY);
                        st.ramp_dirty = false;
                        st.crush.on_black_slider_changed(&mut st.ini);
                        apply_ramp(st, hwnd);
                    } else {
                        // Mid-drag — apply gamma immediately on every tick,
                        // then update the label/HDR panel visuals.
                        st.crush.on_black_slider_visual();
                        apply_ramp(st, hwnd);
                        st.ramp_dirty = false;
                    }
                    InvalidateRect(ctrl, None, false);
                } else if ctrl == st.crush.h_sld_squares {
                    st.crush.on_squares_changed();
                    InvalidateRect(ctrl, None, false);
                } else if ctrl == st.dimmer.h_sld_taskbar_dim {
                    // Always update the overlay alpha and label (visual-only).
                    let msg = st.dimmer.update_dim_visuals(hwnd);
                    set_status(st, &msg, C_ACCENT);
                    // Only persist to disk when the drag finishes.
                    if is_end_track {
                        st.dimmer.save_dim_slider(&mut st.ini);
                    }
                    InvalidateRect(ctrl, None, false);
                } else if ctrl == st.dimmer.h_sld_fade_in {
                    st.dimmer.update_fade_visuals(true);
                    if is_end_track {
                        st.dimmer.save_fade_slider(true, &mut st.ini);
                    }
                    InvalidateRect(ctrl, None, false);
                } else if ctrl == st.dimmer.h_sld_fade_out {
                    st.dimmer.update_fade_visuals(false);
                    if is_end_track {
                        st.dimmer.save_fade_slider(false, &mut st.ini);
                    }
                    InvalidateRect(ctrl, None, false);
                }
            }
            LRESULT(0)
        }

        WM_DRAWITEM => {
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                let di = &*(lparam.0 as *const DRAWITEMSTRUCT);
                if di.hwndItem == st.crush.h_ddl_refresh {
                    ui_drawing::draw_combo_item(di);
                } else if di.hwndItem == st.h_nav_btn[0] {
                    draw_nav_item(di, st.active_tab == 0,
                        GetFocus() == st.h_nav_btn[0],
                        !GetPropW(st.h_nav_btn[0], NAV_BTN_HOVER_PROP).0.is_null(),
                        st.h_app_icon, "",
                        st.nav_icons.crush);
                } else if di.hwndItem == st.h_nav_btn[1] {
                    draw_nav_item(di, st.active_tab == 1,
                        GetFocus() == st.h_nav_btn[1],
                        !GetPropW(st.h_nav_btn[1], NAV_BTN_HOVER_PROP).0.is_null(),
                        HICON(ptr::null_mut()), "⊞",
                        st.nav_icons.dimmer);
                } else if di.hwndItem == st.h_nav_btn[2] {
                    draw_nav_item(di, st.active_tab == 2,
                        GetFocus() == st.h_nav_btn[2],
                        !GetPropW(st.h_nav_btn[2], NAV_BTN_HOVER_PROP).0.is_null(),
                        HICON(ptr::null_mut()), "⌨",
                        st.nav_icons.hotkeys);
                } else if di.hwndItem == st.h_nav_btn[3] {
                    draw_nav_item(di, st.active_tab == 3,
                        GetFocus() == st.h_nav_btn[3],
                        !GetPropW(st.h_nav_btn[3], NAV_BTN_HOVER_PROP).0.is_null(),
                        HICON(ptr::null_mut()), "🐛",
                        st.nav_icons.debug);
                } else if di.hwndItem == st.h_nav_btn[4] {
                    draw_nav_item(di, st.active_tab == 4,
                        GetFocus() == st.h_nav_btn[4],
                        !GetPropW(st.h_nav_btn[4], NAV_BTN_HOVER_PROP).0.is_null(),
                        HICON(ptr::null_mut()), "ℹ",
                        st.nav_icons.about);
                } else if di.hwndItem == st.debug.h_chk_suppress_fs {
                    draw_dark_button_full(di,
                        st.debug.h_chk_suppress_fs, HWND(ptr::null_mut()),
                        HWND(ptr::null_mut()), HWND(ptr::null_mut()),
                        st.dimmer.suppress_fs_enabled, false, false, false);
                } else if di.hwndItem == st.debug.h_chk_suppress_ah {
                    draw_dark_button_full(di,
                        st.debug.h_chk_suppress_ah, HWND(ptr::null_mut()),
                        HWND(ptr::null_mut()), HWND(ptr::null_mut()),
                        st.dimmer.suppress_ah_enabled, false, false, false);
                } else if di.hwndItem == st.h_btn_hdr_toggle {
                    draw_hdr_toggle_switch(di, st.crush.hdr_panel.hdr_active, None);
                } else if st.hotkeys.is_pill(di.hwndItem) {
                    draw_hotkey_pill(di);
                } else {
                    draw_dark_button_full(
                        di,
                        HWND(ptr::null_mut()),
                        st.h_chk_startup,
                        st.dimmer.h_chk_taskbar_dim,
                        st.crush.h_btn_toggle,
                        false,
                        st.chk_startup_state,
                        st.dimmer.enabled,
                        st.crush.btn_toggle_active,
                    );
                }
            }
            LRESULT(1)
        }

        WM_CTLCOLORSTATIC => {
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                let hdc  = HDC(wparam.0 as *mut _);
                let ctrl = HWND(lparam.0 as *mut _);
                let is_sep = st.h_sep_h.contains(&ctrl) || ctrl == st.h_sep_vert;
                if is_sep {
                    SetBkColor(hdc, C_SEP);
                    return LRESULT(st.sep_brush.0 as isize);
                }
                if ctrl == st.h_lbl_status {
                    SetBkColor(hdc, C_BG);
                    SetTextColor(hdc, st.status_color);
                    return LRESULT(st.bg_brush.0 as isize);
                }
                if ctrl == st.h_lbl_error {
                    SetBkColor(hdc, C_BG);
                    SetTextColor(hdc, C_ERR);
                    return LRESULT(st.bg_brush.0 as isize);
                }
                if ctrl == st.about.h_lbl_link {
                    SetBkColor(hdc, C_BG);
                    SetTextColor(hdc, C_ACCENT);
                    return LRESULT(st.bg_brush.0 as isize);
                }
                if ctrl == st.crush.h_lbl_gamma_warn {
                    SetBkColor(hdc, C_BG);
                    SetTextColor(hdc, COLORREF(0x00447799));
                    return LRESULT(st.bg_brush.0 as isize);
                }
                if ctrl == st.crush.h_lbl_hdr_note {
                    SetBkColor(hdc, C_BG);
                    SetTextColor(hdc, st.crush.hdr_note_color);
                    return LRESULT(st.bg_brush.0 as isize);
                }
                // All section headings use C_FG uniformly across every tab.
                // h_lbl_hz_profile ("Each refresh rate...") gets its own darker tone.
                if ctrl == st.crush.h_lbl_hz_profile {
                    SetBkColor(hdc, C_BG);
                    SetTextColor(hdc, COLORREF(0x00666666));
                    return LRESULT(st.bg_brush.0 as isize);
                }
                let is_sub =
                    ctrl == st.crush.h_lbl_sub1            ||
                    ctrl == st.crush.h_lbl_sub2            ||
                    ctrl == st.crush.h_lbl_sl_hint         ||
                    ctrl == st.dimmer.h_lbl_dim_sub        ||
                    ctrl == st.dimmer.h_lbl_fade_in_title  ||
                    ctrl == st.dimmer.h_lbl_fade_out_title ||
                    ctrl == st.debug.h_chk_suppress_fs     ||
                    ctrl == st.debug.h_chk_suppress_ah     ||
                    ctrl == st.hotkeys.h_lbl_desc;
                let is_hk_row_lbl = st.hotkeys.rows.iter().any(|r| r.h_lbl == ctrl);
                SetBkColor(hdc, C_BG);
                SetTextColor(hdc, if is_sub || is_hk_row_lbl { C_LABEL } else { C_FG });
                return LRESULT(st.bg_brush.0 as isize);
            }
            LRESULT(0)
        }

        WM_CTLCOLOREDIT | WM_CTLCOLORBTN => {
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                let hdc = HDC(wparam.0 as *mut _);
                SetBkColor(hdc, C_BG);
                SetTextColor(hdc, C_FG);
                return LRESULT(st.bg_brush.0 as isize);
            }
            LRESULT(0)
        }

        WM_CTLCOLORLISTBOX => {
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                let hdc = HDC(wparam.0 as *mut _);
                SetBkColor(hdc, C_BG3);
                SetTextColor(hdc, C_FG);
                return LRESULT(st.bg3_brush.0 as isize);
            }
            LRESULT(0)
        }

        WM_ERASEBKGND => {
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                let hdc = HDC(wparam.0 as *mut _);
                let mut rc = RECT::default();
                GetClientRect(hwnd, &mut rc);
                FillRect(hdc, &rc, st.bg_brush);
                return LRESULT(1);
            }
            LRESULT(0)
        }

        WM_COMPARE_START => {
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                if !st.crush.previewing {
                    st.crush.previewing         = true;
                    st.crush.btn_toggle_active  = true;
                    st.crush.apply_linear_ramp();
                    InvalidateRect(st.crush.h_btn_toggle, None, false);
                }
            }
            LRESULT(0)
        }

        WM_COMPARE_END => {
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                if st.crush.previewing {
                    uninstall_compare_hook();
                    st.crush.previewing        = false;
                    st.crush.btn_toggle_active = false;
                    apply_ramp(st, hwnd);
                    InvalidateRect(st.crush.h_btn_toggle, None, false);
                }
            }
            LRESULT(0)
        }

        WM_HOTKEY => {
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                match wparam.0 {
                    HK_TOGGLE_DIM => {
                        let (msg, ok) = st.dimmer.on_checkbox_toggled(hwnd, &mut st.ini);
                        show_tab(st, hwnd);
                        if !msg.is_empty() {
                            set_status(st, msg, if ok { C_ACCENT } else { C_WARN });
                        }
                    }
                    HK_TOGGLE_CRUSH => {
                        if st.crush.previewing {
                            SendMessageW(hwnd, WM_COMPARE_END, WPARAM(0), LPARAM(0));
                        } else {
                            SendMessageW(hwnd, WM_COMPARE_START, WPARAM(0), LPARAM(0));
                        }
                    }
                    HK_HOLD_COMPARE => {
                        if !st.crush.previewing {
                            // Resolve the bound key so the keyboard hook knows
                            // which key-up to listen for, then arm it.
                            // Mouse buttons never generate a key-up, so skip
                            // the keyboard hook for them — the compare stays
                            // active until the button is pressed again (toggle).
                            let s = st.ini.read("Hotkeys", "HoldCompare", "None");
                            let vk = parse_hotkey(&s).map(|(_, v)| v).unwrap_or(0);
                            if vk != 0 && !crate::tab_hotkeys::is_mouse_sentinel(vk) {
                                install_compare_hook(hwnd, vk);
                            }
                            SendMessageW(hwnd, WM_COMPARE_START, WPARAM(0), LPARAM(0));
                        } else {
                            // For mouse-button bindings there is no key-up event,
                            // so a second press acts as the release.
                            let s = st.ini.read("Hotkeys", "HoldCompare", "None");
                            let vk = parse_hotkey(&s).map(|(_, v)| v).unwrap_or(0);
                            if vk != 0 && crate::tab_hotkeys::is_mouse_sentinel(vk) {
                                SendMessageW(hwnd, WM_COMPARE_END, WPARAM(0), LPARAM(0));
                            }
                        }
                    }
                    HK_DECREASE => {
                        let v = get_slider_val(st.crush.h_sld_black);
                        let new_v = (v - 1).max(0);
                        SendMessageW(st.crush.h_sld_black, TBM_SETPOS,
                            WPARAM(1), LPARAM(new_v as isize));
                        st.crush.on_black_slider_changed(&mut st.ini);
                        apply_ramp(st, hwnd);
                        InvalidateRect(st.crush.h_sld_black, None, false);
                    }
                    HK_INCREASE => {
                        let v = get_slider_val(st.crush.h_sld_black);
                        let new_v = (v + 1).min(MAX_BLACK);
                        SendMessageW(st.crush.h_sld_black, TBM_SETPOS,
                            WPARAM(1), LPARAM(new_v as isize));
                        st.crush.on_black_slider_changed(&mut st.ini);
                        apply_ramp(st, hwnd);
                        InvalidateRect(st.crush.h_sld_black, None, false);
                    }
                    HK_DIM_DECREASE => {
                        let v = get_slider_val(st.dimmer.h_sld_taskbar_dim);
                        let new_v = (v - 5).max(0);
                        SendMessageW(st.dimmer.h_sld_taskbar_dim, TBM_SETPOS,
                            WPARAM(1), LPARAM(new_v as isize));
                        let msg = st.dimmer.update_dim_visuals(hwnd);
                        st.dimmer.save_dim_slider(&mut st.ini);
                        set_status(st, &msg, C_ACCENT);
                        InvalidateRect(st.dimmer.h_sld_taskbar_dim, None, false);
                    }
                    HK_DIM_INCREASE => {
                        let v = get_slider_val(st.dimmer.h_sld_taskbar_dim);
                        let new_v = (v + 5).min(100);
                        SendMessageW(st.dimmer.h_sld_taskbar_dim, TBM_SETPOS,
                            WPARAM(1), LPARAM(new_v as isize));
                        let msg = st.dimmer.update_dim_visuals(hwnd);
                        st.dimmer.save_dim_slider(&mut st.ini);
                        set_status(st, &msg, C_ACCENT);
                        InvalidateRect(st.dimmer.h_sld_taskbar_dim, None, false);
                    }
                    x if x == HK_DEBUG_FORCE_RAISE as usize && is_debug_mode() => {
                        st.dimmer.force_raise_overlays();
                        set_status(st, "Overlays force-raised (debug)", C_ACCENT);
                    }
                    _ => {}
                }
            }
            LRESULT(0)
        }

        WM_POWERBROADCAST => {
            if wparam.0 == PBT_APMRESUMEAUTOMATIC as usize {
                if let Some(st) = borrow_state::<AppState>(hwnd) {
                    if !st.crush.previewing { apply_ramp(st, hwnd); }
                }
            }
            LRESULT(0)
        }

        WM_WTSSESSION_CHANGE => {
            if wparam.0 == WTS_SESSION_UNLOCK as usize {
                if let Some(st) = borrow_state::<AppState>(hwnd) {
                    if !st.crush.previewing { apply_ramp(st, hwnd); }
                }
            }
            LRESULT(0)
        }


        WM_TRAY_CALLBACK => {
            match lparam.0 as u32 {
                x if x == WM_LBUTTONDBLCLK => {
                    ShowWindow(hwnd, SW_SHOW);
                    SetForegroundWindow(hwnd);
                }
                x if x == WM_RBUTTONUP => {
                    if let Some(st) = borrow_state::<AppState>(hwnd) {
                        let mut pt = POINT::default();
                        GetCursorPos(&mut pt);
                        SetForegroundWindow(hwnd);
                        TrackPopupMenu(
                            st.tray_menu,
                            TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
                            pt.x, pt.y, 0, hwnd, None,
                        );
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }

        WM_ENTERSIZEMOVE => {
            // Kill the debug refresh timer for the duration of the resize drag.
            // The timer fires z-order walks (GetWindow in a loop) every 500 ms;
            // doing that while the window manager holds its internal resize lock
            // causes severe system-wide slowdown.  WM_EXITSIZEMOVE re-arms it.
            KillTimer(hwnd, TIMER_DEBUG_REFRESH);
            LRESULT(0)
        }

        WM_EXITSIZEMOVE => {
            // Resize finished — restart the debug timer if we're still on tab 3.
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                if is_debug_mode() && st.active_tab == 3 {
                    SetTimer(hwnd, TIMER_DEBUG_REFRESH, 500, None);
                }
            }
            LRESULT(0)
        }

        WM_GETMINMAXINFO => {
            let dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd).max(96) as f32;
            let s   = |px: i32| -> i32 { (px as f32 * dpi / 96.0).round() as i32 };
            let mut min_rect = RECT {
                left: 0,
                top: 0,
                right: s(MIN_WIN_W),
                bottom: s(MIN_WIN_H),
            };
            let _ = AdjustWindowRectEx(
                &mut min_rect,
                WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
                false,
                WS_EX_APPWINDOW | WS_EX_COMPOSITED,
            );
            let mmi = &mut *(lparam.0 as *mut MINMAXINFO);
            mmi.ptMinTrackSize.x = min_rect.right - min_rect.left;
            mmi.ptMinTrackSize.y = min_rect.bottom - min_rect.top;
            LRESULT(0)
        }

        WM_CLOSE => {
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                st.crush.hdr_panel.suspend_d3d();
            }
            ShowWindow(hwnd, SW_HIDE);
            LRESULT(0)
        }

        WM_DESTROY => {
            uninstall_compare_hook();
            uninstall_mouse_hook();
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                let _ = st.zorder_winevent_hooks.take();
                st.nav_icons.destroy();
                st.tab_header_icons.destroy();
                tray::remove_tray_icon(hwnd, st.tray_menu, &mut st.tray_added);
                KillTimer(hwnd, TIMER_HDR);
                KillTimer(hwnd, TIMER_RENDER);
                KillTimer(hwnd, TIMER_OVERLAY_FADE);
                KillTimer(hwnd, TIMER_OVERLAY_REPOSITION);
                KillTimer(hwnd, TIMER_DEBUG_REFRESH);
                KillTimer(hwnd, TIMER_STATUS_CLEAR);
                KillTimer(hwnd, TIMER_FULLSCREEN_RECHECK);
                KillTimer(hwnd, TIMER_RAMP_APPLY);
                WTSUnRegisterSessionNotification(hwnd);
                gamma_ramp::reset_display_ramp();
                crate::tab_debug::uninstall_mouse_hook();

                // ── GDI cleanup ───────────────────────────────────────────────
                // HBRUSH / HFONT are raw kernel handles.  Dropping the Rust
                // wrapper does NOT call DeleteObject; we must do it explicitly.
                // Failure to delete these leaks GDI objects on every close/restart.
                DeleteObject(HGDIOBJ(st.bg_brush.0));
                DeleteObject(HGDIOBJ(st.bg3_brush.0));
                DeleteObject(HGDIOBJ(st.sep_brush.0));
                DeleteObject(HGDIOBJ(st._font_normal.0));
                DeleteObject(HGDIOBJ(st._font_title.0));
                DeleteObject(HGDIOBJ(st._font_bold_val.0));
            }
            for id in [HK_TOGGLE_DIM, HK_TOGGLE_CRUSH, HK_HOLD_COMPARE, HK_DECREASE, HK_INCREASE, HK_DIM_DECREASE, HK_DIM_INCREASE] {
                UnregisterHotKey(hwnd, id as i32);
            }
            detach_state::<AppState>(hwnd);
            PostQuitMessage(0);
            LRESULT(0)
        }

        // Posted by the WinEvent hook proc (zorder_winevent_proc) running on a
        // system thread.  We handle it here on the UI thread where DimmerTab is
        // accessible without a lock.  Without this arm the fullscreen-suppression
        // logic silently never ran.
        x if x == crate::tab_dimmer::WM_APP_FULLSCREEN_CHECK => {
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                let fg_hwnd = HWND(wparam.0 as *mut _);
                st.dimmer.on_fullscreen_check(fg_hwnd);
                // Arm a one-shot recheck ~1 s from now to catch games that resize
                // *after* EVENT_SYSTEM_FOREGROUND fires (late-sizing).  This
                // replaces the old continuous 500 ms polling of is_fullscreen_on_monitor
                // inside tick_fade.
                SetTimer(hwnd, TIMER_FULLSCREEN_RECHECK, 1000, None);
            }
            LRESULT(0)
        }

        // A known shell panel (Quick Settings, Action Center, …) just appeared
        // and likely displaced our TOPMOST overlays.  Re-raise immediately on
        // the UI thread — no timer, no polling, pure event-driven.
        x if x == crate::tab_dimmer::WM_APP_RAISE_OVERLAYS => {
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                st.dimmer.force_raise_overlays();
            }
            LRESULT(0)
        }

        WM_NOTIFY => DefWindowProcW(hwnd, msg, wparam, lparam),

        WM_MOUSEWHEEL => {
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                let mut pt = POINT {
                    x: (lparam.0 & 0xFFFF) as i16 as i32,
                    y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
                };
                let mut panel_rc = RECT::default();
                GetWindowRect(st.crush.h_hdr_panel, &mut panel_rc);
                if pt.x >= panel_rc.left && pt.x < panel_rc.right
                    && pt.y >= panel_rc.top  && pt.y < panel_rc.bottom
                {
                    let delta = ((wparam.0 as i32) >> 16) as i16;
                    let step  = if delta > 0 { 1i32 } else { -1i32 };
                    let cur   = get_slider_val(st.crush.h_sld_squares);
                    let next  = (cur + step).clamp(9, 24);
                    if next != cur {
                        SendMessageW(st.crush.h_sld_squares, TBM_SETPOS,
                            WPARAM(1), LPARAM(next as isize));
                        InvalidateRect(st.crush.h_sld_squares, None, false);
                        st.crush.on_squares_changed();
                    }
                    return LRESULT(0);
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }

        WM_MOUSE_CLICK_LOG => {
            // The WH_MOUSE_LL hook is only active while the debug tab is shown,
            // so this message only arrives in --debug mode with tab 3 open.
            // Resolution (WindowFromPoint, class name, title) happens inside
            // log_mouse_click and the result is pushed straight into the unified
            // ZOrderLog — no mutable AppState access needed here.
            let btn = wparam.0;
            let raw = lparam.0 as isize as i64;
            let x   = (raw >> 32) as i32;
            let y   = (raw & 0xFFFF_FFFF) as i32;
            crate::tab_debug::log_mouse_click(btn, POINT { x, y });
            LRESULT(0)
        }

        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}



// ── State construction ────────────────────────────────────────────────────────

unsafe fn create_state(hwnd: HWND, ini_path: PathBuf) -> AppState {
    let hinstance: HINSTANCE = GetModuleHandleW(None).unwrap().into();
    let dpi = GetDpiForWindow(hwnd).max(96);

    let bg_brush  = CreateSolidBrush(C_BG);
    let bg3_brush = CreateSolidBrush(C_BG3);
    let sep_brush = CreateSolidBrush(C_SEP);

    let _font_normal   = make_font(w!("Segoe UI"),  10, dpi, false);
    let _font_title    = make_font(w!("Segoe UI"),  16, dpi, true);
    let _font_bold_val = make_font(w!("Consolas"),  14, dpi, true);

    // ── Shared controls ───────────────────────────────────────────────────────
    let cb = ControlBuilder { parent: hwnd, hinstance, dpi, font: _font_normal };

    let h_chk_startup  = cb.checkbox(w!("Launch with Windows"), IDC_CHK_STARTUP);
    SetWindowSubclass(h_chk_startup, Some(hdr_toggle_subclass_proc), 2, 0);
    let h_btn_quit     = cb.button(w!("Exit"),     IDC_BTN_QUIT);
    let h_btn_minimize    = cb.button(w!("Minimize"),   IDC_BTN_MINIMIZE);
    let h_btn_hdr_toggle  = cb.button(w!("HDR Toggle"), IDC_BTN_HDR_TOGGLE);
    SetWindowSubclass(h_btn_hdr_toggle, Some(hdr_toggle_subclass_proc), 1, 0);
    let h_lbl_status   = cb.static_text(w!(""), SS_CENTER | SS_CENTERIMAGE);
    let h_lbl_error    = cb.static_text(w!(""), SS_CENTER | SS_CENTERIMAGE);

    // ── Separators ────────────────────────────────────────────────────────────
    let sep_style = WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_BLACKRECT);
    let h_sep_vert = CreateWindowExW(WS_EX_LEFT, w!("STATIC"), w!(""),
        sep_style, 0,0,1,1, hwnd, HMENU(ptr::null_mut()), hinstance, None,
    ).unwrap_or_default();
    let mut h_sep_h = [HWND::default(); 4];
    for sep in h_sep_h.iter_mut() {
        *sep = CreateWindowExW(WS_EX_LEFT, w!("STATIC"), w!(""),
            sep_style, 0,0,1,1, hwnd, HMENU(ptr::null_mut()), hinstance, None,
        ).unwrap_or_default();
    }

    // ── Navigation buttons ────────────────────────────────────────────────────
    let nav_icon_px = (16u32 * dpi / 96).max(16);
    let h_app_icon = LoadImageW(
        hinstance, w!("MAINICON"), IMAGE_ICON,
        nav_icon_px as i32, nav_icon_px as i32, LR_DEFAULTCOLOR,
    ).map(|h| HICON(h.0)).unwrap_or_default();

    let h_nav_btn_0 = cb.button(w!("Black Crush Tweak"), IDC_NAV_BTN_0);
    let h_nav_btn_1 = cb.button(w!("Taskbar Dimmer"),    IDC_NAV_BTN_1);
    let h_nav_btn_2 = cb.button(w!("Hotkeys"),           IDC_NAV_BTN_2);
    let h_nav_btn_3 = cb.button(w!("Debug"),             IDC_NAV_BTN_3);
    let h_nav_btn_4 = cb.button(w!("About"),             IDC_NAV_BTN_4);
    let h_nav_btn = [h_nav_btn_0, h_nav_btn_1, h_nav_btn_2, h_nav_btn_3, h_nav_btn_4];
    for &h in &h_nav_btn {
        SetWindowSubclass(h, Some(nav_btn_subclass_proc), 1, 0);
    }

    let tray_menu = tray::build_tray_menu();

    let mut ini = ProfileManager::new(&ini_path);

    // ── Construct tab sub-states ──────────────────────────────────────────────
    // h_sep_h[0..=2] visually belong to the crush tab; [3] is shared/bottom.
    let crush = CrushTab::new(
        hwnd, hinstance, dpi,
        _font_normal, _font_title, _font_bold_val,
        &mut ini,
        &h_sep_h,
    );

    let dimmer = DimmerTab::new(
        hwnd, hinstance, dpi,
        _font_normal, _font_title, _font_bold_val,
        &mut ini,
        hwnd,
    );
    // Restrict clicks to the pill area only (left-aligned geometry).
    SetWindowSubclass(dimmer.h_chk_taskbar_dim, Some(dimmer_toggle_subclass_proc), 3, 0);

  
let hotkeys = HotkeysTab::new(
    hwnd, hinstance, dpi,
    _font_normal, _font_title,
    &mut ini,
);

    let debug = DebugTab::new(hwnd, hinstance, dpi, _font_normal, _font_title);
    let about = AboutTab::new(hwnd, hinstance, dpi, _font_normal, _font_title);

    let nav_icons         = crate::nav_icons::NavIcons::load(dpi);
    let tab_header_icons  = crate::nav_icons::TabHeaderIcons::load(dpi);

    let mut state = AppState {
        hwnd,
        crush, dimmer, hotkeys, debug, about,
        h_chk_startup, h_btn_quit, h_btn_minimize, h_btn_hdr_toggle,
        h_lbl_status, h_lbl_error, h_sep_vert, h_sep_h, h_nav_btn,
        h_app_icon, active_tab: 0,
        nav_icons,
        tab_header_icons,
        bg_brush, bg3_brush, sep_brush,
        _font_normal, _font_title, _font_bold_val,
        tray_menu, tray_added: false,
        ini,
        status_color: C_ACCENT,
        chk_startup_state: false,
        layout_initialized: false,
        zorder_winevent_hooks: None,
        mouse_hotkeys: [0u32; 8],
        nvidia_cam_enabled: unsafe { CrushTab::is_nvidia_cam_enabled() },
        ramp_dirty: false,
    };

    state.zorder_winevent_hooks = Some(unsafe { crate::tab_dimmer::install_zorder_winevent_hooks() });

    // ── Startup checkbox ──────────────────────────────────────────────────────
    state.chk_startup_state = startup::startup_registry_exists();
    if state.chk_startup_state {
        SendMessageW(state.h_chk_startup, BM_SETCHECK, WPARAM(1), LPARAM(0));
    }

    // ── Hz profile seed / restore ─────────────────────────────────────────────
    let hz = get_current_hz();
    crate::tab_dimmer::set_fade_interval_from_hz(hz);
    let sec = hz_section(hz);
    if state.ini.read(&sec, "Black", "__x__") == "__x__" {
        let fallback = state.ini.read_int("_state", "Black", DEFAULT_BLACK)
            .clamp(0, MAX_BLACK);
        state.ini.write_int(&sec, "Black", fallback);
    }
    if let Some((v, _status)) =
        state.crush.try_auto_load_profile_for_hz(hz, &mut state.ini)
    {
        // Profile silently applied on startup — no status message.
    } else {
        let saved_v = state.ini.read_int(&sec, "Black", DEFAULT_BLACK)
            .clamp(0, MAX_BLACK);
        SendMessageW(state.crush.h_sld_black, TBM_SETPOS,
            WPARAM(1), LPARAM(saved_v as isize));
        InvalidateRect(state.crush.h_sld_black, None, true);
        let v_text = if saved_v == 0 { "OFF".to_string() } else { format!("{saved_v}") };
        set_text(state.crush.h_lbl_black_val, &v_text);
        // apply_ramp is NOT called here — the unconditional call below covers
        // both the auto-load path and this manual-restore path.  The old code
        // called it here AND below, applying SetDeviceGammaRamp twice needlessly.
        state.crush.hdr_panel.update(saved_v);
    }

    show_tab(&mut state, hwnd);

    // Attach PNG icon painter to each tab's title label.
    ui_drawing::subclass_tab_header(state.crush.h_lbl_title,   state.tab_header_icons.crush);
    ui_drawing::subclass_tab_header(state.dimmer.h_lbl_dim_title, state.tab_header_icons.dimmer);
    ui_drawing::subclass_tab_header(state.hotkeys.h_lbl_title, state.tab_header_icons.hotkeys);
    ui_drawing::subclass_tab_header(state.debug.h_lbl_title,   state.tab_header_icons.debug);
    ui_drawing::subclass_tab_header(state.about.h_lbl_title,  state.tab_header_icons.about);
    apply_ramp(&mut state, hwnd);

    tray::add_tray_icon(hwnd, hinstance, &mut state.tray_added);
    register_hotkeys(&state.ini, &mut state.mouse_hotkeys, hwnd);
    state
}

// ── Gamma ramp (shared, reads crush tab slider) ───────────────────────────────

unsafe fn apply_ramp(st: &mut AppState, hwnd: HWND) {
    if st.crush.previewing { return; }
    let (ok, v) = st.crush.apply_ramp();

    // ── Persistent error label — always reflects current driver state ─────────
    if st.nvidia_cam_enabled {
        set_error(st, "⚠  Gamma blocked — disable NVIDIA Override / Colour Accuracy Mode");
        return;
    }
    if !ok {
        set_error(st, "⚠  Gamma blocked by GPU driver");
        return;
    }
    // Condition cleared — hide the error label.
    set_error(st, "");
}

unsafe fn maybe_restore_gamma(st: &mut AppState, hwnd: HWND) {
    // Restore the currently active gamma state if an external process has
    // overridden it while the app was visible or inactive.
    if st.crush.maybe_restore_desired_ramp() {
        // If restoration succeeded, clear any stale error state.
        set_error(st, "");
    }
}

// ── Tab visibility ────────────────────────────────────────────────────────────

unsafe fn show_tab(st: &mut AppState, hwnd: HWND) {
    let tab = st.active_tab;

    st.crush.group.set_visible(tab == 0);

    set_visible(st.dimmer.h_lbl_dim_title,   tab == 1);
    set_visible(st.dimmer.h_lbl_dim_sub,     tab == 1);
    set_visible(st.dimmer.h_chk_taskbar_dim, tab == 1);

    let show_dim_controls = tab == 1 && st.dimmer.enabled;
    st.dimmer.grp_dim_controls.set_visible(show_dim_controls);

    st.hotkeys.group.set_visible(tab == 2);
    st.debug.group.set_visible(tab == 3);
    st.about.group.set_visible(tab == 4);

    // Arm TIMER_DEBUG_REFRESH only while the debug tab is visible (and only in
    // --debug mode — is_debug_mode() is false in production so the timer is
    // never started there at all).  Kill it when switching to any other tab so
    // the 500 ms wakeup disappears the moment the user leaves the debug tab.
    if is_debug_mode() {
        if tab == 3 {
            SetTimer(hwnd, TIMER_DEBUG_REFRESH, 500, None);
            crate::tab_debug::install_mouse_hook(hwnd);
            crate::tab_debug::install_debug_hotkeys(hwnd);
        } else {
            KillTimer(hwnd, TIMER_DEBUG_REFRESH);
            crate::tab_debug::uninstall_mouse_hook();
            crate::tab_debug::uninstall_debug_hotkeys(hwnd);
        }
    }
}

use std::sync::atomic::Ordering;

/// Set to 1 when launched with --debug. Readable from any thread/module.
static DEBUG_MODE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Set to true when the tray Restart command is used; triggers a re-spawn
/// at the end of WM_DESTROY, after the single-instance mutex is released.
pub static RESTART_ON_EXIT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub fn is_debug_mode() -> bool {
    DEBUG_MODE.load(Ordering::Relaxed)
}




// ── Helper: map nav-button control ID → tab index ─────────────────────────────

fn nav_btn_to_tab(id: usize) -> Option<usize> {
    match id {
        IDC_NAV_BTN_0 => Some(0),
        IDC_NAV_BTN_1 => Some(1),
        IDC_NAV_BTN_2 => Some(2),
        IDC_NAV_BTN_3 => Some(3),
        IDC_NAV_BTN_4 => Some(4),
        _ => None,
    }
}

// ── Command handler ───────────────────────────────────────────────────────────

unsafe fn on_command(st: &mut AppState, hwnd: HWND, id: usize, notify: u32, _ctrl: HWND) {
    match id {
        IDC_ABOUT_BTN_CHECK if notify == BN_CLICKED as u32 => {
            st.about.on_check_updates();
        }
        IDC_BTN_LOG_CLEAR => {
            st.debug.on_log_clear();
        }
        IDC_BTN_DIM_DEFAULTS => {
            let msg = st.dimmer.restore_defaults(hwnd, &mut st.ini);
            set_status(st, &msg, C_ACCENT);
        }
        id if notify == BN_CLICKED as u32
            && nav_btn_to_tab(id).is_some() =>
        {
            let tab = nav_btn_to_tab(id).unwrap();
            if st.active_tab != tab {
                st.active_tab = tab;
                show_tab(st, hwnd);
                app_layout::apply(st, hwnd);
                for h in &st.h_nav_btn { InvalidateRect(*h, None, false); }
            }
        }
        IDC_BTN_HDR_TOGGLE if notify == BN_CLICKED as u32 => {
            toggle_hdr_via_shortcut();
            // Re-query HDR state after a short delay so the button redraws correctly.
            SetTimer(hwnd, TIMER_HDR, 300, None);
        }
        IDC_BTN_QUIT     => { tray::remove_tray_icon(hwnd, st.tray_menu, &mut st.tray_added); DestroyWindow(hwnd); }
        IDC_BTN_MINIMIZE => { st.crush.hdr_panel.suspend_d3d(); ShowWindow(hwnd, SW_HIDE); }
        IDC_DDL_REFRESH if notify == CBN_SELCHANGE as u32 => {
            if !st.crush.suppress_load {
                match st.crush.apply_refresh_rate() {
                    Ok(hz) => {
                        set_status(st, &format!("Refresh rate set to {hz} Hz"), C_ACCENT);
                        rearm_render_timer(hwnd);
                        update_slider_anim_interval(hz as i32);
                        crate::tab_dimmer::set_fade_interval_from_hz(hz as i32);
                        if let Some((_, status)) =
                            st.crush.try_auto_load_profile_for_hz(hz as i32, &mut st.ini)
                        {
                            set_status(st, &status, C_ACCENT);
                        }
                        // Start reposition window on refresh rate change.
                        if st.dimmer.enabled {
                            st.dimmer.start_reposition_overlays();
                        }
                    }
                    Err(code) => {
                        set_error(st,
                            &format!("Failed to set refresh rate (code {})", code));
                    }
                }
            }
        }
        IDC_DDL_REFRESH if notify == CBN_DROPDOWN as u32 => {
            SetTimer(hwnd, TIMER_SCROLL_REFRESH, 1, None);
        }
        IDC_CHK_STARTUP if notify == BN_CLICKED as u32 => {
            st.chk_startup_state = !st.chk_startup_state;
            SendMessageW(st.h_chk_startup, BM_SETCHECK,
                WPARAM(st.chk_startup_state as usize), LPARAM(0));
            redraw_now(st.h_chk_startup);
            let msg = startup::toggle_startup(st.chk_startup_state);
            set_status(st, msg, C_ACCENT);
        }
        IDC_CHK_TASKBAR_DIM if notify == BN_CLICKED as u32 => {
            let (msg, ok) = st.dimmer.on_checkbox_toggled(hwnd, &mut st.ini);
            show_tab(st, hwnd);
            if !msg.is_empty() {
                set_status(st, msg, if ok { C_ACCENT } else { C_WARN });
            }
        }
        id if (id == IDC_CHK_SUPPRESS_FS as usize
               || id == IDC_CHK_SUPPRESS_AH as usize)
            && notify == BN_CLICKED as u32 =>
        {
            // Owner-drawn buttons don't auto-toggle — flip the flag ourselves,
            // then sync BM_SETCHECK and force a redraw so the visual matches.
            //
            // IDC_CHK_SUPPRESS_FS / _AH live on the debug tab (--debug only).
            if id == IDC_CHK_SUPPRESS_FS as usize {
                st.dimmer.suppress_fs_enabled = !st.dimmer.suppress_fs_enabled;
                let v = st.dimmer.suppress_fs_enabled;
                SendMessageW(st.debug.h_chk_suppress_fs, BM_SETCHECK, WPARAM(v as usize), LPARAM(0));
                redraw_now(st.debug.h_chk_suppress_fs);
            } else {
                st.dimmer.suppress_ah_enabled = !st.dimmer.suppress_ah_enabled;
                let v = st.dimmer.suppress_ah_enabled;
                SendMessageW(st.debug.h_chk_suppress_ah, BM_SETCHECK, WPARAM(v as usize), LPARAM(0));
                redraw_now(st.debug.h_chk_suppress_ah);
            }
        }
        200 => { ShowWindow(hwnd, SW_SHOW); SetForegroundWindow(hwnd); }
        201 => {
            tray::remove_tray_icon(hwnd, st.tray_menu, &mut st.tray_added);
            // Mark restart intent — the actual spawn happens at the end of
            // WM_DESTROY, after the single-instance mutex has been released.
            RESTART_ON_EXIT.store(true, Ordering::SeqCst);
            DestroyWindow(hwnd);
        }
        202 => { tray::remove_tray_icon(hwnd, st.tray_menu, &mut st.tray_added); DestroyWindow(hwnd); }
       
        // Clear button — clear the row then auto-save + re-register.
        id if (IDC_HK_CLR_TOGGLE_DIMMER..=IDC_HK_CLR_DIM_INCREASE).contains(&id)
            && notify == BN_CLICKED as u32 =>
        {
            st.hotkeys.clear_row_by_id(id);
            st.hotkeys.save(&mut st.ini);
            register_hotkeys(&st.ini, &mut st.mouse_hotkeys, hwnd);
            set_status(st, "Hotkey cleared", C_ACCENT);
        }
        // Pill sent EN_CHANGE after key captured — auto-save + re-register.
        id if (IDC_HK_EDT_TOGGLE_DIMMER..=IDC_HK_EDT_DIM_INCREASE).contains(&id)
            && notify == EN_CHANGE as u32 =>
        {
            st.hotkeys.save(&mut st.ini);
            register_hotkeys(&st.ini, &mut st.mouse_hotkeys, hwnd);
            set_status(st, "Hotkey saved", C_ACCENT);
        }
        _ => {}
    }
}

// ── Status bar ────────────────────────────────────────────────────────────────

/// Show a transient message in the normal status label.
/// Automatically hides after 4 seconds via TIMER_STATUS_CLEAR.
unsafe fn set_status(st: &mut AppState, text: &str, color: COLORREF) {
    st.status_color = color;
    set_window_text(st.h_lbl_status, text);
    set_visible(st.h_lbl_status, !text.is_empty());
    InvalidateRect(st.h_lbl_status, None, true);

    // (Re)arm the auto-clear timer.  KillTimer on a non-existent timer is a no-op.
    let hwnd = st.hwnd;
    if text.is_empty() {
        KillTimer(hwnd, TIMER_STATUS_CLEAR);
    } else {
        SetTimer(hwnd, TIMER_STATUS_CLEAR, 4000, None);
    }
}

/// Show or clear the persistent error label (shown below the status label).
/// Pass an empty string to hide it.
unsafe fn set_error(st: &mut AppState, text: &str) {
    set_window_text(st.h_lbl_error, text);
    set_visible(st.h_lbl_error, !text.is_empty());
    InvalidateRect(st.h_lbl_error, None, true);
}

// ── Render timer ──────────────────────────────────────────────────────────────

unsafe fn rearm_render_timer(hwnd: HWND) {
    SetTimer(hwnd, TIMER_RENDER, render_interval_ms(), None);
}

// ── Slider animation interval ─────────────────────────────────────────────────

/// Recompute and store the slider animation timer interval from the current
/// display refresh rate.  Call this whenever the Hz changes (display switch,
/// user dropdown selection).  1000/hz gives one tick per frame; clamped to
/// [1, 16] so we never exceed 16ms (60Hz floor) or go below 1ms.
fn update_slider_anim_interval(hz: i32) {
    let hz = hz.max(60) as u32;
    let ms = (1000 / hz).clamp(1, 16);
    ui_drawing::SLIDER_ANIM_INTERVAL_MS.store(ms, Ordering::Relaxed);
}