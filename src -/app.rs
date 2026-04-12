// app.rs — AppState definition, window entry point, and WndProc message dispatch.
//
// Per-tab logic lives in tab_*.rs.
// One-time state construction lives in app_init.rs.
// Shared runtime helpers (set_status, apply_ramp, show_tab, etc.) live in app_helpers.rs.
// Layout is in app_layout.rs. Painting helpers in ui_drawing.rs. Win32 utils in win32.rs.

#![allow(non_snake_case, clippy::too_many_lines, unused_variables,
         unused_mut, unused_assignments, unused_must_use)]

use std::{mem, path::PathBuf, ptr};

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
    app_helpers::{
        apply_ramp, maybe_restore_gamma,
        set_status, set_error,
        show_tab, nav_btn_to_tab,
        rearm_render_timer, update_slider_anim_interval,
        is_debug_mode, set_debug_mode, RESTART_ON_EXIT,
    },
    app_init::create_state,
    constants::*,
    hotkeys::{
        HK_TOGGLE_DIM, HK_TOGGLE_CRUSH, HK_HOLD_COMPARE,
        HK_DECREASE, HK_INCREASE, HK_DIM_DECREASE, HK_DIM_INCREASE, HK_TOGGLE_HDR,
        install_compare_hook, uninstall_compare_hook,
        uninstall_mouse_hook,
        parse_hotkey, register_hotkeys,
    },
    startup,
    tray,
    gamma_ramp,
    profile_manager::ProfileManager,
    tab_crush::{CrushTab, get_current_hz, render_interval_ms},
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
        set_window_text,
        nav_btn_subclass_proc,
        hdr_toggle_subclass_proc,
        SetWindowSubclass,
    },
    win32::{
        attach_state, borrow_state, detach_state,
        set_text,
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
    /// Suggested RECT from WM_DPICHANGED for deferred resize.
    static DEFERRED_DPI_RECT: Cell<RECT> = Cell::new(RECT::default());
}

// ── Per-window state ──────────────────────────────────────────────────────────

const IDC_BTN_HDR_TOGGLE: usize = 150;

pub struct AppState {
    #[allow(dead_code)]
    pub hwnd: HWND,

    // ── Tab sub-states ────────────────────────────────────────────────────────
    /// Black Crush Tweak tab (tab 0).
    pub crush:  CrushTab,
    /// Taskbar Dimmer tab (tab 1).
    pub dimmer: DimmerTab,
    /// System tab (tab 2).
    pub system: SystemTab,
    /// Hotkeys tab (tab 3).
    pub hotkeys: HotkeysTab,
    /// Debug tab (tab 4).
    pub debug: DebugTab,
    /// About tab (tab 5).
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
    pub h_nav_btn:        [HWND; 6],

    /// App icon used in the nav button for tab 0.
    pub h_app_icon:   HICON,
    /// Decoded PNG icons for nav buttons (None = use built-in glyph).
    pub nav_icons:    crate::nav_icons::NavIcons,
    /// Decoded PNG icons at 32px for tab header titles.
    pub tab_header_icons: crate::nav_icons::TabHeaderIcons,
    /// 0 = Black Crush Tweak, 1 = Taskbar Dimmer, 2 = Debug
    pub active_tab:   usize,

    // ── Shared brushes / fonts (kept alive for WM_PAINT lifetime) ────────────
    pub bg_brush:    HBRUSH,
    pub bg3_brush:   HBRUSH,
    pub sep_brush:   HBRUSH,

    pub _font_normal:   HFONT,
    pub _font_title:    HFONT,
    pub _font_bold_val: HFONT,

    pub tray_menu:   HMENU,
    pub tray_added:  bool,

    pub ini:               ProfileManager,
    pub status_color:      COLORREF,
    pub chk_startup_state: bool,
    pub layout_initialized: bool,

    /// WinEvent hooks for deferred taskbar overlay Z-order (foreground + minimize).
    pub zorder_winevent_hooks: Option<ZOrderWinEventHooks>,

    /// Mouse-button bindings that can't use RegisterHotKey.
    /// Indexed by HK_* constant; value is a MB_* sentinel (0 = unbound).
    pub mouse_hotkeys: [u32; 9],

    /// Cached result of `CrushTab::is_nvidia_cam_enabled()`.
    /// Refreshed at startup and on every TIMER_HDR tick (~2 s).
    /// Avoids a registry enumeration loop inside the hot `apply_ramp` path.
    pub nvidia_cam_enabled: bool,

    /// Debounce flag for black-level gamma application.
    pub ramp_dirty: bool,
}

impl AppState {
    /// Recreate fonts and update all child controls.
    /// Called during deferred DPI resize before layout reflow.
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
            self.system.h_lbl_sect_power,
            self.system.h_lbl_sect_screensaver,
            self.system.h_lbl_sect_mouse,
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

    // If launched via the Windows startup registry entry (--minimized flag),
    // skip showing the window — the tray icon is already added inside
    // create_state, so the app is fully accessible from the tray immediately.
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
            // Note: timeBeginPeriod(1) is NOT called here.  The 1 ms system
            // timer period was needed when TIMER_RENDER fired every ~7-16 ms
            // to pace the D3D Present call at VRR cadence.  Now that Present
            // uses SyncInterval=0 and TIMER_RENDER fires at 100 ms (only
            // doing work when render_dirty is set), the global timer resolution
            // change is unnecessary and was a known VRR flicker trigger.
            // timeBeginPeriod(1) remains available for slider animations via
            // the slider subclass, but is not set for the whole app lifetime.
            // Register for power setting change notifications for the two
            // timeout GUIDs so WM_POWERBROADCAST/PBT_POWERSETTINGCHANGE fires
            // whenever Windows Settings (or anything else) changes them.
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
            SetTimer(hwnd, TIMER_HDR,          2000, None);
            // TIMER_DEBUG_REFRESH is NOT started here.  It is armed by show_tab
            // when the user switches to the debug tab (tab 3) and killed when
            // they switch away — so it never runs in production (non-debug) builds
            // and never wakes the CPU while the debug tab is not visible.
            SetTimer(hwnd, TIMER_RENDER, render_interval_ms(), None);
            LRESULT(0)
        }
        
       // 1. DPI Change: Post resize message to avoid blocking display broadcast.
       // Synchronous layout here prevents WM_DISPLAYCHANGE from arriving promptly.
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

        // Deferred handler: executes resize after display broadcast settles.
        // If a reposition poll is active, it short-circuits here as the
        // taskbar rect is now stable.
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

        // 2. Resolution Change: display switched. Reposition overlays via polling.
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
        let (pw, ph) = client_size_of(st.crush.h_hdr_panel);  // ← moved before init_d3d
        st.crush.hdr_panel.init_d3d(st.crush.h_hdr_panel, pw as u32, ph as u32);
        // Always schedule a recheck after init_d3d.  DXGI colour-space data is
        // unreliable during early startup AND can be stale after the device was
        // suspended while hidden (covers both --minimized launches and every
        // subsequent tray restore).  The 10-tick window (~1 s) is self-limiting
        // and costs nothing when HDR status is already correct.
        st.crush.hdr_panel.schedule_hdr_recheck();
        st.crush.hdr_panel.update(get_slider_val(st.crush.h_sld_black));
        st.crush.update_sl_hint();
        st.crush.update_range_label();
        st.crush.populate_refresh_rates(&mut st.ini);
        update_slider_anim_interval(get_current_hz());
        if !st.layout_initialized {
            st.layout_initialized = true;
        }
        // Only restore gamma on subsequent shows (tray restores, session unlocks, etc.).
        // On the very first WM_SHOWWINDOW, create_state() has just called apply_ramp()
        // and the error label already reflects the true blocked/unblocked state.
        // Calling maybe_restore_gamma here would immediately re-evaluate and potentially
        // clear a freshly-set "gamma blocked" error before the user can ever see it.
        if !first_show {
            maybe_restore_gamma(st, hwnd);
        }

        // Only on first show — don't re-trigger on every tray restore
        if first_show {
            let args: Vec<String> = std::env::args().collect();
            let is_debug = args.iter().any(|a| a.eq_ignore_ascii_case("--debug"));
            set_debug_mode(is_debug);
            if !is_debug_mode() {
                ShowWindow(st.h_nav_btn[3], SW_HIDE);
                KillTimer(hwnd, TIMER_DEBUG_REFRESH);
            }
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
                        // Refresh nvidia_cam_enabled FIRST so every apply_ramp call
                        // in this tick uses an up-to-date value.  Previously this was
                        // done after the refresh_hdr_status block, meaning apply_ramp
                        // could run with a stale 'false' and clear a genuine NVIDIA
                        // error even though CAM had just been enabled.
                        st.nvidia_cam_enabled = CrushTab::is_nvidia_cam_enabled();
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
                        maybe_restore_gamma(st, hwnd);
                        
                        SetTimer(hwnd, TIMER_HDR, 2000, None);
                    }
                    TIMER_RENDER if ui_visible => {
                        if st.crush.hdr_panel.render_tick() {
                            // HDR status changed during a retry tick (e.g. after a toggle or display change).
                            st.crush.update_sl_hint();
                            st.crush.update_range_label();
                            if !st.crush.previewing { apply_ramp(st, hwnd); }
                            InvalidateRect(st.h_btn_hdr_toggle, None, true);
                        }
                    }
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
                        // Don't auto-clear warnings or errors — only transient accent messages.
                            if st.status_color == C_ACCENT {
                            set_status(st, "", C_ACCENT);
                        }
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
                        KillTimer(hwnd, TIMER_RAMP_APPLY);
                        if st.ramp_dirty {
                            st.ramp_dirty = false;
                            apply_ramp(st, hwnd);
                        }
                    }
                    TIMER_CURSOR_HIDE => {
                        st.system.on_cursor_hide_tick(hwnd);
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
                if di.hwndItem == st.crush.h_ddl_refresh
                    || di.hwndItem == st.system.h_ddl_screen_timeout
                    || di.hwndItem == st.system.h_ddl_sleep_timeout
                    || di.hwndItem == st.system.h_ddl_screensaver
                    || di.hwndItem == st.system.h_ddl_cursor_hide
                {
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
                } else if di.hwndItem == st.h_nav_btn[5] {
                    draw_nav_item(di, st.active_tab == 2,
                        GetFocus() == st.h_nav_btn[5],
                        !GetPropW(st.h_nav_btn[5], NAV_BTN_HOVER_PROP).0.is_null(),
                        HICON(ptr::null_mut()), "⚙",
                        st.nav_icons.system);
                } else if di.hwndItem == st.h_nav_btn[2] {
                    draw_nav_item(di, st.active_tab == 3,
                        GetFocus() == st.h_nav_btn[2],
                        !GetPropW(st.h_nav_btn[2], NAV_BTN_HOVER_PROP).0.is_null(),
                        HICON(ptr::null_mut()), "⌨",
                        st.nav_icons.hotkeys);
                } else if di.hwndItem == st.h_nav_btn[3] {
                    draw_nav_item(di, st.active_tab == 4,
                        GetFocus() == st.h_nav_btn[3],
                        !GetPropW(st.h_nav_btn[3], NAV_BTN_HOVER_PROP).0.is_null(),
                        HICON(ptr::null_mut()), "🐛",
                        st.nav_icons.debug);
                } else if di.hwndItem == st.h_nav_btn[4] {
                    draw_nav_item(di, st.active_tab == 5,
                        GetFocus() == st.h_nav_btn[4],
                        !GetPropW(st.h_nav_btn[4], NAV_BTN_HOVER_PROP).0.is_null(),
                        HICON(ptr::null_mut()), "ℹ",
                        st.nav_icons.about);
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
                    HK_TOGGLE_HDR => {
                        toggle_hdr_via_shortcut();
                        st.crush.hdr_panel.schedule_hdr_recheck();
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
                    // lparam points to a POWERBROADCAST_SETTING struct whose
                    // PowerSetting field is the GUID of the changed setting.
                    // Re-read whichever timeout changed and update its dropdown.
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

            WM_SETTINGCHANGE => {
        if let Some(st) = borrow_state::<AppState>(hwnd) {
            // wparam == SPI_SETSCREENSAVEACTIVE (0x0011) or SPI_SETSCREENSAVETIMEOUT (0x000F)
            // but Windows also sends wparam=0 for screensaver changes made via the
            // Settings app, so reload on any relevant code rather than matching exactly.
            let spi = wparam.0 as u32;
            const SPI_SETSCREENSAVEACTIVE:  u32 = 0x0011;
            const SPI_SETSCREENSAVETIMEOUT: u32 = 0x000F;
            if spi == SPI_SETSCREENSAVEACTIVE
                || spi == SPI_SETSCREENSAVETIMEOUT
                || spi == 0
            {
                st.system.reload_screensaver_state();
            }
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
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
                KillTimer(hwnd, TIMER_CURSOR_HIDE);
                // Restore system cursors if they were hidden at exit time.
                if st.system.cursor_is_hidden() {
                    crate::tab_system::restore_system_cursors();
                }
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
            for id in [HK_TOGGLE_DIM, HK_TOGGLE_CRUSH, HK_HOLD_COMPARE, HK_DECREASE, HK_INCREASE, HK_DIM_DECREASE, HK_DIM_INCREASE, HK_TOGGLE_HDR] {
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

        WM_NOTIFY => {
            // UDN_DELTAPOS fires *before* UDS_SETBUDDYINT updates the edit text.
            // Read the edit text directly so a manually-typed value is respected,
            // then apply iDelta relative to that — not the stale nm.iPos.
            let hdr = &*(lparam.0 as *const windows::Win32::UI::Controls::NMHDR);
            if hdr.code == windows::Win32::UI::Controls::UDN_DELTAPOS as u32 {
                #[repr(C)]
                struct NMUPDOWN { hdr: windows::Win32::UI::Controls::NMHDR, iPos: i32, iDelta: i32 }
                let nm = &*(lparam.0 as *const NMUPDOWN);
                if let Some(st) = borrow_state::<AppState>(hwnd) {
                    // Read whatever the user has typed into the edit box.
                    let mut buf = [0u16; 8];
                    let len = windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(
                        st.system.h_edt_ss_timeout, &mut buf) as usize;
                    let typed: i32 = String::from_utf16_lossy(&buf[..len])
                        .trim().parse().unwrap_or(nm.iPos);
                    let new_val = (typed + nm.iDelta).clamp(1, 999) as u32;
                    let msg = st.system.on_ss_timeout_set(new_val);
                    if !msg.is_empty() { set_status(st, &msg, C_ACCENT); }
                }
                // Return 1 to cancel the spin's own position update — we have
                // already applied it above. Without this, DefWindowProcW lets
                // the updown apply the delta a second time (double-increment).
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

        // Restore hidden cursor instantly on any mouse activity — WM_SETCURSOR
        // fires on the UI thread the moment the cursor moves over any window,
        // so this covers the whole screen without needing a hook.
        // The atomic check costs ~1 cycle when the cursor is already visible.
        WM_SETCURSOR => {
            if crate::tab_system::CURSOR_HIDDEN.load(std::sync::atomic::Ordering::Relaxed) {
                crate::tab_system::CURSOR_HIDDEN.store(false, std::sync::atomic::Ordering::Relaxed);
                if let Some(st) = borrow_state::<AppState>(hwnd) {
                    st.system.cursor_hidden = false;
                    crate::tab_system::restore_system_cursors();
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }

        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// ── WM_COMMAND dispatcher ─────────────────────────────────────────────────────
//
// Extracted from wnd_proc to keep the message-routing match readable.
// Called with the already-decoded (id, notify, ctrl) triple.

unsafe fn on_command(
    st:     &mut AppState,
    hwnd:   HWND,
    id:     usize,
    notify: u32,
    _ctrl:  HWND,
) {
    const CBN_SELCHANGE: u32 = 1;
    const EN_CHANGE: u32 = 0x0300;

    // ── Tray menu commands ────────────────────────────────────────────────────
    match id {
        200 => {
            // Show
            ShowWindow(hwnd, SW_SHOW);
            SetForegroundWindow(hwnd);
            return;
        }
        201 => {
            // Restart
            RESTART_ON_EXIT.store(true, std::sync::atomic::Ordering::SeqCst);
            DestroyWindow(hwnd);
            return;
        }
        202 => {
            // Exit
            DestroyWindow(hwnd);
            return;
        }
        _ => {}
    }

    // ── Shared footer buttons ─────────────────────────────────────────────────
    if id == IDC_BTN_QUIT {
        DestroyWindow(hwnd);
        return;
    }
    if id == IDC_BTN_MINIMIZE {
        ShowWindow(hwnd, windows::Win32::UI::WindowsAndMessaging::SW_HIDE);
        return;
    }
    if id == IDC_BTN_HDR_TOGGLE as usize {
        toggle_hdr_via_shortcut();
        st.crush.hdr_panel.schedule_hdr_recheck();
        return;
    }

    // ── Startup checkbox ──────────────────────────────────────────────────────
    if id == IDC_CHK_STARTUP {
    st.chk_startup_state = !st.chk_startup_state;
    let msg = startup::toggle_startup(st.chk_startup_state);
        SendMessageW(st.h_chk_startup,
            BM_SETCHECK, WPARAM(st.chk_startup_state as usize), LPARAM(0));
        InvalidateRect(st.h_chk_startup, None, false);
        set_status(st, msg, C_ACCENT);
        return;
    }

    // ── Navigation buttons ────────────────────────────────────────────────────
    if let Some(tab) = nav_btn_to_tab(id) {
        st.active_tab = tab;
        show_tab(st, hwnd);
        for &h in &st.h_nav_btn {
            InvalidateRect(h, None, false);
        }
        return;
    }

    // ── Black Crush tab ───────────────────────────────────────────────────────
   if id == IDC_DDL_REFRESH && notify == CBN_SELCHANGE {
    if !st.crush.suppress_load {
        match st.crush.apply_refresh_rate() {
            Ok(new_hz) => {
                let new_hz = new_hz as i32;
                update_slider_anim_interval(new_hz);
                crate::tab_dimmer::set_fade_interval_from_hz(new_hz);
               st.crush.populate_refresh_rates(&mut st.ini);
                if let Some((_v, status)) =
                    st.crush.try_auto_load_profile_for_hz(new_hz, &mut st.ini)
                {
                    set_status(st, &status, C_ACCENT);
                    apply_ramp(st, hwnd);
                }
                rearm_render_timer(hwnd);
            }
            Err(code) => {
                let msg = format!("Failed to set refresh rate (code {code})");
                set_status(st, &msg, C_WARN);
            }
        }
        SetTimer(hwnd, TIMER_SCROLL_REFRESH, 50, None);
    }
    return;
}

    // ── Taskbar Dimmer tab ────────────────────────────────────────────────────
    if id == IDC_CHK_TASKBAR_DIM {
        let (msg, ok) = st.dimmer.on_checkbox_toggled(hwnd, &mut st.ini);
        show_tab(st, hwnd);
        if !msg.is_empty() {
            set_status(st, msg, if ok { C_ACCENT } else { C_WARN });
        }
        return;
    }
    if id == IDC_BTN_DIM_DEFAULTS {
        let msg = st.dimmer.restore_defaults(hwnd, &mut st.ini);
        set_status(st, &msg, C_ACCENT);
        return;
    }

    // ── Debug tab ─────────────────────────────────────────────────────────────
    if id == IDC_BTN_LOG_CLEAR {
        st.debug.on_log_clear();
        return;
    }
    if id == IDC_CHK_SUPPRESS_FS as usize {
        st.dimmer.suppress_fs_enabled = !st.dimmer.suppress_fs_enabled;
        InvalidateRect(st.debug.h_chk_suppress_fs, None, false);
        return;
    }
    if id == IDC_CHK_SUPPRESS_AH as usize {
        st.dimmer.suppress_ah_enabled = !st.dimmer.suppress_ah_enabled;
        InvalidateRect(st.debug.h_chk_suppress_ah, None, false);
        return;
    }

    // ── About tab ─────────────────────────────────────────────────────────────
    if id == IDC_ABOUT_BTN_CHECK {
        st.about.on_check_updates();
        return;
    }

    // ── System tab ───────────────────────────────────────────────────────────
    if id == IDC_SYS_DDL_SCREEN_TIMEOUT && notify == CBN_SELCHANGE {
        let msg = st.system.on_screen_timeout_changed();
        if !msg.is_empty() { set_status(st, &msg, C_ACCENT); }
        return;
    }
    if id == IDC_SYS_DDL_SLEEP_TIMEOUT && notify == CBN_SELCHANGE {
        let msg = st.system.on_sleep_timeout_changed();
        if !msg.is_empty() { set_status(st, &msg, C_ACCENT); }
        return;
    }
    if id == IDC_SYS_DDL_SCREENSAVER && notify == CBN_SELCHANGE {
        let msg = st.system.on_screensaver_changed();
        if !msg.is_empty() { set_status(st, &msg, C_ACCENT); }
        return;
    }
    if id == IDC_SYS_DDL_CURSOR_HIDE && notify == CBN_SELCHANGE {
        let msg = st.system.on_cursor_hide_changed(hwnd);
        if !msg.is_empty() { set_status(st, msg, C_ACCENT); }
        // Arm or kill the cursor-hide timer based on new selection.
        if st.system.cursor_hide_idx > 0 {
            SetTimer(hwnd, TIMER_CURSOR_HIDE, 1000, None);
        } else {
            KillTimer(hwnd, TIMER_CURSOR_HIDE);
        }
        return;
    }
    if id == IDC_SYS_BTN_TASKBAR_AUTOHIDE {
        let msg = st.system.on_toggle_taskbar_autohide();
        InvalidateRect(st.system.h_btn_taskbar_autohide, None, false);
        if !msg.is_empty() { set_status(st, msg, C_ACCENT); }
        return;
    }

    // ── Hotkeys tab — clear buttons ───────────────────────────────────────────
    // The "×" clear buttons have IDs IDC_HK_CLR_*.
    // is_pill() matches edit HWNDs; we match clear-button IDs directly.
    let clr_ids = [
        IDC_HK_CLR_TOGGLE_DIMMER,
        IDC_HK_CLR_TOGGLE_CRUSH,
        IDC_HK_CLR_HOLD_COMPARE,
        IDC_HK_CLR_DECREASE,
        IDC_HK_CLR_INCREASE,
        IDC_HK_CLR_DIM_DECREASE,
        IDC_HK_CLR_DIM_INCREASE,
        IDC_HK_CLR_TOGGLE_HDR,
    ];
    if clr_ids.contains(&id) {
        st.hotkeys.clear_row_by_id(id);
        // clear_row_by_id fires notify_parent_changed which re-enters on_command
        // with EN_CHANGE, so save/re-register happens automatically.
        return;
    }

    // ── Hotkeys tab — EN_CHANGE (pill captured a new key, or clear was pressed)
    // The hotkey pill subclass fires notify_parent_changed() → EN_CHANGE on the
    // edit HWND.  Any EN_CHANGE from an IDC_HK_EDT_* ID triggers a full save +
    // re-register cycle.
    let edt_ids = [
        IDC_HK_EDT_TOGGLE_DIMMER,
        IDC_HK_EDT_TOGGLE_CRUSH,
        IDC_HK_EDT_HOLD_COMPARE,
        IDC_HK_EDT_DECREASE,
        IDC_HK_EDT_INCREASE,
        IDC_HK_EDT_DIM_DECREASE,
        IDC_HK_EDT_DIM_INCREASE,
        IDC_HK_EDT_TOGGLE_HDR,
    ];
    if edt_ids.contains(&id) && notify == EN_CHANGE {
        st.hotkeys.save(&mut st.ini);
        register_hotkeys(&st.ini, &mut st.mouse_hotkeys, hwnd);
        return;
    }
}