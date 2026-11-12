//! MCP control center: configure servers, see live status, browse tools.

use std::collections::{HashMap, HashSet};

use eframe::egui::{self, Align, CornerRadius, CursorIcon, Frame, Layout, Margin, RichText, Sense, Stroke, TextEdit, Ui};
use serde_json::{Map, Value, json};

use crate::agent::BayId;
use crate::agent::backend::{McpServerInfo, McpToolInfo};
use crate::kit::{self, icon};
use crate::theme::{Theme, icon_font, mono, semibold, ui as ui_font};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Stdio,
    Http,
    Sse,
}

pub struct Editor {
    original: Option<String>,
    name: String,
    transport: Transport,
    command: String,
    args: String,
    env: String,
    cwd: String,
    url: String,
    headers: String,
    tools: String,
    timeout: String,
    error: Option<String>,
}

impl Editor {
    fn new() -> Self {
        Editor {
            original: None,
            name: String::new(),
            transport: Transport::Stdio,
            command: String::new(),
            args: String::new(),
            env: String::new(),
            cwd: String::new(),
            url: String::new(),
            headers: String::new(),
            tools: String::new(),
            timeout: String::new(),
            error: None,
        }
    }

    fn from_config(name: &str, cfg: &Value) -> Self {
        let s = |k: &str| cfg.get(k).and_then(|v| v.as_str()).unwrap_or("").to_owned();
        let lines = |k: &str, sep: &str| {
            cfg.get(k)
                .and_then(|v| v.as_object())
                .map(|m| m.iter().map(|(k, v)| format!("{k}{sep}{}", v.as_str().unwrap_or(""))).collect::<Vec<_>>().join("\n"))
                .unwrap_or_default()
        };
        let transport = match s("type").as_str() {
            "http" => Transport::Http,
            "sse" => Transport::Sse,
            _ => Transport::Stdio,
        };
        let tools = cfg
            .get("tools")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|t| t.as_str()).filter(|t| *t != "*").collect::<Vec<_>>().join(", "))
            .unwrap_or_default();
        Editor {
            original: Some(name.to_owned()),
            name: name.to_owned(),
            transport,
            command: s("command"),
            args: cfg.get("args").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default(),
            env: lines("env", "="),
            cwd: s("cwd"),
            url: s("url"),
            headers: lines("headers", ": "),
            tools,
            timeout: cfg.get("timeout").map(|t| t.to_string()).unwrap_or_default(),
            error: None,
        }
    }

    fn to_config(&mut self) -> Option<Value> {
        if self.name.trim().is_empty() || self.name.contains(char::is_whitespace) {
            self.error = Some("Name is required and cannot contain spaces.".into());
            return None;
        }
        let kv = |text: &str, sep: char| -> Map<String, Value> {
            text.lines()
                .filter_map(|l| l.split_once(sep))
                .map(|(k, v)| (k.trim().to_owned(), Value::String(v.trim().to_owned())))
                .filter(|(k, _)| !k.is_empty())
                .collect()
        };
        let tools: Vec<Value> = if self.tools.trim().is_empty() {
            vec![json!("*")]
        } else {
            self.tools.split(',').map(|t| json!(t.trim())).filter(|t| t != &json!("")).collect()
        };
        let mut cfg = Map::new();
        match self.transport {
            Transport::Stdio => {
                if self.command.trim().is_empty() {
                    self.error = Some("Command is required.".into());
                    return None;
                }
                cfg.insert("type".into(), json!("local"));
                cfg.insert("command".into(), json!(self.command.trim()));
                let args: Vec<Value> = self.args.lines().map(str::trim).filter(|a| !a.is_empty()).map(|a| json!(a)).collect();
                cfg.insert("args".into(), Value::Array(args));
                let env = kv(&self.env, '=');
                if !env.is_empty() {
                    cfg.insert("env".into(), Value::Object(env));
                }
                if !self.cwd.trim().is_empty() {
                    cfg.insert("cwd".into(), json!(self.cwd.trim()));
                }
            }
            Transport::Http | Transport::Sse => {
                if !self.url.starts_with("http") {
                    self.error = Some("A http(s) URL is required.".into());
                    return None;
                }
                cfg.insert("type".into(), json!(if self.transport == Transport::Http { "http" } else { "sse" }));
                cfg.insert("url".into(), json!(self.url.trim()));
                let headers = kv(&self.headers, ':');
                if !headers.is_empty() {
                    cfg.insert("headers".into(), Value::Object(headers));
                }
            }
        }
        cfg.insert("tools".into(), Value::Array(tools));
        if let Ok(t) = self.timeout.trim().parse::<i64>() {
            cfg.insert("timeout".into(), json!(t));
        }
        self.error = None;
        Some(Value::Object(cfg))
    }
}

pub enum McpAction {
    Refresh,
    Save { name: String, config: Value, update: bool },
    Remove(String),
    SetEnabled(String, bool),
    Tools(String),
    Restart(String),
    Stop(String),
    Start(String),
}

#[derive(Default)]
pub struct McpPanel {
    pub servers: Option<Result<Vec<McpServerInfo>, String>>,
    pub live_bay: Option<BayId>,
    pub tools: HashMap<String, Result<Vec<McpToolInfo>, String>>,
    expanded: HashSet<String>,
    editor: Option<Editor>,
    pub loading: bool,
    confirm_remove: Option<String>,
}

fn status_color(status: Option<&str>, enabled: bool, theme: &Theme) -> egui::Color32 {
    match status {
        Some("connected") => theme.ok,
        Some("failed") => theme.err,
        Some("needs-auth") => theme.warn,
        Some("pending") => theme.accent,
        _ if !enabled => theme.text_faint,
        _ => theme.text_dim,
    }
}

impl McpPanel {
    pub fn show(&mut self, ui: &mut Ui, theme: &Theme, has_agent: bool) -> Vec<McpAction> {
        let mut out = Vec::new();
        ui.horizontal(|ui| {
            ui.label(RichText::new("MCP servers").font(semibold(15.0)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if kit::icon_button(ui, icon::ADD, "Add server", theme).clicked() {
                    self.editor = Some(Editor::new());
                }
                if kit::icon_button(ui, icon::REFRESH, "Refresh", theme).clicked() {
                    out.push(McpAction::Refresh);
                }
            });
        });
        let hint = if has_agent {
            "Live status comes from the focused agent. Changes apply to new and reloaded sessions."
        } else {
            "Open an agent to see live connection status and tools."
        };
        ui.label(RichText::new(hint).font(ui_font(12.0)).color(theme.text_dim));
        ui.add_space(6.0);

        if self.editor.is_some() {
            self.editor_ui(ui, theme, &mut out);
            ui.add_space(8.0);
        }

        egui::ScrollArea::vertical().id_salt("mcp-scroll").auto_shrink([false; 2]).show(ui, |ui| {
            match &self.servers {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new("Loading servers…").color(theme.text_dim));
                    });
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(theme.err));
                }
                Some(Ok(list)) if list.is_empty() => {
                    ui.label(RichText::new("No MCP servers configured.").color(theme.text_dim));
                }
                Some(Ok(list)) => {
                    let list = list.clone();
                    let mut last_source = String::new();
                    for s in &list {
                        if s.source != last_source {
                            kit::section(ui, &s.source, theme);
                            last_source = s.source.clone();
                        }
                        self.server_row(ui, s, theme, has_agent, &mut out);
                        ui.add_space(4.0);
                    }
                }
            }
            ui.add_space(10.0);
            kit::section(ui, "Quick add", theme);
            if ui
                .add(egui::Label::new(RichText::new(format!("{}  MCP Apps demo server (interactive UI)", icon::GLOBE)).color(theme.accent)).sense(Sense::click()))
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_text("npx -y @modelcontextprotocol/server-basic-vanillajs --stdio")
                .clicked()
            {
                let mut e = Editor::new();
                e.name = "mcp-apps-demo".into();
                e.command = "cmd".into();
                e.args = "/c\nnpx\n-y\n@modelcontextprotocol/server-basic-vanillajs\n--stdio".into();
                self.editor = Some(e);
            }
        });
        out
    }

    fn server_row(&mut self, ui: &mut Ui, s: &McpServerInfo, theme: &Theme, has_agent: bool, out: &mut Vec<McpAction>) {
        let open = self.expanded.contains(&s.name);
        Frame::new().fill(theme.surface2).stroke(Stroke::new(1.0, theme.border)).corner_radius(CornerRadius::same(8)).inner_margin(Margin::symmetric(10, 7)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let row = ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 18.0), Sense::hover());
                kit::dot(ui.painter(), r.center(), 3.5, status_color(s.status.as_deref(), s.enabled, theme));
                ui.label(RichText::new(&s.name).font(semibold(13.0)).color(if s.enabled { theme.text } else { theme.text_faint }));
                if let Some(st) = &s.status {
                    ui.label(RichText::new(st).font(ui_font(11.5)).color(theme.text_faint));
                }
                if let Some(p) = &s.plugin {
                    ui.label(RichText::new(format!("{} {p}", icon::PUZZLE)).font(ui_font(11.5)).color(theme.text_faint));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(if open { icon::CHEVRON_UP } else { icon::CHEVRON_DOWN }).font(icon_font(9.0)).color(theme.text_faint));
                    if s.source == "user" || s.source == "workspace" {
                        let mut on = s.enabled;
                        if kit::toggle_named(ui, &mut on, theme, &format!("Enable {}", s.name)).on_hover_text(if on { "Disable" } else { "Enable" }).changed() {
                            out.push(McpAction::SetEnabled(s.name.clone(), on));
                        }
                    }
                });
            });
            let row_resp = row.response.interact(Sense::click()).on_hover_cursor(CursorIcon::PointingHand);
            row_resp.widget_info(|| {
                let status = s.status.clone().unwrap_or_else(|| if s.enabled { "configured".into() } else { "disabled".into() });
                egui::WidgetInfo::selected(egui::Role::Button, true, open, format!("{} ({status})", s.name))
            });
            if row_resp.clicked() {
                if open {
                    self.expanded.remove(&s.name);
                } else {
                    self.expanded.insert(s.name.clone());
                    if has_agent && !self.tools.contains_key(&s.name) {
                        out.push(McpAction::Tools(s.name.clone()));
                    }
                }
            }
            if !open {
                return;
            }
            if let Some(err) = &s.error {
                ui.label(RichText::new(err).font(ui_font(12.0)).color(theme.err));
            }
            if let Some(cfg) = &s.config {
                let summary = cfg
                    .get("url")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
                    .or_else(|| {
                        let cmd = cfg.get("command").and_then(|v| v.as_str())?;
                        let args = cfg.get("args").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(" ")).unwrap_or_default();
                        Some(format!("{cmd} {args}"))
                    })
                    .unwrap_or_default();
                ui.label(RichText::new(summary).font(mono(11.5)).color(theme.text_dim));
            }
            ui.horizontal_wrapped(|ui| {
                if has_agent {
                    if kit::mini_button(ui, "Tools", theme).clicked() {
                        out.push(McpAction::Tools(s.name.clone()));
                    }
                    if kit::mini_button(ui, "Restart", theme).clicked() {
                        out.push(McpAction::Restart(s.name.clone()));
                    }
                    if s.status.as_deref() == Some("connected") {
                        if kit::mini_button(ui, "Stop", theme).clicked() {
                            out.push(McpAction::Stop(s.name.clone()));
                        }
                    } else if kit::mini_button(ui, "Start", theme).clicked() {
                        out.push(McpAction::Start(s.name.clone()));
                    }
                }
                if let Some(cfg) = &s.config {
                    if kit::mini_button(ui, "Edit", theme).clicked() {
                        self.editor = Some(Editor::from_config(&s.name, cfg));
                    }
                    if self.confirm_remove.as_deref() == Some(s.name.as_str()) {
                        if kit::mini_button_tinted(ui, "Confirm remove", theme, theme.err).clicked() {
                            out.push(McpAction::Remove(s.name.clone()));
                            self.confirm_remove = None;
                        }
                    } else if kit::mini_button(ui, "Remove", theme).clicked() {
                        self.confirm_remove = Some(s.name.clone());
                    }
                }
            });
            match self.tools.get(&s.name) {
                Some(Ok(tools)) => {
                    ui.add_space(2.0);
                    for t in tools {
                        ui.horizontal_top(|ui| {
                            ui.label(RichText::new(&t.name).font(mono(11.5)).color(theme.text));
                            if t.has_ui {
                                ui.label(RichText::new("UI").font(semibold(10.5)).color(theme.accent)).on_hover_text("This tool renders an interactive MCP App");
                            }
                        });
                        if let Some(d) = &t.description {
                            ui.add(egui::Label::new(RichText::new(d).font(ui_font(11.5)).color(theme.text_faint)).truncate());
                        }
                    }
                    if tools.is_empty() {
                        ui.label(RichText::new("No tools").color(theme.text_faint));
                    }
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).font(ui_font(11.5)).color(theme.err));
                }
                None => {}
            }
        });
    }

    fn editor_ui(&mut self, ui: &mut Ui, theme: &Theme, out: &mut Vec<McpAction>) {
        let Some(ed) = self.editor.as_mut() else { return };
        let mut close = false;
        Frame::new().fill(theme.surface2).stroke(Stroke::new(1.0, theme.accent)).corner_radius(CornerRadius::same(8)).inner_margin(Margin::symmetric(12, 10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(if ed.original.is_some() { "Edit server" } else { "Add server" }).font(semibold(13.0)));
            let field = |ui: &mut Ui, label: &str, value: &mut String, hint: &str, multiline: bool| {
                ui.label(RichText::new(label).font(ui_font(11.5)).color(theme.text_dim));
                let edit = if multiline { TextEdit::multiline(value).desired_rows(2) } else { TextEdit::singleline(value) };
                ui.add(edit.hint_text(hint).desired_width(f32::INFINITY).font(mono(12.0)));
            };
            field(ui, "Name", &mut ed.name, "my-server", false);
            ui.horizontal(|ui| {
                for (t, label) in [(Transport::Stdio, "Local (stdio)"), (Transport::Http, "HTTP"), (Transport::Sse, "SSE")] {
                    if ui.selectable_label(ed.transport == t, label).clicked() {
                        ed.transport = t;
                    }
                }
            });
            match ed.transport {
                Transport::Stdio => {
                    field(ui, "Command", &mut ed.command, "npx", false);
                    field(ui, "Arguments (one per line)", &mut ed.args, "-y\n@scope/server", true);
                    field(ui, "Environment (KEY=value per line)", &mut ed.env, "API_KEY=…", true);
                    field(ui, "Working directory (optional)", &mut ed.cwd, "", false);
                }
                Transport::Http | Transport::Sse => {
                    field(ui, "URL", &mut ed.url, "https://example.com/mcp", false);
                    field(ui, "Headers (Name: value per line)", &mut ed.headers, "Authorization: Bearer …", true);
                }
            }
            field(ui, "Tools (comma-separated, empty = all)", &mut ed.tools, "", false);
            field(ui, "Timeout ms (optional)", &mut ed.timeout, "", false);
            if let Some(e) = &ed.error {
                ui.label(RichText::new(e).color(theme.err).font(ui_font(12.0)));
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if kit::primary_button(ui, "Save", theme).clicked()
                    && let Some(config) = ed.to_config() {
                        let update = ed.original.as_deref() == Some(ed.name.trim());
                        if let Some(orig) = ed.original.clone().filter(|o| o != ed.name.trim()) {
                            out.push(McpAction::Remove(orig));
                        }
                        out.push(McpAction::Save { name: ed.name.trim().to_owned(), config, update });
                        close = true;
                    }
                if kit::ghost_button(ui, "Cancel", theme).clicked() {
                    close = true;
                }
            });
        });
        if close {
            self.editor = None;
        }
    }
}
