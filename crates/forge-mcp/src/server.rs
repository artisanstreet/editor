//! What every MCP server Forge's gateway serves shares.
//!
//! JSON-RPC over one POSTed body, `initialize`, `ping`, `tools/list` and
//! `tools/call`, for any set of tools. The Artisan MCP is one [`ToolSet`];
//! computer use is another.

use anyhow::Result;
use artisan_api::ThreadId;
use serde_json::{Value, json};
use std::future::Future;

/// The protocol versions these servers speak, newest first.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// One tool: its schema for `tools/list`.
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    /// The properties of the input object; `required` lists the musts.
    pub properties: fn() -> Value,
    pub required: &'static [&'static str],
    pub read_only: bool,
    pub destructive: bool,
}

/// One block of a tool's result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    Text(String),
    /// An image, base64 encoded.
    Image {
        mime_type: &'static str,
        data: String,
    },
}

impl Content {
    fn to_json(&self) -> Value {
        match self {
            Self::Text(text) => json!({ "type": "text", "text": text }),
            Self::Image { mime_type, data } => {
                json!({ "type": "image", "mimeType": mime_type, "data": data })
            }
        }
    }
}

/// A server's tools and what it tells engines about itself.
pub trait ToolSet: Send + Sync {
    /// The name engines know the server by.
    fn name(&self) -> &'static str;
    fn title(&self) -> &'static str;
    fn instructions(&self) -> &'static str;
    fn tools(&self) -> Vec<Tool>;
    /// Runs tool `name` for the thread `caller`. A [`forge_runs::Refused`]
    /// error is the agent's to read; any other is logged as Forge's.
    fn call(
        &self,
        caller: &ThreadId,
        name: &str,
        arguments: Value,
    ) -> impl Future<Output = Result<Vec<Content>>> + Send;
}

/// Answers one POSTed body: a JSON-RPC message or a batch of them, for the
/// thread `caller`. `None` when nothing is owed back (notifications and
/// responses only).
pub async fn handle<T: ToolSet>(set: &T, caller: &ThreadId, body: Value) -> Option<Value> {
    match body {
        Value::Array(messages) => {
            let mut replies = Vec::new();
            for message in messages {
                if let Some(reply) = one(set, caller, message).await {
                    replies.push(reply);
                }
            }
            (!replies.is_empty()).then_some(Value::Array(replies))
        }
        message @ (Value::Null
        | Value::Bool(_)
        | Value::Number(_)
        | Value::String(_)
        | Value::Object(_)) => one(set, caller, message).await,
    }
}

async fn one<T: ToolSet>(set: &T, caller: &ThreadId, message: Value) -> Option<Value> {
    let id = message.get("id").cloned();
    let method = message.get("method").and_then(Value::as_str)?;
    // A notification (no id) is never answered.
    let id = id?;
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => Ok(initialize(set, &params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": list(&set.tools()) })),
        "tools/call" => call(set, caller, &params).await,
        _ => Err((-32601, format!("Unknown method {method}"))),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
        }
    })
}

async fn call<T: ToolSet>(
    set: &T,
    caller: &ThreadId,
    params: &Value,
) -> Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| (-32602, "tools/call needs a tool name".to_owned()))?;
    if !set.tools().iter().any(|tool| tool.name == name) {
        return Err((-32602, format!("Unknown tool {name}")));
    }
    let arguments = params
        .get("arguments")
        .cloned()
        .filter(|arguments| !arguments.is_null())
        .unwrap_or_else(|| json!({}));
    let (content, is_error) = match set.call(caller, name, arguments).await {
        Ok(content) => (content, false),
        Err(error) => {
            let text = if error.downcast_ref::<forge_runs::Refused>().is_some() {
                format!("{error}")
            } else {
                tracing::error!(
                    thread = %caller,
                    server = set.name(),
                    tool = name,
                    "MCP tool failed: {error:#}"
                );
                format!("Forge couldn't do that: {error:#}")
            };
            (vec![Content::Text(text)], true)
        }
    };
    let content: Vec<Value> = content.iter().map(Content::to_json).collect();
    Ok(json!({ "content": content, "isError": is_error }))
}

/// The `tools/list` entries for `tools`.
#[must_use]
pub fn list(tools: &[Tool]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": {
                    "type": "object",
                    "properties": (tool.properties)(),
                    "required": tool.required,
                },
                "annotations": {
                    "readOnlyHint": tool.read_only,
                    "destructiveHint": tool.destructive,
                    "openWorldHint": false,
                },
            })
        })
        .collect()
}

fn initialize<T: ToolSet>(set: &T, params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = asked
        .filter(|asked| PROTOCOL_VERSIONS.contains(asked))
        .unwrap_or(PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": set.name(), "title": set.title(), "version": env!("CARGO_PKG_VERSION") },
        "instructions": set.instructions(),
    })
}
