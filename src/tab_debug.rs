// tab_debug.rs — Live internal state and event logging.
// Purely informational — no persistent state or INI writes.

#![allow(non_snake_case, unused_variables, unused_mut, unused_must_use)]

use windows::{
    core::*,
    Win32::{
        Foundation::*,
        Graphics::Gdi::HFONT,
        System::SystemInformation::GetTickCount64,
        UI::{
            Input::KeyboardAndMouse::{GetFocus, RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, VK_1},
            WindowsAndMessaging::*,
        },
    },
};

use crate::{
    constants::*,
    controls::ControlBuilder,
    tab_dimmer::{DimmerTab, zorder_log},
    ui_drawing::{make_font, hdr_toggle_subclass_proc, SetWindowSubclass},
    win32::{set_text_fmt, ControlGroup},
};

// ── WH_MOUSE_LL hook ──────────────────────────────────────────────────────────

/// Raw HHOOK for the low-level mouse hook; 0 when not installed.
static MOUSE_HOOK: std::sync::atomic::AtomicIsize =
    std::sync::atomic::AtomicIsize::new(0);

/// Install a system-wide WH_MOUSE_LL hook.
/// Idempotent — safe to call when the hook is already active.
/// The hook proc posts `WM_MOUSE_CLICK_LOG` to the main window on every
/// LMB / RMB / MMB down event so the UI thread can do the Win32 lookups.
/// No-op when not running in debug mode — call sites don't need to check.
pub unsafe fn install_mouse_hook(main_hwnd: HWND) {
    if !crate::app::is_debug_mode() { return; }
    if MOUSE_HOOK.load(std::sync::atomic::Ordering::Relaxed) != 0 {
        return;
    }
    // Ensure MAIN_HWND_FOR_HOOK is set even if called before DimmerTab init.
    crate::tab_dimmer::register_main_hwnd(main_hwnd);

    let hook = SetWindowsHookExW(
        WH_MOUSE_LL,
        Some(ll_mouse_proc),
        None, // hMod = NULL is correct for in-process global hooks
        0,    // dwThreadId = 0 → all threads
    )
    .unwrap_or_default();
    MOUSE_HOOK.store(hook.0 as isize, std::sync::atomic::Ordering::Relaxed);
}

/// Remove mouse hook. Idempotent.
pub unsafe fn uninstall_mouse_hook() {
    let raw = MOUSE_HOOK.swap(0, std::sync::atomic::Ordering::Relaxed);
    if raw != 0 {
        let _ = UnhookWindowsHookEx(HHOOK(raw as *mut _));
    }
}

/// Registers debug hotkeys ('1' = force-raise). Idempotent.
pub unsafe fn install_debug_hotkeys(hwnd: HWND) {
    if !crate::app::is_debug_mode() { return; }
    let _ = RegisterHotKey(
        hwnd,
        crate::constants::HK_DEBUG_FORCE_RAISE,
        HOT_KEY_MODIFIERS(0x4000), // MOD_NOREPEAT
        VK_1.0 as u32,
    );
}

/// Unregister debug hotkeys. Idempotent.
pub unsafe fn uninstall_debug_hotkeys(hwnd: HWND) {
    let _ = UnregisterHotKey(hwnd, crate::constants::HK_DEBUG_FORCE_RAISE);
}

/// Hook proc: packs coordinates and posts message to UI thread.
unsafe extern "system" fn ll_mouse_proc(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code >= 0 {
        let is_down = matches!(
            wparam.0 as u32,
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN
        );
        if is_down {
            let ms = &*(lparam.0 as *const MSLLHOOKSTRUCT);
            let btn: usize = match wparam.0 as u32 {
                WM_LBUTTONDOWN => 0,
                WM_RBUTTONDOWN => 1,
                _              => 2, // MMB
            };
            let main_raw = crate::tab_dimmer::MAIN_HWND_FOR_HOOK
                .load(std::sync::atomic::Ordering::Relaxed);
            if main_raw != 0 {
                let packed: isize =
                    ((ms.pt.x as u32 as i64) << 32 | (ms.pt.y as u32 as i64)) as isize;
                let _ = PostMessageW(
                    HWND(main_raw as *mut _),
                    WM_MOUSE_CLICK_LOG,
                    WPARAM(btn),
                    LPARAM(packed),
                );
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

// ── Tab state ──────────────────────────────────────────────────────────────────

#[allow(dead_code)]
pub struct DebugTab {
    // ── Title ─────────────────────────────────────────────────────────────────
    pub h_lbl_title:         HWND,

    // ── "Dimmer State" section ────────────────────────────────────────────────
    pub h_lbl_sect_state:    HWND,
    pub h_sep_state:         HWND,

    pub h_lbl_fs_key:        HWND,
    pub h_lbl_fs_val:        HWND,

    pub h_lbl_ah_key:        HWND,
    pub h_lbl_ah_val:        HWND,

    pub h_lbl_alpha_key:     HWND,
    pub h_lbl_alpha_val:     HWND,

    pub h_lbl_target_key:    HWND,
    pub h_lbl_target_val:    HWND,

    pub h_lbl_overlays_key:  HWND,
    pub h_lbl_overlays_val:  HWND,

    /// 1-based Z-order position of each overlay (1 = topmost on desktop).
    pub h_lbl_zpos_key:      HWND,
    pub h_lbl_zpos_val:      HWND,

    /// 1-based Z-order position of the taskbar window(s) in the desktop Z-stack.
    pub h_lbl_taskbar_zpos_key: HWND,
    pub h_lbl_taskbar_zpos_val: HWND,

    // ── "Suppression" section ─────────────────────────────────────────────────
    pub h_lbl_sect_suppress: HWND,
    pub h_sep_suppress:      HWND,

    pub h_chk_suppress_fs:   HWND,
    pub h_chk_suppress_ah:   HWND,

    // ── "Event Log" section (Z-order events + mouse clicks, unified) ─────────
    pub h_lbl_sect_log:      HWND,
    pub h_sep_log:           HWND,
    /// Read-only multiline edit showing all debug events (newest at top).
    pub h_lst_zlog:          HWND,
    pub h_btn_log_clear:     HWND,

    /// How many log entries had been written the last time we refreshed.
    pub log_count_last: usize,

    /// All debug controls as a group so show/hide is one call.
    pub group: ControlGroup,
}

impl DebugTab {
    /// # Safety
    /// Must be called on the same thread that owns `parent`.
    pub unsafe fn new(
        parent:      HWND,
        hinstance:   HINSTANCE,
        dpi:         u32,
        font_normal: HFONT,
        font_title:  HFONT,
    ) -> Self {
        let cb = ControlBuilder { parent, hinstance, dpi, font: font_normal };

        // ── Tab title (16pt bold) ─────────────────────────────────────────────
        let h_lbl_title = cb.static_text(w!("Debug"), 0);
        SendMessageW(h_lbl_title, WM_SETFONT,
            WPARAM(font_title.0 as usize), LPARAM(1));

        // ── Section headings: 11pt bold — matches tab_crush / tab_hotkeys ─────
        let font_sect = crate::ui_drawing::make_font_cached(w!("Segoe UI"), 11, dpi, true);
        // font_sect is cached and reused across DPI changes.

        // ── "Dimmer State" section ────────────────────────────────────────────
        let h_lbl_sect_state = cb.static_text(w!("Dimmer State"), SS_NOPREFIX);
        SendMessageW(h_lbl_sect_state, WM_SETFONT,
            WPARAM(font_sect.0 as usize), LPARAM(1));
        let h_sep_state = cb.static_text(w!(""), SS_BLACKRECT);

        // Helper: one key label + one value label per row.
        let row = |key: PCWSTR| -> (HWND, HWND) {
            (cb.static_text(key, SS_NOPREFIX), cb.static_text(w!("—"), SS_NOPREFIX))
        };

        let (h_lbl_fs_key,       h_lbl_fs_val)       = row(w!("Fullscreen window active"));
        let (h_lbl_ah_key,       h_lbl_ah_val)        = row(w!("Taskbar set to auto-hide"));
        let (h_lbl_alpha_key,    h_lbl_alpha_val)     = row(w!("Overlay alpha (current)"));
        let (h_lbl_target_key,   h_lbl_target_val)    = row(w!("Overlay alpha (target)"));
        let (h_lbl_overlays_key, h_lbl_overlays_val)  = row(w!("Active overlay windows"));
        // Z-order position — new row showing where each overlay sits in the Z-stack.
        let (h_lbl_zpos_key,     h_lbl_zpos_val)      = row(w!("Overlay Z-pos (topmost band)"));
        // Taskbar Z-order — where Shell_TrayWnd sits within its own shell band.
        let (h_lbl_taskbar_zpos_key, h_lbl_taskbar_zpos_val) = row(w!("Taskbar Z-pos (shell band)"));

        // ── "Suppression" section ─────────────────────────────────────────────
        let h_lbl_sect_suppress = cb.static_text(w!("Suppression"), SS_NOPREFIX);
        SendMessageW(h_lbl_sect_suppress, WM_SETFONT,
            WPARAM(font_sect.0 as usize), LPARAM(1));
        let h_sep_suppress = cb.static_text(w!(""), SS_BLACKRECT);

        // Checkboxes labelled as actions — "Hide overlay when <condition>"
        // is unambiguous: checked = feature on.
        let h_chk_suppress_fs = cb.checkbox(w!("Hide overlay when fullscreen"),
            IDC_CHK_SUPPRESS_FS.into());
        let h_chk_suppress_ah = cb.checkbox(w!("Hide overlay when auto-hide"),
            IDC_CHK_SUPPRESS_AH.into());
        for h in [h_chk_suppress_fs, h_chk_suppress_ah] {
            SendMessageW(h, BM_SETCHECK, WPARAM(1), LPARAM(0)); // checked = enabled by default
            // Install hover-tracking subclass so TOGGLE_HOVER_PROP is set on
            // mouse enter/leave — draw_dark_button_full's checkbox branch reads it.
            // ref_data = 1 → left-aligned pill hit-test (same as dimmer toggle).
            SetWindowSubclass(h, Some(hdr_toggle_subclass_proc), 4, 1);
        }

        // ── "Event Log" section (Z-order events + mouse clicks, unified) ────────
        // The "Clear" button sits inline with the section heading so it is always
        // visible regardless of window height — the log listbox fills remaining
        // space below, so it may be clipped but the button never will be.
        let h_lbl_sect_log = cb.static_text(w!("Event Log"), SS_NOPREFIX);
        SendMessageW(h_lbl_sect_log, WM_SETFONT,
            WPARAM(font_sect.0 as usize), LPARAM(1));
        let h_btn_log_clear = cb.button(w!("Clear"), IDC_BTN_LOG_CLEAR);
        let h_sep_log = cb.static_text(w!(""), SS_BLACKRECT);

        // Read-only multiline edit: full Ctrl+A / Ctrl+C support out of the box.
        let h_lst_zlog = CreateWindowExW(
            WS_EX_CLIENTEDGE,
            w!("EDIT"),
            PCWSTR(std::ptr::null()),
            WS_CHILD | WS_VISIBLE | WS_VSCROLL
                | WINDOW_STYLE(ES_MULTILINE   as u32)
                | WINDOW_STYLE(ES_READONLY    as u32)
                | WINDOW_STYLE(ES_AUTOVSCROLL as u32),
            0, 0, 1, 1,
            parent, HMENU(IDC_LST_ZLOG as isize as *mut _), hinstance, None,
        ).unwrap_or_default();
        SendMessageW(h_lst_zlog, WM_SETFONT, WPARAM(font_normal.0 as usize), LPARAM(1));

        // ── Control group (for single-call show / hide) ───────────────────────
        let group = ControlGroup::new(vec![
            h_lbl_title,
            h_lbl_sect_state,    h_sep_state,
            h_lbl_fs_key,        h_lbl_fs_val,
            h_lbl_ah_key,        h_lbl_ah_val,
            h_lbl_alpha_key,     h_lbl_alpha_val,
            h_lbl_target_key,    h_lbl_target_val,
            h_lbl_overlays_key,  h_lbl_overlays_val,
            h_lbl_zpos_key,      h_lbl_zpos_val,
            h_lbl_taskbar_zpos_key, h_lbl_taskbar_zpos_val,
            h_lbl_sect_suppress, h_sep_suppress,
            h_chk_suppress_fs,   h_chk_suppress_ah,
            h_lbl_sect_log,      h_sep_log,
            h_lst_zlog,          h_btn_log_clear,
        ]);

        // Hidden by default — only shown when the debug tab is active.
        group.set_visible(false);

        Self {
            h_lbl_title,
            h_lbl_sect_state,    h_sep_state,
            h_lbl_fs_key,        h_lbl_fs_val,
            h_lbl_ah_key,        h_lbl_ah_val,
            h_lbl_alpha_key,     h_lbl_alpha_val,
            h_lbl_target_key,    h_lbl_target_val,
            h_lbl_overlays_key,  h_lbl_overlays_val,
            h_lbl_zpos_key,      h_lbl_zpos_val,
            h_lbl_taskbar_zpos_key, h_lbl_taskbar_zpos_val,
            h_lbl_sect_suppress, h_sep_suppress,
            h_chk_suppress_fs,
            h_chk_suppress_ah,
            h_lbl_sect_log,      h_sep_log,
            h_lst_zlog,
            h_btn_log_clear,
            log_count_last: 0,
            group,
        }
    }

    // ── Periodic refresh (called every 500 ms while the debug tab is active) ──

    /// Refresh all value labels from the current dimmer state.
    /// Called by `TIMER_DEBUG_REFRESH` (every 500 ms) when tab 3 is active.
    pub unsafe fn refresh(&mut self, dimmer: &DimmerTab) {
        let bool_str = |b: bool| if b { "YES" } else { "no" };

        // ── Dimmer state rows ─────────────────────────────────────────────────
        set_text_fmt(self.h_lbl_fs_val,
            format_args!("{}", bool_str(dimmer.fullscreen_active)));
        set_text_fmt(self.h_lbl_ah_val,
            format_args!("{}", bool_str(dimmer.taskbar_autohide)));
        set_text_fmt(self.h_lbl_alpha_val,
            format_args!("{:.1}  ({:.0}%)",
                dimmer.overlay_alpha_current,
                dimmer.overlay_alpha_current / 255.0 * 100.0));
        set_text_fmt(self.h_lbl_target_val,
            format_args!("{:.1}  ({:.0}%)",
                dimmer.overlay_alpha_target,
                dimmer.overlay_alpha_target / 255.0 * 100.0));
        set_text_fmt(self.h_lbl_overlays_val,
            format_args!("{}", dimmer.taskbar_overlays.len()));

        // ── Overlay Z-order position ──────────────────────────────────────────
        // Walk the desktop Z-order from HWND_TOP, counting every window we pass,
        // and report the 1-based position for each overlay HWND.
        // Position #1 means the overlay is the topmost window on the desktop —
        // the correct steady state.  A larger number means it has been pushed down.
        let positions: Vec<String> = dimmer.taskbar_overlays.iter().map(|&ov| {
            if ov.0.is_null() { return "—".to_string(); }
            let mut pos: usize = 0;
            let mut cur = GetTopWindow(None).unwrap_or_default();
            loop {
                if cur.0.is_null() { break; }
                pos += 1;
                if cur == ov { return format!("#{}", pos); }
                if pos > 8192 { break; } // safety guard
                cur = GetWindow(cur, GW_HWNDNEXT).unwrap_or_default();
            }
            "not found".to_string()
        }).collect();

        let zpos_text = if positions.is_empty() {
            "no overlays".to_string()
        } else {
            positions.join(", ")
        };
        set_text_fmt(self.h_lbl_zpos_val, format_args!("{}", zpos_text));

        // ── Taskbar Z-order position ──────────────────────────────────────────
        // Shell_TrayWnd lives in a separate HWND_TOPMOST band on Windows 11 and
        // does NOT appear in the GetTopWindow→GW_HWNDNEXT chain visible to our
        // process except when Quick Settings is open.
        // Instead: start FROM Shell_TrayWnd and walk GW_HWNDPREV to count how
        // many windows are above it — that count+1 is its 1-based Z-position.
        {
            let mut tb_parts: Vec<String> = Vec::new();

            // Primary taskbar.
            let mut tray_hwnds: Vec<(&str, HWND)> = Vec::new();
            if let Ok(h) = FindWindowW(w!("Shell_TrayWnd"), None) {
                if !h.0.is_null() { tray_hwnds.push(("primary", h)); }
            }
            // Secondary taskbars.
            unsafe extern "system" fn collect_secondary(hwnd: HWND, lparam: LPARAM) -> BOOL {
                let out = &mut *(lparam.0 as *mut Vec<(&str, HWND)>);
                let mut buf = [0u16; 64];
                let len = GetClassNameW(hwnd, &mut buf) as usize;
                if len == 22 && buf[..len].iter().zip(b"Shell_SecondaryTrayWnd")
                    .all(|(&a, &b)| a == b as u16)
                {
                    out.push(("secondary", hwnd));
                }
                BOOL(1)
            }
            EnumWindows(Some(collect_secondary),
                LPARAM(&mut tray_hwnds as *mut Vec<(&str, HWND)> as isize));

            for (_label, h) in &tray_hwnds {
                // Walk GW_HWNDPREV from the taskbar upward — each step is one
                // window above it.  Stop at 8192 as a safety guard.
                let mut above: usize = 0;
                let mut cur = GetWindow(*h, GW_HWNDPREV).unwrap_or_default();
                while !cur.0.is_null() && above <= 8192 {
                    above += 1;
                    cur = GetWindow(cur, GW_HWNDPREV).unwrap_or_default();
                }
                tb_parts.push(format!("#{}", above + 1));
            }

            let tb_zpos_text = if tray_hwnds.is_empty() {
                "no taskbar".to_string()
            } else {
                tb_parts.join(", ")
            };
            set_text_fmt(self.h_lbl_taskbar_zpos_val, format_args!("{}", tb_zpos_text));
        }

        // ── Z-order event log ─────────────────────────────────────────────────
        // Skip rebuild while the edit has focus — the user may be selecting text.
        if GetFocus() == self.h_lst_zlog {
            return;
        }

        // Grab a snapshot under the lock, then rebuild outside it.
        let (entries, total_count) = {
            if let Some(Ok(log)) = zorder_log().map(|m| m.lock()) {
                if log.count == self.log_count_last {
                    return; // nothing new — skip redraw entirely
                }
                (log.recent(128), log.count)
            } else {
                return;
            }
        };

        // Build one string with all entries, newest first, separated by CRLF.
        // Format per line:
        //   [+delta ms]  T+abs_ms  [SIGIL] body
        // where delta = ms since previous entry (shows gaps at a glance).
        let mut text = String::new();
        let n = entries.len();
        for (i, entry) in entries.iter().rev().enumerate() {
            if !text.is_empty() { text.push_str("\r\n"); }

            // Δ since the NEXT-older entry (the one after us in the rev iterator,
            // which is at index i+1 in the reversed view → entries[n-1-(i+1)]).
            let delta_str = if i + 1 < n {
                let older_tick = entries[n - 1 - (i + 1)].tick;
                let delta = entry.tick.saturating_sub(older_tick);
                format!("+{:>5}ms  ", delta)
            } else {
                "          ".to_owned() // oldest entry — no predecessor
            };

            // Use the rich detail string when available, fall back to kind label.
            let body = if entry.detail.is_empty() {
                entry.kind.label(entry.payload)
            } else {
                entry.detail.clone()
            };

            text.push_str(&format!("{}T+{}  {}",
                delta_str, entry.tick, body));
        }
        text.push('\0');
        let encoded: Vec<u16> = text.encode_utf16().collect();
        SetWindowTextW(self.h_lst_zlog, PCWSTR(encoded.as_ptr()));

        // Newest entry is at the top — scroll there so it's immediately visible.
        SendMessageW(self.h_lst_zlog, WM_VSCROLL, WPARAM(SB_TOP.0 as usize), LPARAM(0));

        self.log_count_last = total_count;
    }

    // ── Button handlers ───────────────────────────────────────────────────────

    /// Called when the "Clear" button is pressed — clears the unified event log.
    pub fn on_log_clear(&mut self) {
        if let Some(Ok(mut log)) = zorder_log().map(|m| m.lock()) {
            *log = crate::tab_dimmer::ZOrderLog::new();
        }
        self.log_count_last = 0;
        unsafe { SetWindowTextW(self.h_lst_zlog, w!("")); }
    }
}

// ── Free helpers (called from WndProc, not requiring &mut DebugTab) ───────────

/// Resolve the click target and push one entry into the unified event log.
/// Called on the UI thread from the `WM_MOUSE_CLICK_LOG` handler in app.rs.
/// The hook is only active while the debug tab is shown, so this is never
/// called in production (non-`--debug`) builds.
pub unsafe fn log_mouse_click(btn: usize, pt: POINT) {
    use crate::tab_dimmer::{zorder_log, ZLogKind};
    use windows::Win32::System::SystemInformation::GetTickCount64;

    let btn_name = match btn {
        0 => "LMB",
        1 => "RMB",
        _ => "MMB",
    };

    // ── Resolve the HWND under the cursor ─────────────────────────────────
    let target = WindowFromPoint(pt);

    // ── Window class name ─────────────────────────────────────────────────
    let mut cls_buf = [0u16; 128];
    let cls_len = GetClassNameW(target, &mut cls_buf) as usize;
    let cls = String::from_utf16_lossy(&cls_buf[..cls_len]);

    // ── Window title (capped at 50 chars) ─────────────────────────────────
    let title_cap = GetWindowTextLengthW(target) as usize;
    let mut title_buf = vec![0u16; title_cap.min(80) + 2];
    let got = GetWindowTextW(target, &mut title_buf) as usize;
    let title_raw = String::from_utf16_lossy(&title_buf[..got]);
    let title: std::borrow::Cow<str> = if title_raw.chars().count() > 50 {
        format!("{}…", title_raw.chars().take(50).collect::<String>()).into()
    } else {
        title_raw.as_str().into()
    };

    let tick = GetTickCount64();
    let packed: u64 = ((pt.x as u32 as u64) << 32) | (pt.y as u32 as u64);
    let detail = format!(
        "[MB] {} ({}, {})  hwnd={:#010x}  [{}]  \"{}\"",
        btn_name, pt.x, pt.y, target.0 as usize, cls, title,
    );

    if let Some(Ok(mut log)) = zorder_log().map(|m| m.lock()) {
        log.push(tick, ZLogKind::MouseClick, packed, detail);
    }
}