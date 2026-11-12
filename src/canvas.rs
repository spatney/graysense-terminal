//! Native agent canvases: UI surfaces declared to the Copilot runtime that agents can
//! open and drive (tables, checklists, dashboards, documents), rendered natively.

use eframe::egui::{self, Align2, Color32, CornerRadius, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use github_copilot_sdk::canvas::CanvasDeclaration;
use github_copilot_sdk::rpc::CanvasAction;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::agent::markdown;
use crate::kit::{self, icon};
use crate::theme::{Theme, semibold, ui as ui_font};

fn action(name: &str, description: &str, schema: Value) -> CanvasAction {
    CanvasAction { name: name.into(), description: Some(description.into()), input_schema: Some(schema) }
}

fn decl(id: &str, name: &str, description: &str, schema: Value, actions: Vec<CanvasAction>) -> CanvasDeclaration {
    let mut d = CanvasDeclaration::new(id, name, description);
    d.input_schema = Some(schema);
    d.actions = Some(actions);
    d
}

pub fn declarations() -> Vec<CanvasDeclaration> {
    let get_state = || action("get_state", "Read the canvas's current state, including any edits the user made.", json!({"type": "object", "properties": {}}));
    vec![
        decl(
            "table",
            "Table",
            "A sortable data table shown beside the conversation. Use it for comparisons, inventories or results the user should scan.",
            json!({"type": "object", "properties": {
                "title": {"type": "string"},
                "columns": {"type": "array", "items": {"type": "string"}},
                "rows": {"type": "array", "items": {"type": "array"}}
            }, "required": ["columns", "rows"]}),
            vec![
                action("set_rows", "Replace all rows.", json!({"type": "object", "properties": {"rows": {"type": "array", "items": {"type": "array"}}}, "required": ["rows"]})),
                action("append_rows", "Append rows to the end.", json!({"type": "object", "properties": {"rows": {"type": "array", "items": {"type": "array"}}}, "required": ["rows"]})),
                get_state(),
            ],
        ),
        decl(
            "checklist",
            "Checklist",
            "An interactive checklist the user can tick off (plans, review steps, migration tasks). Read it back with get_state.",
            json!({"type": "object", "properties": {
                "title": {"type": "string"},
                "items": {"type": "array", "items": {"type": "object", "properties": {
                    "text": {"type": "string"}, "done": {"type": "boolean"}, "note": {"type": "string"}
                }, "required": ["text"]}}
            }, "required": ["items"]}),
            vec![
                action("add_item", "Add an item.", json!({"type": "object", "properties": {"text": {"type": "string"}, "note": {"type": "string"}}, "required": ["text"]})),
                action("set_done", "Mark an item (0-based index) done or not done.", json!({"type": "object", "properties": {"index": {"type": "integer"}, "done": {"type": "boolean"}}, "required": ["index", "done"]})),
                get_state(),
            ],
        ),
        decl(
            "dashboard",
            "Dashboard",
            "Metric tiles and simple line/bar charts for live progress, benchmarks or telemetry.",
            json!({"type": "object", "properties": {
                "title": {"type": "string"},
                "metrics": {"type": "array", "items": {"type": "object", "properties": {
                    "label": {"type": "string"}, "value": {"type": "string"}, "delta": {"type": "string"},
                    "trend": {"type": "string", "enum": ["up", "down", "flat"]}
                }, "required": ["label", "value"]}},
                "charts": {"type": "array", "items": {"type": "object", "properties": {
                    "title": {"type": "string"}, "kind": {"type": "string", "enum": ["line", "bar"]},
                    "labels": {"type": "array", "items": {"type": "string"}},
                    "series": {"type": "array", "items": {"type": "object", "properties": {
                        "name": {"type": "string"}, "values": {"type": "array", "items": {"type": "number"}}
                    }, "required": ["name", "values"]}}
                }, "required": ["title", "series"]}}
            }}),
            vec![
                action("update", "Replace metrics and/or charts (omitted keys are kept).", json!({"type": "object", "properties": {"metrics": {"type": "array"}, "charts": {"type": "array"}}})),
                action("push_point", "Append a value to a chart series.", json!({"type": "object", "properties": {"chart": {"type": "integer"}, "series": {"type": "integer"}, "value": {"type": "number"}, "label": {"type": "string"}}, "required": ["chart", "series", "value"]})),
                get_state(),
            ],
        ),
        decl(
            "document",
            "Document",
            "A markdown document panel for specs, reports, release notes or design docs the user reads alongside the chat.",
            json!({"type": "object", "properties": {"title": {"type": "string"}, "markdown": {"type": "string"}}, "required": ["markdown"]}),
            vec![
                action("set_markdown", "Replace the document body.", json!({"type": "object", "properties": {"markdown": {"type": "string"}}, "required": ["markdown"]})),
                action("append", "Append markdown to the end.", json!({"type": "object", "properties": {"markdown": {"type": "string"}}, "required": ["markdown"]})),
                get_state(),
            ],
        ),
    ]
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct TableData {
    #[serde(default)]
    pub title: Option<String>,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct CheckItem {
    pub text: String,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ChecklistData {
    #[serde(default)]
    pub title: Option<String>,
    pub items: Vec<CheckItem>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Metric {
    pub label: String,
    pub value: String,
    #[serde(default)]
    pub delta: Option<String>,
    #[serde(default)]
    pub trend: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Series {
    pub name: String,
    pub values: Vec<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Chart {
    pub title: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    pub series: Vec<Series>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct DashboardData {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub metrics: Vec<Metric>,
    #[serde(default)]
    pub charts: Vec<Chart>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct DocumentData {
    #[serde(default)]
    pub title: Option<String>,
    pub markdown: String,
}

pub enum CanvasData {
    Table(TableData),
    Checklist(ChecklistData),
    Dashboard(DashboardData),
    Document(DocumentData),
}

fn parse(canvas_id: &str, input: &Value) -> Result<CanvasData, String> {
    let e = |err: serde_json::Error| format!("invalid {canvas_id} input: {err}");
    Ok(match canvas_id {
        "table" => CanvasData::Table(serde_json::from_value(input.clone()).map_err(e)?),
        "checklist" => CanvasData::Checklist(serde_json::from_value(input.clone()).map_err(e)?),
        "dashboard" => CanvasData::Dashboard(serde_json::from_value(input.clone()).map_err(e)?),
        "document" => CanvasData::Document(serde_json::from_value(input.clone()).map_err(e)?),
        other => return Err(format!("unknown canvas '{other}'")),
    })
}

/// Whether `canvas_id` is one of the canvases Graysense declares.
pub fn is_declared(canvas_id: &str) -> bool {
    matches!(canvas_id, "table" | "checklist" | "dashboard" | "document")
}

/// Validate an open request; returns the display title.
pub fn validate_open(canvas_id: &str, input: &Value) -> Result<String, String> {
    let data = parse(canvas_id, input)?;
    Ok(title_of(&data).unwrap_or_else(|| default_title(canvas_id)))
}

fn default_title(canvas_id: &str) -> String {
    let mut c = canvas_id.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn title_of(data: &CanvasData) -> Option<String> {
    match data {
        CanvasData::Table(d) => d.title.clone(),
        CanvasData::Checklist(d) => d.title.clone(),
        CanvasData::Dashboard(d) => d.title.clone(),
        CanvasData::Document(d) => d.title.clone(),
    }
}

pub struct CanvasInstance {
    pub instance_id: String,
    pub canvas_id: String,
    pub data: CanvasData,
    sort: Option<(usize, bool)>,
}

pub enum CanvasUiAction {
    SendToAgent(String),
    Copy(String),
    Close,
    OpenUrl(String),
}

fn cell_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

impl CanvasInstance {
    pub fn new(instance_id: String, canvas_id: String, input: &Value) -> Option<Self> {
        let data = parse(&canvas_id, input).ok()?;
        Some(CanvasInstance { instance_id, canvas_id, data, sort: None })
    }

    pub fn title(&self) -> String {
        title_of(&self.data).unwrap_or_else(|| default_title(&self.canvas_id))
    }

    pub fn icon(&self) -> &'static str {
        match self.data {
            CanvasData::Table(_) => icon::TABLE,
            CanvasData::Checklist(_) => icon::CHECKLIST,
            CanvasData::Dashboard(_) => icon::BAR_CHART,
            CanvasData::Document(_) => icon::DOCUMENT,
        }
    }

    pub fn state(&self) -> Value {
        match &self.data {
            CanvasData::Table(d) => serde_json::to_value(d),
            CanvasData::Checklist(d) => serde_json::to_value(d),
            CanvasData::Dashboard(d) => serde_json::to_value(d),
            CanvasData::Document(d) => serde_json::to_value(d),
        }
        .unwrap_or(Value::Null)
    }

    /// Apply an agent-invoked action; returns the tool result for the model.
    pub fn apply(&mut self, action: &str, input: &Value) -> Result<Value, String> {
        let arr = |k: &str| input.get(k).and_then(|v| v.as_array()).cloned();
        match (&mut self.data, action) {
            (_, "get_state") => return Ok(self.state()),
            (CanvasData::Table(d), "set_rows") => {
                d.rows = arr("rows").ok_or("rows required")?.into_iter().filter_map(|r| r.as_array().cloned()).collect();
            }
            (CanvasData::Table(d), "append_rows") => {
                d.rows.extend(arr("rows").ok_or("rows required")?.into_iter().filter_map(|r| r.as_array().cloned()));
            }
            (CanvasData::Checklist(d), "add_item") => {
                let text = input.get("text").and_then(|v| v.as_str()).ok_or("text required")?.to_owned();
                let note = input.get("note").and_then(|v| v.as_str()).map(str::to_owned);
                d.items.push(CheckItem { text, done: false, note });
            }
            (CanvasData::Checklist(d), "set_done") => {
                let i = input.get("index").and_then(|v| v.as_u64()).ok_or("index required")? as usize;
                let done = input.get("done").and_then(|v| v.as_bool()).unwrap_or(true);
                d.items.get_mut(i).ok_or("index out of range")?.done = done;
            }
            (CanvasData::Dashboard(d), "update") => {
                if let Some(m) = input.get("metrics") {
                    d.metrics = serde_json::from_value(m.clone()).map_err(|e| e.to_string())?;
                }
                if let Some(c) = input.get("charts") {
                    d.charts = serde_json::from_value(c.clone()).map_err(|e| e.to_string())?;
                }
            }
            (CanvasData::Dashboard(d), "push_point") => {
                let ci = input.get("chart").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let si = input.get("series").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let value = input.get("value").and_then(|v| v.as_f64()).ok_or("value required")?;
                let chart = d.charts.get_mut(ci).ok_or("chart out of range")?;
                if let Some(label) = input.get("label").and_then(|v| v.as_str()) {
                    chart.labels.push(label.to_owned());
                }
                chart.series.get_mut(si).ok_or("series out of range")?.values.push(value);
            }
            (CanvasData::Document(d), "set_markdown") => {
                d.markdown = input.get("markdown").and_then(|v| v.as_str()).ok_or("markdown required")?.to_owned();
            }
            (CanvasData::Document(d), "append") => {
                d.markdown.push_str("\n\n");
                d.markdown.push_str(input.get("markdown").and_then(|v| v.as_str()).ok_or("markdown required")?);
            }
            (_, other) => return Err(format!("unsupported action '{other}' for {} canvas", self.canvas_id)),
        }
        Ok(json!({ "ok": true }))
    }

    /// Render the canvas body. `id` must be stable per instance.
    pub fn show(&mut self, ui: &mut Ui, theme: &Theme, id: egui::Id) -> Option<CanvasUiAction> {
        let mut out = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new(self.icon()).font(crate::theme::icon_font(14.0)).color(theme.accent));
            ui.label(RichText::new(self.title()).font(semibold(14.0)).color(theme.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if kit::icon_button_sized(ui, icon::CANCEL, "Close canvas", theme, 22.0, false).clicked() {
                    out = Some(CanvasUiAction::Close);
                }
                if kit::icon_button_sized(ui, icon::SEND, "Send current state to the agent", theme, 22.0, false).clicked() {
                    out = Some(CanvasUiAction::SendToAgent(self.summary_for_agent()));
                }
                if kit::icon_button_sized(ui, icon::COPY, "Copy", theme, 22.0, false).clicked() {
                    out = Some(CanvasUiAction::Copy(self.as_text()));
                }
            });
        });
        ui.add_space(4.0);
        egui::ScrollArea::vertical().id_salt(id.with("scroll")).auto_shrink([false; 2]).show(ui, |ui| {
            match &mut self.data {
                CanvasData::Table(d) => table(ui, d, &mut self.sort, theme),
                CanvasData::Checklist(d) => checklist(ui, d, theme),
                CanvasData::Dashboard(d) => dashboard(ui, d, theme),
                CanvasData::Document(d) => {
                    let mut md = markdown::MdOut { open_url: None, copy: None };
                    markdown::render(ui, &d.markdown, theme, &mut md);
                    if let Some(u) = md.open_url {
                        out = Some(CanvasUiAction::OpenUrl(u));
                    }
                    if let Some(c) = md.copy {
                        out = Some(CanvasUiAction::Copy(c));
                    }
                }
            }
        });
        out
    }

    fn as_text(&self) -> String {
        match &self.data {
            CanvasData::Table(d) => {
                let mut s = d.columns.join("\t");
                for r in &d.rows {
                    s.push('\n');
                    s.push_str(&r.iter().map(cell_text).collect::<Vec<_>>().join("\t"));
                }
                s
            }
            CanvasData::Checklist(d) => d.items.iter().map(|i| format!("- [{}] {}", if i.done { "x" } else { " " }, i.text)).collect::<Vec<_>>().join("\n"),
            CanvasData::Dashboard(d) => d.metrics.iter().map(|m| format!("{}: {}", m.label, m.value)).collect::<Vec<_>>().join("\n"),
            CanvasData::Document(d) => d.markdown.clone(),
        }
    }

    fn summary_for_agent(&self) -> String {
        format!("Here is the current state of the \"{}\" {} canvas after my edits:\n\n```json\n{}\n```", self.title(), self.canvas_id, serde_json::to_string_pretty(&self.state()).unwrap_or_default())
    }
}

fn table(ui: &mut Ui, d: &mut TableData, sort: &mut Option<(usize, bool)>, theme: &Theme) {
    let mut order: Vec<usize> = (0..d.rows.len()).collect();
    if let Some((col, asc)) = *sort {
        order.sort_by(|&a, &b| {
            let va = d.rows[a].get(col).map(cell_text).unwrap_or_default();
            let vb = d.rows[b].get(col).map(cell_text).unwrap_or_default();
            let ord = match (va.parse::<f64>(), vb.parse::<f64>()) {
                (Ok(x), Ok(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
                _ => va.to_lowercase().cmp(&vb.to_lowercase()),
            };
            if asc { ord } else { ord.reverse() }
        });
    }
    egui::Grid::new(ui.next_auto_id()).striped(true).spacing(vec2(16.0, 6.0)).min_col_width(40.0).show(ui, |ui| {
        for (c, name) in d.columns.iter().enumerate() {
            let arrow = match *sort {
                Some((sc, true)) if sc == c => " ↑",
                Some((sc, false)) if sc == c => " ↓",
                _ => "",
            };
            let r = ui.add(egui::Label::new(RichText::new(format!("{name}{arrow}")).font(semibold(12.5)).color(theme.text_dim)).sense(Sense::click()));
            if r.clicked() {
                *sort = match *sort {
                    Some((sc, asc)) if sc == c => Some((c, !asc)),
                    _ => Some((c, true)),
                };
            }
        }
        ui.end_row();
        for &i in &order {
            for c in 0..d.columns.len() {
                ui.label(RichText::new(d.rows[i].get(c).map(cell_text).unwrap_or_default()).font(ui_font(12.5)));
            }
            ui.end_row();
        }
    });
    ui.add_space(6.0);
    ui.label(RichText::new(format!("{} rows", d.rows.len())).font(ui_font(11.5)).color(theme.text_faint));
}

fn checklist(ui: &mut Ui, d: &mut ChecklistData, theme: &Theme) {
    let done = d.items.iter().filter(|i| i.done).count();
    let total = d.items.len().max(1);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
    kit::progress(ui.painter(), r, done as f32 / total as f32, theme.ok, theme.surface3);
    ui.label(RichText::new(format!("{done} of {} complete", d.items.len())).font(ui_font(11.5)).color(theme.text_dim));
    ui.add_space(4.0);
    for item in d.items.iter_mut() {
        ui.horizontal_top(|ui| {
            ui.checkbox(&mut item.done, "");
            ui.vertical(|ui| {
                let text = if item.done { RichText::new(&item.text).strikethrough().color(theme.text_faint) } else { RichText::new(&item.text).color(theme.text) };
                ui.label(text);
                if let Some(n) = &item.note {
                    ui.label(RichText::new(n).font(ui_font(11.5)).color(theme.text_dim));
                }
            });
        });
    }
}

fn dashboard(ui: &mut Ui, d: &mut DashboardData, theme: &Theme) {
    if !d.metrics.is_empty() {
        ui.horizontal_wrapped(|ui| {
            for m in &d.metrics {
                let (rect, _) = ui.allocate_exact_size(vec2(150.0, 64.0), Sense::hover());
                let p = ui.painter();
                p.rect(rect, CornerRadius::same(8), theme.surface2, Stroke::new(1.0, theme.border), StrokeKind::Inside);
                p.text(rect.left_top() + vec2(12.0, 10.0), Align2::LEFT_TOP, &m.label, ui_font(11.5), theme.text_dim);
                p.text(rect.left_top() + vec2(12.0, 28.0), Align2::LEFT_TOP, &m.value, semibold(19.0), theme.text);
                if let Some(delta) = &m.delta {
                    let c = match m.trend.as_deref() {
                        Some("up") => theme.ok,
                        Some("down") => theme.err,
                        _ => theme.text_dim,
                    };
                    p.text(rect.right_top() + vec2(-10.0, 10.0), Align2::RIGHT_TOP, delta, ui_font(11.5), c);
                }
            }
        });
        ui.add_space(8.0);
    }
    let palette = [theme.accent, theme.ok, theme.warn, theme.err, crate::theme::mix(theme.accent, theme.ok, 0.5)];
    for chart in &d.charts {
        chart_ui(ui, theme, chart, &palette);
        ui.add_space(8.0);
    }
}

/// One chart. Bar charts whose category labels can't fit under their bars turn horizontal;
/// otherwise labels are thinned and elided to fit. Hovering shows exact values.
fn chart_ui(ui: &mut Ui, theme: &Theme, chart: &Chart, palette: &[Color32]) {
    ui.label(RichText::new(&chart.title).font(semibold(12.5)).color(theme.text));
    let n = chart.series.iter().map(|s| s.values.len()).max().unwrap_or(0);
    if n == 0 {
        ui.label(RichText::new("No data").font(ui_font(11.5)).color(theme.text_faint));
        return;
    }
    let font = ui_font(10.0);
    let label_of = |i: usize| chart.labels.get(i).cloned().unwrap_or_else(|| (i + 1).to_string());
    let label_w = (0..n).map(|i| ui.painter().layout_no_wrap(label_of(i), font.clone(), theme.text).size().x).fold(0.0f32, f32::max);
    let bar = chart.kind.as_deref() == Some("bar");
    let slot = (ui.available_width() - 60.0) / n as f32;
    let hovered = if bar && label_w > slot - 6.0 { hbar_chart(ui, theme, chart, palette, n, &label_of, label_w) } else { xy_chart(ui, theme, chart, palette, n, &label_of, label_w, bar) };
    if let Some((i, resp)) = hovered {
        let mut tip = label_of(i);
        for s in &chart.series {
            if let Some(v) = s.values.get(i) {
                let name = if s.name.is_empty() || chart.series.len() == 1 { String::new() } else { format!("{}: ", s.name) };
                tip.push_str(&format!("\n{name}{}", format_num(*v)));
            }
        }
        resp.on_hover_text_at_pointer(tip);
    }
    if chart.series.len() > 1 {
        ui.horizontal_wrapped(|ui| {
            for (si, s) in chart.series.iter().enumerate() {
                let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                ui.painter().rect_filled(r, CornerRadius::same(2), palette[si % palette.len()]);
                ui.label(RichText::new(&s.name).font(ui_font(11.5)).color(theme.text_dim));
            }
        });
    }
}

/// Horizontal bars: one row per category, label on the left, value at the end of the bar.
fn hbar_chart(ui: &mut Ui, theme: &Theme, chart: &Chart, palette: &[Color32], n: usize, label_of: &dyn Fn(usize) -> String, label_w: f32) -> Option<(usize, egui::Response)> {
    let ns = chart.series.len().max(1);
    let row = (10.0 + 9.0 * ns as f32).max(22.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row * n as f32 + 12.0), Sense::hover());
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(8), theme.surface2, Stroke::new(1.0, theme.border), StrokeKind::Inside);
    let font = ui_font(10.5);
    let lw = label_w.min(rect.width() * 0.38) + 4.0;
    let left = rect.left() + 12.0 + lw + 8.0;
    let right = rect.right() - 12.0 - if ns == 1 { 40.0 } else { 0.0 };
    let max = chart.series.iter().flat_map(|s| s.values.iter().copied()).fold(0.0f64, f64::max).max(1e-9);
    let hover = resp.hover_pos().and_then(|h| (0..n).find(|i| h.y >= rect.top() + 6.0 + row * *i as f32 && h.y < rect.top() + 6.0 + row * (*i + 1) as f32));
    for i in 0..n {
        let top = rect.top() + 6.0 + row * i as f32;
        if hover == Some(i) {
            p.rect_filled(egui::Rect::from_min_size(pos2(rect.left() + 4.0, top), vec2(rect.width() - 8.0, row)), CornerRadius::same(4), theme.surface3);
        }
        let g = kit::elided(p, &label_of(i), font.clone(), theme.text_dim, lw);
        p.galley(pos2(rect.left() + 12.0, top + (row - g.size().y) / 2.0), g, theme.text_dim);
        let bh = ((row - 10.0) / ns as f32).min(12.0);
        for (si, s) in chart.series.iter().enumerate() {
            let v = s.values.get(i).copied().unwrap_or(0.0).max(0.0);
            let y = top + (row - bh * ns as f32) / 2.0 + bh * si as f32;
            let w = ((right - left) * (v / max) as f32).max(1.5);
            p.rect_filled(egui::Rect::from_min_size(pos2(left, y), vec2(w, bh - 1.0)), CornerRadius::same(2), palette[si % palette.len()]);
            if ns == 1 {
                p.text(pos2(left + w + 6.0, y + bh / 2.0), Align2::LEFT_CENTER, format_num(v), font.clone(), theme.text_faint);
            }
        }
    }
    hover.map(|i| (i, resp))
}

/// Vertical bars or lines with a value axis; category labels thinned and elided to fit.
#[allow(clippy::too_many_arguments)]
fn xy_chart(ui: &mut Ui, theme: &Theme, chart: &Chart, palette: &[Color32], n: usize, label_of: &dyn Fn(usize) -> String, label_w: f32, bar: bool) -> Option<(usize, egui::Response)> {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 160.0), Sense::hover());
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(8), theme.surface2, Stroke::new(1.0, theme.border), StrokeKind::Inside);
    let plot = egui::Rect::from_min_max(pos2(rect.left() + 46.0, rect.top() + 12.0), pos2(rect.right() - 14.0, rect.bottom() - 24.0));
    let max = chart.series.iter().flat_map(|s| s.values.iter().copied()).fold(f64::MIN, f64::max).max(1e-9);
    let min = chart.series.iter().flat_map(|s| s.values.iter().copied()).fold(f64::MAX, f64::min).min(0.0);
    let step = nice_step((max - min).max(1e-9) / 3.0);
    let min = (min / step).floor() * step;
    let ticks = (((max - min) / step).ceil() as usize).max(1);
    let span = step * ticks as f64;
    for k in 0..=ticks {
        let y = plot.bottom() - plot.height() * k as f32 / ticks as f32;
        p.hline(plot.x_range(), y, Stroke::new(1.0, theme.border));
        p.text(pos2(plot.left() - 6.0, y), Align2::RIGHT_CENTER, format_num(min + step * k as f64), ui_font(10.0), theme.text_faint);
    }
    let slot = plot.width() / n as f32;
    let x_of = |i: usize| if bar { plot.left() + slot * (i as f32 + 0.5) } else if n > 1 { plot.left() + plot.width() * i as f32 / (n - 1) as f32 } else { plot.center().x };
    let y_of = |v: f64| plot.bottom() - plot.height() * ((v - min) / span) as f32;
    let hover = resp.hover_pos().filter(|h| rect.contains(*h)).map(|h| {
        let i = if bar { ((h.x - plot.left()) / slot).floor() } else if n > 1 { ((h.x - plot.left()) / plot.width() * (n - 1) as f32).round() } else { 0.0 };
        (i.max(0.0) as usize).min(n - 1)
    });
    if let Some(i) = hover {
        if bar {
            p.rect_filled(egui::Rect::from_min_max(pos2(plot.left() + slot * i as f32, plot.top()), pos2(plot.left() + slot * (i + 1) as f32, plot.bottom())), CornerRadius::same(3), theme.surface3);
        } else {
            p.vline(x_of(i), plot.y_range(), Stroke::new(1.0, theme.border_strong));
        }
    }
    let ns = chart.series.len().max(1);
    for (si, s) in chart.series.iter().enumerate() {
        let color = palette[si % palette.len()];
        if bar {
            let bw = (slot * 0.72 / ns as f32).max(2.0);
            for (i, v) in s.values.iter().enumerate() {
                let x = x_of(i) - bw * ns as f32 / 2.0 + bw * si as f32;
                let (a, b) = (y_of(*v), y_of(0.0f64.max(min)));
                p.rect_filled(egui::Rect::from_min_max(pos2(x, a.min(b)), pos2(x + bw * 0.92, a.max(b))), CornerRadius::same(2), color);
            }
        } else {
            let pts: Vec<egui::Pos2> = s.values.iter().enumerate().map(|(i, v)| pos2(x_of(i), y_of(*v))).collect();
            if pts.len() > 1 {
                p.add(egui::Shape::line(pts.clone(), Stroke::new(1.8, color)));
            }
            for (i, pt) in pts.iter().enumerate() {
                p.circle_filled(*pt, if hover == Some(i) { 3.6 } else { 2.4 }, color);
            }
        }
    }
    // Category labels: as many as fit without overlapping, elided to their share of the width.
    let fit = ((plot.width() / (label_w.min(90.0) + 10.0)).floor() as usize).clamp(1, n);
    let every = n.div_ceil(fit);
    let room = (plot.width() / n.div_ceil(every) as f32 - 6.0).max(12.0);
    for i in (0..n).step_by(every) {
        let g = kit::elided(p, &label_of(i), ui_font(10.0), theme.text_faint, room);
        p.galley(pos2(x_of(i) - g.size().x / 2.0, plot.bottom() + 5.0), g, theme.text_faint);
    }
    hover.map(|i| (i, resp))
}

/// Round a raw axis step up to 1, 2, 2.5 or 5 × 10ⁿ.
fn nice_step(raw: f64) -> f64 {
    let mag = 10f64.powf(raw.log10().floor());
    let f = raw / mag;
    let nice = if f <= 1.0 { 1.0 } else if f <= 2.0 { 2.0 } else if f <= 2.5 { 2.5 } else if f <= 5.0 { 5.0 } else { 10.0 };
    nice * mag
}

fn format_num(v: f64) -> String {
    let short = |x: f64, unit: &str| if (x - x.round()).abs() < 1e-6 { format!("{x:.0}{unit}") } else { format!("{x:.1}{unit}") };
    if v.abs() >= 1e6 {
        short(v / 1e6, "M")
    } else if v.abs() >= 1000.0 {
        short(v / 1000.0, "k")
    } else if v.fract().abs() < 1e-9 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}
