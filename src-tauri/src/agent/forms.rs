//! JSON Schema → form spec, for MCP elicitation and structured `ask_user` prompts. The UI renders
//! the fields; Rust parses the schema and summarizes what was submitted.

use serde::Serialize;
use serde_json::{Map, Value};

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Kind {
    Text { multiline: bool },
    Number { integer: bool, min: Option<f64>, max: Option<f64> },
    Bool,
    Choice { options: Vec<Choice> },
    Multi { options: Vec<Choice> },
}

#[derive(Clone, Debug, Serialize)]
pub struct Choice {
    pub value: Value,
    pub label: String,
}

/// One field of the form, with its starting value.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    pub key: String,
    pub title: String,
    pub description: Option<String>,
    pub required: bool,
    pub kind: Kind,
    pub default: Option<Value>,
}

#[derive(Clone, Debug, Default)]
pub struct FormState {
    pub fields: Vec<Field>,
}

fn options_of(v: &Value) -> Vec<Choice> {
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
                Choice { value: val.clone(), label }
            })
            .collect();
    }
    for key in ["oneOf", "anyOf"] {
        if let Some(list) = v.get(key).and_then(|e| e.as_array()) {
            return list
                .iter()
                .filter_map(|o| {
                    let value = o.get("const")?.clone();
                    let label = o.get("title").and_then(|t| t.as_str()).map(str::to_owned).unwrap_or_else(|| value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string()));
                    Some(Choice { value, label })
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
            fields.push(Field {
                key: key.clone(),
                title: p.get("title").and_then(|t| t.as_str()).map(str::to_owned).unwrap_or_else(|| key.clone()),
                description: p.get("description").and_then(|t| t.as_str()).map(str::to_owned),
                required: required.contains(key),
                kind,
                default: p.get("default").cloned(),
            });
        }
        FormState { fields }
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
                    (Kind::Choice { options }, v) => options.iter().find(|o| &o.value == v).map(|o| o.label.clone()).unwrap_or_else(|| v.to_string()),
                    (_, Value::String(s)) => s.clone(),
                    (_, Value::Array(a)) => a.iter().map(|x| x.as_str().map(str::to_owned).unwrap_or_else(|| x.to_string())).collect::<Vec<_>>().join(", "),
                    (_, other) => other.to_string(),
                };
                Some(shown)
            })
            .collect();
        if parts.is_empty() { "submitted".into() } else { parts.join(" · ") }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn schema_becomes_fields_and_answers_read_back_as_labels() {
        let schema = json!({
            "properties": {
                "env": {"type": "string", "oneOf": [{"const": "prod", "title": "Production"}, {"const": "dev", "title": "Development"}]},
                "count": {"type": "integer", "minimum": 1},
                "confirm": {"type": "boolean", "default": true}
            },
            "required": ["env"]
        });
        let form = FormState::new(Some(&schema));
        let env = form.fields.iter().find(|f| f.key == "env").unwrap();
        assert!(env.required && matches!(&env.kind, Kind::Choice { options } if options.len() == 2));
        assert!(matches!(form.fields.iter().find(|f| f.key == "count").unwrap().kind, Kind::Number { integer: true, min: Some(_), .. }));
        assert_eq!(form.summary(&json!({"env": "prod"})), "Production");
    }
}
