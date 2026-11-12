//! Native Copilot agent bays: SDK-driven view model.

pub mod backend;
pub mod files;
pub mod forms;
pub mod markdown;
pub mod view;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use github_copilot_sdk::SessionEvent;
use github_copilot_sdk::handler::{AutoModeSwitchResponse, ExitPlanModeResult};
use github_copilot_sdk::types::ElicitationResult;
use serde_json::Value;
use tokio::sync::oneshot;

pub type BayId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Interactive,
    Plan,
    Autopilot,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Interactive => "Ask",
            Mode::Plan => "Plan",
            Mode::Autopilot => "Autopilot",
        }
    }
    pub fn next(self) -> Mode {
        match self {
            Mode::Interactive => Mode::Plan,
            Mode::Plan => Mode::Autopilot,
            Mode::Autopilot => Mode::Interactive,
        }
    }
    pub fn parse(s: &str) -> Option<Mode> {
        match s.to_ascii_lowercase().as_str() {
            "interactive" => Some(Mode::Interactive),
            "plan" => Some(Mode::Plan),
            "autopilot" => Some(Mode::Autopilot),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Booting,
    Idle,
    Thinking,
    Streaming,
    Tool(String),
    Waiting(String),
    Compacting,
    Error(String),
    Offline,
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Status::Booting => "Starting".into(),
            Status::Idle => "Ready".into(),
            Status::Thinking => "Thinking".into(),
            Status::Streaming => "Writing".into(),
            Status::Tool(t) => format!("Running {t}"),
            Status::Waiting(w) => format!("Needs {w}"),
            Status::Compacting => "Compacting".into(),
            Status::Error(_) => "Error".into(),
            Status::Offline => "Offline".into(),
        }
    }
    pub fn is_busy(&self) -> bool {
        matches!(self, Status::Thinking | Status::Streaming | Status::Tool(_) | Status::Compacting | Status::Booting)
    }
    pub fn needs_attention(&self) -> bool {
        matches!(self, Status::Waiting(_))
    }
}

// ------------------------------------------------------------------ prompts

pub enum PermDecision {
    Once,
    Session,
    Reject(Option<String>),
}

pub struct PermissionPrompt {
    pub raw: Value,
    pub reply: Option<oneshot::Sender<PermDecision>>,
    pub outcome: Option<String>,
    pub feedback: String,
}

pub struct ElicitPrompt {
    pub message: String,
    pub url: Option<String>,
    pub source: Option<String>,
    pub form: forms::FormState,
    pub reply: Option<oneshot::Sender<ElicitationResult>>,
    pub outcome: Option<String>,
}

pub struct AskPrompt {
    pub question: String,
    pub choices: Vec<String>,
    pub freeform: bool,
    pub answer: String,
    pub reply: Option<oneshot::Sender<Option<(String, bool)>>>,
    pub outcome: Option<String>,
}

pub struct PlanPrompt {
    pub summary: String,
    pub plan: Option<String>,
    pub actions: Vec<String>,
    pub recommended: String,
    pub feedback: String,
    pub reply: Option<oneshot::Sender<ExitPlanModeResult>>,
    pub outcome: Option<String>,
}

pub struct SwitchPrompt {
    pub text: String,
    pub reply: Option<oneshot::Sender<AutoModeSwitchResponse>>,
    pub outcome: Option<String>,
}

// ------------------------------------------------------------------ transcript

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToolState {
    Running,
    Ok,
    Failed,
}

pub struct ToolCard {
    pub name: String,
    pub title: Option<String>,
    pub intention: Option<String>,
    pub args: Value,
    pub mcp_server: Option<String>,
    pub state: ToolState,
    pub progress: Option<String>,
    pub output: String,
    pub result: Option<String>,
    pub error: Option<String>,
    pub files: Vec<String>,
    pub started: Instant,
    pub elapsed: Option<f32>,
    pub expanded: bool,
    pub has_app: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Ok,
    Warn,
    Error,
}

pub struct SubagentCard {
    pub name: String,
    pub description: String,
    pub done: Option<bool>,
    pub tools: u32,
    pub current: Option<String>,
}

pub enum Item {
    User { text: String, queued: bool, files: Vec<String> },
    Assistant { text: String, done: bool },
    Reasoning { text: String, done: bool, open: bool },
    Tool(Box<ToolCard>),
    Permission(PermissionPrompt),
    Elicit(Box<ElicitPrompt>),
    Ask(AskPrompt),
    Plan(PlanPrompt),
    Switch(SwitchPrompt),
    Notice { level: Level, text: String },
    Subagent(SubagentCard),
    /// Something was saved to memory from this conversation (with Undo).
    Memory { ids: Vec<crate::memory::MemoryId>, text: String, learned: bool, undone: bool },
    /// Memories that were attached to the user's last prompt.
    Recall { entries: Vec<(crate::memory::MemoryId, String)>, open: bool },
}

impl Item {
    pub fn pending(&self) -> bool {
        match self {
            Item::Permission(p) => p.reply.is_some(),
            Item::Elicit(p) => p.reply.is_some(),
            Item::Ask(p) => p.reply.is_some(),
            Item::Plan(p) => p.reply.is_some(),
            Item::Switch(p) => p.reply.is_some(),
            _ => false,
        }
    }
}

/// A line for the global COMMS log.
#[derive(Clone)]
pub struct LogLine {
    pub bay: BayId,
    pub level: Level,
    pub icon: &'static str,
    pub text: String,
}

/// Side effects the app must perform for a processed event.
pub enum Effect {
    Log(LogLine),
    OpenApp(AppRequest),
    Finished,
    /// MCP server status/tools changed; the MCP panel should refresh.
    McpChanged,
}

pub struct AppRequest {
    pub tool_call_id: String,
    pub server: Option<String>,
    pub tool: String,
    /// Human tool title (e.g. "Get Time"), used for the dock tab.
    pub label: Option<String>,
    pub args: Value,
    pub result: Value,
    pub resource: Value,
}

pub struct AgentView {
    pub bay: BayId,
    pub session_id: Option<String>,
    pub cwd: PathBuf,
    pub title: Option<String>,
    pub model: Option<String>,
    pub mode: Mode,
    pub status: Status,
    pub intent: Option<String>,
    pub items: Vec<Item>,
    index: HashMap<String, usize>,
    intents: HashMap<String, String>,
    sub_by_agent: HashMap<String, usize>,
    local_echo: VecDeque<String>,
    pub seen: HashSet<String>,
    pub composer: String,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub context: Option<(i64, i64)>,
    pub tool_count: u32,
    pub turns: u32,
    pub files: Vec<String>,
    pub busy_since: Option<Instant>,
    pub activity: f32,
    pub history: Vec<f32>,
    pub auto_approve: Arc<AtomicBool>,
    pub heights: Vec<f32>,
    pub layout_width: f32,
    pub dock_open: bool,
    pub dock_tab: usize,
    pub mission: Option<u64>,
    pub subagents_active: u32,
    pub focus_composer: bool,
    pub history_idx: Option<usize>,
    pub sent_history: Vec<String>,
    pub canvases: Vec<crate::canvas::CanvasInstance>,
    pub apps: Vec<AppTab>,
    /// Files and folders attached to the next message.
    pub attachments: Vec<PathBuf>,
    /// Everything attached so far; shared with the permission handler.
    pub granted: Arc<parking_lot::Mutex<Vec<PathBuf>>>,
    /// Never reads or writes Graysense memory.
    pub incognito: bool,
    /// Project this agent works in (memory scope).
    pub project: crate::memory::Project,
    /// Memories given to this agent when it started (id, text at the time).
    pub memory_used: Vec<(crate::memory::MemoryId, String)>,
    /// Transcript items the memory curator has already read.
    pub learn_cursor: usize,
    /// When to run the curator next (debounced after a turn finishes).
    pub learn_due: Option<Instant>,
    /// Command approvals (true) and denials (false) for approval-rule learning.
    pub approval_log: Vec<(Vec<String>, bool)>,
    /// A new session whose folder and Copilot session are created with the first message
    /// (so the folder can be named after it, and unused sessions leave nothing behind).
    pub deferred: bool,
    /// Name the user gave this session.
    pub name: Option<String>,
    /// Text in the header's rename box while it's open.
    pub rename_draft: Option<String>,
    /// Jump the transcript to the latest message on the next frame.
    pub scroll_to_end: bool,
    /// The Files tab's tree state.
    pub files_state: files::FilesState,
}

/// An MCP App view docked in an agent bay.
pub struct AppTab {
    pub view: u64,
    pub title: String,
}

fn s(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(str::to_owned)
}

fn trunc(text: &str, n: usize) -> String {
    let t = text.trim().replace('\n', " ");
    if t.chars().count() > n {
        format!("{}…", t.chars().take(n).collect::<String>())
    } else {
        t
    }
}

/// Short, human summary of a tool call target (file name, command, pattern…).
pub fn tool_target(name: &str, args: &Value) -> Option<String> {
    let file = |p: &str| std::path::Path::new(p).file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| p.to_owned());
    match name {
        "view" | "edit" | "create" | "read_file" | "write_file" => s(args, "path").map(|p| file(&p)),
        "powershell" | "bash" | "shell" => s(args, "command").map(|c| trunc(c.lines().next().unwrap_or(""), 60)),
        "grep" | "rg" | "glob" => s(args, "pattern").map(|p| trunc(&p, 48)),
        "web_fetch" => s(args, "url"),
        "task" => s(args, "description"),
        "skill" => s(args, "skill"),
        "open_canvas" => args.get("input").and_then(|i| s(i, "title")).or_else(|| s(args, "instanceId")),
        "invoke_canvas_action" => s(args, "actionName"),
        "list_canvas_capabilities" => s(args, "canvasId"),
        "graysense_settings" => args.get("changes").and_then(|c| c.as_object()).map(|c| trunc(&c.keys().map(String::as_str).collect::<Vec<_>>().join(", "), 60)),
        "graysense_mcp_servers" | "graysense_plugins" => match s(args, "action").as_deref() {
            Some("list") | None => None,
            _ => Some(trunc(&[s(args, "action"), s(args, "name").or_else(|| s(args, "source"))].into_iter().flatten().collect::<Vec<_>>().join(" "), 60)),
        },
        _ => args
            .as_object()
            .and_then(|o| o.values().find_map(|v| v.as_str().map(|x| trunc(x, 48)))),
    }
}

pub fn tool_icon(name: &str) -> &'static str {
    match name {
        "view" | "read_file" => "◉",
        "edit" | "apply_patch" | "write_file" => "✎",
        "create" => "✚",
        "powershell" | "bash" | "shell" | "read_powershell" | "write_powershell" => "⌘",
        "grep" | "rg" | "glob" => "⌕",
        "web_fetch" | "web_search" => "◍",
        "task" | "read_agent" | "write_agent" => "⧉",
        "sql" => "▤",
        "report_intent" => "◈",
        "open_canvas" | "invoke_canvas_action" | "list_canvas_capabilities" => "▦",
        _ => "◆",
    }
}

/// Human label for a tool, used when the agent gave no meaningful intent.
pub fn tool_verb(name: &str) -> String {
    match name {
        "view" | "read_file" => "Read".into(),
        "edit" => "Edit".into(),
        "create" | "write_file" => "Create file".into(),
        "apply_patch" => "Apply patch".into(),
        "glob" => "Find files".into(),
        "grep" | "rg" => "Search".into(),
        "powershell" | "bash" | "shell" => "Run command".into(),
        "read_powershell" => "Read output".into(),
        "write_powershell" => "Send input".into(),
        "stop_powershell" => "Stop command".into(),
        "web_fetch" => "Fetch".into(),
        "web_search" => "Web search".into(),
        "task" => "Delegate to subagent".into(),
        "skill" => "Use skill".into(),
        "sql" => "Query".into(),
        "store_memory" => "Remember".into(),
        "graysense_add_mission" => "Add mission".into(),
        "graysense_notify" => "Notify".into(),
        "graysense_settings" => "Change settings".into(),
        "graysense_mcp_servers" => "Manage MCP servers".into(),
        "graysense_plugins" => "Manage plugins".into(),
        "graysense_recall" => "Recall".into(),
        "open_canvas" => "Open canvas".into(),
        "invoke_canvas_action" => "Canvas action".into(),
        "list_canvas_capabilities" => "Inspect canvas".into(),
        other => {
            let mut s = other.replace(['_', '-'], " ");
            if let Some(f) = s.get_mut(0..1) {
                f.make_ascii_uppercase();
            }
            s
        }
    }
}

/// Title for a tool card: the agent's intent when it is meaningful, otherwise a verb.
pub fn tool_label(card: &ToolCard, target: Option<&str>) -> String {
    if card.name == "ask_user" {
        return if card.state == ToolState::Running { "Waiting for your answer".into() } else { "Asked you".into() };
    }
    if card.name == "graysense_settings" && card.args.get("action").and_then(|a| a.as_str()) != Some("set") {
        return "Read Graysense settings".into();
    }
    if card.args.get("action").and_then(|a| a.as_str()) == Some("list") {
        match card.name.as_str() {
            "graysense_mcp_servers" => return "List MCP servers".into(),
            "graysense_plugins" => return "List plugins".into(),
            _ => {}
        }
    }
    let meaningful = |s: &str| s.trim().chars().filter(|c| c.is_alphanumeric()).count() >= 3 && Some(s.trim()) != target;
    let pick = card.intention.as_deref().filter(|s| meaningful(s)).or(card.title.as_deref().filter(|s| meaningful(s)));
    match pick {
        Some(s) => {
            let s = s.trim().trim_end_matches('.');
            let mut c = s.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
        None => tool_verb(&card.name),
    }
}

impl AgentView {
    pub fn new(bay: BayId, cwd: PathBuf, mode: Mode) -> Self {
        let project = crate::memory::Project::for_dir(&cwd);
        AgentView {
            bay,
            session_id: None,
            cwd,
            title: None,
            model: None,
            mode,
            status: Status::Booting,
            intent: None,
            items: Vec::new(),
            index: HashMap::new(),
            intents: HashMap::new(),
            sub_by_agent: HashMap::new(),
            local_echo: VecDeque::new(),
            seen: HashSet::new(),
            composer: String::new(),
            tokens_in: 0,
            tokens_out: 0,
            context: None,
            tool_count: 0,
            turns: 0,
            files: Vec::new(),
            busy_since: None,
            activity: 0.0,
            history: vec![0.0; 48],
            auto_approve: Arc::new(AtomicBool::new(false)),
            heights: Vec::new(),
            layout_width: 0.0,
            dock_open: false,
            dock_tab: 0,
            mission: None,
            subagents_active: 0,
            focus_composer: true,
            history_idx: None,
            sent_history: Vec::new(),
            canvases: Vec::new(),
            apps: Vec::new(),
            attachments: Vec::new(),
            granted: Default::default(),
            incognito: false,
            project,
            memory_used: Vec::new(),
            learn_cursor: 0,
            learn_due: None,
            approval_log: Vec::new(),
            deferred: false,
            name: None,
            rename_draft: None,
            scroll_to_end: false,
            files_state: Default::default(),
        }
    }

    pub fn display_title(&self) -> String {
        if let Some(n) = self.name.as_deref().filter(|n| !n.trim().is_empty()) {
            return n.trim().to_owned();
        }
        match self.title.as_deref().filter(|t| !t.trim().is_empty()) {
            Some(t) => crate::sessions::short_title(t),
            None if self.deferred || self.session_id.is_none() && self.items.is_empty() => "New session".to_owned(),
            None => crate::settings::dir_name(&self.cwd),
        }
    }

    pub fn push(&mut self, item: Item) -> usize {
        self.items.push(item);
        self.items.len() - 1
    }

    pub fn notice(&mut self, level: Level, text: impl Into<String>) {
        self.push(Item::Notice { level, text: text.into() });
    }

    /// Echo a locally-sent prompt immediately; the runtime's `user.message` is de-duplicated.
    pub fn echo_user(&mut self, text: &str) {
        self.echo_user_with(text, Vec::new());
    }

    pub fn echo_user_with(&mut self, text: &str, files: Vec<String>) {
        let queued = self.status.is_busy();
        self.local_echo.push_back(text.trim().to_owned());
        self.push(Item::User { text: text.to_owned(), queued, files });
        if !self.status.is_busy() {
            self.status = Status::Thinking;
            self.busy_since = Some(Instant::now());
        }
        self.sent_history.push(text.to_owned());
        self.history_idx = None;
    }

    pub fn pending_prompts(&self) -> usize {
        self.items.iter().filter(|i| i.pending()).count()
    }

    /// Plain-text excerpt of the conversation since `from`, for the memory curator. Only what the
    /// user said, the agent's replies, tool names, approvals and answers — never tool output, so
    /// text from files or web pages can't steer what gets remembered. `None` if there's nothing
    /// the user said worth reading.
    pub fn transcript_since(&self, from: usize) -> Option<String> {
        let mut s = String::new();
        let mut user_chars = 0;
        for item in self.items.iter().skip(from) {
            match item {
                Item::User { text, .. } => {
                    user_chars += text.trim().len();
                    s.push_str(&format!("USER: {}\n", trunc(text, 2_000)));
                }
                Item::Assistant { text, done: true } if !text.trim().is_empty() => s.push_str(&format!("AGENT: {}\n", trunc(text, 700))),
                Item::Tool(c) => {
                    let target = tool_target(&c.name, &c.args).map(|t| format!(" {t}")).unwrap_or_default();
                    let state = match c.state {
                        ToolState::Ok => "ok",
                        ToolState::Failed => "failed",
                        ToolState::Running => "running",
                    };
                    s.push_str(&format!("TOOL: {}{target} ({state})\n", tool_verb(&c.name)));
                }
                Item::Permission(p) => {
                    let pr = p.raw.get("promptRequest");
                    let what = pr
                        .and_then(|r| r.get("fullCommandText").or_else(|| r.get("intention")).or_else(|| r.get("fileName")).or_else(|| r.get("url")))
                        .and_then(|v| v.as_str())
                        .unwrap_or("a tool");
                    if let Some(outcome) = &p.outcome {
                        let fb = if p.feedback.trim().is_empty() { String::new() } else { format!(" — user said: {}", trunc(&p.feedback, 300)) };
                        s.push_str(&format!("APPROVAL: {outcome} {}{fb}\n", trunc(what, 160)));
                    }
                }
                Item::Ask(a) => {
                    if let Some(answer) = &a.outcome {
                        s.push_str(&format!("AGENT ASKED: {} — USER ANSWERED: {}\n", trunc(&a.question, 300), trunc(answer, 300)));
                    }
                }
                Item::Elicit(e) => {
                    if let Some(answer) = &e.outcome {
                        s.push_str(&format!("AGENT ASKED: {} — USER ANSWERED: {}\n", trunc(&e.message, 300), trunc(answer, 300)));
                    }
                }
                Item::Plan(p) => {
                    if let Some(o) = &p.outcome {
                        s.push_str(&format!("PLAN: {o}\n"));
                    }
                }
                _ => {}
            }
        }
        if user_chars < 8 {
            return None;
        }
        // Keep the most recent part if a long session piled up.
        if s.len() > 14_000 {
            let cut = s.len() - 14_000;
            let cut = (cut..s.len()).find(|i| s.is_char_boundary(*i)).unwrap_or(cut);
            s = format!("…\n{}", &s[cut..]);
        }
        Some(s)
    }

    fn log(&self, level: Level, icon: &'static str, text: impl Into<String>) -> Effect {
        Effect::Log(LogLine { bay: self.bay, level, icon, text: text.into() })
    }

    fn bump(&mut self) {
        self.activity = (self.activity + 0.25).min(1.0);
    }

    /// Fold one session event into the view. Returns side effects for the app.
    pub fn apply(&mut self, ev: &SessionEvent) -> Vec<Effect> {
        let mut fx = Vec::new();
        if !ev.id.is_empty() && !self.seen.insert(ev.id.clone()) {
            return fx;
        }
        let d = &ev.data;
        self.bump();

        // Sub-agent internals are summarised on their parent card.
        if let Some(agent) = ev.agent_id.as_deref() {
            match ev.event_type.as_str() {
                "subagent.started" | "subagent.completed" | "subagent.failed" | "subagent.selected" | "subagent.configured" | "subagent.deselected" => {}
                "tool.execution_start" => {
                    if let Some(&i) = self.sub_by_agent.get(agent)
                        && let Some(Item::Subagent(card)) = self.items.get_mut(i) {
                            card.tools += 1;
                            let name = s(d, "toolName").unwrap_or_default();
                            let args = d.get("arguments").cloned().unwrap_or(Value::Null);
                            card.current = Some(match tool_target(&name, &args) {
                                Some(t) => format!("{name} · {t}"),
                                None => name,
                            });
                        }
                    return fx;
                }
                _ => return fx,
            }
        }

        match ev.event_type.as_str() {
            "session.start" | "session.resume" => {
                if let Some(cwd) = d.pointer("/context/cwd").and_then(|v| v.as_str()) {
                    self.cwd = PathBuf::from(cwd);
                }
                if self.status == Status::Booting {
                    self.status = Status::Idle;
                }
            }
            "user.message" => {
                let source = s(d, "source").unwrap_or_default();
                let content = s(d, "content").unwrap_or_default();
                if content.trim().is_empty() || source == "memory" || source == "system" || source.starts_with("agent-") {
                    return fx;
                }
                if self.local_echo.front().is_some_and(|e| e == content.trim()) {
                    self.local_echo.pop_front();
                    if let Some(Item::User { queued, .. }) = self.items.iter_mut().rev().find(|i| matches!(i, Item::User { text, .. } if text.trim() == content.trim())) {
                        *queued = false;
                    }
                } else {
                    let files = d
                        .get("attachments")
                        .and_then(|a| a.as_array())
                        .map(|list| {
                            list.iter()
                                .filter_map(|a| s(a, "displayName").or_else(|| s(a, "path").map(|p| crate::settings::dir_name(std::path::Path::new(&p)))))
                                .collect()
                        })
                        .unwrap_or_default();
                    self.push(Item::User { text: content.clone(), queued: false, files });
                }
                fx.push(self.log(Level::Info, "▸", format!("You: {}", trunc(&content, 70))));
                self.status = Status::Thinking;
                self.busy_since.get_or_insert_with(Instant::now);
            }
            "assistant.turn_start" => {
                self.turns += 1;
                if !matches!(self.status, Status::Tool(_)) {
                    self.status = Status::Thinking;
                }
                self.busy_since.get_or_insert_with(Instant::now);
            }
            "assistant.intent" => {
                if let Some(intent) = s(d, "intent") {
                    fx.push(self.log(Level::Info, "◈", intent.clone()));
                    self.intent = Some(intent);
                }
            }
            "assistant.reasoning_delta" => {
                let id = format!("rsn:{}", s(d, "reasoningId").unwrap_or_default());
                let delta = s(d, "deltaContent").unwrap_or_default();
                match self.index.get(&id).copied() {
                    Some(i) => {
                        if let Some(Item::Reasoning { text, .. }) = self.items.get_mut(i) {
                            text.push_str(&delta);
                        }
                    }
                    None => {
                        let i = self.push(Item::Reasoning { text: delta, done: false, open: false });
                        self.index.insert(id, i);
                    }
                }
                self.status = Status::Thinking;
            }
            "assistant.reasoning" => {
                let id = format!("rsn:{}", s(d, "reasoningId").unwrap_or_default());
                let content = s(d, "content").unwrap_or_default();
                match self.index.get(&id).copied() {
                    Some(i) => {
                        if let Some(Item::Reasoning { text, done, .. }) = self.items.get_mut(i) {
                            if !content.is_empty() {
                                *text = content;
                            }
                            *done = true;
                        }
                    }
                    None if !content.trim().is_empty() => {
                        let i = self.push(Item::Reasoning { text: content, done: true, open: false });
                        self.index.insert(id, i);
                    }
                    None => {}
                }
            }
            "assistant.message_delta" => {
                if d.get("parentToolCallId").is_some_and(|v| !v.is_null()) {
                    return fx;
                }
                let id = format!("msg:{}", s(d, "messageId").unwrap_or_default());
                let delta = s(d, "deltaContent").unwrap_or_default();
                match self.index.get(&id).copied() {
                    Some(i) => {
                        if let Some(Item::Assistant { text, .. }) = self.items.get_mut(i) {
                            text.push_str(&delta);
                        }
                    }
                    None => {
                        let i = self.push(Item::Assistant { text: delta, done: false });
                        self.index.insert(id, i);
                    }
                }
                self.status = Status::Streaming;
            }
            "assistant.message" => {
                if d.get("parentToolCallId").is_some_and(|v| !v.is_null()) {
                    return fx;
                }
                if let Some(reqs) = d.get("toolRequests").and_then(|v| v.as_array()) {
                    for r in reqs {
                        if let (Some(id), Some(intent)) = (s(r, "toolCallId"), s(r, "intentionSummary")) {
                            self.intents.insert(id, intent);
                        }
                    }
                }
                let id = format!("msg:{}", s(d, "messageId").unwrap_or_default());
                let content = s(d, "content").unwrap_or_default();
                match self.index.get(&id).copied() {
                    Some(i) => {
                        if let Some(Item::Assistant { text, done }) = self.items.get_mut(i) {
                            if !content.is_empty() {
                                *text = content.clone();
                            }
                            *done = true;
                        }
                    }
                    None if !content.trim().is_empty() => {
                        let i = self.push(Item::Assistant { text: content.clone(), done: true });
                        self.index.insert(id, i);
                    }
                    None => {}
                }
                if !content.trim().is_empty() {
                    fx.push(self.log(Level::Info, "◇", trunc(&content, 80)));
                }
            }
            "assistant.usage" => {
                self.tokens_in += d.get("inputTokens").and_then(|v| v.as_u64()).unwrap_or(0);
                self.tokens_out += d.get("outputTokens").and_then(|v| v.as_u64()).unwrap_or(0);
                if self.model.is_none() {
                    self.model = s(d, "model");
                }
            }
            "session.usage_info" => {
                let cur = d.get("currentTokens").and_then(|v| v.as_i64()).unwrap_or(0);
                let lim = d.get("tokenLimit").and_then(|v| v.as_i64()).unwrap_or(0);
                if lim > 0 {
                    self.context = Some((cur, lim));
                }
            }
            "tool.execution_start" => {
                let id = s(d, "toolCallId").unwrap_or_default();
                let name = s(d, "toolName").unwrap_or_else(|| "tool".into());
                if name == "report_intent" {
                    return fx;
                }
                let args = d.get("arguments").cloned().unwrap_or(Value::Null);
                let intention = self.intents.get(&id).cloned();
                let title = s(d, "toolTitle");
                if matches!(name.as_str(), "edit" | "create" | "apply_patch")
                    && let Some(p) = s(&args, "path")
                        && !self.files.contains(&p) {
                            self.files.push(p);
                        }
                let target = tool_target(&name, &args);
                self.tool_count += 1;
                self.status = Status::Tool(tool_verb(&name).to_lowercase());
                let card = ToolCard {
                    name: name.clone(),
                    title,
                    intention,
                    args,
                    mcp_server: s(d, "mcpServerName"),
                    state: ToolState::Running,
                    progress: None,
                    output: String::new(),
                    result: None,
                    error: None,
                    files: Vec::new(),
                    started: Instant::now(),
                    elapsed: None,
                    expanded: false,
                    has_app: false,
                };
                let label = tool_label(&card, target.as_deref());
                fx.push(self.log(Level::Info, tool_icon(&name), match &target {
                    Some(t) => format!("{label} · {t}"),
                    None => label,
                }));
                let i = self.push(Item::Tool(Box::new(card)));
                self.index.insert(format!("tool:{id}"), i);
            }
            "tool.execution_partial_result" | "tool.shell_output" => {
                let id = format!("tool:{}", s(d, "toolCallId").unwrap_or_default());
                let chunk = s(d, "partialOutput").or_else(|| s(d, "output")).unwrap_or_default();
                if let Some(Item::Tool(card)) = self.index.get(&id).copied().and_then(|i| self.items.get_mut(i)) {
                    card.output.push_str(&chunk);
                    if card.output.len() > 96_000 {
                        let cut = card.output.len() - 64_000;
                        let cut = (cut..card.output.len()).find(|i| card.output.is_char_boundary(*i)).unwrap_or(cut);
                        card.output.drain(..cut);
                    }
                }
            }
            "tool.execution_progress" => {
                let id = format!("tool:{}", s(d, "toolCallId").unwrap_or_default());
                if let Some(Item::Tool(card)) = self.index.get(&id).copied().and_then(|i| self.items.get_mut(i)) {
                    card.progress = s(d, "progressMessage");
                }
            }
            "session.mcp_server_status_changed" | "session.mcp_servers_loaded" | "session.mcp_server_removed" | "session.mcp_server_tools_loaded" => {
                fx.push(Effect::McpChanged);
            }
            "tool.execution_complete" => {
                let tid = s(d, "toolCallId").unwrap_or_default();
                let id = format!("tool:{tid}");
                let ok = d.get("success").and_then(|v| v.as_bool()).unwrap_or(true);
                let result = d.get("result").cloned().unwrap_or(Value::Null);
                let ui_resource = result.get("uiResource").cloned().filter(|v| !v.is_null());
                if let Some(Item::Tool(card)) = self.index.get(&id).copied().and_then(|i| self.items.get_mut(i)) {
                    card.state = if ok { ToolState::Ok } else { ToolState::Failed };
                    card.elapsed = Some(card.started.elapsed().as_secs_f32());
                    card.result = s(&result, "detailedContent").or_else(|| s(&result, "content"));
                    card.error = d.pointer("/error/message").and_then(|v| v.as_str()).map(str::to_owned);
                    if let Some(edits) = d.get("fileEdits").and_then(|v| v.as_array()) {
                        card.files = edits.iter().filter_map(|e| s(e, "path")).collect();
                    }
                    if let Some(res) = ui_resource {
                        card.has_app = true;
                        fx.push(Effect::OpenApp(AppRequest {
                            tool_call_id: tid.clone(),
                            server: card.mcp_server.clone(),
                            tool: card.name.clone(),
                            label: card.title.clone(),
                            args: card.args.clone(),
                            result: result.clone(),
                            resource: res,
                        }));
                    }
                    if !ok {
                        let msg = card.error.clone().unwrap_or_else(|| "failed".into());
                        let name = card.name.clone();
                        fx.push(self.log(Level::Warn, "✗", format!("{name} failed · {}", trunc(&msg, 60))));
                    }
                }
                if matches!(self.status, Status::Tool(_)) {
                    self.status = Status::Thinking;
                }
            }
            "subagent.started" => {
                let card = SubagentCard {
                    name: s(d, "agentDisplayName").or_else(|| s(d, "agentName")).unwrap_or_else(|| "subagent".into()),
                    description: s(d, "agentDescription").unwrap_or_default(),
                    done: None,
                    tools: 0,
                    current: None,
                };
                fx.push(self.log(Level::Info, "⧉", format!("Started subagent {}", card.name)));
                let i = self.push(Item::Subagent(card));
                if let Some(agent) = ev.agent_id.clone() {
                    self.sub_by_agent.insert(agent, i);
                }
                if let Some(tc) = s(d, "toolCallId") {
                    self.index.insert(format!("sub:{tc}"), i);
                }
                self.subagents_active += 1;
            }
            "subagent.completed" | "subagent.failed" => {
                let ok = ev.event_type == "subagent.completed" && !d.get("cancelled").and_then(|v| v.as_bool()).unwrap_or(false);
                if let Some(i) = s(d, "toolCallId").and_then(|tc| self.index.get(&format!("sub:{tc}")).copied())
                    && let Some(Item::Subagent(card)) = self.items.get_mut(i) {
                        card.done = Some(ok);
                        card.current = None;
                        if let Some(n) = d.get("totalToolCalls").and_then(|v| v.as_u64()) {
                            card.tools = n as u32;
                        }
                    }
                self.subagents_active = self.subagents_active.saturating_sub(1);
            }
            "session.idle" => {
                let aborted = d.get("aborted").and_then(|v| v.as_bool()).unwrap_or(false);
                for item in self.items.iter_mut() {
                    match item {
                        Item::Assistant { done, .. } | Item::Reasoning { done, .. } => *done = true,
                        Item::Tool(card) if card.state == ToolState::Running => {
                            card.state = if aborted { ToolState::Failed } else { ToolState::Ok };
                        }
                        Item::User { queued, .. } => *queued = false,
                        _ => {}
                    }
                }
                let was_busy = self.busy_since.take();
                self.status = Status::Idle;
                self.intent = None;
                if let Some(start) = was_busy {
                    let secs = start.elapsed().as_secs_f32();
                    fx.push(self.log(Level::Ok, "✓", if aborted { format!("Stopped after {secs:.0}s") } else { format!("Finished in {secs:.0}s") }));
                    if !aborted {
                        fx.push(Effect::Finished);
                    }
                }
            }
            "abort" => {
                self.notice(Level::Warn, "Turn aborted");
            }
            "session.error" => {
                let msg = s(d, "message").unwrap_or_else(|| "unknown error".into());
                self.notice(Level::Error, msg.clone());
                fx.push(self.log(Level::Error, "⚠", trunc(&msg, 80)));
                self.status = Status::Error(msg);
                self.busy_since = None;
            }
            "session.warning" => {
                if let Some(msg) = s(d, "message") {
                    self.notice(Level::Warn, msg);
                }
            }
            "session.info" => {
                if let Some(msg) = s(d, "message")
                    && s(d, "infoType").as_deref() != Some("model") {
                        self.notice(Level::Info, msg);
                    }
            }
            "session.title_changed" => self.title = s(d, "title"),
            "session.model_change" => {
                if let Some(m) = s(d, "newModel") {
                    if self.model.as_deref() != Some(m.as_str()) && self.model.is_some() {
                        fx.push(self.log(Level::Info, "◎", format!("Model switched to {m}")));
                    }
                    self.model = Some(m);
                }
            }
            "session.mode_changed" => {
                if let Some(m) = s(d, "newMode").and_then(|m| Mode::parse(&m)) {
                    self.mode = m;
                }
            }
            "session.compaction_start" => self.status = Status::Compacting,
            "session.compaction_complete" => {
                self.notice(Level::Info, "Context compacted to free up space");
                self.status = Status::Thinking;
            }
            "session.task_complete" => {
                let ok = d.get("success").and_then(|v| v.as_bool()).unwrap_or(true);
                let summary = s(d, "summary").unwrap_or_else(|| "Task complete".into());
                self.notice(if ok { Level::Ok } else { Level::Warn }, summary.clone());
                fx.push(self.log(if ok { Level::Ok } else { Level::Warn }, "★", trunc(&summary, 80)));
            }
            "session.shutdown" => self.status = Status::Offline,
            _ => {}
        }
        fx
    }

    /// Recompute status after prompts are answered.
    pub fn refresh_waiting(&mut self) {
        let pending = self.pending_prompts();
        if pending > 0 {
            if !self.status.needs_attention() {
                self.status = Status::Waiting("approval".into());
            }
        } else if self.status.needs_attention() {
            self.status = Status::Thinking;
        }
    }
}
