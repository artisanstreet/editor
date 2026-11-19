use artisan_api::ThreadId;
use forge_runs::ErrorClass;
use forge_store::coordination::Conflict;
use serde_json::{Map, Number, Value, json};
use std::future::Future;

const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

const STRUCTURED_LIMIT: usize = 256 * 1024;

#[derive(Debug)]
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

#[derive(Debug)]
pub struct ToolResult {
    pub(crate) content: Vec<Content>,
    pub(crate) structured_content: Option<Value>,
}

#[derive(Debug, thiserror::Error)]
pub enum StructuredError {
    #[error("encode the structured result")]
    Encode(#[source] serde_json::Error),
    #[error("response is {bytes} bytes, over 256 KiB; use a smaller limit or narrower filters")]
    TooLarge { bytes: usize },
}

impl ToolResult {
    pub(crate) fn structured(value: Value) -> Result<Self, StructuredError> {
        let text = serde_json::to_string(&value).map_err(StructuredError::Encode)?;
        if text.len() > STRUCTURED_LIMIT {
            return Err(StructuredError::TooLarge { bytes: text.len() });
        }
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

#[derive(Debug)]
pub enum Failure<'a> {
    Refused(String),
    Stale(&'a Conflict),
    Broken,
}

impl Failure<'_> {
    pub fn classified(class: ErrorClass, error: &dyn std::error::Error) -> Self {
        match class {
            ErrorClass::NotFound | ErrorClass::Conflict | ErrorClass::BadRequest => {
                Self::Refused(diagnostics::chain(error))
            }
            ErrorClass::Internal => Self::Broken,
        }
    }
}

pub trait ToolError: std::error::Error + Send + Sync + 'static {
    fn failure(&self) -> Failure<'_>;
}

pub trait ToolSet: Send + Sync {
    type Error: ToolError;

    fn name(&self) -> &'static str;
    fn title(&self) -> &'static str;
    fn instructions(&self) -> &'static str;
    fn tools(&self) -> Vec<Tool>;
    fn call(
        &self,
        caller: &ThreadId,
        name: &str,
        arguments: Value,
    ) -> impl Future<Output = Result<Vec<Content>, Self::Error>> + Send;

    fn canonical_name<'a>(&self, name: &'a str) -> &'a str {
        name
    }

    fn call_result(
        &self,
        caller: &ThreadId,
        name: &str,
        arguments: Value,
    ) -> impl Future<Output = Result<ToolResult, Self::Error>> + Send {
        async move {
            Ok(ToolResult {
                content: self.call(caller, name, arguments).await?,
                structured_content: None,
            })
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RequestId {
    Number(Number),
    Text(String),
}

impl RequestId {
    fn to_json(&self) -> Value {
        match self {
            Self::Number(number) => Value::Number(number.clone()),
            Self::Text(text) => Value::String(text.clone()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Invalid {
    NotAnObject,
    Version,
    Id,
    Method,
    Params,
}

impl Invalid {
    const fn message(self) -> &'static str {
        match self {
            Self::NotAnObject => "A JSON-RPC message is an object.",
            Self::Version => "A JSON-RPC message has jsonrpc \"2.0\".",
            Self::Id => "A JSON-RPC id is a string or a number.",
            Self::Method => "A JSON-RPC request has a method name.",
            Self::Params => "JSON-RPC params are an object or an array.",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Envelope {
    Request {
        id: RequestId,
        method: String,
        params: Value,
    },
    Notification,
    Response,
    Invalid {
        id: Option<RequestId>,
        reason: Invalid,
    },
}

impl Envelope {
    fn parse(message: &Value) -> Self {
        let Some(object) = message.as_object() else {
            return Self::Invalid {
                id: None,
                reason: Invalid::NotAnObject,
            };
        };
        let id = match object.get("id") {
            None => None,
            Some(Value::Number(number)) => Some(RequestId::Number(number.clone())),
            Some(Value::String(text)) => Some(RequestId::Text(text.clone())),
            Some(Value::Null | Value::Bool(_) | Value::Array(_) | Value::Object(_)) => {
                return Self::Invalid {
                    id: None,
                    reason: Invalid::Id,
                };
            }
        };
        if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Self::Invalid {
                id,
                reason: Invalid::Version,
            };
        }
        Self::classify(object, id)
    }

    fn classify(object: &Map<String, Value>, id: Option<RequestId>) -> Self {
        let method = match object.get("method") {
            Some(Value::String(method)) => method.clone(),
            None if id.is_some()
                && (object.contains_key("result") || object.contains_key("error")) =>
            {
                return Self::Response;
            }
            Some(_) | None => {
                return Self::Invalid {
                    id,
                    reason: Invalid::Method,
                };
            }
        };
        let params = match object.get("params") {
            None => Value::Null,
            Some(params @ (Value::Object(_) | Value::Array(_))) => params.clone(),
            Some(Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)) => {
                return Self::Invalid {
                    id,
                    reason: Invalid::Params,
                };
            }
        };
        match id {
            Some(id) => Self::Request { id, method, params },
            None => Self::Notification,
        }
    }
}

fn error_reply(id: Option<&RequestId>, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.map_or(Value::Null, RequestId::to_json),
        "error": { "code": code, "message": message },
    })
}

pub async fn handle<T: ToolSet>(set: &T, caller: &ThreadId, body: Value) -> Option<Value> {
    match body {
        Value::Array(messages) if messages.is_empty() => Some(error_reply(
            None,
            -32600,
            "A JSON-RPC batch holds at least one message.",
        )),
        Value::Array(messages) => {
            let mut replies = Vec::new();
            for message in messages {
                if let Some(reply) = one(set, caller, &message).await {
                    replies.push(reply);
                }
            }
            (!replies.is_empty()).then_some(Value::Array(replies))
        }
        message @ (Value::Null
        | Value::Bool(_)
        | Value::Number(_)
        | Value::String(_)
        | Value::Object(_)) => one(set, caller, &message).await,
    }
}

async fn one<T: ToolSet>(set: &T, caller: &ThreadId, message: &Value) -> Option<Value> {
    let (id, method, params) = match Envelope::parse(message) {
        Envelope::Request { id, method, params } => (id, method, params),
        Envelope::Notification | Envelope::Response => return None,
        Envelope::Invalid { id, reason } => {
            return Some(error_reply(id.as_ref(), -32600, reason.message()));
        }
    };
    let result = match method.as_str() {
        "initialize" => Ok(initialize(set, &params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": list(&set.tools()) })),
        "tools/call" => call(set, caller, &params).await,
        _ => Err((-32601, format!("Unknown method {method}"))),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id.to_json(), "result": result }),
        Err((code, message)) => error_reply(Some(&id), code, &message),
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
        Err(error) => (failed(set.name(), caller, name, &error), true),
    };
    let content: Vec<Value> = result.content.iter().map(Content::to_json).collect();
    let mut response = json!({ "content": content, "isError": is_error });
    if let Some(structured) = result.structured_content {
        response["structuredContent"] = structured;
    }
    Ok(response)
}

fn failed<E: ToolError>(server: &str, caller: &ThreadId, tool: &str, error: &E) -> ToolResult {
    let (text, details) = match error.failure() {
        Failure::Refused(text) => {
            let details = json!({"code":"tool_failed","message":text,"retryable":false});
            (text, details)
        }
        Failure::Stale(conflict) => {
            let text = format!("Forge couldn't do that: {}", diagnostics::chain(error));
            let details = json!({"code":"stale_revision","message":text,"retryable":true,"record":conflict.id,"current_revision":conflict.current_revision,"recovery":"get_coordination_record"});
            (text, details)
        }
        Failure::Broken => {
            let chain = diagnostics::chain(error);
            tracing::error!(thread = %caller, server, tool, "MCP tool failed: {chain}");
            let text = format!("Forge couldn't do that: {chain}");
            let details = json!({"code":"tool_failed","message":text,"retryable":false});
            (text, details)
        }
    };
    ToolResult {
        content: vec![Content::Text(text)],
        structured_content: Some(json!({ "error": details })),
    }
}

#[must_use]
fn list(tools: &[Tool]) -> Vec<Value> {
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
