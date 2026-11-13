//! What the UI sees: serializable views of the core's state. Transcript items are sent as
//! patches (only items whose fingerprint changed), everything else as whole snapshots of the
//! domains that changed.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::Instant;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::agent::backend::{MarketInfo, MarketPlugin, McpServerInfo, McpToolInfo, ModelInfo, PluginInfo};
use crate::agent::forms::Field;
use crate::agent::{self, AgentView, BayId, Item, Level, Mode, Status, ToolState};
use crate::memory::{Kind, Memory, Origin, Proposal, Scope, State as MemState};

/// Longest tool output or result sent to the UI (the tail of output, the head of results).
const MAX_OUTPUT: usize = 24 * 1024;
/// Longest string inside tool arguments or permission requests.
const MAX_ARG: usize = 16 * 1024;

pub fn level_str(l: Level) -> &'static str {
    match l {
        Level::Info => "info",
        Level::Ok => "ok",
        Level::Warn => "warn",
        Level::Error => "error",
    }
}

pub fn mode_str(m: Mode) -> &'static str {
    match m {
        Mode::Interactive => "interactive",
        Mode::Plan => "plan",
        Mode::Autopilot => "autopilot",
    }
}

/// `s` cut to at most `max` bytes (on a character boundary), keeping the start.
pub fn clip_head(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… ({} more characters)", &s[..end], s.len() - end)
}

/// `s` cut to at most `max` bytes (on a character boundary), keeping the end.
pub fn clip_tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    format!("… ({start} earlier characters)\n{}", &s[start..])
}

/// A copy of `v` with long strings clipped, for showing arguments.
pub fn clip_value(v: &Value, max: usize) -> Value {
    match v {
        Value::String(s) if s.len() > max => Value::String(clip_head(s, max)),
        Value::Array(a) => Value::Array(a.iter().take(200).map(|x| clip_value(x, max)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), clip_value(x, max))).collect::<Map<_, _>>()),
        other => other.clone(),
    }
}

fn unix_ms_of(t: Instant) -> i64 {
    chrono::Utc::now().timestamp_millis() - t.elapsed().as_millis() as i64
}

#[derive(Serialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct StatusDto {
    pub kind: &'static str,
    pub label: String,
    pub detail: Option<String>,
    pub busy: bool,
    pub attention: bool,
}

pub fn status_dto(s: &Status) -> StatusDto {
    let (kind, detail) = match s {
        Status::Booting => ("booting", None),
        Status::Idle => ("idle", None),
        Status::Thinking => ("thinking", None),
        Status::Streaming => ("streaming", None),
        Status::Tool(t) => ("tool", Some(t.clone())),
        Status::Waiting(w) => ("waiting", Some(w.clone())),
        Status::Compacting => ("compacting", None),
        Status::Error(e) => ("error", Some(e.clone())),
        Status::Offline => ("offline", None),
    };
    StatusDto { kind, label: s.label(), detail, busy: s.is_busy(), attention: s.needs_attention() }
}

// ------------------------------------------------------------------ transcript items

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct IdText {
    pub id: u64,
    pub text: String,
}

#[derive(Serialize, Clone)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ItemDto {
    User { text: String, files: Vec<String>, took: Option<u64> },
    Assistant { text: String, done: bool },
    Reasoning { text: String, done: bool },
    Tool(Box<ToolDto>),
    Permission(Box<PermissionDto>),
    Elicit { message: String, url: Option<String>, source: Option<String>, fields: Vec<Field>, pending: bool, outcome: Option<String> },
    Ask { question: String, choices: Vec<String>, freeform: bool, pending: bool, outcome: Option<String> },
    Plan { summary: String, plan: Option<String>, actions: Vec<String>, recommended: String, pending: bool, outcome: Option<String> },
    Switch { text: String, pending: bool, outcome: Option<String> },
    Notice { level: &'static str, text: String },
    Subagent { name: String, description: String, done: Option<bool>, tools: u32, current: Option<String> },
    Memory { ids: Vec<u64>, text: String, learned: bool, undone: bool },
    Recall { entries: Vec<IdText> },
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ToolDto {
    pub name: String,
    /// What the step did, as the transcript shows it ("Read", "Run command", or the agent's intent).
    pub label: String,
    pub verb: String,
    pub target: Option<String>,
    /// Rendering family: read, edit, create, shell, search, web, task, canvas, memory, skill, sql, graysense, other.
    pub family: &'static str,
    pub state: &'static str,
    pub progress: Option<String>,
    pub output: String,
    pub result: Option<String>,
    pub error: Option<String>,
    pub files: Vec<String>,
    pub elapsed: Option<f32>,
    pub started: i64,
    pub mcp_server: Option<String>,
    pub has_app: bool,
    pub args: Value,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PermissionDto {
    pub kind: String,
    /// "Run a command", "Edit a file", …
    pub what: String,
    /// One line for the resolved state ("Run command · npm test").
    pub summary: String,
    pub headline: Option<String>,
    pub intention: Option<String>,
    pub command: Option<String>,
    pub warning: Option<String>,
    pub file_name: Option<String>,
    pub diff: Option<String>,
    pub paths: Vec<String>,
    pub url: Option<String>,
    pub server: Option<String>,
    pub tool: Option<String>,
    pub tool_description: Option<String>,
    /// Arguments as readable text (settings changes as `key → value` lines).
    pub args: Option<String>,
    pub can_session: bool,
    pub pending: bool,
    pub outcome: Option<String>,
}

pub fn tool_family(name: &str) -> &'static str {
    match name {
        "view" | "read_file" | "read_powershell" | "read_bash" => "read",
        "edit" | "apply_patch" | "write_file" => "edit",
        "create" => "create",
        "powershell" | "bash" | "shell" | "write_powershell" | "stop_powershell" | "write_bash" | "stop_bash" => "shell",
        "grep" | "rg" | "glob" | "lexical_code_search" | "semantic_code_search" => "search",
        "web_fetch" | "web_search" => "web",
        "task" | "read_agent" | "write_agent" | "list_agents" => "task",
        "open_canvas" | "invoke_canvas_action" | "list_canvas_capabilities" => "canvas",
        "store_memory" | "graysense_recall" | "graysense_remember" => "memory",
        "skill" => "skill",
        "sql" => "sql",
        n if n.starts_with("graysense_") => "graysense",
        _ => "other",
    }
}

fn json_preview(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Object(o) if o.is_empty() => String::new(),
        Value::Object(o) => o
            .iter()
            .map(|(k, v)| match v {
                Value::String(s) => format!("{k}: {}", clip_head(s, 2_000)),
                other => format!("{k}: {}", clip_head(&other.to_string(), 2_000)),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        other => clip_head(&serde_json::to_string_pretty(other).unwrap_or_default(), 4_000),
    }
}

fn permission_dto(p: &agent::PermissionPrompt) -> PermissionDto {
    let pr = p.raw.get("promptRequest").or_else(|| p.raw.get("permissionRequest")).cloned().unwrap_or(Value::Null);
    let s = |k: &str| pr.get(k).and_then(|v| v.as_str()).map(str::to_owned);
    let kind = s("kind").unwrap_or_else(|| "tool".into());
    let tool = s("toolName");
    let graysense_tool = kind == "custom-tool" && tool.as_deref().is_some_and(|t| t.starts_with("graysense_"));
    let what = match kind.as_str() {
        _ if graysense_tool => "Change Graysense",
        "commands" => "Run a command",
        "write" => "Edit a file",
        "read" => "Read a file",
        "url" => "Fetch a URL",
        "mcp" => "Use an MCP tool",
        "custom-tool" => "Use a custom tool",
        "path" => "Access a path",
        "memory" => "Save a memory",
        _ => "Use a tool",
    }
    .to_owned();
    let intention = s("intention");
    let args_v = pr.get("args").cloned().unwrap_or(Value::Null);
    let summary = intention.clone().unwrap_or_else(|| match tool.as_deref() {
        Some(t) => {
            let target = agent::tool_target(t, &args_v);
            format!("{}{}", agent::tool_verb(t), target.map(|t| format!(" · {t}")).unwrap_or_default())
        }
        None => s("fullCommandText").map(|c| clip_head(c.lines().next().unwrap_or(""), 120)).unwrap_or_else(|| what.clone()),
    });
    let headline = (kind == "custom-tool").then(|| {
        let t = tool.clone().unwrap_or_default();
        match t.as_str() {
            "open_canvas" => {
                let canvas = args_v.get("canvasId").and_then(Value::as_str).unwrap_or("canvas");
                let title = args_v.get("input").and_then(|i| i.get("title")).and_then(Value::as_str);
                format!("Open {canvas} canvas{}", title.map(|t| format!(" · {t}")).unwrap_or_default())
            }
            "invoke_canvas_action" => format!("Run canvas action · {}", args_v.get("actionName").and_then(Value::as_str).unwrap_or("?")),
            "graysense_settings" => "Change Graysense settings".to_owned(),
            "graysense_mcp_servers" | "graysense_plugins" => {
                let action = args_v.get("action").and_then(Value::as_str).unwrap_or("change");
                let what = if t == "graysense_plugins" { "plugin" } else { "MCP server" };
                let name = args_v.get("name").or_else(|| args_v.get("source")).and_then(Value::as_str).unwrap_or_default();
                format!("{} {what} {name}", agent::tool_verb(action))
            }
            _ => agent::tool_verb(&t),
        }
    });
    let args = match kind.as_str() {
        "mcp" | "custom-tool" => {
            let changes = args_v.get("changes").and_then(Value::as_object).filter(|_| tool.as_deref() == Some("graysense_settings"));
            let text = match changes {
                Some(ch) => ch.iter().map(|(k, v)| format!("{k}  →  {}", if v.is_null() { "default".to_owned() } else { v.to_string() })).collect::<Vec<_>>().join("\n"),
                None => json_preview(&args_v),
            };
            (!text.is_empty()).then_some(text)
        }
        "commands" | "write" | "read" | "path" | "url" | "memory" => None,
        _ => Some(json_preview(&pr)).filter(|t| !t.is_empty()),
    };
    let paths = s("path")
        .map(|p| vec![p])
        .or_else(|| pr.get("paths").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()))
        .unwrap_or_default();
    PermissionDto {
        what,
        summary,
        headline,
        intention,
        command: s("fullCommandText").map(|c| clip_head(&c, MAX_ARG)),
        warning: s("warning"),
        file_name: s("fileName"),
        diff: s("diff").filter(|d| !d.is_empty()).map(|d| clip_head(&d, 64 * 1024)),
        paths,
        url: s("url"),
        server: s("serverName"),
        tool: s("toolTitle").or(tool),
        tool_description: s("toolDescription").filter(|_| !graysense_tool),
        args,
        can_session: crate::agent::backend::can_offer_session(&p.raw),
        pending: p.reply.is_some(),
        outcome: p.outcome.clone(),
        kind,
    }
}

pub fn item_dto(item: &Item) -> ItemDto {
    match item {
        Item::User { text, files, took } => ItemDto::User { text: text.clone(), files: files.clone(), took: *took },
        Item::Assistant { text, done } => ItemDto::Assistant { text: text.clone(), done: *done },
        Item::Reasoning { text, done, .. } => ItemDto::Reasoning { text: text.clone(), done: *done },
        Item::Tool(c) => {
            let target = agent::tool_target(&c.name, &c.args);
            ItemDto::Tool(Box::new(ToolDto {
                label: agent::tool_label(c, target.as_deref()),
                verb: agent::tool_verb(&c.name),
                family: tool_family(&c.name),
                target,
                name: c.name.clone(),
                state: match c.state {
                    ToolState::Running => "running",
                    ToolState::Ok => "ok",
                    ToolState::Failed => "failed",
                },
                progress: c.progress.clone(),
                output: clip_tail(&c.output, MAX_OUTPUT),
                result: c.result.as_deref().map(|r| clip_head(r, MAX_OUTPUT)),
                error: c.error.clone(),
                files: c.files.clone(),
                elapsed: c.elapsed,
                started: unix_ms_of(c.started),
                mcp_server: c.mcp_server.clone(),
                has_app: c.has_app,
                args: clip_value(&c.args, MAX_ARG),
            }))
        }
        Item::Permission(p) => ItemDto::Permission(Box::new(permission_dto(p))),
        Item::Elicit(e) => ItemDto::Elicit {
            message: e.message.clone(),
            url: e.url.clone(),
            source: e.source.clone(),
            fields: e.form.fields.clone(),
            pending: e.reply.is_some(),
            outcome: e.outcome.clone(),
        },
        Item::Ask(a) => ItemDto::Ask { question: a.question.clone(), choices: a.choices.clone(), freeform: a.freeform, pending: a.reply.is_some(), outcome: a.outcome.clone() },
        Item::Plan(p) => ItemDto::Plan {
            summary: p.summary.clone(),
            plan: p.plan.clone(),
            actions: if p.actions.is_empty() { vec!["interactive".into(), "autopilot".into()] } else { p.actions.clone() },
            recommended: p.recommended.clone(),
            pending: p.reply.is_some(),
            outcome: p.outcome.clone(),
        },
        Item::Switch(s) => ItemDto::Switch { text: s.text.clone(), pending: s.reply.is_some(), outcome: s.outcome.clone() },
        Item::Notice { level, text } => ItemDto::Notice { level: level_str(*level), text: text.clone() },
        Item::Subagent(s) => ItemDto::Subagent { name: s.name.clone(), description: s.description.clone(), done: s.done, tools: s.tools, current: s.current.clone() },
        Item::Memory { ids, text, learned, undone } => ItemDto::Memory { ids: ids.clone(), text: text.clone(), learned: *learned, undone: *undone },
        Item::Recall { entries, .. } => ItemDto::Recall { entries: entries.iter().map(|(id, text)| IdText { id: *id, text: text.clone() }).collect() },
    }
}

/// Hashes a string cheaply: short ones whole, long (streamed, append-only) ones by length and tail.
fn hash_str(h: &mut DefaultHasher, s: &str) {
    s.len().hash(h);
    if s.len() <= 512 {
        s.hash(h);
    } else {
        s.as_bytes()[s.len() - 64..].hash(h);
    }
}

/// Changes whenever the item's DTO would.
pub fn fingerprint(item: &Item) -> u64 {
    let mut h = DefaultHasher::new();
    std::mem::discriminant(item).hash(&mut h);
    match item {
        Item::User { text, files, took } => {
            hash_str(&mut h, text);
            files.len().hash(&mut h);
            took.hash(&mut h);
        }
        Item::Assistant { text, done } | Item::Reasoning { text, done, .. } => {
            hash_str(&mut h, text);
            done.hash(&mut h);
        }
        Item::Tool(c) => {
            (c.state as u8).hash(&mut h);
            hash_str(&mut h, &c.output);
            c.progress.hash(&mut h);
            c.result.as_ref().map(|r| r.len()).hash(&mut h);
            c.error.hash(&mut h);
            c.files.len().hash(&mut h);
            c.elapsed.map(f32::to_bits).hash(&mut h);
            c.has_app.hash(&mut h);
            c.intention.hash(&mut h);
        }
        Item::Permission(p) => {
            p.reply.is_some().hash(&mut h);
            p.outcome.hash(&mut h);
        }
        Item::Elicit(e) => {
            e.reply.is_some().hash(&mut h);
            e.outcome.hash(&mut h);
        }
        Item::Ask(a) => {
            a.reply.is_some().hash(&mut h);
            a.outcome.hash(&mut h);
        }
        Item::Plan(p) => {
            p.reply.is_some().hash(&mut h);
            p.outcome.hash(&mut h);
        }
        Item::Switch(s) => {
            s.reply.is_some().hash(&mut h);
            s.outcome.hash(&mut h);
        }
        Item::Notice { level, text } => {
            (*level as u8).hash(&mut h);
            hash_str(&mut h, text);
        }
        Item::Subagent(s) => {
            s.done.hash(&mut h);
            s.tools.hash(&mut h);
            s.current.hash(&mut h);
        }
        Item::Memory { ids, undone, text, .. } => {
            ids.hash(&mut h);
            undone.hash(&mut h);
            hash_str(&mut h, text);
        }
        Item::Recall { entries, .. } => entries.len().hash(&mut h),
    }
    h.finish()
}

/// New or changed items of one transcript since the UI last saw it.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptPatch {
    pub bay: BayId,
    /// The transcript's length now (the UI drops anything past it).
    pub len: usize,
    pub items: Vec<(usize, ItemDto)>,
}

// ------------------------------------------------------------------ bays

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CanvasDto {
    pub instance_id: String,
    pub canvas_id: String,
    pub title: String,
    pub state: Value,
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppTabDto {
    pub view: u64,
    pub title: String,
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentDto {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QueuedDto {
    pub text: String,
    pub files: Vec<String>,
    pub steer: bool,
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentDto {
    pub session_id: Option<String>,
    pub model: Option<String>,
    /// Thinking effort chosen for the session (null: the model's default).
    pub effort: Option<String>,
    pub mode: &'static str,
    pub status: StatusDto,
    pub intent: Option<String>,
    pub name: Option<String>,
    pub deferred: bool,
    pub incognito: bool,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub context: Option<(i64, i64)>,
    pub tool_count: u32,
    pub turns: u32,
    pub changed_files: Vec<String>,
    /// When the current turn started (Unix ms).
    pub busy_since: Option<i64>,
    pub mission: Option<u64>,
    pub canvases: Vec<CanvasDto>,
    pub apps: Vec<AppTabDto>,
    pub attachments: Vec<AttachmentDto>,
    pub auto_approve: bool,
    pub pending: usize,
    pub memory_used: usize,
    pub subagents_active: u32,
    pub project: String,
    /// The memory scope key of the agent's project.
    pub project_key: String,
    pub sent_history: Vec<String>,
    /// Messages sent mid-turn that the agent hasn't read yet.
    pub queued: Vec<QueuedDto>,
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BayDto {
    pub id: BayId,
    pub kind: &'static str,
    pub title: String,
    pub cwd: String,
    /// The folder as the UI labels it ("Sessions › 2026-05-02 Fix login" or "~\code\app").
    pub folder: String,
    pub agent: Option<AgentDto>,
    /// Terminal bays: the shell they run (an id from `terminal_shells`, or the default).
    pub shell: Option<String>,
}

pub fn agent_dto(v: &AgentView) -> AgentDto {
    use std::sync::atomic::Ordering;
    AgentDto {
        session_id: v.session_id.clone(),
        model: v.model.clone(),
        effort: v.effort.clone(),
        mode: mode_str(v.mode),
        status: status_dto(&v.status),
        intent: v.intent.clone(),
        name: v.name.clone(),
        deferred: v.deferred,
        incognito: v.incognito,
        tokens_in: v.tokens_in,
        tokens_out: v.tokens_out,
        context: v.context,
        tool_count: v.tool_count,
        turns: v.turns,
        changed_files: v.files.clone(),
        busy_since: v.busy_since.filter(|_| v.status.is_busy()).map(|s| s.unix_ms),
        mission: v.mission,
        canvases: v
            .canvases
            .iter()
            .map(|c| CanvasDto { instance_id: c.instance_id.clone(), canvas_id: c.canvas_id.clone(), title: c.title(), state: c.state() })
            .collect(),
        apps: v.apps.iter().map(|a| AppTabDto { view: a.view, title: a.title.clone() }).collect(),
        attachments: v
            .attachments
            .iter()
            .map(|p| AttachmentDto { path: p.display().to_string(), name: crate::settings::dir_name(p), is_dir: p.is_dir() })
            .collect(),
        auto_approve: v.auto_approve.load(Ordering::Relaxed),
        pending: v.pending_prompts(),
        memory_used: v.memory_used.len(),
        subagents_active: v.subagents_active,
        project: v.project.name.clone(),
        project_key: v.project.key.clone(),
        sent_history: v.sent_history.iter().rev().take(50).rev().cloned().collect(),
        queued: v.queued().map(|m| QueuedDto { text: m.text.clone(), files: m.files.clone(), steer: m.steer }).collect(),
    }
}

/// Hash of a serializable value, to tell whether a snapshot changed.
pub fn sig<T: Serialize>(v: &T) -> u64 {
    let mut h = DefaultHasher::new();
    serde_json::to_string(v).unwrap_or_default().hash(&mut h);
    h.finish()
}

// ------------------------------------------------------------------ other domains

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MissionDto {
    pub id: u64,
    pub title: String,
    pub notes: String,
    pub dir: Option<String>,
    pub status: &'static str,
    pub created: i64,
    pub session_id: Option<String>,
    pub from_agent: bool,
    pub bay: Option<BayId>,
}

pub fn mission_dto(m: &crate::missions::Mission) -> MissionDto {
    use crate::missions::Status;
    MissionDto {
        id: m.id,
        title: m.title.clone(),
        notes: m.notes.clone(),
        dir: m.dir.clone(),
        status: match m.status {
            Status::Idea => "idea",
            Status::Active => "active",
            Status::Review => "review",
            Status::Done => "done",
        },
        created: m.created,
        session_id: m.session_id.clone(),
        from_agent: m.from_agent,
        bay: m.bay,
    }
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PastSessionDto {
    pub id: String,
    pub title: String,
    /// RFC 3339.
    pub modified: String,
    /// Unix seconds.
    pub modified_ts: i64,
    pub pinned: bool,
    /// Started outside Graysense (the Copilot CLI or another app).
    pub external: bool,
    pub folder: Option<String>,
    /// Open in a bay.
    pub open_bay: Option<BayId>,
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionsDto {
    pub loaded: bool,
    pub items: Vec<PastSessionDto>,
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryDto {
    pub id: u64,
    pub kind: &'static str,
    pub kind_label: &'static str,
    /// Project key, or `None` for everywhere.
    pub project: Option<String>,
    pub project_name: Option<String>,
    pub text: String,
    pub state: &'static str,
    pub origin: &'static str,
    pub confidence: f32,
    pub pinned: bool,
    /// A change learning proposes: the new text, or `""` to delete it.
    pub proposal: Option<String>,
    pub created: i64,
    pub updated: i64,
    pub last_used: Option<i64>,
    pub uses: u32,
    pub reinforced: u32,
    pub sources: Vec<SourceDto>,
    pub history: Vec<RevisionDto>,
    pub commands: Vec<String>,
    pub stale: bool,
    pub user_owned: bool,
    pub archived_reason: Option<String>,
    /// Notes: their files, folders and links.
    pub attachments: Vec<NoteAttachmentDto>,
    pub votes: i32,
    pub opens: u32,
    pub last_opened: Option<i64>,
    /// Notes: how useful lately, from 0 to 1 (votes, opens and recalls, fading with time).
    pub relevance: f32,
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NoteAttachmentDto {
    /// file, folder or link.
    pub kind: &'static str,
    pub name: String,
    /// The original path, or the URL.
    pub target: String,
    /// Graysense's copy of a file.
    pub stored: Option<String>,
    pub size: Option<u64>,
    /// The file (or Graysense's copy) is still there.
    pub exists: bool,
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SourceDto {
    pub session: Option<String>,
    pub title: Option<String>,
    pub quote: Option<String>,
    pub at: i64,
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RevisionDto {
    pub text: String,
    pub at: i64,
    pub why: String,
}

pub fn kind_str(k: Kind) -> &'static str {
    match k {
        Kind::Preference => "preference",
        Kind::Fact => "fact",
        Kind::Procedure => "procedure",
        Kind::Episode => "episode",
        Kind::Rule => "rule",
        Kind::Note => "note",
    }
}

pub fn parse_kind(s: &str) -> Option<Kind> {
    match s {
        "preference" => Some(Kind::Preference),
        "fact" => Some(Kind::Fact),
        "procedure" => Some(Kind::Procedure),
        "episode" => Some(Kind::Episode),
        "note" => Some(Kind::Note),
        _ => None,
    }
}

pub fn memory_dto(m: &Memory, now: i64) -> MemoryDto {
    let project = match &m.scope {
        Scope::Global => None,
        Scope::Project(key) => Some(key.clone()),
    };
    MemoryDto {
        id: m.id,
        kind: kind_str(m.kind),
        kind_label: m.kind.label(),
        project_name: project.as_deref().map(|k| std::path::Path::new(k).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| k.to_owned())),
        project,
        text: m.text.clone(),
        state: match m.state {
            MemState::Active => "active",
            MemState::Pending => "pending",
            MemState::Archived => "archived",
        },
        origin: match m.origin {
            Origin::User => "user",
            Origin::Learned => "learned",
            Origin::Agent => "agent",
        },
        confidence: m.confidence,
        pinned: m.pinned,
        proposal: m.proposal.as_ref().map(|p| match p {
            Proposal::Update(t) => t.clone(),
            Proposal::Delete => String::new(),
        }),
        created: m.created,
        updated: m.updated,
        last_used: m.last_used,
        uses: m.uses,
        reinforced: m.reinforced,
        sources: m.sources.iter().map(|s| SourceDto { session: s.session.clone(), title: s.title.clone(), quote: s.quote.clone(), at: s.at }).collect(),
        history: m.history.iter().map(|r| RevisionDto { text: r.text.clone(), at: r.at, why: r.why.clone() }).collect(),
        commands: m.commands.clone(),
        stale: m.is_stale(now),
        user_owned: m.is_user_owned(),
        archived_reason: (m.state == MemState::Archived).then(|| {
            let (label, detail) = m.archived_reason();
            match detail {
                Some(d) => format!("{label}: {d}"),
                None => label.to_owned(),
            }
        }),
        attachments: m
            .attachments
            .iter()
            .map(|a| NoteAttachmentDto {
                kind: match a.kind {
                    crate::memory::AttachKind::File => "file",
                    crate::memory::AttachKind::Folder => "folder",
                    crate::memory::AttachKind::Link => "link",
                },
                name: a.name.clone(),
                target: a.target.clone(),
                stored: a.stored.clone(),
                size: a.size,
                exists: a.kind == crate::memory::AttachKind::Link || a.open_path().is_some(),
            })
            .collect(),
        votes: m.votes,
        opens: m.opens,
        last_opened: m.last_opened,
        relevance: if m.kind == Kind::Note { 1.0 - (-m.relevance(now) / 2.0).exp() } else { 0.0 },
    }
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MemoryStateDto {
    pub memories: Vec<MemoryDto>,
    /// When learning last ran (Unix seconds), and its error if it failed.
    pub last_run: Option<(i64, Option<String>)>,
    /// Conversations being read right now.
    pub learning: usize,
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpDto {
    pub loading: bool,
    pub servers: Option<Result<Vec<McpServerDto>, String>>,
    /// The agent whose live session reported the servers' status.
    pub live_bay: Option<BayId>,
    pub tools: Vec<(String, Result<Vec<McpToolDto>, String>)>,
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpServerDto {
    pub name: String,
    pub enabled: bool,
    pub source: String,
    pub plugin: Option<String>,
    pub config: Option<Value>,
    pub status: Option<String>,
    pub error: Option<String>,
}

impl From<&McpServerInfo> for McpServerDto {
    fn from(s: &McpServerInfo) -> Self {
        McpServerDto { name: s.name.clone(), enabled: s.enabled, source: s.source.clone(), plugin: s.plugin.clone(), config: s.config.clone(), status: s.status.clone(), error: s.error.clone() }
    }
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpToolDto {
    pub name: String,
    pub description: Option<String>,
    pub has_ui: bool,
}

impl From<&McpToolInfo> for McpToolDto {
    fn from(t: &McpToolInfo) -> Self {
        McpToolDto { name: t.name.clone(), description: t.description.clone(), has_ui: t.has_ui }
    }
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginsDto {
    pub installed: Option<Result<Vec<PluginDto>, String>>,
    pub markets: Option<Result<Vec<MarketDto>, String>>,
    pub browse: Vec<(String, Result<Vec<MarketPluginDto>, String>)>,
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginDto {
    pub name: String,
    pub marketplace: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub enabled: bool,
}

impl From<&PluginInfo> for PluginDto {
    fn from(p: &PluginInfo) -> Self {
        PluginDto { name: p.name.clone(), marketplace: p.marketplace.clone(), version: p.version.clone(), description: p.description.clone(), enabled: p.enabled }
    }
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MarketDto {
    pub name: String,
    pub source: String,
    pub is_default: bool,
}

impl From<&MarketInfo> for MarketDto {
    fn from(m: &MarketInfo) -> Self {
        MarketDto { name: m.name.clone(), source: m.source.clone(), is_default: m.is_default }
    }
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MarketPluginDto {
    pub name: String,
    pub description: Option<String>,
}

impl From<&MarketPlugin> for MarketPluginDto {
    fn from(m: &MarketPlugin) -> Self {
        MarketPluginDto { name: m.name.clone(), description: m.description.clone() }
    }
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelDto {
    pub id: String,
    pub name: String,
    pub multiplier: Option<f64>,
    pub category: Option<String>,
    pub efforts: Vec<String>,
    pub default_effort: Option<String>,
}

impl From<&ModelInfo> for ModelDto {
    fn from(m: &ModelInfo) -> Self {
        ModelDto { id: m.id.clone(), name: m.name.clone(), multiplier: m.multiplier, category: m.category.clone(), efforts: m.efforts.clone(), default_effort: m.default_effort.clone() }
    }
}

/// App-wide state: the runtime, models, focus, folders.
#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppDto {
    /// idle, connecting, ready, failed.
    pub client: &'static str,
    pub client_error: Option<String>,
    pub models: Vec<ModelDto>,
    /// The model sessions start with when none is chosen, once known.
    pub runtime_model: Option<String>,
    pub focused: Option<BayId>,
    pub maximized: bool,
    pub version: String,
    pub sessions_root: String,
    pub default_dir: String,
    pub home: String,
    pub config_dir: String,
    pub search: SearchStatusDto,
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SearchStatusDto {
    pub building: bool,
    pub built: bool,
    pub sessions: usize,
    pub files: usize,
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LogLineDto {
    pub bay: BayId,
    pub level: &'static str,
    pub text: String,
    pub at: i64,
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToastDto {
    pub id: u64,
    pub level: &'static str,
    pub text: String,
    /// A button on the toast: its label and the action it runs.
    pub action: Option<(String, Value)>,
}

#[derive(Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StatsDto {
    pub cpu: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub app_mem: u64,
}

/// One message to the UI: the domains that changed, transcript patches, and new toasts.
#[derive(Serialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UiUpdate {
    pub seq: u64,
    /// A full snapshot: the UI replaces its state instead of merging.
    pub full: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<AppDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bays: Option<Vec<BayDto>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub transcripts: Vec<TranscriptPatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missions: Option<Vec<MissionDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sessions: Option<SessionsDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryStateDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp: Option<McpDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugins: Option<PluginsDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update: Option<crate::update::UpdateStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<StatsDto>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub activity: Vec<LogLineDto>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub toasts: Vec<ToastDto>,
}

impl UiUpdate {
    pub fn is_empty(&self) -> bool {
        self.app.is_none()
            && self.settings.is_none()
            && self.bays.is_none()
            && self.transcripts.is_empty()
            && self.missions.is_none()
            && self.sessions.is_none()
            && self.memory.is_none()
            && self.mcp.is_none()
            && self.plugins.is_none()
            && self.update.is_none()
            && self.stats.is_none()
            && self.activity.is_empty()
            && self.toasts.is_empty()
    }
}

/// Byte ranges in `s` as UTF-16 offsets (what JavaScript strings index by).
pub fn utf16_ranges(s: &str, ranges: &[std::ops::Range<usize>]) -> Vec<(usize, usize)> {
    let to16 = |byte: usize| s.get(..byte.min(s.len())).map(|p| p.encode_utf16().count()).unwrap_or(0);
    ranges.iter().map(|r| (to16(r.start), to16(r.end))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipping_keeps_the_right_end_on_char_boundaries() {
        let s = "héllo wörld";
        assert!(clip_head(s, 2).starts_with('h'));
        assert!(clip_tail(s, 3).ends_with("rld"));
        assert_eq!(clip_head("short", 10), "short");
    }

    #[test]
    fn byte_ranges_become_utf16_offsets() {
        // "é" is 2 bytes but 1 UTF-16 unit; "🐟" is 4 bytes and 2 units.
        assert_eq!(utf16_ranges("é ab", &[3..5]), vec![(2, 4)]);
        assert_eq!(utf16_ranges("🐟x", &[4..5]), vec![(2, 3)]);
    }
}
