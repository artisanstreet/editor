use std::collections::HashMap;

use artisan_api::{ErrorKind, ItemKind, Output, Phase, Status, ToolKind, Usage};
use serde_json::Value;

use crate::claude::tools::tool_kind;
use crate::{EngineEvent, LiveKind};

use super::tools::Call;
use super::transport::Refusal;

#[derive(Default)]
pub struct ClaudeAiDecoder {
    message: Option<String>,
    uuid: Option<String>,
    answers: Option<String>,
    saved: bool,
    usage: Usage,
    blocks: HashMap<u64, Block>,
    broken: Option<ProtocolError>,
    texts: Vec<(String, String)>,
    calls: Vec<Call>,
    provided: HashMap<String, ItemKind>,
    stop: Option<Stop>,
    failure: Option<Refusal>,
    connector: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    Completed,
    ToolUse,
    Truncated,
    Refused,
    Unsupported(String),
}

impl Stop {
    fn of(reason: &str) -> Self {
        match reason {
            "end_turn" | "stop_sequence" => Self::Completed,
            "tool_use" => Self::ToolUse,
            "max_tokens" => Self::Truncated,
            "refusal" => Self::Refused,
            other => Self::Unsupported(other.to_owned()),
        }
    }
}

#[derive(Debug)]
enum Block {
    Text {
        key: String,
    },
    Tool {
        id: String,
        execution: Execution,
        start: Value,
        input: String,
    },
    Other,
}

#[derive(Debug)]
enum Execution {
    Caller(String),
    Provider(String),
}

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("claude.ai sent a {event} without a block index")]
    NoIndex { event: &'static str },
    #[error("claude.ai opened block {index} while it was still open")]
    IndexInUse { index: u64 },
    #[error("claude.ai sent a {event} for block {index}, which is not an open {expected} block")]
    NotOpen {
        event: &'static str,
        index: u64,
        expected: &'static str,
    },
    #[error("claude.ai closed block {index}, which is not open")]
    StopNotOpen { index: u64 },
    #[error("claude.ai opened tool block {index} without an id")]
    ToolWithoutId { index: u64 },
    #[error("claude.ai opened tool block {index} ({id}) without a name")]
    ToolWithoutName { index: u64, id: String },
    #[error("claude.ai streamed input for tool {name} ({id}) that is not JSON")]
    InvalidToolInput {
        id: String,
        name: String,
        #[source]
        source: serde_json::Error,
    },
}

impl ClaudeAiDecoder {
    pub fn feed(&mut self, event: &Value, out: &mut Vec<EngineEvent>) {
        if self.broken.is_some() {
            return;
        }
        match str_at(event, "type").unwrap_or_default() {
            "message_start" => self.started(event, out),
            "content_block_start" => self.block_started(event, out),
            "content_block_delta" => self.block_delta(event, out),
            "content_block_stop" => self.block_stopped(event, out),
            "message_delta" => self.message_delta(event, out),
            "message_limit" => self.message_limit(event),
            "message_stop" => {
                self.saved = true;
                self.settle(out);
            }
            "error" => self.failed(event),
            _ => {}
        }
    }

    #[must_use]
    pub const fn waiting_on_caller(&self) -> bool {
        !self.calls.is_empty()
    }

    #[must_use]
    pub const fn stop(&self) -> Option<&Stop> {
        self.stop.as_ref()
    }

    #[must_use]
    pub const fn broken(&self) -> bool {
        self.broken.is_some()
    }

    pub const fn take_protocol_error(&mut self) -> Option<ProtocolError> {
        self.broken.take()
    }

    pub const fn take_failure(&mut self) -> Option<Refusal> {
        self.failure.take()
    }

    pub const fn take_connector(&mut self) -> Option<String> {
        self.connector.take()
    }

    #[must_use]
    pub const fn connector_started(&self) -> bool {
        self.connector.is_some()
    }

    pub fn take_calls(&mut self, out: &mut Vec<EngineEvent>) -> Vec<Call> {
        self.settle(out);
        std::mem::take(&mut self.calls)
    }

    #[must_use]
    pub fn leaf(&self) -> Option<&str> {
        if self.saved {
            self.uuid.as_deref()
        } else {
            self.answers.as_deref()
        }
    }

    fn started(&mut self, event: &Value, out: &mut Vec<EngineEvent>) {
        let model = str_at(&event["message"], "model")
            .unwrap_or_default()
            .to_owned();
        self.message = str_at(&event["message"], "id").map(str::to_owned);
        self.uuid = str_at(&event["message"], "uuid").map(str::to_owned);
        self.answers = str_at(&event["message"], "parent_uuid").map(str::to_owned);
        self.saved = false;
        self.texts.clear();
        self.blocks.clear();
        self.stop = None;
        if !model.is_empty() {
            out.push(EngineEvent::Model { id: model });
        }
        self.usage(&event["message"]["usage"], out);
    }

    fn index(&mut self, event: &Value, name: &'static str) -> Option<u64> {
        let index = event["index"].as_u64();
        if index.is_none() {
            self.broken = Some(ProtocolError::NoIndex { event: name });
        }
        index
    }

    fn block_started(&mut self, event: &Value, out: &mut Vec<EngineEvent>) {
        let Some(index) = self.index(event, "content_block_start") else {
            return;
        };
        if self.blocks.contains_key(&index) {
            self.broken = Some(ProtocolError::IndexInUse { index });
            return;
        }
        let block = &event["content_block"];
        let state = match str_at(block, "type").unwrap_or_default() {
            "text" => {
                let key = self.key(index);
                self.texts.push((key.clone(), String::new()));
                Block::Text { key }
            }
            "tool_use" => match self.tool(index, block) {
                Ok(tool) => tool,
                Err(error) => {
                    self.broken = Some(error);
                    return;
                }
            },
            "tool_result" => {
                if let Some(id) = str_at(block, "tool_use_id") {
                    self.provider_result(id, block, out);
                }
                Block::Other
            }
            _ => Block::Other,
        };
        self.blocks.insert(index, state);
    }

    fn tool(&mut self, index: u64, block: &Value) -> Result<Block, ProtocolError> {
        let id = str_at(block, "id")
            .filter(|id| !id.is_empty())
            .ok_or(ProtocolError::ToolWithoutId { index })?
            .to_owned();
        let Some(name) = str_at(block, "name").filter(|name| !name.is_empty()) else {
            return Err(ProtocolError::ToolWithoutName { index, id });
        };
        if is_connector(block) {
            self.connector = Some(name.to_owned());
        }
        let execution = if block["tool_origin"].is_null() {
            Execution::Caller(name.to_owned())
        } else {
            Execution::Provider(name.to_owned())
        };
        Ok(Block::Tool {
            id,
            execution,
            start: block["input"].clone(),
            input: String::new(),
        })
    }

    fn block_delta(&mut self, event: &Value, out: &mut Vec<EngineEvent>) {
        let delta = &event["delta"];
        match str_at(delta, "type").unwrap_or_default() {
            "text_delta" => {
                let Some(index) = self.index(event, "text_delta") else {
                    return;
                };
                let Some(Block::Text { key }) = self.blocks.get(&index) else {
                    self.broken = Some(ProtocolError::NotOpen {
                        event: "text_delta",
                        index,
                        expected: "text",
                    });
                    return;
                };
                let key = key.clone();
                let text = str_at(delta, "text").unwrap_or_default();
                if text.is_empty() {
                    return;
                }
                if let Some((_, streamed)) = self.texts.iter_mut().find(|(k, _)| *k == key) {
                    if streamed.is_empty() {
                        out.push(EngineEvent::Item {
                            key: key.clone(),
                            parent: None,
                            status: Status::Running,
                            kind: ItemKind::AssistantMessage {
                                text: String::new().into(),
                                phase: Phase::Commentary,
                            },
                        });
                    }
                    streamed.push_str(text);
                }
                out.push(EngineEvent::Live {
                    target: LiveKind::Text { key },
                    text: text.to_owned(),
                    append: true,
                });
            }
            "thinking_summary_delta" => {
                let summary = str_at(&delta["summary"], "summary").unwrap_or_default();
                if summary.trim().is_empty() {
                    return;
                }
                out.push(EngineEvent::Live {
                    target: LiveKind::Narration,
                    text: recap_line(summary),
                    append: false,
                });
            }
            "input_json_delta" => {
                let Some(index) = self.index(event, "input_json_delta") else {
                    return;
                };
                let Some(Block::Tool { input, .. }) = self.blocks.get_mut(&index) else {
                    self.broken = Some(ProtocolError::NotOpen {
                        event: "input_json_delta",
                        index,
                        expected: "tool",
                    });
                    return;
                };
                input.push_str(str_at(delta, "partial_json").unwrap_or_default());
            }
            _ => {}
        }
    }

    fn block_stopped(&mut self, event: &Value, out: &mut Vec<EngineEvent>) {
        let Some(index) = self.index(event, "content_block_stop") else {
            return;
        };
        let Some(block) = self.blocks.remove(&index) else {
            self.broken = Some(ProtocolError::StopNotOpen { index });
            return;
        };
        let Block::Tool {
            id,
            execution,
            start,
            input,
        } = block
        else {
            return;
        };
        let input = if input.trim().is_empty() {
            if start.is_object() {
                start
            } else {
                Value::Object(serde_json::Map::new())
            }
        } else {
            match serde_json::from_str(&input) {
                Ok(input) => input,
                Err(source) => {
                    let (Execution::Caller(name) | Execution::Provider(name)) = execution;
                    self.broken = Some(ProtocolError::InvalidToolInput { id, name, source });
                    return;
                }
            }
        };
        let name = match execution {
            Execution::Provider(name) => {
                let kind = ItemKind::Tool {
                    tool: provider_tool(&name, &input),
                    title: None,
                    output: None,
                    exit_code: None,
                    approval: None,
                };
                self.provided.insert(id.clone(), kind.clone());
                out.push(EngineEvent::Item {
                    key: id,
                    parent: None,
                    status: Status::Running,
                    kind,
                });
                return;
            }
            Execution::Caller(name) => name,
        };
        if name == "TodoWrite" {
            self.calls.push(Call { id, name, input });
            return;
        }
        let kind = match tool_kind(&name, &input) {
            Ok(tool) => ItemKind::Tool {
                tool,
                title: input["description"].as_str().map(str::to_owned),
                output: None,
                exit_code: None,
                approval: None,
            },
            Err(_) => ItemKind::Tool {
                tool: ToolKind::Other {
                    name: name.clone(),
                    input: Some(input.to_string().into()),
                },
                title: None,
                output: None,
                exit_code: None,
                approval: None,
            },
        };
        self.calls.push(Call {
            id: id.clone(),
            name,
            input,
        });
        out.push(EngineEvent::Item {
            key: id,
            parent: None,
            status: Status::Running,
            kind,
        });
    }

    fn provider_result(&mut self, id: &str, block: &Value, out: &mut Vec<EngineEvent>) {
        let Some(kind) = self.provided.remove(id) else {
            return;
        };
        let text = block["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|part| match str_at(part, "type") {
                Some("knowledge") => Some(match (str_at(part, "title"), str_at(part, "url")) {
                    (Some(title), Some(url)) => format!("{title} — {url}"),
                    (title, url) => title.or(url)?.to_owned(),
                }),
                _ => part["text"].as_str().map(str::to_owned),
            })
            .collect::<Vec<_>>()
            .join("\n");
        let failed = block["is_error"].as_bool() == Some(true);
        let kind = if let ItemKind::Tool {
            tool,
            title,
            approval,
            ..
        } = kind
        {
            ItemKind::Tool {
                tool,
                title,
                output: (!text.is_empty()).then(|| Output::from_text(&text)),
                exit_code: None,
                approval,
            }
        } else {
            kind
        };
        out.push(EngineEvent::Item {
            key: id.to_owned(),
            parent: None,
            status: if failed { Status::Failed } else { Status::Done },
            kind,
        });
    }

    fn message_delta(&mut self, event: &Value, out: &mut Vec<EngineEvent>) {
        self.usage(&event["usage"], out);
        if let Some(reason) = str_at(&event["delta"], "stop_reason") {
            self.stop = Some(Stop::of(reason));
        }
        self.settle(out);
        if self.stop == Some(Stop::Refused) {
            out.push(EngineEvent::Item {
                key: format!("{}:refusal", self.message.as_deref().unwrap_or_default()),
                parent: None,
                status: Status::Done,
                kind: ItemKind::Error {
                    kind: ErrorKind::Refusal,
                    message: "Claude declined this request.".to_owned().into(),
                    retryable: false,
                },
            });
        }
    }

    fn message_limit(&mut self, event: &Value) {
        let limit = &event["message_limit"];
        if str_at(limit, "type") == Some("exceeded_limit") {
            self.failure = Some(super::transport::limit_refusal(limit));
        }
    }

    fn failed(&mut self, event: &Value) {
        let error = &event["error"];
        let message = str_at(error, "message")
            .unwrap_or("claude.ai stopped the answer")
            .to_owned();
        self.failure = Some(match str_at(error, "type").unwrap_or_default() {
            "overloaded_error" | "api_error" => Refusal::Overloaded(message),
            "rate_limit_error" => super::transport::rate_limit(&message),
            _ => Refusal::Other(format!("claude.ai stopped the answer: {message}")),
        });
    }

    fn settle(&mut self, out: &mut Vec<EngineEvent>) {
        let texts: Vec<_> = std::mem::take(&mut self.texts)
            .into_iter()
            .filter(|(_, text)| !text.is_empty())
            .collect();
        let last = texts.len().saturating_sub(1);
        let completed = self.stop == Some(Stop::Completed);
        for (index, (key, text)) in texts.into_iter().enumerate() {
            let phase = if completed && index == last {
                Phase::Final
            } else {
                Phase::Commentary
            };
            out.push(EngineEvent::Item {
                key,
                parent: None,
                status: Status::Done,
                kind: ItemKind::AssistantMessage {
                    text: text.into(),
                    phase,
                },
            });
        }
    }

    fn usage(&mut self, usage: &Value, out: &mut Vec<EngineEvent>) {
        if !usage.is_object() {
            return;
        }
        let count = |field: &str| usage[field].as_u64().unwrap_or_default();
        let before = self.usage.clone();
        self.usage.input += count("input_tokens");
        self.usage.output += count("output_tokens");
        self.usage.reasoning += usage["output_tokens_details"]["thinking_tokens"]
            .as_u64()
            .unwrap_or_default();
        self.usage.cache_read += count("cache_read_input_tokens");
        let written = count("cache_creation_input_tokens");
        if written > 0 {
            self.usage.cache_write = Some(self.usage.cache_write.unwrap_or_default() + written);
        }
        if usage["input_tokens"].is_u64() {
            self.usage.context_used =
                Some(count("input_tokens") + count("cache_read_input_tokens") + written);
        }
        if self.usage != before {
            out.push(EngineEvent::Usage(self.usage.clone()));
        }
    }

    fn key(&self, index: u64) -> String {
        format!("{}:{index}", self.message.as_deref().unwrap_or_default())
    }
}

fn provider_tool(name: &str, input: &Value) -> ToolKind {
    match (name, str_at(input, "query"), str_at(input, "url")) {
        ("web_search", Some(query), _) => ToolKind::WebSearch {
            query: query.to_owned(),
        },
        ("web_fetch", _, Some(url)) => ToolKind::WebFetch {
            url: url.to_owned(),
        },
        _ => ToolKind::Other {
            name: name.to_owned(),
            input: Some(input.to_string().into()),
        },
    }
}

fn is_connector(block: &Value) -> bool {
    !block["tool_origin"].is_null()
        && (block["mcp_server_url"]
            .as_str()
            .is_some_and(|url| !url.is_empty())
            || block["is_mcp_app"].as_bool() == Some(true))
}

fn str_at<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}

fn recap_line(summary: &str) -> String {
    let line = summary.trim();
    match line.strip_suffix('.') {
        Some(rest) if !rest.ends_with('.') => rest.trim_end().to_owned(),
        _ => line.to_owned(),
    }
}

#[cfg(test)]
#[path = "../../tests/claudeai/decoder.rs"]
mod tests;
