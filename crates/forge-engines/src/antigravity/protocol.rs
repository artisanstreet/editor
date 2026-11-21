use serde::Deserialize;
use serde_json::{Value, json};

const PROTOCOL_VERSION: u64 = 1;
pub(super) const SIGN_IN_REQUIRED: i64 = -32000;

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("Antigravity wrote a line that isn't JSON")]
    Json(#[source] serde_json::Error),
    #[error("Antigravity wrote a JSON-RPC frame with neither a method nor an id")]
    Shapeless,
    #[error("Antigravity sent {method} with params Forge can't read")]
    Params {
        method: String,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Debug)]
pub(super) enum Frame {
    Response {
        id: u64,
        result: Result<Value, crate::rpc::RpcError>,
    },
    Update {
        update: Box<Change>,
    },
    Notice {
        method: String,
    },
    Permission {
        id: Value,
        request: Box<PermissionRequest>,
    },
    Form {
        id: Value,
        request: Box<FormRequest>,
    },
    Unsupported {
        id: Value,
        method: String,
    },
}

impl Frame {
    pub(super) fn parse(line: &str) -> Result<Self, FrameError> {
        let mut frame: Value = serde_json::from_str(line).map_err(FrameError::Json)?;
        let method = frame["method"].as_str().map(str::to_owned);
        let id = frame
            .get_mut("id")
            .map(Value::take)
            .filter(|id| !id.is_null());
        let params = frame.get_mut("params").map_or(Value::Null, Value::take);
        match (method, id) {
            (Some(method), Some(id)) if method == "session/request_permission" => {
                let request = decode(&method, params)?;
                Ok(Self::Permission {
                    id,
                    request: Box::new(request),
                })
            }
            (Some(method), Some(id)) if method == "elicitation/create" => {
                let request = decode(&method, params)?;
                Ok(Self::Form {
                    id,
                    request: Box::new(request),
                })
            }
            (Some(method), Some(id)) => Ok(Self::Unsupported { id, method }),
            (Some(method), None) if method == "session/update" => {
                let SessionUpdate { update } = decode(&method, params)?;
                Ok(Self::Update {
                    update: Box::new(update),
                })
            }
            (Some(method), None) => Ok(Self::Notice { method }),
            (None, Some(id)) => {
                let id = id.as_u64().ok_or(FrameError::Shapeless)?;
                let result = match frame.get("error") {
                    Some(error) => Err(crate::rpc::RpcError::refusal(error)),
                    None => Ok(frame.get_mut("result").map_or(Value::Null, Value::take)),
                };
                Ok(Self::Response { id, result })
            }
            (None, None) => Err(FrameError::Shapeless),
        }
    }
}

fn decode<T: serde::de::DeserializeOwned>(method: &str, params: Value) -> Result<T, FrameError> {
    serde_json::from_value(params).map_err(|source| FrameError::Params {
        method: method.to_owned(),
        source,
    })
}

#[derive(Debug, Deserialize)]
struct SessionUpdate {
    update: Change,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "sessionUpdate", rename_all = "snake_case")]
pub(super) enum Change {
    UserMessageChunk {
        content: Block,
    },
    AgentMessageChunk {
        content: Block,
    },
    AgentThoughtChunk {
        content: Block,
    },
    Call(Call),
    ToolCallUpdate(Call),
    Plan {
        entries: Vec<PlanEntry>,
    },
    #[serde(rename_all = "camelCase")]
    AvailableCommandsUpdate {
        available_commands: Vec<Command>,
    },
    #[serde(rename_all = "camelCase")]
    CurrentModeUpdate {
        current_mode_id: String,
    },
    #[serde(rename_all = "camelCase")]
    ConfigOptionUpdate {
        config_options: Vec<ConfigOption>,
    },
    SessionInfoUpdate {
        #[serde(default)]
        title: Option<String>,
    },
    UsageUpdate {
        #[serde(default)]
        used: Option<u64>,
        #[serde(default)]
        size: Option<u64>,
        #[serde(default)]
        cost: Option<Cost>,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum Block {
    Text {
        text: String,
    },
    #[serde(rename_all = "camelCase")]
    Image {
        #[serde(default)]
        mime_type: Option<String>,
    },
    ResourceLink {
        uri: String,
        #[serde(default)]
        name: Option<String>,
    },
    Resource {
        resource: Value,
    },
    #[serde(other)]
    Other,
}

impl Block {
    pub(super) fn text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            Self::Image { .. }
            | Self::ResourceLink { .. }
            | Self::Resource { .. }
            | Self::Other => None,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct Call {
    pub tool_call_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub kind: Option<ToolKind>,
    #[serde(default)]
    pub status: Option<ToolStatus>,
    #[serde(default)]
    pub content: Option<Vec<ToolContent>>,
    #[serde(default)]
    pub locations: Option<Vec<Location>>,
    #[serde(default)]
    pub raw_input: Option<Value>,
    #[serde(default)]
    pub raw_output: Option<Value>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    #[serde(other)]
    Other,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ToolStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ToolContent {
    Content {
        content: Block,
    },
    #[serde(rename_all = "camelCase")]
    Diff {
        path: String,
        #[serde(default)]
        old_text: Option<String>,
        #[serde(default)]
        new_text: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Terminal {
        terminal_id: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct Location {
    pub path: String,
    #[serde(default)]
    pub line: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct PlanEntry {
    pub content: String,
    pub status: PlanStatus,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum PlanStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct Command {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub input: Option<CommandInput>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct CommandInput {
    #[serde(default)]
    pub hint: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct Cost {
    pub amount: f64,
    #[serde(default)]
    pub currency: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct ConfigOption {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub current_value: Option<String>,
    #[serde(default)]
    pub options: Vec<ConfigChoice>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub(super) enum ConfigChoice {
    Value {
        value: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        description: Option<String>,
    },
    Group {
        #[serde(default)]
        name: Option<String>,
        options: Vec<Self>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Choice {
    pub value: String,
    pub name: Option<String>,
    pub description: Option<String>,
}

impl ConfigOption {
    pub(super) fn is(&self, category: &str) -> bool {
        self.category.as_deref() == Some(category) || self.id == category
    }

    pub(super) fn choices(&self) -> Vec<Choice> {
        fn flatten(choices: &[ConfigChoice], out: &mut Vec<Choice>) {
            for choice in choices {
                match choice {
                    ConfigChoice::Value {
                        value,
                        name,
                        description,
                    } => out.push(Choice {
                        value: value.clone(),
                        name: name.clone(),
                        description: description.clone(),
                    }),
                    ConfigChoice::Group { options, .. } => flatten(options, out),
                }
            }
        }
        let mut out = Vec::new();
        flatten(&self.options, &mut out);
        out
    }
}

pub(super) const MODEL: &str = "model";
pub(super) const THOUGHT_LEVEL: &str = "thought_level";

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct Opened {
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub models: Option<Models>,
    #[serde(default)]
    pub modes: Option<Modes>,
    #[serde(default)]
    pub config_options: Vec<ConfigOption>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct Models {
    #[serde(default)]
    pub available_models: Vec<Listed>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct Listed {
    pub model_id: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct Modes {
    #[serde(default)]
    pub current_mode_id: Option<String>,
    #[serde(default)]
    pub available_modes: Vec<Mode>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct Mode {
    pub id: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct Prompted {
    pub stop_reason: StopReason,
    #[serde(default)]
    pub usage: Option<TurnUsage>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum StopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) struct TurnUsage {
    #[serde(default, rename = "inputTokens")]
    pub input: u64,
    #[serde(default, rename = "outputTokens")]
    pub output: u64,
    #[serde(default, rename = "thoughtTokens")]
    pub thought: Option<u64>,
    #[serde(default, rename = "cachedReadTokens")]
    pub cached_read: Option<u64>,
    #[serde(default, rename = "cachedWriteTokens")]
    pub cached_write: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct PermissionRequest {
    pub session_id: String,
    pub tool_call: Call,
    pub options: Vec<PermissionOption>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct PermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: OptionKind,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum OptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct FormRequest {
    #[serde(default)]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    pub requested_schema: FormSchema,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct FormSchema {
    #[serde(default)]
    pub properties: serde_json::Map<String, Value>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(super) struct FormProperty {
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub one_of: Vec<FormChoice>,
    #[serde(default)]
    pub any_of: Vec<FormChoice>,
    #[serde(default, rename = "enum")]
    pub values: Vec<String>,
    #[serde(default)]
    pub items: Option<Box<Self>>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct FormChoice {
    #[serde(rename = "const")]
    pub value: String,
    #[serde(default)]
    pub title: Option<String>,
}

pub(super) const OTHER_SUFFIX: &str = "_other";

pub(super) fn submitted(content: serde_json::Map<String, Value>) -> Value {
    let mut reply = serde_json::Map::new();
    reply.insert("action".to_owned(), Value::from("accept"));
    reply.insert("content".to_owned(), Value::Object(content));
    Value::Object(reply)
}

pub(super) fn withdrawn() -> Value {
    json!({ "action": "cancel" })
}

pub(super) fn selected(option: &str) -> Value {
    json!({ "outcome": { "outcome": "selected", "optionId": option } })
}

pub(super) fn dismissed() -> Value {
    json!({ "outcome": { "outcome": "cancelled" } })
}

pub(super) fn initialize() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "clientCapabilities": {
            "fs": { "readTextFile": false, "writeTextFile": false },
            "terminal": false,
            "elicitation": { "form": {} },
        },
        "clientInfo": {
            "name": "artisan",
            "title": "Artisan Editor",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

pub(super) fn mcp_servers(servers: &[crate::McpServer]) -> Value {
    let pairs = |map: &std::collections::BTreeMap<String, String>| -> Vec<Value> {
        map.iter()
            .map(|(name, value)| json!({ "name": name, "value": value }))
            .collect()
    };
    Value::Array(
        servers
            .iter()
            .map(|server| match &server.url {
                Some(url) => json!({
                    "type": if server.sse { "sse" } else { "http" },
                    "name": server.name,
                    "url": url,
                    "headers": pairs(&server.headers),
                }),
                None => json!({
                    "name": server.name,
                    "command": server.command,
                    "args": server.args,
                    "env": pairs(&server.env),
                }),
            })
            .collect(),
    )
}
