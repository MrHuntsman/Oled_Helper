// tab_about.rs — About tab (tab 4).
//
// Shows app name, version, a GitHub link, and an automatic update check
// that fires on first show. The check hits the GitHub releases API on a
// background thread and posts WM_UPDATE_RESULT back to the window.
//
// Font / style conventions:
//   • 16pt bold Segoe UI  — tab title  (font_title)
//   • 11pt bold Segoe UI  — section headings  (font_sect, cached)
//   • 10pt Segoe UI       — body labels  (font_normal, default)
//   • SS_BLACKRECT separators under every section heading
//   • SS_NOPREFIX on any static that might contain '&'

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

/// Posted from the background thread once the update check completes.
/// wparam = 0  → up-to-date
/// wparam = 1  → new version available; lparam = Box<String> pointer (tag_name), caller frees
/// wparam = 2  → network / parse error
pub const WM_UPDATE_RESULT: u32 = WM_USER + 20;

// ── Tab state ─────────────────────────────────────────────────────────────────

pub struct AboutTab {
    // Title
    pub h_lbl_title:       HWND,

    // "About" section
    pub h_lbl_sect_about:  HWND,
    pub h_sep_about:       HWND,
    pub h_lbl_version:     HWND,
    pub h_lbl_link:        HWND,

    // "Updates" section
    pub h_lbl_sect_update: HWND,
    pub h_sep_update:      HWND,
    /// "Checking…", "Up to date.", or "vX.Y available". Styled with C_ACCENT +
    /// SS_NOTIFY when an update is found so clicking opens the releases page.
    pub h_lbl_check_info:  HWND,
    /// "Update Now" button — shown only when an update is available.
    pub h_btn_update:      HWND,
    /// Download progress / result.
    pub h_lbl_dl_status:   HWND,

    // "Changelog" section
    pub h_lbl_sect_changelog: HWND,
    pub h_sep_changelog:      HWND,
    /// Multi-line label showing the release body text.
    pub h_lbl_changelog:      HWND,

    pub group: ControlGroup,

    /// Guards against spawning the background check more than once.
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

        // Tab title (16pt bold)
        let h_lbl_title = cb.static_text(w!("About"), 0);
        SendMessageW(h_lbl_title, WM_SETFONT,
            WPARAM(font_title.0 as usize), LPARAM(1));

        // Section headings: 11pt bold
        let font_sect = crate::ui_drawing::make_font_cached(w!("Segoe UI"), 11, dpi, true);

        // "About" section
        let h_lbl_sect_about = cb.static_text(w!("Application"), SS_NOPREFIX);
        SendMessageW(h_lbl_sect_about, WM_SETFONT,
            WPARAM(font_sect.0 as usize), LPARAM(1));
        let h_sep_about = cb.static_text(w!(""), SS_BLACKRECT);

        let ver_w: Vec<u16> = format!("Version {APP_VERSION}\0").encode_utf16().collect();
        let h_lbl_version = cb.static_text(PCWSTR(ver_w.as_ptr()), SS_NOPREFIX);

        // GitHub link — accent-coloured via WM_CTLCOLORSTATIC.
        let link_w: Vec<u16> = format!("{GITHUB_URL}\0").encode_utf16().collect();
        let h_lbl_link = cb.static_text(PCWSTR(link_w.as_ptr()), SS_NOPREFIX | SS_NOTIFY);

        // "Updates" section
        let h_lbl_sect_update = cb.static_text(w!("Updates"), SS_NOPREFIX);
        SendMessageW(h_lbl_sect_update, WM_SETFONT,
            WPARAM(font_sect.0 as usize), LPARAM(1));
        let h_sep_update = cb.static_text(w!(""), SS_BLACKRECT);

        // Status label — blank until spawn_update_check completes.
        let h_lbl_check_info = cb.static_text(w!(""), SS_NOPREFIX);

        let h_btn_update    = cb.button(w!("Update Now"), IDC_ABOUT_BTN_UPDATE);
        let h_lbl_dl_status = cb.static_text(w!(""), SS_NOPREFIX);

        // "Changelog" section
        let h_lbl_sect_changelog = cb.static_text(w!("Changelog"), SS_NOPREFIX);
        SendMessageW(h_lbl_sect_changelog, WM_SETFONT,
            WPARAM(font_sect.0 as usize), LPARAM(1));
        let h_sep_changelog = cb.static_text(w!(""), SS_BLACKRECT);
        // 0x2000 = SS_EDITCONTROL (not exported by windows-rs) — enables word wrap.
        let h_lbl_changelog = cb.static_text(w!(""), SS_NOPREFIX | 0x2000);

        let group = ControlGroup::new(vec![
            h_lbl_title,
            h_lbl_sect_about, h_sep_about,
            h_lbl_version,
            h_lbl_link,
            h_lbl_sect_update, h_sep_update,
            h_lbl_check_info,
            // h_btn_update and h_lbl_dl_status are excluded from the group so
            // group.set_visible(true) never auto-shows them; shown manually in
            // on_update_result (wparam == 1 only).
            h_lbl_sect_changelog, h_sep_changelog,
            h_lbl_changelog,
        ]);

        // Hidden by default — shown when tab 4 is active.
        group.set_visible(false);
        // Update button, download status, and changelog hidden until needed.
        unsafe {
            ShowWindow(h_btn_update,         SW_HIDE);
            ShowWindow(h_lbl_dl_status,      SW_HIDE);
            ShowWindow(h_lbl_sect_changelog, SW_HIDE);
            ShowWindow(h_sep_changelog,      SW_HIDE);
            ShowWindow(h_lbl_changelog,      SW_HIDE);
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

    /// Spawns the background update check. No-op after the first call.
    /// Result arrives as `WM_UPDATE_RESULT` on `hwnd`.
    pub fn spawn_update_check(&mut self, hwnd: HWND) {
        if self.check_started { return; }
        self.check_started = true;

        unsafe {
            let msg: Vec<u16> = "Checking for updates...\0".encode_utf16().collect();
            SetWindowTextW(self.h_lbl_check_info, PCWSTR(msg.as_ptr()));
        }

        // HWND is !Send — pass the raw pointer as usize across the thread boundary.
        let hwnd_raw = hwnd.0 as usize;
        std::thread::spawn(move || {
            let (wp, lp): (usize, isize) = match check_github_release() {
                Ok(Some(tag)) => (1, Box::into_raw(Box::new(tag)) as isize),
                Ok(None)      => (0, 0),
                Err(_)        => (2, 0),
            };
            unsafe {
                let hwnd = HWND(hwnd_raw as *mut std::ffi::c_void);
                PostMessageW(hwnd, WM_UPDATE_RESULT, WPARAM(wp), LPARAM(lp));
            }
        });
    }

    /// Call from WndProc when `msg == WM_UPDATE_RESULT`.
    pub unsafe fn on_update_result(&mut self, _hwnd: HWND, wparam: usize, lparam: isize) {
        match wparam {
            0 => {
                let msg: Vec<u16> = "Up to date.\0".encode_utf16().collect();
                SetWindowTextW(self.h_lbl_check_info, PCWSTR(msg.as_ptr()));
                let style = GetWindowLongW(self.h_lbl_check_info, GWL_STYLE) as u32;
                SetWindowLongW(self.h_lbl_check_info, GWL_STYLE, (style & !SS_NOTIFY) as i32);
            }
            1 => {
                let release = *Box::from_raw(lparam as *mut ReleaseInfo);
                let msg: Vec<u16> = format!("{} available\0", release.tag)
                    .encode_utf16().collect();
                SetWindowTextW(self.h_lbl_check_info, PCWSTR(msg.as_ptr()));
                // Make label clickable — STN_CLICKED → on_open_releases().
                let style = GetWindowLongW(self.h_lbl_check_info, GWL_STYLE) as u32;
                SetWindowLongW(self.h_lbl_check_info, GWL_STYLE, (style | SS_NOTIFY) as i32);
                // Only show the button if the About tab is currently visible.
                // If another tab is active, group.set_visible(true) will reveal
                // it on navigation — prevents bleed-through on startup.
                if IsWindowVisible(self.h_lbl_title).as_bool() {
                    ShowWindow(self.h_btn_update, SW_SHOW);
                }
                if !release.body.is_empty() {
                    let body_w: Vec<u16> = format!("{}\0", release.body)
                        .encode_utf16().collect();
                    SetWindowTextW(self.h_lbl_changelog, PCWSTR(body_w.as_ptr()));
                    // Same visibility guard as h_btn_update above.
                    if IsWindowVisible(self.h_lbl_title).as_bool() {
                        ShowWindow(self.h_lbl_sect_changelog, SW_SHOW);
                        ShowWindow(self.h_sep_changelog,      SW_SHOW);
                        ShowWindow(self.h_lbl_changelog,      SW_SHOW);
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
            let (wp, lp): (usize, isize) = match download_update(hwnd_raw) {
                Ok(())   => (1, 0),
                Err(msg) => (0, Box::into_raw(Box::new(msg)) as isize),
            };
            unsafe {
                let hwnd = HWND(hwnd_raw as *mut std::ffi::c_void);
                PostMessageW(hwnd, WM_DOWNLOAD_DONE, WPARAM(wp), LPARAM(lp));
            }
        });
    }

    /// Call from WndProc on `WM_DOWNLOAD_PROGRESS`.
    pub unsafe fn on_download_progress(&self, received: usize, total: usize) {
        let text = if total > 0 {
            format!("Downloading... {}%", received * 100 / total)
        } else {
            format!("Downloading... {} KB", received / 1024)
        };
        let w: Vec<u16> = text.encode_utf16().collect();
        SetWindowTextW(self.h_lbl_dl_status, PCWSTR(w.as_ptr()));
    }

    /// Call from WndProc on `WM_DOWNLOAD_DONE`.
    /// wparam = 1 success, 0 failure; on failure lparam = Box<String> error (caller frees).
    /// On success: rename-swap and relaunch. On failure: show error and re-enable button.
    pub unsafe fn on_download_done(&mut self, hwnd: HWND, wparam: usize, lparam: isize) {
        if wparam == 0 {
            let reason = if lparam != 0 {
                *Box::from_raw(lparam as *mut String)
            } else {
                "Download failed.".to_string()
            };
            let msg: Vec<u16> = format!("{reason}\0").encode_utf16().collect();
            SetWindowTextW(self.h_lbl_dl_status, PCWSTR(msg.as_ptr()));
            EnableWindow(self.h_btn_update, true);
            return;
        }

        let msg: Vec<u16> = "Installing...".encode_utf16().collect();
        SetWindowTextW(self.h_lbl_dl_status, PCWSTR(msg.as_ptr()));

        // Rename-swap on the UI thread (no file handles held by GDI etc.).
        match apply_update() {
            Ok((new_exe, old_exe)) => {
                // Store new exe path so main.rs can spawn it *after* run() returns
                // and the single-instance mutex drops. Launching here would hit
                // "already running" and exit immediately.
                if let Ok(mut guard) = crate::app::UPDATE_RELAUNCH_PATH.lock() {
                    *guard = Some(new_exe.to_string_lossy().into_owned());
                }
                // Store old exe path so main.rs can delete it after spawning the
                // new process (the new process doesn't know where the old one was).
                if let Ok(mut guard) = crate::app::OLD_EXE_PATH.lock() {
                    *guard = Some(old_exe.to_string_lossy().into_owned());
                }
                // Destroy window → WM_DESTROY → PostQuitMessage → unwinds run()
                // cleanly (tray removal, GDI cleanup, mutex release).
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
    pub unsafe fn on_open_link(&self) { shell_open(GITHUB_URL); }

    /// Opens the releases page (update-available label clicked).
    pub unsafe fn on_open_releases(&self) { shell_open(&format!("{GITHUB_URL}/releases")); }
}

// ── GitHub release check (background thread) ─────────────────────────────────

pub struct ReleaseInfo {
    pub tag:  String,
    pub body: String,
}

/// `Ok(Some(info))` if newer release exists, `Ok(None)` if up-to-date, `Err(())` on failure.
type StdResult<T, E> = std::result::Result<T, E>;

fn check_github_release() -> StdResult<Option<ReleaseInfo>, ()> {
    let url = "https://api.github.com/repos/MrHuntsman/Oled_Helper/releases/latest";

    let resp = minreq::get(url)
        .with_header("User-Agent", "Oled_Helper")
        .with_timeout(10)
        .send()
        .map_err(|_| ())?;

    if resp.status_code == 404 { return Ok(None); } // no releases yet
    if resp.status_code != 200 { return Err(()); }

    let json = resp.as_str().map_err(|_| ())?;

    let tag = match extract_json_string(json, "tag_name") {
        Some(t) => t,
        None    => return Ok(None),
    };

    let tag_clean = tag.trim_start_matches('v');
    let cur_clean = APP_VERSION.trim_start_matches('v');

    let remote  = match SemVer::parse(tag_clean) { Some(v) => v, None => return Ok(None) };
    let current = match SemVer::parse(cur_clean)  { Some(v) => v, None => return Ok(None) };

    // Skip pre-release tags for stable builds.
    if remote.pre.is_some() && current.pre.is_none() { return Ok(None); }

    if remote > current {
        let body = extract_json_string(json, "body").unwrap_or_default();
        Ok(Some(ReleaseInfo { tag, body }))
    } else {
        Ok(None)
    }
}

// ── Minimal semver parser ─────────────────────────────────────────────────────
// Handles MAJOR.MINOR.PATCH[-pre] (e.g. -alpha.1, -beta.2, -rc.3).

#[derive(Eq, PartialEq)]
struct SemVer {
    major: u32,
    minor: u32,
    patch: u32,
    /// Raw pre-release string, e.g. "alpha.1". None = stable.
    pre: Option<String>,
}

impl SemVer {
    fn parse(s: &str) -> Option<Self> {
        let (core, pre) = match s.find('-') {
            Some(i) => (&s[..i], Some(s[i + 1..].to_string())),
            None    => (s, None),
        };
        let mut parts = core.splitn(3, '.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.parse().ok()?;
        Some(Self { major, minor, patch, pre })
    }

    /// Decomposes a pre-release string into (identifier_rank, numeric_suffix).
    /// e.g. "beta.1" → (2, 1), "rc.3" → (3, 3), stable → (4, 0)
    fn pre_rank(pre: &Option<String>) -> (u8, u32) {
        let s = match pre.as_deref() {
            None    => return (4, 0),
            Some(s) => s,
        };
        let (label, num) = match s.find('.') {
            Some(i) => (&s[..i], s[i + 1..].parse::<u32>().unwrap_or(0)),
            None    => (s, 0),
        };
        let rank = match label { "rc" => 3, "beta" => 2, "alpha" => 1, _ => 0 };
        (rank, num)
    }
}

impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering::Equal;
        let c = self.major.cmp(&other.major); if c != Equal { return c; }
        let c = self.minor.cmp(&other.minor); if c != Equal { return c; }
        let c = self.patch.cmp(&other.patch); if c != Equal { return c; }
        Self::pre_rank(&self.pre).cmp(&Self::pre_rank(&other.pre))
    }
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> { Some(self.cmp(other)) }
}

/// Minimal JSON string field extractor — no serde needed.
fn extract_json_string(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let pos    = json.find(&needle)?;
    let after  = json[pos + needle.len()..].trim_start();
    let after  = after.strip_prefix(':')?.trim_start();
    if after.starts_with("null") { return None; }
    let after  = after.strip_prefix('"')?;
    let mut result = String::new();
    let mut chars  = after.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"'  => break,
            '\\' => match chars.next() {
                Some('n')  => result.push('\n'),
                Some('r')  => result.push('\r'),
                Some('t')  => result.push('\t'),
                Some('"')  => result.push('"'),
                Some('\\') => result.push('\\'),
                Some(c)    => { result.push('\\'); result.push(c); }
                None       => break,
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

// ── Self-update: download + rename-swap ───────────────────────────────────────

/// Downloads the latest `OledHelper.exe` to `<exe_dir>/OledHelper_update.exe`.
/// Posts `WM_DOWNLOAD_PROGRESS` per chunk. Returns `Ok(())` or `Err(msg)`.
fn download_update(hwnd_raw: usize) -> std::result::Result<(), String> {
    use std::io::Write;

    let url = concat!(
        "https://github.com/MrHuntsman/Oled_Helper",
        "/releases/latest/download/OledHelper.exe"
    );

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .ok_or_else(|| "cannot determine exe directory".to_string())?;
    let dest = exe_dir.join("OledHelper_update.exe");

    let _ = std::fs::remove_file(&dest); // remove any leftover partial download

    let resp = minreq::get(url)
        .with_header("User-Agent", "Oled_Helper")
        .with_timeout(120)
        .send()
        .map_err(|e| format!("network error: {e}"))?;

    if resp.status_code != 200 {
        return Err(format!("unexpected HTTP {}", resp.status_code));
    }

    let bytes = resp.as_bytes();
    let total = bytes.len();

    // Sanity-check: valid PE starts with "MZ". Catches HTML error pages.
    if total < 2 || &bytes[..2] != b"MZ" {
        return Err(format!("not a valid executable ({total} bytes)"));
    }

    let mut file = std::fs::File::create(&dest)
        .map_err(|e| format!("cannot create temp file: {e}"))?;

    // Write in 64 KB chunks and report progress.
    const CHUNK: usize = 65536;
    let mut written = 0usize;
    for chunk in bytes.chunks(CHUNK) {
        file.write_all(chunk).map_err(|e| format!("write error: {e}"))?;
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

    file.flush().map_err(|e| format!("flush error: {e}"))?;
    file.sync_all().map_err(|e| format!("sync error: {e}"))?;
    drop(file);

    Ok(())
}

/// Renames the current exe → `OledHelper_old.exe` and the downloaded
/// `OledHelper_update.exe` → `OledHelper.exe`. Returns `(new_path, old_path)`.
fn apply_update() -> std::result::Result<(std::path::PathBuf, std::path::PathBuf), String> {
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    // Strip \\?\ prefix — CreateProcess (std::process::Command) rejects it.
    let current = strip_unc_prefix(current);

    let dir     = current.parent().ok_or_else(|| "cannot determine exe directory".to_string())?;
    let update  = dir.join("OledHelper_update.exe");
    let old_exe = dir.join("OledHelper_old.exe");
    let new_exe = dir.join("OledHelper.exe");

    if !update.exists() { return Err("OledHelper_update.exe not found".to_string()); }

    let _ = std::fs::remove_file(&old_exe); // remove previous backup if any

    // Renaming a running exe is allowed on Windows.
    std::fs::rename(&current, &old_exe).map_err(|e| format!("cannot rename current exe: {e}"))?;
    std::fs::rename(&update,  &new_exe).map_err(|e| format!("cannot rename update: {e}"))?;

    Ok((new_exe, old_exe))
}

/// Strips the `\\?\` extended-length prefix added by `current_exe()`.
/// `std::process::Command` (CreateProcess) does not accept `\\?\` paths.
fn strip_unc_prefix(path: std::path::PathBuf) -> std::path::PathBuf {
    let s = path.to_string_lossy();
    if let Some(stripped) = s.strip_prefix(r"\\?\") {
        std::path::PathBuf::from(stripped)
    } else {
        path
    }
}