//! Copilot SDK backend: owns the client and sessions on the tokio runtime and
//! bridges every interactive callback (permissions, forms, plans, canvases) to the UI.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use github_copilot_sdk::canvas::{CanvasError, CanvasHandler, CanvasResult};
use github_copilot_sdk::handler::{
    AutoModeSwitchHandler, AutoModeSwitchResponse, ElicitationHandler, ExitPlanModeHandler, ExitPlanModeResult,
    PermissionHandler, PermissionResult, UserInputHandler, UserInputResponse,
};
use github_copilot_sdk::rpc::{
    CanvasProviderCloseRequest, CanvasProviderInvokeActionRequest, CanvasProviderOpenRequest, CanvasProviderOpenResult,
    McpAppsCallToolRequest, McpAppsReadResourceRequest, McpConfigAddRequest, McpConfigDisableRequest,
    McpConfigEnableRequest, McpConfigRemoveRequest, McpConfigUpdateRequest, McpDiscoverRequest, McpListToolsRequest,
    McpRestartServerRequest, McpStartServerRequest, McpStopServerRequest, ModeSetRequest, ModelSwitchToRequest,
    PermissionDecision, PluginsDisableRequest, PluginsEnableRequest, PluginsInstallRequest,
    PluginsMarketplacesAddRequest, PluginsMarketplacesBrowseRequest, PluginsMarketplacesRemoveRequest,
    PluginsUninstallRequest, PluginsUpdateRequest,
};
use github_copilot_sdk::session::Session;
use github_copilot_sdk::session_events::{ContextTier, SessionMode};
use github_copilot_sdk::subscription::RecvErrorKind;
use github_copilot_sdk::tool::ToolHandler;
use github_copilot_sdk::{
    AskUserVariant, Client, ClientInfo, ClientOptions, ElicitationMode, ElicitationRequest, ElicitationResult,
    ExitPlanModeData, PermissionRequestData, RequestId, ResumeSessionConfig, SessionConfig, SessionEvent, SessionId,
    Tool, ToolInvocation, ToolResult, WorkingDirectoryTrust,
};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use super::{AskPrompt, BayId, ElicitPrompt, Mode, PermDecision, PermissionPrompt, PlanPrompt, SwitchPrompt, forms};

// ------------------------------------------------------------------ public API

#[derive(Clone)]
pub struct UiTx {
    tx: std::sync::mpsc::Sender<Evt>,
}

impl UiTx {
    pub fn new(tx: std::sync::mpsc::Sender<Evt>) -> Self {
        Self { tx }
    }
    pub fn send(&self, e: Evt) {
        let _ = self.tx.send(e);
    }
}

pub struct AgentSpec {
    pub cwd: PathBuf,
    pub model: Option<String>,
    /// Reasoning effort for a new session (None: the model's default).
    pub effort: Option<String>,
    /// Context window tier for a new session ("default" or "long_context"; None: the model's usual one).
    pub context_tier: Option<String>,
    pub mode: Mode,
    pub prompt: Option<String>,
    /// Files attached to `prompt`.
    pub files: Vec<PathBuf>,
    pub auto_approve: Arc<AtomicBool>,
    pub resume: Option<String>,
    /// Files/folders the user attached; reading them never needs another approval.
    pub granted: Arc<parking_lot::Mutex<Vec<PathBuf>>>,
    /// Memory for this agent; `None` when memory is off or the agent is incognito.
    pub memory: Option<AgentMemory>,
}

/// What a model switch does to one of the session's settings (thinking effort, context window).
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// Leave it as it is.
    Keep,
    /// Drop the override: the model's own default.
    Clear,
    Set(String),
}

/// What an agent can do with Graysense memory.
#[derive(Clone)]
pub struct AgentMemory {
    pub store: Arc<parking_lot::Mutex<crate::memory::MemoryStore>>,
    pub project: crate::memory::Project,
}

pub enum Cmd {
    Connect,
    Create { bay: BayId, spec: AgentSpec },
    /// `steer`: if the agent is mid-turn, it reads the message at its next step instead of after the turn.
    Send { bay: BayId, text: String, files: Vec<PathBuf>, steer: bool },
    Abort { bay: BayId },
    SetModel { bay: BayId, model: String, effort: Change, tier: Change },
    SetMode { bay: BayId, mode: Mode },
    Close { bay: BayId },
    ListModels,
    ListSessions,
    /// Name an open session: `user` names stick; otherwise only applied until the user names it.
    NameSession { bay: BayId, name: String, user: bool },
    /// Delete a session's history (its folder is left alone).
    DeleteSession { id: String },
    /// Run the memory curator over a conversation excerpt (already formatted).
    Learn { bay: BayId, model: Option<String>, message: String },
    Mcp(McpCmd),
    Plugin(PluginCmd),
    App(AppCmd),
    Shutdown(oneshot::Sender<()>),
    Registered { bay: BayId, session: Arc<Session>, prompt: Option<String>, files: Vec<PathBuf>, mode: Mode, resumed: bool },
    StartFailed { bay: BayId, error: String },
}

pub enum McpCmd {
    Refresh { bay: Option<BayId>, cwd: Option<String> },
    Add { name: String, config: Value, update: bool },
    Remove { name: String },
    SetEnabled { name: String, enabled: bool },
    Tools { bay: BayId, server: String },
    Restart { bay: BayId, server: String },
    Stop { bay: BayId, server: String },
    Start { bay: BayId, server: String },
}

pub enum PluginCmd {
    Refresh,
    Install { source: String },
    Uninstall { name: String },
    SetEnabled { name: String, enabled: bool },
    Update { name: String },
    UpdateAll,
    MarketAdd { source: String },
    MarketRemove { name: String },
    Browse { market: String },
}

pub enum AppCmd {
    CallTool { bay: BayId, view: u64, req_id: Value, server: String, tool: String, args: Option<Value> },
    ReadResource { bay: BayId, view: u64, req_id: Value, server: String, uri: String },
}

#[derive(Clone, Debug)]
pub enum ClientState {
    Idle,
    Connecting,
    Ready,
    Failed(String),
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub multiplier: Option<f64>,
    /// "powerful", "versatile" or "lightweight", for grouping in the picker.
    pub category: Option<String>,
    /// Reasoning effort levels the model offers (none when it has no setting).
    pub efforts: Vec<String>,
    pub default_effort: Option<String>,
    /// Context window sizes the model offers, standard first (none when it has one size).
    pub contexts: Vec<ContextSize>,
}

/// A context window size a model offers.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSize {
    /// "default" or "long_context".
    pub tier: String,
    /// Prompt tokens it holds, when the runtime says.
    pub tokens: Option<i64>,
    /// Tokens cost more than in the standard window.
    pub pricier: bool,
}

/// The context sizes a listed model offers: the standard and long windows when it has a long one.
fn context_sizes(m: &github_copilot_sdk::rpc::Model) -> Vec<ContextSize> {
    let prices = m.billing.as_ref().and_then(|b| b.token_prices.as_ref());
    let long = prices.and_then(|p| p.long_context.as_ref());
    let listed = m.supported_context_tiers.as_ref().is_some_and(|t| t.iter().any(|t| t == "long_context"));
    if long.is_none() && !listed {
        return Vec::new();
    }
    let more = |a: Option<f64>, b: Option<f64>| matches!((a, b), (Some(long), Some(std)) if long > std);
    let pricier = long.is_some_and(|l| more(l.input_price, prices.and_then(|p| p.input_price)) || more(l.output_price, prices.and_then(|p| p.output_price)));
    vec![
        ContextSize { tier: "default".into(), tokens: prices.and_then(|p| p.max_prompt_tokens), pricier: false },
        ContextSize { tier: "long_context".into(), tokens: long.and_then(|l| l.max_prompt_tokens), pricier },
    ]
}

/// The SDK's name for a context tier Graysense offers.
fn sdk_tier(tier: &str) -> Option<ContextTier> {
    match tier {
        "default" => Some(ContextTier::Default),
        "long_context" => Some(ContextTier::LongContext),
        _ => None,
    }
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    pub summary: Option<String>,
    pub modified: String,
    pub client: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerInfo {
    pub name: String,
    pub enabled: bool,
    pub source: String,
    pub plugin: Option<String>,
    pub config: Option<Value>,
    pub status: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolInfo {
    pub name: String,
    pub description: Option<String>,
    pub has_ui: bool,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInfo {
    pub name: String,
    pub marketplace: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub enabled: bool,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketInfo {
    pub name: String,
    pub source: String,
    pub is_default: bool,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketPlugin {
    pub name: String,
    pub description: Option<String>,
}

pub enum Evt {
    Client(ClientState),
    SessionReady { bay: BayId, session_id: String },
    SessionFailed { bay: BayId, error: String },
    SessionClosed { bay: BayId },
    Event { bay: BayId, ev: Box<SessionEvent> },
    History { bay: BayId, events: Vec<SessionEvent> },
    Notice { bay: BayId, error: bool, text: String },
    Toast { error: bool, text: String },
    Permission { bay: BayId, prompt: PermissionPrompt },
    Elicit { bay: BayId, prompt: Box<ElicitPrompt> },
    Ask { bay: BayId, prompt: AskPrompt },
    Plan { bay: BayId, prompt: PlanPrompt },
    Switch { bay: BayId, prompt: SwitchPrompt },
    CanvasOpen { bay: BayId, instance_id: String, canvas_id: String, input: Value },
    CanvasAction { bay: BayId, instance_id: String, action: String, input: Value, reply: oneshot::Sender<Result<Value, String>> },
    CanvasClosed { bay: BayId, instance_id: String },
    Models(Vec<ModelInfo>),
    Sessions(Vec<SessionInfo>),
    McpServers { servers: Result<Vec<McpServerInfo>, String>, live_bay: Option<BayId> },
    McpTools { server: String, tools: Result<Vec<McpToolInfo>, String> },
    Plugins(Result<Vec<PluginInfo>, String>),
    Markets(Result<Vec<MarketInfo>, String>),
    MarketPlugins { market: String, plugins: Result<Vec<MarketPlugin>, String> },
    AppReply { view: u64, req_id: Value, result: Result<Value, String> },
    AddMission { bay: BayId, title: String, notes: String },
    /// An agent reads (`changes: None`) or changes Graysense settings; the app replies.
    Config { changes: Option<serde_json::Map<String, Value>>, reply: oneshot::Sender<Result<Value, String>> },
    Notify { bay: BayId, title: String, message: String, level: String },
    /// An agent saved a memory through the runtime's `store_memory` tool.
    Remembered { bay: BayId, id: crate::memory::MemoryId, text: String },
    /// Memories attached to the prompt the user just sent (id, text).
    Recalled { bay: BayId, entries: Vec<(crate::memory::MemoryId, String)> },
    /// The curator finished reading a conversation excerpt (raw JSON text, or an error).
    Learned { bay: BayId, result: Result<String, String> },
    /// MCP servers were added, removed or switched on or off: the core refreshes their status.
    McpChanged,
}

pub struct Backend {
    tx: mpsc::UnboundedSender<Cmd>,
}

impl Backend {
    pub fn spawn(rt: &tokio::runtime::Runtime, ui: UiTx) -> Backend {
        let (tx, rx) = mpsc::unbounded_channel();
        rt.spawn(run(rx, tx.clone(), ui));
        Backend { tx }
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }

    /// Gracefully stop the runtime (bounded wait).
    pub fn shutdown(&self, rt: &tokio::runtime::Runtime) {
        let (tx, rx) = oneshot::channel();
        if self.tx.send(Cmd::Shutdown(tx)).is_ok() {
            let _ = rt.block_on(async { tokio::time::timeout(std::time::Duration::from_secs(3), rx).await });
        }
    }
}

// ------------------------------------------------------------------ actor

async fn connect(ui: &UiTx) -> Option<Client> {
    ui.send(Evt::Client(ClientState::Connecting));
    let info = ClientInfo::new().with_application_name("graysense").with_application_version(env!("CARGO_PKG_VERSION"));
    // MCP Apps (SEP-1865) is gated in the runtime; Graysense ships a renderer, so turn it on.
    let options = ClientOptions::default().with_client_info(info).with_env([("COPILOT_MCP_APPS", "true")]);
    match Client::start(options).await {
        Ok(client) => {
            tracing::info!("copilot runtime connected");
            ui.send(Evt::Client(ClientState::Ready));
            Some(client)
        }
        Err(err) => {
            tracing::error!("copilot runtime failed to start: {err}");
            ui.send(Evt::Client(ClientState::Failed(err.to_string())));
            None
        }
    }
}

fn needs_client(cmd: &Cmd) -> bool {
    !matches!(cmd, Cmd::Shutdown(_) | Cmd::Registered { .. } | Cmd::StartFailed { .. })
}

fn bay_of(cmd: &Cmd) -> Option<BayId> {
    match cmd {
        Cmd::Send { bay, .. } | Cmd::Abort { bay } | Cmd::SetModel { bay, .. } | Cmd::SetMode { bay, .. } | Cmd::Close { bay } | Cmd::NameSession { bay, .. } => Some(*bay),
        Cmd::App(AppCmd::CallTool { bay, .. }) | Cmd::App(AppCmd::ReadResource { bay, .. }) => Some(*bay),
        Cmd::Mcp(McpCmd::Tools { bay, .. }) | Cmd::Mcp(McpCmd::Restart { bay, .. }) | Cmd::Mcp(McpCmd::Stop { bay, .. }) | Cmd::Mcp(McpCmd::Start { bay, .. }) => Some(*bay),
        _ => None,
    }
}

async fn run(mut rx: mpsc::UnboundedReceiver<Cmd>, self_tx: mpsc::UnboundedSender<Cmd>, ui: UiTx) {
    let mut client: Option<Client> = None;
    let mut sessions: HashMap<BayId, Arc<Session>> = HashMap::new();
    let mut starting: HashSet<BayId> = HashSet::new();
    let mut parked: HashMap<BayId, Vec<Cmd>> = HashMap::new();
    let mut backlog: VecDeque<Cmd> = VecDeque::new();

    loop {
        let cmd = match backlog.pop_front() {
            Some(c) => c,
            None => match rx.recv().await {
                Some(c) => c,
                None => break,
            },
        };

        if client.is_none() && needs_client(&cmd) {
            client = connect(&ui).await;
            if client.is_none() {
                if let Cmd::Create { bay, .. } = cmd {
                    ui.send(Evt::SessionFailed { bay, error: "Copilot runtime unavailable".into() });
                }
                continue;
            }
        }

        // Session-scoped commands wait for their session to finish starting.
        if let Some(bay) = bay_of(&cmd)
            && !sessions.contains_key(&bay) {
                if starting.contains(&bay) {
                    parked.entry(bay).or_default().push(cmd);
                } else if !matches!(cmd, Cmd::Close { .. }) {
                    ui.send(Evt::Notice { bay, error: true, text: "This agent session is not connected.".into() });
                }
                continue;
            }

        match cmd {
            Cmd::Connect => {}
            Cmd::Shutdown(done) => {
                for (_, s) in sessions.drain() {
                    let _ = s.disconnect().await;
                }
                if let Some(c) = client.take() {
                    let _ = c.stop().await;
                }
                let _ = done.send(());
                break;
            }
            Cmd::Create { bay, spec } => {
                let Some(c) = client.clone() else { continue };
                starting.insert(bay);
                let ui = ui.clone();
                let self_tx = self_tx.clone();
                tokio::spawn(async move {
                    let prompt = spec.prompt.clone();
                    let files = spec.files.clone();
                    let mode = spec.mode;
                    let resumed = spec.resume.is_some();
                    match start_session(&c, bay, spec, ui.clone(), self_tx.clone()).await {
                        Ok(session) => {
                            let _ = self_tx.send(Cmd::Registered { bay, session: Arc::new(session), prompt, files, mode, resumed });
                        }
                        Err(error) => {
                            let _ = self_tx.send(Cmd::StartFailed { bay, error });
                        }
                    }
                });
            }
            Cmd::StartFailed { bay, error } => {
                starting.remove(&bay);
                parked.remove(&bay);
                ui.send(Evt::SessionFailed { bay, error });
            }
            Cmd::Registered { bay, session, prompt, files, mode, resumed } => {
                starting.remove(&bay);
                ui.send(Evt::SessionReady { bay, session_id: session.id().to_string() });
                if resumed {
                    let s = session.clone();
                    let ui = ui.clone();
                    tokio::spawn(async move {
                        if let Ok(events) = s.get_events().await {
                            ui.send(Evt::History { bay, events });
                        }
                    });
                }
                if mode != Mode::Interactive && !resumed {
                    let s = session.clone();
                    tokio::spawn(async move {
                        let _ = s.rpc().mode().set(ModeSetRequest { mode: to_session_mode(mode), ..Default::default() }).await;
                    });
                }
                if let Some(p) = prompt {
                    backlog.push_back(Cmd::Send { bay, text: p, files, steer: false });
                }
                sessions.insert(bay, session);
                if let Some(cmds) = parked.remove(&bay) {
                    backlog.extend(cmds);
                }
            }
            Cmd::Send { bay, text, files, steer } => {
                let s = sessions[&bay].clone();
                let ui = ui.clone();
                tokio::spawn(async move {
                    let mut opts = github_copilot_sdk::MessageOptions::new(text);
                    if steer {
                        // An idle session runs it as usual.
                        opts = opts.with_mode(github_copilot_sdk::DeliveryMode::Immediate);
                    }
                    if !files.is_empty() {
                        let attachments = files
                            .into_iter()
                            .map(|path| {
                                let display_name = path.file_name().map(|n| n.to_string_lossy().into_owned());
                                if path.is_dir() {
                                    github_copilot_sdk::Attachment::Directory { path, display_name }
                                } else {
                                    github_copilot_sdk::Attachment::File { path, display_name, line_range: None }
                                }
                            })
                            .collect();
                        opts = opts.with_attachments(attachments);
                    }
                    if let Err(e) = s.send(opts).await {
                        ui.send(Evt::Notice { bay, error: true, text: format!("Send failed: {e}") });
                    }
                });
            }
            Cmd::Abort { bay } => {
                let s = sessions[&bay].clone();
                tokio::spawn(async move {
                    let _ = s.abort().await;
                });
            }
            Cmd::SetModel { bay, model, effort, tier } => {
                let s = sessions[&bay].clone();
                let c = client.clone().unwrap();
                let ui = ui.clone();
                tokio::spawn(async move {
                    let reasoning_effort = if let Change::Set(e) = &effort { Some(e.clone()) } else { None };
                    let context_tier = if let Change::Set(t) = &tier { sdk_tier(t) } else { None };
                    let req = ModelSwitchToRequest { model_id: model.clone(), reasoning_effort, context_tier, ..Default::default() };
                    let r = if effort == Change::Clear || tier == Change::Clear {
                        // The typed request leaves out empty fields ("keep it"); clearing needs an explicit null.
                        let mut params = serde_json::to_value(&req).unwrap_or_else(|_| json!({}));
                        params["sessionId"] = json!(s.id().to_string());
                        if effort == Change::Clear {
                            params["reasoningEffort"] = Value::Null;
                        }
                        if tier == Change::Clear {
                            params["contextTier"] = Value::Null;
                        }
                        c.call("session.model.switchTo", Some(params)).await.map(|_| ())
                    } else {
                        s.rpc().model().switch_to(req).await.map(|_| ())
                    };
                    if let Err(e) = r {
                        ui.send(Evt::Notice { bay, error: true, text: format!("Couldn't switch to {model}: {e}") });
                    }
                });
            }
            Cmd::SetMode { bay, mode } => {
                let s = sessions[&bay].clone();
                let ui = ui.clone();
                tokio::spawn(async move {
                    if let Err(e) = s.rpc().mode().set(ModeSetRequest { mode: to_session_mode(mode), ..Default::default() }).await {
                        ui.send(Evt::Notice { bay, error: true, text: format!("Couldn't change mode: {e}") });
                    }
                });
            }
            Cmd::Close { bay } => {
                if let Some(s) = sessions.remove(&bay) {
                    tokio::spawn(async move {
                        let _ = s.disconnect().await;
                    });
                }
            }
            Cmd::NameSession { bay, name, user } => {
                let s = sessions[&bay].clone();
                tokio::spawn(async move {
                    let r = if user {
                        s.rpc().name().set(github_copilot_sdk::rpc::NameSetRequest { name }).await
                    } else {
                        s.rpc().name().set_auto(github_copilot_sdk::rpc::NameSetAutoRequest { summary: name }).await.map(|_| ())
                    };
                    if let Err(e) = r {
                        tracing::debug!("session name not stored by the runtime: {e}");
                    }
                });
            }
            Cmd::DeleteSession { id } => {
                let c = client.clone().unwrap();
                let ui = ui.clone();
                let self_tx = self_tx.clone();
                tokio::spawn(async move {
                    if let Err(e) = c.delete_session(&SessionId::from(id)).await {
                        ui.send(Evt::Toast { error: true, text: format!("Couldn't delete the session: {e}") });
                    }
                    let _ = self_tx.send(Cmd::ListSessions);
                });
            }
            Cmd::ListModels => {
                let c = client.clone().unwrap();
                let ui = ui.clone();
                tokio::spawn(async move {
                    match c.rpc().models().list().await {
                        Ok(list) => {
                            use github_copilot_sdk::rpc::{ModelPickerCategory, ModelPolicyState};
                            let models = list
                                .models
                                .into_iter()
                                // Models the user's plan or organization turned off can't be picked.
                                .filter(|m| !m.policy.as_ref().is_some_and(|p| matches!(p.state, ModelPolicyState::Disabled)))
                                .map(|m| ModelInfo {
                                    multiplier: m.billing.as_ref().and_then(|b| b.multiplier),
                                    category: match m.model_picker_category {
                                        Some(ModelPickerCategory::Powerful) => Some("powerful".into()),
                                        Some(ModelPickerCategory::Versatile) => Some("versatile".into()),
                                        Some(ModelPickerCategory::Lightweight) => Some("lightweight".into()),
                                        _ => None,
                                    },
                                    contexts: context_sizes(&m),
                                    efforts: m.supported_reasoning_efforts.unwrap_or_default(),
                                    default_effort: m.default_reasoning_effort,
                                    id: m.id,
                                    name: m.name,
                                })
                                .collect();
                            ui.send(Evt::Models(models));
                        }
                        Err(e) => tracing::warn!("models.list failed: {e}"),
                    }
                });
            }
            Cmd::ListSessions => {
                let c = client.clone().unwrap();
                let ui = ui.clone();
                tokio::spawn(async move {
                    match c.list_sessions(None).await {
                        Ok(list) => {
                            let mut out: Vec<SessionInfo> = list
                                .into_iter()
                                .filter(|m| m.client_name.as_deref() != Some(CURATOR_CLIENT))
                                .map(|m| SessionInfo { id: m.session_id.to_string(), summary: m.summary, modified: m.modified_time, client: m.last_client_name.or(m.client_name) })
                                .collect();
                            out.sort_by(|a, b| b.modified.cmp(&a.modified));
                            ui.send(Evt::Sessions(out));
                        }
                        Err(e) => ui.send(Evt::Toast { error: true, text: format!("Couldn't list sessions: {e}") }),
                    }
                });
            }
            Cmd::Learn { bay, model, message } => {
                let c = client.clone().unwrap();
                let ui = ui.clone();
                tokio::spawn(async move {
                    let result = curate(&c, model, message).await;
                    if let Err(e) = &result {
                        tracing::warn!("memory curator failed: {e}");
                    }
                    ui.send(Evt::Learned { bay, result });
                });
            }
            Cmd::Mcp(m) => mcp(client.clone().unwrap(), &sessions, m, ui.clone(), self_tx.clone()),
            Cmd::Plugin(p) => plugins(client.clone().unwrap(), p, ui.clone(), self_tx.clone()),
            Cmd::App(a) => apps(&sessions, a, ui.clone()),
        }
    }
}

fn to_session_mode(mode: Mode) -> SessionMode {
    match mode {
        Mode::Interactive => SessionMode::Interactive,
        Mode::Plan => SessionMode::Plan,
        Mode::Autopilot => SessionMode::Autopilot,
    }
}

const CANVAS_PROVIDER_ID: &str = "graysense.canvases";

fn canvas_identity() -> github_copilot_sdk::CanvasProviderIdentity {
    let mut id = github_copilot_sdk::CanvasProviderIdentity::new(CANVAS_PROVIDER_ID);
    id.name = Some("Graysense".into());
    id
}

async fn start_session(client: &Client, bay: BayId, spec: AgentSpec, ui: UiTx, self_tx: mpsc::UnboundedSender<Cmd>) -> Result<Session, String> {
    let bridge = Arc::new(Bridge {
        bay,
        ui: ui.clone(),
        auto: spec.auto_approve.clone(),
        canvases: Default::default(),
        granted: spec.granted.clone(),
        memory: spec.memory.clone(),
    });
    // Graysense memory backs the runtime's own `store_memory` tool. The runtime stops retrieving
    // on its own while a host store is set; relevant notes ride along with each user prompt via a
    // hook instead (user-message authority, never the system prompt).
    let memory_config = if spec.memory.is_some() { github_copilot_sdk::MemoryConfiguration::enabled() } else { github_copilot_sdk::MemoryConfiguration::disabled() };
    let memory_store: Option<Arc<dyn github_copilot_sdk::MemoryStore>> =
        spec.memory.clone().map(|m| Arc::new(LocalMemory { bay, ui: ui.clone(), memory: m }) as Arc<dyn github_copilot_sdk::MemoryStore>);
    let hooks: Option<Arc<dyn github_copilot_sdk::hooks::SessionHooks>> = spec
        .memory
        .clone()
        .map(|m| Arc::new(MemoryHooks { bay, ui: ui.clone(), memory: m, shown: Default::default(), started: AtomicBool::new(false) }) as Arc<dyn github_copilot_sdk::hooks::SessionHooks>);
    let tools = graysense_tools(bay, ui.clone(), spec.memory.clone(), client.clone(), self_tx);
    let prepared = if let Some(id) = spec.resume.clone() {
        let mut config = ResumeSessionConfig::new(SessionId::from(id))
            .with_client_name("graysense")
            .with_streaming(true)
            .with_permission_handler(bridge.clone())
            .with_elicitation_handler(bridge.clone())
            .with_ask_user_variant(AskUserVariant::Elicitation)
            .with_user_input_handler(bridge.clone())
            .with_exit_plan_mode_handler(bridge.clone())
            .with_auto_mode_switch_handler(bridge.clone())
            .with_canvas_handler(bridge.clone())
            .with_canvas_provider(canvas_identity())
            .with_request_canvas_renderer(true)
            .with_enable_mcp_apps(true)
            .with_interactive_user(true)
            .with_include_sub_agent_streaming_events(false)
            .with_memory(memory_config);
        if let Some(store) = memory_store {
            config = config.with_memory_store(store);
        }
        if let Some(hooks) = hooks {
            config = config.with_hooks(hooks);
        }
        config.tools = Some(tools);
        config.canvases = Some(crate::canvas::declarations());
        client.prepare_resume_session(config).map_err(|e| e.to_string())?
    } else {
        let mut config = SessionConfig::default()
            .with_client_name("graysense")
            .with_working_directory(spec.cwd.clone())
            .with_trust_working_directory(WorkingDirectoryTrust::Session)
            .with_enable_config_discovery(true)
            .with_streaming(true)
            .with_permission_handler(bridge.clone())
            .with_elicitation_handler(bridge.clone())
            .with_ask_user_variant(AskUserVariant::Elicitation)
            .with_user_input_handler(bridge.clone())
            .with_exit_plan_mode_handler(bridge.clone())
            .with_auto_mode_switch_handler(bridge.clone())
            .with_canvases(crate::canvas::declarations())
            .with_canvas_handler(bridge.clone())
            .with_canvas_provider(canvas_identity())
            .with_request_canvas_renderer(true)
            .with_enable_mcp_apps(true)
            .with_tools(tools)
            .with_interactive_user(true)
            .with_include_sub_agent_streaming_events(false)
            .with_memory(memory_config);
        if let Some(store) = memory_store {
            config = config.with_memory_store(store);
        }
        if let Some(hooks) = hooks {
            config = config.with_hooks(hooks);
        }
        if let Some(model) = spec.model.clone() {
            config = config.with_model(model);
        }
        if let Some(effort) = spec.effort.clone() {
            config = config.with_reasoning_effort(effort);
        }
        if let Some(tier) = spec.context_tier.clone() {
            config = config.with_context_tier(tier);
        }
        client.prepare_session(config).map_err(|e| e.to_string())?
    };

    let mut sub = prepared.subscribe();
    let pump = ui.clone();
    // A session that never started must not report "closed": the bay may already be running a
    // replacement (e.g. a fresh session after a failed resume).
    let live = Arc::new(AtomicBool::new(false));
    let pump_live = live.clone();
    tokio::spawn(async move {
        loop {
            match sub.recv().await {
                Ok(ev) => pump.send(Evt::Event { bay, ev: Box::new(ev) }),
                Err(err) => match err.kind() {
                    RecvErrorKind::Lagged(_) => continue,
                    _ => break,
                },
            }
        }
        if pump_live.load(Ordering::SeqCst) {
            pump.send(Evt::SessionClosed { bay });
        }
    });
    let session = prepared.start().await.map_err(|e| e.to_string())?;
    live.store(true, Ordering::SeqCst);
    tracing::info!(capabilities = ?session.capabilities(), "session started");
    Ok(session)
}

// ------------------------------------------------------------------ MCP / plugins / apps

fn status_str<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_else(|| "unknown".into())
}

fn mcp(client: Client, sessions: &HashMap<BayId, Arc<Session>>, cmd: McpCmd, ui: UiTx, self_tx: mpsc::UnboundedSender<Cmd>) {
    let session_for = |bay: BayId| sessions.get(&bay).cloned();
    match cmd {
        McpCmd::Refresh { bay, cwd } => {
            let live = bay.and_then(session_for).map(|s| (bay.unwrap(), s));
            tokio::spawn(async move {
                let result: Result<Vec<McpServerInfo>, String> = async {
                    let configs = client.rpc().mcp().config().list().await.map_err(|e| e.to_string())?.servers;
                    let discovered = client
                        .rpc()
                        .mcp()
                        .discover(McpDiscoverRequest { working_directory: cwd, ..Default::default() })
                        .await
                        .map_err(|e| e.to_string())?
                        .servers;
                    let mut out: Vec<McpServerInfo> = discovered
                        .into_iter()
                        .map(|d| McpServerInfo {
                            config: configs.get(&d.name).cloned(),
                            name: d.name,
                            enabled: d.enabled,
                            source: status_str(&d.source),
                            plugin: d.source_plugin,
                            status: None,
                            error: None,
                        })
                        .collect();
                    for (name, cfg) in &configs {
                        if !out.iter().any(|s| &s.name == name) {
                            out.push(McpServerInfo { name: name.clone(), enabled: true, source: "user".into(), plugin: None, config: Some(cfg.clone()), status: None, error: None });
                        }
                    }
                    if let Some((_, s)) = &live
                        && let Ok(list) = s.rpc().mcp().list().await {
                            for srv in list.servers {
                                let status = status_str(&srv.status);
                                match out.iter_mut().find(|o| o.name == srv.name) {
                                    Some(o) => {
                                        o.status = Some(status);
                                        o.error = srv.error;
                                    }
                                    None => out.push(McpServerInfo {
                                        name: srv.name,
                                        enabled: true,
                                        source: srv.source.as_ref().map(status_str).unwrap_or_else(|| "session".into()),
                                        plugin: srv.source_plugin,
                                        config: None,
                                        status: Some(status),
                                        error: srv.error,
                                    }),
                                }
                            }
                        }
                    out.sort_by(|a, b| a.source.cmp(&b.source).then(a.name.cmp(&b.name)));
                    Ok(out)
                }
                .await;
                ui.send(Evt::McpServers { servers: result, live_bay: live.map(|(b, _)| b) });
            });
        }
        McpCmd::Add { name, config, update } => {
            tokio::spawn(async move {
                tracing::info!(%name, update, config = %config, "mcp.config add/update");
                let r = if update {
                    client.rpc().mcp().config().update(McpConfigUpdateRequest { name: name.clone(), config }).await
                } else {
                    client.rpc().mcp().config().add(McpConfigAddRequest { name: name.clone(), config }).await
                };
                tracing::info!(%name, ok = r.is_ok(), "mcp.config add/update returned");
                let _ = client.rpc().mcp().config().reload().await;
                report(&ui, r.map_err(|e| e.to_string()), &format!("{} MCP server “{name}”", if update { "Updated" } else { "Added" }));
                ui.send(Evt::McpChanged);
            });
        }
        McpCmd::Remove { name } => {
            tokio::spawn(async move {
                let r = client.rpc().mcp().config().remove(McpConfigRemoveRequest { name: name.clone(), ..Default::default() }).await;
                let _ = client.rpc().mcp().config().reload().await;
                report(&ui, r.map_err(|e| e.to_string()), &format!("Removed MCP server “{name}”"));
                ui.send(Evt::McpChanged);
            });
        }
        McpCmd::SetEnabled { name, enabled } => {
            tokio::spawn(async move {
                let names = vec![name.clone()];
                let r = if enabled {
                    client.rpc().mcp().config().enable(McpConfigEnableRequest { names }).await
                } else {
                    client.rpc().mcp().config().disable(McpConfigDisableRequest { names }).await
                };
                report(&ui, r.map_err(|e| e.to_string()), &format!("{} “{name}”", if enabled { "Enabled" } else { "Disabled" }));
                ui.send(Evt::McpChanged);
            });
        }
        McpCmd::Tools { bay, server } => {
            let s = sessions[&bay].clone();
            tokio::spawn(async move {
                let r = s
                    .rpc()
                    .mcp()
                    .list_tools(McpListToolsRequest { server_name: server.clone() })
                    .await
                    .map(|res| {
                        res.tools
                            .into_iter()
                            .map(|t| McpToolInfo { has_ui: t.ui.as_ref().and_then(|u| u.resource_uri.as_ref()).is_some(), name: t.name, description: t.description })
                            .collect()
                    })
                    .map_err(|e| e.to_string());
                ui.send(Evt::McpTools { server, tools: r });
            });
        }
        McpCmd::Restart { bay, server } => {
            let s = sessions[&bay].clone();
            tokio::spawn(async move {
                let r = s.rpc().mcp().restart_server(McpRestartServerRequest { server_name: server.clone(), ..Default::default() }).await;
                report(&ui, r.map_err(|e| e.to_string()), &format!("Restarted “{server}”"));
                let _ = self_tx.send(Cmd::Mcp(McpCmd::Refresh { bay: Some(bay), cwd: None }));
            });
        }
        McpCmd::Stop { bay, server } => {
            let s = sessions[&bay].clone();
            tokio::spawn(async move {
                let r = s.rpc().mcp().stop_server(McpStopServerRequest { server_name: server.clone() }).await;
                report(&ui, r.map_err(|e| e.to_string()), &format!("Stopped “{server}”"));
                let _ = self_tx.send(Cmd::Mcp(McpCmd::Refresh { bay: Some(bay), cwd: None }));
            });
        }
        McpCmd::Start { bay, server } => {
            let s = sessions[&bay].clone();
            tokio::spawn(async move {
                let r = s.rpc().mcp().start_server(McpStartServerRequest { server_name: server.clone(), ..Default::default() }).await;
                report(&ui, r.map_err(|e| e.to_string()), &format!("Started “{server}”"));
                let _ = self_tx.send(Cmd::Mcp(McpCmd::Refresh { bay: Some(bay), cwd: None }));
            });
        }
    }
}

fn report(ui: &UiTx, r: Result<(), String>, ok_text: &str) {
    match r {
        Ok(()) => ui.send(Evt::Toast { error: false, text: ok_text.to_owned() }),
        Err(e) => {
            tracing::warn!("{ok_text} failed: {e}");
            ui.send(Evt::Toast { error: true, text: e })
        }
    }
}

fn plugins(client: Client, cmd: PluginCmd, ui: UiTx, self_tx: mpsc::UnboundedSender<Cmd>) {
    tokio::spawn(async move {
        let p = client.rpc().plugins();
        match cmd {
            PluginCmd::Refresh => {
                let installed = p.list().await.map_err(|e| e.to_string()).map(|r| {
                    r.plugins
                        .into_iter()
                        .map(|i| PluginInfo { name: i.name, marketplace: i.marketplace, version: i.version, description: i.description, enabled: i.enabled })
                        .collect()
                });
                ui.send(Evt::Plugins(installed));
                let markets = p.marketplaces().list().await.map_err(|e| e.to_string()).map(|r| {
                    r.marketplaces.into_iter().map(|m| MarketInfo { name: m.name, source: m.source, is_default: m.is_default.unwrap_or(false) }).collect()
                });
                ui.send(Evt::Markets(markets));
            }
            PluginCmd::Install { source } => {
                let r = p.install(PluginsInstallRequest { source: source.clone(), ..Default::default() }).await;
                match r {
                    Ok(res) => {
                        let mut text = format!("Installed {}", res.plugin.name);
                        if let Some(msg) = res.post_install_message {
                            text.push_str(&format!(" — {msg}"));
                        }
                        ui.send(Evt::Toast { error: false, text });
                    }
                    Err(e) => ui.send(Evt::Toast { error: true, text: format!("Install failed: {e}") }),
                }
                let _ = self_tx.send(Cmd::Plugin(PluginCmd::Refresh));
            }
            PluginCmd::Uninstall { name } => {
                let r = p.uninstall(PluginsUninstallRequest { name: name.clone(), ..Default::default() }).await;
                report(&ui, r.map_err(|e| e.to_string()), &format!("Uninstalled {name}"));
                let _ = self_tx.send(Cmd::Plugin(PluginCmd::Refresh));
            }
            PluginCmd::SetEnabled { name, enabled } => {
                let names = vec![name.clone()];
                let r = if enabled {
                    p.enable(PluginsEnableRequest { names, ..Default::default() }).await
                } else {
                    p.disable(PluginsDisableRequest { names, ..Default::default() }).await
                };
                report(&ui, r.map_err(|e| e.to_string()), &format!("{} {name}", if enabled { "Enabled" } else { "Disabled" }));
                let _ = self_tx.send(Cmd::Plugin(PluginCmd::Refresh));
            }
            PluginCmd::Update { name } => {
                match p.update(PluginsUpdateRequest { name: name.clone() }).await {
                    Ok(r) => ui.send(Evt::Toast { error: false, text: format!("Updated {name} to {}", r.new_version.unwrap_or_else(|| "latest".into())) }),
                    Err(e) => ui.send(Evt::Toast { error: true, text: format!("Update failed: {e}") }),
                }
                let _ = self_tx.send(Cmd::Plugin(PluginCmd::Refresh));
            }
            PluginCmd::UpdateAll => {
                match p.update_all().await {
                    Ok(r) => {
                        let ok = r.results.iter().filter(|e| e.success).count();
                        ui.send(Evt::Toast { error: false, text: format!("Updated {ok}/{} plugins", r.results.len()) });
                    }
                    Err(e) => ui.send(Evt::Toast { error: true, text: format!("Update failed: {e}") }),
                }
                let _ = self_tx.send(Cmd::Plugin(PluginCmd::Refresh));
            }
            PluginCmd::MarketAdd { source } => {
                match p.marketplaces().add(PluginsMarketplacesAddRequest { source, ..Default::default() }).await {
                    Ok(r) => ui.send(Evt::Toast { error: false, text: format!("Added marketplace {}", r.name) }),
                    Err(e) => ui.send(Evt::Toast { error: true, text: format!("Couldn't add marketplace: {e}") }),
                }
                let _ = self_tx.send(Cmd::Plugin(PluginCmd::Refresh));
            }
            PluginCmd::MarketRemove { name } => {
                let r = p.marketplaces().remove(PluginsMarketplacesRemoveRequest { name: name.clone(), ..Default::default() }).await.map(|_| ());
                report(&ui, r.map_err(|e| e.to_string()), &format!("Removed marketplace {name}"));
                let _ = self_tx.send(Cmd::Plugin(PluginCmd::Refresh));
            }
            PluginCmd::Browse { market } => {
                let r = p
                    .marketplaces()
                    .browse(PluginsMarketplacesBrowseRequest { name: market.clone() })
                    .await
                    .map(|r| r.plugins.into_iter().map(|m| MarketPlugin { name: m.name, description: m.description }).collect())
                    .map_err(|e| e.to_string());
                ui.send(Evt::MarketPlugins { market, plugins: r });
            }
        }
    });
}

fn apps(sessions: &HashMap<BayId, Arc<Session>>, cmd: AppCmd, ui: UiTx) {
    match cmd {
        AppCmd::CallTool { bay, view, req_id, server, tool, args } => {
            let s = sessions[&bay].clone();
            tokio::spawn(async move {
                let arguments = args.and_then(|a| serde_json::from_value::<HashMap<String, Value>>(a).ok());
                let req = McpAppsCallToolRequest { arguments, origin_server_name: server.clone(), server_name: server, tool_name: tool };
                let result = s.rpc().mcp().apps().call_tool(req).await.map(|m| serde_json::to_value(m).unwrap_or(Value::Null)).map_err(|e| e.to_string());
                ui.send(Evt::AppReply { view, req_id, result });
            });
        }
        AppCmd::ReadResource { bay, view, req_id, server, uri } => {
            let s = sessions[&bay].clone();
            tokio::spawn(async move {
                let result = s
                    .rpc()
                    .mcp()
                    .apps()
                    .read_resource(McpAppsReadResourceRequest { server_name: server, uri })
                    .await
                    .map(|r| json!({ "contents": serde_json::to_value(r.contents).unwrap_or(Value::Null) }))
                    .map_err(|e| e.to_string());
                ui.send(Evt::AppReply { view, req_id, result });
            });
        }
    }
}

// ------------------------------------------------------------------ handler bridge

struct Bridge {
    bay: BayId,
    ui: UiTx,
    auto: Arc<AtomicBool>,
    /// Canvas instances this session opened through Graysense's own provider.
    canvases: parking_lot::Mutex<std::collections::HashSet<String>>,
    granted: Arc<parking_lot::Mutex<Vec<PathBuf>>>,
    memory: Option<AgentMemory>,
}

impl Bridge {
    /// Saving to Graysense memory needs no extra prompt: it lands in the user's own,
    /// reviewable store with an Undo chip in the transcript. Commands covered by an
    /// approval rule the user accepted run without asking.
    fn covered_by_memory(&self, raw: &Value) -> bool {
        let Some(mem) = &self.memory else { return false };
        if !crate::memory::enabled() {
            return false;
        }
        let Some(pr) = raw.get("promptRequest") else { return false };
        match pr.get("kind").and_then(Value::as_str) {
            Some("memory") => true,
            Some("commands") if pr.get("warning").is_none() => {
                let cmds: Vec<String> = pr.get("commandIdentifiers").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default();
                mem.store.lock().rule_allows(&mem.project.key, &cmds)
            }
            _ => false,
        }
    }
    /// Read/path requests limited to files the user explicitly attached are pre-approved.
    fn covers_attachments(&self, raw: &Value) -> bool {
        let Some(pr) = raw.get("promptRequest") else { return false };
        let paths: Vec<String> = match pr.get("kind").and_then(Value::as_str) {
            Some("read") => pr.get("path").and_then(Value::as_str).map(|p| vec![p.to_owned()]).unwrap_or_default(),
            Some("path") => pr.get("paths").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default(),
            _ => return false,
        };
        let granted = self.granted.lock();
        if paths.is_empty() || granted.is_empty() {
            return false;
        }
        let norm = |p: &str| p.replace('/', "\\").trim_end_matches('\\').to_lowercase();
        let granted: Vec<String> = granted.iter().map(|g| norm(&g.to_string_lossy())).collect();
        paths.iter().all(|p| {
            let p = norm(p);
            granted.iter().any(|g| p == *g || p.starts_with(&format!("{g}\\")))
        })
    }
    /// Canvas tools that only drive Graysense's own native canvases have no side
    /// effects outside the app, so they never need a human in the loop.
    fn is_own_canvas_call(&self, raw: &Value) -> bool {
        let Some(pr) = raw.get("promptRequest").or_else(|| raw.get("permissionRequest")) else { return false };
        if pr.get("kind").and_then(Value::as_str) != Some("custom-tool") {
            return false;
        }
        let args = pr.get("args").cloned().unwrap_or(Value::Null);
        let arg = |k: &str| args.get(k).and_then(Value::as_str);
        let ours = matches!(arg("extensionId"), None | Some(CANVAS_PROVIDER_ID));
        match pr.get("toolName").and_then(Value::as_str) {
            Some("list_canvas_capabilities") => ours,
            Some("open_canvas") => ours && arg("canvasId").is_some_and(crate::canvas::is_declared),
            Some("invoke_canvas_action") => arg("instanceId").is_some_and(|id| self.canvases.lock().contains(id)),
            _ => false,
        }
    }
}

/// Anything that reaches into Graysense's own memory or settings files always asks a human,
/// even in auto-approve mode, so an agent can't quietly rewrite what future agents believe.
fn touches_graysense_state(raw: &Value) -> bool {
    let s = raw.to_string().to_lowercase().replace("\\\\", "\\").replace('/', "\\");
    ["\\graysense\\memory.json", "\\graysense\\settings.json", "\\graysense\\workspace.json"].iter().any(|p| s.contains(p))
}

fn approved_once() -> PermissionResult {
    serde_json::from_value::<PermissionDecision>(json!({ "kind": "approve-once", "approvedInteractively": true }))
        .map(PermissionResult::from)
        .unwrap_or_else(|_| PermissionResult::approve_once())
}

/// Build an "approve for session" decision from the derived prompt metadata.
fn session_decision(raw: &Value) -> Option<PermissionResult> {
    let pr = raw.get("promptRequest")?;
    if pr.get("canOfferSessionApproval").and_then(|v| v.as_bool()) == Some(false) {
        return None;
    }
    let kind = pr.get("kind")?.as_str()?;
    let decision = match kind {
        "commands" => json!({ "kind": "approve-for-session", "approval": { "kind": "commands", "commandIdentifiers": pr.get("commandIdentifiers").cloned().unwrap_or(json!([])) } }),
        "write" => json!({ "kind": "approve-for-session", "approval": { "kind": "write" } }),
        "read" => json!({ "kind": "approve-for-session", "approval": { "kind": "read" } }),
        "mcp" => json!({ "kind": "approve-for-session", "approval": { "kind": "mcp", "serverName": pr.get("serverName")?, "toolName": pr.get("toolName").cloned().unwrap_or(Value::Null) } }),
        "custom-tool" => json!({ "kind": "approve-for-session", "approval": { "kind": "custom-tool", "toolName": pr.get("toolName")? } }),
        "url" => {
            let url = pr.get("url")?.as_str()?;
            let host = url.split("://").nth(1)?.split(['/', '?', '#']).next()?.to_owned();
            json!({ "kind": "approve-for-session", "domain": host })
        }
        _ => return None,
    };
    serde_json::from_value::<PermissionDecision>(decision).ok().map(PermissionResult::from)
}

pub fn can_offer_session(raw: &Value) -> bool {
    session_decision(raw).is_some()
}

#[async_trait]
impl PermissionHandler for Bridge {
    async fn handle(&self, _sid: SessionId, _rid: RequestId, data: PermissionRequestData) -> PermissionResult {
        let raw = data.extra.clone();
        // Memory switched off while this agent was running: decline saves instead of asking.
        if self.memory.is_some() && !crate::memory::enabled() && raw.pointer("/promptRequest/kind").and_then(Value::as_str) == Some("memory") {
            return PermissionResult::reject(Some("Memory is turned off in Graysense, so nothing was saved.".to_owned()));
        }
        if data.managed_approval_required != Some(true)
            && !touches_graysense_state(&raw)
            && (self.auto.load(Ordering::Relaxed) || self.is_own_canvas_call(&raw) || self.covers_attachments(&raw) || self.covered_by_memory(&raw) || is_safe_config_call(&raw))
        {
            return PermissionResult::approve_once();
        }
        let (tx, rx) = oneshot::channel();
        self.ui.send(Evt::Permission { bay: self.bay, prompt: PermissionPrompt { raw: raw.clone(), reply: Some(tx), outcome: None, feedback: String::new() } });
        match rx.await {
            Ok(PermDecision::Once) => approved_once(),
            Ok(PermDecision::Session) => session_decision(&raw).unwrap_or_else(approved_once),
            Ok(PermDecision::Reject(feedback)) => PermissionResult::reject(feedback),
            Err(_) => PermissionResult::reject(Some("The user dismissed this request.".to_owned())),
        }
    }
}

#[async_trait]
impl ElicitationHandler for Bridge {
    async fn handle(&self, _sid: SessionId, _rid: RequestId, request: ElicitationRequest) -> ElicitationResult {
        let (tx, rx) = oneshot::channel();
        let url = if request.mode == Some(ElicitationMode::Url) { request.url.clone() } else { None };
        let prompt = ElicitPrompt {
            form: forms::FormState::new(request.requested_schema.as_ref()),
            message: request.message,
            url,
            source: request.elicitation_source,
            reply: Some(tx),
            outcome: None,
        };
        self.ui.send(Evt::Elicit { bay: self.bay, prompt: Box::new(prompt) });
        rx.await.unwrap_or(ElicitationResult { action: "cancel".into(), content: None })
    }
}

#[async_trait]
impl UserInputHandler for Bridge {
    async fn handle(&self, _sid: SessionId, _rid: Option<RequestId>, question: String, choices: Option<Vec<String>>, allow_freeform: Option<bool>) -> Option<UserInputResponse> {
        let (tx, rx) = oneshot::channel();
        let prompt = AskPrompt { question, choices: choices.unwrap_or_default(), freeform: allow_freeform.unwrap_or(true), reply: Some(tx), outcome: None };
        self.ui.send(Evt::Ask { bay: self.bay, prompt });
        rx.await.ok().flatten().map(|(answer, was_freeform)| UserInputResponse { answer, was_freeform })
    }
}

#[async_trait]
impl ExitPlanModeHandler for Bridge {
    async fn handle(&self, _sid: SessionId, _rid: Option<RequestId>, data: ExitPlanModeData) -> ExitPlanModeResult {
        let (tx, rx) = oneshot::channel();
        let prompt = PlanPrompt { summary: data.summary, plan: data.plan_content, actions: data.actions, recommended: data.recommended_action, reply: Some(tx), outcome: None };
        self.ui.send(Evt::Plan { bay: self.bay, prompt });
        rx.await.unwrap_or(ExitPlanModeResult { approved: false, selected_action: None, feedback: Some("Dismissed".into()) })
    }
}

#[async_trait]
impl AutoModeSwitchHandler for Bridge {
    async fn handle(&self, _sid: SessionId, error_code: Option<String>, retry_after_seconds: Option<f64>) -> AutoModeSwitchResponse {
        let (tx, rx) = oneshot::channel();
        let mut text = "The current model is rate limited".to_owned();
        if let Some(code) = error_code {
            text.push_str(&format!(" ({code})"));
        }
        if let Some(secs) = retry_after_seconds {
            text.push_str(&format!(", retry in {:.0}s", secs));
        }
        text.push_str(". Switch to Auto so the agent can keep working?");
        self.ui.send(Evt::Switch { bay: self.bay, prompt: SwitchPrompt { text, reply: Some(tx), outcome: None } });
        rx.await.unwrap_or(AutoModeSwitchResponse::No)
    }
}

#[async_trait]
impl CanvasHandler for Bridge {
    async fn on_open(&self, ctx: CanvasProviderOpenRequest) -> CanvasResult<CanvasProviderOpenResult> {
        let input = ctx.input.clone().unwrap_or(Value::Null);
        let title = crate::canvas::validate_open(&ctx.canvas_id, &input).map_err(|e| CanvasError::new("invalid_input", e))?;
        self.canvases.lock().insert(ctx.instance_id.clone());
        self.ui.send(Evt::CanvasOpen { bay: self.bay, instance_id: ctx.instance_id, canvas_id: ctx.canvas_id, input });
        Ok(CanvasProviderOpenResult { status: Some("Open in Graysense".into()), title: Some(title), url: None })
    }

    async fn on_action(&self, ctx: CanvasProviderInvokeActionRequest) -> CanvasResult<Value> {
        let (tx, rx) = oneshot::channel();
        self.ui.send(Evt::CanvasAction { bay: self.bay, instance_id: ctx.instance_id, action: ctx.action_name, input: ctx.input.unwrap_or(Value::Null), reply: tx });
        match rx.await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(CanvasError::new("action_failed", e)),
            Err(_) => Err(CanvasError::new("canvas_closed", "The canvas was closed")),
        }
    }

    async fn on_close(&self, ctx: CanvasProviderCloseRequest) -> CanvasResult<()> {
        self.canvases.lock().remove(&ctx.instance_id);
        self.ui.send(Evt::CanvasClosed { bay: self.bay, instance_id: ctx.instance_id });
        Ok(())
    }
}

// ------------------------------------------------------------------ Graysense tools for agents

struct AddMissionTool {
    bay: BayId,
    ui: UiTx,
}

#[async_trait]
impl ToolHandler for AddMissionTool {
    async fn call(&self, inv: ToolInvocation) -> Result<ToolResult, github_copilot_sdk::Error> {
        let title = inv.arguments.get("title").and_then(|v| v.as_str()).unwrap_or("Untitled idea").trim().to_owned();
        let notes = inv.arguments.get("notes").and_then(|v| v.as_str()).unwrap_or("").trim().to_owned();
        self.ui.send(Evt::AddMission { bay: self.bay, title: title.clone(), notes });
        Ok(ToolResult::Text(format!("Added \"{title}\" to the user's Graysense mission board.")))
    }
}

struct NotifyTool {
    bay: BayId,
    ui: UiTx,
}

#[async_trait]
impl ToolHandler for NotifyTool {
    async fn call(&self, inv: ToolInvocation) -> Result<ToolResult, github_copilot_sdk::Error> {
        let get = |k: &str| inv.arguments.get(k).and_then(|v| v.as_str()).unwrap_or("").to_owned();
        self.ui.send(Evt::Notify { bay: self.bay, title: get("title"), message: get("message"), level: get("level") });
        Ok(ToolResult::Text("Notification shown to the user.".into()))
    }
}

fn graysense_tools(bay: BayId, ui: UiTx, memory: Option<AgentMemory>, client: Client, self_tx: mpsc::UnboundedSender<Cmd>) -> Vec<Tool> {
    let note_ui = ui.clone();
    let mut tools = vec![
        Tool::new("graysense_add_mission")
            .with_description(
                "Add an idea or follow-up task to the user's Graysense mission board, a scratchpad of ideas the user can later \
                 delegate to a Copilot agent. Use it for worthwhile follow-ups you discover but should not do now.",
            )
            .with_parameters(json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "Short imperative title, e.g. 'Add retry to the uploader'" },
                    "notes": { "type": "string", "description": "Context, links, acceptance criteria (markdown)" }
                },
                "required": ["title"]
            }))
            .with_skip_permission(true)
            .with_handler(Arc::new(AddMissionTool { bay, ui: ui.clone() })),
        Tool::new("graysense_notify")
            .with_description("Show a desktop notification to the user in Graysense. Use sparingly, for milestones or when you need their attention.")
            .with_parameters(json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "message": { "type": "string" },
                    "level": { "type": "string", "enum": ["info", "success", "warning", "error"] }
                },
                "required": ["title", "message"]
            }))
            .with_skip_permission(true)
            .with_handler(Arc::new(NotifyTool { bay, ui: ui.clone() })),
        // Everything configurable in Graysense is also configurable by agents. These tools go
        // through the normal approval flow; appearance changes and reads are approved automatically.
        Tool::new("graysense_settings")
            .with_description(
                "Read or change the settings of Graysense, the app you are running in: theme, terminal font size, layout, side panel, \
                 default model, terminal shell, the folder new sessions are created in, memory switches and more. \
                 Call with action \"get\" first to see every setting, its current value and what it accepts, then action \"set\" with \
                 `changes`, e.g. {\"theme\": \"paper\", \"terminal_font_size\": 15}. Appearance changes apply at once; others ask the user.",
            )
            .with_parameters(json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["get", "set"] },
                    "changes": { "type": "object", "description": "For \"set\": setting keys (from \"get\") and their new values; null restores a default." }
                },
                "required": ["action"]
            }))
            .with_handler(Arc::new(SettingsTool { ui: ui.clone() })),
        Tool::new("graysense_mcp_servers")
            .with_description(
                "List, add, update, remove, enable or disable the MCP servers configured for Copilot (shared with the Copilot CLI). \
                 For add/update, `config` is the server's JSON, e.g. {\"type\": \"local\", \"command\": \"npx\", \"args\": [\"-y\", \"some-mcp-server\"], \"tools\": [\"*\"]} \
                 or {\"type\": \"http\", \"url\": \"https://example.com/mcp\", \"headers\": {}, \"tools\": [\"*\"]}. New agents load the change.",
            )
            .with_parameters(json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["list", "add", "update", "remove", "enable", "disable"] },
                    "name": { "type": "string", "description": "Server name (all actions except list)" },
                    "config": { "type": "object", "description": "Server config (add, update)" }
                },
                "required": ["action"]
            }))
            .with_handler(Arc::new(McpTool { client: client.clone(), ui: ui.clone() })),
        Tool::new("graysense_plugins")
            .with_description(
                "List, install, uninstall, enable, disable or update Copilot plugins. \"install\" takes `source` (e.g. plugin@marketplace, \
                 a GitHub owner/repo or a URL); the other actions take the plugin's `name`.",
            )
            .with_parameters(json!({
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["list", "install", "uninstall", "enable", "disable", "update"] },
                    "name": { "type": "string" },
                    "source": { "type": "string" }
                },
                "required": ["action"]
            }))
            .with_handler(Arc::new(PluginsTool { client, ui, self_tx })),
    ];
    if let Some(memory) = memory {
        tools.push(
            Tool::new("graysense_recall")
                .with_description(
                    "Search Graysense memory: notes the user saved (things they did, sent or decided, with the files and links that go with them) \
                     plus preferences, project facts and how-tos learned from earlier sessions. The most relevant ones are already attached to \
                     the user's messages; use this to look up anything else, e.g. \"what did I send Charles\" or \"Rayfin logo\". Notes list their files' paths.",
                )
                .with_parameters(json!({
                    "type": "object",
                    "properties": { "query": { "type": "string", "description": "What you want to know, in a few words" } },
                    "required": ["query"]
                }))
                .with_skip_permission(true)
                .with_handler(Arc::new(RecallTool { memory: memory.clone() })),
        );
        tools.push(
            Tool::new("graysense_save_note")
                .with_description(
                    "Save something to the user's Graysense notes when they ask you to keep it for later (\"save this for next time\", \"note that I sent \
                     Charles the logo\"). Write the note as the user would want to find it later: who, what, when and why. Attach the files (absolute \
                     paths; Graysense keeps a copy) and links that go with it. Not for preferences or project facts: use store_memory for those.",
                )
                .with_parameters(json!({
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "The note, e.g. \"Sent Charles the Rayfin logo (dark version) for the partner deck.\"" },
                        "files": { "type": "array", "items": { "type": "string" }, "description": "Absolute paths of files or folders to keep with the note" },
                        "links": { "type": "array", "items": { "type": "string" }, "description": "URLs to keep with the note" }
                    },
                    "required": ["text"]
                }))
                .with_skip_permission(true)
                .with_handler(Arc::new(SaveNoteTool { bay, ui: note_ui, memory })),
        );
    }
    tools
}

// ------------------------------------------------------------------ agent-configurable settings

/// Reads and patches Graysense settings on the UI thread, which owns them.
struct SettingsTool {
    ui: UiTx,
}

#[async_trait]
impl ToolHandler for SettingsTool {
    async fn call(&self, inv: ToolInvocation) -> Result<ToolResult, github_copilot_sdk::Error> {
        let changes = match inv.arguments.get("action").and_then(Value::as_str).unwrap_or("get") {
            "get" => None,
            "set" => match inv.arguments.get("changes").and_then(Value::as_object) {
                Some(c) if !c.is_empty() => Some(c.clone()),
                _ => return Ok(ToolResult::Text("Pass `changes`: an object of setting keys and their new values.".into())),
            },
            other => return Ok(ToolResult::Text(format!("Unknown action “{other}”: use get or set."))),
        };
        let (tx, rx) = oneshot::channel();
        self.ui.send(Evt::Config { changes, reply: tx });
        let text = match tokio::time::timeout(std::time::Duration::from_secs(15), rx).await {
            Ok(Ok(Ok(v))) => serde_json::to_string_pretty(&v).unwrap_or_default(),
            Ok(Ok(Err(e))) => e,
            _ => "Graysense didn't respond.".into(),
        };
        Ok(ToolResult::Text(text))
    }
}

/// Reads and safe appearance changes need no approval; everything else asks the user.
fn is_safe_config_call(raw: &Value) -> bool {
    let Some(pr) = raw.get("promptRequest").or_else(|| raw.get("permissionRequest")) else { return false };
    if pr.get("kind").and_then(Value::as_str) != Some("custom-tool") {
        return false;
    }
    let args = pr.get("args").cloned().unwrap_or(Value::Null);
    let action = args.get("action").and_then(Value::as_str).unwrap_or("");
    match pr.get("toolName").and_then(Value::as_str) {
        Some("graysense_settings") => {
            action == "get" || action == "set" && args.get("changes").and_then(Value::as_object).is_some_and(|c| c.keys().all(|k| crate::settings::SAFE_KEYS.contains(&k.as_str())))
        }
        Some("graysense_mcp_servers" | "graysense_plugins") => action == "list",
        _ => false,
    }
}

fn arg(inv: &ToolInvocation, key: &str) -> Option<String> {
    inv.arguments.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
}

struct McpTool {
    client: Client,
    ui: UiTx,
}

#[async_trait]
impl ToolHandler for McpTool {
    async fn call(&self, inv: ToolInvocation) -> Result<ToolResult, github_copilot_sdk::Error> {
        let action = arg(&inv, "action").unwrap_or_else(|| "list".into());
        let c = &self.client;
        let result: Result<String, String> = async {
            if action == "list" {
                let configured = c.rpc().mcp().config().list().await.map_err(|e| e.to_string())?.servers;
                let found = c.rpc().mcp().discover(McpDiscoverRequest::default()).await.map(|d| d.servers).unwrap_or_default();
                let servers: Vec<Value> = found
                    .into_iter()
                    .map(|d| json!({ "name": d.name, "enabled": d.enabled, "source": status_str(&d.source), "config": configured.get(&d.name) }))
                    .collect();
                return Ok(serde_json::to_string_pretty(&json!({ "servers": servers })).unwrap_or_default());
            }
            let name = arg(&inv, "name").ok_or("Pass `name`.")?;
            match action.as_str() {
                "add" | "update" => {
                    let config = inv.arguments.get("config").cloned().filter(Value::is_object).ok_or("Pass `config`, the server's JSON config.")?;
                    let r = if action == "add" {
                        c.rpc().mcp().config().add(McpConfigAddRequest { name: name.clone(), config }).await
                    } else {
                        c.rpc().mcp().config().update(McpConfigUpdateRequest { name: name.clone(), config }).await
                    };
                    r.map_err(|e| e.to_string())?;
                    let _ = c.rpc().mcp().config().reload().await;
                    Ok(format!("{} MCP server “{name}”. New agents will load it.", if action == "add" { "Added" } else { "Updated" }))
                }
                "remove" => {
                    c.rpc().mcp().config().remove(McpConfigRemoveRequest { name: name.clone(), ..Default::default() }).await.map_err(|e| e.to_string())?;
                    let _ = c.rpc().mcp().config().reload().await;
                    Ok(format!("Removed MCP server “{name}”."))
                }
                "enable" => c.rpc().mcp().config().enable(McpConfigEnableRequest { names: vec![name.clone()] }).await.map(|_| format!("Enabled “{name}”.")).map_err(|e| e.to_string()),
                "disable" => c.rpc().mcp().config().disable(McpConfigDisableRequest { names: vec![name.clone()] }).await.map(|_| format!("Disabled “{name}”.")).map_err(|e| e.to_string()),
                other => Err(format!("Unknown action “{other}”.")),
            }
        }
        .await;
        finish_config_call(&self.ui, &action, result, || self.ui.send(Evt::McpChanged))
    }
}

struct PluginsTool {
    client: Client,
    ui: UiTx,
    self_tx: mpsc::UnboundedSender<Cmd>,
}

#[async_trait]
impl ToolHandler for PluginsTool {
    async fn call(&self, inv: ToolInvocation) -> Result<ToolResult, github_copilot_sdk::Error> {
        let action = arg(&inv, "action").unwrap_or_else(|| "list".into());
        let p = self.client.rpc().plugins();
        let result: Result<String, String> = async {
            match action.as_str() {
                "list" => {
                    let plugins: Vec<Value> = p
                        .list()
                        .await
                        .map_err(|e| e.to_string())?
                        .plugins
                        .into_iter()
                        .map(|i| json!({ "name": i.name, "marketplace": i.marketplace, "version": i.version, "enabled": i.enabled, "description": i.description }))
                        .collect();
                    Ok(serde_json::to_string_pretty(&json!({ "plugins": plugins })).unwrap_or_default())
                }
                "install" => {
                    let source = arg(&inv, "source").or_else(|| arg(&inv, "name")).ok_or("Pass `source`.")?;
                    let r = p.install(PluginsInstallRequest { source, ..Default::default() }).await.map_err(|e| e.to_string())?;
                    Ok(format!("Installed {}{}", r.plugin.name, r.post_install_message.map(|m| format!(" — {m}")).unwrap_or_default()))
                }
                _ => {
                    let name = arg(&inv, "name").ok_or("Pass `name`.")?;
                    let names = vec![name.clone()];
                    match action.as_str() {
                        "uninstall" => p.uninstall(PluginsUninstallRequest { name: name.clone(), ..Default::default() }).await.map(|_| format!("Uninstalled {name}.")),
                        "enable" => p.enable(PluginsEnableRequest { names, ..Default::default() }).await.map(|_| format!("Enabled {name}.")),
                        "disable" => p.disable(PluginsDisableRequest { names, ..Default::default() }).await.map(|_| format!("Disabled {name}.")),
                        "update" => p.update(PluginsUpdateRequest { name: name.clone() }).await.map(|r| format!("Updated {name} to {}.", r.new_version.unwrap_or_else(|| "the latest version".into()))),
                        other => return Err(format!("Unknown action “{other}”.")),
                    }
                    .map_err(|e| e.to_string())
                }
            }
        }
        .await;
        finish_config_call(&self.ui, &action, result, || {
            let _ = self.self_tx.send(Cmd::Plugin(PluginCmd::Refresh));
        })
    }
}

/// Report a configuration change to the user (toast) and the agent, and refresh the UI.
fn finish_config_call(ui: &UiTx, action: &str, result: Result<String, String>, refresh: impl FnOnce()) -> Result<ToolResult, github_copilot_sdk::Error> {
    if action != "list" {
        refresh();
        match &result {
            Ok(text) => ui.send(Evt::Toast { error: false, text: format!("Copilot: {text}") }),
            Err(e) => ui.send(Evt::Toast { error: true, text: e.clone() }),
        }
    }
    Ok(ToolResult::Text(result.unwrap_or_else(|e| format!("Failed: {e}"))))
}

// ------------------------------------------------------------------ memory

/// Client name of the background curator sessions (hidden from the session list).
pub const CURATOR_CLIENT: &str = "graysense-memory";

/// Attaches memory to user prompts: the core set on the first prompt of a session, then only
/// newly relevant notes, and reports each recall to the UI.
struct MemoryHooks {
    bay: BayId,
    ui: UiTx,
    memory: AgentMemory,
    shown: parking_lot::Mutex<HashSet<crate::memory::MemoryId>>,
    started: AtomicBool,
}

#[async_trait]
impl github_copilot_sdk::hooks::SessionHooks for MemoryHooks {
    async fn on_user_prompt_submitted(
        &self,
        input: github_copilot_sdk::hooks::UserPromptSubmittedInput,
        _ctx: github_copilot_sdk::hooks::HookContext,
    ) -> Option<github_copilot_sdk::hooks::UserPromptSubmittedOutput> {
        if !crate::memory::enabled() {
            return None;
        }
        let first = !self.started.swap(true, Ordering::SeqCst);
        let project = Some(&self.memory.project);
        // Notes the user asked about come attached to the prompt already; don't add them again.
        let (asked, included) = crate::memory::split_included(&input.prompt);
        let skip = {
            let mut shown = self.shown.lock();
            shown.extend(included);
            shown.clone()
        };
        let (mut block, ids, saved) = {
            let store = self.memory.store.lock();
            let saved = store.captured_from(&input.prompt).map(|m| m.text.clone());
            let (block, ids) = if !crate::memory::sharing() {
                (String::new(), Vec::new())
            } else if first {
                store.context_for(project, Some(asked), 3_600, &skip)
            } else {
                store.followup_for(project, asked, &skip, 3)
            };
            (block, ids, saved)
        };
        if !ids.is_empty() {
            let entries = {
                let mut store = self.memory.store.lock();
                store.mark_used(&ids);
                ids.iter().filter_map(|i| store.get(*i).map(|m| (*i, m.text.clone()))).collect()
            };
            self.shown.lock().extend(ids.iter().copied());
            self.ui.send(Evt::Recalled { bay: self.bay, entries });
        } else {
            block.clear();
        }
        // Graysense saves "remember that …" itself the moment it's sent; keep the agent from
        // storing a second, differently worded copy.
        if let Some(text) = saved {
            block.push_str(&format!(
                "\n<graysense_saved>Graysense already saved this to the user's memory as: \"{text}\". Don't store it again; a short acknowledgement is enough.</graysense_saved>"
            ));
        }
        if block.trim().is_empty() {
            return None;
        }
        Some(github_copilot_sdk::hooks::UserPromptSubmittedOutput { additional_context: Some(block.trim_start().to_owned()), ..Default::default() })
    }
}

/// Host-owned store behind the runtime's `store_memory` tool.
struct LocalMemory {
    bay: BayId,
    ui: UiTx,
    memory: AgentMemory,
}

#[async_trait]
impl github_copilot_sdk::MemoryStore for LocalMemory {
    async fn store(&self, m: github_copilot_sdk::StoredMemory) -> Result<(), github_copilot_sdk::Error> {
        use crate::memory::{Kind, Origin, Scope, Source, State};
        if !crate::memory::enabled() {
            return Err(github_copilot_sdk::Error::with_message(github_copilot_sdk::ErrorKind::InvalidConfig, "Memory is turned off in Graysense, so nothing was saved."));
        }
        let fact = m.fact.trim().to_owned();
        let lower = format!("{} {}", m.subject, fact).to_lowercase();
        let about_user = ["user prefer", "user likes", "user wants", "user's preference", "prefers ", "the user "].iter().any(|k| lower.contains(k)) && !lower.contains("this repo") && !lower.contains("this project");
        let kind = if about_user || lower.contains("prefer") {
            Kind::Preference
        } else if ["run ", "use `", "command", "build", "test", "install", "deploy", "how to"].iter().any(|k| lower.contains(k)) {
            Kind::Procedure
        } else {
            Kind::Fact
        };
        let scope = if about_user { Scope::Global } else { self.memory.project.scope() };
        let quote = [m.reason.trim(), m.citations.trim()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
        let source = Source { session: Some(m.session_id.to_string()), title: None, quote: (!quote.is_empty()).then_some(quote), at: crate::memory::now() };
        let id = self.memory.store.lock().insert(kind, scope, &fact, Origin::Agent, State::Active, 0.9, Some(source));
        match id {
            Some(id) => {
                self.ui.send(Evt::Remembered { bay: self.bay, id, text: fact });
                Ok(())
            }
            None => Err(github_copilot_sdk::Error::with_message(github_copilot_sdk::ErrorKind::InvalidConfig, "Graysense declined to store this memory (it looked like a secret or was empty).")),
        }
    }
}

struct RecallTool {
    memory: AgentMemory,
}

#[async_trait]
impl ToolHandler for RecallTool {
    async fn call(&self, inv: ToolInvocation) -> Result<ToolResult, github_copilot_sdk::Error> {
        if !crate::memory::sharing() {
            return Ok(ToolResult::Text("Graysense memory is turned off.".into()));
        }
        let query = inv.arguments.get("query").and_then(|v| v.as_str()).unwrap_or("").to_owned();
        let mut store = self.memory.store.lock();
        let hits: Vec<(u64, String)> = store
            .search(&query, Some(&self.memory.project), 8)
            .into_iter()
            .map(|m| (m.id, if m.kind == crate::memory::Kind::Note { crate::memory::MemoryStore::note_for_agent(m) } else { format!("- ({}) {}", m.kind.label(), m.text) }))
            .collect();
        let ids: Vec<u64> = hits.iter().map(|(id, _)| *id).collect();
        store.mark_used(&ids);
        let text = if hits.is_empty() { "No matching notes in Graysense memory.".to_owned() } else { hits.into_iter().map(|(_, l)| l).collect::<Vec<_>>().join("\n") };
        Ok(ToolResult::Text(text))
    }
}

/// `graysense_save_note`: an agent saves something to the user's notes when asked to.
struct SaveNoteTool {
    bay: BayId,
    ui: UiTx,
    memory: AgentMemory,
}

#[async_trait]
impl ToolHandler for SaveNoteTool {
    async fn call(&self, inv: ToolInvocation) -> Result<ToolResult, github_copilot_sdk::Error> {
        if !crate::memory::enabled() {
            return Ok(ToolResult::Text("Memory is turned off in Graysense, so nothing was saved.".into()));
        }
        let text = inv.arguments.get("text").and_then(|v| v.as_str()).unwrap_or("").to_owned();
        let list = |key: &str| -> Vec<String> { inv.arguments.get(key).and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default() };
        let files: Vec<PathBuf> = list("files").into_iter().map(PathBuf::from).filter(|p| p.is_absolute() && p.exists()).collect();
        let links = list("links");
        let source = crate::memory::Source { session: Some(inv.session_id.to_string()), title: None, quote: None, at: crate::memory::now() };
        let saved = self.memory.store.lock().add_note(&text, &files, &links, crate::memory::Scope::Global, Some(source));
        match saved {
            Ok(id) => {
                let shown = text.lines().next().unwrap_or("").to_owned();
                self.ui.send(Evt::Remembered { bay: self.bay, id, text: shown });
                Ok(ToolResult::Text(format!("Saved to the user's Graysense notes (with {} attachment{}).", files.len() + links.len(), if files.len() + links.len() == 1 { "" } else { "s" })))
            }
            Err(e) => Ok(ToolResult::Text(format!("Not saved: {e}"))),
        }
    }
}

/// Refuses every permission request: the curator has nothing to run.
struct DenyAll;

#[async_trait]
impl PermissionHandler for DenyAll {
    async fn handle(&self, _sid: SessionId, _rid: RequestId, _data: PermissionRequestData) -> PermissionResult {
        PermissionResult::reject(Some("The Graysense memory curator cannot use tools.".to_owned()))
    }
}

/// Run one curator pass in a throwaway, tool-less session and return its JSON answer.
async fn curate(client: &Client, model: Option<String>, message: String) -> Result<String, String> {
    // Keep the curator light and isolated: no MCP servers, skills, instructions or tools.
    let servers: Vec<String> = client.rpc().mcp().config().list().await.ok().map(|l| l.servers.keys().cloned().collect()).unwrap_or_default();
    let mut config = SessionConfig::default()
        .with_client_name(CURATOR_CLIENT)
        .with_working_directory(crate::settings::config_dir())
        .with_enable_config_discovery(false)
        .with_skip_custom_instructions(true)
        .with_enable_skills(false)
        .with_memory(github_copilot_sdk::MemoryConfiguration::disabled())
        .with_disabled_mcp_servers(servers.into_iter().chain(["github-mcp-server".to_owned()]))
        .with_available_tools(["graysense_none"])
        .with_permission_handler(Arc::new(DenyAll))
        .with_system_message(github_copilot_sdk::SystemMessageConfig::new().with_mode("replace").with_content(crate::memory::CURATOR_PROMPT));
    if let Some(model) = model {
        config = config.with_model(model);
    }
    let session = client.create_session(config).await.map_err(|e| e.to_string())?;
    let id = session.id().clone();
    let opts = github_copilot_sdk::MessageOptions::new(message).with_response_schema(crate::memory::curator_schema()).with_wait_timeout(std::time::Duration::from_secs(120));
    let result = session.send_and_wait(opts).await;
    let _ = session.disconnect().await;
    let _ = client.delete_session(&id).await;
    let event = result.map_err(|e| e.to_string())?.ok_or_else(|| "the curator returned nothing".to_owned())?;
    Ok(event.data.get("content").and_then(|v| v.as_str()).unwrap_or_default().to_owned())
}
