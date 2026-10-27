//! claude.ai's event stream, mapped onto Forge's items.
//!
//! claude.ai's stream is a superset of the Messages API stream: the same
//! event names, plus `thinking_summary_delta`, which carries the provider's
//! own one-line recap of the model's thinking, `message_limit`, and
//! `tool_origin` on a tool call. That recap is the text claude.ai shows
//! above each tool group, and the public API never returns it.
//!
//! One completion carries a whole turn. The model answers, the provider
//! runs the tools it owns (its sandbox, web search, the account's
//! connectors) and the model answers again, all in the same stream. A tool
//! the provider runs is marked with a `tool_origin` and comes back answered
//! in a `tool_result` block. A tool the caller owns is left open: after its
//! block closes the stream only pings, waiting for the caller's result.

use std::collections::HashMap;

use artisan_api::{ItemKind, Output, Phase, Status, ToolKind, Usage};
use serde_json::Value;

use crate::claude::tools::tool_kind;
use crate::{EngineEvent, LiveKind};

use super::tools::Call;
use super::transport::Refusal;

/// Decodes one claude.ai turn, across every completion it takes.
#[derive(Default)]
pub struct ClaudeAiDecoder {
    /// The message being streamed, for the item keys.
    message: Option<String>,
    /// Where the message being streamed sits in claude.ai's conversation
    /// tree: its uuid, the uuid of the message it answers, and whether
    /// claude.ai saved it (it saves a message only once its stream stops).
    uuid: Option<String>,
    answers: Option<String>,
    saved: bool,
    usage: Usage,
    /// The blocks the stream has opened, by their index.
    blocks: HashMap<u64, Block>,
    /// The text blocks of the message being streamed, with their text.
    texts: Vec<(String, String)>,
    /// The caller's tool calls the model made, waiting for a result.
    calls: Vec<Call>,
    /// The provider's tool calls, by id, waiting for the provider's result.
    provided: HashMap<String, ItemKind>,
    /// The completion ended the turn: the model answered.
    ended: bool,
    /// claude.ai failed the completion in the stream.
    failure: Option<Refusal>,
    /// A tool from the account's claude.ai connectors the provider began.
    connector: Option<String>,
}

/// A block the stream has opened.
#[derive(Clone, Debug, Default)]
struct Block {
    /// The block's type.
    kind: String,
    /// The tool's id, for a tool call.
    id: String,
    /// The tool's name, for a tool call.
    name: String,
    /// Whether the provider runs the tool.
    provider: bool,
    /// The JSON a tool call has streamed.
    input: String,
}

impl ClaudeAiDecoder {
    /// The events one stream event means.
    ///
    /// Events the decoder does not know are ignored, so a newer claude.ai
    /// does not fail the run.
    pub fn feed(&mut self, event: &Value, out: &mut Vec<EngineEvent>) {
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

    /// Whether the model is waiting on tools the caller owns.
    #[must_use]
    pub const fn waiting_on_caller(&self) -> bool {
        !self.calls.is_empty()
    }

    /// Whether the model answered and the turn is over.
    #[must_use]
    pub const fn ended(&self) -> bool {
        self.ended
    }

    /// The error claude.ai sent in the stream, if it sent one.
    pub const fn take_failure(&mut self) -> Option<Refusal> {
        self.failure.take()
    }

    /// The connector tool the provider began running, if one began. The
    /// provider runs those without asking, so the caller decides whether
    /// the turn may go on.
    pub const fn take_connector(&mut self) -> Option<String> {
        self.connector.take()
    }

    /// Whether a connector tool began and has not been taken.
    #[must_use]
    pub const fn connector_started(&self) -> bool {
        self.connector.is_some()
    }

    /// The caller's tool calls, taken to run. The message's text so far is
    /// written out first: the stream says nothing more until the results
    /// are in, so it would otherwise only live in the preview.
    pub fn take_calls(&mut self, out: &mut Vec<EngineEvent>) -> Vec<Call> {
        self.settle(out);
        std::mem::take(&mut self.calls)
    }

    /// The message the next one follows in claude.ai's conversation tree:
    /// the model's message when claude.ai saved it, else the message it
    /// answered. claude.ai holds a stream that stops for the caller's tools
    /// open, so a message with tool calls is never saved; the results then
    /// follow the message that asked for them. Without a parent claude.ai
    /// starts the message from the root, and the model loses everything
    /// before it.
    #[must_use]
    pub fn leaf(&self) -> Option<&str> {
        if self.saved {
            self.uuid.as_deref()
        } else {
            self.answers.as_deref()
        }
    }

    /// The model started a message.
    fn started(&mut self, event: &Value, out: &mut Vec<EngineEvent>) {
        let model = str_at(&event["message"], "model")
            .unwrap_or_default()
            .to_owned();
        self.message = str_at(&event["message"], "id").map(str::to_owned);
        self.uuid = str_at(&event["message"], "uuid").map(str::to_owned);
        self.answers = str_at(&event["message"], "parent_uuid").map(str::to_owned);
        self.saved = false;
        self.texts.clear();
        self.ended = false;
        if !model.is_empty() {
            out.push(EngineEvent::Model { id: model });
        }
        self.usage(&event["message"]["usage"], out);
    }

    /// A content block opened.
    fn block_started(&mut self, event: &Value, out: &mut Vec<EngineEvent>) {
        let index = event["index"].as_u64().unwrap_or_default();
        let block = &event["content_block"];
        let kind = str_at(block, "type").unwrap_or_default().to_owned();
        match kind.as_str() {
            // The item opens with its first text, so a block that stays
            // empty is never shown.
            "text" => self.texts.push((self.key(index), String::new())),
            "tool_result" => {
                // The provider ran this one itself.
                if let Some(id) = str_at(block, "tool_use_id") {
                    self.provider_result(id, block, out);
                }
                return;
            }
            "tool_use" if is_connector(block) => {
                self.connector = Some(str_at(block, "name").unwrap_or("a connector").to_owned());
            }
            _ => {}
        }
        self.blocks.insert(
            index,
            Block {
                id: str_at(block, "id").unwrap_or_default().to_owned(),
                name: str_at(block, "name").unwrap_or_default().to_owned(),
                provider: !block["tool_origin"].is_null(),
                kind,
                input: String::new(),
            },
        );
    }

    /// A content block grew.
    fn block_delta(&mut self, event: &Value, out: &mut Vec<EngineEvent>) {
        let index = event["index"].as_u64().unwrap_or_default();
        let delta = &event["delta"];
        match str_at(delta, "type").unwrap_or_default() {
            "text_delta" => {
                let text = str_at(delta, "text").unwrap_or_default();
                if text.is_empty() {
                    return;
                }
                let key = self.key(index);
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
                // The recap is already a line: it is published whole, not
                // trimmed into one the way the model's own prose is, less a
                // closing full stop (an ellipsis stays).
                out.push(EngineEvent::Live {
                    target: LiveKind::Narration,
                    text: recap_line(summary),
                    append: false,
                });
            }
            "input_json_delta" => {
                if let Some(block) = self.blocks.get_mut(&index) {
                    block
                        .input
                        .push_str(str_at(delta, "partial_json").unwrap_or_default());
                }
            }
            _ => {}
        }
    }

    /// A content block closed. A tool call is only known here: claude.ai
    /// streams the tool's arguments, so the call is reported when it is
    /// whole.
    fn block_stopped(&mut self, event: &Value, out: &mut Vec<EngineEvent>) {
        let index = event["index"].as_u64().unwrap_or_default();
        let Some(block) = self.blocks.remove(&index) else {
            return;
        };
        if block.kind != "tool_use" {
            return;
        }
        let input: Value = serde_json::from_str(&block.input)
            .unwrap_or_else(|_| event["content_block"]["input"].clone());
        if block.provider {
            // The provider runs it on its own machines; the Editor shows it
            // with what it returned.
            let kind = ItemKind::Tool {
                tool: provider_tool(&block.name, &input),
                title: None,
                output: None,
                exit_code: None,
                approval: None,
            };
            self.provided.insert(block.id.clone(), kind.clone());
            out.push(EngineEvent::Item {
                key: block.id,
                parent: None,
                status: Status::Running,
                kind,
            });
            return;
        }
        if block.name == "TodoWrite" {
            // The todo list is one item per run, which the run writes when
            // it takes the call (`tools::item_key`); an item under the
            // call's id would stay running beside it.
            self.calls.push(Call {
                id: block.id,
                name: block.name,
                input,
            });
            return;
        }
        let kind = match tool_kind(&block.name, &input) {
            Ok(tool) => ItemKind::Tool {
                tool,
                title: input["description"].as_str().map(str::to_owned),
                output: None,
                exit_code: None,
                approval: None,
            },
            // A call Forge does not offer, or one missing what it needs: it
            // is answered with why, and shown as it was asked.
            Err(_) => ItemKind::Tool {
                tool: ToolKind::Other {
                    name: block.name.clone(),
                    input: Some(input.to_string().into()),
                },
                title: None,
                output: None,
                exit_code: None,
                approval: None,
            },
        };
        self.calls.push(Call {
            id: block.id.clone(),
            name: block.name,
            input,
        });
        out.push(EngineEvent::Item {
            key: block.id,
            parent: None,
            status: Status::Running,
            kind,
        });
    }

    /// The provider answered one of its own tools.
    fn provider_result(&mut self, id: &str, block: &Value, out: &mut Vec<EngineEvent>) {
        let Some(kind) = self.provided.remove(id) else {
            return;
        };
        // Text parts carry a tool's output; a web search returns its pages
        // as `knowledge` parts, shown one per line.
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

    /// The message ended. Its text is the reply when the model stopped to
    /// answer, and a note when it stopped for a tool.
    fn message_delta(&mut self, event: &Value, out: &mut Vec<EngineEvent>) {
        let reason = str_at(&event["delta"], "stop_reason");
        self.usage(&event["usage"], out);
        if reason.is_some_and(|reason| reason != "tool_use") {
            self.ended = true;
        }
        self.settle(out);
    }

    /// claude.ai says where the account stands against its limits.
    fn message_limit(&mut self, event: &Value) {
        let limit = &event["message_limit"];
        if str_at(limit, "type") == Some("exceeded_limit") {
            self.failure = Some(super::transport::limit_refusal(limit));
        }
    }

    /// claude.ai failed the completion partway.
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

    /// Writes the message's text out: the last of it is the reply when the
    /// model answered, the rest are notes.
    fn settle(&mut self, out: &mut Vec<EngineEvent>) {
        let texts: Vec<_> = std::mem::take(&mut self.texts)
            .into_iter()
            .filter(|(_, text)| !text.is_empty())
            .collect();
        let last = texts.len().saturating_sub(1);
        for (index, (key, text)) in texts.into_iter().enumerate() {
            let phase = if self.ended && index == last {
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

    /// The turn's token counts, when claude.ai reports them. It often does
    /// not; nothing is reported then rather than zeros.
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
        // What the message read is what the context holds, as Claude Code
        // reports it: the fresh input plus what came from the cache.
        if usage["input_tokens"].is_u64() {
            self.usage.context_used =
                Some(count("input_tokens") + count("cache_read_input_tokens") + written);
        }
        if self.usage != before {
            out.push(EngineEvent::Usage(self.usage.clone()));
        }
    }

    /// The item key for a block of the current message.
    fn key(&self, index: u64) -> String {
        format!("{}:{index}", self.message.as_deref().unwrap_or_default())
    }
}

/// What a tool the provider runs is, for the Editor: its web search and
/// fetch read as Claude Code's, anything else by name.
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

/// Whether a tool call is one of the account's claude.ai connectors (an
/// MCP server claude.ai runs), rather than the provider's own sandbox or
/// web tools or the caller's.
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

/// The provider's recap as the highlight shows it: trimmed, and without a
/// single closing full stop. An ellipsis ("..." or "…") is left alone.
fn recap_line(summary: &str) -> String {
    let line = summary.trim();
    match line.strip_suffix('.') {
        Some(rest) if !rest.ends_with('.') => rest.trim_end().to_owned(),
        _ => line.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn feed(decoder: &mut ClaudeAiDecoder, events: &[Value]) -> Vec<EngineEvent> {
        let mut out = Vec::new();
        for event in events {
            decoder.feed(event, &mut out);
        }
        out
    }

    #[test]
    fn a_recap_loses_its_closing_full_stop_only() {
        assert_eq!(recap_line("Reading the config."), "Reading the config");
        assert_eq!(recap_line("  Reading the config.  "), "Reading the config");
        assert_eq!(recap_line("Reading the config"), "Reading the config");
        assert_eq!(recap_line("Reading the config..."), "Reading the config...");
        assert_eq!(
            recap_line("Reading the config\u{2026}"),
            "Reading the config\u{2026}"
        );
        assert_eq!(recap_line("Is it v2.1?"), "Is it v2.1?");
    }

    #[test]
    fn the_providers_recap_is_published_whole() {
        let mut decoder = ClaudeAiDecoder::default();
        let out = feed(
            &mut decoder,
            &[json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {
                    "type": "thinking_summary_delta",
                    "summary": { "summary": "Checking which setup steps the task actually requires." }
                }
            })],
        );
        assert_eq!(
            out,
            vec![EngineEvent::Live {
                target: LiveKind::Narration,
                text: "Checking which setup steps the task actually requires".into(),
                append: false
            }]
        );
    }

    #[test]
    fn the_reply_settles_with_its_text() {
        let mut decoder = ClaudeAiDecoder::default();
        let out = feed(
            &mut decoder,
            &[
                json!({ "type": "message_start", "message": { "id": "msg_1", "model": "claude-opus-5-5" } }),
                json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } }),
                json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "Hello" } }),
                json!({ "type": "content_block_stop", "index": 0 }),
                json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn" } }),
                json!({ "type": "message_stop" }),
            ],
        );
        assert!(decoder.ended());
        assert_eq!(
            out.last(),
            Some(&EngineEvent::Item {
                key: "msg_1:0".into(),
                parent: None,
                status: Status::Done,
                kind: ItemKind::AssistantMessage {
                    text: "Hello".into(),
                    phase: Phase::Final
                }
            })
        );
        assert!(
            !out.iter()
                .any(|event| matches!(event, EngineEvent::Usage(_))),
            "no usage is reported when claude.ai reports none"
        );
    }

    #[test]
    fn text_before_a_callers_tool_is_kept_when_the_call_is_taken() {
        let mut decoder = ClaudeAiDecoder::default();
        feed(
            &mut decoder,
            &[
                json!({ "type": "message_start", "message": { "id": "msg_1" } }),
                json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text" } }),
                json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": "Let me look." } }),
                json!({ "type": "content_block_stop", "index": 0 }),
                json!({ "type": "content_block_start", "index": 1, "content_block": { "type": "tool_use", "id": "toolu_1", "name": "Read", "tool_origin": null } }),
                json!({ "type": "content_block_delta", "index": 1, "delta": { "type": "input_json_delta", "partial_json": "{\"file_path\":\"/a\"}" } }),
                json!({ "type": "content_block_stop", "index": 1 }),
            ],
        );
        assert!(decoder.waiting_on_caller());
        let mut out = Vec::new();
        let calls = decoder.take_calls(&mut out);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].input["file_path"], "/a");
        assert_eq!(
            out,
            vec![EngineEvent::Item {
                key: "msg_1:0".into(),
                parent: None,
                status: Status::Done,
                kind: ItemKind::AssistantMessage {
                    text: "Let me look.".into(),
                    phase: Phase::Commentary
                }
            }]
        );
    }

    #[test]
    fn a_tool_the_provider_runs_is_shown_with_its_result_and_never_the_callers() {
        let mut decoder = ClaudeAiDecoder::default();
        let out = feed(
            &mut decoder,
            &[
                json!({ "type": "message_start", "message": { "id": "msg_1" } }),
                json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "tool_use", "id": "toolu_1", "name": "bash_tool", "input": {}, "tool_origin": "first_party" } }),
                json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "input_json_delta", "partial_json": "{\"command\":\"ls\"}" } }),
                json!({ "type": "content_block_stop", "index": 0 }),
            ],
        );
        assert!(
            !decoder.waiting_on_caller(),
            "the provider's tool is not Forge's to answer, even before its result"
        );
        assert!(matches!(
            out.last(),
            Some(EngineEvent::Item { status: Status::Running, kind: ItemKind::Tool { tool: ToolKind::Other { name, .. }, .. }, .. }) if name == "bash_tool"
        ));
        let out = feed(
            &mut decoder,
            &[json!({
                "type": "content_block_start", "index": 1,
                "content_block": { "type": "tool_result", "tool_use_id": "toolu_1", "name": "bash_tool", "content": [{ "type": "text", "text": "file.txt" }] }
            })],
        );
        assert!(matches!(
            out.as_slice(),
            [EngineEvent::Item {
                status: Status::Done,
                kind: ItemKind::Tool {
                    output: Some(_),
                    ..
                },
                ..
            }]
        ));
    }

    #[test]
    fn the_providers_web_search_shows_its_query_and_the_pages_it_found() {
        let mut decoder = ClaudeAiDecoder::default();
        let out = feed(
            &mut decoder,
            &[
                json!({ "type": "message_start", "message": { "id": "msg_1" } }),
                json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "tool_use", "id": "toolu_w", "name": "web_search", "input": {}, "tool_origin": "first_party", "mcp_server_url": null } }),
                json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "input_json_delta", "partial_json": "{\"query\":\"latest Rust release\"}" } }),
                json!({ "type": "content_block_stop", "index": 0 }),
                json!({
                    "type": "content_block_start", "index": 1,
                    "content_block": { "type": "tool_result", "tool_use_id": "toolu_w", "name": "web_search", "tool_origin": "first_party", "content": [
                        { "type": "knowledge", "title": "Rust 1.99.0 released", "url": "https://lwn.net/Articles/1098068/" }
                    ] }
                }),
            ],
        );
        assert!(!decoder.waiting_on_caller());
        assert!(
            !decoder.connector_started(),
            "web search is not a connector"
        );
        let Some(EngineEvent::Item {
            status: Status::Done,
            kind:
                ItemKind::Tool {
                    tool: ToolKind::WebSearch { query },
                    output: Some(output),
                    ..
                },
            ..
        }) = out.last()
        else {
            panic!("the search settles as a web search: {out:?}");
        };
        assert_eq!(query, "latest Rust release");
        assert_eq!(
            output,
            &Output::from_text("Rust 1.99.0 released — https://lwn.net/Articles/1098068/")
        );
    }

    #[test]
    fn an_exceeded_limit_fails_the_turn_with_its_reset() {
        let mut decoder = ClaudeAiDecoder::default();
        feed(
            &mut decoder,
            &[
                json!({ "type": "message_limit", "message_limit": { "type": "exceeded_limit", "representativeClaim": "five_hour", "resetsAt": 100 } }),
            ],
        );
        assert_eq!(
            decoder.take_failure(),
            Some(Refusal::UsageLimit {
                resets_at: Some(100_000),
                message: "session usage limit".to_owned()
            })
        );
    }

    #[test]
    fn streamed_concurrency_limits_are_temporary_for_both_event_shapes() {
        let limit = json!({"type":"exceeded_limit", "representativeClaim":"concurrents"});
        for event in [
            json!({"type":"message_limit", "message_limit":limit}),
            json!({"type":"error", "error":{"type":"rate_limit_error", "message":limit.to_string()}}),
        ] {
            let mut decoder = ClaudeAiDecoder::default();
            feed(&mut decoder, &[event]);
            assert_eq!(
                decoder.take_failure(),
                Some(Refusal::RateLimited(
                    "Claude.ai has too many active responses. Try again shortly.".to_owned()
                ))
            );
        }
    }

    #[test]
    fn an_unknown_event_is_ignored() {
        let mut decoder = ClaudeAiDecoder::default();
        assert!(feed(&mut decoder, &[json!({ "type": "something_new" })]).is_empty());
    }
}
