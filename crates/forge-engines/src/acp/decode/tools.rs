//! ACP tool calls: `tool_call` and `tool_call_update` merge into one record
//! per `toolCallId`, which becomes a `Tool` or `Agent` item.

use super::AcpDecoder;
use crate::{EngineEvent, LiveKind};
use artisan_api::{
    Approval, ErrorKind, FileChange, FileOp, ItemKind, Output, SearchMode, Status, ToolKind,
};
use serde_json::{Value, json};

/// What the engine said about one tool call so far.
#[derive(Debug, Default)]
pub(super) struct Tool {
    /// ACP `ToolKind` (`execute`, `read`, `edit`, …).
    pub(super) kind: Option<String>,
    pub(super) title: Option<String>,
    pub(super) raw_input: Value,
    exit_code: Option<i32>,
    /// The latest output, until the call settles.
    text: String,
    changes: Vec<FileChange>,
    locations: Vec<String>,
    pub(super) status: Option<Status>,
    /// The permission request shown on it, with its decision once answered.
    pub(super) approval: Option<Approval>,
    /// The item it settled as. Its native fields are dropped then; only a
    /// late output or report changes it.
    pub(super) settled: Option<ItemKind>,
}

impl AcpDecoder {
    pub(super) fn merge_tool(&mut self, id: &str, update: &Value) {
        let tool = self.tools.entry(id.to_owned()).or_default();
        if let Some(kind) = update["kind"].as_str() {
            tool.kind = Some(kind.to_owned());
        }
        if let Some(title) = update["title"].as_str().filter(|title| !title.is_empty()) {
            tool.title = Some(title.to_owned());
        }
        if !update["rawInput"].is_null() {
            tool.raw_input = update["rawInput"].clone();
            tool.settled = None;
        }
        if !update["rawOutput"].is_null() {
            let output = &update["rawOutput"];
            tool.exit_code = ["exitCode", "exit_code"]
                .iter()
                .find_map(|name| output[name].as_i64())
                .and_then(|code| i32::try_from(code).ok());
            let text = match output {
                Value::String(text) => text.clone(),
                Value::Object(_) => ["output", "stdout", "content", "stderr", "error"]
                    .iter()
                    .filter_map(|name| output[name].as_str())
                    .filter(|text| !text.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => String::new(),
            };
            if !text.is_empty() {
                tool.text = text;
            }
        }
        if let Some(locations) = update["locations"].as_array() {
            tool.locations = locations
                .iter()
                .filter_map(|location| location["path"].as_str().map(str::to_owned))
                .collect();
        }
        if let Some(content) = update["content"].as_array() {
            let text = content_blocks_text(&update["content"]);
            if !text.is_empty() {
                tool.text = text;
            }
            let changes: Vec<FileChange> = content
                .iter()
                .filter(|block| block["type"] == "diff")
                .filter_map(|block| file_change(tool.kind.as_deref(), block))
                .collect();
            if !changes.is_empty() {
                if let Some(ItemKind::Tool {
                    tool: ToolKind::Edit { changes: settled },
                    ..
                }) = &mut tool.settled
                {
                    *settled = changes.clone().into();
                }
                tool.changes = changes;
            }
        }
        let status = match update["status"].as_str() {
            Some("completed") => Some(Status::Done),
            Some("failed") => Some(Status::Failed),
            Some("pending" | "in_progress") if tool.status != Some(Status::AwaitingUser) => {
                Some(Status::Running)
            }
            _ => None,
        };
        if let Some(status) = status {
            tool.status = Some(
                tool.status
                    .map_or(status, |current| current.advance(status)),
            );
        }
    }

    pub(super) fn emit_tool(&mut self, id: &str, events: &mut Vec<EngineEvent>) {
        let Some(tool) = self.tools.get_mut(id) else {
            return;
        };
        let input_name = tool.raw_input["_toolName"].as_str();
        if self.hidden.contains(id)
            || matches!(
                input_name,
                Some("createPlan" | "askQuestion" | "updateTodos")
            )
        {
            return;
        }
        let status = tool.status.unwrap_or(Status::Running);
        let kind = if let Some(settled) = &tool.settled {
            let mut kind = settled.clone();
            if !tool.text.is_empty() {
                let text = std::mem::take(&mut tool.text);
                match &mut kind {
                    ItemKind::Agent { result, .. } => *result = Some(text.into()),
                    ItemKind::Tool { output, .. } => *output = Some(Output::from_text(&text)),
                    _ => {}
                }
            }
            if let ItemKind::Tool { approval, .. } = &mut kind {
                approval.clone_from(&tool.approval);
            }
            kind
        } else if input_name == Some("task") {
            ItemKind::Agent {
                name: str_field(&tool.raw_input, &["subagentType"]),
                description: (str_field(&tool.raw_input, &["description"]).unwrap_or_default())
                    .into(),
                prompt: str_field(&tool.raw_input, &["prompt"]).map(Into::into),
                agent_id: None,
                context_used: None,
                model: None,
                background: false,
                result: (status.is_terminal() && !tool.text.is_empty())
                    .then(|| tool.text.clone().into()),
                label: None,
            }
        } else {
            if !status.is_terminal() && !tool.text.is_empty() {
                let key = id.to_owned();
                let target = if tool.kind.as_deref() == Some("execute") {
                    LiveKind::Shell { key }
                } else {
                    LiveKind::Progress { key }
                };
                events.push(EngineEvent::Live {
                    target,
                    text: tool.text.clone(),
                    append: false,
                });
            }
            let canonical = match tool_kind(tool) {
                Ok(kind) => kind,
                Err(message) => {
                    // Tool calls may announce identity before their input.
                    // Only completed malformed calls become explicit errors.
                    if status.is_terminal() {
                        self.emit_changed(
                            id,
                            Status::Done,
                            ItemKind::Error {
                                kind: ErrorKind::Execution,
                                message: message.into(),
                                retryable: false,
                            },
                            events,
                        );
                    }
                    return;
                }
            };
            ItemKind::Tool {
                tool: canonical,
                title: tool.title.clone(),
                output: (status.is_terminal() && !tool.text.is_empty())
                    .then(|| Output::from_text(&tool.text)),
                exit_code: tool.exit_code,
                approval: tool.approval.clone(),
            }
        };
        if status.is_terminal() {
            tool.settled = Some(kind.clone());
            let native_name = tool.raw_input["_toolName"].as_str().map(str::to_owned);
            tool.raw_input = native_name.map_or(Value::Null, |name| json!({"_toolName": name}));
            tool.changes = Vec::new();
            tool.text = String::new();
        }
        self.emit_changed(id, status, kind, events);
    }

    /// Emits an item unless it is exactly what was emitted last.
    pub(super) fn emit_changed(
        &mut self,
        key: &str,
        status: Status,
        kind: ItemKind,
        events: &mut Vec<EngineEvent>,
    ) {
        let item = (status, kind);
        if self.emitted.get(key) == Some(&item) {
            return;
        }
        if status.is_terminal() {
            self.emitted.remove(key);
        } else {
            self.emitted.insert(key.to_owned(), item.clone());
        }
        events.push(EngineEvent::Item {
            key: key.to_owned(),
            parent: None,
            status: item.0,
            kind: item.1,
        });
    }
}

/// The text of ACP `ToolCallContent` blocks (`{ type: "content", content }`).
pub(super) fn content_blocks_text(blocks: &Value) -> String {
    blocks
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == "content")
        .filter_map(|block| crate::native::content_text(&block["content"]))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn str_field(value: &Value, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| value[name].as_str())
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

pub(super) fn shell_command(tool: &Tool) -> Result<artisan_api::TextContent, String> {
    if let Some(ItemKind::Tool {
        tool: ToolKind::Shell { command, .. },
        ..
    }) = &tool.settled
    {
        return Ok(command.clone());
    }
    match &tool.raw_input["command"] {
        Value::Array(parts) => {
            let Some(parts) = parts.iter().map(Value::as_str).collect::<Option<Vec<_>>>() else {
                return Err("ACP command contains a non-string argument".into());
            };
            shlex::try_join(parts)
                .map(Into::into)
                .map_err(|error| format!("ACP command cannot be represented: {error}"))
        }
        _ => str_field(&tool.raw_input,&["command","cmd"])
            // Cursor permission messages may carry the actual command in title.
            .or_else(||tool.title.as_deref().map(|title|title.trim().trim_matches('`').to_owned()))
            .map(Into::into)
            .ok_or_else(||"ACP command is missing".to_owned()),
    }
}

fn tool_path(tool: &Tool) -> String {
    str_field(
        &tool.raw_input,
        &["path", "file_path", "filePath", "abs_path"],
    )
    .or_else(|| tool.locations.first().cloned())
    .unwrap_or_default()
}

#[expect(
    clippy::too_many_lines,
    reason = "one arm per ACP tool kind, then one check that nothing required is empty"
)]
fn tool_kind(tool: &Tool) -> Result<ToolKind, String> {
    let input = &tool.raw_input;
    let kind = match tool.kind.as_deref() {
        Some("execute") => ToolKind::Shell {
            command: shell_command(tool)?,
            cwd: str_field(input, &["cwd"]),
            background: false,
        },
        Some("read") => {
            let offset = input["offset"].as_u64();
            let range = input["limit"]
                .as_u64()
                .filter(|limit| *limit > 0)
                .map(|limit| {
                    let start = offset.unwrap_or(0).saturating_add(1);
                    (start, start.saturating_add(limit - 1))
                });
            ToolKind::Read {
                path: tool_path(tool),
                range,
            }
        }
        Some(kind @ ("edit" | "delete" | "move")) => {
            let changes = if tool.changes.is_empty() {
                let op = match kind {
                    "delete" => FileOp::Delete,
                    "move" => FileOp::Move {
                        from: str_field(input, &["from", "source", "old_path"]).unwrap_or_default(),
                    },
                    _ => FileOp::Modify,
                };
                let paths = if tool.locations.is_empty() {
                    vec![tool_path(tool)]
                } else {
                    tool.locations.clone()
                };
                paths
                    .into_iter()
                    .filter(|path| !path.is_empty())
                    .map(|path| FileChange {
                        path,
                        op: op.clone(),
                        diff: None,
                    })
                    .collect()
            } else {
                tool.changes.clone()
            };
            ToolKind::Edit {
                changes: changes.into(),
            }
        }
        Some("search") => {
            if let Some(query) = str_field(input, &["searchTerm"]) {
                ToolKind::WebSearch { query }
            } else {
                let glob = str_field(input, &["globPattern"]).is_some()
                    || tool
                        .title
                        .as_deref()
                        .is_some_and(|title| title.starts_with("Find"));
                let list = tool
                    .title
                    .as_deref()
                    .is_some_and(|title| title.starts_with("List"));
                ToolKind::Search {
                    pattern: str_field(input, &["pattern", "globPattern", "query", "regex"])
                        .or_else(|| tool.title.clone())
                        .unwrap_or_default(),
                    path: str_field(input, &["path"]),
                    mode: if list {
                        SearchMode::List
                    } else if glob {
                        SearchMode::Glob
                    } else {
                        SearchMode::Grep
                    },
                }
            }
        }
        Some("fetch") => match str_field(input, &["url"]) {
            Some(url) => ToolKind::WebFetch { url },
            None => ToolKind::WebSearch {
                query: str_field(input, &["query"])
                    .or_else(|| tool.title.clone())
                    .unwrap_or_default(),
            },
        },
        _ => match (
            str_field(input, &["providerIdentifier", "server"]),
            str_field(input, &["toolName", "tool"]),
        ) {
            (Some(server), Some(tool_name)) => ToolKind::Mcp {
                server,
                tool: tool_name,
                input: (input.get("args").map(Value::to_string)).map(Into::into),
            },
            _ => ToolKind::Other {
                name: tool
                    .title
                    .clone()
                    .or_else(|| tool.kind.clone())
                    .unwrap_or_else(|| "tool".to_owned()),
                input: ((!input.is_null()).then(|| input.to_string())).map(Into::into),
            },
        },
    };
    let invalid =
        match &kind {
            ToolKind::Shell { command, .. } => command
                .inline()
                .expect("adapter command is inline before storage")
                .trim()
                .is_empty(),
            ToolKind::Read { path, .. } => path.trim().is_empty(),
            ToolKind::Edit { changes } => changes.is_empty()
                || changes
                    .inline()
                    .expect("new native changes are inline")
                    .iter()
                    .any(|change| {
                        change.path.trim().is_empty()
                            || matches!(&change.op,FileOp::Move {from} if from.trim().is_empty())
                    }),
            ToolKind::Search { pattern, .. } => pattern.trim().is_empty(),
            ToolKind::WebFetch { url } => url.trim().is_empty(),
            ToolKind::WebSearch { query } => query.trim().is_empty(),
            ToolKind::Other { name, .. } => name.trim().is_empty(),
            _ => false,
        };
    if invalid {
        Err("ACP tool omitted required native fields".into())
    } else {
        Ok(kind)
    }
}

fn file_change(kind: Option<&str>, block: &Value) -> Option<FileChange> {
    let path = block["path"].as_str()?.to_owned();
    let old = block["oldText"].as_str();
    let new = block["newText"].as_str().unwrap_or_default();
    let op = match (kind, old) {
        (_, None) => FileOp::Add,
        (Some("delete"), Some(_)) if new.is_empty() => FileOp::Delete,
        _ => FileOp::Modify,
    };
    let diff = similar::TextDiff::from_lines(old.unwrap_or_default(), new)
        .unified_diff()
        .header(&path, &path)
        .to_string();
    Some(FileChange {
        path,
        op,
        diff: ((!diff.is_empty()).then_some(diff)).map(Into::into),
    })
}
