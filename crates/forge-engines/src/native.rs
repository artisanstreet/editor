//! Readings of provider JSON that more than one decoder needs.

use serde_json::Value;

/// The text of a provider content value: a string, one content block, or
/// the text parts of a block list (MCP `content`, Codex `contentItems`,
/// OpenCode tool `content`, Claude `tool_result` blocks) joined by newlines.
/// Parts without text (images, audio, resources) are skipped; `None` when
/// nothing carries text.
pub fn content_text(content: &Value) -> Option<String> {
    match content {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => {
            let texts: Vec<&str> = parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect();
            (!texts.is_empty()).then(|| texts.join("\n"))
        }
        Value::Object(block) => block.get("text").and_then(Value::as_str).map(str::to_owned),
        Value::Null | Value::Bool(_) | Value::Number(_) => None,
    }
}
