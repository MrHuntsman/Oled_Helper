// app_init.rs — AppState construction.
//
// Extracted from app.rs to keep the WndProc file focused on message dispatch.
// This file owns the one-time setup: creating all child controls, restoring
// persisted settings from INI, and arming the initial timers.

#![allow(non_snake_case, unused_must_use, unused_variables)]

use std::{path::PathBuf, ptr};

use windows::{
    core::*,
    Win32::{
        Foundation::*,
        Graphics::Gdi::{CreateSolidBrush, DeleteObject, HBRUSH, HGDIOBJ, InvalidateRect},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::*,
            HiDpi::GetDpiForWindow,
            Input::KeyboardAndMouse::*,
            WindowsAndMessaging::*,
        },
    },
};

use crate::{
    app::AppState,
    app_helpers::{apply_ramp, show_tab},
    constants::*,
    controls::ControlBuilder,
    hotkeys::register_hotkeys,
    profile_manager::ProfileManager,
    startup,
    tab_about::AboutTab,
    tab_crush::{CrushTab, get_current_hz, hz_section},
    tab_debug::DebugTab,
    tab_dimmer::DimmerTab,
    tab_hotkeys::HotkeysTab,
    tab_system::SystemTab,
    tray,
    ui_drawing::{
        self, install_action_btn_hover, make_font,
        hdr_toggle_subclass_proc, dimmer_toggle_subclass_proc,
        nav_btn_subclass_proc, SetWindowSubclass,
    },
    win32::set_text,
};

// IDC_BTN_HDR_TOGGLE is local to app.rs but needed here too.
const IDC_BTN_HDR_TOGGLE: usize = 150;

/// Build the full AppState from scratch.
/// Called once from WM_CREATE via `attach_state`.
pub unsafe fn create_state(hwnd: HWND, ini_path: PathBuf) -> AppState {
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

    let h_chk_startup     = cb.checkbox(w!("Launch with Windows"), IDC_CHK_STARTUP);
    SetWindowSubclass(h_chk_startup, Some(hdr_toggle_subclass_proc), 2, 0);
    let h_btn_quit        = cb.button(w!("Exit"),       IDC_BTN_QUIT);
    let h_btn_minimize    = cb.button(w!("Minimize"),   IDC_BTN_MINIMIZE);
    let h_btn_hdr_toggle  = cb.button(w!("HDR Toggle"), IDC_BTN_HDR_TOGGLE);
    SetWindowSubclass(h_btn_hdr_toggle, Some(hdr_toggle_subclass_proc), 1, 0);
    let h_lbl_status = cb.static_text(w!(""), SS_CENTER | SS_CENTERIMAGE);
    let h_lbl_error  = cb.static_text(w!(""), SS_CENTER | SS_CENTERIMAGE);

    // ── Separators ────────────────────────────────────────────────────────────
    let sep_style = WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_BLACKRECT);
    let h_sep_vert = CreateWindowExW(WS_EX_LEFT, w!("STATIC"), w!(""),
        sep_style, 0, 0, 1, 1, hwnd, HMENU(ptr::null_mut()), hinstance, None,
    ).unwrap_or_default();
    let mut h_sep_h = [HWND::default(); 4];
    for sep in h_sep_h.iter_mut() {
        *sep = CreateWindowExW(WS_EX_LEFT, w!("STATIC"), w!(""),
            sep_style, 0, 0, 1, 1, hwnd, HMENU(ptr::null_mut()), hinstance, None,
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

    let tray_menu = tray::build_tray_menu();
    let mut ini   = ProfileManager::new(&ini_path);

    // ── Tab sub-states ────────────────────────────────────────────────────────
    // h_sep_h[0..=2] visually belong to the crush tab; [3] is shared/bottom.
    let crush = CrushTab::new(
        hwnd, hinstance, dpi,
        _font_normal, _font_title, _font_bold_val,
        &mut ini, &h_sep_h,
    );

    let dimmer = DimmerTab::new(
        hwnd, hinstance, dpi,
        _font_normal, _font_title, _font_bold_val,
        &mut ini, hwnd,
    );
    // Restrict clicks to the pill area only (left-aligned geometry).
    SetWindowSubclass(dimmer.h_chk_taskbar_dim, Some(dimmer_toggle_subclass_proc), 3, 0);

    let hotkeys = HotkeysTab::new(hwnd, hinstance, dpi, _font_normal, _font_title, &mut ini);

    let debug      = DebugTab::new(hwnd, hinstance, dpi, _font_normal, _font_title);
    let mut system = SystemTab::new(hwnd, hinstance, dpi, _font_normal, _font_title);
    SetWindowSubclass(system.h_btn_taskbar_autohide, Some(hdr_toggle_subclass_proc), 5, 0);

    // ── Restore cursor-hide setting from INI ──────────────────────────────────
    // Initialise the idle clock immediately so the first timer tick doesn't fire
    // at t=0 (which would be 49 days of idle on a freshly booted machine).
    crate::tab_system::cursor_touch();
    {
        let secs: u32 = ini.read("Mouse", "CursorHideSeconds", "0")
            .parse().unwrap_or(0);
        let idx = crate::tab_system::cursor_hide_to_index(secs);
        system.cursor_hide_idx = idx;
        SendMessageW(system.h_ddl_cursor_hide, CB_SETCURSEL, WPARAM(idx), LPARAM(0));
        // Timer armed below after state is fully built.
    }

    let about = AboutTab::new(hwnd, hinstance, dpi, _font_normal, _font_title);

    // ── Hover tracking for owner-drawn buttons/checkboxes ─────────────────────
    install_action_btn_hover(crush.h_btn_toggle);
    install_action_btn_hover(h_btn_quit);
    install_action_btn_hover(h_btn_minimize);

    let nav_icons        = crate::nav_icons::NavIcons::load(dpi);
    let tab_header_icons = crate::nav_icons::TabHeaderIcons::load(dpi);

    let mut state = AppState {
        hwnd,
        crush, dimmer, system, hotkeys, debug, about,
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
        mouse_hotkeys: [0u32; 9],
        nvidia_cam_enabled: CrushTab::is_nvidia_cam_enabled(),
        ramp_dirty: false,
    };

    state.zorder_winevent_hooks = Some(crate::tab_dimmer::install_zorder_winevent_hooks());

    // ── Startup checkbox ──────────────────────────────────────────────────────
    state.chk_startup_state = startup::startup_registry_exists();
    if state.chk_startup_state {
        SendMessageW(state.h_chk_startup, BM_SETCHECK, WPARAM(1), LPARAM(0));
    }

    // ── Hz profile seed / restore ─────────────────────────────────────────────
    let hz  = get_current_hz();
    crate::tab_dimmer::set_fade_interval_from_hz(hz);
    let sec = hz_section(hz);
    if state.ini.read(&sec, "Black", "__x__") == "__x__" {
        let fallback = state.ini.read_int("_state", "Black", DEFAULT_BLACK)
            .clamp(0, MAX_BLACK);
        state.ini.write_int(&sec, "Black", fallback);
    }
    if let Some((_v, _status)) =
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
        state.crush.hdr_panel.update(saved_v);
    }

    show_tab(&mut state, hwnd);

    // Attach PNG icon painter to each tab's title label.
    ui_drawing::subclass_tab_header(state.crush.h_lbl_title,      state.tab_header_icons.crush);
    ui_drawing::subclass_tab_header(state.dimmer.h_lbl_dim_title,  state.tab_header_icons.dimmer);
    ui_drawing::subclass_tab_header(state.system.h_lbl_title,     state.tab_header_icons.system);
    ui_drawing::subclass_tab_header(state.hotkeys.h_lbl_title,    state.tab_header_icons.hotkeys);
    ui_drawing::subclass_tab_header(state.debug.h_lbl_title,      state.tab_header_icons.debug);
    ui_drawing::subclass_tab_header(state.about.h_lbl_title,      state.tab_header_icons.about);

    apply_ramp(&mut state, hwnd);

    tray::add_tray_icon(hwnd, hinstance, &mut state.tray_added);
    register_hotkeys(&state.ini, &mut state.mouse_hotkeys, hwnd);

    // Arm the cursor-hide timer if the persisted setting is active.
    if state.system.cursor_hide_idx > 0 {
        SetTimer(hwnd, TIMER_CURSOR_HIDE, 1000, None);
    }

    state
}