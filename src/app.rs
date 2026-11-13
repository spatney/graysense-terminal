//! Graysense app shell: title bar, rail, side panels, bay grid, status bar, overlays.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, Frame, Id, Key, KeyboardShortcut, Margin, Modifiers, PointerButton,
    Rect, ResizeDirection, RichText, Sense, Stroke, StrokeKind, Ui, UiBuilder, ViewportCommand, pos2, vec2,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::agent::backend::{AgentSpec, AppCmd, Backend, ClientState, Cmd, Evt, McpCmd, ModelInfo, PluginCmd, SessionInfo, UiTx};
use crate::agent::view::{self as agent_view, AgentAction, AgentCtx, HEADER_H};
use crate::agent::{AgentView, AppTab, BayId, Effect, Item, Level, LogLine, Mode, Status};
use crate::canvas::CanvasInstance;
use crate::control::mcp::{McpAction, McpPanel};
use crate::control::memory::{MemoryAction, MemoryPanel};
use crate::control::plugins::{PluginAction, PluginPanel};
use crate::kit::{self, icon};
use crate::layout;
use crate::mcp_apps::{AppHost, AppRequest};
use crate::memory::{MemoryId, MemoryStore, Origin, Project, Scope, Source, State as MemoryState};
use crate::missions::{BoardAction, MissionBoard, MissionDrag};
use crate::palette::{Command, Mode as PaletteMode, Palette};
use crate::sessions::{self, SessionIndex};
use crate::settings::{self, Settings, SidePanel};
use crate::sysmon::{self, SysStats};
use crate::term::view::TermAction;
use crate::term::{SpawnSpec, TermPane, render as term_render, view as term_view};
use crate::theme::{self, Theme, icon_font, mix, semibold, ui as ui_font};
use crate::update::{self, UiAction as UpdateAction, Updater};

enum BayKind {
    Agent(Box<AgentView>),
    Shell(Box<TermPane>),
}

struct Bay {
    id: BayId,
    kind: BayKind,
}

impl Bay {
    fn title(&self) -> String {
        match &self.kind {
            BayKind::Agent(v) => v.display_title(),
            BayKind::Shell(p) => p.title.clone(),
        }
    }
}

struct Toast {
    text: String,
    level: Level,
    born: Instant,
}

enum Pick {
    Agent,
    Shell,
    Mission(u64),
    Attach(BayId),
    ExportMemory { markdown: bool },
    ImportMemory,
    SessionsRoot,
}

#[derive(Serialize, Deserialize, Default)]
struct Workspace {
    bays: Vec<SavedBay>,
    focused: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind")]
enum SavedBay {
    Agent {
        cwd: String,
        session_id: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        incognito: bool,
        /// A new session that hasn't started (it gets its folder with the first message).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        fresh: bool,
    },
    Shell { cwd: String },
}

enum BayAction {
    Focus(usize),
    Close(usize),
    Maximize(usize),
    Agent(usize, AgentAction),
    Term(usize, TermAction),
    DropMission(usize, u64),
    DropFiles(usize, Vec<PathBuf>),
}

pub struct GraysenseApp {
    rt: Arc<tokio::runtime::Runtime>,
    backend: Backend,
    evt_rx: Receiver<Evt>,
    theme: Theme,
    settings: Settings,
    bays: Vec<Bay>,
    focused: usize,
    maximized: bool,
    next_bay: BayId,
    missions: MissionBoard,
    mcp: McpPanel,
    plugins: PluginPanel,
    apps: AppHost,
    palette: Palette,
    models: Vec<ModelInfo>,
    sessions: Vec<SessionInfo>,
    client: ClientState,
    toasts: Vec<Toast>,
    activity: VecDeque<LogLine>,
    sys: Arc<Mutex<SysStats>>,
    metrics: Option<(f32, f32, term_render::Metrics)>,
    picker: Option<(Pick, Receiver<Vec<PathBuf>>)>,
    t0: Instant,
    last_frame: Instant,
    frame_ms: f32,
    focus_bay: bool,
    last_save: Instant,
    last_sample: Instant,
    /// Debounced MCP panel refresh after server status events.
    mcp_refresh_at: Option<Instant>,
    /// Last persisted workspace (serialized) to skip redundant writes.
    last_ws: String,
    native: crate::win::NativeWindow,
    window_title: String,
    settings_dirty: bool,
    window_focused: bool,
    capture_focus: bool,
    memory: Arc<Mutex<MemoryStore>>,
    memory_panel: MemoryPanel,
    /// Curator runs in flight, by bay (the bay may close before the answer arrives).
    learn_jobs: std::collections::HashMap<BayId, LearnJob>,
    /// Names, pins and folders of the sessions Graysense knows.
    session_index: SessionIndex,
    /// Apply `settings.side_width` to the side panel on the next frame (an agent changed it).
    force_side_width: bool,
    sessions_panel_ui: crate::control::sessions::SessionsPanel,
    show_activity: bool,
    show_shortcuts: bool,
    search_ui: crate::search_ui::SearchUi,
    /// Latest search index (sessions + session files), rebuilt in the background.
    search_index: Arc<crate::search::Index>,
    search_gen: u64,
    search_rx: Option<Receiver<crate::search::Index>>,
    search_built: Option<Instant>,
    egui_ctx: egui::Context,
    updater: Updater,
    /// Asking before a restart-to-update stops agents that are working.
    confirm_restart: bool,
    /// Scroll Settings to About on its next frame.
    reveal_about: bool,
}

/// What a curator run was reading, so its answer can be filed correctly.
#[derive(Clone, Serialize, Deserialize)]
struct LearnJob {
    project_root: PathBuf,
    session: Option<String>,
    title: String,
    /// The curator message, kept so unfinished work survives a restart.
    message: String,
}

fn learn_queue_path() -> PathBuf {
    settings::config_dir().join("memory-queue.json")
}

/// Seconds an agent must sit idle after a turn before its conversation is read.
const LEARN_DELAY: Duration = Duration::from_secs(60);

pub struct Startup {
    pub restore: bool,
    pub agent: Option<Option<String>>,
    pub shell: bool,
    pub cwd: Option<PathBuf>,
}

impl Startup {
    /// `graysense [DIR] [--agent [PROMPT]] [--shell] [--no-restore]`
    pub fn from_args() -> Self {
        let mut s = Startup { restore: true, agent: None, shell: false, cwd: None };
        let mut args = std::env::args().skip(1).peekable();
        while let Some(a) = args.next() {
            match a.as_str() {
                "--agent" | "-a" => {
                    let prompt = args.next_if(|p| !p.starts_with("--"));
                    s.agent = Some(prompt);
                }
                "--shell" | "-s" => s.shell = true,
                "--no-restore" => s.restore = false,
                "--help" | "-h" => {
                    println!("graysense [DIR] [--agent [PROMPT]] [--shell] [--no-restore]");
                    std::process::exit(0);
                }
                other => {
                    let p = PathBuf::from(other);
                    if p.is_dir() {
                        s.cwd = Some(p);
                    }
                }
            }
        }
        s
    }
}

impl GraysenseApp {
    pub fn new(cc: &eframe::CreationContext<'_>, rt: Arc<tokio::runtime::Runtime>, startup: Startup) -> Self {
        let ctx = cc.egui_ctx.clone();
        theme::install_fonts(&ctx);
        let settings = Settings::load();
        let theme = Theme::get(settings.theme);
        theme::apply_style(&ctx, &theme);
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        let native = crate::win::NativeWindow::from_handle(cc);
        native.apply_theme(theme.border_strong, theme.dark);
        let (tx, rx) = channel();
        let backend = Backend::spawn(&rt, UiTx::new(tx, ctx.clone()));
        backend.send(Cmd::Connect);
        let updater = Updater::new(ctx.clone(), settings.auto_update);
        let mut app = GraysenseApp {
            rt,
            backend,
            evt_rx: rx,
            theme,
            settings,
            bays: Vec::new(),
            focused: 0,
            maximized: false,
            next_bay: 1,
            missions: MissionBoard::load(),
            mcp: McpPanel::default(),
            plugins: PluginPanel::default(),
            apps: AppHost::new(),
            palette: Palette::default(),
            models: Vec::new(),
            sessions: Vec::new(),
            client: ClientState::Idle,
            toasts: Vec::new(),
            activity: VecDeque::new(),
            sys: sysmon::spawn(),
            metrics: None,
            picker: None,
            t0: Instant::now(),
            last_frame: Instant::now(),
            frame_ms: 0.0,
            focus_bay: true,
            last_save: Instant::now(),
            mcp_refresh_at: None,
            last_ws: String::new(),
            native,
            window_title: "Graysense".into(),
            last_sample: Instant::now(),
            settings_dirty: false,
            window_focused: true,
            capture_focus: false,
            memory: Arc::new(Mutex::new(MemoryStore::load())),
            memory_panel: MemoryPanel::default(),
            learn_jobs: Default::default(),
            session_index: SessionIndex::load(),
            force_side_width: false,
            sessions_panel_ui: Default::default(),
            show_activity: false,
            show_shortcuts: false,
            search_ui: Default::default(),
            search_index: Arc::new(crate::search::Index::empty()),
            search_gen: 0,
            search_rx: None,
            search_built: None,
            egui_ctx: ctx.clone(),
            updater,
            confirm_restart: false,
            reveal_about: false,
        };
        crate::memory::set_switches(app.settings.memory.enabled, app.settings.memory.inject);
        app.note_new_version();
        if startup.restore {
            app.restore_workspace(&ctx);
        }
        let cwd = startup.cwd.clone().unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| app.settings.default_dir()));
        if startup.shell {
            app.new_shell(&ctx, cwd.clone());
        }
        if let Some(prompt) = startup.agent {
            app.new_agent(cwd, prompt, None, None);
        }
        app.refresh_side();
        app
    }

    // ------------------------------------------------------------------ workspace

    fn workspace_path() -> PathBuf {
        settings::config_dir().join("workspace.json")
    }

    fn restore_workspace(&mut self, ctx: &egui::Context) {
        let Some(ws) = settings::read_json::<Workspace>(&Self::workspace_path()) else { return };
        for b in ws.bays {
            match b {
                SavedBay::Agent { cwd, session_id, incognito, fresh } => {
                    let cwd = PathBuf::from(cwd);
                    if fresh {
                        self.new_session(None, incognito);
                    } else if cwd.is_dir() {
                        self.new_agent_ex(cwd, None, session_id, None, incognito);
                    }
                }
                SavedBay::Shell { cwd } => {
                    let cwd = PathBuf::from(cwd);
                    if cwd.is_dir() {
                        self.new_shell(ctx, cwd);
                    }
                }
            }
        }
        self.focused = ws.focused.min(self.bays.len().saturating_sub(1));
    }

    fn workspace(&self) -> Workspace {
        let bays = self
            .bays
            .iter()
            .map(|b| match &b.kind {
                // Sessions that never got a message aren't saved by the runtime; reopen those fresh.
                BayKind::Agent(v) => SavedBay::Agent {
                    cwd: v.cwd.display().to_string(),
                    session_id: v.session_id.clone().filter(|_| v.items.iter().any(|i| matches!(i, Item::User { .. }))),
                    incognito: v.incognito,
                    fresh: v.deferred,
                },
                BayKind::Shell(p) => SavedBay::Shell { cwd: p.cwd.display().to_string() },
            })
            .collect();
        Workspace { bays, focused: self.focused }
    }

    fn save_workspace(&self) {
        settings::write_json(&Self::workspace_path(), &self.workspace());
    }

    /// Persist the open bays whenever they change, so a crash never loses the layout.
    fn save_workspace_if_changed(&mut self) {
        let ws = self.workspace();
        let sig = serde_json::to_string(&ws).unwrap_or_default();
        if sig != self.last_ws {
            settings::write_json(&Self::workspace_path(), &ws);
            self.last_ws = sig;
        }
    }

    // ------------------------------------------------------------------ bays

    fn alloc_bay(&mut self) -> BayId {
        let id = self.next_bay;
        self.next_bay += 1;
        id
    }

    fn bay_index(&self, id: BayId) -> Option<usize> {
        self.bays.iter().position(|b| b.id == id)
    }

    fn agent_mut(&mut self, id: BayId) -> Option<&mut AgentView> {
        self.bays.iter_mut().find(|b| b.id == id).and_then(|b| match &mut b.kind {
            BayKind::Agent(v) => Some(v.as_mut()),
            _ => None,
        })
    }

    fn focus(&mut self, index: usize) {
        if index < self.bays.len() {
            self.focused = index;
            self.focus_bay = true;
        }
    }

    fn new_agent(&mut self, cwd: PathBuf, prompt: Option<String>, resume: Option<String>, mission: Option<u64>) -> BayId {
        self.new_agent_ex(cwd, prompt, resume, mission, false)
    }

    fn new_agent_ex(&mut self, cwd: PathBuf, prompt: Option<String>, resume: Option<String>, mission: Option<u64>, incognito: bool) -> BayId {
        let id = self.alloc_bay();
        let mut view = AgentView::new(id, cwd.clone(), Mode::Interactive);
        view.project = self.project_for(&cwd);
        view.mission = mission;
        view.session_id = resume.clone();
        view.incognito = incognito;
        if let Some(meta) = resume.as_deref().and_then(|r| self.session_index.get(r)) {
            view.name = meta.name.clone();
        }
        if resume.is_some() {
            view.notice(Level::Info, "Resuming session…");
            // Resumed history was already read when it happened.
            view.learn_cursor = usize::MAX;
        }
        if let Some(p) = &prompt {
            view.title = Some(p.clone());
            view.echo_user(p);
            self.capture_explicit_memory(&mut view, p);
        }
        let first = prompt.as_deref().map(sessions::short_title);
        let spec = self.agent_spec(&view, prompt, Vec::new(), resume);
        self.backend.send(Cmd::Create { bay: id, spec });
        if let Some(name) = first {
            self.backend.send(Cmd::NameSession { bay: id, name, user: false });
        }
        self.bays.push(Bay { id, kind: BayKind::Agent(Box::new(view)) });
        self.focus(self.bays.len() - 1);
        if !sessions::is_under(&cwd, &self.settings.sessions_root()) {
            self.settings.push_recent_dir(&cwd);
            self.settings_dirty = true;
        }
        if self.models.is_empty() {
            self.backend.send(Cmd::ListModels);
        }
        id
    }

    /// A new session the way the user set it up: in its own folder under the sessions folder
    /// (named after its first message), or in the last folder used.
    fn new_session(&mut self, prompt: Option<String>, incognito: bool) -> BayId {
        if !self.settings.session_folders {
            let dir = self.settings.default_dir();
            return self.new_agent_ex(dir, prompt, None, None, incognito);
        }
        if let Some(p) = prompt {
            let dir = self.create_session_folder(&p);
            return self.new_agent_ex(dir, Some(p), None, None, incognito);
        }
        // Nothing to name it after yet: the folder and Copilot session start with the first message.
        let id = self.alloc_bay();
        let root = self.settings.sessions_root();
        let mut view = AgentView::new(id, root.clone(), Mode::Interactive);
        view.project = self.project_for(&root);
        view.deferred = true;
        view.incognito = incognito;
        view.status = Status::Idle;
        self.bays.push(Bay { id, kind: BayKind::Agent(Box::new(view)) });
        self.focus(self.bays.len() - 1);
        if self.models.is_empty() {
            self.backend.send(Cmd::ListModels);
        }
        id
    }

    fn create_session_folder(&mut self, first_message: &str) -> PathBuf {
        let root = self.settings.sessions_root();
        match sessions::create_folder(&root, &sessions::short_title(first_message)) {
            Ok(dir) => dir,
            Err(e) => {
                self.toast(Level::Error, format!("Couldn't create a folder in {}: {e}", settings::short_path(&root)));
                self.settings.default_dir()
            }
        }
    }

    /// The first message of a deferred session: create its folder, then start it with the message.
    fn start_deferred(&mut self, index: usize, text: String) {
        let dir = self.create_session_folder(&text);
        let project = self.project_for(&dir);
        let enabled = self.settings.memory.enabled;
        let Some(Bay { id, kind: BayKind::Agent(view) }) = self.bays.get_mut(index) else { return };
        let bay = *id;
        let files = std::mem::take(&mut view.attachments);
        view.granted.lock().extend(files.iter().cloned());
        view.cwd = dir;
        view.project = project;
        view.deferred = false;
        view.title = Some(text.clone());
        let names = files.iter().map(|p| settings::dir_name(p)).collect();
        view.echo_user_with(&text, names);
        capture_explicit_memory(&self.memory, enabled, view, &text);
        let Some(Bay { kind: BayKind::Agent(view), .. }) = self.bays.get(index) else { return };
        let spec = self.agent_spec(view, Some(text.clone()), files, None);
        self.backend.send(Cmd::Create { bay, spec });
        let name = view.name.clone().unwrap_or_else(|| sessions::short_title(&text));
        let user = view.name.is_some();
        self.backend.send(Cmd::NameSession { bay, name, user });
    }

    /// Memory scope for a folder: every session folder shares the sessions folder's scope.
    fn project_for(&self, dir: &std::path::Path) -> Project {
        let root = self.settings.sessions_root();
        Project::for_dir(if sessions::is_under(dir, &root) { &root } else { dir })
    }

    fn agent_spec(&self, view: &AgentView, prompt: Option<String>, files: Vec<PathBuf>, resume: Option<String>) -> AgentSpec {
        let memory = (self.settings.memory.enabled && !view.incognito).then(|| crate::agent::backend::AgentMemory {
            store: self.memory.clone(),
            project: view.project.clone(),
        });
        AgentSpec {
            cwd: view.cwd.clone(),
            // Choices made before the session existed (a deferred one) carry over.
            model: view.model.clone().or_else(|| self.settings.default_model.clone()),
            mode: view.mode,
            prompt,
            files,
            auto_approve: view.auto_approve.clone(),
            resume,
            granted: view.granted.clone(),
            memory,
        }
    }

    /// A restored session that no longer exists (deleted, or never saved because it stayed
    /// empty) continues as a fresh agent in the same folder instead of a dead bay.
    fn restart_fresh(&mut self, bay: BayId) -> bool {
        let Some(v) = self.agent_mut(bay) else { return false };
        if v.session_id.is_none() {
            return false;
        }
        v.items.retain(|i| !matches!(i, Item::Notice { text, .. } if text == "Resuming session…"));
        v.session_id = None;
        v.learn_cursor = 0;
        v.notice(Level::Info, "That session is no longer available, so this is a new one.");
        let Some(Bay { kind: BayKind::Agent(v), .. }) = self.bays.iter().find(|b| b.id == bay) else { return false };
        let spec = self.agent_spec(v, None, Vec::new(), None);
        self.backend.send(Cmd::Create { bay, spec });
        true
    }

    // ------------------------------------------------------------------ memory

    /// "Remember that …" in a prompt is saved right away (the agent still gets the message).
    fn capture_explicit_memory(&mut self, view: &mut AgentView, prompt: &str) {
        capture_explicit_memory(&self.memory, self.settings.memory.enabled, view, prompt);
    }

    /// Read a finished conversation in the background and file what's worth keeping.
    fn start_learning(&mut self, index: usize) {
        let model = self.curator_model();
        let Some(Bay { id, kind: BayKind::Agent(v) }) = self.bays.get_mut(index) else { return };
        let bay = *id;
        if v.incognito || self.learn_jobs.contains_key(&bay) {
            return;
        }
        let from = if v.learn_cursor == usize::MAX { v.items.len() } else { v.learn_cursor };
        v.learn_cursor = v.items.len();
        v.learn_due = None;
        let Some(transcript) = v.transcript_since(from) else { return };
        let transcript = crate::memory::redact_secrets(&transcript);
        let message = {
            let store = self.memory.lock();
            let existing = store.curator_context(Some(&v.project), &transcript, 40);
            crate::memory::curator_message(&existing, Some(&v.project), &transcript)
        };
        let job = LearnJob { project_root: v.project.root.clone(), session: v.session_id.clone(), title: v.display_title(), message: message.clone() };
        self.learn_jobs.insert(bay, job);
        self.backend.send(Cmd::Learn { bay, model, message });
    }

    /// Conversations not yet read when the app closes are queued and read on the next launch.
    fn queue_unread_conversations(&mut self) {
        if !self.settings.memory.learning() {
            return;
        }
        let mut queue: Vec<LearnJob> = self.learn_jobs.drain().map(|(_, j)| j).collect();
        for b in &self.bays {
            let BayKind::Agent(v) = &b.kind else { continue };
            if v.incognito || v.learn_cursor == usize::MAX {
                continue;
            }
            if let Some(transcript) = v.transcript_since(v.learn_cursor) {
                let transcript = crate::memory::redact_secrets(&transcript);
                let store = self.memory.lock();
                let existing = store.curator_context(Some(&v.project), &transcript, 40);
                let message = crate::memory::curator_message(&existing, Some(&v.project), &transcript);
                queue.push(LearnJob { project_root: v.project.root.clone(), session: v.session_id.clone(), title: v.display_title(), message });
            }
        }
        if !queue.is_empty() {
            settings::write_json(&learn_queue_path(), &queue);
        }
    }

    /// Pick up conversations queued by the previous run (once models are known).
    fn resume_learn_queue(&mut self) {
        let path = learn_queue_path();
        let Some(queue) = settings::read_json::<Vec<LearnJob>>(&path) else { return };
        let _ = std::fs::remove_file(&path);
        if !self.settings.memory.learning() {
            return;
        }
        let model = self.curator_model();
        for job in queue.into_iter().take(6) {
            let bay = self.alloc_bay();
            self.backend.send(Cmd::Learn { bay, model: model.clone(), message: job.message.clone() });
            self.learn_jobs.insert(bay, job);
        }
    }

    /// The model the curator should use: the user's pick, else the cheapest available.
    fn curator_model(&self) -> Option<String> {
        if let Some(m) = &self.settings.memory.model {
            return Some(m.clone());
        }
        let cost = |m: &ModelInfo| {
            let small = ["mini", "flash", "haiku", "nano", "lite"].iter().any(|k| m.id.to_lowercase().contains(k));
            (m.multiplier.unwrap_or(1.0) * 1000.0) as i64 * 2 + if small { 0 } else { 1 }
        };
        self.models.iter().min_by_key(|m| cost(m)).map(|m| m.id.clone())
    }

    /// Curator results: apply them, then tell the agent's transcript and the user.
    fn finish_learning(&mut self, bay: BayId, result: Result<String, String>) {
        let Some(job) = self.learn_jobs.remove(&bay) else { return };
        self.memory_panel.last_run = Some((crate::memory::now(), result.as_ref().err().cloned()));
        let Ok(content) = result else { return };
        let project = Project::for_dir(&job.project_root);
        let ops = crate::memory::parse_curator(&content, Some(&project));
        tracing::info!(ops = ops.len(), "memory curator finished");
        if ops.is_empty() {
            return;
        }
        let source = Source { session: job.session, title: Some(job.title), quote: None, at: crate::memory::now() };
        let outcome = self.memory.lock().apply(ops, source, self.settings.memory.auto_accept());
        let saved = outcome.saved.len();
        if saved == 0 && outcome.suggested == 0 && outcome.updated == 0 {
            return;
        }
        let text = match (saved, outcome.saved.first()) {
            (1, Some((_, t))) => t.clone(),
            (n, _) if n > 1 => format!("{n} things from this conversation"),
            _ => format!("{} update{} to existing memories", outcome.updated, if outcome.updated == 1 { "" } else { "s" }),
        };
        if saved > 0 || outcome.updated > 0 {
            match self.agent_mut(bay) {
                Some(v) => {
                    v.push(Item::Memory { ids: outcome.saved.iter().map(|(id, _)| *id).collect(), text, learned: true, undone: false });
                }
                // The conversation is closed (or was queued at exit): say it here instead.
                None => self.toast(Level::Ok, format!("Learned: {text}")),
            }
        }
        if outcome.suggested > 0 {
            self.toast(Level::Info, format!("{} new memor{} to review", outcome.suggested, if outcome.suggested == 1 { "y" } else { "ies" }));
        }
    }

    /// Per-frame memory housekeeping: due curator runs, approval learning, persistence.
    fn memory_tick(&mut self) {
        crate::memory::set_switches(self.settings.memory.enabled, self.settings.memory.inject);
        let learning = self.settings.memory.learning();
        let now = Instant::now();
        let mut due = Vec::new();
        let mut approvals = Vec::new();
        for (i, b) in self.bays.iter_mut().enumerate() {
            let BayKind::Agent(v) = &mut b.kind else { continue };
            if !learning || v.incognito {
                v.learn_due = None;
                v.approval_log.clear();
                // Nothing said while learning is off is ever learned later.
                if v.learn_cursor != usize::MAX {
                    v.learn_cursor = v.items.len();
                }
                continue;
            }
            if v.learn_due.is_some_and(|t| now >= t) && !v.status.is_busy() && !v.status.needs_attention() {
                due.push(i);
            }
            for (cmds, ok) in v.approval_log.drain(..) {
                approvals.push((v.project.clone(), cmds, ok));
            }
        }
        for i in due {
            self.start_learning(i);
        }
        for (project, cmds, ok) in approvals {
            let suggestion = self.memory.lock().record_approval(&project, &cmds, ok);
            if let Some(s) = suggestion {
                self.toast(Level::Info, format!("Suggested rule: {s} — review it in Memory"));
            }
        }
    }

    fn memory_action(&mut self, a: MemoryAction) {
        match a {
            MemoryAction::SettingsChanged => self.settings_dirty = true,
            MemoryAction::Toast(t) => self.toast(Level::Ok, t),
            MemoryAction::ExportJson => self.pick_save(Pick::ExportMemory { markdown: false }, "graysense-memory.json"),
            MemoryAction::ExportMarkdown => self.pick_save(Pick::ExportMemory { markdown: true }, "graysense-memory.md"),
            MemoryAction::Import => {
                let (tx, rx) = channel();
                std::thread::spawn(move || {
                    let file = rfd::FileDialog::new().set_title("Import memories").add_filter("Graysense memory", &["json"]).pick_file();
                    let _ = tx.send(file.into_iter().collect());
                });
                self.picker = Some((Pick::ImportMemory, rx));
            }
        }
    }

    fn pick_save(&mut self, purpose: Pick, name: &str) {
        let (tx, rx) = channel();
        let name = name.to_owned();
        std::thread::spawn(move || {
            let file = rfd::FileDialog::new().set_title("Export memories").set_file_name(name).save_file();
            let _ = tx.send(file.into_iter().collect());
        });
        self.picker = Some((purpose, rx));
    }

    fn open_memory(&mut self, id: Option<MemoryId>) {
        if self.settings.side_panel != Some(SidePanel::Memory) {
            self.set_side(Some(SidePanel::Memory));
        }
        if let Some(id) = id {
            let t = self.t0.elapsed().as_secs_f64();
            let store = self.memory.lock();
            self.memory_panel.reveal(id, &store, t);
        }
    }

    /// The project of the focused bay (for the Memory panel's project filter).
    fn focused_project(&self) -> Option<Project> {
        match &self.bays.get(self.focused)?.kind {
            BayKind::Agent(v) => Some(v.project.clone()),
            BayKind::Shell(p) => Some(Project::for_dir(&p.cwd)),
        }
    }

    fn focused_dir(&self) -> Option<PathBuf> {
        match &self.bays.get(self.focused)?.kind {
            BayKind::Agent(v) => Some(v.cwd.clone()),
            BayKind::Shell(p) => Some(p.cwd.clone()),
        }
    }

    /// The focused bay's folder, unless it's a session that hasn't got one yet.
    fn live_focused_dir(&self) -> Option<PathBuf> {
        match &self.bays.get(self.focused)?.kind {
            BayKind::Agent(v) if v.deferred => None,
            _ => self.focused_dir().filter(|d| d.is_dir()),
        }
    }

    /// Resume a past session in the directory it was created in.
    fn resume_session(&mut self, id: String) {
        if let Some(i) = self.bays.iter().position(|b| matches!(&b.kind, BayKind::Agent(v) if v.session_id.as_deref() == Some(id.as_str()))) {
            self.focus(i);
            return;
        }
        let dir = session_cwd(&id).filter(|d| d.is_dir()).unwrap_or_else(|| self.settings.default_dir());
        self.new_agent(dir, None, Some(id), None);
    }

    /// Files dropped on the welcome screen: open an agent where they live, with them attached.
    fn start_agent_with_files(&mut self, dropped: Vec<PathBuf>) {
        let Some(first) = dropped.first() else { return };
        let cwd = if first.is_dir() { first.clone() } else { first.parent().map(PathBuf::from).unwrap_or_else(|| self.settings.default_dir()) };
        let id = self.new_agent(cwd, None, None, None);
        if let Some(v) = self.agent_mut(id) {
            v.attachments = dropped.into_iter().filter(|p| !p.is_dir()).collect();
            v.focus_composer = true;
        }
    }

    fn new_shell(&mut self, ctx: &egui::Context, cwd: PathBuf) {
        let program = settings::resolve_shell(self.settings.shell.as_deref());
        let args = if program.to_lowercase().contains("powershell") || program.to_lowercase().contains("pwsh") { vec!["-NoLogo".into()] } else { Vec::new() };
        match TermPane::spawn(ctx, SpawnSpec { program, args, cwd: cwd.clone() }, &self.theme, 120, 32) {
            Ok(pane) => {
                let id = self.alloc_bay();
                self.bays.push(Bay { id, kind: BayKind::Shell(Box::new(pane)) });
                self.focus(self.bays.len() - 1);
                if !sessions::is_under(&cwd, &self.settings.sessions_root()) {
                    self.settings.push_recent_dir(&cwd);
                    self.settings_dirty = true;
                }
            }
            Err(e) => self.toast(Level::Error, format!("Couldn't start shell: {e}")),
        }
    }

    fn close_bay(&mut self, index: usize) {
        if index >= self.bays.len() {
            return;
        }
        if self.settings.memory.learning() {
            self.start_learning(index);
        }
        let bay = self.bays.remove(index);
        if let BayKind::Agent(_) = bay.kind {
            self.backend.send(Cmd::Close { bay: bay.id });
            self.apps.close_bay(bay.id);
            self.missions.on_bay_closed(bay.id);
        }
        if self.focused >= self.bays.len() {
            self.focused = self.bays.len().saturating_sub(1);
        }
        if self.bays.len() <= 1 {
            self.maximized = false;
        }
        self.focus_bay = true;
    }

    fn launch_mission(&mut self, id: u64) {
        let Some(m) = self.missions.get(id).cloned() else { return };
        let bay = match m.dir.as_ref().map(PathBuf::from).filter(|p| p.is_dir()) {
            Some(dir) => self.new_agent(dir, Some(m.brief()), None, Some(id)),
            None => {
                let bay = self.new_session(Some(m.brief()), false);
                if let Some(v) = self.agent_mut(bay) {
                    v.mission = Some(id);
                }
                bay
            }
        };
        self.missions.link(id, bay);
        self.toast(Level::Ok, format!("Launched “{}”", m.title));
    }

    fn send_mission_to(&mut self, index: usize, id: u64) {
        let Some(m) = self.missions.get(id).cloned() else { return };
        let Some(bay) = self.bays.get_mut(index) else { return };
        let bay_id = bay.id;
        match &mut bay.kind {
            BayKind::Agent(v) if v.deferred => {
                v.mission = Some(id);
                self.start_deferred(index, m.brief());
                self.missions.link(id, bay_id);
                self.toast(Level::Ok, format!("Delegated “{}”", m.title));
            }
            BayKind::Agent(v) => {
                v.echo_user(&m.brief());
                v.mission = Some(id);
                self.backend.send(Cmd::Send { bay: bay_id, text: m.brief(), files: Vec::new() });
                self.missions.link(id, bay_id);
                self.toast(Level::Ok, format!("Delegated “{}”", m.title));
            }
            BayKind::Shell(p) => p.paste(&m.brief()),
        }
        self.focus(index);
    }

    fn toast(&mut self, level: Level, text: impl Into<String>) {
        self.toasts.push(Toast { text: text.into(), level, born: Instant::now() });
        if self.toasts.len() > 5 {
            self.toasts.remove(0);
        }
    }

    fn log(&mut self, line: LogLine) {
        self.activity.push_front(line);
        self.activity.truncate(300);
    }

    fn attention(&self, ctx: &egui::Context) {
        if !self.window_focused {
            ctx.send_viewport_cmd(ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
        }
    }

    fn pick_dir(&mut self, purpose: Pick) {
        let (tx, rx) = channel();
        let start = self.settings.default_dir();
        std::thread::spawn(move || {
            let dir = rfd::FileDialog::new().set_directory(start).pick_folder();
            let _ = tx.send(dir.into_iter().collect());
        });
        self.picker = Some((purpose, rx));
    }

    fn pick_attachments(&mut self, bay: BayId, start: PathBuf) {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let files = rfd::FileDialog::new().set_title("Attach files").set_directory(start).pick_files().unwrap_or_default();
            let _ = tx.send(files);
        });
        self.picker = Some((Pick::Attach(bay), rx));
    }

    fn term_metrics(&mut self, ctx: &egui::Context) -> term_render::Metrics {
        let size = self.settings.term_font_size;
        let ppp = ctx.pixels_per_point();
        match &self.metrics {
            Some((s, p, m)) if *s == size && *p == ppp => m.clone(),
            _ => {
                let m = term_render::metrics(ctx, size);
                self.metrics = Some((size, ppp, m.clone()));
                m
            }
        }
    }

    fn set_theme(&mut self, ctx: &egui::Context, id: theme::ThemeId) {
        self.settings.theme = id;
        self.theme = Theme::get(id);
        theme::apply_style(ctx, &self.theme);
        self.native.apply_theme(self.theme.border_strong, self.theme.dark);
        for b in &self.bays {
            if let BayKind::Shell(p) = &b.kind {
                p.set_palette(&self.theme);
            }
        }
        self.settings_dirty = true;
    }

    fn focused_agent_bay(&self) -> Option<BayId> {
        let b = self.bays.get(self.focused)?;
        matches!(b.kind, BayKind::Agent(_)).then_some(b.id).or_else(|| self.bays.iter().find(|b| matches!(b.kind, BayKind::Agent(_))).map(|b| b.id))
    }

    fn refresh_side(&mut self) {
        match self.settings.side_panel {
            Some(SidePanel::Mcp) => {
                let cwd = self.bays.get(self.focused).map(|b| match &b.kind {
                    BayKind::Agent(v) => v.cwd.display().to_string(),
                    BayKind::Shell(p) => p.cwd.display().to_string(),
                });
                self.backend.send(Cmd::Mcp(McpCmd::Refresh { bay: self.focused_agent_bay(), cwd }));
            }
            Some(SidePanel::Plugins) => self.backend.send(Cmd::Plugin(PluginCmd::Refresh)),
            Some(SidePanel::Sessions) => self.backend.send(Cmd::ListSessions),
            _ => {}
        }
    }

    fn set_side(&mut self, panel: Option<SidePanel>) {
        self.settings.side_panel = panel;
        self.settings_dirty = true;
        self.refresh_side();
    }

    /// Search sessions and their files (Ctrl+K).
    fn open_search(&mut self, query: &str) {
        self.search_ui.open_with(query);
        self.refresh_search_index(false);
    }

    /// Rebuild the search index in the background (unless one is fresh or already building).
    fn refresh_search_index(&mut self, force: bool) {
        if self.search_rx.is_some() || (!force && self.search_built.is_some_and(|t| t.elapsed() < Duration::from_secs(30))) {
            return;
        }
        let root = self.settings.sessions_root();
        let mut transcripts = 0;
        let sources: Vec<crate::search::SessionSource> = self
            .sessions
            .iter()
            .map(|s| {
                let folder = self.session_index.get(&s.id).and_then(|m| m.folder.clone()).map(PathBuf::from);
                let ours = s.client.as_deref().is_some_and(|c| c.starts_with("graysense")) || folder.as_deref().is_some_and(|f| sessions::is_under(f, &root));
                // Conversations are indexed for Graysense's own recent sessions; others by name.
                let transcript = ours && {
                    transcripts += 1;
                    transcripts <= 300
                };
                let modified = chrono::DateTime::parse_from_rfc3339(&s.modified).map(|d| d.timestamp()).unwrap_or(0);
                crate::search::SessionSource { id: s.id.clone(), title: self.session_title(s), folder, modified, transcript }
            })
            .collect();
        let state_dir = dirs::home_dir().unwrap_or_default().join(".copilot").join("session-state");
        let cache_dir = settings::config_dir().join("search");
        let (tx, rx) = channel();
        let ctx = self.egui_ctx.clone();
        std::thread::spawn(move || {
            let index = crate::search::Index::build(&root, &sources, &state_dir, &cache_dir);
            let _ = tx.send(index);
            ctx.request_repaint();
        });
        self.search_rx = Some(rx);
    }

    fn poll_search_index(&mut self) {
        let Some(rx) = &self.search_rx else { return };
        if let Ok(index) = rx.try_recv() {
            tracing::info!(sessions = index.stats.sessions, transcripts = index.stats.transcripts, files = index.stats.files, kb = index.stats.bytes / 1024, ms = index.stats.millis as u64, "search index built");
            self.search_index = Arc::new(index);
            self.search_gen += 1;
            self.search_built = Some(Instant::now());
            self.search_rx = None;
        }
    }

    fn search_overlay(&mut self, ctx: &egui::Context) {
        let stats = &self.search_index.stats;
        let status = if self.search_rx.is_some() && self.search_built.is_none() {
            "Indexing…".to_owned()
        } else {
            format!("{} sessions  ·  {} files{}", stats.sessions, stats.files, if self.search_rx.is_some() { "  ·  updating…" } else { "" })
        };
        let focused_agent = self.bays.get(self.focused).filter(|b| matches!(b.kind, BayKind::Agent(_))).map(|b| b.id);
        let index = self.search_index.clone();
        let theme = self.theme.clone();
        let Some(action) = self.search_ui.show(ctx, &theme, &index, self.search_gen, &status, focused_agent.is_some()) else { return };
        use crate::search_ui::SearchAction;
        match action {
            SearchAction::OpenSession(id) => self.resume_session(id),
            SearchAction::OpenFile(p) => open_path(&p),
            SearchAction::RevealFile(p) => reveal_path(&p),
            SearchAction::AttachFile(p) => {
                if let Some(v) = focused_agent.and_then(|b| self.agent_mut(b)) {
                    let name = settings::dir_name(&p);
                    if !v.attachments.contains(&p) {
                        v.attachments.push(p);
                    }
                    v.focus_composer = true;
                    self.focus_bay = true;
                    self.toast(Level::Ok, format!("Attached {name}"));
                }
            }
        }
    }

    /// Every shortcut on one sheet (Ctrl+/), like chat apps and editors offer.
    fn shortcuts_overlay(&mut self, ctx: &egui::Context) {
        type Group = (&'static str, &'static [(&'static [&'static str], &'static str)]);
        const GROUPS: [Group; 4] = [
            (
                "General",
                &[
                    (&["Ctrl", "K"], "Search sessions and files"),
                    (&["Ctrl", "Shift", "P"], "Command palette"),
                    (&["Ctrl", "/"], "Keyboard shortcuts"),
                    (&["Ctrl", "Shift", "B"], "Toggle the side panel"),
                    (&["Ctrl", "Shift", "H"], "Sessions"),
                    (&["Ctrl", "Shift", "M"], "Memory"),
                    (&["Ctrl", "Shift", "I"], "Capture an idea"),
                ],
            ),
            (
                "Sessions and bays",
                &[
                    (&["Ctrl", "Shift", "N"], "New session"),
                    (&["Ctrl", "Shift", "T"], "New terminal in the current folder"),
                    (&["Ctrl", "Shift", "W"], "Close bay"),
                    (&["Ctrl", "Tab"], "Next bay"),
                    (&["Ctrl", "Alt", "1–9"], "Go to bay"),
                    (&["Ctrl", "Shift", "Enter"], "Maximize bay"),
                    (&["Ctrl", "Shift", "L"], "Cycle layout"),
                ],
            ),
            (
                "Message box",
                &[(&["Enter"], "Send"), (&["Shift", "Enter"], "New line"), (&["Esc"], "Stop the agent"), (&["↑"], "Previous message"), (&["/"], "Commands"), (&["Tab"], "Complete a command")],
            ),
            ("Terminal", &[(&["Ctrl", "="], "Larger font"), (&["Ctrl", "-"], "Smaller font"), (&["Ctrl", "0"], "Reset font size")]),
        ];
        let theme = self.theme.clone();
        let frame = Frame::new().fill(theme.surface).stroke(Stroke::new(1.0, theme.border_strong)).corner_radius(CornerRadius::same(12)).inner_margin(Margin::same(20));
        let modal = egui::Modal::new(Id::unique("shortcuts")).frame(frame).backdrop_color(theme::with_alpha(Color32::BLACK, 0.45)).show(ctx, |ui| {
            ui.set_width(660.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Keyboard shortcuts").font(semibold(16.0)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if kit::icon_button_sized(ui, icon::CANCEL, "Close (Esc)", &theme, 26.0, false).clicked() {
                        self.show_shortcuts = false;
                    }
                });
            });
            ui.add_space(10.0);
            ui.columns(2, |cols| {
                for (gi, (group, items)) in GROUPS.iter().enumerate() {
                    let ui = &mut cols[if gi % 2 == 0 { 0 } else { 1 }];
                    kit::section(ui, group, &theme);
                    for (keys, what) in items.iter() {
                        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
                        ui.painter().text(pos2(r.left(), r.center().y), Align2::LEFT_CENTER, *what, ui_font(12.5), theme.text);
                        kit::kbd(ui.painter(), pos2(r.right() - 6.0, r.center().y), keys, &theme);
                    }
                    ui.add_space(8.0);
                }
            });
        });
        if modal.should_close() {
            self.show_shortcuts = false;
        }
    }

    // ------------------------------------------------------------------ updates

    /// Tell the user when an installed copy starts on a new version.
    fn note_new_version(&mut self) {
        if !self.updater.installed() {
            return;
        }
        let now = self.updater.version.clone();
        if self.settings.last_version.as_deref() == Some(now.as_str()) {
            return;
        }
        if self.settings.last_version.is_some() {
            self.toast(Level::Ok, format!("Graysense updated to {now}. Its release notes are in Settings › About."));
        }
        self.settings.last_version = Some(now);
        self.settings_dirty = true;
    }

    /// Open Settings scrolled to About, where update progress and errors show.
    fn reveal_updates(&mut self) {
        self.set_side(Some(SidePanel::Settings));
        self.reveal_about = true;
    }

    fn update_action(&mut self, ctx: &egui::Context, a: UpdateAction) {
        match a {
            UpdateAction::Check => self.updater.check(),
            UpdateAction::Download => self.updater.download(),
            UpdateAction::Restart => self.restart_to_update(ctx, false),
            UpdateAction::SetAuto(on) => {
                self.settings.auto_update = on;
                self.settings_dirty = true;
                self.updater.set_auto(on);
            }
            UpdateAction::OpenUrl(u) => ctx.open_url(egui::OpenUrl::new_tab(u)),
        }
    }

    /// Install the downloaded update and reopen Graysense with the same bays. Asks first while
    /// agents are working, since restarting stops them.
    fn restart_to_update(&mut self, ctx: &egui::Context, confirmed: bool) {
        let working = self.bays.iter().filter(|b| matches!(&b.kind, BayKind::Agent(v) if v.status.is_busy())).count();
        if working > 0 && !confirmed {
            self.confirm_restart = true;
            return;
        }
        self.confirm_restart = false;
        match self.updater.restart() {
            // Closing saves the workspace; the installer waits for this process to exit.
            Ok(()) => ctx.send_viewport_cmd(ViewportCommand::Close),
            Err(e) => self.toast(Level::Error, format!("Couldn't install the update: {e}")),
        }
    }

    fn restart_confirm_ui(&mut self, ctx: &egui::Context) {
        let theme = self.theme.clone();
        let working = self.bays.iter().filter(|b| matches!(&b.kind, BayKind::Agent(v) if v.status.is_busy())).count();
        let version = self.updater.next.as_ref().map(|(v, _)| v.clone()).unwrap_or_default();
        let frame = Frame::new().fill(theme.surface).stroke(Stroke::new(1.0, theme.border_strong)).corner_radius(CornerRadius::same(12)).inner_margin(Margin::same(20));
        let mut choice = None;
        let modal = egui::Modal::new(Id::unique("restart-to-update")).frame(frame).backdrop_color(theme::with_alpha(Color32::BLACK, 0.45)).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(RichText::new(format!("Restart to install Graysense {version}?")).font(semibold(15.0)));
            ui.add_space(6.0);
            let agents = if working == 1 { "An agent is".to_owned() } else { format!("{working} agents are") };
            ui.add(egui::Label::new(RichText::new(format!("{agents} still working. Restarting stops them; your sessions reopen when Graysense is back.")).color(theme.text_dim)).wrap());
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if kit::primary_button(ui, "Restart now", &theme).clicked() {
                    choice = Some(true);
                }
                if kit::ghost_button(ui, "Not now", &theme).clicked() {
                    choice = Some(false);
                }
            });
        });
        if modal.should_close() {
            choice.get_or_insert(false);
        }
        match choice {
            Some(true) => self.restart_to_update(ctx, true),
            Some(false) => self.confirm_restart = false,
            None => {}
        }
    }

    /// Apply settings an agent changed (the Settings panel applies its own as you click).
    fn settings_changed(&mut self, ctx: &egui::Context, before: &Settings) {
        if self.settings.theme != before.theme {
            let t = self.settings.theme;
            self.set_theme(ctx, t);
        }
        if self.settings.term_font_size != before.term_font_size {
            self.set_font(self.settings.term_font_size);
        }
        if self.settings.side_panel != before.side_panel {
            self.set_side(self.settings.side_panel);
        }
        if self.settings.side_width != before.side_width {
            self.force_side_width = true;
        }
        if self.settings.layout != before.layout {
            self.maximized = false;
        }
        if self.settings.auto_update != before.auto_update {
            self.updater.set_auto(self.settings.auto_update);
        }
        crate::memory::set_switches(self.settings.memory.enabled, self.settings.memory.inject);
        self.settings_dirty = true;
    }

    // ------------------------------------------------------------------ events

    fn drain_events(&mut self, ctx: &egui::Context) {
        while let Ok(evt) = self.evt_rx.try_recv() {
            self.handle_event(ctx, evt);
        }
    }

    fn handle_event(&mut self, ctx: &egui::Context, evt: Evt) {
        match evt {
            Evt::Client(state) => {
                if let ClientState::Failed(e) = &state {
                    self.toast(Level::Error, format!("Copilot runtime failed to start: {e}"));
                }
                if matches!(state, ClientState::Ready) {
                    self.backend.send(Cmd::ListModels);
                    self.backend.send(Cmd::ListSessions);
                }
                self.client = state;
            }
            Evt::Config { changes, reply } => {
                let models: Vec<String> = self.models.iter().map(|m| m.id.clone()).collect();
                let result = match changes {
                    None => Ok(serde_json::json!({ "settings": self.settings.describe(&models), "to_change": "Call again with action \"set\" and `changes`." })),
                    Some(changes) => {
                        let before = self.settings.clone();
                        self.settings.apply_patch(&changes, &models).map(|done| {
                            self.settings_changed(ctx, &before);
                            self.toast(Level::Ok, format!("Copilot changed {}", done.join(", ")));
                            serde_json::json!({ "changed": done })
                        })
                    }
                };
                let _ = reply.send(result);
            }
            Evt::SessionReady { bay, session_id } => {
                self.missions.on_session(bay, &session_id);
                let mut known = None;
                if let Some(v) = self.agent_mut(bay) {
                    v.session_id = Some(session_id.clone());
                    if v.status == Status::Booting {
                        v.status = Status::Idle;
                    }
                    known = Some((v.cwd.display().to_string(), v.title.as_deref().map(sessions::short_title), v.name.clone()));
                }
                if let Some((folder, auto_name, name)) = known {
                    let e = self.session_index.entry(&session_id);
                    e.folder = Some(folder);
                    if e.auto_name.is_none() {
                        e.auto_name = auto_name;
                    }
                    if name.is_some() {
                        e.name = name;
                    }
                }
                if self.settings.side_panel == Some(SidePanel::Mcp) {
                    self.refresh_side();
                }
            }
            Evt::SessionFailed { bay, error } => {
                if error.contains("not found") && self.restart_fresh(bay) {
                    return;
                }
                if let Some(v) = self.agent_mut(bay) {
                    let hint = if error.contains("unavailable") {
                        " Install the GitHub Copilot CLI and sign in with `copilot login`, then open a new agent."
                    } else {
                        ""
                    };
                    v.notice(Level::Error, format!("Couldn't start the agent: {error}.{hint}"));
                    v.status = Status::Error(error);
                }
            }
            Evt::SessionClosed { bay } => {
                if let Some(v) = self.agent_mut(bay) {
                    v.status = Status::Offline;
                }
            }
            Evt::Event { bay, ev } => {
                let effects = match self.agent_mut(bay) {
                    Some(v) => v.apply(&ev),
                    None => return,
                };
                self.apply_effects(ctx, bay, effects);
            }
            Evt::History { bay, events } => {
                if let Some(v) = self.agent_mut(bay) {
                    v.items.retain(|i| !matches!(i, Item::Notice { text, .. } if text == "Resuming session…"));
                    for ev in &events {
                        let _ = v.apply(ev);
                    }
                    // Learning picks up from here: the history was read when it happened.
                    if v.learn_cursor == usize::MAX {
                        v.learn_cursor = v.items.len();
                    }
                    v.status = Status::Idle;
                }
            }
            Evt::Notice { bay, error, text } => {
                if let Some(v) = self.agent_mut(bay) {
                    v.notice(if error { Level::Error } else { Level::Info }, text);
                }
            }
            Evt::Toast { error, text } => self.toast(if error { Level::Error } else { Level::Ok }, text),
            Evt::Permission { bay, prompt } => self.push_prompt(ctx, bay, Item::Permission(prompt), "approval"),
            Evt::Elicit { bay, prompt } => self.push_prompt(ctx, bay, Item::Elicit(prompt), "input"),
            Evt::Ask { bay, prompt } => self.push_prompt(ctx, bay, Item::Ask(prompt), "an answer"),
            Evt::Plan { bay, prompt } => self.push_prompt(ctx, bay, Item::Plan(prompt), "plan review"),
            Evt::Switch { bay, prompt } => self.push_prompt(ctx, bay, Item::Switch(prompt), "a decision"),
            Evt::CanvasOpen { bay, instance_id, canvas_id, input } => {
                if let Some(v) = self.agent_mut(bay)
                    && let Some(c) = CanvasInstance::new(instance_id.clone(), canvas_id, &input) {
                        v.canvases.retain(|x| x.instance_id != instance_id);
                        v.canvases.push(c);
                        v.dock_open = true;
                        v.dock_tab = v.canvases.len() - 1;
                    }
            }
            Evt::CanvasAction { bay, instance_id, action, input, reply } => {
                let result = match self.agent_mut(bay).and_then(|v| v.canvases.iter_mut().find(|c| c.instance_id == instance_id)) {
                    Some(c) => c.apply(&action, &input),
                    None => Err("canvas is not open".into()),
                };
                let _ = reply.send(result);
            }
            Evt::CanvasClosed { bay, instance_id } => {
                if let Some(v) = self.agent_mut(bay) {
                    v.canvases.retain(|c| c.instance_id != instance_id);
                }
            }
            Evt::Models(m) => {
                let first = self.models.is_empty();
                self.models = m;
                if first {
                    self.resume_learn_queue();
                }
            }
            Evt::Sessions(s) => {
                self.sessions = s;
                if self.search_built.is_none() {
                    self.refresh_search_index(true);
                }
            }
            Evt::McpServers { servers, live_bay } => {
                self.mcp.servers = Some(servers);
                self.mcp.live_bay = live_bay;
                self.mcp.loading = false;
            }
            Evt::McpTools { server, tools } => {
                self.mcp.tools.insert(server, tools);
            }
            Evt::Plugins(p) => self.plugins.installed = Some(p),
            Evt::Markets(m) => self.plugins.markets = Some(m),
            Evt::MarketPlugins { market, plugins } => {
                self.plugins.browse.insert(market, plugins);
            }
            Evt::AppReply { view, req_id, result } => self.apps.reply(view, req_id, result),
            Evt::AddMission { bay, title, notes } => {
                self.missions.add(&title, &notes, true);
                let idx = self.bay_index(bay).map(|i| i + 1).unwrap_or(0);
                self.toast(Level::Info, format!("Agent {idx} added a mission: {title}"));
            }
            Evt::Notify { bay, title, message, level } => {
                let lvl = match level.as_str() {
                    "success" => Level::Ok,
                    "warning" => Level::Warn,
                    "error" => Level::Error,
                    _ => Level::Info,
                };
                let idx = self.bay_index(bay).map(|i| i + 1).unwrap_or(0);
                self.toast(lvl, format!("Agent {idx} · {title}: {message}"));
                self.attention(ctx);
            }
            Evt::Remembered { bay, id, text } => {
                if let Some(v) = self.agent_mut(bay) {
                    // The prompt itself may already have saved it ("remember that …").
                    let shown = v.items.iter().any(|i| matches!(i, Item::Memory { ids, .. } if ids.contains(&id)));
                    if !shown {
                        v.push(Item::Memory { ids: vec![id], text, learned: false, undone: false });
                    }
                }
            }
            Evt::Recalled { bay, entries } => {
                if let Some(v) = self.agent_mut(bay) {
                    for e in &entries {
                        if !v.memory_used.iter().any(|(id, _)| *id == e.0) {
                            v.memory_used.push(e.clone());
                        }
                    }
                    v.push(Item::Recall { entries, open: false });
                }
            }
            Evt::Learned { bay, result } => self.finish_learning(bay, result),
        }
    }

    fn push_prompt(&mut self, ctx: &egui::Context, bay: BayId, item: Item, what: &str) {
        let idx = self.bay_index(bay);
        let Some(v) = self.agent_mut(bay) else { return };
        v.push(item);
        v.status = Status::Waiting(what.to_owned());
        let title = v.display_title();
        if idx != Some(self.focused) || !self.window_focused {
            self.toast(Level::Warn, format!("{title} needs {what}"));
        }
        self.attention(ctx);
    }

    fn apply_effects(&mut self, ctx: &egui::Context, bay: BayId, effects: Vec<Effect>) {
        for e in effects {
            match e {
                Effect::Log(line) => self.log(line),
                Effect::OpenApp(req) => {
                    if let Some(view) = self.apps.open(bay, &req) {
                        let title = self.apps.title(view).unwrap_or_else(|| req.tool.clone());
                        if let Some(v) = self.agent_mut(bay) {
                            v.apps.push(AppTab { view, title });
                            v.dock_open = true;
                            v.dock_tab = v.canvases.len() + v.apps.len() - 1;
                        }
                    }
                }
                Effect::Finished => {
                    self.missions.on_agent_finished(bay);
                    if let Some(v) = self.agent_mut(bay) {
                        v.learn_due = Some(Instant::now() + LEARN_DELAY);
                    }
                    let idx = self.bay_index(bay);
                    if idx != Some(self.focused) || !self.window_focused {
                        let title = self.agent_mut(bay).map(|v| v.display_title()).unwrap_or_default();
                        self.toast(Level::Ok, format!("{title} finished"));
                        self.attention(ctx);
                    }
                }
                Effect::McpChanged => {
                    if self.settings.side_panel == Some(SidePanel::Mcp) && self.mcp_refresh_at.is_none() {
                        self.mcp_refresh_at = Some(Instant::now() + Duration::from_millis(400));
                        ctx.request_repaint_after(Duration::from_millis(450));
                    }
                }
            }
        }
    }

    fn poll_picker(&mut self, ctx: &egui::Context) {
        let Some((_, rx)) = &self.picker else { return };
        let Ok(paths) = rx.try_recv() else { return };
        let (purpose, _) = self.picker.take().unwrap();
        let Some(dir) = paths.first().cloned() else { return };
        match purpose {
            Pick::Agent => {
                self.new_agent(dir, None, None, None);
            }
            Pick::Shell => self.new_shell(ctx, dir),
            Pick::Mission(id) => self.missions.set_dir(id, dir),
            Pick::SessionsRoot => {
                self.toast(Level::Ok, format!("New sessions will go in {}", settings::short_path(&dir)));
                self.settings.sessions_root = Some(dir.display().to_string());
                self.settings_dirty = true;
            }
            Pick::Attach(bay) => {
                if let Some(v) = self.agent_mut(bay) {
                    for p in paths {
                        if !v.attachments.contains(&p) {
                            v.attachments.push(p);
                        }
                    }
                    v.focus_composer = true;
                }
            }
            Pick::ExportMemory { markdown } => {
                let text = {
                    let store = self.memory.lock();
                    if markdown {
                        store.export_markdown(&|key: &str| std::path::Path::new(key).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| key.to_owned()))
                    } else {
                        serde_json::to_string_pretty(&store.export_json()).unwrap_or_default()
                    }
                };
                match std::fs::write(&dir, text) {
                    Ok(()) => self.toast(Level::Ok, format!("Exported memory to {}", settings::short_path(&dir))),
                    Err(e) => self.toast(Level::Error, format!("Couldn't export: {e}")),
                }
            }
            Pick::ImportMemory => {
                let parsed = std::fs::read_to_string(&dir).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
                match parsed {
                    Some(v) => {
                        let n = self.memory.lock().import_json(&v);
                        self.toast(Level::Ok, format!("Imported {n} memor{} for review", if n == 1 { "y" } else { "ies" }));
                        self.memory_panel.show_review();
                    }
                    None => self.toast(Level::Error, "That file isn't a Graysense memory export"),
                }
            }
        }
    }

    fn pump_terms(&mut self, ctx: &egui::Context, dt: f32) {
        let sample = self.last_sample.elapsed() > Duration::from_millis(250);
        if sample {
            self.last_sample = Instant::now();
        }
        let mut clips = Vec::new();
        for b in self.bays.iter_mut() {
            match &mut b.kind {
                BayKind::Shell(p) => {
                    if let Some(c) = p.pump(dt) {
                        clips.push(c);
                    }
                    if sample {
                        p.sample_history();
                    }
                }
                BayKind::Agent(v) => {
                    v.activity = (v.activity - dt * 0.8).max(0.0);
                    if sample {
                        v.history.remove(0);
                        v.history.push(v.activity);
                    }
                }
            }
        }
        for c in clips {
            ctx.copy_text(c);
        }
    }

    // ------------------------------------------------------------------ commands

    fn run(&mut self, cmd: Command, ctx: &egui::Context) {
        match cmd {
            Command::NewAgent => {
                self.new_session(None, false);
            }
            Command::NewAgentIn => self.palette.open(PaletteMode::Dirs { agent: true }),
            Command::NewIncognitoAgent => {
                self.new_session(None, true);
            }
            Command::NewShell => {
                // Like VS Code's terminal: open where the focused bay works.
                let dir = self.live_focused_dir().unwrap_or_else(|| self.settings.default_dir());
                self.new_shell(ctx, dir);
            }
            Command::NewShellIn => self.palette.open(PaletteMode::Dirs { agent: false }),
            Command::Resume => {
                self.backend.send(Cmd::ListSessions);
                self.palette.open(PaletteMode::Sessions);
            }
            Command::ResumeSession(id) => self.resume_session(id),
            Command::OpenDir(d, agent) => {
                let dir = PathBuf::from(d);
                if agent {
                    self.new_agent(dir, None, None, None);
                } else {
                    self.new_shell(ctx, dir);
                }
            }
            Command::BrowseDir(agent) => self.pick_dir(if agent { Pick::Agent } else { Pick::Shell }),
            Command::CaptureIdea => {
                self.set_side(Some(SidePanel::Missions));
                self.capture_focus = true;
            }
            Command::CloseBay => self.close_bay(self.focused),
            Command::NextBay => {
                if !self.bays.is_empty() {
                    self.focus((self.focused + 1) % self.bays.len());
                }
            }
            Command::PrevBay => {
                if !self.bays.is_empty() {
                    self.focus((self.focused + self.bays.len() - 1) % self.bays.len());
                }
            }
            Command::Maximize => self.maximized = !self.maximized && self.bays.len() > 1,
            Command::Layout(l) => {
                self.settings.layout = l;
                self.maximized = false;
                self.settings_dirty = true;
            }
            Command::Theme(t) => self.set_theme(ctx, t),
            Command::Side(p) => self.set_side(Some(p)),
            Command::ToggleSide => {
                let next = if self.settings.side_panel.is_some() { None } else { Some(SidePanel::Missions) };
                self.set_side(next);
            }
            Command::FontBigger => self.set_font(self.settings.term_font_size + 1.0),
            Command::FontSmaller => self.set_font(self.settings.term_font_size - 1.0),
            Command::FontReset => self.set_font(13.5),
            Command::ToggleReasoning => {
                self.settings.show_reasoning = !self.settings.show_reasoning;
                self.settings_dirty = true;
            }
            Command::TogglePerf => {
                self.settings.perf_hud = !self.settings.perf_hud;
                self.settings_dirty = true;
            }
            Command::OpenConfig => {
                let _ = std::process::Command::new("explorer").arg(settings::config_dir()).spawn();
            }
            Command::ReloadMcp => {
                self.set_side(Some(SidePanel::Mcp));
            }
            Command::ToggleMemory => {
                let on = !self.settings.memory.enabled;
                self.settings.memory.enabled = on;
                self.settings_dirty = true;
                self.toast(Level::Ok, if on { "Memory is on" } else { "Memory is off: agents won't learn or remember" });
            }
            Command::ReviewMemories => {
                self.memory_panel.show_review();
                self.set_side(Some(SidePanel::Memory));
            }
            Command::Search => self.open_search(""),
            Command::Shortcuts => self.show_shortcuts = true,
            Command::CheckUpdates => {
                self.reveal_updates();
                self.updater.check();
            }
            Command::RestartToUpdate => self.restart_to_update(ctx, false),
        }
    }

    fn set_font(&mut self, size: f32) {
        self.settings.term_font_size = size.clamp(9.0, 28.0);
        self.settings_dirty = true;
        for b in &self.bays {
            if let BayKind::Shell(p) = &b.kind {
                p.dirty.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let sc = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
        let cs = Modifiers::CTRL | Modifiers::SHIFT;
        let mut cmds = Vec::new();
        if sc(cs, Key::P) || sc(Modifiers::NONE, Key::F1) {
            self.palette.open(PaletteMode::Commands);
        }
        if sc(cs, Key::N) {
            cmds.push(Command::NewAgent);
        }
        if sc(cs, Key::T) {
            cmds.push(Command::NewShell);
        }
        if sc(cs, Key::W) {
            cmds.push(Command::CloseBay);
        }
        if sc(cs, Key::Tab) {
            cmds.push(Command::PrevBay);
        } else if sc(Modifiers::CTRL, Key::Tab) {
            cmds.push(Command::NextBay);
        }
        if sc(cs, Key::Enter) {
            cmds.push(Command::Maximize);
        }
        if sc(cs, Key::L) {
            cmds.push(Command::Layout(self.settings.layout.next()));
        }
        if sc(cs, Key::I) {
            cmds.push(Command::CaptureIdea);
        }
        if sc(cs, Key::B) {
            cmds.push(Command::ToggleSide);
        }
        if sc(cs, Key::M) {
            cmds.push(if self.settings.side_panel == Some(SidePanel::Memory) { Command::ToggleSide } else { Command::Side(SidePanel::Memory) });
        }
        if sc(cs, Key::H) {
            cmds.push(if self.settings.side_panel == Some(SidePanel::Sessions) { Command::ToggleSide } else { Command::Side(SidePanel::Sessions) });
        }
        // Ctrl+K and Ctrl+/ belong to the shell while a terminal has focus (kill line, undo).
        let in_terminal = matches!(self.bays.get(self.focused), Some(Bay { kind: BayKind::Shell(_), .. })) && ctx.memory(|m| m.focused().is_none());
        if sc(cs, Key::F) || (!in_terminal && sc(Modifiers::CTRL, Key::K)) {
            cmds.push(Command::Search);
        }
        if !in_terminal && sc(Modifiers::CTRL, Key::Slash) {
            cmds.push(Command::Shortcuts);
        }
        if sc(Modifiers::CTRL, Key::Equals) || sc(Modifiers::CTRL, Key::Plus) {
            cmds.push(Command::FontBigger);
        }
        if sc(Modifiers::CTRL, Key::Minus) {
            cmds.push(Command::FontSmaller);
        }
        if sc(Modifiers::CTRL, Key::Num0) {
            cmds.push(Command::FontReset);
        }
        let digits = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9];
        for (i, k) in digits.into_iter().enumerate() {
            if sc(Modifiers::CTRL | Modifiers::ALT, k) {
                self.focus(i);
            }
        }
        for c in cmds {
            self.run(c, ctx);
        }
    }

    fn window_resize(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
            return;
        }
        let screen = ctx.content_rect();
        let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else { return };
        let m = 5.0;
        let (l, r, t, b) = (pos.x - screen.left() < m, screen.right() - pos.x < m, pos.y - screen.top() < m, screen.bottom() - pos.y < m);
        let dir = match (l, r, t, b) {
            (true, _, true, _) => ResizeDirection::NorthWest,
            (_, true, true, _) => ResizeDirection::NorthEast,
            (true, _, _, true) => ResizeDirection::SouthWest,
            (_, true, _, true) => ResizeDirection::SouthEast,
            (true, ..) => ResizeDirection::West,
            (_, true, ..) => ResizeDirection::East,
            (_, _, true, _) => ResizeDirection::North,
            (_, _, _, true) => ResizeDirection::South,
            _ => return,
        };
        ctx.set_cursor_icon(match dir {
            ResizeDirection::North | ResizeDirection::South => CursorIcon::ResizeVertical,
            ResizeDirection::East | ResizeDirection::West => CursorIcon::ResizeHorizontal,
            ResizeDirection::NorthWest | ResizeDirection::SouthEast => CursorIcon::ResizeNwSe,
            _ => CursorIcon::ResizeNeSw,
        });
        if ctx.input(|i| i.pointer.primary_pressed()) {
            ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
        }
    }

    // ------------------------------------------------------------------ chrome

    fn title_bar(&mut self, ui: &mut Ui) {
        let theme = self.theme.clone();
        let ctx = ui.ctx().clone();
        egui::Panel::top("titlebar").exact_size(40.0).frame(Frame::new().fill(theme.surface)).show_separator_line(false).show(ui, |ui| {
            let rect = ui.max_rect();
            let p = ui.painter().clone();
            p.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, theme.border));
            let cy = rect.center().y;
            let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));

            let drag = ui.interact(rect, Id::unique("titlebar-drag"), Sense::click_and_drag());
            if drag.drag_started_by(PointerButton::Primary) {
                ctx.send_viewport_cmd(ViewportCommand::StartDrag);
            }
            if drag.double_clicked() {
                ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
            }

            kit::mark(&p, pos2(rect.left() + 22.0, cy), 20.0);
            let name = p.text(pos2(rect.left() + 38.0, cy), Align2::LEFT_CENTER, "Graysense", semibold(13.5), theme.text);

            // Window controls.
            let bw = 46.0;
            let close_r = Rect::from_min_size(pos2(rect.right() - bw, rect.top()), vec2(bw, rect.height()));
            let max_r = close_r.translate(vec2(-bw, 0.0));
            let min_r = max_r.translate(vec2(-bw, 0.0));
            for (r, glyph, idn, danger) in [
                (min_r, icon::MINIMIZE, "wmin", false),
                (max_r, if maximized { icon::RESTORE } else { icon::MAXIMIZE }, "wmax", false),
                (close_r, icon::CLOSE, "wclose", true),
            ] {
                let resp = ui.interact(r, Id::unique(idn), Sense::click());
                resp.widget_info(|| {
                    let name = match idn {
                        "wmin" => "Minimize",
                        "wmax" if maximized => "Restore",
                        "wmax" => "Maximize",
                        _ => "Close window",
                    };
                    egui::WidgetInfo::labeled(egui::Role::Button, true, name)
                });
                if resp.hovered() {
                    p.rect_filled(r, 0.0, if danger { Color32::from_rgb(0xC4, 0x2B, 0x1C) } else { theme.surface2 });
                }
                p.text(r.center(), Align2::CENTER_CENTER, glyph, icon_font(10.0), if danger && resp.hovered() { Color32::WHITE } else { theme.text_dim });
                if resp.clicked() {
                    match idn {
                        "wmin" => ctx.send_viewport_cmd(ViewportCommand::Minimized(true)),
                        "wmax" => ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized)),
                        _ => ctx.send_viewport_cmd(ViewportCommand::Close),
                    }
                }
            }

            // Command search.
            let sw = 300.0f32.min((rect.width() - 520.0).max(160.0));
            let search = Rect::from_min_size(pos2(min_r.left() - 14.0 - sw, cy - 14.0), vec2(sw, 28.0));
            let sresp = ui.interact(search, Id::unique("search"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            sresp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Search or run a command"));
            p.rect(search, CornerRadius::same(7), if sresp.hovered() { theme.surface3 } else { theme.surface2 }, Stroke::new(1.0, theme.border), StrokeKind::Inside);
            p.text(search.left_center() + vec2(12.0, 0.0), Align2::LEFT_CENTER, icon::SEARCH, icon_font(11.0), theme.text_faint);
            p.text(search.left_center() + vec2(30.0, 0.0), Align2::LEFT_CENTER, "Search or run a command", ui_font(12.5), theme.text_faint);
            if sw > 240.0 {
                kit::kbd(&p, search.right_center() - vec2(8.0, 0.0), &["Ctrl", "Shift", "P"], &theme);
            }
            if sresp.clicked() {
                self.palette.open(PaletteMode::Commands);
            }

            let tabs = Rect::from_min_max(pos2(name.right() + 24.0, rect.top()), pos2(search.left() - 12.0, rect.bottom()));
            self.tabs(ui, tabs, &theme);
        });
    }

    fn tabs(&mut self, ui: &mut Ui, area: Rect, theme: &Theme) {
        let p = ui.painter().clone();
        let t = self.t0.elapsed().as_secs_f32();
        let mut x = area.left();
        let mut action: Option<(usize, bool)> = None;
        for (i, bay) in self.bays.iter().enumerate() {
            let title = bay.title();
            let galley = kit::elided(&p, &title, ui_font(12.5), theme.text, 200.0);
            let w = galley.size().x + 52.0;
            if x + w > area.right() - 34.0 {
                let more = self.bays.len() - i;
                p.text(pos2(x + 4.0, area.center().y), Align2::LEFT_CENTER, format!("+{more}"), ui_font(12.0), theme.text_dim);
                x += 30.0;
                break;
            }
            let r = Rect::from_min_size(pos2(x, area.top() + 6.0), vec2(w, area.height() - 12.0));
            let resp = ui.interact(r, Id::unique(("tab", bay.id)), Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            let active = i == self.focused;
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, active, &title));
            if active || resp.hovered() {
                p.rect_filled(r, CornerRadius::same(7), if active { theme.surface3 } else { theme.surface2 });
            }
            let dot_c = r.left_center() + vec2(14.0, 0.0);
            match &bay.kind {
                BayKind::Agent(v) => {
                    let c = agent_view::status_color(&v.status, theme);
                    if v.status.is_busy() {
                        kit::spinner(&p, dot_c, 4.5, t, c);
                    } else if v.incognito {
                        p.text(dot_c, Align2::CENTER_CENTER, icon::HIDE, icon_font(10.0), c);
                    } else {
                        kit::dot(&p, dot_c, 3.5, c);
                    }
                }
                BayKind::Shell(pane) => {
                    p.text(dot_c, Align2::CENTER_CENTER, icon::TERMINAL, icon_font(10.0), if pane.activity > 0.05 { theme.accent } else { theme.text_faint });
                }
            }
            p.galley(pos2(r.left() + 26.0, r.center().y - galley.size().y / 2.0), galley, if active { theme.text } else { theme.text_dim });
            let close = Rect::from_center_size(pos2(r.right() - 13.0, r.center().y), vec2(18.0, 18.0));
            if resp.hovered() || active {
                let cresp = ui.interact(close, Id::unique(("tabclose", bay.id)), Sense::click());
                cresp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Close tab"));
                if cresp.hovered() {
                    p.rect_filled(close, CornerRadius::same(4), theme.surface2);
                }
                p.text(close.center(), Align2::CENTER_CENTER, icon::CANCEL, icon_font(8.0), theme.text_dim);
                if cresp.clicked() {
                    action = Some((i, true));
                }
            }
            if resp.clicked() && action.is_none() {
                action = Some((i, false));
            }
            if resp.middle_clicked() {
                action = Some((i, true));
            }
            x += w + 4.0;
        }
        // New-bay menu.
        let plus = Rect::from_min_size(pos2(x + 2.0, area.center().y - 13.0), vec2(26.0, 26.0));
        let presp = ui.interact(plus, Id::unique("newbay"), Sense::click()).on_hover_text("New agent or terminal");
        kit::paint_icon_button(ui, plus, &presp, icon::ADD, theme, false);
        let mut cmd = None;
        egui::Popup::menu(&presp).width(230.0).show(|ui| {
            let items = [
                (icon::BOT, "New session", Command::NewAgent),
                (icon::OPEN_FOLDER, "New session in folder…", Command::NewAgentIn),
                (icon::HIDE, "New incognito session", Command::NewIncognitoAgent),
                (icon::TERMINAL, "New terminal", Command::NewShell),
                (icon::HISTORY, "Resume a session…", Command::Resume),
            ];
            for (glyph, label, c) in items {
                let r = ui.button(format!("{glyph}   {label}"));
                let r = if matches!(c, Command::NewIncognitoAgent) { r.on_hover_text("Doesn't use or add to memory") } else { r };
                if r.clicked() {
                    cmd = Some(c);
                    ui.close();
                }
            }
        });
        if let Some(c) = cmd {
            let ctx = ui.ctx().clone();
            self.run(c, &ctx);
        }
        if let Some((i, close)) = action {
            if close {
                self.close_bay(i);
            } else {
                self.focus(i);
            }
        }
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        let theme = self.theme.clone();
        let ctx = ui.ctx().clone();
        let mut update_click = None;
        egui::Panel::bottom("statusbar").exact_size(26.0).frame(Frame::new().fill(theme.surface)).show_separator_line(false).show(ui, |ui| {
            let rect = ui.max_rect();
            let p = ui.painter().clone();
            p.hline(rect.x_range(), rect.top() + 0.5, Stroke::new(1.0, theme.border));
            let cy = rect.center().y;
            let (c, label) = match &self.client {
                ClientState::Ready => (theme.ok, "Copilot ready".to_owned()),
                ClientState::Connecting => (theme.accent, "Connecting to Copilot…".to_owned()),
                ClientState::Failed(_) => (theme.err, "Copilot unavailable".to_owned()),
                ClientState::Idle => (theme.text_faint, "Copilot idle".to_owned()),
            };
            kit::dot(&p, pos2(rect.left() + 14.0, cy), 3.5, c);
            let lr = p.text(pos2(rect.left() + 24.0, cy), Align2::LEFT_CENTER, label, ui_font(11.5), theme.text_dim);
            let mut x = lr.right() + 16.0;
            if let ClientState::Failed(err) = &self.client {
                let resp = ui
                    .interact(Rect::from_min_max(pos2(rect.left() + 6.0, rect.top()), pos2(lr.right() + 4.0, rect.bottom())), Id::unique("copilot-status"), Sense::click())
                    .on_hover_cursor(CursorIcon::PointingHand)
                    .on_hover_text(format!("{err}\n\nInstall the GitHub Copilot CLI and sign in with `copilot login`.\nClick to try again."));
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Retry connecting to Copilot"));
                if resp.clicked() {
                    self.client = ClientState::Connecting;
                    self.backend.send(Cmd::Connect);
                }
            }

            if let Some(b) = self.bays.get(self.focused) {
                let info = match &b.kind {
                    BayKind::Agent(v) => {
                        let mut s = format!("{}  ·  {}", v.model.clone().unwrap_or_else(|| "Default model".into()), sessions::display_path(&v.cwd, &self.settings.sessions_root()));
                        if v.tokens_in + v.tokens_out > 0 {
                            s.push_str(&format!("  ·  {} in / {} out", human(v.tokens_in), human(v.tokens_out)));
                        }
                        if v.tool_count > 0 {
                            s.push_str(&format!("  ·  {} tool call{}", v.tool_count, if v.tool_count == 1 { "" } else { "s" }));
                        }
                        s
                    }
                    BayKind::Shell(pane) => format!("{}×{}  ·  {}", pane.cols, pane.rows, sessions::display_path(&pane.cwd, &self.settings.sessions_root())),
                };
                x = p.text(pos2(x, cy), Align2::LEFT_CENTER, info, ui_font(11.5), theme.text_faint).right() + 16.0;
            }
            let _ = x;

            let s = self.sys.lock().clone();
            let mut right = rect.right() - 12.0;
            let clock = chrono::Local::now().format("%H:%M").to_string();
            right = p.text(pos2(right, cy), Align2::RIGHT_CENTER, clock, ui_font(11.5), theme.text_dim).left() - 14.0;
            let mem = if s.mem_total > 0 { s.mem_used as f32 / s.mem_total as f32 * 100.0 } else { 0.0 };
            right = p.text(pos2(right, cy), Align2::RIGHT_CENTER, format!("MEM {mem:.0}%"), ui_font(11.5), theme.text_faint).left() - 12.0;
            right = p.text(pos2(right, cy), Align2::RIGHT_CENTER, format!("CPU {:.0}%", s.cpu), ui_font(11.5), theme.text_faint).left() - 8.0;
            let spark = Rect::from_min_size(pos2(right - 46.0, cy - 6.0), vec2(44.0, 12.0));
            kit::sparkline(&p, spark, &s.cpu_hist, 100.0, mix(theme.text_faint, theme.accent, 0.5));
            right = spark.left() - 14.0;
            if self.settings.perf_hud {
                right = p
                    .text(pos2(right, cy), Align2::RIGHT_CENTER, format!("{:.1} ms/frame  ·  {} MB", self.frame_ms, s.app_mem / (1024 * 1024)), ui_font(11.5), theme.accent)
                    .left()
                    - 14.0;
            }
            let update = match self.updater.phase {
                update::Phase::Ready if self.updater.ready() => Some(("Restart to update".to_owned(), true)),
                update::Phase::Downloading(f) => Some((format!("Downloading update  {:.0}%", f * 100.0), false)),
                _ => None,
            };
            if let Some((text, ready)) = update {
                let color = if ready { theme.accent } else { theme.text_faint };
                let g = p.layout_no_wrap(text.clone(), ui_font(11.5), color);
                let r = Rect::from_min_max(pos2(right - g.size().x - 32.0, cy - 10.0), pos2(right, cy + 10.0));
                let tip = if ready { "Install the update now; your sessions reopen afterwards" } else { "Open Settings › About" };
                let resp = ui.interact(r, Id::unique("update-status"), Sense::click()).on_hover_cursor(CursorIcon::PointingHand).on_hover_text(tip);
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &text));
                if ready || resp.hovered() {
                    let (fill, stroke) = if ready { (theme.accent_soft(), theme::with_alpha(theme.accent, 0.45)) } else { (theme.surface2, theme.border) };
                    p.rect(r, CornerRadius::same(10), fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
                }
                p.text(pos2(r.left() + 15.0, cy), Align2::CENTER_CENTER, if ready { icon::UPDATE } else { icon::DOWNLOAD }, icon_font(10.5), color);
                p.galley(pos2(r.left() + 26.0, cy - g.size().y / 2.0), g, color);
                right = r.left() - 12.0;
                if resp.clicked() {
                    update_click = Some(ready);
                }
            }
            let running = self.bays.iter().filter(|b| matches!(&b.kind, BayKind::Agent(v) if v.status.is_busy())).count();
            let waiting = self.bays.iter().filter(|b| matches!(&b.kind, BayKind::Agent(v) if v.status.needs_attention())).count();
            if waiting > 0 {
                right = p.text(pos2(right, cy), Align2::RIGHT_CENTER, format!("{waiting} waiting for you"), ui_font(11.5), theme.warn).left() - 14.0;
            }
            if running > 0 {
                p.text(pos2(right, cy), Align2::RIGHT_CENTER, format!("{running} working"), ui_font(11.5), theme.accent);
            }
        });
        match update_click {
            Some(true) => self.restart_to_update(&ctx, false),
            Some(false) => self.reveal_updates(),
            None => {}
        }
    }

    fn rail(&mut self, ui: &mut Ui) {
        let theme = self.theme.clone();
        egui::Panel::left("rail").exact_size(48.0).resizable(false).frame(Frame::new().fill(theme.surface)).show_separator_line(false).show(ui, |ui| {
            let rect = ui.max_rect();
            let p = ui.painter().clone();
            p.vline(rect.right() - 0.5, rect.y_range(), Stroke::new(1.0, theme.border));
            let pending: usize = self.bays.iter().map(|b| if let BayKind::Agent(v) = &b.kind { v.pending_prompts() } else { 0 }).sum();
            let inbox = self.memory.lock().inbox_count();
            let items = [
                (SidePanel::Missions, icon::LIGHTBULB, "Missions  (Ctrl+Shift+I)", self.missions.open_count()),
                (SidePanel::Sessions, icon::HISTORY, "Sessions  (Ctrl+Shift+H)", pending),
                (SidePanel::Memory, icon::MEMORY, "Memory", inbox),
                (SidePanel::Mcp, icon::CONNECT, "MCP servers", 0),
                (SidePanel::Plugins, icon::PUZZLE, "Plugins", 0),
            ];
            let mut clicked = None;
            for (i, (panel, glyph, tip, badge)) in items.into_iter().enumerate() {
                let r = Rect::from_center_size(pos2(rect.center().x, rect.top() + 28.0 + i as f32 * 44.0), vec2(38.0, 38.0));
                if self.rail_button(ui, &p, r, glyph, tip, self.settings.side_panel == Some(panel), badge, &theme, panel == SidePanel::Sessions && pending > 0).is_some() {
                    clicked = Some(panel);
                }
            }
            let r = Rect::from_center_size(pos2(rect.center().x, rect.bottom() - 28.0), vec2(38.0, 38.0));
            let tip = if self.updater.ready() { "Settings  (an update is ready)" } else { "Settings" };
            if self.rail_button(ui, &p, r, icon::SETTINGS, tip, self.settings.side_panel == Some(SidePanel::Settings), usize::from(self.updater.ready()), &theme, false).is_some() {
                clicked = Some(SidePanel::Settings);
            }
            if let Some(panel) = clicked {
                let next = if self.settings.side_panel == Some(panel) { None } else { Some(panel) };
                self.set_side(next);
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn rail_button(&self, ui: &mut Ui, p: &egui::Painter, r: Rect, glyph: &str, tip: &str, active: bool, badge: usize, theme: &Theme, warn: bool) -> Option<()> {
        let resp = ui.interact(r, Id::unique(("rail", tip)), Sense::click()).on_hover_text(tip).on_hover_cursor(CursorIcon::PointingHand);
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, active, if badge > 0 { format!("{tip} ({badge})") } else { tip.to_owned() }));
        if resp.hovered() || active {
            p.rect_filled(r, CornerRadius::same(8), if active { theme.surface3 } else { theme.surface2 });
        }
        if active {
            p.rect_filled(Rect::from_min_size(pos2(r.left() - 5.0, r.top() + 9.0), vec2(3.0, r.height() - 18.0)), CornerRadius::same(2), theme.accent);
        }
        p.text(r.center(), Align2::CENTER_CENTER, glyph, icon_font(17.0), if active { theme.text } else { theme.text_dim });
        if badge > 0 {
            let c = r.right_top() + vec2(-8.0, 8.0);
            let txt = if badge > 99 { "99+".to_owned() } else { badge.to_string() };
            let w = (txt.len() as f32 * 6.0 + 6.0).max(15.0);
            let br = Rect::from_center_size(c, vec2(w, 15.0));
            p.rect(br, CornerRadius::same(8), if warn { theme.warn } else { theme.accent }, Stroke::new(2.0, theme.surface), StrokeKind::Outside);
            p.text(br.center() + vec2(0.0, 0.5), Align2::CENTER_CENTER, txt, semibold(9.5), if warn { Color32::BLACK } else { theme.on_accent });
        }
        resp.clicked().then_some(())
    }

    fn side_panel(&mut self, ui: &mut Ui) {
        let Some(panel) = self.settings.side_panel else { return };
        let theme = self.theme.clone();
        let mut side = egui::Panel::left("side")
            .resizable(true)
            .default_size(self.settings.side_width)
            .min_size(260.0)
            .max_size(620.0);
        if std::mem::take(&mut self.force_side_width) {
            side = side.exact_size(self.settings.side_width);
        }
        let resp = side
            .frame(Frame::new().fill(theme.surface).inner_margin(Margin::symmetric(14, 12)))
            .show(ui, |ui| match panel {
                SidePanel::Missions => self.missions_panel(ui, &theme),
                SidePanel::Sessions => self.sessions_panel(ui, &theme),
                SidePanel::Memory => {
                    let project = self.focused_project();
                    let t = self.t0.elapsed().as_secs_f64();
                    self.memory_panel.busy = !self.learn_jobs.is_empty();
                    let actions = {
                        let mut store = self.memory.lock();
                        self.memory_panel.show(ui, &theme, &mut store, &mut self.settings.memory, project.as_ref(), &self.models, t)
                    };
                    for a in actions {
                        self.memory_action(a);
                    }
                }
                SidePanel::Mcp => {
                    let has_agent = self.focused_agent_bay().is_some();
                    let actions = self.mcp.show(ui, &theme, has_agent);
                    for a in actions {
                        self.mcp_action(a);
                    }
                }
                SidePanel::Plugins => {
                    let actions = self.plugins.show(ui, &theme);
                    for a in actions {
                        self.plugin_action(a);
                    }
                }
                SidePanel::Settings => self.settings_panel(ui, &theme),
            });
        let w = resp.response.rect.width();
        if (w - self.settings.side_width).abs() > 1.0 {
            self.settings.side_width = w;
            self.settings_dirty = true;
        }
    }

    fn missions_panel(&mut self, ui: &mut Ui, theme: &Theme) {
        if self.capture_focus {
            ui.memory_mut(|m| m.request_focus(Id::unique("mission-capture")));
            self.capture_focus = false;
        }
        let statuses: Vec<(BayId, String, Color32)> = self
            .bays
            .iter()
            .filter_map(|b| match &b.kind {
                BayKind::Agent(v) => Some((b.id, v.status.label(), agent_view::status_color(&v.status, theme))),
                _ => None,
            })
            .collect();
        let lookup = move |bay: BayId| statuses.iter().find(|(id, _, _)| *id == bay).map(|(_, l, c)| (l.clone(), *c));
        let actions = self.missions.show(ui, theme, &lookup);
        for a in actions {
            match a {
                BoardAction::Launch(id) => self.launch_mission(id),
                BoardAction::SendToFocused(id) => {
                    if self.bays.is_empty() {
                        self.launch_mission(id);
                    } else {
                        self.send_mission_to(self.focused, id);
                    }
                }
                BoardAction::FocusBay(bay) => {
                    if let Some(i) = self.bay_index(bay) {
                        self.focus(i);
                    }
                }
                BoardAction::PickDir(id) => self.pick_dir(Pick::Mission(id)),
            }
        }
    }

    fn sessions_panel(&mut self, ui: &mut Ui, theme: &Theme) {
        use crate::control::sessions::{OpenRow, PastRow, SessionsAction};
        let t = self.t0.elapsed().as_secs_f32();
        let open: Vec<OpenRow> = self
            .bays
            .iter()
            .enumerate()
            .filter_map(|(index, b)| match &b.kind {
                BayKind::Agent(v) => Some(OpenRow { index, title: v.display_title(), status: v.status.label(), color: agent_view::status_color(&v.status, theme), busy: v.status.is_busy() }),
                _ => None,
            })
            .collect();
        let open_ids: std::collections::HashSet<&str> = self.bays.iter().filter_map(|b| if let BayKind::Agent(v) = &b.kind { v.session_id.as_deref() } else { None }).collect();
        let now = chrono::Utc::now();
        let past: Vec<PastRow> = self
            .sessions
            .iter()
            .filter(|s| !open_ids.contains(s.id.as_str()))
            .map(|s| {
                let modified = chrono::DateTime::parse_from_rfc3339(&s.modified).ok().map(|d| d.with_timezone(&chrono::Local));
                PastRow {
                    title: self.session_title(s),
                    when: modified.map(|m| session_when(m, &s.modified, now)).unwrap_or_default(),
                    modified,
                    pinned: self.session_index.pinned(&s.id),
                    cli: s.client.as_deref().is_some_and(|c| !c.starts_with("graysense")),
                    id: s.id.clone(),
                }
            })
            .collect();

        // The list fills the panel; a collapsible activity feed sits at the bottom.
        let full = ui.available_rect_before_wrap();
        let activity_h = if self.show_activity { (full.height() * 0.35).clamp(120.0, 260.0) } else { 30.0 };
        let list = Rect::from_min_max(full.min, pos2(full.right(), full.bottom() - activity_h));
        let actions = ui.scope_builder(UiBuilder::new().max_rect(list), |ui| self.sessions_panel_ui.show(ui, theme, &open, &past, t)).inner;
        let activity = Rect::from_min_max(pos2(full.left(), full.bottom() - activity_h), full.max);
        ui.scope_builder(UiBuilder::new().max_rect(activity), |ui| self.activity_ui(ui, theme));
        ui.allocate_rect(full, Sense::hover());

        for a in actions {
            match a {
                SessionsAction::New => self.run(Command::NewAgent, &ui.ctx().clone()),
                SessionsAction::Focus(i) => self.focus(i),
                SessionsAction::Resume(id) => self.resume_session(id),
                SessionsAction::Rename(id, name) => match self.bays.iter().position(|b| matches!(&b.kind, BayKind::Agent(v) if v.session_id.as_deref() == Some(id.as_str()))) {
                    Some(i) => self.rename_bay(i, name),
                    None => self.session_index.entry(&id).name = (!name.is_empty()).then_some(name),
                },
                SessionsAction::Pin(id, pinned) => self.session_index.entry(&id).pinned = pinned,
                SessionsAction::Reveal(id) => {
                    let folder = self.session_index.get(&id).and_then(|m| m.folder.clone()).map(PathBuf::from).or_else(|| session_cwd(&id));
                    match folder.filter(|f| f.is_dir()) {
                        Some(f) => {
                            let _ = std::process::Command::new("explorer").arg(f).spawn();
                        }
                        None => self.toast(Level::Warn, "That session's folder no longer exists"),
                    }
                }
                SessionsAction::Delete(id) => {
                    if let Some(i) = self.bays.iter().position(|b| matches!(&b.kind, BayKind::Agent(v) if v.session_id.as_deref() == Some(id.as_str()))) {
                        self.close_bay(i);
                    }
                    self.sessions.retain(|s| s.id != id);
                    self.session_index.remove(&id);
                    self.backend.send(Cmd::DeleteSession { id });
                    self.toast(Level::Ok, "Session deleted. Its files are still in its folder.");
                }
                SessionsAction::Search(q) => self.open_search(&q),
            }
        }
    }

    /// The best name for a past session: the user's, then Graysense's short title, then Copilot's.
    fn session_title(&self, s: &SessionInfo) -> String {
        let meta = self.session_index.get(&s.id);
        if let Some(name) = meta.and_then(|m| m.name.as_deref()) {
            return name.to_owned();
        }
        meta.and_then(|m| m.auto_name.as_deref())
            .or(s.summary.as_deref())
            .filter(|x| !x.trim().is_empty())
            .map(sessions::short_title)
            .unwrap_or_else(|| format!("Session {}", &s.id[..8.min(s.id.len())]))
    }

    fn activity_ui(&mut self, ui: &mut Ui, theme: &Theme) {
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
        let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, self.show_activity, "Activity"));
        let p = ui.painter();
        p.hline(r.x_range(), r.top(), Stroke::new(1.0, theme.border));
        p.text(pos2(r.left() + 8.0, r.center().y), Align2::LEFT_CENTER, "ACTIVITY", semibold(10.5), if resp.hovered() { theme.text_dim } else { theme.text_faint });
        p.text(pos2(r.right() - 10.0, r.center().y), Align2::CENTER_CENTER, if self.show_activity { icon::CHEVRON_DOWN } else { icon::CHEVRON_UP }, icon_font(9.0), theme.text_faint);
        if resp.clicked() {
            self.show_activity = !self.show_activity;
        }
        if !self.show_activity {
            return;
        }
        egui::ScrollArea::vertical().id_salt("activity").auto_shrink([false; 2]).show(ui, |ui| {
            if self.activity.is_empty() {
                ui.label(RichText::new("Agent activity will appear here.").color(theme.text_faint));
            }
            for line in &self.activity {
                let idx = self.bay_index(line.bay).map(|i| (i + 1).to_string()).unwrap_or_else(|| "–".into());
                let color = match line.level {
                    Level::Info => theme.text_dim,
                    Level::Ok => theme.ok,
                    Level::Warn => theme.warn,
                    Level::Error => theme.err,
                };
                ui.horizontal_top(|ui| {
                    ui.label(RichText::new(idx).font(semibold(11.0)).color(theme.text_faint));
                    ui.label(RichText::new(line.icon).font(ui_font(11.5)).color(color));
                    ui.add(egui::Label::new(RichText::new(&line.text).font(ui_font(12.0)).color(theme.text_dim)).truncate());
                });
            }
        });
    }

    fn settings_panel(&mut self, ui: &mut Ui, theme: &Theme) {
        ui.label(RichText::new("Settings").font(semibold(15.0)));
        ui.add(egui::Label::new(RichText::new("Agents can change any of these for you: try “switch to the light theme”.").font(ui_font(11.5)).color(theme.text_faint)).wrap());
        ui.add_space(6.0);
        egui::ScrollArea::vertical().id_salt("settings-scroll").auto_shrink([false; 2]).show(ui, |ui| self.settings_sections(ui, theme));
    }

    fn settings_sections(&mut self, ui: &mut Ui, theme: &Theme) {
        let ctx = ui.ctx().clone();
        kit::section(ui, "Appearance", theme);
        let mut pick = None;
        ui.horizontal_wrapped(|ui| {
            for t in theme::ThemeId::ALL {
                let th = Theme::get(t);
                let selected = t == self.settings.theme;
                let (cell, resp) = ui.allocate_exact_size(vec2(68.0, 66.0), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, selected, t.name()));
                let r = Rect::from_min_size(cell.min + vec2(2.0, 0.0), vec2(64.0, 46.0));
                let p = ui.painter();
                p.rect(r, CornerRadius::same(8), th.bg, Stroke::new(if selected { 2.0 } else { 1.0 }, if selected { th.accent } else if resp.hovered() { theme.border_strong } else { theme.border }), StrokeKind::Inside);
                p.rect_filled(Rect::from_min_size(r.min + vec2(8.0, 8.0), vec2(48.0, 14.0)), CornerRadius::same(3), th.surface2);
                p.rect_filled(Rect::from_min_size(r.min + vec2(8.0, 26.0), vec2(22.0, 10.0)), CornerRadius::same(3), th.accent);
                let short = t.name().split(' ').next().unwrap_or_default();
                p.text(pos2(r.center().x, r.bottom() + 10.0), Align2::CENTER_CENTER, short, ui_font(11.5), if selected { theme.text } else { theme.text_dim });
                let resp = resp.on_hover_text(t.name()).on_hover_cursor(CursorIcon::PointingHand);
                if resp.clicked() {
                    pick = Some(t);
                }
            }
        });
        if let Some(t) = pick {
            self.set_theme(&ctx, t);
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Terminal font size");
            let mut s = self.settings.term_font_size;
            if ui.add(egui::DragValue::new(&mut s).range(9.0..=28.0).speed(0.2)).changed() {
                self.set_font(s);
            }
        });
        let mut smooth = self.settings.smooth_cursor;
        if kit::toggle_row(ui, &mut smooth, theme, "Smooth cursor").changed() {
            self.settings.smooth_cursor = smooth;
            self.settings_dirty = true;
        }
        let mut reasoning = self.settings.show_reasoning;
        if kit::toggle_row(ui, &mut reasoning, theme, "Show agent reasoning").changed() {
            self.settings.show_reasoning = reasoning;
            self.settings_dirty = true;
        }
        let mut perf = self.settings.perf_hud;
        if kit::toggle_row(ui, &mut perf, theme, "Show frame time & memory").changed() {
            self.settings.perf_hud = perf;
            self.settings_dirty = true;
        }
        ui.add_space(8.0);
        kit::section(ui, "Agents", theme);
        ui.label(RichText::new("Default model for new agents").color(theme.text_dim));
        let current = self.settings.default_model.clone().unwrap_or_else(|| "Copilot default".into());
        egui::ComboBox::from_id_salt("default-model").selected_text(current).width(240.0).show_ui(ui, |ui| {
            if ui.selectable_label(self.settings.default_model.is_none(), "Copilot default").clicked() {
                self.settings.default_model = None;
                self.settings_dirty = true;
            }
            for m in &self.models {
                if ui.selectable_label(self.settings.default_model.as_deref() == Some(m.id.as_str()), &m.name).clicked() {
                    self.settings.default_model = Some(m.id.clone());
                    self.settings_dirty = true;
                }
            }
        });
        ui.add_space(8.0);
        kit::section(ui, "Sessions", theme);
        let mut own = self.settings.session_folders;
        if kit::toggle_row(ui, &mut own, theme, "Give each new session its own folder").changed() {
            self.settings.session_folders = own;
            self.settings_dirty = true;
        }
        ui.label(RichText::new("Sessions folder").color(theme.text_dim));
        let root = self.settings.sessions_root();
        let g = kit::elided(ui.painter(), &settings::short_path(&root), theme::mono(12.0), theme.text, ui.available_width());
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
        ui.painter().galley(pos2(r.left(), r.center().y - g.size().y / 2.0), g, theme.text);
        resp.on_hover_text(root.display().to_string());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if kit::mini_button(ui, "Change…", theme).clicked() {
                self.pick_dir(Pick::SessionsRoot);
            }
            if kit::mini_button(ui, "Open", theme).clicked() {
                let _ = std::fs::create_dir_all(&root);
                let _ = std::process::Command::new("explorer").arg(&root).spawn();
            }
            if self.settings.sessions_root.is_some() && kit::mini_button(ui, "Use default", theme).clicked() {
                self.settings.sessions_root = None;
                self.settings_dirty = true;
            }
        });
        ui.add(egui::Label::new(RichText::new("New sessions get a folder here named after their first message. Search (Ctrl+K) covers what's in them.").font(ui_font(11.5)).color(theme.text_faint)).wrap());
        ui.add_space(8.0);
        kit::section(ui, "Terminal", theme);
        ui.label(RichText::new("Shell (empty = pwsh, then Windows PowerShell)").color(theme.text_dim));
        let mut shell = self.settings.shell.clone().unwrap_or_default();
        if ui.add(egui::TextEdit::singleline(&mut shell).hint_text("pwsh.exe").desired_width(f32::INFINITY)).changed() {
            self.settings.shell = (!shell.trim().is_empty()).then_some(shell);
            self.settings_dirty = true;
        }
        ui.add_space(8.0);
        kit::section(ui, "Shortcuts", theme);
        ui.horizontal(|ui| {
            if kit::ghost_button(ui, "Keyboard shortcuts", theme).clicked() {
                self.show_shortcuts = true;
            }
            ui.label(RichText::new("Ctrl+/").font(ui_font(11.5)).color(theme.text_faint));
        });
        ui.add_space(8.0);
        kit::section(ui, "About", theme);
        if std::mem::take(&mut self.reveal_about) {
            ui.scroll_to_cursor(Some(egui::Align::TOP));
        }
        let t = self.t0.elapsed().as_secs_f32();
        if let Some(a) = update::settings_ui(ui, &self.updater, self.settings.auto_update, theme, t) {
            self.update_action(&ctx, a);
        }
        ui.add_space(8.0);
        if ui.button(format!("{}  Open config folder", icon::FOLDER)).clicked() {
            self.run(Command::OpenConfig, &ctx);
        }
    }

    fn mcp_action(&mut self, a: McpAction) {
        let bay = self.focused_agent_bay();
        match a {
            McpAction::Refresh => self.refresh_side(),
            McpAction::Save { name, config, update } => self.backend.send(Cmd::Mcp(McpCmd::Add { name, config, update })),
            McpAction::Remove(name) => self.backend.send(Cmd::Mcp(McpCmd::Remove { name })),
            McpAction::SetEnabled(name, enabled) => self.backend.send(Cmd::Mcp(McpCmd::SetEnabled { name, enabled })),
            McpAction::Tools(server) => {
                if let Some(bay) = bay {
                    self.mcp.tools.remove(&server);
                    self.backend.send(Cmd::Mcp(McpCmd::Tools { bay, server }));
                }
            }
            McpAction::Restart(server) => {
                if let Some(bay) = bay {
                    self.backend.send(Cmd::Mcp(McpCmd::Restart { bay, server }));
                }
            }
            McpAction::Stop(server) => {
                if let Some(bay) = bay {
                    self.backend.send(Cmd::Mcp(McpCmd::Stop { bay, server }));
                }
            }
            McpAction::Start(server) => {
                if let Some(bay) = bay {
                    self.backend.send(Cmd::Mcp(McpCmd::Start { bay, server }));
                }
            }
        }
    }

    fn plugin_action(&mut self, a: PluginAction) {
        let cmd = match a {
            PluginAction::Refresh => PluginCmd::Refresh,
            PluginAction::Install(source) => PluginCmd::Install { source },
            PluginAction::Uninstall(name) => PluginCmd::Uninstall { name },
            PluginAction::SetEnabled(name, enabled) => PluginCmd::SetEnabled { name, enabled },
            PluginAction::Update(name) => PluginCmd::Update { name },
            PluginAction::UpdateAll => PluginCmd::UpdateAll,
            PluginAction::MarketAdd(source) => PluginCmd::MarketAdd { source },
            PluginAction::MarketRemove(name) => PluginCmd::MarketRemove { name },
            PluginAction::Browse(market) => {
                self.plugins.browse.remove(&market);
                PluginCmd::Browse { market }
            }
        };
        self.backend.send(Cmd::Plugin(cmd));
    }

    // ------------------------------------------------------------------ bays

    fn welcome(&mut self, ui: &mut Ui, area: Rect) {
        let theme = self.theme.clone();
        let ctx = ui.ctx().clone();
        // Like an editor's start page: what to do, then where you left off.
        let now = chrono::Utc::now();
        let recent: Vec<(String, String, String)> = self.sessions.iter().take(5).map(|s| (s.id.clone(), self.session_title(s), relative_time(&s.modified, now))).collect();
        let col_w = 340.0;
        let start_h = 4.0 * 40.0;
        let recent_h = if recent.is_empty() { 0.0 } else { 34.0 + recent.len() as f32 * 32.0 + 32.0 };
        let total = 150.0 + start_h + recent_h;
        let top = (area.center().y - total / 2.0).max(area.top() + 24.0);
        let cx = area.center().x;
        let p = ui.painter();
        kit::mark(p, pos2(cx, top + 22.0), 52.0);
        p.text(pos2(cx, top + 72.0), Align2::CENTER_CENTER, "Graysense", semibold(24.0), theme.text);
        p.text(pos2(cx, top + 100.0), Align2::CENTER_CENTER, "A fast, flat terminal for Copilot agents", ui_font(13.5), theme.text_dim);
        let mut cmd = None;
        let mut resume = None;
        let col = Rect::from_min_size(pos2(cx - col_w / 2.0, top + 140.0), vec2(col_w, start_h + recent_h));
        ui.scope_builder(UiBuilder::new().max_rect(col), |ui| {
            for (glyph, label, keys, command) in [
                (icon::BOT, "New session", &["Ctrl", "Shift", "N"][..], Command::NewAgent),
                (icon::TERMINAL, "New terminal", &["Ctrl", "Shift", "T"][..], Command::NewShell),
                (icon::SEARCH, "Search sessions and files", &["Ctrl", "K"][..], Command::Search),
                (icon::LIGHTBULB, "Capture an idea", &["Ctrl", "Shift", "I"][..], Command::CaptureIdea),
            ] {
                let (r, resp) = ui.allocate_exact_size(vec2(col_w, 36.0), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
                let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
                let p = ui.painter();
                if resp.hovered() {
                    p.rect_filled(r, CornerRadius::same(8), theme.surface2);
                }
                p.text(r.left_center() + vec2(16.0, 0.0), Align2::CENTER_CENTER, glyph, icon_font(14.0), theme.accent);
                p.text(r.left_center() + vec2(36.0, 0.0), Align2::LEFT_CENTER, label, ui_font(13.5), theme.text);
                if !keys.is_empty() {
                    kit::kbd(p, r.right_center() - vec2(8.0, 0.0), keys, &theme);
                }
                if resp.clicked() {
                    cmd = Some(command);
                }
                ui.add_space(4.0);
            }
            if recent.is_empty() {
                return;
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(RichText::new("RECENT").font(semibold(10.5)).color(theme.text_faint));
            });
            ui.add_space(2.0);
            for (id, title, when) in &recent {
                let (r, resp) = ui.allocate_exact_size(vec2(col_w, 30.0), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Resume {title}")));
                let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
                let p = ui.painter();
                if resp.hovered() {
                    p.rect_filled(r, CornerRadius::same(8), theme.surface2);
                }
                p.text(r.left_center() + vec2(16.0, 0.0), Align2::CENTER_CENTER, icon::HISTORY, icon_font(12.0), theme.text_faint);
                let wr = p.text(r.right_center() - vec2(10.0, 0.0), Align2::RIGHT_CENTER, when, ui_font(11.5), theme.text_faint);
                let g = kit::elided(p, title, ui_font(13.0), theme.text_dim, wr.left() - r.left() - 48.0);
                p.galley(pos2(r.left() + 36.0, r.center().y - g.size().y / 2.0), g, if resp.hovered() { theme.text } else { theme.text_dim });
                if resp.clicked() {
                    resume = Some(id.clone());
                }
            }
            ui.add_space(2.0);
            let (r, resp) = ui.allocate_exact_size(vec2(col_w, 26.0), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "All sessions"));
            let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
            ui.painter().text(r.left_center() + vec2(36.0, 0.0), Align2::LEFT_CENTER, "All sessions…", ui_font(12.5), if resp.hovered() { theme.text } else { theme.accent });
            if resp.clicked() {
                cmd = Some(Command::Side(SidePanel::Sessions));
            }
        });
        if let Some(id) = resume {
            self.resume_session(id);
        }
        if let Some(c) = cmd {
            self.run(c, &ctx);
        }
    }

    fn bays_ui(&mut self, ui: &mut Ui, t: f32, dt: f32) -> Vec<(u64, Rect)> {
        let ctx = ui.ctx().clone();
        let sessions_root = self.settings.sessions_root();
        let area = ui.max_rect().shrink(8.0);
        let files_hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
        let file_pos = if files_hovering || !dropped.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(50));
            self.native.cursor_pos(ctx.pixels_per_point()).or_else(|| ctx.input(|i| i.pointer.latest_pos()))
        } else {
            None
        };
        if self.bays.is_empty() {
            self.welcome(ui, area);
            if files_hovering {
                drop_overlay(ui, area, &self.theme, "Drop to start an agent here");
            }
            if !dropped.is_empty() {
                self.start_agent_with_files(dropped);
            }
            return Vec::new();
        }
        let theme = self.theme.clone();
        let models = self.models.clone();
        let n = self.bays.len();
        self.focused = self.focused.min(n - 1);
        let rects: Vec<Option<Rect>> = if self.maximized {
            (0..n).map(|i| (i == self.focused).then_some(area)).collect()
        } else {
            layout::compute(area, n, self.focused, self.settings.layout, 8.0).into_iter().map(Some).collect()
        };
        let metrics = self.term_metrics(&ctx);
        let dragging_mission = egui::DragAndDrop::has_payload_of_type::<MissionDrag>(&ctx);
        let pointer = ctx.input(|i| i.pointer.interact_pos());
        let pressed = ctx.input(|i| i.pointer.any_pressed());
        let released = ctx.input(|i| i.pointer.any_released());
        let focus_request = std::mem::take(&mut self.focus_bay);
        let mut placements = Vec::new();
        let mut actions: Vec<BayAction> = Vec::new();
        let file_target = (files_hovering || !dropped.is_empty())
            .then(|| file_pos.and_then(|p| rects.iter().position(|r| r.is_some_and(|r| r.contains(p)))).unwrap_or(self.focused));

        for i in 0..n {
            let Some(rect) = rects[i] else { continue };
            let focused = i == self.focused;
            if pressed && pointer.is_some_and(|p| rect.contains(p)) && !focused {
                actions.push(BayAction::Focus(i));
            }
            let border = if focused && n > 1 { mix(theme.border_strong, theme.accent, 0.6) } else { theme.border };
            kit::panel(ui.painter(), rect, &theme, 8, border);
            let bay = &mut self.bays[i];
            let bay_id = bay.id;
            let header = Rect::from_min_size(rect.min, vec2(rect.width(), HEADER_H));
            let body = Rect::from_min_max(pos2(rect.left() + 1.0, header.bottom()), pos2(rect.right() - 1.0, rect.bottom() - 1.0));
            ui.scope_builder(UiBuilder::new().max_rect(rect).id_salt(("bay", bay_id)), |ui| {
                ui.set_clip_rect(rect.intersect(ui.clip_rect()));
                match &mut bay.kind {
                    BayKind::Agent(view) => {
                        if focus_request && focused {
                            view.focus_composer = true;
                        }
                        let cx = AgentCtx { theme: &theme, t, models: &models, show_reasoning: self.settings.show_reasoning, focused, maximized: self.maximized, sessions_root: &sessions_root };
                        let mut out = Vec::new();
                        agent_view::header(ui, view, header, &cx, &mut out);
                        if let Some(p) = agent_view::body(ui, view, body, &cx, &mut out) {
                            placements.push(p);
                        }
                        actions.extend(out.into_iter().map(|a| BayAction::Agent(i, a)));
                    }
                    BayKind::Shell(pane) => {
                        let term_id = Id::unique(("term", bay_id));
                        if focus_request && focused {
                            ui.memory_mut(|m| m.request_focus(term_id));
                        }
                        shell_header(ui, pane, header, &theme, focused, self.maximized, i, &sessions_root, &mut actions);
                        let tacts = term_view::show(ui, pane, body, term_id, &metrics, &theme, self.settings.smooth_cursor, dt);
                        actions.extend(tacts.into_iter().map(|a| BayAction::Term(i, a)));
                    }
                }
            });
            if file_target == Some(i) {
                if files_hovering {
                    let label = if matches!(self.bays[i].kind, BayKind::Agent(_)) { "Drop to attach to this agent" } else { "Drop to paste the path" };
                    drop_overlay(ui, rect, &theme, label);
                }
                if !dropped.is_empty() {
                    actions.push(BayAction::DropFiles(i, dropped.clone()));
                }
            }
            if dragging_mission && pointer.is_some_and(|p| rect.contains(p)) {
                ui.painter().rect_stroke(rect.shrink(1.0), CornerRadius::same(8), Stroke::new(2.0, theme.accent), StrokeKind::Inside);
                ui.painter().rect_filled(rect.shrink(1.0), CornerRadius::same(8), theme::with_alpha(theme.accent, 0.06));
                ui.painter().text(rect.center(), Align2::CENTER_CENTER, "Drop to delegate", semibold(15.0), theme.accent);
                if released
                    && let Some(payload) = egui::DragAndDrop::take_payload::<MissionDrag>(&ctx) {
                        actions.push(BayAction::DropMission(i, payload.0));
                    }
            }
        }

        for a in actions {
            self.bay_action(&ctx, a);
        }
        placements
    }

    fn bay_action(&mut self, ctx: &egui::Context, a: BayAction) {
        match a {
            BayAction::Focus(i) => {
                self.focused = i.min(self.bays.len().saturating_sub(1));
            }
            BayAction::Close(i) => self.close_bay(i),
            BayAction::Maximize(i) => {
                self.focused = i;
                self.maximized = !self.maximized && self.bays.len() > 1;
            }
            BayAction::DropMission(i, id) => self.send_mission_to(i, id),
            BayAction::DropFiles(i, files) => {
                let Some(bay) = self.bays.get_mut(i) else { return };
                match &mut bay.kind {
                    BayKind::Agent(v) => {
                        for f in files {
                            if !v.attachments.contains(&f) {
                                v.attachments.push(f);
                            }
                        }
                        v.focus_composer = true;
                    }
                    BayKind::Shell(p) => {
                        let text = files.iter().map(|f| shell_quote(f)).collect::<Vec<_>>().join(" ");
                        p.paste(&text);
                    }
                }
                self.focus(i);
            }
            BayAction::Term(i, t) => match t {
                TermAction::Focus => self.focused = i,
                TermAction::Copy(text) => ctx.copy_text(text),
                TermAction::OpenUrl(u) => ctx.open_url(egui::OpenUrl::new_tab(u)),
                TermAction::Close => self.close_bay(i),
                TermAction::Restart => {
                    if let Some(Bay { kind: BayKind::Shell(p), .. }) = self.bays.get_mut(i) {
                        let spec = SpawnSpec { program: p.spec.program.clone(), args: p.spec.args.clone(), cwd: p.cwd.clone() };
                        match TermPane::spawn(ctx, spec, &self.theme, p.cols, p.rows) {
                            Ok(np) => **p = np,
                            Err(e) => self.toast(Level::Error, format!("Restart failed: {e}")),
                        }
                    }
                }
            },
            BayAction::Agent(i, a) => {
                let deferred = matches!(self.bays.get(i), Some(Bay { kind: BayKind::Agent(v), .. }) if v.deferred);
                match a {
                    AgentAction::Send(text) if deferred => return self.start_deferred(i, text),
                    AgentAction::Folder(f) => return self.folder_action(ctx, i, f),
                    AgentAction::Rename(name) => return self.rename_bay(i, name),
                    _ => {}
                }
                let Some(bay) = self.bays.get_mut(i) else { return };
                let bay_id = bay.id;
                let BayKind::Agent(view) = &mut bay.kind else { return };
                match a {
                    AgentAction::Send(text) => {
                        let files = std::mem::take(&mut view.attachments);
                        view.granted.lock().extend(files.iter().cloned());
                        let names = files.iter().map(|p| settings::dir_name(p)).collect();
                        // The first message names the session (until the user names it).
                        if view.title.is_none() {
                            view.title = Some(text.clone());
                            let name = sessions::short_title(&text);
                            if let Some(id) = &view.session_id {
                                self.session_index.entry(id).auto_name = Some(name.clone());
                            }
                            if view.name.is_none() {
                                self.backend.send(Cmd::NameSession { bay: bay_id, name, user: false });
                            }
                        }
                        view.echo_user_with(&text, names);
                        view.learn_due = None;
                        capture_explicit_memory(&self.memory, self.settings.memory.enabled, view, &text);
                        self.backend.send(Cmd::Send { bay: bay_id, text, files });
                    }
                    AgentAction::ForgetMemories(ids) => {
                        let mut store = self.memory.lock();
                        for id in ids {
                            store.archive(id, "undone");
                        }
                        drop(store);
                        self.toast(Level::Ok, "Forgotten");
                    }
                    AgentAction::OpenMemory(id) => self.open_memory(id),
                    AgentAction::Abort if deferred => {}
                    AgentAction::Abort => self.backend.send(Cmd::Abort { bay: bay_id }),
                    AgentAction::SetModel(m) => {
                        view.model = Some(m.clone());
                        if !deferred {
                            self.backend.send(Cmd::SetModel { bay: bay_id, model: m });
                        }
                    }
                    AgentAction::SetMode(m) => {
                        view.mode = m;
                        if !deferred {
                            self.backend.send(Cmd::SetMode { bay: bay_id, mode: m });
                        }
                    }
                    AgentAction::OpenUrl(u) => ctx.open_url(egui::OpenUrl::new_tab(u)),
                    AgentAction::Copy(text) => {
                        ctx.copy_text(text);
                        self.toast(Level::Ok, "Copied");
                    }
                    AgentAction::EditPrompt(text) => {
                        view.composer = text;
                        view.focus_composer = true;
                    }
                    AgentAction::File(f) => match f {
                        crate::agent::files::FileAction::Open(p) => open_path(&p),
                        crate::agent::files::FileAction::Reveal(p) => reveal_path(&p),
                        crate::agent::files::FileAction::Attach(p) => {
                            if !view.attachments.contains(&p) {
                                view.attachments.push(p);
                            }
                            view.focus_composer = true;
                        }
                    },
                    AgentAction::Focus => {
                        self.focused = i;
                        view.focus_composer = true;
                    }
                    AgentAction::Close => self.close_bay(i),
                    AgentAction::Maximize => {
                        self.focused = i;
                        self.maximized = !self.maximized && self.bays.len() > 1;
                    }
                    AgentAction::CloseCanvas(id) => view.canvases.retain(|c| c.instance_id != id),
                    AgentAction::Attach => {
                        let start = view.cwd.clone();
                        self.pick_attachments(bay_id, start);
                    }
                    AgentAction::Folder(_) | AgentAction::Rename(_) => {}
                }
            }
        }
    }

    fn folder_action(&mut self, ctx: &egui::Context, index: usize, action: agent_view::FolderAction) {
        use agent_view::FolderAction;
        let Some(Bay { kind: BayKind::Agent(v), .. }) = self.bays.get(index) else { return };
        let dir = v.cwd.clone();
        match action {
            FolderAction::Reveal => {
                let _ = std::process::Command::new("explorer").arg(&dir).spawn();
            }
            FolderAction::VsCode => {
                if let Err(e) = open_in_vscode(&dir) {
                    self.toast(Level::Error, e);
                }
            }
            FolderAction::Terminal => self.new_shell(ctx, dir),
            FolderAction::NewAgent => {
                self.new_agent(dir, None, None, None);
            }
            FolderAction::CopyPath => {
                ctx.copy_text(dir.display().to_string());
                self.toast(Level::Ok, "Path copied");
            }
        }
    }

    /// Name a session (empty = back to its automatic name).
    fn rename_bay(&mut self, index: usize, name: String) {
        let name = name.trim().chars().take(100).collect::<String>();
        let Some(Bay { id, kind: BayKind::Agent(v) }) = self.bays.get_mut(index) else { return };
        let bay = *id;
        v.name = (!name.is_empty()).then(|| name.clone());
        let auto = v.title.as_deref().map(sessions::short_title);
        let (session, deferred) = (v.session_id.clone(), v.deferred);
        if let Some(sid) = &session {
            self.session_index.entry(sid).name = v.name.clone();
        }
        if !deferred && session.is_some() {
            match (name.is_empty(), auto) {
                (false, _) => self.backend.send(Cmd::NameSession { bay, name, user: true }),
                (true, Some(auto)) => self.backend.send(Cmd::NameSession { bay, name: auto, user: true }),
                (true, None) => {}
            }
        }
    }

    fn app_request(&mut self, req: AppRequest, ctx: &egui::Context) {
        match req {
            AppRequest::CallTool { bay, view, req_id, server, tool, args } => self.backend.send(Cmd::App(AppCmd::CallTool { bay, view, req_id, server, tool, args })),
            AppRequest::ReadResource { bay, view, req_id, server, uri } => self.backend.send(Cmd::App(AppCmd::ReadResource { bay, view, req_id, server, uri })),
            AppRequest::Message { bay, text } => {
                if let Some(v) = self.agent_mut(bay) {
                    v.echo_user(&text);
                }
                self.backend.send(Cmd::Send { bay, text, files: Vec::new() });
            }
            AppRequest::OpenUrl(u) => ctx.open_url(egui::OpenUrl::new_tab(u)),
            AppRequest::Log(line) => tracing::info!("mcp app: {line}"),
        }
    }

    /// Keep the OS window title (taskbar, Alt+Tab) in step with the focused bay.
    fn sync_window_title(&mut self, ctx: &egui::Context) {
        let title = match self.bays.get(self.focused) {
            Some(b) => format!("{} — Graysense", b.title()),
            None => "Graysense".to_owned(),
        };
        if title != self.window_title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    fn toasts_ui(&mut self, ctx: &egui::Context) {
        self.toasts.retain(|t| t.born.elapsed() < Duration::from_secs(6));
        if self.toasts.is_empty() {
            return;
        }
        let theme = self.theme.clone();
        let screen = ctx.content_rect();
        let mut y = screen.bottom() - 40.0;
        let mut dismiss = None;
        for (i, t) in self.toasts.iter().enumerate().rev() {
            let (glyph, color) = match t.level {
                Level::Info => (icon::INFO, theme.accent),
                Level::Ok => (icon::CHECK, theme.ok),
                Level::Warn => (icon::WARNING, theme.warn),
                Level::Error => (icon::ERROR, theme.err),
            };
            let w = 360.0;
            let area = egui::Area::new(Id::unique(("toast", i, t.born.elapsed().as_nanos() as u64 / 1_000_000_000_000))).order(egui::Order::Foreground).fixed_pos(pos2(screen.right() - w - 16.0, y - 48.0));
            let r = area.show(ctx, |ui| {
                Frame::new().fill(theme.surface).stroke(Stroke::new(1.0, theme.border_strong)).corner_radius(CornerRadius::same(8)).shadow(ui.visuals().popup_shadow).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
                    ui.set_width(w - 24.0);
                    ui.horizontal_top(|ui| {
                        ui.label(RichText::new(glyph).font(icon_font(13.0)).color(color));
                        ui.add(egui::Label::new(RichText::new(&t.text).font(ui_font(12.5)).color(theme.text)).wrap());
                    });
                })
            });
            if r.response.interact(Sense::click()).clicked() {
                dismiss = Some(i);
            }
            y -= r.response.rect.height() + 8.0;
        }
        if let Some(i) = dismiss {
            self.toasts.remove(i);
        }
        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

fn human(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

/// Save a "remember that …" request from a prompt immediately, with an Undo chip.
/// 
/// This works even while background learning is paused: the user asked explicitly.
fn capture_explicit_memory(memory: &Mutex<MemoryStore>, enabled: bool, view: &mut AgentView, prompt: &str) {
    if view.incognito || !enabled {
        return;
    }
    let Some((text, kind, about_project)) = crate::memory::explicit_request(prompt) else { return };
    let scope = if about_project { view.project.scope() } else { Scope::Global };
    let source = Source { session: view.session_id.clone(), title: None, quote: Some(prompt.trim().to_owned()), at: crate::memory::now() };
    let id = memory.lock().insert(kind, scope, &text, Origin::User, MemoryState::Active, 0.95, Some(source));
    if let Some(id) = id {
        view.push(Item::Memory { ids: vec![id], text, learned: false, undone: false });
    }
}

/// Open a file for viewing with its default app (folders open in File Explorer). Files that would
/// *run* when opened (scripts, programs) open in an editor instead: an agent may have written them.
fn open_path(p: &std::path::Path) {
    const RUNS: &[&str] = &["exe", "bat", "cmd", "com", "msi", "ps1", "psm1", "vbs", "vbe", "js", "jse", "wsf", "wsh", "scr", "lnk", "reg", "hta", "cpl", "pif", "appref-ms"];
    let ext = p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    if !p.is_dir() && RUNS.contains(&ext.as_str()) {
        if open_in_vscode(p).is_err() {
            let _ = std::process::Command::new("notepad").arg(p).spawn();
        }
        return;
    }
    let _ = std::process::Command::new("explorer").arg(p).spawn();
}

/// Show a file selected in File Explorer.
fn reveal_path(p: &std::path::Path) {
    use std::os::windows::process::CommandExt;
    if p.is_dir() {
        let _ = std::process::Command::new("explorer").arg(p).spawn();
    } else {
        let _ = std::process::Command::new("explorer").raw_arg(format!("/select,\"{}\"", p.display())).spawn();
    }
}

/// Compact age for file lists: "now", "5m", "3h", "2d", "4mo".
pub(crate) fn short_age(secs: u64) -> String {
    match secs {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        86_400..=2_591_999 => format!("{}d", secs / 86_400),
        _ => format!("{}mo", secs / 2_592_000),
    }
}

/// Open a folder in VS Code (a per-user or machine-wide install, or the `code` launcher on PATH).
fn open_in_vscode(dir: &std::path::Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let installs = [dirs::data_local_dir().map(|d| d.join("Programs").join("Microsoft VS Code").join("Code.exe")), std::env::var_os("ProgramFiles").map(|p| PathBuf::from(p).join("Microsoft VS Code").join("Code.exe"))];
    let exe = installs.into_iter().flatten().find(|p| p.is_file()).or_else(|| settings::which("code.cmd"));
    let Some(exe) = exe else { return Err("VS Code isn't installed, or `code` isn't on PATH".into()) };
    // Batch launchers (code.cmd) are run by Rust through cmd.exe with safe quoting.
    std::process::Command::new(exe).arg(dir).creation_flags(0x0800_0000).spawn().map(|_| ()).map_err(|e| format!("Couldn't start VS Code: {e}"))
}

/// Working directory a Copilot session was started in (from its `workspace.yaml`).
fn session_cwd(id: &str) -> Option<PathBuf> {
    let file = dirs::home_dir()?.join(".copilot").join("session-state").join(id).join("workspace.yaml");
    let text = std::fs::read_to_string(file).ok()?;
    text.lines().find_map(|l| l.strip_prefix("cwd:")).map(|v| PathBuf::from(v.trim().trim_matches('"').trim_matches('\'')))
}

/// "When" for the sessions list, matching its date groups: "5m" today, "9:41 PM" yesterday,
/// "Mon" this week, "Oct 3" before that.
fn session_when(t: chrono::DateTime<chrono::Local>, iso: &str, now: chrono::DateTime<chrono::Utc>) -> String {
    match (now.with_timezone(&chrono::Local).date_naive() - t.date_naive()).num_days() {
        ..=0 => relative_time(iso, now),
        1 => t.format("%-I:%M %p").to_string(),
        2..=6 => t.format("%a").to_string(),
        _ => t.format("%b %-d").to_string(),
    }
}

/// "just now", "5m", "3h", "Yesterday", "Mon", "Nov 10".
pub(crate) fn relative_time(iso: &str, now: chrono::DateTime<chrono::Utc>) -> String {
    let Ok(t) = chrono::DateTime::parse_from_rfc3339(iso) else { return String::new() };
    let t = t.with_timezone(&chrono::Utc);
    let secs = (now - t).num_seconds().max(0);
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        86_400..=172_799 => "Yesterday".into(),
        172_800..=518_399 => t.with_timezone(&chrono::Local).format("%a").to_string(),
        _ => t.with_timezone(&chrono::Local).format("%b %-d").to_string(),
    }
}

/// Accent outline + label shown over a bay while files are dragged onto it.
fn drop_overlay(ui: &Ui, rect: Rect, theme: &Theme, label: &str) {
    let p = ui.painter();
    p.rect_filled(rect.shrink(1.0), CornerRadius::same(8), theme::with_alpha(theme.accent, 0.08));
    p.rect_stroke(rect.shrink(1.0), CornerRadius::same(8), Stroke::new(2.0, theme.accent), StrokeKind::Inside);
    let pill = Rect::from_center_size(rect.center(), vec2(p.layout_no_wrap(label.to_owned(), semibold(14.0), theme.accent).size().x + 56.0, 40.0));
    p.rect(pill, CornerRadius::same(20), theme.surface, Stroke::new(1.0, theme.accent), StrokeKind::Inside);
    p.text(pill.left_center() + vec2(24.0, 0.0), Align2::CENTER_CENTER, icon::ATTACH, icon_font(14.0), theme.accent);
    p.text(pill.left_center() + vec2(40.0, 0.0), Align2::LEFT_CENTER, label, semibold(14.0), theme.accent);
}

/// Quote a path for pasting into a shell prompt.
fn shell_quote(path: &std::path::Path) -> String {
    let s = path.display().to_string();
    if s.contains([' ', '&', '(', ')', '\'', ';', ',', '$', '`']) { format!("\"{s}\"") } else { s }
}

#[allow(clippy::too_many_arguments)]
fn shell_header(ui: &mut Ui, pane: &TermPane, rect: Rect, theme: &Theme, focused: bool, maximized: bool, index: usize, sessions_root: &std::path::Path, out: &mut Vec<BayAction>) {
    let p = ui.painter_at(rect);
    p.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, theme.border));
    let cy = rect.center().y;
    p.text(pos2(rect.left() + 17.0, cy), Align2::CENTER_CENTER, icon::TERMINAL, icon_font(12.0), if focused { theme.accent } else { theme.text_dim });
    let spark = Rect::from_min_size(pos2(rect.right() - 130.0, cy - 6.0), vec2(56.0, 12.0));
    let avail = (spark.left() - 12.0 - (rect.left() + 32.0)).max(0.0);
    let title = kit::elided(&p, &pane.title, semibold(13.0), theme.text, avail.min(avail * 0.7 + 40.0));
    let tw = title.size().x;
    p.galley(pos2(rect.left() + 32.0, cy - title.size().y / 2.0), title, theme.text);
    let status = if pane.exited.is_some() { "exited".to_owned() } else { sessions::display_path(&pane.cwd, sessions_root) };
    let sx = rect.left() + 32.0 + tw + 10.0;
    if spark.left() - 12.0 - sx > 50.0 {
        let g = kit::elided(&p, &status, ui_font(11.5), theme.text_faint, spark.left() - 12.0 - sx);
        p.galley(pos2(sx, cy - g.size().y / 2.0), g, theme.text_faint);
    }
    if spark.left() > rect.left() + 120.0 {
        kit::sparkline(&p, spark, &pane.history, 1.0, theme::with_alpha(theme.accent, 0.8));
    }
    let close = Rect::from_min_size(pos2(rect.right() - 32.0, cy - 13.0), vec2(26.0, 26.0));
    let max = close.translate(vec2(-28.0, 0.0));
    let cresp = ui.interact(close, ui.scope_id().with("close"), Sense::click()).on_hover_text("Close terminal");
    cresp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Close terminal"));
    kit::paint_icon_button(ui, close, &cresp, icon::CANCEL, theme, false);
    if cresp.clicked() {
        out.push(BayAction::Close(index));
    }
    let mresp = ui.interact(max, ui.scope_id().with("max"), Sense::click()).on_hover_text("Maximize (Ctrl+Shift+Enter)");
    mresp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, if maximized { "Restore bay" } else { "Maximize bay" }));
    kit::paint_icon_button(ui, max, &mresp, if maximized { icon::BACK_TO_WINDOW } else { icon::FULLSCREEN }, theme, maximized);
    if mresp.clicked() {
        out.push(BayAction::Maximize(index));
    }
}

impl eframe::App for GraysenseApp {
    fn ui(&mut self, ui: &mut Ui, frame: &mut eframe::Frame) {
        let start = Instant::now();
        let ctx = ui.ctx().clone();
        let dt = self.last_frame.elapsed().as_secs_f32().min(0.1);
        self.last_frame = Instant::now();
        let t = self.t0.elapsed().as_secs_f32();
        self.window_focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));

        self.drain_events(&ctx);
        self.poll_picker(&ctx);
        if let Some(v) = self.updater.tick(self.settings.auto_update) {
            self.toast(Level::Info, format!("Graysense {v} is ready. Restart to update, or it installs when you close Graysense."));
        }
        self.pump_terms(&ctx, dt);
        if !self.palette.open {
            self.handle_shortcuts(&ctx);
        }
        self.window_resize(&ctx);

        ui.painter().rect_filled(ui.max_rect(), 0.0, self.theme.bg);
        self.title_bar(ui);
        self.status_bar(ui);
        self.rail(ui);
        self.side_panel(ui);
        let placements = egui::CentralPanel::no_frame().show(ui, |ui| self.bays_ui(ui, t, dt)).inner;

        self.palette.memory_on = self.settings.memory.enabled;
        self.palette.update_ready = self.updater.ready();
        if let Some(cmd) = self.palette.show(&ctx, &self.theme, &self.sessions, &self.settings.recent_dirs) {
            self.run(cmd, &ctx);
        }
        if self.show_shortcuts {
            self.shortcuts_overlay(&ctx);
        }
        if self.confirm_restart {
            self.restart_confirm_ui(&ctx);
        }
        self.poll_search_index();
        self.search_overlay(&ctx);
        self.toasts_ui(&ctx);
        self.sync_window_title(&ctx);

        // Child webviews paint above egui: hide them while menus or the palette are open.
        let overlay = self.palette.open || self.show_shortcuts || self.confirm_restart || self.search_ui.open || egui::Popup::is_any_open(&ctx);
        let theme = self.theme.clone();
        self.apps.layout(frame, &ctx, &placements, overlay, &theme);
        for (view, rect) in &placements {
            if let Some(err) = self.apps.failure(*view) {
                let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, Id::unique(("app-failed", *view))));
                p.rect(*rect, CornerRadius::same(6), theme.surface2, Stroke::new(1.0, theme.border), StrokeKind::Inside);
                p.text(rect.center() - vec2(0.0, 14.0), Align2::CENTER_CENTER, "This app couldn't be displayed", semibold(13.5), theme.text);
                let g = kit::elided(&p, &err, ui_font(11.5), theme.text_dim, rect.width() - 32.0);
                p.galley(pos2(rect.center().x - g.size().x / 2.0, rect.center().y + 4.0), g, theme.text_dim);
            }
        }
        for req in self.apps.pump(&theme) {
            self.app_request(req, &ctx);
        }
        if !self.apps.is_empty() && ctx.input(|i| i.pointer.any_pressed()) {
            self.apps.release_focus();
        }

        if self.mcp_refresh_at.is_some_and(|t| Instant::now() >= t) {
            self.mcp_refresh_at = None;
            self.refresh_side();
        }
        self.memory_tick();

        if self.last_save.elapsed() > Duration::from_secs(3) {
            self.missions.save_if_dirty();
            self.memory.lock().save_if_dirty();
            self.session_index.save_if_dirty();
            self.save_workspace_if_changed();
            if self.settings_dirty {
                self.settings.save();
                self.settings_dirty = false;
            }
            self.last_save = Instant::now();
        }

        self.frame_ms = self.frame_ms * 0.9 + start.elapsed().as_secs_f32() * 1000.0 * 0.1;
        ctx.request_repaint_after(Duration::from_secs(2));
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.theme.bg.to_normalized_gamma_f32()
    }

    fn on_exit(&mut self) {
        self.save_workspace();
        self.missions.save_if_dirty();
        self.queue_unread_conversations();
        self.memory.lock().save_if_dirty();
        self.session_index.save_if_dirty();
        self.settings.save();
        self.backend.shutdown(&self.rt);
        // Last, so the installer (which waits for this process to exit) starts after cleanup.
        self.updater.on_exit();
    }
}
