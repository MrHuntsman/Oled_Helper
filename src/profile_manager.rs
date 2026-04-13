// profile_manager.rs — ordered INI file manager.
// Preserves section/key insertion order, matching Win32 WritePrivateProfileString behaviour.

use std::{
    fs,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
};

// ── Storage ───────────────────────────────────────────────────────────────────

#[allow(dead_code)]
struct Section {
    name:    String,
    entries: Vec<(String, String)>, // ordered key-value pairs
}

#[allow(dead_code)]
impl Section {
    fn new(name: &str) -> Self {
        Self { name: name.to_owned(), entries: Vec::new() }
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    fn set(&mut self, key: &str, value: &str) {
        if let Some(pos) = self.entries.iter().position(|(k, _)| k.eq_ignore_ascii_case(key)) {
            self.entries[pos].1 = value.to_owned();
        } else {
            self.entries.push((key.to_owned(), value.to_owned()));
        }
    }

    fn delete_key(&mut self, key: &str) {
        self.entries.retain(|(k, _)| !k.eq_ignore_ascii_case(key));
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Ordered INI file manager — direct functional port of `ProfileManager.cs`.
pub struct ProfileManager {
    path:     PathBuf,
    sections: Vec<Section>,
}

#[allow(dead_code)]
impl ProfileManager {
    /// Opens (or creates) the INI file at `path`.
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        let path = path.as_ref().to_owned();
        let sections = if path.exists() {
            parse_ini(&path).unwrap_or_default()
        } else {
            fs::write(&path, "").ok();
            Vec::new()
        };
        Self { path, sections }
    }

    // ── Read / write ─────────────────────────────────────────────────────────

    pub fn write(&mut self, section: &str, key: &str, value: &str) {
        if let Some(s) = self.sections.iter_mut().find(|s| s.name.eq_ignore_ascii_case(section)) {
            s.set(key, value);
        } else {
            let mut s = Section::new(section);
            s.set(key, value);
            self.sections.push(s);
        }
        self.flush();
    }

    pub fn read(&self, section: &str, key: &str, default: &str) -> String {
        self.sections
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(section))
            .and_then(|s| s.get(key))
            .unwrap_or(default)
            .to_owned()
    }

    pub fn delete_section(&mut self, section: &str) {
        self.sections.retain(|s| !s.name.eq_ignore_ascii_case(section));
        self.flush();
    }

    /// Returns all section names in insertion order.
    pub fn get_sections(&self) -> Vec<String> {
        self.sections.iter().map(|s| s.name.clone()).collect()
    }

    // ── Typed helpers ─────────────────────────────────────────────────────────

    pub fn read_int(&self, section: &str, key: &str, fallback: i32) -> i32 {
        self.read(section, key, "").parse::<i32>().unwrap_or(fallback)
    }

    pub fn write_int(&mut self, section: &str, key: &str, value: i32) {
        self.write(section, key, &value.to_string());
    }

    /// Returns the first profile section whose `LinkedHz` matches `hz`, skipping `skip`.
    pub fn find_profile_for_hz(&self, hz: i32, skip: &[&str]) -> Option<String> {
        for s in &self.sections {
            if skip.iter().any(|sk| sk.eq_ignore_ascii_case(&s.name)) { continue; }
            let linked = s.get("LinkedHz").and_then(|v| v.parse::<i32>().ok()).unwrap_or(-1);
            if linked == hz { return Some(s.name.clone()); }
        }
        None
    }

    pub fn delete_key(&mut self, section: &str, key: &str) {
        if let Some(s) = self.sections.iter_mut().find(|s| s.name.eq_ignore_ascii_case(section)) {
            s.delete_key(key);
        }
        self.flush();
    }

    // ── Internal ──────────────────────────────────────────────────────────────

    fn flush(&self) {
        if let Ok(mut f) = fs::File::create(&self.path) {
            for section in self.sorted_sections() {
                let _ = writeln!(f, "[{}]", section.name);
                for (k, v) in &section.entries {
                    let _ = writeln!(f, "{}={}", k, v);
                }
                let _ = writeln!(f);
            }
        }
    }

    /// Stable write order:
    ///   1. `[_state]`
    ///   2. `[hz_*]` — numerically descending
    ///   3. `[Hotkeys]`
    ///   4. `[Mouse]`
    ///   5. Everything else — original insertion order
    fn sorted_sections(&self) -> Vec<&Section> {
        let keys: Vec<(u8, i32, usize)> = self.sections
            .iter()
            .enumerate()
            .map(|(idx, s)| {
                let lower = s.name.to_ascii_lowercase();
                if lower == "_state" {
                    (0u8, 0i32, idx)
                } else if let Some(rest) = lower.strip_prefix("hz_") {
                    let hz = rest.parse::<i32>().unwrap_or(i32::MAX);
                    (1u8, -hz, idx)
                } else if lower == "hotkeys" {
                    (2u8, 0i32, idx)
                } else if lower == "mouse" {
                    (3u8, 0i32, idx)
                } else {
                    (4u8, 0i32, idx)
                }
            })
            .collect();

        let mut order: Vec<usize> = (0..self.sections.len()).collect();
        order.sort_by_key(|&i| keys[i]);
        order.iter().map(|&i| &self.sections[i]).collect()
    }
}

// ── Parser ────────────────────────────────────────────────────────────────────

fn parse_ini(path: &Path) -> io::Result<Vec<Section>> {
    let file   = fs::File::open(path)?;
    let reader = io::BufReader::new(file);
    let mut sections: Vec<Section> = Vec::new();
    let mut current: Option<Section> = None;

    for line in reader.lines() {
        let line    = line?;
        let trimmed = line.trim();

        if trimmed.starts_with(';') || trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if let Some(prev) = current.take() { sections.push(prev); }
            let name = trimmed[1..trimmed.len() - 1].trim().to_owned();
            current = Some(Section::new(&name));
        } else if let Some(eq) = trimmed.find('=') {
            if let Some(ref mut sec) = current {
                let key   = trimmed[..eq].trim().to_owned();
                let value = trimmed[eq + 1..].trim().to_owned();
                sec.entries.push((key, value));
            }
        }
    }

    if let Some(last) = current { sections.push(last); }

    Ok(sections)
}