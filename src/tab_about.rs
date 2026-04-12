// tab_about.rs — About tab (tab 4).
//
// Displays app name, version, a GitHub link, and an automatic update check
// that fires when the app first shows.  The check hits
// https://api.github.com/repos/MrHuntsman/Oled_Helper/releases/latest on a
// background thread and posts WM_UPDATE_RESULT back to the window.
//
// Style follows the conventions established in tab_crush.rs / tab_hotkeys.rs:
//   • 16pt bold Segoe UI  — tab title  (font_title)
//   • 11pt bold Segoe UI  — section headings  (font_sect, cached)
//   • 10pt Segoe UI        — body labels / info  (font_normal, default)
//   • SS_BLACKRECT separators under every section heading
//   • SS_NOPREFIX on every static label that might contain '&'

#![allow(non_snake_case, unused_must_use)]

use windows::{
    core::*,
    Win32::{
        Foundation::*,
        Graphics::Gdi::HFONT,
        UI::{
            WindowsAndMessaging::*,
            Input::KeyboardAndMouse::EnableWindow,
        },
    },
};

use crate::{
    constants::*,
    controls::ControlBuilder,
    ui_drawing::make_font,
    win32::ControlGroup,
};

pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GITHUB_URL:  &str = "https://github.com/MrHuntsman/Oled_Helper";

/// Posted from the background thread to the main window once the check completes.
/// wparam = 0  → up-to-date
/// wparam = 1  → new version available; lparam = Box<String> pointer (tag_name), caller frees it
/// wparam = 2  → network / parse error
pub const WM_UPDATE_RESULT: u32 = WM_USER + 20;

// ── Tab state ──────────────────────────────────────────────────────────────────

pub struct AboutTab {
    // ── Title ─────────────────────────────────────────────────────────────────
    pub h_lbl_title:       HWND,

    // ── "About" section ───────────────────────────────────────────────────────
    pub h_lbl_sect_about:  HWND,
    pub h_sep_about:       HWND,
    pub h_lbl_version:     HWND,
    pub h_lbl_link:        HWND,

    // ── "Updates" section ─────────────────────────────────────────────────────
    pub h_lbl_sect_update: HWND,
    pub h_sep_update:      HWND,
    /// Shows "Checking…", "Up to date.", or "vX.Y available — click to download".
    /// When an update is found it is styled with C_ACCENT and has SS_NOTIFY so
    /// clicking it opens the releases page.
    pub h_lbl_check_info:  HWND,
    /// "Update Now" button — shown when an update is available.
    pub h_btn_update:      HWND,
    /// Shows download progress / result.
    pub h_lbl_dl_status:   HWND,

    // ── "Changelog" section ───────────────────────────────────────────────────
    pub h_lbl_sect_changelog: HWND,
    pub h_sep_changelog:      HWND,
    /// Multi-line label showing the release body text.
    pub h_lbl_changelog:      HWND,

    pub group: ControlGroup,

    /// Ensures the background check is only spawned once per process lifetime.
    check_started: bool,
}

impl AboutTab {
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
        let h_lbl_title = cb.static_text(w!("About"), 0);
        SendMessageW(h_lbl_title, WM_SETFONT,
            WPARAM(font_title.0 as usize), LPARAM(1));

        // ── Section headings: 11pt bold ───────────────────────────────────────
        let font_sect = crate::ui_drawing::make_font_cached(w!("Segoe UI"), 11, dpi, true);

        // ── "About" section ───────────────────────────────────────────────────
        let h_lbl_sect_about = cb.static_text(w!("Application"), SS_NOPREFIX);
        SendMessageW(h_lbl_sect_about, WM_SETFONT,
            WPARAM(font_sect.0 as usize), LPARAM(1));
        let h_sep_about = cb.static_text(w!(""), SS_BLACKRECT);

        let ver_w: Vec<u16> = format!("Version {APP_VERSION}\0")
            .encode_utf16().collect();
        let h_lbl_version = cb.static_text(PCWSTR(ver_w.as_ptr()), SS_NOPREFIX);

        // GitHub link — styled as a URL via WM_CTLCOLORSTATIC (accent colour).
        let link_w: Vec<u16> = format!("{GITHUB_URL}\0")
            .encode_utf16().collect();
        let h_lbl_link = cb.static_text(PCWSTR(link_w.as_ptr()), SS_NOPREFIX | SS_NOTIFY);

        // ── "Updates" section ─────────────────────────────────────────────────
        let h_lbl_sect_update = cb.static_text(w!("Updates"), SS_NOPREFIX);
        SendMessageW(h_lbl_sect_update, WM_SETFONT,
            WPARAM(font_sect.0 as usize), LPARAM(1));
        let h_sep_update = cb.static_text(w!(""), SS_BLACKRECT);

        // Status label — initially blank; filled once spawn_update_check completes.
        let h_lbl_check_info = cb.static_text(w!(""), SS_NOPREFIX);

        let h_btn_update   = cb.button(w!("Update Now"), IDC_ABOUT_BTN_UPDATE);
        let h_lbl_dl_status = cb.static_text(w!(""), SS_NOPREFIX);

        // ── "Changelog" section ───────────────────────────────────────────────
        let h_lbl_sect_changelog = cb.static_text(w!("Changelog"), SS_NOPREFIX);
        SendMessageW(h_lbl_sect_changelog, WM_SETFONT,
            WPARAM(font_sect.0 as usize), LPARAM(1));
        let h_sep_changelog = cb.static_text(w!(""), SS_BLACKRECT);
        // Multi-line label for release notes — SS_EDITCONTROL enables word wrap.
        // 0x2000 = SS_EDITCONTROL (not exported by windows-rs).
        let h_lbl_changelog = cb.static_text(w!(""), SS_NOPREFIX | 0x2000);

        let group = ControlGroup::new(vec![
            h_lbl_title,
            h_lbl_sect_about, h_sep_about,
            h_lbl_version,
            h_lbl_link,
            h_lbl_sect_update, h_sep_update,
            h_lbl_check_info,
            h_btn_update, h_lbl_dl_status,
            h_lbl_sect_changelog, h_sep_changelog,
            h_lbl_changelog,
        ]);

        // Hidden by default — only shown when tab 4 is active.
        group.set_visible(false);
        // Update button and download status hidden until update is found.
        unsafe {
            ShowWindow(h_btn_update,    SW_HIDE);
            ShowWindow(h_lbl_dl_status, SW_HIDE);
            ShowWindow(h_lbl_sect_changelog, SW_HIDE);
            ShowWindow(h_sep_changelog, SW_HIDE);
            ShowWindow(h_lbl_changelog, SW_HIDE);
        }

        Self {
            h_lbl_title,
            h_lbl_sect_about, h_sep_about,
            h_lbl_version,
            h_lbl_link,
            h_lbl_sect_update, h_sep_update,
            h_lbl_check_info,
            h_btn_update, h_lbl_dl_status,
            h_lbl_sect_changelog, h_sep_changelog,
            h_lbl_changelog,
            group,
            check_started: false,
        }
    }

    // ── Update check ──────────────────────────────────────────────────────────

    /// Call once (e.g. on first show / app startup).
    /// Spawns a background thread; result arrives as `WM_UPDATE_RESULT` on `hwnd`.
    /// Safe to call multiple times — subsequent calls are no-ops.
    pub fn spawn_update_check(&mut self, hwnd: HWND) {
        if self.check_started { return; }
        self.check_started = true;

        unsafe {
            let msg: Vec<u16> = "Checking for updates...\0".encode_utf16().collect();
            SetWindowTextW(self.h_lbl_check_info, PCWSTR(msg.as_ptr()));
        }

        // HWND is !Send; pass the raw pointer as usize across the thread boundary.
        let hwnd_raw = hwnd.0 as usize;
        std::thread::spawn(move || {
            let (wp, lp): (usize, isize) = match check_github_release() {
                Ok(Some(tag)) => {
                    let ptr = Box::into_raw(Box::new(tag)) as isize;
                    (1, ptr)
                }
                Ok(None) => (0, 0),
                Err(_)   => (2, 0),
            };
            unsafe {
                let hwnd = HWND(hwnd_raw as *mut std::ffi::c_void);
                PostMessageW(hwnd, WM_UPDATE_RESULT, WPARAM(wp), LPARAM(lp));
            }
        });
    }

    /// Call from the main `WndProc` when `msg == WM_UPDATE_RESULT`.
    pub unsafe fn on_update_result(&mut self, _hwnd: HWND, wparam: usize, lparam: isize) {
        match wparam {
            0 => {
                let msg: Vec<u16> = "Up to date.\0".encode_utf16().collect();
                SetWindowTextW(self.h_lbl_check_info, PCWSTR(msg.as_ptr()));
                let style = GetWindowLongW(self.h_lbl_check_info, GWL_STYLE) as u32;
                SetWindowLongW(self.h_lbl_check_info, GWL_STYLE,
                    (style & !SS_NOTIFY) as i32);
            }
            1 => {
                let release = *Box::from_raw(lparam as *mut ReleaseInfo);
                let msg: Vec<u16> = format!("{} available\0", release.tag)
                    .encode_utf16().collect();
                SetWindowTextW(self.h_lbl_check_info, PCWSTR(msg.as_ptr()));
                // Make label clickable — STN_CLICKED will call on_open_releases().
                let style = GetWindowLongW(self.h_lbl_check_info, GWL_STYLE) as u32;
                SetWindowLongW(self.h_lbl_check_info, GWL_STYLE,
                    (style | SS_NOTIFY) as i32);
                // Only make the button visible now if the About tab is currently
                // shown.  If another tab is active, the button stays hidden and
                // will be revealed by group.set_visible(true) when the user
                // navigates to the About tab — preventing it from bleeding over
                // other tabs' content.
                if IsWindowVisible(self.h_lbl_title).as_bool() {
                    ShowWindow(self.h_btn_update, SW_SHOW);
                }
                // Populate changelog if the release has body text.
                if !release.body.is_empty() {
                    let body_w: Vec<u16> = format!("{}\0", release.body)
                        .encode_utf16().collect();
                    SetWindowTextW(self.h_lbl_changelog, PCWSTR(body_w.as_ptr()));
                    // Only show changelog controls if the About tab is currently
                    // active — same guard used for h_btn_update above.  If another
                    // tab is active, group.set_visible(true) will reveal them when
                    // the user navigates to About, preventing bleed-through on startup.
                    if IsWindowVisible(self.h_lbl_title).as_bool() {
                        ShowWindow(self.h_lbl_sect_changelog, SW_SHOW);
                        ShowWindow(self.h_sep_changelog, SW_SHOW);
                        ShowWindow(self.h_lbl_changelog, SW_SHOW);
                    }
                }
            }
            _ => {
                let msg: Vec<u16> = "Update check failed.\0".encode_utf16().collect();
                SetWindowTextW(self.h_lbl_check_info, PCWSTR(msg.as_ptr()));
            }
        }
    }

    // ── Download / self-update ────────────────────────────────────────────────

    /// Called when the user clicks "Update Now".
    pub fn on_update_now(&mut self, hwnd: HWND) {
        unsafe {
            EnableWindow(self.h_btn_update, false);
            let msg: Vec<u16> = "Downloading...\0".encode_utf16().collect();
            SetWindowTextW(self.h_lbl_dl_status, PCWSTR(msg.as_ptr()));
            ShowWindow(self.h_lbl_dl_status, SW_SHOW);
        }

        let hwnd_raw = hwnd.0 as usize;
        std::thread::spawn(move || {
            let result = download_update(hwnd_raw);
            let wp = if result { 1usize } else { 0 };
            unsafe {
                let hwnd = HWND(hwnd_raw as *mut std::ffi::c_void);
                PostMessageW(hwnd, WM_DOWNLOAD_DONE, WPARAM(wp), LPARAM(0));
            }
        });
    }

    /// Called from WndProc on `WM_DOWNLOAD_PROGRESS`.
    pub unsafe fn on_download_progress(&self, received: usize, total: usize) {
        let text = if total > 0 {
            format!("Downloading... {}%", received * 100 / total)
        } else {
            format!("Downloading... {} KB", received / 1024)
        };
        let w: Vec<u16> = text.encode_utf16().collect();
        SetWindowTextW(self.h_lbl_dl_status, PCWSTR(w.as_ptr()));
    }

    /// Called from WndProc on `WM_DOWNLOAD_DONE`.
    /// On success: rename files and relaunch. On failure: re-enable button.
    pub unsafe fn on_download_done(&mut self, hwnd: HWND, success: bool) {
        if !success {
            let msg: Vec<u16> = "Download failed. Try again.".encode_utf16().collect();
            SetWindowTextW(self.h_lbl_dl_status, PCWSTR(msg.as_ptr()));
            EnableWindow(self.h_btn_update, true);
            return;
        }

        let msg: Vec<u16> = "Installing...".encode_utf16().collect();
        SetWindowTextW(self.h_lbl_dl_status, PCWSTR(msg.as_ptr()));

        // Perform the rename-swap on the UI thread (no file handles held by GDI etc.).
        match apply_update() {
            Ok((new_exe, old_exe)) => {
                // Store the new exe path so main.rs can spawn it *after*
                // run() returns and the single-instance named mutex is released.
                // Launching here (before the mutex drops) causes the new instance
                // to hit "already running" and exit immediately.
                if let Ok(mut guard) = crate::app::UPDATE_RELAUNCH_PATH.lock() {
                    *guard = Some(new_exe.to_string_lossy().into_owned());
                }
                // Store the old exe path so main.rs can delete it *after*
                // spawning the new process.  The old process knows exactly
                // where it put OledHelper_old.exe; the new process does not
                // need to guess via current_exe().
                if let Ok(mut guard) = crate::app::OLD_EXE_PATH.lock() {
                    *guard = Some(old_exe.to_string_lossy().into_owned());
                }
                // Destroy the window — triggers WM_DESTROY → PostQuitMessage,
                // which exits GetMessage and unwinds run() cleanly (tray removal,
                // GDI cleanup, mutex release) before main.rs spawns the new exe.
                DestroyWindow(hwnd);
            }
            Err(e) => {
                let msg: Vec<u16> = format!("Install failed: {e}").encode_utf16().collect();
                SetWindowTextW(self.h_lbl_dl_status, PCWSTR(msg.as_ptr()));
                EnableWindow(self.h_btn_update, true);
            }
        }
    }

    // ── Link handlers ─────────────────────────────────────────────────────────

    /// Opens the repo homepage (GitHub link label clicked).
    pub unsafe fn on_open_link(&self) {
        shell_open(GITHUB_URL);
    }

    /// Opens the releases page (update-available label clicked).
    pub unsafe fn on_open_releases(&self) {
        shell_open(&format!("{GITHUB_URL}/releases"));
    }
}

// ── GitHub release check (runs on background thread) ─────────────────────────

pub struct ReleaseInfo {
    pub tag:  String,
    pub body: String,
}

/// Returns `Ok(Some(info))` if a newer release exists, `Ok(None)` if up-to-date,
/// `Err(())` on any network or parse failure.
type StdResult<T, E> = std::result::Result<T, E>;

fn check_github_release() -> StdResult<Option<ReleaseInfo>, ()> {
    let url = "https://api.github.com/repos/MrHuntsman/Oled_Helper/releases/latest";

    let resp = minreq::get(url)
        .with_header("User-Agent", "Oled_Helper")
        .with_timeout(10)
        .send()
        .map_err(|_| ())?;

    // 404 = no releases published yet — not an error.
    if resp.status_code == 404 {
        return Ok(None);
    }
    if resp.status_code != 200 {
        return Err(());
    }

    let json = resp.as_str().map_err(|_| ())?;

    let tag = match extract_json_string(json, "tag_name") {
        Some(t) => t,
        None    => return Ok(None),
    };

    let tag_clean = tag.trim_start_matches('v');
    let cur_clean = APP_VERSION.trim_start_matches('v');

    if tag_clean != cur_clean {
        let body = extract_json_string(json, "body").unwrap_or_default();
        Ok(Some(ReleaseInfo { tag, body }))
    } else {
        Ok(None)
    }
}

/// Minimal JSON string field extractor — handles escaped characters, no serde needed.
fn extract_json_string(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let pos = json.find(&needle)?;
    let after = json[pos + needle.len()..].trim_start();
    let after = after.strip_prefix(':')?.trim_start();
    if after.starts_with("null") { return None; }
    let after = after.strip_prefix('"')?;
    let mut result = String::new();
    let mut chars = after.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"'  => break,
            '\\' => match chars.next() {
                Some('n')  => result.push('\n'),
                Some('r')  => result.push('\r'),
                Some('t')  => result.push('\t'),
                Some('"')  => result.push('"'),
                Some('\\') => result.push('\\'),
                Some(c)   => { result.push('\\'); result.push(c); }
                None      => break,
            },
            c => result.push(c),
        }
    }
    Some(result)
}

// ── Shell helper ──────────────────────────────────────────────────────────────

unsafe fn shell_open(url: &str) {
    let url_w: Vec<u16> = format!("{url}\0").encode_utf16().collect();
    windows::Win32::UI::Shell::ShellExecuteW(
        HWND(std::ptr::null_mut()),
        w!("open"),
        PCWSTR(url_w.as_ptr()),
        PCWSTR::null(),
        PCWSTR::null(),
        SW_SHOWNORMAL,
    );
}
// ── Self-update: download + rename swap ──────────────────────────────────────

/// Downloads the latest `OledHelper.exe` asset into `<exe_dir>/OledHelper_update.exe`.
/// Posts `WM_DOWNLOAD_PROGRESS` on each chunk. Returns true on success.
fn download_update(hwnd_raw: usize) -> bool {
    use std::io::Write;

    let url = concat!(
        "https://github.com/MrHuntsman/Oled_Helper",
        "/releases/latest/download/OledHelper.exe"
    );

    // Destination: same folder as the running exe.
    let exe_dir = match std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    {
        Some(d) => d,
        None    => return false,
    };
    let dest = exe_dir.join("OledHelper_update.exe");

    // Remove any leftover partial download.
    let _ = std::fs::remove_file(&dest);

    let resp = match minreq::get(url)
        .with_header("User-Agent", "Oled_Helper")
        .with_timeout(120)
        .send()
    {
        Ok(r) if r.status_code == 200 => r,
        _ => return false,
    };

    let bytes = resp.as_bytes();
    let total  = bytes.len();

    let mut file = match std::fs::File::create(&dest) {
        Ok(f)  => f,
        Err(_) => return false,
    };

    // Write in 64 KB chunks and report progress.
    const CHUNK: usize = 65536;
    let mut written = 0usize;
    for chunk in bytes.chunks(CHUNK) {
        if file.write_all(chunk).is_err() {
            return false;
        }
        written += chunk.len();
        unsafe {
            let hwnd = windows::Win32::Foundation::HWND(hwnd_raw as *mut std::ffi::c_void);
            windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                hwnd,
                WM_DOWNLOAD_PROGRESS,
                windows::Win32::Foundation::WPARAM(written),
                windows::Win32::Foundation::LPARAM(total as isize),
            );
        }
    }

    true
}

/// Renames the current exe to `OledHelper_old.exe` and the downloaded
/// `OledHelper_update.exe` to `OledHelper.exe`. Returns the path to the new exe.
fn apply_update() -> std::result::Result<(std::path::PathBuf, std::path::PathBuf), String> {
    let current = std::env::current_exe()
        .map_err(|e| e.to_string())?;
    let dir = current.parent()
        .ok_or_else(|| "cannot determine exe directory".to_string())?;

    let update  = dir.join("OledHelper_update.exe");
    let old_exe = dir.join("OledHelper_old.exe");
    let new_exe = dir.join("OledHelper.exe");

    if !update.exists() {
        return Err("OledHelper_update.exe not found".to_string());
    }

    // Remove any previous leftover backup.
    let _ = std::fs::remove_file(&old_exe);

    // Rename running exe → backup (allowed on Windows for running executables).
    std::fs::rename(&current, &old_exe)
        .map_err(|e| format!("cannot rename current exe: {e}"))?;

    // Rename downloaded → final name.
    std::fs::rename(&update, &new_exe)
        .map_err(|e| format!("cannot rename update: {e}"))?;

    Ok((new_exe, old_exe))
}