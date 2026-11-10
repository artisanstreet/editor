use anyhow::Result;
use artisan_api::ThreadId;
use serde_json::{Value, json};
use std::future::Future;

const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub properties: fn() -> Value,
    pub required: &'static [&'static str],
    pub output_schema: Option<fn() -> Value>,
    pub read_only: bool,
    pub destructive: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Content {
    Text(String),
    Image {
        mime_type: &'static str,
        data: String,
    },
}

pub struct ToolResult {
    pub content: Vec<Content>,
    pub structured_content: Option<Value>,
}

impl ToolResult {
    pub fn structured(value: Value) -> Result<Self> {
        let text = serde_json::to_string(&value)?;
        anyhow::ensure!(
            text.len() <= 256 * 1024,
            "response exceeds 256 KiB; use a smaller limit or narrower filters"
        );
        Ok(Self {
            content: vec![Content::Text(text)],
            structured_content: Some(value),
        })
    }
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

pub trait ToolSet: Send + Sync {
    fn name(&self) -> &'static str;
    fn title(&self) -> &'static str;
    fn instructions(&self) -> &'static str;
    fn tools(&self) -> Vec<Tool>;
    fn call(
        &self,
        caller: &ThreadId,
        name: &str,
        arguments: Value,
    ) -> impl Future<Output = Result<Vec<Content>>> + Send;

    fn canonical_name<'a>(&self, name: &'a str) -> &'a str {
        name
    }

    fn call_result(
        &self,
        caller: &ThreadId,
        name: &str,
        arguments: Value,
    ) -> impl Future<Output = Result<ToolResult>> + Send {
        async move {
            Ok(ToolResult {
                content: self.call(caller, name, arguments).await?,
                structured_content: None,
            })
        }
    }
}

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
    if !set
        .tools()
        .iter()
        .any(|tool| tool.name == set.canonical_name(name))
    {
        return Err((-32602, format!("Unknown tool {name}")));
    }
    let arguments = params
        .get("arguments")
        .cloned()
        .filter(|arguments| !arguments.is_null())
        .unwrap_or_else(|| json!({}));
    if name == set.canonical_name(name)
        && let Some(tool) = set.tools().into_iter().find(|tool| tool.name == name)
        && let Err(message) = crate::validation::validate(
            &json!({"type":"object","properties":(tool.properties)(),"required":tool.required,"additionalProperties":false}),
            &arguments,
            "arguments",
            0,
        )
    {
        return Ok(
            json!({"content":[{"type":"text","text":message}],"structuredContent":{"error":{"code":"invalid_arguments","message":message,"retryable":false}},"isError":true}),
        );
    }
    let (result, is_error) = match set.call_result(caller, name, arguments).await {
        Ok(result) => (result, false),
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
            let details = error.downcast_ref::<forge_store::coordination::Conflict>().map_or_else(
                || json!({"code":"tool_failed","message":text,"retryable":false}),
                |conflict| json!({"code":"stale_revision","message":text,"retryable":true,"record":conflict.id,"current_revision":conflict.current_revision,"recovery":"get_coordination_record"}),
            );
            (
                ToolResult {
                    content: vec![Content::Text(text)],
                    structured_content: Some(json!({ "error": details })),
                },
                true,
            )
        }
    };
    let content: Vec<Value> = result.content.iter().map(Content::to_json).collect();
    let mut response = json!({ "content": content, "isError": is_error });
    if let Some(structured) = result.structured_content {
        response["structuredContent"] = structured;
    }
    Ok(response)
}

#[must_use]
pub fn list(tools: &[Tool]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            let mut entry = json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": {
                    "type": "object",
                    "properties": (tool.properties)(),
                    "required": tool.required,
                    "additionalProperties": false,
                },
                "annotations": {
                    "readOnlyHint": tool.read_only,
                    "destructiveHint": tool.destructive,
                    "openWorldHint": matches!(tool.name, "install_harness" | "add_mcp_server" | "sign_in_mcp_server"),
                    "idempotentHint": tool.read_only,
                },
            });
            if let Some(schema) = tool.output_schema { entry["outputSchema"] = schema(); }
            entry
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
