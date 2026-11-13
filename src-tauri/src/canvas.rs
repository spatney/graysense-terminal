//! Native agent canvases: UI surfaces declared to the Copilot runtime that agents can
//! open and drive (tables, checklists, dashboards, documents). The UI renders their state.

use github_copilot_sdk::canvas::CanvasDeclaration;
use github_copilot_sdk::rpc::CanvasAction;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

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
}

impl CanvasInstance {
    pub fn new(instance_id: String, canvas_id: String, input: &Value) -> Option<Self> {
        let data = parse(&canvas_id, input).ok()?;
        Some(CanvasInstance { instance_id, canvas_id, data })
    }

    pub fn title(&self) -> String {
        title_of(&self.data).unwrap_or_else(|| default_title(&self.canvas_id))
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

    /// A message telling the agent what the canvas looks like after the user's edits.
    pub fn summary_for_agent(&self) -> String {
        format!("Here is the current state of the \"{}\" {} canvas after my edits:\n\n```json\n{}\n```", self.title(), self.canvas_id, serde_json::to_string_pretty(&self.state()).unwrap_or_default())
    }
}
