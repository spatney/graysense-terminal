//! JSON Schema → native form, for MCP elicitation and structured `ask_user` prompts.

use eframe::egui::{self, RichText, TextEdit, Ui};
use serde_json::{Map, Value};

use crate::kit;
use crate::theme::{Theme, semibold, ui as ui_font};

enum Kind {
    Text { multiline: bool },
    Number { integer: bool, min: Option<f64>, max: Option<f64> },
    Bool,
    Choice { options: Vec<(Value, String)> },
    Multi { options: Vec<(Value, String)> },
}

struct Field {
    key: String,
    title: String,
    description: Option<String>,
    required: bool,
    kind: Kind,
    text: String,
    flag: bool,
    choice: Option<usize>,
    multi: Vec<bool>,
}

pub struct FormState {
    fields: Vec<Field>,
    pub error: Option<String>,
}

fn options_of(v: &Value) -> Vec<(Value, String)> {
    if let Some(list) = v.get("enum").and_then(|e| e.as_array()) {
        let names = v.get("enumNames").and_then(|e| e.as_array());
        return list
            .iter()
            .enumerate()
            .map(|(i, val)| {
                let label = names
                    .and_then(|n| n.get(i))
                    .and_then(|n| n.as_str())
                    .map(str::to_owned)
                    .unwrap_or_else(|| val.as_str().map(str::to_owned).unwrap_or_else(|| val.to_string()));
                (val.clone(), label)
            })
            .collect();
    }
    for key in ["oneOf", "anyOf"] {
        if let Some(list) = v.get(key).and_then(|e| e.as_array()) {
            return list
                .iter()
                .filter_map(|o| {
                    let val = o.get("const")?.clone();
                    let label = o.get("title").and_then(|t| t.as_str()).map(str::to_owned).unwrap_or_else(|| val.as_str().map(str::to_owned).unwrap_or_else(|| val.to_string()));
                    Some((val, label))
                })
                .collect();
        }
    }
    Vec::new()
}

impl FormState {
    pub fn new(schema: Option<&Value>) -> Self {
        let mut fields = Vec::new();
        let empty = Map::new();
        let props = schema.and_then(|s| s.get("properties")).and_then(|p| p.as_object()).unwrap_or(&empty);
        let required: Vec<String> = schema
            .and_then(|s| s.get("required"))
            .and_then(|r| r.as_array())
            .map(|r| r.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        for (key, p) in props {
            let ty = p.get("type").and_then(|t| t.as_str()).unwrap_or("string");
            let options = options_of(p);
            let kind = match ty {
                "boolean" => Kind::Bool,
                "number" | "integer" => Kind::Number {
                    integer: ty == "integer",
                    min: p.get("minimum").and_then(|v| v.as_f64()),
                    max: p.get("maximum").and_then(|v| v.as_f64()),
                },
                "array" => Kind::Multi { options: p.get("items").map(options_of).unwrap_or_default() },
                _ if !options.is_empty() => Kind::Choice { options },
                _ => Kind::Text { multiline: p.get("maxLength").and_then(|v| v.as_u64()).is_none_or(|n| n > 200) && p.get("format").is_none() && key.contains("description") },
            };
            let default = p.get("default");
            let mut field = Field {
                key: key.clone(),
                title: p.get("title").and_then(|t| t.as_str()).map(str::to_owned).unwrap_or_else(|| key.clone()),
                description: p.get("description").and_then(|t| t.as_str()).map(str::to_owned),
                required: required.contains(key),
                text: String::new(),
                flag: false,
                choice: None,
                multi: Vec::new(),
                kind,
            };
            match (&field.kind, default) {
                (Kind::Bool, Some(Value::Bool(b))) => field.flag = *b,
                (Kind::Choice { options }, Some(d)) => field.choice = options.iter().position(|(v, _)| v == d),
                (Kind::Multi { options }, d) => {
                    let defaults = d.and_then(|d| d.as_array()).cloned().unwrap_or_default();
                    field.multi = options.iter().map(|(v, _)| defaults.contains(v)).collect();
                }
                (_, Some(Value::String(s))) => field.text = s.clone(),
                (_, Some(Value::Number(n))) => field.text = n.to_string(),
                _ => {}
            }
            fields.push(field);
        }
        FormState { fields, error: None }
    }

    /// The whole form is one single-choice question, so picking an option can submit it.
    pub fn is_single_choice(&self) -> bool {
        matches!(self.fields.as_slice(), [f] if matches!(&f.kind, Kind::Choice { options } if options.len() <= 8))
    }

    /// Human summary of submitted content (e.g. the chosen option).
    pub fn summary(&self, content: &Value) -> String {
        let Some(obj) = content.as_object() else { return "submitted".into() };
        let parts: Vec<String> = self
            .fields
            .iter()
            .filter_map(|f| {
                let v = obj.get(&f.key)?;
                let shown = match (&f.kind, v) {
                    (Kind::Choice { options }, v) => options.iter().find(|(val, _)| val == v).map(|(_, l)| l.clone()).unwrap_or_else(|| v.to_string()),
                    (_, Value::String(s)) => s.clone(),
                    (_, Value::Array(a)) => a.iter().map(|x| x.as_str().map(str::to_owned).unwrap_or_else(|| x.to_string())).collect::<Vec<_>>().join(", "),
                    (_, other) => other.to_string(),
                };
                Some(shown)
            })
            .collect();
        if parts.is_empty() { "submitted".into() } else { parts.join(" · ") }
    }

    /// Render the fields. Returns true when a single-choice option was just picked.
    pub fn show(&mut self, ui: &mut Ui, theme: &Theme, id: egui::Id) -> bool {
        let mut picked = false;
        for (i, f) in self.fields.iter_mut().enumerate() {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(&f.title).font(semibold(12.5)).color(theme.text));
                if f.required {
                    ui.label(RichText::new("*").color(theme.err));
                }
            });
            if let Some(d) = &f.description {
                ui.label(RichText::new(d).font(ui_font(12.0)).color(theme.text_dim));
            }
            let fid = id.with(i);
            match &f.kind {
                Kind::Text { multiline } => {
                    let edit = if *multiline { TextEdit::multiline(&mut f.text).desired_rows(3) } else { TextEdit::singleline(&mut f.text) };
                    ui.add(edit.id(fid).desired_width(f32::INFINITY));
                }
                Kind::Number { .. } => {
                    ui.add(TextEdit::singleline(&mut f.text).id(fid).desired_width(160.0));
                }
                Kind::Bool => {
                    kit::toggle_named(ui, &mut f.flag, theme, &f.title);
                }
                Kind::Choice { options } => {
                    if options.len() <= 8 {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                            for (k, (_, label)) in options.iter().enumerate() {
                                if kit::chip(ui, f.choice == Some(k), label, theme, false).clicked() {
                                    f.choice = Some(k);
                                    picked = true;
                                }
                            }
                        });
                    } else {
                        let selected = f.choice.map(|k| options[k].1.clone()).unwrap_or_else(|| "Select…".into());
                        egui::ComboBox::from_id_salt(fid).selected_text(selected).width(260.0).show_ui(ui, |ui| {
                            for (k, (_, label)) in options.iter().enumerate() {
                                ui.selectable_value(&mut f.choice, Some(k), label);
                            }
                        });
                    }
                }
                Kind::Multi { options } => {
                    if f.multi.len() != options.len() {
                        f.multi = vec![false; options.len()];
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                        for (k, (_, label)) in options.iter().enumerate() {
                            if kit::chip(ui, f.multi[k], label, theme, true).clicked() {
                                f.multi[k] = !f.multi[k];
                            }
                        }
                    });
                }
            }
        }
        if let Some(err) = &self.error {
            ui.label(RichText::new(err).color(theme.err).font(ui_font(12.0)));
        }
        picked
    }

    /// Validate and produce the JSON object to send back.
    pub fn to_json(&mut self) -> Option<Value> {
        let mut out = Map::new();
        for f in &self.fields {
            let value = match &f.kind {
                Kind::Text { .. } => {
                    if f.text.trim().is_empty() {
                        None
                    } else {
                        Some(Value::String(f.text.clone()))
                    }
                }
                Kind::Number { integer, min, max } => {
                    if f.text.trim().is_empty() {
                        None
                    } else {
                        let Ok(n) = f.text.trim().parse::<f64>() else {
                            self.error = Some(format!("“{}” must be a number", f.title));
                            return None;
                        };
                        if min.is_some_and(|m| n < m) || max.is_some_and(|m| n > m) {
                            self.error = Some(format!("“{}” is out of range", f.title));
                            return None;
                        }
                        Some(if *integer { Value::from(n.round() as i64) } else { Value::from(n) })
                    }
                }
                Kind::Bool => Some(Value::Bool(f.flag)),
                Kind::Choice { options } => f.choice.map(|k| options[k].0.clone()),
                Kind::Multi { options } => Some(Value::Array(options.iter().zip(&f.multi).filter(|(_, on)| **on).map(|(o, _)| o.0.clone()).collect())),
            };
            match value {
                Some(v) => {
                    out.insert(f.key.clone(), v);
                }
                None if f.required => {
                    self.error = Some(format!("“{}” is required", f.title));
                    return None;
                }
                None => {}
            }
        }
        self.error = None;
        Some(Value::Object(out))
    }
}
