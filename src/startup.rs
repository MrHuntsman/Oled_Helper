// startup.rs — HKCU\...\Run registry helpers.
// Uses the Run key rather than a Startup-folder shortcut to avoid the
// Bearfoos.A!ml false-positive triggered by writing .lnk files.

// ── advapi32 bindings ─────────────────────────────────────────────────────────

#[link(name = "advapi32")]
extern "system" {
    fn RegOpenKeyExW(
        h_key:       isize,
        lp_sub_key:  *const u16,
        ul_options:  u32,
        sam_desired: u32,
        phk_result:  *mut isize,
    ) -> i32;
    fn RegSetValueExW(
        h_key:         isize,
        lp_value_name: *const u16,
        reserved:      u32,
        dw_type:       u32,
        lp_data:       *const u8,
        cb_data:       u32,
    ) -> i32;
    fn RegDeleteValueW(h_key: isize, lp_value_name: *const u16) -> i32;
    fn RegQueryValueExW(
        h_key:         isize,
        lp_value_name: *const u16,
        lp_reserved:   *mut u32,
        lp_type:       *mut u32,
        lp_data:       *mut u8,
        lpcb_data:     *mut u32,
    ) -> i32;
    fn RegCloseKey(h_key: isize) -> i32;
}

const HKCU:      isize = -2147483647isize; // 0x80000001
const KEY_READ:  u32   = 0x20019;
const KEY_WRITE: u32   = 0x20006;
const REG_SZ:    u32   = 1;

const RUN_KEY:    &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const VALUE_NAME: &str = "OledHelper";

// ── Helpers ───────────────────────────────────────────────────────────────────

fn reg_run_key_w(access: u32) -> Option<isize> {
    let sub: Vec<u16> = RUN_KEY.encode_utf16().chain([0]).collect();
    let mut hk: isize = 0;
    let rc = unsafe { RegOpenKeyExW(HKCU, sub.as_ptr(), 0, access, &mut hk) };
    if rc == 0 { Some(hk) } else { None }
}

fn value_name_w() -> Vec<u16> {
    VALUE_NAME.encode_utf16().chain([0]).collect()
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Returns `true` if the OledHelper Run value exists in HKCU.
pub fn startup_registry_exists() -> bool {
    let Some(hk) = reg_run_key_w(KEY_READ) else { return false };
    let name = value_name_w();
    let mut typ:  u32 = 0;
    let mut size: u32 = 0;
    let rc = unsafe {
        RegQueryValueExW(hk, name.as_ptr(), std::ptr::null_mut(),
                         &mut typ, std::ptr::null_mut(), &mut size)
    };
    unsafe { RegCloseKey(hk) };
    rc == 0
}

/// Returns `true` when launched with `--minimized` (i.e. via the Run entry).
///
/// If true, call `hdr_panel.schedule_hdr_recheck()` after `init_d3d`.
/// The display subsystem isn't fully up at startup, so `is_any_monitor_hdr()`
/// returns false even on HDR monitors; the deferred recheck fires on the first
/// `render_tick` after the window becomes visible, by which time HDR is reliable.
pub fn launched_minimized() -> bool {
    std::env::args().any(|a| a == "--minimized")
}

/// Adds or removes the HKCU Run entry. Returns a status string for the UI.
pub unsafe fn toggle_startup(enabled: bool) -> &'static str {
    if enabled {
        if let Ok(exe) = std::env::current_exe() {
            let cmd = format!("\"{}\" --minimized", exe.display());
            let cmd_w: Vec<u16> = cmd.encode_utf16().chain([0]).collect();
            if let Some(hk) = reg_run_key_w(KEY_WRITE) {
                let name = value_name_w();
                RegSetValueExW(
                    hk, name.as_ptr(), 0, REG_SZ,
                    cmd_w.as_ptr() as *const u8,
                    (cmd_w.len() * 2) as u32,
                );
                RegCloseKey(hk);
            }
        }
        "Added to Windows startup"
    } else {
        if let Some(hk) = reg_run_key_w(KEY_WRITE) {
            let name = value_name_w();
            RegDeleteValueW(hk, name.as_ptr());
            RegCloseKey(hk);
        }
        "Removed from Windows startup"
    }
}