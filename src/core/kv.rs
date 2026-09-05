//! Tiny line-oriented `key value` config format.
//!
//! Used for settings, key bindings, progression and the server browser history.
//! Hand-rolled deliberately: it is a few hundred bytes of code instead of a
//! serialisation dependency, it never panics on malformed input, and a corrupt
//! file degrades to defaults rather than refusing to launch.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Default, Clone)]
pub struct Kv {
    map: HashMap<String, String>,
    /// Preserved so unknown keys written by a newer build survive a round trip.
    order: Vec<String>,
}

impl Kv {
    pub fn new() -> Kv { Kv::default() }

    pub fn parse(text: &str) -> Kv {
        let mut kv = Kv::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("//") { continue; }
            let (k, v) = match line.split_once(char::is_whitespace) {
                Some((k, v)) => (k.trim(), v.trim()),
                None => (line, ""),
            };
            if k.is_empty() { continue; }
            kv.set(k, v);
        }
        kv
    }

    pub fn load(path: &Path) -> Kv {
        match std::fs::read_to_string(path) {
            Ok(t) => Kv::parse(&t),
            Err(_) => Kv::default(),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut out = String::with_capacity(self.order.len() * 32);
        out.push_str("# HARDPOINT configuration - edited by the game, safe to hand-edit\n");
        for k in &self.order {
            if let Some(v) = self.map.get(k) {
                out.push_str(k);
                out.push(' ');
                out.push_str(v);
                out.push('\n');
            }
        }
        // Write to a temp file then rename, so a crash mid-save cannot corrupt
        // the player's settings.
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, out)?;
        std::fs::rename(&tmp, path)
    }

    pub fn set(&mut self, key: &str, value: impl AsRef<str>) {
        let key = key.to_string();
        if !self.map.contains_key(&key) { self.order.push(key.clone()); }
        self.map.insert(key, value.as_ref().to_string());
    }

    pub fn set_f32(&mut self, key: &str, v: f32) { self.set(key, format!("{:.4}", v)); }
    pub fn set_i32(&mut self, key: &str, v: i32) { self.set(key, v.to_string()); }
    pub fn set_bool(&mut self, key: &str, v: bool) { self.set(key, if v { "1" } else { "0" }); }

    pub fn get(&self, key: &str) -> Option<&str> { self.map.get(key).map(|s| s.as_str()) }

    pub fn str_or<'a>(&'a self, key: &str, def: &'a str) -> &'a str {
        self.get(key).filter(|s| !s.is_empty()).unwrap_or(def)
    }

    pub fn f32_or(&self, key: &str, def: f32) -> f32 {
        self.get(key).and_then(|s| s.parse().ok()).filter(|v: &f32| v.is_finite()).unwrap_or(def)
    }

    pub fn i32_or(&self, key: &str, def: i32) -> i32 {
        self.get(key).and_then(|s| s.parse().ok()).unwrap_or(def)
    }

    pub fn u32_or(&self, key: &str, def: u32) -> u32 {
        self.get(key).and_then(|s| s.parse().ok()).unwrap_or(def)
    }

    pub fn bool_or(&self, key: &str, def: bool) -> bool {
        match self.get(key) {
            Some("1") | Some("true") | Some("yes") | Some("on") => true,
            Some("0") | Some("false") | Some("no") | Some("off") => false,
            _ => def,
        }
    }

    /// Values stored as a `|`-separated list, for things like recent servers.
    pub fn list(&self, key: &str) -> Vec<&str> {
        match self.get(key) {
            Some(s) if !s.is_empty() => s.split('|').filter(|p| !p.is_empty()).collect(),
            _ => Vec::new(),
        }
    }

    pub fn set_list(&mut self, key: &str, items: &[String]) {
        self.set(key, items.join("|"));
    }

    pub fn keys_with_prefix(&self, prefix: &str) -> Vec<(&str, &str)> {
        let mut v: Vec<(&str, &str)> = self.map.iter()
            .filter(|(k, _)| k.starts_with(prefix))
            .map(|(k, val)| (k.as_str(), val.as_str()))
            .collect();
        v.sort_unstable_by_key(|(k, _)| *k);
        v
    }
}

/// Per-user data directory, following the XDG spec on Linux with sane
/// fallbacks. Never panics: if every candidate fails we use the working
/// directory so the game still runs (settings simply will not persist).
pub fn data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("HARDPOINT_DATA_DIR") {
        if !d.is_empty() { return PathBuf::from(d); }
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(d) = std::env::var("APPDATA") {
            if !d.is_empty() { return PathBuf::from(d).join("Hardpoint"); }
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(h) = std::env::var("HOME") {
            if !h.is_empty() {
                return PathBuf::from(h).join("Library/Application Support/Hardpoint");
            }
        }
    }
    if let Ok(d) = std::env::var("XDG_DATA_HOME") {
        if !d.is_empty() { return PathBuf::from(d).join("hardpoint"); }
    }
    if let Ok(h) = std::env::var("HOME") {
        if !h.is_empty() { return PathBuf::from(h).join(".local/share/hardpoint"); }
    }
    PathBuf::from(".hardpoint")
}
