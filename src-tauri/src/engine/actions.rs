//! Everything the UI can ask the core to do, as one serializable [`Action`] (sent by the `act`
//! command). Each returns a small JSON result (a new bay's id, …) or an error to show.

use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::{BayKind, Core};
use crate::agent::backend::{AppCmd, Cmd, EffortChange, McpCmd, PluginCmd};
use crate::agent::{BayId, Item, Level, Mode, PermDecision};
use crate::memory::{Origin, Scope, State as MemoryState};
use crate::missions::Status as MissionStatus;
use crate::sessions;
use crate::settings;
use crate::util::{open_in_vscode, open_path, reveal_path};

#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Action {
    // bays
    NewSession { prompt: Option<String>, #[serde(default)] incognito: bool },
    NewAgent { dir: String, prompt: Option<String> },
    NewShell { dir: Option<String>, shell: Option<String> },
    FocusBay { bay: BayId },
    CloseBay { bay: BayId },
    MaximizeBay { bay: BayId },
    MoveBay { bay: BayId, to: usize },
    ShellTitle { bay: BayId, title: String },
    // agents
    Send { bay: BayId, text: String },
    Abort { bay: BayId },
    /// Switch model and/or thinking effort (ffort left out keeps the current one when the model offers it).
    SetModel {
        bay: BayId,
        model: String,
        #[serde(default)]
        effort: Option<String>,
    },
    SetMode { bay: BayId, mode: String },
    Rename { bay: BayId, name: String },
    Attach { bay: BayId, paths: Vec<String> },
    Detach { bay: BayId, path: String },
    SetAutoApprove { bay: BayId, on: bool },
    ClearTranscript { bay: BayId },
    Reply { bay: BayId, index: usize, reply: Reply },
    ForgetMemories { bay: BayId, index: usize },
    CloseCanvas { bay: BayId, instance: String },
    CanvasEdit { bay: BayId, instance: String, action: String, input: Value },
    CanvasSend { bay: BayId, instance: String },
    CloseApp { bay: BayId, view: u64 },
    AppCallTool { view: u64, req_id: Value, tool: String, args: Option<Value> },
    AppReadResource { view: u64, req_id: Value, uri: String },
    AppMessage { view: u64, text: String },
    LearnNow { bay: BayId },
    // files and folders
    OpenPath { path: String },
    RevealPath { path: String },
    OpenInVsCode { path: String },
    // sessions
    ResumeSession { id: String },
    RenameSession { id: String, name: String },
    PinSession { id: String, pinned: bool },
    DeleteSession { id: String },
    RefreshSessions,
    // missions
    MissionAdd { text: String },
    MissionEdit { id: u64, title: String, notes: String },
    MissionStatus { id: u64, status: String },
    MissionRemove { id: u64 },
    MissionUndoRemove,
    MissionSetDir { id: u64, dir: Option<String> },
    MissionLaunch { id: u64 },
    MissionDelegate { id: u64, bay: BayId },
    // memory
    MemoryAdd { text: String, kind: String, project: Option<String> },
    MemoryEdit { id: u64, text: String, kind: String, project: Option<String> },
    MemoryPin { id: u64, pinned: bool },
    MemoryArchive { id: u64 },
    MemoryRestore { id: u64 },
    MemoryPurge { id: u64 },
    MemoryAccept { id: u64 },
    MemoryDismiss { id: u64 },
    MemoryAcceptAll,
    MemoryClearAll,
    MemoryExport { path: String, markdown: bool },
    MemoryImport { path: String },
    // notes (the user's saved context: memories of kind Note)
    NoteAdd {
        text: String,
        #[serde(default)]
        files: Vec<String>,
        #[serde(default)]
        links: Vec<String>,
        project: Option<String>,
        session: Option<String>,
        title: Option<String>,
    },
    NoteAttach {
        id: u64,
        #[serde(default)]
        files: Vec<String>,
        #[serde(default)]
        links: Vec<String>,
    },
    NoteDetach { id: u64, index: usize },
    NoteVote { id: u64, delta: i32 },
    NoteOpen { id: u64, index: usize },
    NoteReveal { id: u64, index: usize },
    NoteTouched { id: u64 },
    NoteAsk { question: String, ids: Vec<u64> },
    // settings and panels
    SetSettings { changes: Map<String, Value> },
    SetSide { panel: Option<String> },
    // MCP servers and plugins
    McpRefresh,
    McpSave { name: String, config: Value, update: bool },
    McpRemove { name: String },
    McpSetEnabled { name: String, enabled: bool },
    McpTools { server: String },
    McpRestart { server: String },
    McpStop { server: String },
    McpStart { server: String },
    PluginsRefresh,
    PluginInstall { source: String },
    PluginUninstall { name: String },
    PluginSetEnabled { name: String, enabled: bool },
    PluginUpdate { name: String },
    PluginUpdateAll,
    MarketAdd { source: String },
    MarketRemove { name: String },
    MarketBrowse { market: String },
    // updates and search
    UpdateCheck,
    UpdateDownload,
    UpdateRestart { confirmed: bool },
    SearchRefresh,
}

/// An answer to a prompt in a transcript.
#[derive(Deserialize, Debug)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Reply {
    /// `once`, `session` or `reject`.
    Permission { decision: String, feedback: Option<String> },
    Ask { answer: String, freeform: bool },
    /// `accept` (with `content` for forms), `decline` or `cancel`.
    Elicit { action: String, content: Option<Value> },
    Plan { approved: bool, action: Option<String>, feedback: Option<String> },
    /// `yes`, `always` or `no`.
    Switch { choice: String },
}

fn mode_of(s: &str) -> Result<Mode, String> {
    Mode::parse(if s.eq_ignore_ascii_case("ask") { "interactive" } else { s }).ok_or_else(|| format!("Unknown mode “{s}”. Use ask, plan or autopilot."))
}

fn scope_of(project: Option<String>) -> Scope {
    match project.filter(|p| !p.trim().is_empty()) {
        Some(key) => Scope::Project(key),
        None => Scope::Global,
    }
}

impl Core {
    pub fn act(&mut self, action: Action) -> Result<Value, String> {
        let ok = Ok(Value::Null);
        match action {
            Action::NewSession { prompt, incognito } => {
                let prompt = prompt.filter(|p| !p.trim().is_empty());
                let bay = self.new_session(prompt, incognito);
                Ok(json!(bay))
            }
            Action::NewAgent { dir, prompt } => {
                let dir = PathBuf::from(dir);
                if !dir.is_dir() {
                    return Err(format!("{} isn't a folder.", dir.display()));
                }
                Ok(json!(self.new_agent(dir, prompt.filter(|p| !p.trim().is_empty()), None, None, false)))
            }
            Action::NewShell { dir, shell } => {
                let dir = dir.map(PathBuf::from).filter(|d| d.is_dir()).or_else(|| self.live_focused_dir()).unwrap_or_else(|| self.settings.default_dir());
                Ok(json!(self.new_shell(dir, shell)))
            }
            Action::FocusBay { bay } => {
                self.focus(bay);
                ok
            }
            Action::CloseBay { bay } => {
                self.close_bay(bay);
                ok
            }
            Action::MaximizeBay { bay } => {
                self.focus(bay);
                self.maximized = !self.maximized && self.bays.len() > 1;
                ok
            }
            Action::MoveBay { bay, to } => {
                if let Some(from) = self.bay_index(bay) {
                    let b = self.bays.remove(from);
                    let to = to.min(self.bays.len());
                    self.bays.insert(to, b);
                }
                ok
            }
            Action::ShellTitle { bay, title } => {
                if let Some(BayKind::Shell(s)) = self.bays.iter_mut().find(|b| b.id == bay).map(|b| &mut b.kind) {
                    s.title = Some(crate::terminal::shells::friendly_title(&title));
                }
                ok
            }
            Action::Send { bay, text } => self.send(bay, text).map(|_| Value::Null),
            Action::Abort { bay } => {
                if self.agent(bay).is_some_and(|v| !v.deferred) {
                    self.backend.send(Cmd::Abort { bay });
                }
                ok
            }
            Action::SetModel { bay, model, effort } => {
                let v = self.agent(bay).ok_or("That agent is closed.")?;
                // "auto" (or "") asks for the model's own default; leaving it out keeps the current level.
                let clear = matches!(effort.as_deref().map(str::trim), Some("auto" | ""));
                let wanted = if clear { None } else { effort.or_else(|| v.effort.clone()) };
                let resolved = self.effort_for(Some(&model), wanted.clone());
                let change = match &resolved {
                    Some(e) => EffortChange::Set(e.clone()),
                    // A level the new model doesn't offer mustn't linger either.
                    None if clear || wanted.is_some() => EffortChange::Clear,
                    None => EffortChange::Keep,
                };
                let v = self.agent_mut(bay).ok_or("That agent is closed.")?;
                v.model = Some(model.clone());
                v.effort = resolved;
                if !v.deferred {
                    self.backend.send(Cmd::SetModel { bay, model, effort: change });
                }
                ok
            }
            Action::SetMode { bay, mode } => {
                let mode = mode_of(&mode)?;
                let v = self.agent_mut(bay).ok_or("That agent is closed.")?;
                v.mode = mode;
                if !v.deferred {
                    self.backend.send(Cmd::SetMode { bay, mode });
                }
                ok
            }
            Action::Rename { bay, name } => {
                self.rename_bay(bay, name);
                ok
            }
            Action::Attach { bay, paths } => {
                let v = self.agent_mut(bay).ok_or("That agent is closed.")?;
                for p in paths.into_iter().map(PathBuf::from).filter(|p| p.exists()) {
                    if !v.attachments.contains(&p) {
                        v.attachments.push(p);
                    }
                }
                ok
            }
            Action::Detach { bay, path } => {
                if let Some(v) = self.agent_mut(bay) {
                    v.attachments.retain(|p| p.display().to_string() != path);
                }
                ok
            }
            Action::SetAutoApprove { bay, on } => {
                if let Some(v) = self.agent_mut(bay) {
                    v.auto_approve.store(on, std::sync::atomic::Ordering::Relaxed);
                }
                ok
            }
            Action::ClearTranscript { bay } => {
                if let Some(v) = self.agent_mut(bay) {
                    v.clear_transcript();
                }
                ok
            }
            Action::Reply { bay, index, reply } => self.reply(bay, index, reply).map(|_| Value::Null),
            Action::ForgetMemories { bay, index } => {
                let ids = match self.agent_mut(bay).and_then(|v| v.items.get_mut(index)) {
                    Some(Item::Memory { ids, undone, .. }) if !*undone => {
                        *undone = true;
                        ids.clone()
                    }
                    _ => return ok,
                };
                let mut store = self.memory.lock();
                for id in ids {
                    store.archive(id, "undone");
                }
                drop(store);
                self.toast(Level::Ok, "Forgotten");
                ok
            }
            Action::CloseCanvas { bay, instance } => {
                if let Some(v) = self.agent_mut(bay) {
                    v.canvases.retain(|c| c.instance_id != instance);
                }
                ok
            }
            Action::CanvasEdit { bay, instance, action, input } => {
                let c = self.agent_mut(bay).and_then(|v| v.canvases.iter_mut().find(|c| c.instance_id == instance)).ok_or("That canvas is closed.")?;
                c.apply(&action, &input)
            }
            Action::CanvasSend { bay, instance } => {
                let text = self.agent(bay).and_then(|v| v.canvases.iter().find(|c| c.instance_id == instance)).map(|c| c.summary_for_agent()).ok_or("That canvas is closed.")?;
                self.send(bay, text).map(|_| Value::Null)
            }
            Action::CloseApp { bay, view } => {
                self.apps.retain(|a| a.id != view);
                if let Some(v) = self.agent_mut(bay) {
                    v.apps.retain(|a| a.view != view);
                }
                ok
            }
            Action::AppCallTool { view, req_id, tool, args } => {
                let app = self.app_view(view).ok_or("That app is closed.")?;
                let (bay, server) = (app.bay, app.server.clone());
                self.backend.send(Cmd::App(AppCmd::CallTool { bay, view, req_id, server, tool, args }));
                ok
            }
            Action::AppReadResource { view, req_id, uri } => {
                let app = self.app_view(view).ok_or("That app is closed.")?;
                let (bay, server) = (app.bay, app.server.clone());
                self.backend.send(Cmd::App(AppCmd::ReadResource { bay, view, req_id, server, uri }));
                ok
            }
            Action::AppMessage { view, text } => {
                let bay = self.app_view(view).map(|a| a.bay).ok_or("That app is closed.")?;
                self.send(bay, text).map(|_| Value::Null)
            }
            Action::LearnNow { bay } => {
                if !self.settings.memory.learning() {
                    return Err("Learning is off. Turn it on in Memory settings.".into());
                }
                self.start_learning(bay);
                self.toast(Level::Info, "Reading this conversation for things to remember…");
                ok
            }
            Action::OpenPath { path } => {
                open_path(std::path::Path::new(&path));
                ok
            }
            Action::RevealPath { path } => {
                let p = PathBuf::from(&path);
                // The sessions folder is made with the first session; showing it makes it too.
                if !p.exists() && p == self.settings.sessions_root() {
                    let _ = std::fs::create_dir_all(&p);
                }
                reveal_path(&p);
                ok
            }
            Action::OpenInVsCode { path } => open_in_vscode(std::path::Path::new(&path)).map(|_| Value::Null),
            Action::ResumeSession { id } => Ok(json!(self.resume_session(id))),
            Action::RenameSession { id, name } => {
                match self.bays.iter().find(|b| matches!(&b.kind, BayKind::Agent(v) if v.session_id.as_deref() == Some(id.as_str()))).map(|b| b.id) {
                    Some(bay) => self.rename_bay(bay, name),
                    None => {
                        let name = name.trim().chars().take(100).collect::<String>();
                        self.session_index.entry(&id).name = (!name.is_empty()).then_some(name);
                    }
                }
                self.dirty.sessions = true;
                ok
            }
            Action::PinSession { id, pinned } => {
                self.session_index.entry(&id).pinned = pinned;
                self.dirty.sessions = true;
                ok
            }
            Action::DeleteSession { id } => {
                if let Some(bay) = self.bays.iter().find(|b| matches!(&b.kind, BayKind::Agent(v) if v.session_id.as_deref() == Some(id.as_str()))).map(|b| b.id) {
                    self.close_bay(bay);
                }
                self.sessions.retain(|s| s.id != id);
                self.session_index.remove(&id);
                self.backend.send(Cmd::DeleteSession { id });
                self.dirty.sessions = true;
                self.toast(Level::Ok, "Session deleted. Its files are still in its folder.");
                ok
            }
            Action::RefreshSessions => {
                self.backend.send(Cmd::ListSessions);
                ok
            }
            Action::MissionAdd { text } => {
                let (title, notes) = crate::missions::split_capture(&text);
                if title.is_empty() {
                    return Err("Write the idea first.".into());
                }
                Ok(json!(self.missions.add(&title, &notes, false)))
            }
            Action::MissionEdit { id, title, notes } => {
                self.missions.edit(id, &title, &notes);
                ok
            }
            Action::MissionStatus { id, status } => {
                let status = match status.as_str() {
                    "idea" => MissionStatus::Idea,
                    "active" => MissionStatus::Active,
                    "review" => MissionStatus::Review,
                    "done" => MissionStatus::Done,
                    other => return Err(format!("Unknown mission status “{other}”.")),
                };
                self.missions.set_status(id, status);
                ok
            }
            Action::MissionRemove { id } => {
                let index = self.missions.missions.iter().position(|m| m.id == id);
                if let (Some(index), Some(m)) = (index, self.missions.remove(id)) {
                    let title = m.title.clone();
                    self.removed_mission = Some((m, index));
                    self.toast_with(Level::Ok, format!("Deleted “{title}”"), Some(("Undo".into(), json!({ "type": "missionUndoRemove" }))));
                }
                ok
            }
            Action::MissionUndoRemove => {
                if let Some((m, index)) = self.removed_mission.take() {
                    self.missions.restore(m, index);
                }
                ok
            }
            Action::MissionSetDir { id, dir } => {
                self.missions.set_dir(id, dir.map(PathBuf::from).filter(|d| d.is_dir()));
                ok
            }
            Action::MissionLaunch { id } => {
                self.launch_mission(id);
                ok
            }
            Action::MissionDelegate { id, bay } => self.send_mission_to(bay, id).map(|_| Value::Null),
            Action::MemoryAdd { text, kind, project } => {
                let kind = crate::engine::dto::parse_kind(&kind).ok_or("Unknown memory kind.")?;
                let text = text.trim();
                if text.chars().count() < 4 {
                    return Err("Write a little more: a memory needs a few words.".into());
                }
                if crate::memory::looks_secret(text) {
                    return Err("That looks like a secret (a key, token or password). Graysense doesn't store those.".into());
                }
                let id = self.memory.lock().insert(kind, scope_of(project), text, Origin::User, MemoryState::Active, 1.0, None);
                id.map(|id| json!(id)).ok_or_else(|| "That's already remembered.".into())
            }
            Action::MemoryEdit { id, text, kind, project } => {
                let kind = crate::engine::dto::parse_kind(&kind).ok_or("Unknown memory kind.")?;
                if crate::memory::looks_secret(&text) {
                    return Err("That looks like a secret (a key, token or password). Graysense doesn't store those.".into());
                }
                self.memory.lock().edit(id, text.trim(), kind, scope_of(project));
                ok
            }
            Action::MemoryPin { id, pinned } => {
                self.memory.lock().set_pinned(id, pinned);
                ok
            }
            Action::MemoryArchive { id } => {
                self.memory.lock().archive(id, "deleted");
                self.toast_with(Level::Ok, "Memory deleted", Some(("Undo".into(), json!({ "type": "memoryRestore", "id": id }))));
                ok
            }
            Action::MemoryRestore { id } => {
                self.memory.lock().restore(id);
                ok
            }
            Action::MemoryPurge { id } => {
                self.memory.lock().purge(id);
                ok
            }
            Action::MemoryAccept { id } => {
                self.memory.lock().accept(id);
                ok
            }
            Action::MemoryDismiss { id } => {
                self.memory.lock().dismiss(id);
                ok
            }
            Action::MemoryAcceptAll => {
                let mut store = self.memory.lock();
                let ids: Vec<u64> = store.memories.iter().filter(|m| m.state == MemoryState::Pending || m.proposal.is_some()).map(|m| m.id).collect();
                for id in &ids {
                    store.accept(*id);
                }
                drop(store);
                self.toast(Level::Ok, format!("Accepted {} memor{}", ids.len(), if ids.len() == 1 { "y" } else { "ies" }));
                ok
            }
            Action::MemoryClearAll => {
                self.memory.lock().clear_all();
                self.toast(Level::Ok, "Forgot everything learned. Your notes are kept.");
                ok
            }
            Action::MemoryExport { path, markdown } => {
                let text = {
                    let store = self.memory.lock();
                    if markdown {
                        store.export_markdown(&|key: &str| std::path::Path::new(key).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| key.to_owned()))
                    } else {
                        serde_json::to_string_pretty(&store.export_json()).unwrap_or_default()
                    }
                };
                std::fs::write(&path, text).map_err(|e| format!("Couldn't export: {e}"))?;
                self.toast(Level::Ok, format!("Exported memory to {}", settings::short_path(std::path::Path::new(&path))));
                ok
            }
            Action::MemoryImport { path } => {
                let parsed = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).ok_or("That file isn't a Graysense memory export.")?;
                let n = self.memory.lock().import_json(&parsed);
                self.toast(Level::Ok, format!("Imported {n} memor{} for review", if n == 1 { "y" } else { "ies" }));
                Ok(json!(n))
            }
            Action::NoteAdd { text, files, links, project, session, title } => {
                let files: Vec<PathBuf> = files.into_iter().map(PathBuf::from).filter(|p| p.exists()).collect();
                let source = (session.is_some() || title.is_some()).then(|| crate::memory::Source { session, title, quote: None, at: crate::memory::now() });
                let id = self.memory.lock().add_note(&text, &files, &links, scope_of(project), source)?;
                self.toast_with(Level::Ok, "Saved to notes", Some(("Show".into(), json!({ "type": "revealMemory", "id": id }))));
                Ok(json!(id))
            }
            Action::NoteAttach { id, files, links } => {
                let files: Vec<PathBuf> = files.into_iter().map(PathBuf::from).filter(|p| p.exists()).collect();
                self.memory.lock().attach_to_note(id, &files, &links);
                ok
            }
            Action::NoteDetach { id, index } => {
                self.memory.lock().detach_from_note(id, index);
                ok
            }
            Action::NoteVote { id, delta } => {
                self.memory.lock().vote(id, delta);
                ok
            }
            Action::NoteOpen { id, index } | Action::NoteReveal { id, index } if self.note_attachment(id, index).is_none() => {
                let _ = (id, index);
                Err("That file or link is gone.".into())
            }
            Action::NoteOpen { id, index } => {
                let a = self.note_attachment(id, index).unwrap_or_else(|| unreachable!());
                self.memory.lock().note_opened(id);
                if a.kind == crate::memory::AttachKind::Link {
                    return Ok(json!({ "url": a.target }));
                }
                let path = a.open_path().ok_or_else(|| format!("{} isn't there anymore.", a.target))?;
                open_path(&path);
                ok
            }
            Action::NoteReveal { id, index } => {
                let a = self.note_attachment(id, index).unwrap_or_else(|| unreachable!());
                let path = a.open_path().ok_or_else(|| format!("{} isn't there anymore.", a.target))?;
                self.memory.lock().note_opened(id);
                reveal_path(&path);
                ok
            }
            Action::NoteTouched { id } => {
                self.memory.lock().note_opened(id);
                ok
            }
            Action::NoteAsk { question, ids } => self.ask_notes(question, ids).map(|bay| json!(bay)),
            Action::SetSettings { changes } => {
                let models: Vec<String> = self.models.iter().map(|m| m.id.clone()).collect();
                let before = self.settings.clone();
                let done = self.settings.apply_patch(&changes, &models)?;
                self.settings_changed(&before);
                Ok(json!(done))
            }
            Action::SetSide { panel } => {
                let mut changes = Map::new();
                changes.insert("side_panel".into(), panel.map(Value::String).unwrap_or(Value::Null));
                let before = self.settings.clone();
                self.settings.apply_patch(&changes, &[])?;
                self.settings_changed(&before);
                ok
            }
            Action::McpRefresh => {
                self.refresh_mcp();
                ok
            }
            Action::McpSave { name, config, update } => {
                self.backend.send(Cmd::Mcp(McpCmd::Add { name, config, update }));
                ok
            }
            Action::McpRemove { name } => {
                self.backend.send(Cmd::Mcp(McpCmd::Remove { name }));
                ok
            }
            Action::McpSetEnabled { name, enabled } => {
                self.backend.send(Cmd::Mcp(McpCmd::SetEnabled { name, enabled }));
                ok
            }
            Action::McpTools { server } | Action::McpRestart { server } | Action::McpStop { server } | Action::McpStart { server } if self.focused_agent_bay().is_none() => {
                let _ = server;
                Err("Open an agent first: MCP servers run inside agent sessions.".into())
            }
            Action::McpTools { server } => {
                let bay = self.focused_agent_bay().unwrap_or_default();
                self.mcp.tools.retain(|(s, _)| *s != server);
                self.dirty.mcp = true;
                self.backend.send(Cmd::Mcp(McpCmd::Tools { bay, server }));
                ok
            }
            Action::McpRestart { server } => {
                let bay = self.focused_agent_bay().unwrap_or_default();
                self.backend.send(Cmd::Mcp(McpCmd::Restart { bay, server }));
                ok
            }
            Action::McpStop { server } => {
                let bay = self.focused_agent_bay().unwrap_or_default();
                self.backend.send(Cmd::Mcp(McpCmd::Stop { bay, server }));
                ok
            }
            Action::McpStart { server } => {
                let bay = self.focused_agent_bay().unwrap_or_default();
                self.backend.send(Cmd::Mcp(McpCmd::Start { bay, server }));
                ok
            }
            Action::PluginsRefresh => self.plugin(PluginCmd::Refresh),
            Action::PluginInstall { source } => self.plugin(PluginCmd::Install { source }),
            Action::PluginUninstall { name } => self.plugin(PluginCmd::Uninstall { name }),
            Action::PluginSetEnabled { name, enabled } => self.plugin(PluginCmd::SetEnabled { name, enabled }),
            Action::PluginUpdate { name } => self.plugin(PluginCmd::Update { name }),
            Action::PluginUpdateAll => self.plugin(PluginCmd::UpdateAll),
            Action::MarketAdd { source } => self.plugin(PluginCmd::MarketAdd { source }),
            Action::MarketRemove { name } => self.plugin(PluginCmd::MarketRemove { name }),
            Action::MarketBrowse { market } => {
                self.plugins.browse.retain(|(m, _)| *m != market);
                self.dirty.plugins = true;
                self.plugin(PluginCmd::Browse { market })
            }
            Action::UpdateCheck => {
                self.updater.check();
                ok
            }
            Action::UpdateDownload => {
                self.updater.download();
                ok
            }
            Action::UpdateRestart { confirmed } => {
                let working = self.restart_to_update(confirmed)?;
                Ok(json!({ "working": working }))
            }
            Action::SearchRefresh => {
                self.backend.send(Cmd::ListSessions);
                self.refresh_search_index(true);
                ok
            }
        }
    }

    fn plugin(&mut self, cmd: PluginCmd) -> Result<Value, String> {
        self.backend.send(Cmd::Plugin(cmd));
        Ok(Value::Null)
    }

    /// The focused bay's folder, unless it's a session that hasn't got one yet.
    fn live_focused_dir(&self) -> Option<PathBuf> {
        let b = self.focused.and_then(|id| self.bays.iter().find(|b| b.id == id))?;
        match &b.kind {
            BayKind::Agent(v) if v.deferred => None,
            _ => Some(b.cwd().to_path_buf()).filter(|d| d.is_dir()),
        }
    }

    /// Send a message (with the attached files) to an agent.
    pub fn send(&mut self, bay: BayId, text: String) -> Result<(), String> {
        self.send_as(bay, text.clone(), text)
    }

    /// One of a note's attachments.
    fn note_attachment(&self, id: u64, index: usize) -> Option<crate::memory::Attachment> {
        self.memory.lock().get(id).and_then(|m| m.attachments.get(index).cloned())
    }

    /// Ask Copilot a question about saved notes: a new session gets the question, the notes
    /// (as context the bubble doesn't show) and their files.
    fn ask_notes(&mut self, question: String, ids: Vec<u64>) -> Result<BayId, String> {
        use crate::memory::Kind;
        let question = question.trim().to_owned();
        if question.is_empty() {
            return Err("Ask a question first.".into());
        }
        let (sent, files, entries) = {
            let mut store = self.memory.lock();
            let notes: Vec<crate::memory::Memory> = ids.iter().filter_map(|id| store.get(*id)).filter(|m| m.kind == Kind::Note).cloned().collect();
            store.mark_used(&ids);
            let sent = crate::memory::ask_with_notes(&question, &notes.iter().collect::<Vec<_>>());
            let files: Vec<PathBuf> = notes.iter().flat_map(|m| m.attachments.iter().filter_map(|a| a.open_path())).filter(|p| p.is_file()).take(8).collect();
            (sent, files, notes.into_iter().map(|m| (m.id, m.text)).collect::<Vec<_>>())
        };
        let bay = self.new_session(None, false);
        if let Some(v) = self.agent_mut(bay) {
            v.attachments = files;
        }
        self.send_as(bay, question, sent)?;
        if let Some(v) = self.agent_mut(bay).filter(|_| !entries.is_empty()) {
            v.memory_used.extend(entries.iter().cloned());
            v.push(Item::Recall { entries });
        }
        Ok(bay)
    }

    /// Send `sent` to an agent while its transcript shows `shown` (they differ when Graysense adds
    /// context the user doesn't need to see again).
    pub fn send_as(&mut self, bay: BayId, shown: String, sent: String) -> Result<(), String> {
        let text = shown.trim_end().to_owned();
        let sent = sent.trim_end().to_owned();
        let v = self.agent(bay).ok_or("That agent is closed.")?;
        if text.trim().is_empty() || sent.trim().is_empty() {
            return Err("Write a message first.".into());
        }
        if v.deferred {
            self.start_deferred_as(bay, text, sent);
            return Ok(());
        }
        let (enabled, memory) = (self.settings.memory.enabled, self.memory.clone());
        let mut name_session = None;
        let mut index_name = None;
        let v = self.agent_mut(bay).ok_or("That agent is closed.")?;
        let files = std::mem::take(&mut v.attachments);
        v.granted.lock().extend(files.iter().cloned());
        let names = files.iter().map(|p| settings::dir_name(p)).collect();
        // The first message names the session (until the user names it).
        if v.title.is_none() {
            v.title = Some(text.clone());
            let name = sessions::short_title(&text);
            if let Some(id) = &v.session_id {
                index_name = Some((id.clone(), name.clone()));
            }
            if v.name.is_none() {
                name_session = Some(name);
            }
        }
        v.echo_user_shown(&text, &sent, names);
        v.learn_due = None;
        super::capture_explicit_memory(&memory, enabled, v, &text);
        if let Some((id, name)) = index_name {
            self.session_index.entry(&id).auto_name = Some(name);
            self.dirty.sessions = true;
        }
        if let Some(name) = name_session {
            self.backend.send(Cmd::NameSession { bay, name, user: false });
        }
        self.backend.send(Cmd::Send { bay, text: sent, files });
        Ok(())
    }

    /// Name a session (empty = back to its automatic name).
    fn rename_bay(&mut self, bay: BayId, name: String) {
        let name = name.trim().chars().take(100).collect::<String>();
        let Some(v) = self.agent_mut(bay) else { return };
        v.name = (!name.is_empty()).then(|| name.clone());
        let auto = v.title.as_deref().map(sessions::short_title);
        let (session, deferred, user_name) = (v.session_id.clone(), v.deferred, v.name.clone());
        if let Some(sid) = &session {
            self.session_index.entry(sid).name = user_name;
            self.dirty.sessions = true;
        }
        if !deferred && session.is_some() {
            match (name.is_empty(), auto) {
                (false, _) => self.backend.send(Cmd::NameSession { bay, name, user: true }),
                (true, Some(auto)) => self.backend.send(Cmd::NameSession { bay, name: auto, user: true }),
                (true, None) => {}
            }
        }
    }

    /// Answer a prompt (approval, question, form, plan review, model switch) in a transcript.
    fn reply(&mut self, bay: BayId, index: usize, reply: Reply) -> Result<(), String> {
        let v = self.agent_mut(bay).ok_or("That agent is closed.")?;
        let mut approval = None;
        match (v.items.get_mut(index), reply) {
            (Some(Item::Permission(p)), Reply::Permission { decision, feedback }) => {
                let tx = p.reply.take().ok_or("That was already answered.")?;
                let pr = p.raw.get("promptRequest").or_else(|| p.raw.get("permissionRequest")).cloned().unwrap_or(Value::Null);
                let commands: Vec<String> = if pr.get("kind").and_then(Value::as_str) == Some("commands") {
                    pr.get("commandIdentifiers").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default()
                } else {
                    Vec::new()
                };
                let feedback = feedback.filter(|f| !f.trim().is_empty());
                let (answer, outcome, allowed) = match decision.as_str() {
                    "once" => (PermDecision::Once, "allowed", true),
                    "session" => (PermDecision::Session, "allowed for this session", true),
                    _ => (PermDecision::Reject(feedback.clone()), "denied", false),
                };
                let _ = tx.send(answer);
                p.outcome = Some(outcome.into());
                p.feedback = feedback.unwrap_or_default();
                if !commands.is_empty() {
                    approval = Some((commands, allowed));
                }
            }
            (Some(Item::Ask(a)), Reply::Ask { answer, freeform }) => {
                let tx = a.reply.take().ok_or("That was already answered.")?;
                if answer.trim().is_empty() {
                    a.reply = Some(tx);
                    return Err("Write an answer first.".into());
                }
                let _ = tx.send(Some((answer.clone(), freeform)));
                a.outcome = Some(answer);
            }
            (Some(Item::Elicit(e)), Reply::Elicit { action, content }) => {
                let tx = e.reply.take().ok_or("That was already answered.")?;
                let outcome = match (action.as_str(), &content) {
                    ("accept", Some(c)) => e.form.summary(c),
                    ("accept", None) => "completed".into(),
                    ("decline", _) => "declined".into(),
                    _ => "cancelled".into(),
                };
                let _ = tx.send(github_copilot_sdk::ElicitationResult { action: if matches!(action.as_str(), "accept" | "decline") { action } else { "cancel".into() }, content });
                e.outcome = Some(outcome);
            }
            (Some(Item::Plan(p)), Reply::Plan { approved, action, feedback }) => {
                let tx = p.reply.take().ok_or("That was already answered.")?;
                let feedback = feedback.filter(|f| !f.trim().is_empty());
                let _ = tx.send(github_copilot_sdk::handler::ExitPlanModeResult { approved, selected_action: action.clone(), feedback });
                p.outcome = Some(if approved { format!("approved ({})", action.unwrap_or_default()) } else { "revising".into() });
            }
            (Some(Item::Switch(s)), Reply::Switch { choice }) => {
                use github_copilot_sdk::handler::AutoModeSwitchResponse as R;
                let tx = s.reply.take().ok_or("That was already answered.")?;
                let (r, label) = match choice.as_str() {
                    "yes" => (R::Yes, "switched"),
                    "always" => (R::YesAlways, "always switch"),
                    _ => (R::No, "declined"),
                };
                let _ = tx.send(r);
                s.outcome = Some(label.into());
            }
            _ => return Err("That prompt is gone.".into()),
        }
        if let Some(a) = approval {
            v.approval_log.push(a);
        }
        v.refresh_waiting();
        Ok(())
    }
}
