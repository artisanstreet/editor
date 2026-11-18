use serde_json::Value;

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
