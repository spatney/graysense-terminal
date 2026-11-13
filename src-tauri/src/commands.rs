//! The commands the UI invokes. Core actions run off the main thread (`async` commands): the
//! core's thread makes window calls that wait for the main thread.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{Value, json};
use tauri::ipc::Channel;
use tauri::State;

use crate::agent::BayId;
use crate::engine::actions::Action;
use crate::engine::dto::{UiUpdate, utf16_ranges};
use crate::engine::{Core, terminal_id};
use crate::terminal::{Launch, TerminalEvent, TerminalInfo, TerminalShell, Terminals, shells};

pub struct CoreState(pub Arc<Mutex<Core>>);

/// Everything the UI shows, for a UI that's starting (or reloading).
#[tauri::command(async)]
pub fn snapshot(core: State<'_, CoreState>) -> UiUpdate {
    let (pending, full) = core.0.lock().snapshot();
    pending.wait();
    full
}

/// Run one action; what it changed reaches the UI (as an update) before the result does.
#[tauri::command(async)]
pub fn act(core: State<'_, CoreState>, action: Action) -> Result<Value, String> {
    let (result, out) = {
        let mut c = core.0.lock();
        let result = c.act(action);
        (result, c.send_out())
    };
    out.wait();
    result
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub kind: &'static str,
    pub session_id: Option<String>,
    pub path: Option<String>,
    pub title: String,
    pub location: String,
    pub modified: i64,
    pub snippet: String,
    pub snippet_ranges: Vec<(usize, usize)>,
    pub title_ranges: Vec<(usize, usize)>,
    pub line: Option<usize>,
}

/// Search sessions and the files in session folders (an empty query lists recent ones).
#[tauri::command(async)]
pub fn search(core: State<'_, CoreState>, query: String, limit: Option<usize>) -> Vec<SearchHit> {
    let index = {
        let mut c = core.0.lock();
        if query.trim().is_empty() {
            c.refresh_search_index(false);
        }
        c.search_index()
    };
    index
        .search(&query, limit.unwrap_or(40).min(200))
        .into_iter()
        .map(|hit| {
            let doc = index.doc(&hit);
            let (kind, session_id, path) = match &doc.kind {
                crate::search::DocKind::Session(id) => ("session", Some(id.clone()), None),
                crate::search::DocKind::File(p) => ("file", None, Some(p.display().to_string())),
            };
            SearchHit {
                kind,
                session_id,
                path,
                title: doc.title.clone(),
                location: doc.location.clone(),
                modified: doc.modified,
                snippet_ranges: utf16_ranges(&hit.snippet, &hit.snippet_ranges),
                title_ranges: utf16_ranges(&doc.title, &hit.title_ranges),
                snippet: hit.snippet,
                line: hit.line,
            }
        })
        .collect()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppViewDto {
    pub id: u64,
    pub bay: BayId,
    pub title: String,
    pub server: String,
    pub tool: String,
    pub tool_call_id: String,
    pub html: String,
    pub meta: Value,
    pub args: Value,
    /// The tool's result as an MCP `CallToolResult`.
    pub result: Value,
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

/// An MCP App's page and what it was opened with.
#[tauri::command(async)]
pub fn app_view(core: State<'_, CoreState>, view: u64) -> Result<AppViewDto, String> {
    let c = core.0.lock();
    let a = c.app_view(view).ok_or("That app is closed.")?;
    Ok(AppViewDto {
        id: a.id,
        bay: a.bay,
        title: a.title.clone(),
        server: a.server.clone(),
        tool: a.tool.clone(),
        tool_call_id: a.tool_call_id.clone(),
        html: a.html.clone(),
        meta: a.meta.clone(),
        args: a.args.clone(),
        result: call_tool_result(&a.result),
    })
}

#[tauri::command(async)]
pub fn list_dir(path: String) -> Result<Vec<crate::util::DirEntry>, String> {
    crate::util::list_dir(Path::new(&path))
}

#[tauri::command(async)]
pub fn complete_path(directory: String, prefix: String) -> Result<Vec<crate::util::DirEntry>, String> {
    crate::util::complete_path(Path::new(&directory), &prefix)
}

/// Keep an image pasted into a message box (the request body is its bytes); returns its path, to
/// attach.
#[tauri::command(async)]
pub fn save_pasted_image(request: tauri::ipc::Request<'_>) -> Result<String, String> {
    let bytes: std::borrow::Cow<[u8]> = match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => bytes.into(),
        // Without the IPC protocol (its postMessage fallback), bytes arrive as a JSON array.
        tauri::ipc::InvokeBody::Json(value) => <Vec<u8> as serde::Deserialize>::deserialize(value).map_err(|_| "That isn't an image.")?.into(),
    };
    let dir = crate::settings::config_dir().join("pasted-images");
    crate::util::save_pasted_image(&dir, &bytes).map(|p| p.display().to_string())
}

/// The start of a text file, for a preview (`None` for binaries).
#[tauri::command(async)]
pub fn read_text(path: String) -> Result<Option<String>, String> {
    crate::util::read_text_head(Path::new(&path), 256 * 1024)
}

// ------------------------------------------------------------------ terminals

#[tauri::command(async)]
pub fn terminal_shells(core: State<'_, CoreState>) -> Vec<TerminalShell> {
    let custom = core.0.lock().settings.shell.clone();
    shells::available(custom.as_deref()).iter().enumerate().map(|(i, s)| s.describe(i == 0)).collect()
}

/// Start (or restart) a terminal bay's shell, streaming what it prints to `on_event`.
#[tauri::command(async)]
pub fn terminal_spawn(core: State<'_, CoreState>, terminals: State<'_, Terminals>, bay: BayId, cols: u16, rows: u16, on_event: Channel<TerminalEvent>) -> Result<TerminalInfo, String> {
    let (cwd, shell, custom) = {
        let c = core.0.lock();
        let (cwd, shell) = c.shell_bay(bay).ok_or("That terminal is closed.")?;
        (cwd, shell, c.settings.shell.clone())
    };
    let cwd = if cwd.is_dir() { cwd } else { dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")) };
    let shell = shells::resolve(shell.as_deref(), custom.as_deref()).ok_or("Graysense couldn't find a shell on this computer.")?;
    terminals.spawn(Launch { id: terminal_id(bay), cwd, shell, cols, rows }, on_event)
}

/// Type into a terminal. Not async, so keystrokes arrive in order.
#[tauri::command]
pub fn terminal_write(terminals: State<'_, Terminals>, bay: BayId, data: String) -> Result<(), String> {
    terminals.write(&terminal_id(bay), &data)
}

#[tauri::command]
pub fn terminal_resize(terminals: State<'_, Terminals>, bay: BayId, cols: u16, rows: u16) -> Result<(), String> {
    terminals.resize(&terminal_id(bay), cols, rows)
}

/// Hold a terminal's output while xterm.js catches up, or let it flow.
#[tauri::command]
pub fn terminal_pause(terminals: State<'_, Terminals>, bay: BayId, paused: bool) -> Result<(), String> {
    terminals.set_paused(&terminal_id(bay), paused)
}

#[tauri::command(async)]
pub fn terminal_kill(terminals: State<'_, Terminals>, bay: BayId) {
    terminals.kill(&terminal_id(bay));
}

// ------------------------------------------------------------------ window

fn parse_hex_color(value: &str) -> Option<tauri::window::Color> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).ok();
    Some(tauri::window::Color(channel(0)?, channel(2)?, channel(4)?, 255))
}

/// Show the main window. It starts hidden; the UI calls this once its first screen is painted,
/// so a launch never flashes an empty window or the wrong theme.
#[tauri::command]
pub fn window_reveal(window: tauri::Window, background: Option<String>) {
    if let Some(color) = background.as_deref().and_then(parse_hex_color) {
        let _ = window.set_background_color(Some(color));
    }
    reveal(&window);
}

pub fn reveal(window: &tauri::Window) {
    if window.is_visible().unwrap_or(true) {
        return;
    }
    let _ = window.show();
    let _ = window.set_focus();
}

/// Paint the native title bar and frame in the theme's colours.
#[tauri::command]
pub fn window_theme(window: tauri::Window, caption: String, text: String, border: String, dark: bool) {
    if let Some(color) = parse_hex_color(&caption) {
        let _ = window.set_background_color(Some(color));
    }
    crate::win::apply_theme(&window, &caption, &text, &border, dark);
}
