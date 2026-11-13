use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::theme::ThemeId;

/// `%APPDATA%\Graysense` (created on first use). `GRAYSENSE_CONFIG_DIR` points Graysense at
/// another profile (e.g. for tests), leaving the user's own untouched.
pub fn config_dir() -> PathBuf {
    let dir = std::env::var_os("GRAYSENSE_CONFIG_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("Graysense"));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// `~\Graysense\Sessions`: where new sessions get their own folders unless the user picks another.
pub fn default_sessions_root() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join("Graysense").join("Sessions")
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
    /// Folder that holds each session's own folder (`None` = `~\Graysense\Sessions`).
    pub sessions_root: Option<String>,
    /// New sessions get their own folder under the sessions folder (otherwise the last folder used).
    pub session_folders: bool,
    /// Installed copies download new versions in the background and install them on restart.
    pub auto_update: bool,
    /// The installed version that last ran, to tell the user when it has been updated.
    pub last_version: Option<String>,
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
            sessions_root: None,
            session_folders: true,
            auto_update: true,
            last_version: None,
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

    pub fn sessions_root(&self) -> PathBuf {
        self.sessions_root.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(PathBuf::from).unwrap_or_else(default_sessions_root)
    }

    /// Every setting an agent can change, with its current value and what it accepts
    /// (the `graysense_settings` tool's `get`).
    pub fn describe(&self, models: &[String]) -> Value {
        let mut out = Map::new();
        let mut add = |key: &str, value: Value, accepts: Value, about: &str| {
            out.insert(key.to_owned(), json!({ "value": value, "accepts": accepts, "description": about, "asks_user": !SAFE_KEYS.contains(&key) }));
        };
        let model_choice = json!({ "one_of": models, "or": "null for the default" });
        add("theme", json!(theme_key(self.theme)), json!(["graphite", "carbon", "slate", "paper"]), "Color theme. Paper is light; the others are dark.");
        add("terminal_font_size", json!(self.term_font_size), json!("number from 9 to 28"), "Font size of terminal bays, in points.");
        add("layout", json!(layout_key(self.layout)), json!(["grid", "focus", "columns", "rows"]), "How bays are tiled.");
        add("side_panel", json!(self.side_panel.map(panel_key).unwrap_or("hidden")), json!(PANELS), "Which side panel is open.");
        add("side_panel_width", json!(self.side_width), json!("number from 260 to 620"), "Side panel width, in points.");
        add("show_reasoning", json!(self.show_reasoning), json!("boolean"), "Show agents' reasoning in transcripts.");
        add("smooth_cursor", json!(self.smooth_cursor), json!("boolean"), "Animate the terminal cursor.");
        add("performance_hud", json!(self.perf_hud), json!("boolean"), "Show frame time and memory use in the status bar.");
        add("default_model", json!(self.default_model), model_choice.clone(), "Model new agents start with.");
        add("shell", json!(self.shell), json!("program or command line, or null for PowerShell"), "Shell that new terminal bays run.");
        add("sessions_folder", json!(self.sessions_root().display().to_string()), json!("absolute folder path, or null for ~\\Graysense\\Sessions"), "Where new sessions get their own folders.");
        add("session_folders", json!(self.session_folders), json!("boolean"), "Give every new session its own folder inside sessions_folder.");
        add("auto_update", json!(self.auto_update), json!("boolean"), "Download Graysense updates in the background; they install on restart or when Graysense closes.");
        add("memory.enabled", json!(self.memory.enabled), json!("boolean"), "Master switch for Graysense memory.");
        add("memory.learn", json!(self.memory.learn), json!("boolean"), "Learn from conversations in the background.");
        add("memory.share", json!(self.memory.inject), json!("boolean"), "Share relevant memories with agents.");
        add("memory.review_all", json!(self.memory.review_all), json!("boolean"), "Ask the user before saving anything learned.");
        add("memory.learning_model", json!(self.memory.model), model_choice, "Model the memory curator uses (null = lowest cost).");
        Value::Object(out)
    }

    /// Apply `{key: value}` changes from an agent, all or nothing. Returns what changed.
    pub fn apply_patch(&mut self, changes: &Map<String, Value>, models: &[String]) -> Result<Vec<String>, String> {
        let mut next = self.clone();
        let mut done = Vec::new();
        for (key, v) in changes {
            let bad = || format!("Invalid value for {key}: {v}. Call graysense_settings with action \"get\" to see what it accepts.");
            let text = || v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).ok_or_else(bad);
            let flag = || v.as_bool().ok_or_else(bad);
            let number = |lo: f64, hi: f64| v.as_f64().filter(|n| (lo..=hi).contains(n)).map(|n| n as f32).ok_or_else(bad);
            let model = || -> Result<Option<String>, String> {
                if v.is_null() {
                    return Ok(None);
                }
                let m = text()?;
                if !models.is_empty() && !models.contains(&m) {
                    return Err(format!("Unknown model {m}. Available: {}", models.join(", ")));
                }
                Ok(Some(m))
            };
            match key.as_str() {
                "theme" => next.theme = v.as_str().and_then(parse_theme).ok_or_else(bad)?,
                "terminal_font_size" => next.term_font_size = number(9.0, 28.0)?,
                "layout" => next.layout = v.as_str().and_then(parse_layout).ok_or_else(bad)?,
                "side_panel" => next.side_panel = if v.is_null() { None } else { parse_panel(&text()?).ok_or_else(bad)? },
                "side_panel_width" => next.side_width = number(260.0, 620.0)?,
                "show_reasoning" => next.show_reasoning = flag()?,
                "smooth_cursor" => next.smooth_cursor = flag()?,
                "performance_hud" => next.perf_hud = flag()?,
                "default_model" => next.default_model = model()?,
                "shell" => next.shell = if v.is_null() { None } else { Some(text()?) },
                "sessions_folder" => {
                    next.sessions_root = if v.is_null() {
                        None
                    } else {
                        let p = text()?;
                        if !Path::new(&p).is_absolute() || Path::new(&p).is_file() {
                            return Err("sessions_folder must be an absolute path to a folder".into());
                        }
                        Some(p)
                    }
                }
                "session_folders" => next.session_folders = flag()?,
                "auto_update" => next.auto_update = flag()?,
                "memory.enabled" => next.memory.enabled = flag()?,
                "memory.learn" => next.memory.learn = flag()?,
                "memory.share" => next.memory.inject = flag()?,
                "memory.review_all" => next.memory.review_all = flag()?,
                "memory.learning_model" => next.memory.model = model()?,
                other => return Err(format!("Unknown setting “{other}”. Call graysense_settings with action \"get\" to list them.")),
            }
            done.push(format!("{key} → {}", if v.is_null() { "default".to_owned() } else { v.to_string().trim_matches('"').to_owned() }));
        }
        *self = next;
        Ok(done)
    }
}

/// Settings an agent may change without asking: appearance and layout only.
pub const SAFE_KEYS: &[&str] = &["theme", "terminal_font_size", "layout", "side_panel", "side_panel_width", "show_reasoning", "smooth_cursor", "performance_hud"];

const PANELS: [&str; 7] = ["missions", "sessions", "memory", "mcp", "plugins", "settings", "hidden"];

fn theme_key(t: ThemeId) -> &'static str {
    match t {
        ThemeId::Graphite => "graphite",
        ThemeId::Carbon => "carbon",
        ThemeId::Slate => "slate",
        ThemeId::Paper => "paper",
    }
}

fn parse_theme(s: &str) -> Option<ThemeId> {
    let s = s.trim().to_ascii_lowercase();
    ThemeId::ALL.into_iter().find(|t| theme_key(*t) == s || (s == "light" && *t == ThemeId::Paper) || (s == "dark" && *t == ThemeId::Graphite))
}

fn layout_key(l: LayoutMode) -> &'static str {
    match l {
        LayoutMode::Auto => "grid",
        LayoutMode::Focus => "focus",
        LayoutMode::Columns => "columns",
        LayoutMode::Rows => "rows",
    }
}

fn parse_layout(s: &str) -> Option<LayoutMode> {
    [LayoutMode::Auto, LayoutMode::Focus, LayoutMode::Columns, LayoutMode::Rows].into_iter().find(|l| layout_key(*l).eq_ignore_ascii_case(s.trim()))
}

fn panel_key(p: SidePanel) -> &'static str {
    match p {
        SidePanel::Missions => "missions",
        SidePanel::Sessions => "sessions",
        SidePanel::Memory => "memory",
        SidePanel::Mcp => "mcp",
        SidePanel::Plugins => "plugins",
        SidePanel::Settings => "settings",
    }
}

/// `Some(None)` = hide the panel.
fn parse_panel(s: &str) -> Option<Option<SidePanel>> {
    let s = s.to_ascii_lowercase();
    if s == "hidden" || s == "none" {
        return Some(None);
    }
    [SidePanel::Missions, SidePanel::Sessions, SidePanel::Memory, SidePanel::Mcp, SidePanel::Plugins, SidePanel::Settings].into_iter().find(|p| panel_key(*p) == s).map(Some)
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

/// `path` with the home folder shortened to "~" (e.g. "~\code\app").
pub fn short_path(path: &Path) -> String {
    shorten_home(path, dirs::home_dir().as_deref())
}

fn shorten_home(path: &Path, home: Option<&Path>) -> String {
    let path = path.to_string_lossy().replace('/', "\\");
    if let Some(home) = home {
        let home = home.to_string_lossy().replace('/', "\\");
        let home = home.trim_end_matches('\\');
        // Only at a folder boundary ("C:\Users\Meg" isn't inside "C:\Users\Me"), and `get`
        // keeps the cut on a char boundary.
        if !home.is_empty()
            && let Some(rest) = path.get(home.len()..)
            && path[..home.len()].eq_ignore_ascii_case(home)
            && (rest.is_empty() || rest.starts_with('\\'))
        {
            return format!("~{rest}");
        }
    }
    path
}

pub fn dir_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_is_shortened_only_at_a_folder_boundary() {
        let home = Some(Path::new(r"C:\Users\Me"));
        assert_eq!(shorten_home(Path::new(r"c:\users\me\Graysense\Sessions\x"), home), r"~\Graysense\Sessions\x");
        assert_eq!(shorten_home(Path::new("C:/Users/Me"), home), "~");
        assert_eq!(shorten_home(Path::new(r"C:\Users\Meg\x"), home), r"C:\Users\Meg\x");
        assert_eq!(shorten_home(Path::new(r"C:\Users\Mé\x"), home), r"C:\Users\Mé\x");
    }
}
