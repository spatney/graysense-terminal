use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// Color themes (the CSS has a matching `[data-theme]` palette for each).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeId {
    Graphite,
    Carbon,
    Slate,
    Paper,
}

impl ThemeId {
    pub const ALL: [ThemeId; 4] = [ThemeId::Graphite, ThemeId::Carbon, ThemeId::Slate, ThemeId::Paper];
}

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
    /// Thinking effort new agents start with (None: each model's own default).
    pub default_effort: Option<String>,
    /// Context window tier new agents start with on models that offer it (None: each model's usual window).
    pub default_context: Option<String>,
    /// New sessions approve all tool calls unless the user switches it off for that session.
    pub default_auto_approve: bool,
    /// Record the files agents change, so undoing a turn can restore them too.
    pub track_file_changes: bool,
    /// Custom shell program for terminal bays (defaults to pwsh, then powershell).
    pub shell: Option<String>,
    pub show_reasoning: bool,
    /// The terminal cursor blinks.
    pub cursor_blink: bool,
    /// CPU and memory use in the status bar.
    pub perf_hud: bool,
    /// Interface zoom (1 = 100%).
    pub zoom: f32,
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
    /// Merge memories that say the same thing, about once a week.
    pub tidy: bool,
}

impl Default for MemorySettings {
    fn default() -> Self {
        Self { enabled: true, learn: true, inject: true, review_all: false, model: None, tidy: true }
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
            default_effort: None,
            default_context: None,
            default_auto_approve: false,
            track_file_changes: true,
            shell: None,
            show_reasoning: true,
            cursor_blink: true,
            perf_hud: true,
            zoom: 1.0,
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
        read_json_backed(&Self::path()).unwrap_or_default()
    }

    pub fn save(&self) -> bool {
        write_json_backed(&Self::path(), self).is_ok()
    }

    pub fn push_recent_dir(&mut self, dir: &Path) {
        let s = dir.display().to_string();
        self.recent_dirs.retain(|d| !d.eq_ignore_ascii_case(&s));
        self.recent_dirs.insert(0, s);
        self.recent_dirs.truncate(12);
    }

    /// Drop `dir` from the recent folders. Returns whether it was there.
    pub fn forget_recent_dir(&mut self, dir: &Path) -> bool {
        let s = dir.display().to_string();
        let before = self.recent_dirs.len();
        self.recent_dirs.retain(|d| !d.eq_ignore_ascii_case(&s));
        self.recent_dirs.len() != before
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
        add("cursor_blink", json!(self.cursor_blink), json!("boolean"), "Blink the terminal cursor.");
        add("performance_hud", json!(self.perf_hud), json!("boolean"), "Show CPU and memory use in the status bar.");
        add("zoom", json!(self.zoom), json!("number from 0.7 to 1.6"), "Interface zoom (1 = 100%).");
        add("default_model", json!(self.default_model), model_choice.clone(), "Model new agents start with.");
        add("default_effort", json!(self.default_effort), json!({ "one_of": EFFORTS, "or": "null for each model's default" }), "How hard new agents think (reasoning effort). Models that don't offer the level use their default.");
        add("default_context", json!(self.default_context), json!({ "one_of": CONTEXT_TIERS, "or": "null for each model's usual window" }), "Context window new agents start with: \"default\" (standard) or \"long_context\" (larger, e.g. 1M tokens; can cost more per token). Models with one size ignore it.");
        add("default_auto_approve", json!(self.default_auto_approve), json!("boolean"), "Approve all tool calls in new sessions without asking, including commands and file changes. Existing and resumed sessions are unchanged; each new session can override this.");
        add("track_file_changes", json!(self.track_file_changes), json!("boolean"), "Record the files agents change, so undoing a turn can restore them too. Applies to sessions started or resumed after the change.");
        add("shell", json!(self.shell), json!("a shell id (pwsh, powershell, cmd, git-bash, wsl), a program or command line, or null for the default"), "Shell that new terminal bays run.");
        add("sessions_folder", json!(self.sessions_root().display().to_string()), json!("absolute folder path, or null for ~\\Graysense\\Sessions"), "Where new sessions get their own folders.");
        add("session_folders", json!(self.session_folders), json!("boolean"), "Give every new session its own folder inside sessions_folder.");
        add("auto_update", json!(self.auto_update), json!("boolean"), "Download Graysense updates in the background; they install on restart or when Graysense closes.");
        add("memory.enabled", json!(self.memory.enabled), json!("boolean"), "Master switch for Graysense memory.");
        add("memory.learn", json!(self.memory.learn), json!("boolean"), "Learn from conversations in the background.");
        add("memory.share", json!(self.memory.inject), json!("boolean"), "Share relevant memories with agents.");
        add("memory.review_all", json!(self.memory.review_all), json!("boolean"), "Ask the user before saving anything learned.");
        add("memory.learning_model", json!(self.memory.model), model_choice, "Model the memory curator uses (null = lowest cost).");
        add("memory.tidy", json!(self.memory.tidy), json!("boolean"), "Merge memories that say the same thing, about once a week (while learning is on).");
        Value::Object(out)
    }

    /// Every setting's current value, under the keys `apply_patch` takes (what the UI shows).
    pub fn values(&self) -> Value {
        let described = self.describe(&[]);
        let mut out = Map::new();
        if let Some(all) = described.as_object() {
            for (key, d) in all {
                out.insert(key.clone(), d.get("value").cloned().unwrap_or(Value::Null));
            }
        }
        Value::Object(out)
    }

    /// The preferences sync carries, under the keys `apply_patch` takes.
    pub fn synced_values(&self) -> BTreeMap<String, Value> {
        let all = self.values();
        SYNCED_KEYS.iter().filter_map(|k| all.get(*k).map(|v| ((*k).to_owned(), v.clone()))).collect()
    }

    /// Apply preferences that came from another PC, one by one: a model this account doesn't
    /// offer must not stop the theme from changing. Returns what changed.
    pub fn apply_synced(&mut self, changes: &BTreeMap<String, Value>, models: &[String]) -> Vec<String> {
        let mut done = Vec::new();
        for (key, value) in changes.iter().filter(|(k, _)| SYNCED_KEYS.contains(&k.as_str())) {
            let mut one = Map::new();
            one.insert(key.clone(), value.clone());
            if let Ok(mut applied) = self.apply_patch(&one, models) {
                done.append(&mut applied);
            }
        }
        done
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
                "cursor_blink" => next.cursor_blink = flag()?,
                "zoom" => next.zoom = number(0.7, 1.6)?,
                "performance_hud" => next.perf_hud = flag()?,
                "default_model" => next.default_model = model()?,
                "default_auto_approve" => next.default_auto_approve = flag()?,
                "track_file_changes" => next.track_file_changes = flag()?,
                "default_effort" => {
                    next.default_effort = if v.is_null() {
                        None
                    } else {
                        let e = text()?.to_ascii_lowercase();
                        Some(EFFORTS.contains(&e.as_str()).then_some(e).ok_or_else(bad)?)
                    }
                }
                "default_context" => {
                    next.default_context = if v.is_null() {
                        None
                    } else {
                        let t = text()?.to_ascii_lowercase();
                        Some(CONTEXT_TIERS.contains(&t.as_str()).then_some(t).ok_or_else(bad)?)
                    }
                }
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
                "memory.tidy" => next.memory.tidy = flag()?,
                other => return Err(format!("Unknown setting “{other}”. Call graysense_settings with action \"get\" to list them.")),
            }
            done.push(format!("{key} → {}", if v.is_null() { "default".to_owned() } else { v.to_string().trim_matches('"').to_owned() }));
        }
        *self = next;
        Ok(done)
    }
}

/// Reasoning effort levels, least to most (models offer some of them).
pub const EFFORTS: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];

/// Context window tiers: the standard window and the long one some models offer.
pub const CONTEXT_TIERS: [&str; 2] = ["default", "long_context"];

/// Settings an agent may change without asking: appearance and layout only.
pub const SAFE_KEYS: &[&str] = &["theme", "terminal_font_size", "layout", "side_panel", "side_panel_width", "show_reasoning", "cursor_blink", "performance_hud", "zoom"];

/// Preferences that make sense on any PC, which sync carries. Not what's about one PC (folders,
/// shell, window, zoom, updates), and nothing that changes what agents may do without asking
/// (`default_auto_approve`) or what program runs (`shell`): a repository must never be able to
/// switch those on.
pub const SYNCED_KEYS: &[&str] = &[
    "theme",
    "layout",
    "show_reasoning",
    "cursor_blink",
    "default_model",
    "default_effort",
    "default_context",
    "memory.learn",
    "memory.share",
    "memory.review_all",
    "memory.learning_model",
    "memory.tidy",
];

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

// ------------------------------------------------------------------ state files

/// A state file that couldn't be loaded or saved, for the user to hear about.
pub struct FileProblem {
    pub text: String,
    /// The file to show (a damaged one is shown where it was moved aside).
    pub path: PathBuf,
}

static FILE_PROBLEMS: std::sync::Mutex<Vec<FileProblem>> = std::sync::Mutex::new(Vec::new());
/// Files that are there but couldn't be read (nor moved aside): never written over this run.
static PROTECTED: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());
/// Files whose last save failed (so a failure that persists is reported once).
static FAILING: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// A backup is refreshed when it's older than this, so a damaged file loses little.
const BACKUP_EVERY: std::time::Duration = std::time::Duration::from_secs(15 * 60);

fn locked<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Problems found reading or saving state files since the last call.
pub fn take_file_problems() -> Vec<FileProblem> {
    std::mem::take(&mut *locked(&FILE_PROBLEMS))
}

fn report(text: String, path: PathBuf) {
    tracing::warn!("{text}");
    locked(&FILE_PROBLEMS).push(FileProblem { text, path });
}

fn is_protected(path: &Path) -> bool {
    locked(&PROTECTED).iter().any(|p| p == path)
}

/// What a state file holds, for messages ("Your memories and notes couldn't be read").
fn file_label(path: &Path) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    match name.as_str() {
        "settings.json" => "Your settings".into(),
        "memory.json" => "Your memories and notes".into(),
        "missions.json" => "Your missions".into(),
        "sessions.json" => "The list of sessions".into(),
        "workspace.json" => "The sessions and terminals that were open".into(),
        "sync.json" => "The sync setup".into(),
        "memory-queue.json" => "Conversations waiting to be read for memories".into(),
        _ => name,
    }
}

fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".bak");
    path.with_file_name(name)
}

/// Another program (an antivirus, a sync client) can hold a file for a moment: try a few times.
fn retrying<T>(mut op: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    let mut tries = 0;
    loop {
        match op() {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound && tries < 3 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(25 * tries));
            }
            r => return r,
        }
    }
}

fn parse<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> serde_json::Result<T> {
    // Notepad and other editors can save a byte order mark, which JSON doesn't allow.
    serde_json::from_slice(bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes))
}

/// Read a JSON state file; a missing one is `None`. One that's there but can't be used is never
/// lost: a damaged file is moved aside (`memory.corrupt-<time>.json`), one that can't be read is
/// never written over this run, and the user hears about it (see [`take_file_problems`]).
pub fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    load_json(path, false)
}

/// [`read_json`] for a file saved with [`write_json_backed`]: a damaged file falls back on its backup.
pub fn read_json_backed<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    load_json(path, true)
}

fn load_json<T: for<'de> Deserialize<'de>>(path: &Path, backed: bool) -> Option<T> {
    let label = file_label(path);
    let backup = || if backed { std::fs::read(backup_path(path)).ok().and_then(|b| parse::<T>(&b).ok()) } else { None };
    let bytes = match retrying(|| std::fs::read(path)) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            locked(&PROTECTED).push(path.to_owned());
            let restored = backup();
            let then = if restored.is_some() { "Graysense is using its backup for now, and" } else { "Graysense" };
            report(format!("{label} couldn't be read ({e}). {then} won't save over the file until it restarts."), path.to_owned());
            return restored;
        }
    };
    let err = match parse(&bytes) {
        Ok(v) => return Some(v),
        Err(e) => e,
    };
    tracing::warn!("failed to parse {}: {err}", path.display());
    let restored = backup();
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let aside = path.with_file_name(format!("{stem}.corrupt-{}.json", chrono::Local::now().format("%Y%m%d-%H%M%S")));
    let start = if restored.is_some() { "Graysense restored its last backup" } else { "Graysense started fresh" };
    if std::fs::rename(path, &aside).is_ok() {
        let kept = aside.file_name().unwrap_or_default().to_string_lossy().to_string();
        report(format!("{label} couldn't be read: the file is damaged. {start} and kept the damaged file as {kept}."), aside);
    } else {
        locked(&PROTECTED).push(path.to_owned());
        report(format!("{label} couldn't be read: the file is damaged. {start} and won't save over the file until it restarts."), path.to_owned());
    }
    restored
}

/// Save a state file so it's never half-written: write a temp file, flush it to the disk, then
/// replace the old one. A failure is reported once (until a save works again).
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if is_protected(path) {
        return Err(std::io::Error::other(format!("{} couldn't be read, so it's left as it is", path.display())));
    }
    let result = write_now(path, value);
    let mut failing = locked(&FAILING);
    let known = failing.iter().position(|p| p == path);
    match (&result, known) {
        (Ok(()), Some(i)) => {
            failing.swap_remove(i);
        }
        (Err(e), None) => {
            failing.push(path.to_owned());
            drop(failing);
            report(format!("{} couldn't be saved ({e}). Graysense keeps trying.", file_label(path)), path.to_owned());
        }
        _ => {}
    }
    result
}

fn write_now<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    use std::io::Write;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let text = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}-{}.tmp", std::process::id(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let tmp = path.with_file_name(name);
    let written = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&text)?;
        file.sync_all()?;
        drop(file);
        retrying(|| std::fs::rename(&tmp, path))
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// [`write_json`], keeping a backup of the file (refreshed now and then) for
/// [`read_json_backed`] to fall back on.
pub fn write_json_backed<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let bak = backup_path(path);
    let stale = std::fs::metadata(&bak).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_none_or(|age| age > BACKUP_EVERY);
    if stale && !is_protected(path) && path.exists() {
        let _ = std::fs::copy(path, &bak);
    }
    write_json(path, value)
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
    fn default_auto_approve_is_opt_in_and_persists() {
        let mut settings: Settings = serde_json::from_value(json!({})).unwrap();
        assert!(!settings.default_auto_approve);
        assert_eq!(settings.describe(&[])["default_auto_approve"]["asks_user"], true);

        let changes = json!({ "default_auto_approve": true });
        settings.apply_patch(changes.as_object().unwrap(), &[]).unwrap();
        let mut restored: Settings = serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert!(restored.default_auto_approve);
        assert_eq!(restored.values()["default_auto_approve"], true);

        let changes = json!({ "default_auto_approve": false });
        restored.apply_patch(changes.as_object().unwrap(), &[]).unwrap();
        assert!(!restored.default_auto_approve);
    }

    #[test]
    fn invalid_default_auto_approve_does_not_apply_other_changes() {
        let mut settings = Settings::default();
        let changes = json!({ "default_auto_approve": "true", "cursor_blink": false });
        assert!(settings.apply_patch(changes.as_object().unwrap(), &[]).is_err());
        assert!(!settings.default_auto_approve);
        assert!(settings.cursor_blink);
    }

    #[test]
    fn the_default_context_window_takes_a_tier_or_null() {
        let mut settings = Settings::default();
        settings.apply_patch(json!({ "default_context": "long_context" }).as_object().unwrap(), &[]).unwrap();
        assert_eq!(settings.values()["default_context"], "long_context");
        // A size isn't a tier: each model's windows have their own sizes.
        assert!(settings.apply_patch(json!({ "default_context": "1M" }).as_object().unwrap(), &[]).is_err());
        settings.apply_patch(json!({ "default_context": null }).as_object().unwrap(), &[]).unwrap();
        assert_eq!(settings.default_context, None);
    }

    #[test]
    fn home_is_shortened_only_at_a_folder_boundary() {
        let home = Some(Path::new(r"C:\Users\Me"));
        assert_eq!(shorten_home(Path::new(r"c:\users\me\Graysense\Sessions\x"), home), r"~\Graysense\Sessions\x");
        assert_eq!(shorten_home(Path::new("C:/Users/Me"), home), "~");
        assert_eq!(shorten_home(Path::new(r"C:\Users\Meg\x"), home), r"C:\Users\Meg\x");
        assert_eq!(shorten_home(Path::new(r"C:\Users\Mé\x"), home), r"C:\Users\Mé\x");
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gs-state-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn reported(path: &Path) -> bool {
        locked(&FILE_PROBLEMS).iter().any(|p| p.path.parent() == path.parent())
    }

    #[test]
    fn a_damaged_file_is_kept_aside_and_its_backup_restores_it() {
        let dir = scratch("damaged");
        let path = dir.join("memory.json");
        write_json_backed(&path, &json!({ "n": 1 })).unwrap();
        // The first save had nothing to back up; this one keeps the first.
        write_json_backed(&path, &json!({ "n": 2 })).unwrap();
        assert_eq!(read_json_backed::<Value>(&path), Some(json!({ "n": 2 })));
        std::fs::write(&path, "{ \"n\": 2, trunc").unwrap();

        assert_eq!(read_json_backed::<Value>(&path), Some(json!({ "n": 1 })));
        let aside: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.starts_with("memory.corrupt-")).collect();
        assert_eq!(aside.len(), 1, "the damaged file is kept");
        assert_eq!(std::fs::read_to_string(dir.join(&aside[0])).unwrap(), "{ \"n\": 2, trunc");
        assert!(reported(&path));
        // Without a backup it starts fresh, and the damaged file is still kept.
        let other = dir.join("missions.json");
        std::fs::write(&other, "not json").unwrap();
        assert_eq!(read_json::<Value>(&other), None);
        assert!(!other.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_is_not_a_problem_and_a_byte_order_mark_is_fine() {
        let dir = scratch("missing");
        assert_eq!(read_json::<Value>(&dir.join("settings.json")), None);
        assert!(!reported(&dir.join("settings.json")));
        let path = dir.join("settings.json");
        std::fs::write(&path, b"\xEF\xBB\xBF{\"zoom\": 1.5}").unwrap();
        assert_eq!(read_json::<Value>(&path), Some(json!({ "zoom": 1.5 })));
        // Saves leave no temp files behind.
        write_json(&path, &json!({ "zoom": 2 })).unwrap();
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn a_file_that_cannot_be_read_is_never_written_over() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = scratch("locked");
        let path = dir.join("sessions.json");
        std::fs::write(&path, "[\"precious\"]").unwrap();
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(&path).unwrap();
        assert_eq!(read_json::<Value>(&path), None);
        drop(held);
        assert!(reported(&path));
        assert!(write_json(&path, &json!([])).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[\"precious\"]");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
