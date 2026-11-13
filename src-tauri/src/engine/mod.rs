//! The core of Graysense, independent of the UI: agent and terminal bays, sessions, missions,
//! memory, MCP servers, plugins, search and updates. The UI drives it with actions
//! (`actions.rs`) and sees it through `UiUpdate`s (`dto.rs`), sent as the state changes.

pub mod actions;
pub mod dto;
mod learn;

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager};

use crate::agent::backend::{AgentSpec, Backend, ClientState, Cmd, EffortChange, Evt, MarketInfo, MarketPlugin, McpCmd, McpServerInfo, McpToolInfo, ModelInfo, PluginCmd, PluginInfo, SessionInfo, UiTx};
use crate::agent::{AgentView, AppTab, BayId, Effect, Item, Level, Mode, Status};
use crate::canvas::CanvasInstance;
use crate::memory::{MemoryStore, Origin, Project, Scope, Source, State as MemoryState};
use crate::missions::{Mission, MissionBoard};
use crate::sessions::{self, SessionIndex};
use crate::settings::{self, Settings, SidePanel};
use crate::sysmon::{self, SysStats};
use crate::terminal::Terminals;
use crate::update::Updater;
use dto::*;

pub use learn::LearnJob;

pub enum BayKind {
    Agent(Box<AgentView>),
    Shell(ShellBay),
}

pub struct ShellBay {
    pub cwd: PathBuf,
    /// The shell it runs (an id from `terminal_shells`), or the default.
    pub shell: Option<String>,
    /// The title the shell set (OSC 0/2), if any.
    pub title: Option<String>,
}

pub struct Bay {
    pub id: BayId,
    pub kind: BayKind,
}

impl Bay {
    pub fn title(&self) -> String {
        match &self.kind {
            BayKind::Agent(v) => v.display_title(),
            BayKind::Shell(s) => s.title.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| settings::dir_name(&s.cwd)),
        }
    }

    pub fn cwd(&self) -> &Path {
        match &self.kind {
            BayKind::Agent(v) => &v.cwd,
            BayKind::Shell(s) => &s.cwd,
        }
    }
}

/// An MCP App (an interactive UI an MCP tool returned), docked in an agent bay.
pub struct AppView {
    pub id: u64,
    pub bay: BayId,
    pub title: String,
    pub server: String,
    pub tool: String,
    pub tool_call_id: String,
    pub html: String,
    /// The resource's `_meta` (its CSP domains, preferred size…).
    pub meta: Value,
    pub args: Value,
    pub result: Value,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Workspace {
    bays: Vec<SavedBay>,
    focused: usize,
    /// The model the runtime starts sessions with when none is chosen (learned from one that did).
    runtime_model: Option<String>,
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
    Shell {
        cwd: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        shell: Option<String>,
    },
}

/// What Graysense was started (or, with one already running, asked) to open.
pub struct Startup {
    pub restore: bool,
    pub agent: Option<Option<String>>,
    pub shell: bool,
    pub cwd: Option<PathBuf>,
}

impl Startup {
    /// `graysense [DIR] [--agent [PROMPT]] [--shell] [--no-restore]`; relative folders are read
    /// from `cwd`.
    pub fn parse(args: impl IntoIterator<Item = String>, cwd: Option<&Path>) -> Self {
        let mut s = Startup { restore: true, agent: None, shell: false, cwd: None };
        let mut args = args.into_iter().peekable();
        while let Some(a) = args.next() {
            match a.as_str() {
                "--agent" | "-a" => s.agent = Some(args.next_if(|p| !p.starts_with("--"))),
                "--shell" | "-s" => s.shell = true,
                "--no-restore" => s.restore = false,
                other if other.starts_with("--") => {}
                other => {
                    let p = PathBuf::from(other);
                    let p = match (p.is_absolute(), cwd) {
                        (false, Some(base)) => base.join(p),
                        _ => p,
                    };
                    if p.is_dir() {
                        s.cwd = Some(p);
                    }
                }
            }
        }
        s
    }
}

struct McpState {
    loading: bool,
    servers: Option<Result<Vec<McpServerInfo>, String>>,
    live_bay: Option<BayId>,
    tools: Vec<(String, Result<Vec<McpToolInfo>, String>)>,
}

#[derive(Default)]
struct PluginState {
    installed: Option<Result<Vec<PluginInfo>, String>>,
    markets: Option<Result<Vec<MarketInfo>, String>>,
    browse: Vec<(String, Result<Vec<MarketPlugin>, String>)>,
}

struct SearchState {
    index: Arc<crate::search::Index>,
    rx: Option<Receiver<crate::search::Index>>,
    built: Option<Instant>,
}

/// What the last update told the UI, to send only what changed.
#[derive(Default)]
struct Sent {
    app: u64,
    settings: u64,
    bays: u64,
    missions: u64,
    memory: Option<u64>,
    update: Option<crate::update::UpdateStatus>,
    items: HashMap<BayId, Vec<u64>>,
    title: String,
}

/// Domains that changed without a cheap way to tell.
#[derive(Default)]
struct Dirty {
    sessions: bool,
    memory: bool,
    mcp: bool,
    plugins: bool,
}

/// What a flush hands to the window: the update, replies for MCP Apps, window chores.
#[derive(Default)]
pub struct Outbox {
    pub update: Option<UiUpdate>,
    pub app_replies: Vec<Value>,
    pub title: Option<String>,
    pub attention: bool,
    pub quit: bool,
}

impl Outbox {
    fn is_empty(&self) -> bool {
        self.update.is_none() && self.app_replies.is_empty() && self.title.is_none() && !self.attention && !self.quit
    }
}

/// Takes flushes to the window one at a time, in the order they were made. Window calls wait
/// for the main thread, so they can't be made under the core's lock; but callers delivering on
/// their own once they let go of it could swap two updates, and the UI drops an update older
/// than one it has applied (and with it whatever only that update carried).
#[derive(Clone)]
pub struct Courier(std::sync::mpsc::Sender<(Outbox, std::sync::mpsc::Sender<()>)>);

impl Courier {
    pub fn start(app: AppHandle) -> Self {
        let (tx, rx) = channel::<(Outbox, std::sync::mpsc::Sender<()>)>();
        let spawned = std::thread::Builder::new().name("graysense-deliver".into()).spawn(move || {
            for (out, done) in rx {
                deliver(&app, out);
                let _ = done.send(());
            }
        });
        if let Err(e) = spawned {
            tracing::error!("couldn't start the delivery thread: {e}");
        }
        Self(tx)
    }

    fn send(&self, out: Outbox) -> Delivery {
        if out.is_empty() {
            return Delivery(None);
        }
        let (done, delivered) = channel();
        let _ = self.0.send((out, done));
        Delivery(Some(delivered))
    }
}

/// A flush on its way to the window.
#[must_use = "wait for it (without the core's lock) or drop it"]
pub struct Delivery(Option<Receiver<()>>);

impl Delivery {
    /// Block until the window has it. Never call this while holding the core's lock.
    pub fn wait(self) {
        if let Some(rx) = self.0 {
            let _ = rx.recv_timeout(Duration::from_secs(5));
        }
    }
}

pub struct Core {
    rt: Arc<tokio::runtime::Runtime>,
    backend: Backend,
    pub settings: Settings,
    settings_dirty: bool,
    bays: Vec<Bay>,
    focused: Option<BayId>,
    maximized: bool,
    next_bay: BayId,
    missions: MissionBoard,
    /// The last mission deleted, and where it was (for Undo).
    removed_mission: Option<(Mission, usize)>,
    memory: Arc<Mutex<MemoryStore>>,
    memory_last_run: Option<(i64, Option<String>)>,
    learn_jobs: HashMap<BayId, LearnJob>,
    session_index: SessionIndex,
    sessions: Vec<SessionInfo>,
    sessions_loaded: bool,
    models: Vec<ModelInfo>,
    client: ClientState,
    mcp: McpState,
    plugins: PluginState,
    apps: Vec<AppView>,
    next_view: u64,
    updater: Updater,
    search: SearchState,
    activity: VecDeque<LogLineDto>,
    activity_new: Vec<LogLineDto>,
    toasts_new: Vec<ToastDto>,
    next_toast: u64,
    app_replies: Vec<Value>,
    attention: bool,
    quit: bool,
    /// The window has keyboard focus (set from window events without taking the core's lock).
    pub window_focused: Arc<AtomicBool>,
    dirty: Dirty,
    seq: u64,
    sent: Sent,
    last_ws: String,
    last_save: Instant,
    mcp_refresh_at: Option<Instant>,
    sys: Arc<Mutex<SysStats>>,
    last_stats: Instant,
    pub terminals: Terminals,
    courier: Courier,
    /// See [`Workspace::runtime_model`].
    runtime_model: Option<String>,
}

/// How long nothing may change before a flush stops waiting to batch more changes.
const FLUSH_GAP: Duration = Duration::from_millis(40);
const TICK: Duration = Duration::from_millis(100);

impl Core {
    pub fn new(rt: Arc<tokio::runtime::Runtime>, startup: Startup, terminals: Terminals, courier: Courier) -> (Self, Receiver<Evt>) {
        let settings = Settings::load();
        let (tx, rx) = channel();
        let backend = Backend::spawn(&rt, UiTx::new(tx));
        backend.send(Cmd::Connect);
        let updater = Updater::new(Arc::new(|| {}), settings.auto_update);
        let mut core = Core {
            rt,
            backend,
            settings,
            settings_dirty: false,
            bays: Vec::new(),
            focused: None,
            maximized: false,
            next_bay: 1,
            missions: MissionBoard::load(),
            removed_mission: None,
            memory: Arc::new(Mutex::new(MemoryStore::load())),
            memory_last_run: None,
            learn_jobs: HashMap::new(),
            session_index: SessionIndex::load(),
            sessions: Vec::new(),
            sessions_loaded: false,
            models: Vec::new(),
            client: ClientState::Idle,
            mcp: McpState { loading: false, servers: None, live_bay: None, tools: Vec::new() },
            plugins: PluginState::default(),
            apps: Vec::new(),
            next_view: 1,
            updater,
            search: SearchState { index: Arc::new(crate::search::Index::empty()), rx: None, built: None },
            activity: VecDeque::new(),
            activity_new: Vec::new(),
            toasts_new: Vec::new(),
            next_toast: 1,
            app_replies: Vec::new(),
            attention: false,
            quit: false,
            window_focused: Arc::new(AtomicBool::new(true)),
            dirty: Dirty { sessions: true, memory: true, mcp: true, plugins: true },
            seq: 0,
            sent: Sent::default(),
            last_ws: String::new(),
            last_save: Instant::now(),
            mcp_refresh_at: None,
            sys: sysmon::spawn(),
            last_stats: Instant::now() - Duration::from_secs(10),
            terminals,
            courier,
            runtime_model: settings::read_json::<Workspace>(&Self::workspace_path()).and_then(|w| w.runtime_model),
        };
        crate::memory::set_switches(core.settings.memory.enabled, core.settings.memory.inject);
        core.note_new_version();
        if startup.restore {
            core.restore_workspace();
        }
        core.open_startup(startup);
        core.refresh_side();
        (core, rx)
    }

    /// Open what the command line asked for (also for a second launch handing it over).
    pub fn open_startup(&mut self, startup: Startup) {
        let cwd = startup.cwd.clone();
        if startup.shell {
            let dir = cwd.clone().unwrap_or_else(|| self.settings.default_dir());
            self.new_shell(dir, None);
        }
        match startup.agent {
            Some(prompt) => match cwd {
                Some(dir) => {
                    self.new_agent(dir, prompt, None, None, false);
                }
                None => {
                    self.new_session(prompt, false);
                }
            },
            None if !startup.shell && startup.cwd.is_some() => {
                self.new_agent(startup.cwd.unwrap(), None, None, None, false);
            }
            None => {}
        }
    }

    // ------------------------------------------------------------------ workspace

    fn workspace_path() -> PathBuf {
        settings::config_dir().join("workspace.json")
    }

    fn restore_workspace(&mut self) {
        let Some(ws) = settings::read_json::<Workspace>(&Self::workspace_path()) else { return };
        for b in ws.bays {
            match b {
                SavedBay::Agent { cwd, session_id, incognito, fresh } => {
                    let cwd = PathBuf::from(cwd);
                    if fresh {
                        self.new_session(None, incognito);
                    } else if cwd.is_dir() {
                        self.new_agent(cwd, None, session_id, None, incognito);
                    }
                }
                SavedBay::Shell { cwd, shell } => {
                    let cwd = PathBuf::from(cwd);
                    if cwd.is_dir() {
                        self.new_shell(cwd, shell);
                    }
                }
            }
        }
        self.focused = self.bays.get(ws.focused.min(self.bays.len().saturating_sub(1))).map(|b| b.id);
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
                BayKind::Shell(s) => SavedBay::Shell { cwd: s.cwd.display().to_string(), shell: s.shell.clone() },
            })
            .collect();
        Workspace { bays, focused: self.focused_index().unwrap_or(0), runtime_model: self.runtime_model.clone() }
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

    pub fn bay_index(&self, id: BayId) -> Option<usize> {
        self.bays.iter().position(|b| b.id == id)
    }

    fn focused_index(&self) -> Option<usize> {
        self.focused.and_then(|id| self.bay_index(id))
    }

    pub fn agent_mut(&mut self, id: BayId) -> Option<&mut AgentView> {
        self.bays.iter_mut().find(|b| b.id == id).and_then(|b| match &mut b.kind {
            BayKind::Agent(v) => Some(v.as_mut()),
            _ => None,
        })
    }

    pub fn agent(&self, id: BayId) -> Option<&AgentView> {
        self.bays.iter().find(|b| b.id == id).and_then(|b| match &b.kind {
            BayKind::Agent(v) => Some(v.as_ref()),
            _ => None,
        })
    }

    pub fn focus(&mut self, id: BayId) {
        if self.bay_index(id).is_some() {
            self.focused = Some(id);
        }
    }

    /// A terminal bay's folder and shell.
    pub fn shell_bay(&self, id: BayId) -> Option<(PathBuf, Option<String>)> {
        self.bays.iter().find(|b| b.id == id).and_then(|b| match &b.kind {
            BayKind::Shell(s) => Some((s.cwd.clone(), s.shell.clone())),
            BayKind::Agent(_) => None,
        })
    }

    fn push_bay(&mut self, bay: Bay) {
        let id = bay.id;
        self.bays.push(bay);
        self.focused = Some(id);
        self.dirty.sessions = true;
    }

    pub fn new_agent(&mut self, cwd: PathBuf, prompt: Option<String>, resume: Option<String>, mission: Option<u64>, incognito: bool) -> BayId {
        let id = self.alloc_bay();
        let mut view = AgentView::new(id, cwd.clone(), Mode::Interactive);
        view.auto_approve.store(resume.is_none() && self.settings.default_auto_approve, Ordering::Relaxed);
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
            capture_explicit_memory(&self.memory, self.settings.memory.enabled, &mut view, p);
        }
        let first = prompt.as_deref().map(sessions::short_title);
        let spec = self.agent_spec(&view, prompt, Vec::new(), resume);
        self.backend.send(Cmd::Create { bay: id, spec });
        if let Some(name) = first {
            self.backend.send(Cmd::NameSession { bay: id, name, user: false });
        }
        self.push_bay(Bay { id, kind: BayKind::Agent(Box::new(view)) });
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
    pub fn new_session(&mut self, prompt: Option<String>, incognito: bool) -> BayId {
        if !self.settings.session_folders {
            let dir = self.settings.default_dir();
            return self.new_agent(dir, prompt, None, None, incognito);
        }
        if let Some(p) = prompt {
            let dir = self.create_session_folder(&p);
            return self.new_agent(dir, Some(p), None, None, incognito);
        }
        // An empty new session is already open: go to it rather than adding another.
        let blank = self.bays.iter().find_map(|b| match &b.kind {
            BayKind::Agent(v) if v.deferred && v.items.is_empty() && v.attachments.is_empty() && v.incognito == incognito => Some(b.id),
            _ => None,
        });
        if let Some(id) = blank {
            self.focus(id);
            return id;
        }
        // Nothing to name it after yet: the folder and Copilot session start with the first message.
        let id = self.alloc_bay();
        let root = self.settings.sessions_root();
        let mut view = AgentView::new(id, root.clone(), Mode::Interactive);
        view.auto_approve.store(self.settings.default_auto_approve, Ordering::Relaxed);
        view.project = self.project_for(&root);
        view.deferred = true;
        view.incognito = incognito;
        view.status = Status::Idle;
        view.model = self.settings.default_model.clone();
        view.effort = self.settings.default_effort.clone();
        self.push_bay(Bay { id, kind: BayKind::Agent(Box::new(view)) });
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
    fn start_deferred(&mut self, bay: BayId, text: String) {
        self.start_deferred_as(bay, text.clone(), text);
    }

    /// Like [`Self::start_deferred`], showing `shown` while sending `sent` (which can carry more context).
    fn start_deferred_as(&mut self, bay: BayId, shown: String, sent: String) {
        let text = shown;
        let dir = self.create_session_folder(&text);
        let project = self.project_for(&dir);
        let enabled = self.settings.memory.enabled;
        let memory = self.memory.clone();
        let Some(view) = self.agent_mut(bay) else { return };
        let files = std::mem::take(&mut view.attachments);
        view.granted.lock().extend(files.iter().cloned());
        view.cwd = dir;
        view.project = project;
        view.deferred = false;
        view.status = Status::Booting;
        view.title = Some(text.clone());
        let names = files.iter().map(|p| settings::dir_name(p)).collect();
        view.echo_user_shown(&text, &sent, names, false);
        capture_explicit_memory(&memory, enabled, view, &text);
        let (name, user) = (view.name.clone().unwrap_or_else(|| sessions::short_title(&text)), view.name.is_some());
        let Some(view) = self.agent(bay) else { return };
        let spec = self.agent_spec(view, Some(sent), files, None);
        self.backend.send(Cmd::Create { bay, spec });
        self.backend.send(Cmd::NameSession { bay, name, user });
    }

    /// Memory scope for a folder: every session folder shares the sessions folder's scope.
    fn project_for(&self, dir: &Path) -> Project {
        let root = self.settings.sessions_root();
        Project::for_dir(if sessions::is_under(dir, &root) { &root } else { dir })
    }

    fn agent_spec(&self, view: &AgentView, prompt: Option<String>, files: Vec<PathBuf>, resume: Option<String>) -> AgentSpec {
        let memory = (self.settings.memory.enabled && !view.incognito).then(|| crate::agent::backend::AgentMemory { store: self.memory.clone(), project: view.project.clone() });
        // Choices made before the session existed (a deferred one) carry over.
        let model = view.model.clone().or_else(|| self.settings.default_model.clone());
        let effort = self.effort_for(model.as_deref().or(self.runtime_model.as_deref()), view.effort.clone().or_else(|| self.settings.default_effort.clone()));
        AgentSpec {
            cwd: view.cwd.clone(),
            model,
            effort,
            mode: view.mode,
            prompt,
            files,
            auto_approve: view.auto_approve.clone(),
            resume,
            granted: view.granted.clone(),
            memory,
        }
    }

    /// The reasoning effort to ask model for: the one wanted when the model offers it, else the
    /// model's default. None (the runtime decides) for models without the setting, or unknown ones.
    pub fn effort_for(&self, model: Option<&str>, wanted: Option<String>) -> Option<String> {
        effort_for(&self.models, model, wanted)
    }

    /// A restored session that no longer exists continues as a fresh agent in the same folder.
    fn restart_fresh(&mut self, bay: BayId) -> bool {
        let Some(v) = self.agent_mut(bay) else { return false };
        if v.session_id.is_none() {
            return false;
        }
        v.items.retain(|i| !matches!(i, Item::Notice { text, .. } if text == "Resuming session…"));
        v.session_id = None;
        v.learn_cursor = 0;
        v.notice(Level::Info, "That session is no longer available, so this is a new one.");
        let Some(v) = self.agent(bay) else { return false };
        let spec = self.agent_spec(v, None, Vec::new(), None);
        self.backend.send(Cmd::Create { bay, spec });
        true
    }

    pub fn new_shell(&mut self, cwd: PathBuf, shell: Option<String>) -> BayId {
        let id = self.alloc_bay();
        if !sessions::is_under(&cwd, &self.settings.sessions_root()) {
            self.settings.push_recent_dir(&cwd);
            self.settings_dirty = true;
        }
        self.push_bay(Bay { id, kind: BayKind::Shell(ShellBay { cwd, shell, title: None }) });
        id
    }

    pub fn close_bay(&mut self, id: BayId) {
        let Some(index) = self.bay_index(id) else { return };
        if self.settings.memory.learning() {
            self.start_learning(id);
        }
        let bay = self.bays.remove(index);
        match bay.kind {
            BayKind::Agent(_) => {
                self.backend.send(Cmd::Close { bay: bay.id });
                self.apps.retain(|a| a.bay != bay.id);
                self.missions.on_bay_closed(bay.id);
            }
            BayKind::Shell(_) => self.terminals.kill(&terminal_id(bay.id)),
        }
        self.sent.items.remove(&bay.id);
        if self.focused == Some(bay.id) {
            self.focused = self.bays.get(index.min(self.bays.len().saturating_sub(1))).map(|b| b.id);
        }
        if self.bays.len() <= 1 {
            self.maximized = false;
        }
        self.dirty.sessions = true;
    }

    /// Resume a past session in the folder it was created in (or focus it if it's open).
    pub fn resume_session(&mut self, id: String) -> BayId {
        if let Some(b) = self.bays.iter().find(|b| matches!(&b.kind, BayKind::Agent(v) if v.session_id.as_deref() == Some(id.as_str()))) {
            let bay = b.id;
            self.focus(bay);
            return bay;
        }
        let dir = self.session_index.get(&id).and_then(|m| m.folder.clone()).map(PathBuf::from).filter(|d| d.is_dir()).or_else(|| session_cwd(&id).filter(|d| d.is_dir())).unwrap_or_else(|| self.settings.default_dir());
        self.new_agent(dir, None, Some(id), None, false)
    }

    pub fn launch_mission(&mut self, id: u64) {
        let Some(m) = self.missions.get(id).cloned() else { return };
        let bay = match m.dir.as_ref().map(PathBuf::from).filter(|p| p.is_dir()) {
            Some(dir) => self.new_agent(dir, Some(m.brief()), None, Some(id), false),
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

    /// Hand a mission to an open bay: an agent works on it, a terminal gets its text pasted.
    pub fn send_mission_to(&mut self, bay: BayId, id: u64) -> Result<(), String> {
        let Some(m) = self.missions.get(id).cloned() else { return Err("That mission no longer exists.".into()) };
        let Some(index) = self.bay_index(bay) else { return Err("That bay is closed.".into()) };
        match &mut self.bays[index].kind {
            BayKind::Agent(v) if v.deferred => {
                v.mission = Some(id);
                self.start_deferred(bay, m.brief());
            }
            BayKind::Agent(v) => {
                if v.status.is_busy() {
                    return Err(format!("{} is busy. Wait for it to finish, or launch the mission in a new session.", v.display_title()));
                }
                v.echo_user(&m.brief());
                v.mission = Some(id);
                v.learn_due = None;
                self.backend.send(Cmd::Send { bay, text: m.brief(), files: Vec::new(), steer: false });
            }
            BayKind::Shell(_) => return Err("Missions go to agents. Drop it on an agent, or launch it in a new session.".into()),
        }
        self.missions.link(id, bay);
        self.focus(bay);
        self.toast(Level::Ok, format!("Delegated “{}”", m.title));
        Ok(())
    }

    pub fn toast(&mut self, level: Level, text: impl Into<String>) {
        self.toast_with(level, text, None);
    }

    pub fn toast_with(&mut self, level: Level, text: impl Into<String>, action: Option<(String, Value)>) {
        let id = self.next_toast;
        self.next_toast += 1;
        self.toasts_new.push(ToastDto { id, level: level_str(level), text: text.into(), action });
    }

    fn log(&mut self, line: crate::agent::LogLine) {
        let dto = LogLineDto { bay: line.bay, level: level_str(line.level), text: line.text, at: chrono::Utc::now().timestamp_millis() };
        self.activity.push_front(dto.clone());
        self.activity.truncate(300);
        self.activity_new.push(dto);
    }

    fn focused_agent_bay(&self) -> Option<BayId> {
        let focused = self.focused.and_then(|id| self.agent(id)).map(|v| v.bay);
        focused.or_else(|| self.bays.iter().find(|b| matches!(b.kind, BayKind::Agent(_))).map(|b| b.id))
    }

    /// Reload what the open side panel shows.
    pub fn refresh_side(&mut self) {
        match self.settings.side_panel {
            Some(SidePanel::Mcp) => self.refresh_mcp(),
            Some(SidePanel::Plugins) => self.backend.send(Cmd::Plugin(PluginCmd::Refresh)),
            Some(SidePanel::Sessions) => self.backend.send(Cmd::ListSessions),
            _ => {}
        }
    }

    fn refresh_mcp(&mut self) {
        let cwd = self.focused.and_then(|id| self.bays.iter().find(|b| b.id == id)).map(|b| b.cwd().display().to_string());
        self.mcp.loading = true;
        self.dirty.mcp = true;
        self.backend.send(Cmd::Mcp(McpCmd::Refresh { bay: self.focused_agent_bay(), cwd }));
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

    /// Rebuild the search index in the background (unless one is fresh or already building).
    pub fn refresh_search_index(&mut self, force: bool) {
        if self.search.rx.is_some() || (!force && self.search.built.is_some_and(|t| t.elapsed() < Duration::from_secs(30))) {
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
        let _ = std::fs::create_dir_all(&cache_dir);
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let index = crate::search::Index::build(&root, &sources, &state_dir, &cache_dir);
            let _ = tx.send(index);
        });
        self.search.rx = Some(rx);
    }

    fn poll_search_index(&mut self) {
        let Some(rx) = &self.search.rx else { return };
        match rx.try_recv() {
            Ok(index) => {
                tracing::info!(sessions = index.stats.sessions, transcripts = index.stats.transcripts, files = index.stats.files, kb = index.stats.bytes / 1024, ms = index.stats.millis as u64, "search index built");
                self.search.index = Arc::new(index);
                self.search.built = Some(Instant::now());
                self.search.rx = None;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.search.rx = None,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }

    pub fn search_index(&self) -> Arc<crate::search::Index> {
        self.search.index.clone()
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
            self.toast_with(Level::Ok, format!("Graysense updated to {now}."), Some(("What's new".into(), json!({ "type": "openUrl", "url": self.updater.notes_url() }))));
        }
        self.settings.last_version = Some(now);
        self.settings_dirty = true;
    }

    /// Install the downloaded update and reopen Graysense with the same bays. Returns how many
    /// agents are working when it needs the user to confirm first (restarting stops them).
    pub fn restart_to_update(&mut self, confirmed: bool) -> Result<usize, String> {
        let working = self.bays.iter().filter(|b| matches!(&b.kind, BayKind::Agent(v) if v.status.is_busy())).count();
        if working > 0 && !confirmed {
            return Ok(working);
        }
        self.updater.restart()?;
        self.quit = true;
        Ok(0)
    }

    /// Apply settings that changed (from the UI or an agent).
    pub fn settings_changed(&mut self, before: &Settings) {
        if self.settings.side_panel != before.side_panel {
            self.refresh_side();
        }
        if self.settings.layout != before.layout {
            self.maximized = false;
        }
        if self.settings.auto_update != before.auto_update {
            self.updater.set_auto(self.settings.auto_update);
        }
        if self.settings.sessions_root() != before.sessions_root() {
            self.refresh_search_index(true);
            self.dirty.sessions = true;
        }
        crate::memory::set_switches(self.settings.memory.enabled, self.settings.memory.inject);
        self.settings_dirty = true;
    }

    // ------------------------------------------------------------------ events

    pub fn handle_event(&mut self, evt: Evt) {
        match evt {
            Evt::Client(state) => {
                if let ClientState::Failed(e) = &state {
                    self.toast(Level::Error, format!("The Copilot runtime couldn't start: {e}"));
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
                    None => Ok(json!({ "settings": self.settings.describe(&models), "to_change": "Call again with action \"set\" and `changes`." })),
                    Some(changes) => {
                        let before = self.settings.clone();
                        self.settings.apply_patch(&changes, &models).map(|done| {
                            self.settings_changed(&before);
                            self.toast(Level::Ok, format!("Copilot changed {}", done.join(", ")));
                            json!({ "changed": done })
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
                self.dirty.sessions = true;
                if self.settings.side_panel == Some(SidePanel::Mcp) {
                    self.refresh_mcp();
                }
            }
            Evt::SessionFailed { bay, error } => {
                if error.contains("not found") && self.restart_fresh(bay) {
                    return;
                }
                if let Some(v) = self.agent_mut(bay) {
                    let hint = if error.contains("unavailable") { " Install the GitHub Copilot CLI and sign in with `copilot login`, then open a new session." } else { "" };
                    v.notice(Level::Error, format!("Couldn't start the agent: {error}.{hint}"));
                    v.status = Status::Error(error);
                }
            }
            Evt::SessionClosed { bay } => {
                if let Some(v) = self.agent_mut(bay) {
                    v.show_queued();
                    v.status = Status::Offline;
                }
            }
            Evt::Event { bay, ev } => {
                // A new session started without a model says which one the runtime picks (a resumed
                // one doesn't report a startup model).
                if ev.event_type == "session.model_change"
                    && ev.data.get("source").and_then(Value::as_str) == Some("startup")
                    && self.agent(bay).is_some_and(|v| v.model.is_none())
                    && let Some(m) = ev.data.get("newModel").and_then(Value::as_str)
                {
                    self.runtime_model = Some(m.to_owned());
                    // It couldn't be given its thinking level before the model was known.
                    let wanted = self.agent(bay).and_then(|v| v.effort.clone());
                    if ev.data.get("reasoningEffort").is_none_or(Value::is_null)
                        && let Some(e) = self.effort_for(Some(m), wanted)
                    {
                        self.backend.send(Cmd::SetModel { bay, model: m.to_owned(), effort: EffortChange::Set(e) });
                    }
                }
                let effects = match self.agent_mut(bay) {
                    Some(v) => v.apply(&ev),
                    None => return,
                };
                self.apply_effects(bay, effects);
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
                    // A replayed turn's end (session.idle) isn't kept in the history.
                    v.status = Status::Idle;
                    v.busy_since = None;
                }
            }
            Evt::Notice { bay, error, text } => {
                if let Some(v) = self.agent_mut(bay) {
                    v.notice(if error { Level::Error } else { Level::Info }, text);
                }
            }
            Evt::Toast { error, text } => self.toast(if error { Level::Error } else { Level::Ok }, text),
            Evt::Permission { bay, prompt } => self.push_prompt(bay, Item::Permission(prompt), "approval"),
            Evt::Elicit { bay, prompt } => self.push_prompt(bay, Item::Elicit(prompt), "input"),
            Evt::Ask { bay, prompt } => self.push_prompt(bay, Item::Ask(prompt), "an answer"),
            Evt::Plan { bay, prompt } => self.push_prompt(bay, Item::Plan(prompt), "plan review"),
            Evt::Switch { bay, prompt } => self.push_prompt(bay, Item::Switch(prompt), "a decision"),
            Evt::CanvasOpen { bay, instance_id, canvas_id, input } => {
                if let Some(v) = self.agent_mut(bay)
                    && let Some(c) = CanvasInstance::new(instance_id.clone(), canvas_id, &input)
                {
                    v.canvases.retain(|x| x.instance_id != instance_id);
                    v.canvases.push(c);
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
                self.sessions_loaded = true;
                self.dirty.sessions = true;
                if self.search.built.is_none() {
                    self.refresh_search_index(true);
                }
            }
            Evt::McpServers { servers, live_bay } => {
                self.mcp.servers = Some(servers);
                self.mcp.live_bay = live_bay;
                self.mcp.loading = false;
                self.dirty.mcp = true;
            }
            Evt::McpTools { server, tools } => {
                self.mcp.tools.retain(|(s, _)| *s != server);
                self.mcp.tools.push((server, tools));
                self.dirty.mcp = true;
            }
            Evt::Plugins(p) => {
                self.plugins.installed = Some(p);
                self.dirty.plugins = true;
            }
            Evt::Markets(m) => {
                self.plugins.markets = Some(m);
                self.dirty.plugins = true;
            }
            Evt::MarketPlugins { market, plugins } => {
                self.plugins.browse.retain(|(m, _)| *m != market);
                self.plugins.browse.push((market, plugins));
                self.dirty.plugins = true;
            }
            Evt::AppReply { view, req_id, result } => {
                let msg = match result {
                    Ok(r) => json!({ "view": view, "message": { "jsonrpc": "2.0", "id": req_id, "result": r } }),
                    Err(e) => json!({ "view": view, "message": { "jsonrpc": "2.0", "id": req_id, "error": { "code": -32000, "message": e } } }),
                };
                self.app_replies.push(msg);
            }
            Evt::AddMission { bay, title, notes } => {
                self.missions.add(&title, &notes, true);
                let who = self.agent(bay).map(|v| v.display_title()).unwrap_or_else(|| "An agent".into());
                self.toast_with(Level::Info, format!("{who} added a mission: {title}"), Some(("Open missions".into(), json!({ "type": "setSide", "panel": "missions" }))));
            }
            Evt::Notify { bay, title, message, level } => {
                let lvl = match level.as_str() {
                    "success" => Level::Ok,
                    "warning" => Level::Warn,
                    "error" => Level::Error,
                    _ => Level::Info,
                };
                let who = self.agent(bay).map(|v| v.display_title()).unwrap_or_default();
                self.toast_with(lvl, format!("{who} · {title}: {message}"), Some(("Show".into(), json!({ "type": "focusBay", "bay": bay }))));
                self.attention = true;
            }
            Evt::Remembered { bay, id, text } => {
                if let Some(v) = self.agent_mut(bay) {
                    // The prompt itself may already have saved it ("remember that …").
                    let shown = v.items.iter().any(|i| matches!(i, Item::Memory { ids, .. } if ids.contains(&id)));
                    if !shown {
                        v.push(Item::Memory { ids: vec![id], text, learned: false, undone: false });
                    }
                }
                self.dirty.memory = true;
            }
            Evt::Recalled { bay, entries } => {
                if let Some(v) = self.agent_mut(bay) {
                    for e in &entries {
                        if !v.memory_used.iter().any(|(id, _)| *id == e.0) {
                            v.memory_used.push(e.clone());
                        }
                    }
                    v.push(Item::Recall { entries });
                }
            }
            Evt::Learned { bay, result } => self.finish_learning(bay, result),
            Evt::McpChanged => self.refresh_mcp(),
        }
    }

    fn push_prompt(&mut self, bay: BayId, item: Item, what: &str) {
        let focused = self.focused == Some(bay) && self.window_focused.load(Ordering::Relaxed);
        let Some(v) = self.agent_mut(bay) else { return };
        v.push(item);
        v.status = Status::Waiting(what.to_owned());
        let title = v.display_title();
        if !focused {
            self.toast_with(Level::Warn, format!("{title} needs {what}"), Some(("Show".into(), json!({ "type": "focusBay", "bay": bay }))));
        }
        self.attention = true;
    }

    fn apply_effects(&mut self, bay: BayId, effects: Vec<Effect>) {
        for e in effects {
            match e {
                Effect::Log(line) => self.log(line),
                Effect::OpenApp(req) => self.open_app(bay, req),
                Effect::Finished => {
                    self.missions.on_agent_finished(bay);
                    if let Some(v) = self.agent_mut(bay) {
                        v.learn_due = Some(Instant::now() + learn::LEARN_DELAY);
                    }
                    let focused = self.focused == Some(bay) && self.window_focused.load(Ordering::Relaxed);
                    if !focused {
                        let title = self.agent(bay).map(|v| v.display_title()).unwrap_or_default();
                        self.toast_with(Level::Ok, format!("{title} finished"), Some(("Show".into(), json!({ "type": "focusBay", "bay": bay }))));
                        self.attention = true;
                    }
                }
                Effect::McpChanged => {
                    if self.settings.side_panel == Some(SidePanel::Mcp) && self.mcp_refresh_at.is_none() {
                        self.mcp_refresh_at = Some(Instant::now() + Duration::from_millis(400));
                    }
                }
            }
        }
    }

    /// Register an MCP App a tool returned and dock it in its agent's bay.
    fn open_app(&mut self, bay: BayId, req: crate::agent::AppRequest) {
        use base64::Engine;
        let res = &req.resource;
        let html = match res.get("text").and_then(|t| t.as_str()) {
            Some(text) => text.to_owned(),
            None => {
                let decoded = res.get("blob").and_then(|b| b.as_str()).and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok()).and_then(|b| String::from_utf8(b).ok());
                match decoded {
                    Some(html) => html,
                    None => return,
                }
            }
        };
        let id = self.next_view;
        self.next_view += 1;
        let server = req.server.clone().unwrap_or_default();
        let label = req.label.clone().unwrap_or_else(|| req.tool.strip_prefix(&format!("{server}-")).unwrap_or(&req.tool).to_owned());
        let title = if server.is_empty() { label } else { format!("{label} · {server}") };
        self.apps.push(AppView {
            id,
            bay,
            title: title.clone(),
            server,
            tool: req.tool,
            tool_call_id: req.tool_call_id,
            html,
            meta: res.get("_meta").cloned().unwrap_or(Value::Null),
            args: req.args,
            result: req.result,
        });
        if let Some(v) = self.agent_mut(bay) {
            v.apps.push(AppTab { view: id, title });
        }
    }

    pub fn app_view(&self, id: u64) -> Option<&AppView> {
        self.apps.iter().find(|a| a.id == id)
    }

    // ------------------------------------------------------------------ housekeeping

    /// Periodic work: finished background jobs, learning, saving.
    pub fn tick(&mut self) {
        if let Some(v) = self.updater.tick(self.settings.auto_update) {
            self.toast_with(Level::Info, format!("Graysense {v} is ready. Restart to update, or it installs when you close Graysense."), Some(("Restart".into(), json!({ "type": "updateRestart", "confirmed": false }))));
        }
        self.poll_search_index();
        if self.mcp_refresh_at.is_some_and(|t| Instant::now() >= t) {
            self.mcp_refresh_at = None;
            self.refresh_mcp();
        }
        self.memory_tick();
        if self.last_save.elapsed() > Duration::from_secs(3) {
            self.save();
            self.last_save = Instant::now();
        }
    }

    fn save(&mut self) {
        self.missions.save_if_dirty();
        self.memory.lock().save_if_dirty();
        self.session_index.save_if_dirty();
        self.save_workspace_if_changed();
        if self.settings_dirty {
            self.settings.save();
            self.settings_dirty = false;
        }
    }

    /// Graysense is closing: save everything, queue unread conversations, stop the runtime.
    pub fn on_exit(&mut self) {
        self.save_workspace_if_changed();
        self.missions.save_if_dirty();
        self.queue_unread_conversations();
        self.memory.lock().save_if_dirty();
        self.session_index.save_if_dirty();
        self.settings.save();
        self.terminals.kill_all();
        self.backend.shutdown(&self.rt);
        // Last, so the installer (which waits for this process to exit) starts after cleanup.
        self.updater.on_exit();
    }

    // ------------------------------------------------------------------ the UI's view

    fn app_dto(&self) -> AppDto {
        let (client, client_error) = match &self.client {
            ClientState::Idle => ("idle", None),
            ClientState::Connecting => ("connecting", None),
            ClientState::Ready => ("ready", None),
            ClientState::Failed(e) => ("failed", Some(e.clone())),
        };
        let stats = &self.search.index.stats;
        AppDto {
            client,
            client_error,
            models: self.models.iter().map(ModelDto::from).collect(),
            runtime_model: self.runtime_model.clone(),
            focused: self.focused,
            maximized: self.maximized,
            version: self.updater.version.clone(),
            sessions_root: self.settings.sessions_root().display().to_string(),
            default_dir: self.settings.default_dir().display().to_string(),
            home: dirs::home_dir().map(|h| h.display().to_string()).unwrap_or_default(),
            config_dir: settings::config_dir().display().to_string(),
            search: SearchStatusDto { building: self.search.rx.is_some(), built: self.search.built.is_some(), sessions: stats.sessions, files: stats.files },
        }
    }

    fn settings_value(&self) -> Value {
        let mut v = self.settings.values();
        if let Some(o) = v.as_object_mut() {
            o.insert("recent_dirs".into(), json!(self.settings.recent_dirs));
        }
        v
    }

    fn bay_dtos(&self) -> Vec<BayDto> {
        let root = self.settings.sessions_root();
        self.bays
            .iter()
            .map(|b| BayDto {
                id: b.id,
                kind: match b.kind {
                    BayKind::Agent(_) => "agent",
                    BayKind::Shell(_) => "shell",
                },
                title: b.title(),
                cwd: b.cwd().display().to_string(),
                folder: sessions::display_path(b.cwd(), &root),
                agent: match &b.kind {
                    BayKind::Agent(v) => Some(agent_dto(v)),
                    BayKind::Shell(_) => None,
                },
                shell: match &b.kind {
                    BayKind::Shell(s) => s.shell.clone(),
                    BayKind::Agent(_) => None,
                },
            })
            .collect()
    }

    fn sessions_dto(&self) -> SessionsDto {
        let open: HashMap<&str, BayId> = self.bays.iter().filter_map(|b| if let BayKind::Agent(v) = &b.kind { v.session_id.as_deref().map(|s| (s, b.id)) } else { None }).collect();
        let items = self
            .sessions
            .iter()
            .map(|s| {
                let meta = self.session_index.get(&s.id);
                PastSessionDto {
                    id: s.id.clone(),
                    title: self.session_title(s),
                    modified: s.modified.clone(),
                    modified_ts: chrono::DateTime::parse_from_rfc3339(&s.modified).map(|d| d.timestamp()).unwrap_or(0),
                    pinned: self.session_index.pinned(&s.id),
                    external: s.client.as_deref().is_some_and(|c| !c.starts_with("graysense")),
                    folder: meta.and_then(|m| m.folder.clone()),
                    open_bay: open.get(s.id.as_str()).copied(),
                }
            })
            .collect();
        SessionsDto { loaded: self.sessions_loaded, items }
    }

    fn memory_dto(&self) -> MemoryStateDto {
        let now = crate::memory::now();
        let store = self.memory.lock();
        MemoryStateDto { memories: store.memories.iter().map(|m| memory_dto(m, now)).collect(), last_run: self.memory_last_run.clone(), learning: self.learn_jobs.len() }
    }

    fn mcp_dto(&self) -> McpDto {
        McpDto {
            loading: self.mcp.loading,
            servers: self.mcp.servers.as_ref().map(|r| r.as_ref().map(|list| list.iter().map(McpServerDto::from).collect()).map_err(Clone::clone)),
            live_bay: self.mcp.live_bay,
            tools: self.mcp.tools.iter().map(|(s, r)| (s.clone(), r.as_ref().map(|t| t.iter().map(McpToolDto::from).collect()).map_err(Clone::clone))).collect(),
        }
    }

    fn plugins_dto(&self) -> PluginsDto {
        PluginsDto {
            installed: self.plugins.installed.as_ref().map(|r| r.as_ref().map(|l| l.iter().map(PluginDto::from).collect()).map_err(Clone::clone)),
            markets: self.plugins.markets.as_ref().map(|r| r.as_ref().map(|l| l.iter().map(MarketDto::from).collect()).map_err(Clone::clone)),
            browse: self.plugins.browse.iter().map(|(m, r)| (m.clone(), r.as_ref().map(|l| l.iter().map(MarketPluginDto::from).collect()).map_err(Clone::clone))).collect(),
        }
    }

    fn stats_dto(&self) -> StatsDto {
        let s = self.sys.lock();
        StatsDto { cpu: s.cpu, mem_used: s.mem_used, mem_total: s.mem_total, app_mem: s.app_mem }
    }

    /// New and changed transcript items, per agent bay (all of them when `full`).
    fn transcript_patches(&mut self, full: bool) -> Vec<TranscriptPatch> {
        let mut out = Vec::new();
        for b in &self.bays {
            let BayKind::Agent(v) = &b.kind else { continue };
            let sent = self.sent.items.entry(b.id).or_default();
            let mut items = Vec::new();
            for (i, item) in v.items.iter().enumerate() {
                let fp = fingerprint(item);
                if full || sent.get(i) != Some(&fp) {
                    items.push((i, item_dto(item)));
                    if i < sent.len() {
                        sent[i] = fp;
                    } else {
                        sent.push(fp);
                    }
                }
            }
            let shrank = sent.len() > v.items.len();
            sent.truncate(v.items.len());
            if full || shrank || !items.is_empty() {
                out.push(TranscriptPatch { bay: b.id, len: v.items.len(), items });
            }
        }
        out
    }

    /// Flush, and send what changed on its way to the window (in order with every other flush).
    pub fn send_out(&mut self) -> Delivery {
        let out = self.flush();
        self.courier.send(out)
    }

    /// Everything that changed since the last flush.
    fn flush(&mut self) -> Outbox {
        let mut u = UiUpdate::default();
        let app = self.app_dto();
        let s = sig(&app);
        if s != self.sent.app {
            self.sent.app = s;
            u.app = Some(app);
        }
        let settings = self.settings_value();
        let s = sig(&settings);
        if s != self.sent.settings {
            self.sent.settings = s;
            u.settings = Some(settings);
        }
        let bays = self.bay_dtos();
        let s = sig(&bays);
        if s != self.sent.bays {
            self.sent.bays = s;
            u.bays = Some(bays);
        }
        u.transcripts = self.transcript_patches(false);
        let missions: Vec<MissionDto> = self.missions.missions.iter().map(mission_dto).collect();
        let s = sig(&missions);
        if s != self.sent.missions {
            self.sent.missions = s;
            u.missions = Some(missions);
        }
        if std::mem::take(&mut self.dirty.sessions) {
            u.sessions = Some(self.sessions_dto());
        }
        let rev = self.memory.lock().revision;
        if std::mem::take(&mut self.dirty.memory) || self.sent.memory != Some(rev) {
            self.sent.memory = Some(rev);
            u.memory = Some(self.memory_dto());
        }
        if std::mem::take(&mut self.dirty.mcp) {
            u.mcp = Some(self.mcp_dto());
        }
        if std::mem::take(&mut self.dirty.plugins) {
            u.plugins = Some(self.plugins_dto());
        }
        let status = self.updater.status();
        if self.sent.update.as_ref() != Some(&status) {
            self.sent.update = Some(status.clone());
            u.update = Some(status);
        }
        if self.last_stats.elapsed() >= Duration::from_secs(2) {
            self.last_stats = Instant::now();
            u.stats = Some(self.stats_dto());
        }
        u.activity = std::mem::take(&mut self.activity_new);
        u.toasts = std::mem::take(&mut self.toasts_new);

        let mut out = Outbox { app_replies: std::mem::take(&mut self.app_replies), attention: std::mem::take(&mut self.attention) && !self.window_focused.load(Ordering::Relaxed), quit: self.quit, ..Default::default() };
        let title = match self.focused.and_then(|id| self.bays.iter().find(|b| b.id == id)) {
            Some(b) => format!("{} — Graysense", b.title()),
            None => "Graysense".to_owned(),
        };
        if title != self.sent.title {
            self.sent.title = title.clone();
            out.title = Some(title);
        }
        if !u.is_empty() {
            self.seq += 1;
            u.seq = self.seq;
            out.update = Some(u);
        }
        out
    }

    /// The whole state, for a UI that's (re)starting. Pending changes go out first, so the
    /// snapshot is exactly the state after update `seq`.
    pub fn snapshot(&mut self) -> (Delivery, UiUpdate) {
        let pending = self.send_out();
        let u = UiUpdate {
            seq: self.seq,
            full: true,
            app: Some(self.app_dto()),
            settings: Some(self.settings_value()),
            bays: Some(self.bay_dtos()),
            transcripts: self.transcript_patches(true),
            missions: Some(self.missions.missions.iter().map(mission_dto).collect()),
            sessions: Some(self.sessions_dto()),
            memory: Some(self.memory_dto()),
            mcp: Some(self.mcp_dto()),
            plugins: Some(self.plugins_dto()),
            update: Some(self.updater.status()),
            stats: Some(self.stats_dto()),
            activity: self.activity.iter().rev().cloned().collect(),
            toasts: Vec::new(),
        };
        (pending, u)
    }
}

/// See [`Core::effort_for`].
fn effort_for(models: &[ModelInfo], model: Option<&str>, wanted: Option<String>) -> Option<String> {
    let wanted = wanted?;
    let info = models.iter().find(|m| Some(m.id.as_str()) == model)?;
    if info.efforts.is_empty() {
        return None;
    }
    if info.efforts.contains(&wanted) { Some(wanted) } else { info.default_effort.clone() }
}

/// The id of a terminal bay's shell in [`Terminals`].
pub fn terminal_id(bay: BayId) -> String {
    format!("bay-{bay}")
}

/// Hand a flush's results to the window (on the courier's thread: window calls wait for the
/// main thread).
fn deliver(app: &AppHandle, out: Outbox) {
    if let Some(update) = out.update {
        let _ = app.emit("ui", update);
    }
    for reply in out.app_replies {
        let _ = app.emit("app-reply", reply);
    }
    if let Some(w) = app.get_webview_window("main") {
        if let Some(title) = out.title {
            let _ = w.set_title(&title);
        }
        if out.attention {
            let _ = w.request_user_attention(Some(tauri::UserAttentionType::Informational));
        }
    }
    if out.quit {
        app.exit(0);
    }
}

/// The core's thread: backend events in, UI updates out, housekeeping in between.
pub fn run_loop(core: Arc<Mutex<Core>>, rx: Receiver<Evt>) {
    let spawned = std::thread::Builder::new().name("graysense-core".into()).spawn(move || {
        let mut last_tick = Instant::now();
        let mut last_flush = Instant::now();
        let mut pending = true;
        loop {
            let wait = if pending { FLUSH_GAP.saturating_sub(last_flush.elapsed()) } else { TICK.saturating_sub(last_tick.elapsed()) };
            let first = match rx.recv_timeout(wait.max(Duration::from_millis(1))) {
                Ok(e) => Some(e),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            let mut c = core.lock();
            if let Some(e) = first {
                c.handle_event(e);
                while let Ok(e) = rx.try_recv() {
                    c.handle_event(e);
                }
                pending = true;
            }
            if last_tick.elapsed() >= TICK {
                c.tick();
                last_tick = Instant::now();
                pending = true;
            }
            if pending && last_flush.elapsed() >= FLUSH_GAP {
                pending = false;
                last_flush = Instant::now();
                drop(c.send_out());
            }
        }
    });
    if let Err(e) = spawned {
        tracing::error!("couldn't start the core thread: {e}");
    }
}

/// Save a "remember that …" request from a prompt immediately, with an Undo chip. This works
/// even while background learning is paused: the user asked explicitly.
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

/// Working directory a Copilot session was started in (from its `workspace.yaml`).
fn session_cwd(id: &str) -> Option<PathBuf> {
    let file = dirs::home_dir()?.join(".copilot").join("session-state").join(id).join("workspace.yaml");
    let text = std::fs::read_to_string(file).ok()?;
    text.lines().find_map(|l| l.strip_prefix("cwd:")).map(|v| PathBuf::from(v.trim().trim_matches('"').trim_matches('\'')))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_arguments() {
        let s = Startup::parse(["--agent".to_owned(), "fix the build".to_owned(), "--no-restore".to_owned()], None);
        assert_eq!(s.agent, Some(Some("fix the build".to_owned())));
        assert!(!s.restore && !s.shell);
        let s = Startup::parse(["--agent".to_owned(), "--shell".to_owned()], None);
        assert_eq!(s.agent, Some(None));
        assert!(s.shell);
        let tmp = std::env::temp_dir();
        let s = Startup::parse([".".to_owned()], Some(&tmp));
        assert_eq!(s.cwd, Some(tmp.join(".")));
    }

    #[test]
    fn the_thinking_level_follows_what_the_model_offers() {
        let model = |id: &str, efforts: &[&str], default: Option<&str>| ModelInfo {
            id: id.into(),
            name: id.into(),
            multiplier: Some(1.0),
            category: None,
            efforts: efforts.iter().map(|e| e.to_string()).collect(),
            default_effort: default.map(str::to_owned),
        };
        let models = [model("opus", &["low", "medium", "high", "max"], Some("medium")), model("mini", &[], None)];
        let want = |e: &str| Some(e.to_owned());
        assert_eq!(effort_for(&models, Some("opus"), want("max")), want("max"));
        // A level the model doesn't offer falls back to its default; a model without levels gets none.
        assert_eq!(effort_for(&models, Some("opus"), want("xhigh")), want("medium"));
        assert_eq!(effort_for(&models, Some("mini"), want("high")), None);
        // Unknown models, or nothing chosen, leave it to the runtime.
        assert_eq!(effort_for(&models, Some("new-model"), want("high")), None);
        assert_eq!(effort_for(&models, Some("opus"), None), None);
    }
}
