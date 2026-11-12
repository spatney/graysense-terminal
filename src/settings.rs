use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::theme::ThemeId;

/// `%APPDATA%\Graysense` (created on first use).
pub fn config_dir() -> PathBuf {
    let dir = dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Graysense");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Left rail destinations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SidePanel {
    Missions,
    Sessions,
    Memory,
    Mcp,
    Plugins,
    Settings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayoutMode {
    Auto,
    Focus,
    Columns,
    Rows,
}

impl LayoutMode {
    pub fn next(self) -> Self {
        match self {
            LayoutMode::Auto => LayoutMode::Focus,
            LayoutMode::Focus => LayoutMode::Columns,
            LayoutMode::Columns => LayoutMode::Rows,
            LayoutMode::Rows => LayoutMode::Auto,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeId,
    pub term_font_size: f32,
    pub side_panel: Option<SidePanel>,
    pub side_width: f32,
    pub layout: LayoutMode,
    pub recent_dirs: Vec<String>,
    pub default_model: Option<String>,
    /// Custom shell program for terminal bays (defaults to pwsh, then powershell).
    pub shell: Option<String>,
    pub show_reasoning: bool,
    pub smooth_cursor: bool,
    pub perf_hud: bool,
    pub memory: MemorySettings,
}

/// Memory controls. `enabled = false` stops both learning and using memories.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct MemorySettings {
    pub enabled: bool,
    /// Learn from conversations in the background.
    pub learn: bool,
    /// Give new agents the relevant memories.
    pub inject: bool,
    /// Put every learned memory in the inbox instead of auto-saving confident ones.
    pub review_all: bool,
    /// Model for the background curator (`None` = the cheapest available).
    pub model: Option<String>,
}

impl Default for MemorySettings {
    fn default() -> Self {
        Self { enabled: true, learn: true, inject: true, review_all: false, model: None }
    }
}

impl MemorySettings {
    pub fn learning(&self) -> bool {
        self.enabled && self.learn
    }

    /// Confidence at which learned memories are saved without review.
    pub fn auto_accept(&self) -> Option<f32> {
        (!self.review_all).then_some(0.85)
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeId::Graphite,
            term_font_size: 13.5,
            side_panel: Some(SidePanel::Missions),
            side_width: 320.0,
            layout: LayoutMode::Auto,
            recent_dirs: Vec::new(),
            default_model: None,
            shell: None,
            show_reasoning: true,
            smooth_cursor: true,
            perf_hud: false,
            memory: MemorySettings::default(),
        }
    }
}

impl Settings {
    fn path() -> PathBuf {
        config_dir().join("settings.json")
    }

    pub fn load() -> Self {
        read_json(&Self::path()).unwrap_or_default()
    }

    pub fn save(&self) {
        write_json(&Self::path(), self);
    }

    pub fn push_recent_dir(&mut self, dir: &Path) {
        let s = dir.display().to_string();
        self.recent_dirs.retain(|d| !d.eq_ignore_ascii_case(&s));
        self.recent_dirs.insert(0, s);
        self.recent_dirs.truncate(12);
    }

    pub fn default_dir(&self) -> PathBuf {
        self.recent_dirs
            .iter()
            .map(PathBuf::from)
            .find(|p| p.is_dir())
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

pub fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str(&text) {
        Ok(v) => Some(v),
        Err(err) => {
            tracing::warn!("failed to parse {}: {err}", path.display());
            None
        }
    }
}

/// Atomic-ish write: write to a temp file then rename over the target.
pub fn write_json<T: Serialize>(path: &Path, value: &T) {
    let Ok(text) = serde_json::to_string_pretty(value) else { return };
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// Resolve the shell used by terminal bays.
pub fn resolve_shell(custom: Option<&str>) -> String {
    if let Some(s) = custom.filter(|s| !s.trim().is_empty()) {
        return s.to_string();
    }
    if which("pwsh.exe").is_some() {
        "pwsh.exe".into()
    } else {
        "powershell.exe".into()
    }
}

pub fn which(exe: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|p| p.join(exe))
        .find(|p| p.is_file() || std::fs::symlink_metadata(p).is_ok())
}

pub fn short_path(path: &Path) -> String {
    let full = path.display().to_string();
    if let Some(home) = dirs::home_dir() {
        let h = home.display().to_string();
        if full.len() >= h.len() && full[..h.len()].eq_ignore_ascii_case(&h) {
            return format!("~{}", &full[h.len()..]);
        }
    }
    full
}

pub fn dir_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}
