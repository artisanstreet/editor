//! The decoder against what Codex says it may send. Examples are generated
//! from Codex's own protocol schema (`schema/`) three ways: sparse (optional
//! fields left out), filled in, and filled in with `null` wherever it is
//! allowed; each covers every enum value and every variant of a union. Every message
//! Artisan reads must parse, and none may turn into an error in the
//! transcript: that is how a free-text question (`"options": null`) once
//! showed as "invalid Codex asynchronous questions".

use super::decoder::Decoder;
use super::protocol::{Notification, Request};
use crate::EngineEvent;
use artisan_api::ItemKind;
use serde_json::{Map, Value, json};

const NOTIFICATIONS: &str = include_str!("schema/ServerNotification.json");
const REQUESTS: &str = include_str!("schema/ServerRequest.json");
/// Examples per schema node, so unions inside unions stay bounded.
const MAX_EXAMPLES: usize = 48;
/// Deeper than any message Artisan reads; recursive types stop here.
const MAX_DEPTH: usize = 14;

/// Example values for one schema, in one of two modes.
struct Examples<'a> {
    definitions: &'a Map<String, Value>,
    /// Every field there, lists with an entry; otherwise only required
    /// fields, lists empty.
    full: bool,
    /// `null` wherever it is allowed.
    nulls: bool,
}

impl Examples<'_> {
    fn of(&self, schema: &Value, name: &str, depth: usize) -> Vec<Value> {
        if depth > MAX_DEPTH {
            return vec![Value::Null];
        }
        let mut values = self.of_unbounded(schema, name, depth + 1);
        values.truncate(MAX_EXAMPLES);
        if values.is_empty() {
            values.push(Value::Null);
        }
        values
    }

    fn of_unbounded(&self, schema: &Value, name: &str, depth: usize) -> Vec<Value> {
        if let Some(reference) = schema["$ref"].as_str() {
            let key = reference.trim_start_matches("#/definitions/");
            return self.of(&self.definitions[key], name, depth);
        }
        if let Some(value) = schema.get("const") {
            return vec![value.clone()];
        }
        if let Some(values) = schema["enum"].as_array() {
            return values.clone();
        }
        // Parts of one object: each example merges one of every part's.
        if let Some(parts) = schema["allOf"].as_array() {
            if let [only] = parts.as_slice() {
                return self.of(only, name, depth);
            }
            let parts: Vec<Vec<Value>> = parts
                .iter()
                .map(|part| self.of(part, name, depth))
                .collect();
            let count = parts.iter().map(Vec::len).max().unwrap_or(1);
            return (0..count)
                .map(|index| {
                    let mut merged = Map::new();
                    for part in &parts {
                        if let Value::Object(fields) = &part[index % part.len()] {
                            merged.extend(fields.clone());
                        }
                    }
                    Value::Object(merged)
                })
                .collect();
        }
        for union in ["oneOf", "anyOf"] {
            if let Some(branches) = schema[union].as_array() {
                // Shared fields beside the union go into every variant.
                if schema.get("properties").is_some() {
                    let mut base = schema.clone();
                    base.as_object_mut().expect("object schema").remove(union);
                    let parts = json!({ "allOf": [base, { union: branches }] });
                    return self.of(&parts, name, depth);
                }
                let null = |branch: &Value| branch["type"] == "null";
                let values = branches
                    .iter()
                    .filter(|branch| !null(branch))
                    .flat_map(|branch| self.of(branch, name, depth));
                return self.or_null(branches.iter().any(null), values);
            }
        }
        let (kind, nullable) = match &schema["type"] {
            Value::Array(kinds) => (
                kinds
                    .iter()
                    .find(|kind| *kind != "null")
                    .and_then(Value::as_str)
                    .unwrap_or("null"),
                kinds.iter().any(|kind| kind == "null"),
            ),
            Value::String(kind) => (kind.as_str(), false),
            _ if schema.get("properties").is_some() => ("object", false),
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::Object(_) => {
                return vec![Value::Null];
            }
        };
        let values = match kind {
            "object" => self.object(schema, depth),
            "array" if self.full => self
                .of(&schema["items"], name, depth)
                .into_iter()
                .map(|item| json!([item]))
                .collect(),
            "array" => vec![json!([])],
            "string" => vec![json!(if name == "threadId" { "root" } else { "x" })],
            "integer" | "number" => vec![json!(1)],
            "boolean" => vec![json!(self.full)],
            _ => vec![Value::Null],
        };
        self.or_null(nullable, values)
    }

    /// `values`, after a `null` first where it is allowed and wanted: the
    /// field is `null` in one example and present in the others, so a
    /// `null` deep inside a present parent is tried too.
    fn or_null(&self, nullable: bool, values: impl IntoIterator<Item = Value>) -> Vec<Value> {
        let null = (self.nulls && nullable).then_some(Value::Null);
        null.into_iter().chain(values).collect()
    }

    /// One object per example of its widest field, the other fields cycling
    /// through theirs.
    fn object(&self, schema: &Value, depth: usize) -> Vec<Value> {
        let required: Vec<&str> = schema["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let fields: Vec<(&String, Vec<Value>)> = schema["properties"]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(name, _)| self.full || required.contains(&name.as_str()))
            .map(|(name, field)| (name, self.of(field, name, depth)))
            .collect();
        let count = fields
            .iter()
            .map(|(_, values)| values.len())
            .max()
            .unwrap_or(1);
        (0..count)
            .map(|index| {
                Value::Object(
                    fields
                        .iter()
                        .map(|(name, values)| {
                            ((*name).clone(), values[index % values.len()].clone())
                        })
                        .collect(),
                )
            })
            .collect()
    }
}

/// Every `(method, params)` example the schema in `source` allows.
fn messages(source: &str) -> Vec<(String, Value)> {
    let schema: Value = serde_json::from_str(source).expect("Codex schema is JSON");
    let definitions = schema["definitions"]
        .as_object()
        .expect("schema definitions");
    let mut messages = Vec::new();
    // Sparse, filled, and filled with `null` wherever allowed.
    for (full, nulls) in [(false, true), (true, false), (true, true)] {
        let examples = Examples {
            definitions,
            full,
            nulls,
        };
        for message in schema["oneOf"]
            .as_array()
            .expect("schema lists its messages")
        {
            let method = message["properties"]["method"]["enum"][0]
                .as_str()
                .expect("message method")
                .to_owned();
            for params in examples.of(&message["properties"]["params"], "params", 0) {
                messages.push((method.clone(), params));
            }
        }
    }
    messages
}

/// A decoder with the run's thread started, as when messages arrive.
fn started() -> Decoder {
    let mut decoder = Decoder::default();
    let request = decoder.request("thread/start", json!({}));
    decoder
        .feed(&json!({ "id": request, "result": { "thread": { "id": "root" } } }).to_string())
        .expect("thread/start result");
    decoder
}

/// Errors the decoder raises about Codex's data, rather than ones Codex
/// reports about its own work.
fn complaints(events: &[EngineEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            EngineEvent::Item {
                kind: ItemKind::Error { message, .. },
                ..
            } => message.inline().map(str::to_owned),
            EngineEvent::Session { .. }
            | EngineEvent::Model { .. }
            | EngineEvent::Title { .. }
            | EngineEvent::Item { .. }
            | EngineEvent::Image { .. }
            | EngineEvent::Status { .. }
            | EngineEvent::Live { .. }
            | EngineEvent::Usage(_)
            | EngineEvent::Commands(_)
            | EngineEvent::InputAccepted { .. } => None,
        })
        .filter(|message| message.contains("Codex"))
        .collect()
}

fn check(
    kind: &str,
    method: &str,
    params: &Value,
    parsed: Result<bool, serde_json::Error>,
    failures: &mut Vec<String>,
) {
    match parsed {
        // A method Artisan doesn't read.
        Ok(false) => return,
        Ok(true) => {}
        Err(error) => {
            failures.push(format!(
                "{kind} {method} doesn't parse: {error}\n  {params}"
            ));
            return;
        }
    }
    let frame = if kind == "request" {
        json!({ "id": 7, "method": method, "params": params })
    } else {
        json!({ "method": method, "params": params })
    };
    match started().feed(&frame.to_string()) {
        Err(error) => failures.push(format!(
            "{kind} {method} fails the run: {error:#}\n  {params}"
        )),
        // Codex reporting a failure of its own is meant to show.
        Ok(_) if method == "error" || params["turn"]["status"] == "failed" => {}
        Ok(events) => {
            for complaint in complaints(&events) {
                failures.push(format!(
                    "{kind} {method} shows an error: {complaint}\n  {params}"
                ));
            }
        }
    }
}

#[test]
fn every_notification_codex_may_send_reads_without_error() {
    let mut failures = Vec::new();
    let messages = messages(NOTIFICATIONS);
    assert!(messages.len() > 100, "the schema yields examples");
    for (method, params) in &messages {
        let parsed = Notification::parse(method, params)
            .map(|notice| !matches!(notice, Notification::Unknown));
        check("notification", method, params, parsed, &mut failures);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn every_request_codex_may_send_reads_without_error() {
    let mut failures = Vec::new();
    for (method, params) in &messages(REQUESTS) {
        let parsed =
            Request::parse(method, params).map(|request| !matches!(request, Request::Unknown));
        check("request", method, params, parsed, &mut failures);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
