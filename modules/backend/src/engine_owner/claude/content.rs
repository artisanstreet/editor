//! Content projection for Claude assistant frames and stream content blocks.
//!
//! A buffered `assistant` frame may carry text, tool uses, and thinking in
//! one content array beside one per-response `usage` object. The projection
//! keeps every supported part in provider order and the usage sample once,
//! so a mixed frame never loses thinking to text (or the reverse) and never
//! counts usage twice. Stream `content_block_*` frames keep their block
//! index: message identity alone cannot tell several thinking stretches of
//! one message apart. Signatures, redacted payloads, and unknown fields never
//! cross this boundary.

use serde_json::{Map, Value};

use super::protocol::ClaudeEvent;
use super::usage::ClaudeUsageSample;

/// One supported part of a buffered assistant frame, in provider order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ClaudeAssistantContent {
    /// The frame's joined public text with its verbatim TypeScript phase
    /// (`commentary` when the same frame carries a tool use, otherwise
    /// `unspecified`), placed at the first text part.
    Text { text: String, phase: &'static str },
    /// One thinking block's text. Empty when the display omits thinking;
    /// public summary prose only when the launch requested `summarized`.
    Thinking { text: String },
}

/// One buffered assistant frame after bounded decoding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClaudeAssistantFrame {
    /// Provider message identity (the stream `message_start` id), if valid.
    pub(crate) message_id: Option<String>,
    /// Supported content parts in provider order.
    pub(crate) content: Vec<ClaudeAssistantContent>,
    /// Per-response usage, projected once per frame.
    pub(crate) usage: Option<ClaudeUsageSample>,
}

impl ClaudeAssistantFrame {
    /// Returns the frame's non-empty public text and phase, if any.
    pub(crate) fn text(&self) -> Option<(&str, &'static str)> {
        self.content.iter().find_map(|part| match part {
            ClaudeAssistantContent::Text { text, phase } if !text.is_empty() => {
                Some((text.as_str(), *phase))
            }
            _ => None,
        })
    }
}

/// Projects one assistant `message` object into its ordered content.
///
/// Text parts join into one part at the first text position, preserving the
/// existing text and phase attribution; each thinking block keeps its own
/// part. Returns `None` when the frame carries nothing supported at all.
pub(crate) fn assistant_content(
    message_id: Option<String>,
    content: &[Value],
    usage: Option<ClaudeUsageSample>,
) -> Option<ClaudeAssistantFrame> {
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut text_at = None;
    let mut has_tool_use = false;
    for item in content {
        match item.get("type").and_then(Value::as_str).unwrap_or("") {
            "text" => {
                if let Some(fragment) = item.get("text").and_then(Value::as_str) {
                    text_at.get_or_insert(parts.len());
                    text.push_str(fragment);
                }
            }
            "tool_use" => has_tool_use |= item.get("id").and_then(Value::as_str).is_some(),
            "thinking" => parts.push(ClaudeAssistantContent::Thinking {
                text: item
                    .get("thinking")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
            }),
            _ => {}
        }
    }
    if let Some(position) = text_at.filter(|_| !text.is_empty()) {
        let phase = if has_tool_use {
            "commentary"
        } else {
            "unspecified"
        };
        parts.insert(position, ClaudeAssistantContent::Text { text, phase });
    }
    if parts.is_empty() && usage.is_none() {
        return None;
    }
    Some(ClaudeAssistantFrame {
        message_id,
        content: parts,
        usage,
    })
}

fn block_index(event: &Map<String, Value>) -> Option<u64> {
    event.get("index").and_then(Value::as_u64)
}

/// Decodes one stream `content_block_start`: only thinking blocks open a
/// tracked stretch; every other block kind stays bookkeeping.
pub(crate) fn decode_block_start(event: &Map<String, Value>) -> ClaudeEvent {
    let kind = event
        .get("content_block")
        .and_then(|block| block.get("type"))
        .and_then(Value::as_str);
    match (kind, block_index(event)) {
        (Some("thinking"), Some(index)) => ClaudeEvent::ThinkingStarted { index },
        _ => ClaudeEvent::Unknown,
    }
}

/// Decodes one stream `content_block_stop` for any block kind.
pub(crate) fn decode_block_stop(event: &Map<String, Value>) -> ClaudeEvent {
    match block_index(event) {
        Some(index) => ClaudeEvent::ContentBlockStopped { index },
        None => ClaudeEvent::Unknown,
    }
}

/// Decodes one streamed `thinking_delta`; empty fragments (omitted display,
/// estimate-only deltas) and index-less deltas carry nothing to project.
pub(crate) fn decode_thinking_delta(
    event: &Map<String, Value>,
    delta: &Map<String, Value>,
) -> ClaudeEvent {
    match (
        block_index(event),
        delta.get("thinking").and_then(Value::as_str),
    ) {
        (Some(index), Some(text)) if !text.is_empty() => ClaudeEvent::ThinkingDelta {
            index,
            text: text.to_owned(),
        },
        _ => ClaudeEvent::Unknown,
    }
}
