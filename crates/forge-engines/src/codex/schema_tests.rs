use super::decoder::Decoder;
use super::protocol::{Notification, Request};
use crate::EngineEvent;
use artisan_api::ItemKind;
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;

const READ_NOTIFICATIONS: [&str; 21] = [
    "error",
    "item/agentMessage/delta",
    "item/commandExecution/outputDelta",
    "item/completed",
    "item/mcpToolCall/progress",
    "item/plan/delta",
    "item/reasoning/summaryPartAdded",
    "item/reasoning/summaryTextDelta",
    "item/started",
    "model/rerouted",
    "model/safetyBuffering/updated",
    "serverRequest/resolved",
    "thread/goal/cleared",
    "thread/goal/updated",
    "thread/name/updated",
    "thread/started",
    "thread/tokenUsage/updated",
    "turn/completed",
    "turn/plan/updated",
    "turn/started",
    "warning",
];
const READ_REQUESTS: [&str; 5] = [
    "item/commandExecution/requestApproval",
    "item/fileChange/requestApproval",
    "item/permissions/requestApproval",
    "item/tool/requestUserInput",
    "mcpServer/elicitation/request",
];
const NOTIFICATIONS: &str = include_str!("schema/ServerNotification.json");
const REQUESTS: &str = include_str!("schema/ServerRequest.json");
const MAX_EXAMPLES: usize = 48;
const MAX_DEPTH: usize = 14;

struct Examples<'a> {
    definitions: &'a Map<String, Value>,
    full: bool,
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

    fn or_null(&self, nullable: bool, values: impl IntoIterator<Item = Value>) -> Vec<Value> {
        let null = (self.nulls && nullable).then_some(Value::Null);
        null.into_iter().chain(values).collect()
    }

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

fn messages(source: &str) -> Vec<(String, Value)> {
    let schema: Value = serde_json::from_str(source).expect("Codex schema is JSON");
    let definitions = schema["definitions"]
        .as_object()
        .expect("schema definitions");
    let mut messages = Vec::new();
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

fn started() -> Decoder {
    let mut decoder = Decoder::default();
    let request = decoder.request("thread/start", json!({}));
    decoder
        .feed(&json!({ "id": request, "result": { "thread": { "id": "root" } } }).to_string())
        .expect("thread/start result");
    decoder
}

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
    let mut read = BTreeSet::new();
    for (method, params) in &messages {
        let parsed = Notification::parse(method, params)
            .map(|notice| !matches!(notice, Notification::Unknown));
        if matches!(parsed, Ok(true)) {
            read.insert(method.as_str());
        }
        check("notification", method, params, parsed, &mut failures);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert_eq!(
        read,
        BTreeSet::from(READ_NOTIFICATIONS),
        "every notification Artisan reads is checked against the schema"
    );
}

#[test]
fn every_request_codex_may_send_reads_without_error() {
    let mut failures = Vec::new();
    let messages = messages(REQUESTS);
    let mut read = BTreeSet::new();
    for (method, params) in &messages {
        let parsed =
            Request::parse(method, params).map(|request| !matches!(request, Request::Unknown));
        if matches!(parsed, Ok(true)) {
            read.insert(method.as_str());
        }
        check("request", method, params, parsed, &mut failures);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert_eq!(
        read,
        BTreeSet::from(READ_REQUESTS),
        "every request Artisan reads is checked against the schema"
    );
}
