// app.rs — AppState, main window entry, WndProc, and shared logic.
// Per-tab logic lives in tab_*.rs; helpers in ui_drawing.rs and win32.rs.

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
        HK_DECREASE, HK_INCREASE, HK_TOGGLE_HDR,
        MOUSE_HK_SLOTS,
        install_compare_hook, uninstall_compare_hook,
        install_repeat_hook, uninstall_repeat_hook, WM_CRUSH_REPEAT_END,
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
    tab_system::{SystemTab, IDC_SYS_DDL_SCREEN_TIMEOUT, IDC_SYS_DDL_SLEEP_TIMEOUT,
                 IDC_SYS_DDL_SCREENSAVER, IDC_SYS_EDT_SS_TIMEOUT},
    tab_dimmer::{DimmerTab, ZOrderWinEventHooks, ZLogKind, zorder_log},
    ui_drawing::{
        self,
        client_size_of,
        draw_dark_button_full, draw_hdr_toggle_switch, draw_nav_item, NAV_BTN_HOVER_PROP,
        get_slider_val, make_font,
        set_bounds, set_window_text,
        slider_subclass_proc, nav_btn_subclass_proc,
        hdr_toggle_subclass_proc, dimmer_toggle_subclass_proc,
        install_action_btn_hover,
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
// WM_DPICHANGED posts this instead of calling SetWindowPos synchronously,
// so WM_DISPLAYCHANGE can arrive (and reposition overlays) before reflow.
use windows::Win32::UI::WindowsAndMessaging::WM_APP;
const WM_APP_DEFERRED_DPI_RESIZE: u32 = WM_APP + 10;

use std::cell::Cell;
thread_local! {
    /// Suggested RECT from WM_DPICHANGED, read by the deferred handler.
    static DEFERRED_DPI_RECT: Cell<RECT> = Cell::new(RECT::default());
}

// ── Per-window state ──────────────────────────────────────────────────────────

const IDC_BTN_HDR_TOGGLE: usize = 150;

pub struct AppState {
    #[allow(dead_code)]
    hwnd: HWND,

    // ── Tab sub-states ────────────────────────────────────────────────────────
    pub crush:   CrushTab,   // tab 0 — Black Crush Tweak
    pub dimmer:  DimmerTab,  // tab 1 — Taskbar Dimmer
    pub system:  SystemTab,  // tab 2
    pub hotkeys: HotkeysTab, // tab 3
    pub debug:   DebugTab,   // tab 4
    pub about:   AboutTab,   // tab 5

    // ── Shared controls ───────────────────────────────────────────────────────
    pub h_chk_startup:    HWND,
    #[allow(dead_code)] pub h_btn_quit:       HWND,
    #[allow(dead_code)] pub h_btn_minimize:   HWND,
    pub h_btn_hdr_toggle: HWND,
    pub h_lbl_status:     HWND,
    /// Persistent error label (gamma blocked, etc.), shown below h_lbl_status.
    pub h_lbl_error:      HWND,
    pub h_sep_vert:       HWND,
    pub h_sep_h:          [HWND; 4],
    /// Left-panel nav buttons, one per tab.
    pub h_nav_btn:        [HWND; 6],

    /// App icon used for the tab 0 nav button.
    h_app_icon:   HICON,
    /// PNG icons for nav buttons (None = use built-in glyph).
    nav_icons:    crate::nav_icons::NavIcons,
    /// PNG icons at 32px for tab header titles.
    tab_header_icons: crate::nav_icons::TabHeaderIcons,
    active_tab:   usize,
    pub update_available: bool,

    // ── Brushes / fonts (kept alive for WM_PAINT) ─────────────────────────────
    bg_brush:    HBRUSH,
    bg3_brush:   HBRUSH,
    sep_brush:   HBRUSH,

    _font_normal:   HFONT,
    _font_title:    HFONT,
    _font_bold_val: HFONT,

    tray_menu:   HMENU,
    tray_added:  bool,
    /// True once the first close-to-tray balloon has been shown.
    tray_balloon_shown: bool,

    ini:               ProfileManager,
    status_color:      COLORREF,
    chk_startup_state: bool,
    layout_initialized: bool,

    /// WinEvent hooks for taskbar overlay Z-order (foreground + minimize).
    zorder_winevent_hooks: Option<ZOrderWinEventHooks>,

    /// Mouse-button bindings (can't use RegisterHotKey).
    /// Indexed by HK_* constant; 0 = unbound.
    mouse_hotkeys: [u32; 9],

    /// Cached result of `CrushTab::is_nvidia_cam_enabled()`.
    /// Refreshed at startup and every TIMER_HDR tick (~2 s).
    nvidia_cam_enabled: bool,

    /// Last gamma-block state seen by TIMER_HDR.
    /// None = not yet evaluated; Some(true/false) = last known state.
    /// Only calls set_error on transitions to avoid wiping unrelated messages.
    last_gamma_blocked: Option<bool>,

    /// Debounce flag for black-level gamma application.
    ramp_dirty: bool,

    /// Direction of the currently held increase/decrease hotkey: +1, -1, or 0 (none).
    crush_repeat_delta: i32,
    /// True while waiting for the initial repeat delay to expire.
    crush_repeat_initial: bool,
}

impl AppState {
    /// Recreate fonts at new DPI and push them to all child controls.
    /// Called during deferred DPI resize, before layout reflow.
    pub unsafe fn rebuild_fonts(&mut self, hwnd: HWND) {
        let dpi = GetDpiForWindow(hwnd).max(96);

        // Delete old fonts.
        DeleteObject(HGDIOBJ(self._font_normal.0));
        DeleteObject(HGDIOBJ(self._font_title.0));
        DeleteObject(HGDIOBJ(self._font_bold_val.0));

        // Create DPI-correct replacements.
        self._font_normal   = make_font(w!("Segoe UI"), 10, dpi, false);
        self._font_title    = make_font(w!("Segoe UI"), 16, dpi, true);
        self._font_bold_val = make_font(w!("Consolas"), 14, dpi, true);

        // Broadcast normal font to all children.
        EnumChildWindows(
            hwnd,
            Some(set_font_enum_proc),
            LPARAM(self._font_normal.0 as isize),
        );

        // Override title and bold-value fonts on specific controls.
        for &h in &[
            self.crush.h_lbl_title,
            self.dimmer.h_lbl_dim_title,
            self.system.h_lbl_title,
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

        // Section heading font (11pt bold). Cached to avoid repeated allocations.
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
            self.system.h_lbl_sect_power,
            self.system.h_lbl_sect_screensaver,
        ] {
            SendMessageW(h, WM_SETFONT, WPARAM(font_sect.0 as usize), LPARAM(1));
        }
        // font_sect is leaked intentionally, same pattern as ::new()

        // Reload nav / tab-header icons at the new DPI.
        self.nav_icons.destroy();
        self.tab_header_icons.destroy();
        self.nav_icons        = crate::nav_icons::NavIcons::load(dpi);
        self.tab_header_icons = crate::nav_icons::TabHeaderIcons::load(dpi);

        // Patch zoom-icon statics: overwrite subclass ref_data with new HBITMAP.
        // SetWindowSubclass with the same proc + UID updates ref_data in-place.
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

        // Patch tab-header title labels with the new DPI-correct bitmaps.
        // paint_tab_header reads TAB_HDR_BITMAP on the next WM_PAINT.
        let hdr_pairs: &[(HWND, Option<HBITMAP>)] = &[
            (self.crush.h_lbl_title,      self.tab_header_icons.crush),
            (self.dimmer.h_lbl_dim_title, self.tab_header_icons.dimmer),
            (self.system.h_lbl_title,     self.tab_header_icons.system),
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

/// EnumChildWindows callback: sends WM_SETFONT to every child.
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

    // Skip showing window if launched with --minimized; tray icon is already up.
    if !startup::launched_minimized() {
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);
    }

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
            // timeBeginPeriod(1) is intentionally not called globally — it was
            // a known VRR flicker trigger and is no longer needed at 100 ms render intervals.
            // Register for power-setting change notifications (screen/sleep timeouts).
            RegisterPowerSettingNotification(
                HANDLE(hwnd.0),
                &crate::tab_system::GUID_VIDEO_POWERDOWN_TIMEOUT,
                DEVICE_NOTIFY_WINDOW_HANDLE,
            );
            RegisterPowerSettingNotification(
                HANDLE(hwnd.0),
                &crate::tab_system::GUID_STANDBY_TIMEOUT,
                DEVICE_NOTIFY_WINDOW_HANDLE,
            );
            SetTimer(hwnd, TIMER_HDR, 2000, None);
            // TIMER_DEBUG_REFRESH is armed/killed by show_tab — never runs in production.
            SetTimer(hwnd, TIMER_RENDER, render_interval_ms(), None);
            LRESULT(0)
        }
        
       // WM_DPICHANGED: post resize instead of calling SetWindowPos synchronously,
       // so WM_DISPLAYCHANGE can arrive before layout reflow.
       WM_DPICHANGED => {
            let prc = lparam.0 as *const RECT;
            let new_rect = unsafe { *prc };

            // Log immediately — anchors the whole DPI-change sequence.
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

            // Stash suggested rect for the deferred handler.
            DEFERRED_DPI_RECT.with(|cell| cell.set(new_rect));

            unsafe {
                PostMessageW(hwnd, WM_APP_DEFERRED_DPI_RESIZE, WPARAM(0), LPARAM(0));
            }

            if let Some(st) = borrow_state::<AppState>(hwnd) {
                if st.dimmer.enabled {
                    st.dimmer.start_reposition_overlays();
                }
            }
            LRESULT(0)
        }

        // Deferred resize handler: runs after display broadcast settles.
        WM_APP_DEFERRED_DPI_RESIZE => {
            let new_rect = DEFERRED_DPI_RECT.with(|cell| cell.get());

            // Three [DR] log lines bracket each expensive call so the debug log
            // shows where time is spent (phase 0 = entry, 1 = post-SetWindowPos,
            // 2 = post-RedrawWindow).
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

            // Rebuild fonts before SetWindowPos triggers layout reflow.
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
                RedrawWindow(hwnd, None, None,
                    RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_ERASE | RDW_FRAME | RDW_UPDATENOW);
            }

            log_dr!(2u64, "after RedrawWindow");

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

        // WM_DISPLAYCHANGE: display switched — reposition overlays via polling.
        WM_DISPLAYCHANGE => {
            if let Some(st) = borrow_state::<AppState>(hwnd) {
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

                // Re-query HDR state over the next several ticks.  DXGI often
                // reports stale colour space data immediately after a display
                // change while the driver finishes its pipeline switch.
                st.crush.hdr_panel.schedule_hdr_recheck();

                // When an external tool (game, Windows display settings, etc.)
                // changes the refresh rate the CBN_SELCHANGE path is never
                // reached, so the Hz-keyed profile is never applied.  Detect
                // the new Hz now and load the matching profile — exactly the
                // same steps the dropdown handler performs.
                let hz = get_current_hz();
                st.crush.populate_refresh_rates(&mut st.ini);
                update_slider_anim_interval(hz);
                crate::tab_dimmer::set_fade_interval_from_hz(hz);
                if let Some((_, status)) =
                    st.crush.try_auto_load_profile_for_hz(hz, &mut st.ini)
                {
                    set_status(st, &status, C_ACCENT);
                } else {
                    // Hz has no saved non-zero profile — still reapply the
                    // (possibly zero) ramp so the driver gets the correct
                    // neutral curve after the mode switch resets gamma.
                    apply_ramp(st, hwnd);
                }

                if st.dimmer.enabled {
                    st.dimmer.start_reposition_overlays();
                }
                RedrawWindow(hwnd, None, None,
                    RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_ERASE | RDW_FRAME | RDW_UPDATENOW);
            }
            LRESULT(0)
        }

       

        WM_SHOWWINDOW if wparam.0 != 0 => {
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                let first_show = !st.layout_initialized;
                if first_show {
                    app_layout::apply(st, hwnd);
                    crate::ui_drawing::install_bitmap_static(
                        st.crush.h_lbl_zoom_out_icon, st.nav_icons.zoom_out);
                    crate::ui_drawing::install_bitmap_static(
                        st.crush.h_lbl_zoom_icon, st.nav_icons.zoom);
                }
                let (pw, ph) = client_size_of(st.crush.h_hdr_panel);
                st.crush.hdr_panel.init_d3d(st.crush.h_hdr_panel, pw as u32, ph as u32);
                // Always recheck after init_d3d — DXGI colour-space data is unreliable
                // at startup and after suspend/hide (tray restores included).
                st.crush.hdr_panel.schedule_hdr_recheck();
                st.crush.hdr_panel.update(get_slider_val(st.crush.h_sld_black));
                st.crush.update_sl_hint();
                st.crush.update_range_label();
                st.crush.populate_refresh_rates(&mut st.ini);
                update_slider_anim_interval(get_current_hz());
                if !st.layout_initialized {
                    st.layout_initialized = true;
                }
                maybe_restore_gamma(st, hwnd);

                // First show only — skip on tray restores.
                if first_show {
                    let args: Vec<String> = std::env::args().collect();
                    let is_debug = args.iter().any(|a| a.eq_ignore_ascii_case("--debug"));
                    DEBUG_MODE.store(is_debug, Ordering::Relaxed);
                    if !is_debug_mode() {
                        ShowWindow(st.h_nav_btn[3], SW_HIDE);
                        KillTimer(hwnd, TIMER_DEBUG_REFRESH);
                    }
                    st.about.spawn_update_check(hwnd);
                }
            }
            LRESULT(0)
        }

        WM_SIZE => {
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                app_layout::apply(st, hwnd);
                let (pw, ph) = client_size_of(st.crush.h_hdr_panel);
                st.crush.hdr_panel.resize(pw as u32, ph as u32);
                // RDW_UPDATENOW is required with WS_EX_COMPOSITED — without it
                // the stale frame stays visible until the next input event.
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
                            // Only repaint if HDR status actually changed.
                            if st.crush.hdr_panel.refresh_hdr_status() {
                                st.crush.update_sl_hint();
                                st.crush.update_range_label();
                                if !st.crush.previewing { apply_ramp(st, hwnd); }
                                InvalidateRect(st.h_btn_hdr_toggle, None, true);
                                UpdateWindow(st.h_btn_hdr_toggle);
                                // Sync tray tooltip with new HDR state.
                                let crush_val = crate::ui_drawing::get_slider_val(st.crush.h_sld_black);
                                tray::update_tray_tooltip(hwnd, st.dimmer.enabled, crush_val, st.crush.hdr_panel.hdr_active);
                            }

                            // Poll gamma block independently of slider moves.
                            {
                                st.nvidia_cam_enabled = CrushTab::is_nvidia_cam_enabled();
                                let blocked = st.nvidia_cam_enabled || is_gamma_blocked(&st.crush);

                                let tick = windows::Win32::System::SystemInformation::GetTickCount64();
                                let detail = format!(
                                    "[GAMMA] tab={} nvidia_cam={} blocked={} debug_mode={} v={}",
                                    st.active_tab,
                                    st.nvidia_cam_enabled,
                                    blocked,
                                    is_debug_mode(),
                                    crate::ui_drawing::get_slider_val(st.crush.h_sld_black),
                                );
                                if let Some(Ok(mut log)) = zorder_log().map(|m| m.lock()) {
                                    log.push(tick, ZLogKind::DisplayChange, blocked as u64, detail);
                                }

                                // Only call set_error on transitions to avoid wiping unrelated messages.
                                let gamma_blocked_now = st.nvidia_cam_enabled || blocked;
                                let state_changed = st.last_gamma_blocked != Some(gamma_blocked_now);
                                st.last_gamma_blocked = Some(gamma_blocked_now);

                                if st.active_tab == 0 {
                                    if gamma_blocked_now {
                                        // Refresh idempotently — ensures label shows even after a tab switch.
                                        if st.nvidia_cam_enabled {
                                            set_error(st, "⚠ Gamma blocked by GPU driver - disable \"Override to reference mode\"");
                                        } else {
                                            set_error(st, "⚠ Gamma blocked by GPU driver");
                                        }
                                    } else if state_changed {
                                        set_error(st, "");
                                    }
                                }
                            }
                        }
                        maybe_restore_gamma(st, hwnd);
                        SetTimer(hwnd, TIMER_HDR, 2000, None);
                    }
                    TIMER_RENDER if ui_visible => {
                        if st.crush.hdr_panel.render_tick() {
                            // HDR status changed on a retry tick.
                            st.crush.update_sl_hint();
                            st.crush.update_range_label();
                            if !st.crush.previewing { apply_ramp(st, hwnd); }
                            InvalidateRect(st.h_btn_hdr_toggle, None, true);
                        }
                    }
                    TIMER_OVERLAY_FADE  => { st.dimmer.tick_fade(); }
                    TIMER_DEBUG_REFRESH => {
                        // Armed/killed by show_tab; skip while hidden to tray.
                        if ui_visible {
                            st.debug.refresh(&st.dimmer);
                        }
                    }
                    TIMER_SCROLL_REFRESH => {
                        KillTimer(hwnd, TIMER_SCROLL_REFRESH);
                        SendMessageW(st.crush.h_ddl_refresh, CB_SETTOPINDEX, WPARAM(0), LPARAM(0));
                    }
                    TIMER_STATUS_CLEAR => {
                        KillTimer(hwnd, TIMER_STATUS_CLEAR);
                        set_status(st, "", C_ACCENT);
                    }
                    TIMER_FULLSCREEN_RECHECK => {
                        // One-shot: catches late-sizing games ~1 s after a foreground event.
                        KillTimer(hwnd, TIMER_FULLSCREEN_RECHECK);
                        st.dimmer.on_fullscreen_recheck();
                    }
                    TIMER_OVERLAY_REPOSITION => {
                        st.dimmer.tick_reposition();
                    }
                    TIMER_RAMP_APPLY => {
                        KillTimer(hwnd, TIMER_RAMP_APPLY);
                        if st.ramp_dirty {
                            st.ramp_dirty = false;
                            apply_ramp(st, hwnd);
                        }
                    }
                    TIMER_CRUSH_REPEAT => {
                        let delta = st.crush_repeat_delta;
                        if delta != 0 {
                            // First tick ends the initial delay — switch to fast repeat.
                            if st.crush_repeat_initial {
                                st.crush_repeat_initial = false;
                                SetTimer(hwnd, TIMER_CRUSH_REPEAT, 100, None);
                            }
                            let v     = get_slider_val(st.crush.h_sld_black);
                            let new_v = (v + delta).clamp(MIN_BLACK, MAX_BLACK);
                            if new_v != v {
                                SendMessageW(st.crush.h_sld_black, TBM_SETPOS,
                                    WPARAM(1), LPARAM(new_v as isize));
                                st.crush.on_black_slider_changed(&mut st.ini);
                                apply_ramp(st, hwnd);
                                InvalidateRect(st.crush.h_sld_black, None, false);
                            }
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
                let notify_code = (wparam.0 & 0xFFFF) as u32;
                let is_end_track = matches!(notify_code, 0x4 | 0x8); // TB_THUMBPOSITION | TB_ENDTRACK

                if ctrl == st.crush.h_sld_black {
                    if is_end_track {
                        // Drag finished — full commit: ramp + INI + dropdown.
                        KillTimer(hwnd, TIMER_RAMP_APPLY);
                        st.ramp_dirty = false;
                        st.crush.on_black_slider_changed(&mut st.ini);
                        apply_ramp(st, hwnd);
                        refresh_tray_state(st, hwnd);
                    } else {
                        // Mid-drag: apply gamma immediately and update visuals.
                        st.crush.on_black_slider_visual();
                        apply_ramp(st, hwnd);
                        st.ramp_dirty = false;
                    }
                    InvalidateRect(ctrl, None, false);
                } else if ctrl == st.crush.h_sld_squares {
                    st.crush.on_squares_changed();
                    InvalidateRect(ctrl, None, false);
                } else if ctrl == st.dimmer.h_sld_taskbar_dim {
                    // Visual update on every tick; persist only on drag-end.
                    let msg = st.dimmer.update_dim_visuals(hwnd);
                    set_status(st, &msg, C_ACCENT);
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
                if di.hwndItem == st.crush.h_ddl_refresh
                    || di.hwndItem == st.system.h_ddl_screen_timeout
                    || di.hwndItem == st.system.h_ddl_sleep_timeout
                    || di.hwndItem == st.system.h_ddl_screensaver
                {
                    ui_drawing::draw_combo_item(di);
                } else if di.hwndItem == st.h_nav_btn[0] {
                    draw_nav_item(di, st.active_tab == 0,
                        GetFocus() == st.h_nav_btn[0],
                        !GetPropW(st.h_nav_btn[0], NAV_BTN_HOVER_PROP).0.is_null(),
                        st.h_app_icon, "",
                        st.nav_icons.crush, false);
                } else if di.hwndItem == st.h_nav_btn[1] {
                    draw_nav_item(di, st.active_tab == 1,
                        GetFocus() == st.h_nav_btn[1],
                        !GetPropW(st.h_nav_btn[1], NAV_BTN_HOVER_PROP).0.is_null(),
                        HICON(ptr::null_mut()), "⊞",
                        st.nav_icons.dimmer, false);
                } else if di.hwndItem == st.h_nav_btn[5] {
                    draw_nav_item(di, st.active_tab == 2,
                        GetFocus() == st.h_nav_btn[5],
                        !GetPropW(st.h_nav_btn[5], NAV_BTN_HOVER_PROP).0.is_null(),
                        HICON(ptr::null_mut()), "⚙",
                        st.nav_icons.system, false);
                } else if di.hwndItem == st.h_nav_btn[2] {
                    draw_nav_item(di, st.active_tab == 3,
                        GetFocus() == st.h_nav_btn[2],
                        !GetPropW(st.h_nav_btn[2], NAV_BTN_HOVER_PROP).0.is_null(),
                        HICON(ptr::null_mut()), "⌨",
                        st.nav_icons.hotkeys, false);
                } else if di.hwndItem == st.h_nav_btn[3] {
                    draw_nav_item(di, st.active_tab == 4,
                        GetFocus() == st.h_nav_btn[3],
                        !GetPropW(st.h_nav_btn[3], NAV_BTN_HOVER_PROP).0.is_null(),
                        HICON(ptr::null_mut()), "🐛",
                        st.nav_icons.debug, false);
                } else if di.hwndItem == st.h_nav_btn[4] {
                    draw_nav_item(di, st.active_tab == 5,
                        GetFocus() == st.h_nav_btn[4],
                        !GetPropW(st.h_nav_btn[4], NAV_BTN_HOVER_PROP).0.is_null(),
                        HICON(ptr::null_mut()), "ℹ",
                        st.nav_icons.about, st.update_available);
                } else if di.hwndItem == st.debug.h_chk_suppress_fs {
                    draw_dark_button_full(di,
                        st.debug.h_chk_suppress_fs, HWND(ptr::null_mut()),
                        HWND(ptr::null_mut()), HWND(ptr::null_mut()),
                        st.dimmer.suppress_fs_enabled, false, false, false, false);
                } else if di.hwndItem == st.debug.h_chk_suppress_ah {
                    draw_dark_button_full(di,
                        st.debug.h_chk_suppress_ah, HWND(ptr::null_mut()),
                        HWND(ptr::null_mut()), HWND(ptr::null_mut()),
                        st.dimmer.suppress_ah_enabled, false, false, false, false);
                } else if di.hwndItem == st.h_btn_hdr_toggle {
                    draw_hdr_toggle_switch(di, st.crush.hdr_panel.hdr_active, None);
                } else if di.hwndItem == st.system.h_btn_taskbar_autohide {
                    draw_hdr_toggle_switch(di, st.system.taskbar_autohide_state, None);
                } else if di.hwndItem == st.system.h_ddl_screensaver {
                    ui_drawing::draw_combo_item(di);
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
                        false, // is_hovered
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
                if ctrl == st.about.h_lbl_check_info && st.update_available {
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
                // Section headings use C_FG; h_lbl_hz_profile uses a darker tone.
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
                    ctrl == st.hotkeys.h_lbl_desc          ||
                    ctrl == st.system.h_lbl_desc;
                let is_hk_row_lbl = st.hotkeys.rows.iter().any(|r| r.h_lbl == ctrl);
                SetBkColor(hdc, C_BG);
                SetTextColor(hdc, if is_sub || is_hk_row_lbl { C_LABEL } else { C_FG });
                return LRESULT(st.bg_brush.0 as isize);
            }
            LRESULT(0)
        }

        WM_CTLCOLOREDIT => {
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                let hdc = HDC(wparam.0 as *mut _);
                SetBkColor(hdc, C_BG3);
                SetTextColor(hdc, C_FG);
                return LRESULT(st.bg3_brush.0 as isize);
            }
            LRESULT(0)
        }
        WM_CTLCOLORBTN => {
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

        x if x == WM_CRUSH_REPEAT_END => {
            KillTimer(hwnd, TIMER_CRUSH_REPEAT);
            uninstall_repeat_hook();
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                st.crush_repeat_delta   = 0;
                st.crush_repeat_initial = false;
            }
            LRESULT(0)
        }

        crate::tab_about::WM_UPDATE_RESULT => {
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                st.about.on_update_result(hwnd, wparam.0, lparam.0);
                if wparam.0 == 1 {
                    st.update_available = true;
                    InvalidateRect(st.h_nav_btn[4], None, false);
                    if st.active_tab == 5 {
                        app_layout::apply(st, hwnd);
                    }
                }
            }
            LRESULT(0)
        }

        WM_DOWNLOAD_PROGRESS => {
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                st.about.on_download_progress(wparam.0, lparam.0 as usize);
            }
            LRESULT(0)
        }

        WM_DOWNLOAD_DONE => {
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                st.about.on_download_done(hwnd, wparam.0, lparam.0);
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
                        refresh_tray_state(st, hwnd);
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
                            // Resolve bound key for the keyboard hook (key-up detection).
                            // Mouse buttons have no key-up, so skip the hook for them.
                            let s = st.ini.read("Hotkeys", "HoldCompare", "None");
                            let vk = parse_hotkey(&s).map(|(_, v)| v).unwrap_or(0);
                            if vk != 0 && !crate::tab_hotkeys::is_mouse_sentinel(vk) {
                                install_compare_hook(hwnd, vk);
                            }
                            SendMessageW(hwnd, WM_COMPARE_START, WPARAM(0), LPARAM(0));
                        } else {
                            // Mouse buttons have no key-up — second press acts as release.
                            let s = st.ini.read("Hotkeys", "HoldCompare", "None");
                            let vk = parse_hotkey(&s).map(|(_, v)| v).unwrap_or(0);
                            if vk != 0 && crate::tab_hotkeys::is_mouse_sentinel(vk) {
                                SendMessageW(hwnd, WM_COMPARE_END, WPARAM(0), LPARAM(0));
                            }
                        }
                    }
                    HK_DECREASE => {
                        let v = get_slider_val(st.crush.h_sld_black);
                        let new_v = (v - 1).max(MIN_BLACK);
                        SendMessageW(st.crush.h_sld_black, TBM_SETPOS,
                            WPARAM(1), LPARAM(new_v as isize));
                        st.crush.on_black_slider_changed(&mut st.ini);
                        apply_ramp(st, hwnd);
                        InvalidateRect(st.crush.h_sld_black, None, false);
                        refresh_tray_state(st, hwnd);
                        if let Some((_, vk)) = parse_hotkey(&st.ini.read("Hotkeys", "DecreaseBlackCrush", "None")) {
                            st.crush_repeat_delta = -1;
                            st.crush_repeat_initial = true;
                            install_repeat_hook(hwnd, vk);
                            SetTimer(hwnd, TIMER_CRUSH_REPEAT, 400, None);
                        }
                    }
                    HK_INCREASE => {
                        let v = get_slider_val(st.crush.h_sld_black);
                        let new_v = (v + 1).min(MAX_BLACK);
                        SendMessageW(st.crush.h_sld_black, TBM_SETPOS,
                            WPARAM(1), LPARAM(new_v as isize));
                        st.crush.on_black_slider_changed(&mut st.ini);
                        apply_ramp(st, hwnd);
                        InvalidateRect(st.crush.h_sld_black, None, false);
                        refresh_tray_state(st, hwnd);
                        if let Some((_, vk)) = parse_hotkey(&st.ini.read("Hotkeys", "IncreaseBlackCrush", "None")) {
                            st.crush_repeat_delta = 1;
                            st.crush_repeat_initial = true;
                            install_repeat_hook(hwnd, vk);
                            SetTimer(hwnd, TIMER_CRUSH_REPEAT, 400, None);
                        }
                    }
                    HK_TOGGLE_HDR => {
                        toggle_hdr_via_shortcut();
                        st.crush.hdr_panel.schedule_hdr_recheck();
                        // Tooltip updated on next TIMER_HDR tick when HDR state settles.
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
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                if wparam.0 == PBT_APMRESUMEAUTOMATIC as usize {
                    if !st.crush.previewing { apply_ramp(st, hwnd); }
                } else if wparam.0 == PBT_POWERSETTINGCHANGE as usize {
                    // lparam → POWERBROADCAST_SETTING; re-read the changed timeout and sync its dropdown.
                    let pbs = &*(lparam.0 as *const POWERBROADCAST_SETTING);
                    let guid = pbs.PowerSetting;
                    if guid == crate::tab_system::GUID_VIDEO_POWERDOWN_TIMEOUT {
                        let idx = crate::tab_system::read_screen_timeout()
                            .map(crate::tab_system::timeout_to_index).unwrap_or(0);
                        st.system.screen_timeout_idx = idx;
                        SendMessageW(st.system.h_ddl_screen_timeout,
                            CB_SETCURSEL, WPARAM(idx), LPARAM(0));
                    } else if guid == crate::tab_system::GUID_STANDBY_TIMEOUT {
                        let idx = crate::tab_system::read_sleep_timeout()
                            .map(crate::tab_system::timeout_to_index).unwrap_or(0);
                        st.system.sleep_timeout_idx = idx;
                        SendMessageW(st.system.h_ddl_sleep_timeout,
                            CB_SETCURSEL, WPARAM(idx), LPARAM(0));
                    }
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
                x if x == WM_LBUTTONUP => {
                    ShowWindow(hwnd, SW_SHOW);
                    SetForegroundWindow(hwnd);
                }
                x if x == WM_RBUTTONUP => {
                    if let Some(st) = borrow_state::<AppState>(hwnd) {
                        let mut pt = POINT::default();
                        GetCursorPos(&mut pt);
                        SetForegroundWindow(hwnd);
                        // Rebuild so checkmarks always reflect current state.
                        let old = st.tray_menu;
                        st.tray_menu = build_tray_menu_for_state(st);
                        DestroyMenu(old);
                        st.dimmer.suppress_tray_menu = true;
                        TrackPopupMenu(
                            st.tray_menu,
                            TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
                            pt.x, pt.y, 0, hwnd, None,
                        );
                        st.dimmer.suppress_tray_menu = false;
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }

        WM_ENTERSIZEMOVE => {
            // Kill debug refresh timer during resize — z-order walks during a resize lock cause slowdowns.
            KillTimer(hwnd, TIMER_DEBUG_REFRESH);
            LRESULT(0)
        }

        WM_EXITSIZEMOVE => {
            // Re-arm debug timer if still on tab 4.
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                if is_debug_mode() && st.active_tab == 4 {
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
                if !st.tray_balloon_shown {
                    st.tray_balloon_shown = true;
                    st.ini.write("App", "TrayBalloonShown", "1");
                    tray::show_tray_balloon(hwnd);
                }
            }
            ShowWindow(hwnd, SW_HIDE);
            LRESULT(0)
        }

        WM_DESTROY => {
            uninstall_compare_hook();
            uninstall_repeat_hook();
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
                KillTimer(hwnd, TIMER_CRUSH_REPEAT);
                WTSUnRegisterSessionNotification(hwnd);
                gamma_ramp::reset_display_ramp();
                crate::tab_debug::uninstall_mouse_hook();

                // GDI cleanup — HBRUSH/HFONT are raw handles; Drop won't call DeleteObject.
                DeleteObject(HGDIOBJ(st.bg_brush.0));
                DeleteObject(HGDIOBJ(st.bg3_brush.0));
                DeleteObject(HGDIOBJ(st.sep_brush.0));
                DeleteObject(HGDIOBJ(st._font_normal.0));
                DeleteObject(HGDIOBJ(st._font_title.0));
                DeleteObject(HGDIOBJ(st._font_bold_val.0));
            }
            for id in [HK_TOGGLE_DIM, HK_TOGGLE_CRUSH, HK_HOLD_COMPARE, HK_DECREASE, HK_INCREASE, HK_TOGGLE_HDR] {
                UnregisterHotKey(hwnd, id as i32);
            }
            detach_state::<AppState>(hwnd);
            PostQuitMessage(0);
            LRESULT(0)
        }

        // Posted from zorder_winevent_proc (system thread) — handled on the UI thread
        // so DimmerTab is accessible without a lock.
        x if x == crate::tab_dimmer::WM_APP_FULLSCREEN_CHECK => {
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                let fg_hwnd = HWND(wparam.0 as *mut _);
                st.dimmer.on_fullscreen_check(fg_hwnd);
                // One-shot recheck ~1 s later to catch late-sizing games.
                SetTimer(hwnd, TIMER_FULLSCREEN_RECHECK, 1000, None);
            }
            LRESULT(0)
        }

        // Shell panel appeared (Quick Settings, Action Center, etc.) — re-raise overlays immediately.
        x if x == crate::tab_dimmer::WM_APP_RAISE_OVERLAYS => {
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                st.dimmer.force_raise_overlays();
            }
            LRESULT(0)
        }

        WM_NOTIFY => {
            // UDN_DELTAPOS fires before UDS_SETBUDDYINT — read the edit directly
            // to respect manually-typed values rather than the stale nm.iPos.
            let hdr = &*(lparam.0 as *const windows::Win32::UI::Controls::NMHDR);
            if hdr.code == windows::Win32::UI::Controls::UDN_DELTAPOS as u32 {
                #[repr(C)]
                struct NMUPDOWN { hdr: windows::Win32::UI::Controls::NMHDR, iPos: i32, iDelta: i32 }
                let nm = &*(lparam.0 as *const NMUPDOWN);
                if let Some(st) = borrow_state::<AppState>(hwnd) {
                    let mut buf = [0u16; 8];
                    let len = windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(
                        st.system.h_edt_ss_timeout, &mut buf) as usize;
                    let typed: i32 = String::from_utf16_lossy(&buf[..len])
                        .trim().parse().unwrap_or(nm.iPos);
                    let new_val = (typed + nm.iDelta).clamp(1, 999) as u32;
                    let msg = st.system.on_ss_timeout_set(new_val);
                    if !msg.is_empty() { set_status(st, &msg, C_ACCENT); }
                }
                // Return 1 to suppress the spin's own update — we already applied the delta.
                return LRESULT(1);
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }

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
            // Only arrives in --debug mode with tab 4 open (hook is active then).
            let btn = wparam.0;
            let raw = lparam.0 as isize as i64;
            let x   = (raw >> 32) as i32;
            let y   = (raw & 0xFFFF_FFFF) as i32;
            crate::tab_debug::log_mouse_click(btn, POINT { x, y });
            LRESULT(0)
        }

        // Show hand cursor over the GitHub link / update label.
        WM_SETCURSOR => {
            let cursor_hwnd = HWND(wparam.0 as *mut _);
            if let Some(st) = borrow_state::<AppState>(hwnd) {
                if cursor_hwnd == st.about.h_lbl_link
                    || (cursor_hwnd == st.about.h_lbl_check_info && st.update_available) {
                    let hand = LoadCursorW(None, IDC_HAND).unwrap_or_default();
                    SetCursor(hand);
                    return LRESULT(1);
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
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
    // h_lbl_error uses WS_EX_LEFT (not WS_EX_TRANSPARENT) so GDI clips around it —
    // otherwise h_lbl_status repaints would erase the error text.
    let h_lbl_error = CreateWindowExW(
        WS_EX_LEFT,
        w!("STATIC"), w!(""),
        WS_CHILD | WINDOW_STYLE(SS_CENTER as u32 | SS_CENTERIMAGE as u32 | SS_NOPREFIX),
        0, 0, 1, 1,
        hwnd,
        HMENU(ptr::null_mut()),
        hinstance,
        None,
    ).unwrap_or_default();
    SendMessageW(h_lbl_error, WM_SETFONT, WPARAM(_font_normal.0 as usize), LPARAM(1));

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
    let h_nav_btn_5 = cb.button(w!("System"),            IDC_NAV_BTN_5);
    let h_nav_btn_2 = cb.button(w!("Hotkeys"),           IDC_NAV_BTN_2);
    let h_nav_btn_3 = cb.button(w!("Debug"),             IDC_NAV_BTN_3);
    let h_nav_btn_4 = cb.button(w!("About"),             IDC_NAV_BTN_4);
    let h_nav_btn = [h_nav_btn_0, h_nav_btn_1, h_nav_btn_2, h_nav_btn_3, h_nav_btn_4, h_nav_btn_5];
    for &h in &h_nav_btn {
        SetWindowSubclass(h, Some(nav_btn_subclass_proc), 1, 0);
    }

    let mut ini = ProfileManager::new(&ini_path);

    // ── Construct tab sub-states ──────────────────────────────────────────────
    // h_sep_h[0..=2] belong to the crush tab; [3] is shared/bottom.
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
    // Restrict dimmer toggle clicks to the pill area only.
    SetWindowSubclass(dimmer.h_chk_taskbar_dim, Some(dimmer_toggle_subclass_proc), 3, 0);

    let hotkeys = HotkeysTab::new(
        hwnd, hinstance, dpi,
        _font_normal, _font_title,
        &mut ini,
    );

    let debug = DebugTab::new(hwnd, hinstance, dpi, _font_normal, _font_title);
    let mut system = SystemTab::new(hwnd, hinstance, dpi, _font_normal, _font_title);
    SetWindowSubclass(system.h_btn_taskbar_autohide, Some(hdr_toggle_subclass_proc), 5, 0);

    let about = AboutTab::new(hwnd, hinstance, dpi, _font_normal, _font_title);

    // Hover tracking for owner-drawn buttons/checkboxes.
    install_action_btn_hover(crush.h_btn_toggle);
    install_action_btn_hover(h_btn_quit);
    install_action_btn_hover(h_btn_minimize);

    let nav_icons         = crate::nav_icons::NavIcons::load(dpi);
    let tab_header_icons  = crate::nav_icons::TabHeaderIcons::load(dpi);

    let mut state = AppState {
        hwnd,
        crush, dimmer, system, hotkeys, debug, about,
        h_chk_startup, h_btn_quit, h_btn_minimize, h_btn_hdr_toggle,
        h_lbl_status, h_lbl_error, h_sep_vert, h_sep_h, h_nav_btn,
        h_app_icon, active_tab: 0, update_available: false,
        nav_icons,
        tab_header_icons,
        bg_brush, bg3_brush, sep_brush,
        _font_normal, _font_title, _font_bold_val,
        tray_menu: unsafe { windows::Win32::UI::WindowsAndMessaging::CreatePopupMenu().unwrap_or_default() }, tray_added: false,
        tray_balloon_shown: ini.read("App", "TrayBalloonShown", "0") == "1",
        ini,
        status_color: C_ACCENT,
        chk_startup_state: false,
        layout_initialized: false,
        zorder_winevent_hooks: None,
        mouse_hotkeys: [0u32; 9],
        nvidia_cam_enabled: unsafe { CrushTab::is_nvidia_cam_enabled() },
        last_gamma_blocked: None,
        ramp_dirty: false,
        crush_repeat_delta: 0,
        crush_repeat_initial: false,
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
        // Profile silently applied — no status message on startup.
    } else {
        let saved_v = state.ini.read_int(&sec, "Black", DEFAULT_BLACK)
            .clamp(0, MAX_BLACK);
        SendMessageW(state.crush.h_sld_black, TBM_SETPOS,
            WPARAM(1), LPARAM(saved_v as isize));
        InvalidateRect(state.crush.h_sld_black, None, true);
        let v_text = if saved_v == 0 { "OFF".to_string() } else { format!("{saved_v}") };
        set_text(state.crush.h_lbl_black_val, &v_text);
        // apply_ramp is called unconditionally below — no need to call it here too.
        state.crush.hdr_panel.update(saved_v);
    }

    show_tab(&mut state, hwnd);

    // Attach PNG icon painter to each tab's title label.
    ui_drawing::subclass_tab_header(state.crush.h_lbl_title,   state.tab_header_icons.crush);
    ui_drawing::subclass_tab_header(state.dimmer.h_lbl_dim_title, state.tab_header_icons.dimmer);
    ui_drawing::subclass_tab_header(state.system.h_lbl_title,  state.tab_header_icons.system);
    ui_drawing::subclass_tab_header(state.hotkeys.h_lbl_title, state.tab_header_icons.hotkeys);
    ui_drawing::subclass_tab_header(state.debug.h_lbl_title,   state.tab_header_icons.debug);
    ui_drawing::subclass_tab_header(state.about.h_lbl_title,  state.tab_header_icons.about);
    apply_ramp(&mut state, hwnd);

    let tray_menu = build_tray_menu_for_state(&state);
    state.tray_menu = tray_menu;
    tray::add_tray_icon(hwnd, hinstance, &mut state.tray_added);
    register_hotkeys(&state.ini, &mut state.mouse_hotkeys, hwnd);

    state
}

// ── Gamma ramp (shared, reads crush tab slider) ───────────────────────────────

unsafe fn apply_ramp(st: &mut AppState, hwnd: HWND) {
    if st.crush.previewing { return; }
    st.crush.apply_ramp();
}

unsafe fn maybe_restore_gamma(st: &mut AppState, hwnd: HWND) {
    st.crush.maybe_restore_desired_ramp();
}

/// Returns true if the GPU driver is ignoring gamma ramp writes.
/// Compares the current display ramp to the desired one without writing first —
/// avoids a race with async driver overrides that the old write-then-readback had.
/// Returns false if GetDeviceGammaRamp fails (assume unblocked).
unsafe fn is_gamma_blocked(crush: &crate::tab_crush::CrushTab) -> bool {
    let desired = crush.desired_ramp();
    match gamma_ramp::get_display_ramp() {
        Some(actual) => actual != desired,
        None => false,
    }
}

// ── Tab visibility ────────────────────────────────────────────────────────────

unsafe fn show_tab(st: &mut AppState, hwnd: HWND) {
    let tab = st.active_tab;

    st.crush.group.set_visible(tab == 0);

    // h_lbl_status and h_lbl_error belong to the Black Crush tab only.
    // Hide them entirely when any other tab is active, and restore the
    // h_lbl_status/h_lbl_error belong to tab 0 only.
    if tab == 0 {
        // Sync error label — last_gamma_blocked may have changed while on another tab.
        if let Some(blocked) = st.last_gamma_blocked {
            if blocked {
                if st.nvidia_cam_enabled {
                    set_error(st, "⚠  Gamma blocked — disable NVIDIA Override / Colour Accuracy Mode");
                } else {
                    set_error(st, "⚠  Gamma blocked by GPU driver");
                }
            } else {
                set_error(st, "");
            }
        }
        // h_lbl_status is managed by set_status/TIMER_STATUS_CLEAR; no action needed here.
    } else {
        // Hide both labels so they don't bleed into other tabs.
        set_visible(st.h_lbl_status, false);
        set_visible(st.h_lbl_error, false);
    }

    set_visible(st.dimmer.h_lbl_dim_title,   tab == 1);
    set_visible(st.dimmer.h_lbl_dim_sub,     tab == 1);
    set_visible(st.dimmer.h_chk_taskbar_dim, tab == 1);

    let show_dim_controls = tab == 1 && st.dimmer.enabled;
    st.dimmer.grp_dim_controls.set_visible(show_dim_controls);

    st.system.group.set_visible(tab == 2);

    st.hotkeys.group.set_visible(tab == 3);
    st.debug.group.set_visible(tab == 4);
    st.about.group.set_visible(tab == 5);
    // h_btn_update / h_lbl_dl_status are outside the group — hide explicitly to prevent bleed.
    if tab != 5 {
        ShowWindow(st.about.h_btn_update,    SW_HIDE);
        ShowWindow(st.about.h_lbl_dl_status, SW_HIDE);
    } else if st.update_available {
        ShowWindow(st.about.h_btn_update, SW_SHOW);
        // h_lbl_dl_status stays hidden until a download is active.
    }
    // Re-layout on About so changelog is positioned even if update arrived while elsewhere.
    if tab == 5 {
        app_layout::apply(st, hwnd);
    }

    // TIMER_DEBUG_REFRESH runs only while tab 4 is active (--debug mode only).
    if is_debug_mode() {
        if tab == 4 {
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

/// True when launched with --debug. Readable from any thread.
static DEBUG_MODE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Set by tray Restart — triggers a re-spawn at the end of WM_DESTROY,
/// after the single-instance mutex is released.
pub static RESTART_ON_EXIT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Path of the freshly-installed exe, set by `on_download_done`.
/// main.rs spawns it after `run()` returns (mutex released by then).
pub static UPDATE_RELAUNCH_PATH: std::sync::Mutex<Option<String>> =
    std::sync::Mutex::new(None);

/// Path of the backup exe (`OledHelper_old.exe`), set by `on_download_done`.
/// main.rs deletes it after spawning the new process.
pub static OLD_EXE_PATH: std::sync::Mutex<Option<String>> =
    std::sync::Mutex::new(None);

pub fn is_debug_mode() -> bool {
    DEBUG_MODE.load(Ordering::Relaxed)
}

// ── Helper: map nav-button control ID → tab index ─────────────────────────────

fn nav_btn_to_tab(id: usize) -> Option<usize> {
    match id {
        IDC_NAV_BTN_0 => Some(0),
        IDC_NAV_BTN_1 => Some(1),
        IDC_NAV_BTN_5 => Some(2), // System
        IDC_NAV_BTN_2 => Some(3), // Hotkeys
        IDC_NAV_BTN_3 => Some(4), // Debug
        IDC_NAV_BTN_4 => Some(5), // About
        _ => None,
    }
}

// ── Tray menu helpers ─────────────────────────────────────────────────────────

/// Enumerate the refresh-rate combobox to build the tray profile list.
///
/// Returns `(items, active_index)` where each item is
/// `(combobox_index_str, display_label)`. The combobox index is stored as
/// the "key" so the apply handler can call `CB_SETCURSEL` directly without
/// re-scanning. Active index reflects the currently selected combobox entry.
unsafe fn tray_profile_list(st: &AppState) -> (Vec<(String, String)>, Option<usize>) {
    let h_ddl = st.crush.h_ddl_refresh;
    let count = SendMessageW(h_ddl, CB_GETCOUNT, WPARAM(0), LPARAM(0)).0;
    if count <= 0 { return (Vec::new(), None); }

    let cur_sel = SendMessageW(h_ddl, CB_GETCURSEL, WPARAM(0), LPARAM(0)).0;

    let mut profiles: Vec<(String, String)> = Vec::new();
    let mut active: Option<usize> = None;
    let mut buf = vec![0u16; 64];

    for i in 0..count as usize {
        let len = SendMessageW(h_ddl, CB_GETLBTEXTLEN, WPARAM(i), LPARAM(0)).0;
        if len <= 0 { continue; }
        buf.resize((len as usize) + 1, 0);
        SendMessageW(h_ddl, CB_GETLBTEXT, WPARAM(i), LPARAM(buf.as_mut_ptr() as isize));
        let label = String::from_utf16_lossy(&buf[..len as usize]);
        if i == cur_sel as usize {
            active = Some(profiles.len());
        }
        profiles.push((i.to_string(), label));
    }
    (profiles, active)
}

unsafe fn build_tray_menu_for_state(st: &AppState) -> windows::Win32::UI::WindowsAndMessaging::HMENU {
    let (profiles, active_profile) = tray_profile_list(st);
    tray::build_tray_menu(&tray::TrayMenuState {
        dimmer_on:      st.dimmer.enabled,
        hdr_on:         st.crush.hdr_panel.hdr_active,
        hdr_avail:      true, // no separate availability field; always show as enabled
        profiles:       &profiles,
        active_profile,
    })
}

/// Rebuild tray menu and update tooltip to reflect current state.
/// Call after any state change that affects the tray (dimmer toggle, crush change, HDR change).
unsafe fn refresh_tray_state(st: &mut AppState, hwnd: HWND) {
    let old = st.tray_menu;
    st.tray_menu = build_tray_menu_for_state(st);
    windows::Win32::UI::WindowsAndMessaging::DestroyMenu(old);
    let crush_val = crate::ui_drawing::get_slider_val(st.crush.h_sld_black);
    tray::update_tray_tooltip(hwnd, st.dimmer.enabled, crush_val, st.crush.hdr_panel.hdr_active);
}

// ── Command handler ───────────────────────────────────────────────────────────

unsafe fn on_command(st: &mut AppState, hwnd: HWND, id: usize, notify: u32, _ctrl: HWND) {
    match id {
        // STN_CLICKED: update label → open releases page.
        _ if _ctrl == st.about.h_lbl_check_info && notify == 0 => {
            st.about.on_open_releases();
        }
        IDC_ABOUT_BTN_UPDATE if notify == BN_CLICKED as u32 => {
            st.about.on_update_now(hwnd);
        }
        // STN_CLICKED: GitHub link label.
        _ if _ctrl == st.about.h_lbl_link && notify == 0 => {
            st.about.on_open_link();
        }
        IDC_SYS_BTN_TASKBAR_AUTOHIDE if notify == BN_CLICKED as u32 => {
            let msg = st.system.on_toggle_taskbar_autohide();
            set_status(st, msg, C_ACCENT);
        }
        IDC_SYS_DDL_SCREEN_TIMEOUT if notify == CBN_SELCHANGE as u32 => {
            let msg = st.system.on_screen_timeout_changed();
            set_status(st, &msg, C_ACCENT);
        }
        IDC_SYS_DDL_SLEEP_TIMEOUT if notify == CBN_SELCHANGE as u32 => {
            let msg = st.system.on_sleep_timeout_changed();
            set_status(st, &msg, C_ACCENT);
        }
        IDC_SYS_DDL_SCREENSAVER if notify == CBN_SELCHANGE as u32 => {
            let msg = st.system.on_screensaver_changed();
            set_status(st, &msg, C_ACCENT);
        }
        IDC_SYS_EDT_SS_TIMEOUT if notify == 0x0200 => { // EN_KILLFOCUS
            let msg = unsafe { st.system.commit_ss_timeout() };
            if !msg.is_empty() { set_status(st, &msg, C_ACCENT); }
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
                if tab == 2 { st.system.refresh(); }
                show_tab(st, hwnd);
                app_layout::apply(st, hwnd);
                for h in &st.h_nav_btn { InvalidateRect(*h, None, false); }
            }
        }
        IDC_BTN_HDR_TOGGLE if notify == BN_CLICKED as u32 => {
            toggle_hdr_via_shortcut();
            // Recheck needed — Windows takes time to settle after Win+Alt+B.
            st.crush.hdr_panel.schedule_hdr_recheck();
            // Tooltip updated on next TIMER_HDR tick when HDR state settles.
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
            refresh_tray_state(st, hwnd);
        }
        id if (id == IDC_CHK_SUPPRESS_FS as usize
               || id == IDC_CHK_SUPPRESS_AH as usize)
            && notify == BN_CLICKED as u32 =>
        {
            // Owner-drawn buttons don't auto-toggle — flip the flag, sync BM_SETCHECK, redraw.
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
            // Actual spawn happens at WM_DESTROY after the mutex is released.
            RESTART_ON_EXIT.store(true, Ordering::SeqCst);
            DestroyWindow(hwnd);
        }
        202 => { tray::remove_tray_icon(hwnd, st.tray_menu, &mut st.tray_added); DestroyWindow(hwnd); }

        // ── Tray quick-action toggles ─────────────────────────────────────────
        x if x == tray::TRAY_CMD_TOGGLE_DIMMER as usize => {
            let (msg, ok) = st.dimmer.on_checkbox_toggled(hwnd, &mut st.ini);
            show_tab(st, hwnd);
            if !msg.is_empty() {
                set_status(st, msg, if ok { C_ACCENT } else { C_WARN });
            }
            refresh_tray_state(st, hwnd);
        }
        x if x == tray::TRAY_CMD_TOGGLE_HDR as usize => {
            toggle_hdr_via_shortcut();
            st.crush.hdr_panel.schedule_hdr_recheck();
            // Tooltip updated on next TIMER_HDR tick when HDR state settles.
        }

        // ── Tray profile submenu ──────────────────────────────────────────────
        // Each item key is a combobox index (as a string). Selecting one sets
        // CB_SETCURSEL on the refresh-rate dropdown then fires the same code
        // path as the user choosing from the dropdown directly.
        x if x >= tray::TRAY_CMD_PROFILE_BASE as usize => {
            let (profiles, _) = tray_profile_list(st);
            let menu_idx = x - tray::TRAY_CMD_PROFILE_BASE as usize;
            if let Some((cb_idx_str, _label)) = profiles.get(menu_idx).cloned() {
                if let Ok(cb_idx) = cb_idx_str.parse::<usize>() {
                    // Select the item in the dropdown so apply_refresh_rate reads it.
                    SendMessageW(st.crush.h_ddl_refresh, CB_SETCURSEL,
                        WPARAM(cb_idx), LPARAM(0));
                    // Mirror IDC_DDL_REFRESH / CBN_SELCHANGE exactly.
                    match st.crush.apply_refresh_rate() {
                        Ok(hz) => {
                            rearm_render_timer(hwnd);
                            update_slider_anim_interval(hz as i32);
                            crate::tab_dimmer::set_fade_interval_from_hz(hz as i32);
                            if let Some((_, status)) =
                                st.crush.try_auto_load_profile_for_hz(hz as i32, &mut st.ini)
                            {
                                set_status(st, &status, C_ACCENT);
                            } else {
                                set_status(st, &format!("Refresh rate: {} Hz", hz), C_ACCENT);
                                apply_ramp(st, hwnd);
                            }
                            if st.dimmer.enabled {
                                st.dimmer.start_reposition_overlays();
                            }
                        }
                        Err(code) => {
                            set_error(st, &format!("Failed to set refresh rate (code {})", code));
                        }
                    }
                    refresh_tray_state(st, hwnd);
                }
            }
        }
       
        // Clear button — clear the row then auto-save + re-register.
        id if (IDC_HK_CLR_TOGGLE_DIMMER..=IDC_HK_CLR_INCREASE).contains(&id)
            || id == IDC_HK_CLR_TOGGLE_HDR
            && notify == BN_CLICKED as u32 =>
        {
            st.hotkeys.clear_row_by_id(id);
            st.hotkeys.save(&mut st.ini);
            register_hotkeys(&st.ini, &mut st.mouse_hotkeys, hwnd);
            set_status(st, "Hotkey cleared", C_ACCENT);
        }
        // Pill sent EN_CHANGE after key captured — auto-save + re-register.
        id if (IDC_HK_EDT_TOGGLE_DIMMER..=IDC_HK_EDT_INCREASE).contains(&id)
            || id == IDC_HK_EDT_TOGGLE_HDR
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

/// Show a transient status message; auto-hides after 4 s via TIMER_STATUS_CLEAR.
unsafe fn set_status(st: &mut AppState, text: &str, color: COLORREF) {
    st.status_color = color;
    set_window_text(st.h_lbl_status, text);
    set_visible(st.h_lbl_status, !text.is_empty());
    InvalidateRect(st.h_lbl_status, None, true);

    // Rearm auto-clear; KillTimer on a missing timer is a no-op.
    let hwnd = st.hwnd;
    if text.is_empty() {
        KillTimer(hwnd, TIMER_STATUS_CLEAR);
    } else {
        SetTimer(hwnd, TIMER_STATUS_CLEAR, 4000, None);
    }
}

/// Show or clear the persistent error label. Pass "" to hide.
unsafe fn set_error(st: &mut AppState, text: &str) {
    // Log every call in debug mode to track what clears the label.
    if is_debug_mode() {
        let tick = windows::Win32::System::SystemInformation::GetTickCount64();
        let detail = format!("[SET_ERROR] text={:?} tab={}", text, st.active_tab);
        if let Some(Ok(mut log)) = zorder_log().map(|m| m.lock()) {
            log.push(tick, ZLogKind::DisplayChange, text.is_empty() as u64, detail);
        }
    }

    set_window_text(st.h_lbl_error, text);
    set_visible(st.h_lbl_error, !text.is_empty());
    RedrawWindow(
        st.h_lbl_error, None, None,
        RDW_INVALIDATE | RDW_UPDATENOW | RDW_ERASE,
    );
}

// ── Render timer ──────────────────────────────────────────────────────────────

unsafe fn rearm_render_timer(hwnd: HWND) {
    SetTimer(hwnd, TIMER_RENDER, render_interval_ms(), None);
}

// ── Slider animation interval ─────────────────────────────────────────────────

/// Update slider animation interval from the current Hz (call on any Hz change).
/// Computes 1000/hz, clamped to [1, 16] ms.
fn update_slider_anim_interval(hz: i32) {
    let hz = hz.max(60) as u32;
    let ms = (1000 / hz).clamp(1, 16);
    ui_drawing::SLIDER_ANIM_INTERVAL_MS.store(ms, Ordering::Relaxed);
}