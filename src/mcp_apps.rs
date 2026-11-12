//! MCP Apps host (SEP-1865): renders interactive HTML UIs shipped by MCP servers in
//! WebView2 child windows docked inside agent bays, and bridges their JSON-RPC.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use base64::Engine;
use eframe::egui;
use parking_lot::Mutex;
use serde_json::{Value, json};
use wry::dpi::{PhysicalPosition, PhysicalSize};
use wry::http::{Request, Response};
use wry::{NewWindowResponse, WebView, WebViewBuilder};

use crate::agent::BayId;
use crate::theme::Theme;

const PROTOCOL_VERSION: &str = "2026-01-26";

/// Runs before page scripts. The stock View transport posts to `window.parent`, which is
/// `window` itself in a top-level WebView; route JSON-RPC to the host instead.
const SHIM: &str = r#"(() => {
  if (window !== window.top || window.__gsBridge) return; window.__gsBridge = true;
  const nativePost = window.postMessage.bind(window);
  window.postMessage = (msg, targetOrigin, transfer) => {
    if (msg && msg.jsonrpc === "2.0") { window.ipc.postMessage(JSON.stringify(msg)); return; }
    return nativePost(msg, targetOrigin, transfer);
  };
  window.__mcpDeliver = (msg) => nativePost(msg, "*");
})();"#;

struct Doc {
    html: String,
    csp: String,
}

pub struct AppView {
    pub id: u64,
    pub bay: BayId,
    pub title: String,
    pub server: String,
    pub tool: String,
    pub tool_call_id: String,
    args: Value,
    result: Value,
    webview: Option<WebView>,
    failed: Option<String>,
    initialized: bool,
    last_bounds: Option<(i32, i32, u32, u32)>,
    last_visible: Option<bool>,
}

/// Requests from views that the app must route to the agent backend / OS.
pub enum AppRequest {
    CallTool { bay: BayId, view: u64, req_id: Value, server: String, tool: String, args: Option<Value> },
    ReadResource { bay: BayId, view: u64, req_id: Value, server: String, uri: String },
    Message { bay: BayId, text: String },
    OpenUrl(String),
    Log(String),
}

pub struct AppHost {
    views: Vec<AppView>,
    next: u64,
    inbox: Rc<RefCell<Vec<(u64, String)>>>,
    docs: Arc<Mutex<HashMap<u64, Doc>>>,
}

fn sanitize(list: Option<&Value>) -> String {
    list.and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .filter(|s| !s.chars().any(|c| matches!(c, ';' | '\r' | '\n' | '\'' | '"' | ' ')))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

fn build_csp(meta: Option<&Value>) -> String {
    let csp = meta.and_then(|m| m.pointer("/ui/csp"));
    let res = sanitize(csp.and_then(|c| c.get("resourceDomains")));
    let connect = sanitize(csp.and_then(|c| c.get("connectDomains")));
    let frames = sanitize(csp.and_then(|c| c.get("frameDomains")));
    let base = sanitize(csp.and_then(|c| c.get("baseUriDomains")));
    format!(
        "default-src 'none'; script-src 'self' 'unsafe-inline' {res}; style-src 'self' 'unsafe-inline' {res}; \
         connect-src 'self' {connect}; img-src 'self' data: blob: {res}; font-src 'self' data: {res}; media-src 'self' data: {res}; \
         frame-src {}; object-src 'none'; base-uri {}; sandbox allow-scripts allow-same-origin allow-forms",
        if frames.is_empty() { "'none'".to_owned() } else { frames },
        if base.is_empty() { "'self'".to_owned() } else { base },
    )
}

fn serve(docs: &Mutex<HashMap<u64, Doc>>, req: &Request<Vec<u8>>) -> Response<Cow<'static, [u8]>> {
    let path = req.uri().path().to_owned();
    let id = path.trim_start_matches("/view/").parse::<u64>().ok();
    let docs = docs.lock();
    match id.and_then(|id| docs.get(&id)) {
        Some(doc) => Response::builder()
            .header("Content-Type", "text/html; charset=utf-8")
            .header("Content-Security-Policy", doc.csp.as_str())
            .header("Cache-Control", "no-store")
            .body(Cow::Owned(doc.html.clone().into_bytes()))
            .unwrap(),
        None => Response::builder().status(404).body(Cow::Borrowed(&b"not found"[..])).unwrap(),
    }
}

/// Convert a Copilot `tool.execution_complete` result into an MCP `CallToolResult`.
fn call_tool_result(result: &Value) -> Value {
    let content = match result.get("contents").and_then(|c| c.as_array()) {
        Some(blocks) if !blocks.is_empty() => Value::Array(blocks.clone()),
        _ => json!([{ "type": "text", "text": result.get("content").and_then(|c| c.as_str()).unwrap_or("") }]),
    };
    let mut out = json!({ "content": content });
    if let Some(sc) = result.get("structuredContent").filter(|v| !v.is_null()) {
        out["structuredContent"] = sc.clone();
    }
    if let Some(meta) = result.get("mcpResultMeta").filter(|v| !v.is_null()) {
        out["_meta"] = meta.clone();
    }
    out
}

impl AppHost {
    pub fn new() -> Self {
        AppHost { views: Vec::new(), next: 1, inbox: Rc::new(RefCell::new(Vec::new())), docs: Arc::new(Mutex::new(HashMap::new())) }
    }

    /// Register a new view from a tool result's `uiResource`. Returns the view id.
    pub fn open(&mut self, bay: BayId, req: &crate::agent::AppRequest) -> Option<u64> {
        let res = &req.resource;
        let html = if let Some(text) = res.get("text").and_then(|t| t.as_str()) {
            text.to_owned()
        } else {
            let blob = res.get("blob").and_then(|b| b.as_str())?;
            String::from_utf8(base64::engine::general_purpose::STANDARD.decode(blob).ok()?).ok()?
        };
        let id = self.next;
        self.next += 1;
        self.docs.lock().insert(id, Doc { html, csp: build_csp(res.get("_meta")) });
        let server = req.server.clone().unwrap_or_default();
        let label = req.label.clone().unwrap_or_else(|| req.tool.strip_prefix(&format!("{server}-")).unwrap_or(&req.tool).to_owned());
        let title = if server.is_empty() { label } else { format!("{label} · {server}") };
        self.views.push(AppView {
            id,
            bay,
            title,
            server,
            tool: req.tool.clone(),
            tool_call_id: req.tool_call_id.clone(),
            args: req.args.clone(),
            result: req.result.clone(),
            webview: None,
            failed: None,
            initialized: false,
            last_bounds: None,
            last_visible: None,
        });
        Some(id)
    }

    pub fn title(&self, id: u64) -> Option<String> {
        self.views.iter().find(|v| v.id == id).map(|v| v.title.clone())
    }

    pub fn close_bay(&mut self, bay: BayId) {
        let ids: Vec<u64> = self.views.iter().filter(|v| v.bay == bay).map(|v| v.id).collect();
        for id in ids {
            self.close(id);
        }
    }

    pub fn close(&mut self, id: u64) {
        if let Some(pos) = self.views.iter().position(|v| v.id == id) {
            let v = self.views.remove(pos);
            if let Some(wv) = &v.webview {
                let _ = wv.evaluate_script(r#"window.__mcpDeliver({"jsonrpc":"2.0","id":"teardown","method":"ui/resource-teardown","params":{}})"#);
                let _ = wv.set_visible(false);
            }
            self.docs.lock().remove(&id);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.views.is_empty()
    }

    /// Position the visible views; everything else is hidden. Creates webviews lazily.
    pub fn layout(&mut self, frame: &eframe::Frame, ctx: &egui::Context, placements: &[(u64, egui::Rect)], suppress: bool, theme: &Theme) {
        let ppp = ctx.pixels_per_point();
        for v in self.views.iter_mut() {
            let place = placements.iter().find(|(id, _)| *id == v.id).map(|(_, r)| *r);
            let visible = place.is_some() && !suppress;
            if visible && v.webview.is_none() && v.failed.is_none() {
                let inbox = self.inbox.clone();
                let docs = self.docs.clone();
                let ctx2 = ctx.clone();
                let id = v.id;
                let bg = theme.surface;
                let built = {
                    use wry::WebViewBuilderExtWindows;
                    WebViewBuilder::new()
                        .with_focused(false)
                        .with_visible(false)
                        .with_bounds(wry::Rect { position: PhysicalPosition::new(0, 0).into(), size: PhysicalSize::new(1u32, 1u32).into() })
                        .with_background_color((bg.r(), bg.g(), bg.b(), 255))
                        .with_devtools(cfg!(debug_assertions))
                        .with_incognito(true)
                        .with_default_context_menus(false)
                        .with_browser_accelerator_keys(false)
                        .with_initialization_script(SHIM)
                        .with_custom_protocol("gsapp".into(), move |_id, req| serve(&docs, &req))
                        .with_ipc_handler(move |req: Request<String>| {
                            let body = req.into_body();
                            if body.len() < 4 * 1024 * 1024 {
                                inbox.borrow_mut().push((id, body));
                                ctx2.request_repaint();
                            }
                        })
                        .with_navigation_handler(|url| url.starts_with("http://gsapp.localhost/") || url.starts_with("gsapp://") || url == "about:blank")
                        .with_new_window_req_handler(|_url, _features| NewWindowResponse::Deny)
                        .with_download_started_handler(|_url, _path| false)
                        .with_url(format!("gsapp://localhost/view/{id}"))
                        .build_as_child(frame)
                };
                match built {
                    Ok(wv) => v.webview = Some(wv),
                    Err(e) => {
                        tracing::error!("webview creation failed: {e}");
                        v.failed = Some(e.to_string());
                    }
                }
            }
            let Some(wv) = &v.webview else { continue };
            if let Some(r) = place.filter(|_| visible) {
                let b = ((r.min.x * ppp).round() as i32, (r.min.y * ppp).round() as i32, (r.width() * ppp).round().max(1.0) as u32, (r.height() * ppp).round().max(1.0) as u32);
                if v.last_bounds != Some(b) {
                    let _ = wv.set_bounds(wry::Rect { position: PhysicalPosition::new(b.0, b.1).into(), size: PhysicalSize::new(b.2, b.3).into() });
                    v.last_bounds = Some(b);
                }
            }
            if v.last_visible != Some(visible) {
                let _ = wv.set_visible(visible);
                v.last_visible = Some(visible);
            }
        }
    }

    /// Return keyboard focus to the main window (call when the user clicks native UI).
    pub fn release_focus(&self) {
        for v in &self.views {
            if v.last_visible == Some(true)
                && let Some(wv) = &v.webview {
                    let _ = wv.focus_parent();
                }
        }
    }

    pub fn failure(&self, id: u64) -> Option<String> {
        self.views.iter().find(|v| v.id == id).and_then(|v| v.failed.clone())
    }

    fn deliver(&self, id: u64, msg: &Value) {
        if let Some(wv) = self.views.iter().find(|v| v.id == id).and_then(|v| v.webview.as_ref()) {
            let _ = wv.evaluate_script(&format!("window.__mcpDeliver({msg})"));
        }
    }

    pub fn reply(&self, id: u64, req_id: Value, result: Result<Value, String>) {
        let msg = match result {
            Ok(r) => json!({ "jsonrpc": "2.0", "id": req_id, "result": r }),
            Err(e) => json!({ "jsonrpc": "2.0", "id": req_id, "error": { "code": -32000, "message": e } }),
        };
        self.deliver(id, &msg);
    }

    /// Process queued view → host messages.
    pub fn pump(&mut self, theme: &Theme) -> Vec<AppRequest> {
        let msgs: Vec<(u64, String)> = self.inbox.borrow_mut().drain(..).collect();
        let mut out = Vec::new();
        for (id, body) in msgs {
            let Ok(msg) = serde_json::from_str::<Value>(&body) else { continue };
            if msg.get("jsonrpc").and_then(|v| v.as_str()) != Some("2.0") {
                continue;
            }
            let Some(view) = self.views.iter().find(|v| v.id == id) else { continue };
            let (bay, server) = (view.bay, view.server.clone());
            let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("").to_owned();
            let req_id = msg.get("id").cloned();
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            match (method.as_str(), req_id) {
                ("ui/initialize", Some(rid)) => {
                    let ctx = json!({
                        "theme": if theme.dark { "dark" } else { "light" },
                        "platform": "desktop",
                        "displayMode": "inline",
                        "availableDisplayModes": ["inline"],
                        "locale": "en-US",
                        "userAgent": format!("Graysense/{}", env!("CARGO_PKG_VERSION")),
                        "toolInfo": { "id": view.tool_call_id, "tool": { "name": view.tool, "inputSchema": { "type": "object" } } },
                        "styles": { "variables": style_vars(theme) }
                    });
                    let result = json!({
                        "protocolVersion": params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or(PROTOCOL_VERSION),
                        "hostInfo": { "name": "Graysense", "version": env!("CARGO_PKG_VERSION") },
                        "hostCapabilities": { "openLinks": {}, "serverTools": {}, "serverResources": {}, "logging": {}, "message": { "text": {} }, "updateModelContext": { "text": {} } },
                        "hostContext": ctx
                    });
                    self.reply(id, rid, Ok(result));
                }
                ("ui/notifications/initialized", None) => {
                    if let Some(v) = self.views.iter_mut().find(|v| v.id == id) {
                        v.initialized = true;
                    }
                    let (args, result) = {
                        let v = self.views.iter().find(|v| v.id == id).unwrap();
                        (v.args.clone(), call_tool_result(&v.result))
                    };
                    self.deliver(id, &json!({ "jsonrpc": "2.0", "method": "ui/notifications/tool-input", "params": { "arguments": args } }));
                    self.deliver(id, &json!({ "jsonrpc": "2.0", "method": "ui/notifications/tool-result", "params": result }));
                }
                ("tools/call", Some(rid)) => {
                    let tool = params.get("name").and_then(|v| v.as_str()).unwrap_or("").to_owned();
                    out.push(AppRequest::CallTool { bay, view: id, req_id: rid, server, tool, args: params.get("arguments").cloned() });
                }
                ("resources/read", Some(rid)) => {
                    let uri = params.get("uri").and_then(|v| v.as_str()).unwrap_or("").to_owned();
                    out.push(AppRequest::ReadResource { bay, view: id, req_id: rid, server, uri });
                }
                ("ui/message", Some(rid)) => {
                    let blocks = match params.get("content") {
                        Some(Value::Array(a)) => a.clone(),
                        Some(other) => vec![other.clone()],
                        None => Vec::new(),
                    };
                    let text = blocks.iter().filter_map(|b| b.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join("\n");
                    if !text.trim().is_empty() {
                        out.push(AppRequest::Message { bay, text });
                    }
                    self.reply(id, rid, Ok(json!({})));
                }
                ("ui/open-link", Some(rid)) => {
                    if let Some(url) = params.get("url").and_then(|v| v.as_str()).filter(|u| u.starts_with("https://") || u.starts_with("http://")) {
                        out.push(AppRequest::OpenUrl(url.to_owned()));
                        self.reply(id, rid, Ok(json!({})));
                    } else {
                        self.reply(id, rid, Err("only http(s) links are allowed".into()));
                    }
                }
                ("ui/request-display-mode", Some(rid)) => self.reply(id, rid, Ok(json!({ "mode": "inline" }))),
                ("ui/update-model-context", Some(rid)) | ("ping", Some(rid)) => self.reply(id, rid, Ok(json!({}))),
                ("notifications/message", None) => {
                    let data = params.get("data").map(|d| d.as_str().map(str::to_owned).unwrap_or_else(|| d.to_string())).unwrap_or_default();
                    out.push(AppRequest::Log(format!("[{}] {data}", server)));
                }
                ("ui/notifications/size-changed", None) | ("ui/notifications/request-teardown", None) => {}
                (_, Some(rid)) if !method.is_empty() => {
                    let msg = json!({ "jsonrpc": "2.0", "id": rid, "error": { "code": -32601, "message": format!("method not supported: {method}") } });
                    self.deliver(id, &msg);
                }
                _ => {}
            }
        }
        out
    }
}

fn hex(c: egui::Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

fn style_vars(t: &Theme) -> Value {
    json!({
        "--color-background-primary": hex(t.surface),
        "--color-background-secondary": hex(t.surface2),
        "--color-background-tertiary": hex(t.surface3),
        "--color-text-primary": hex(t.text),
        "--color-text-secondary": hex(t.text_dim),
        "--color-text-tertiary": hex(t.text_faint),
        "--color-border-primary": hex(t.border_strong),
        "--color-border-secondary": hex(t.border),
        "--color-ring-primary": hex(t.accent),
        "--color-text-info": hex(t.accent),
        "--color-text-success": hex(t.ok),
        "--color-text-warning": hex(t.warn),
        "--color-text-danger": hex(t.err),
        "--font-sans": "'Segoe UI Variable Text', 'Segoe UI', system-ui, sans-serif",
        "--font-mono": "'Cascadia Mono', Consolas, monospace",
        "--border-radius-md": "8px"
    })
}
