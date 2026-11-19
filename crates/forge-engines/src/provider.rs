use serde_json::Value;

const CONTEXT_CODES: [&str; 3] = [
    "context_length_exceeded",
    "model_context_window_exceeded",
    "request_too_large",
];

const PROMPT_TOO_LONG: &str = "prompt is too long";

pub(crate) fn exceeds_context(error: &Value) -> bool {
    let kind = error["type"].as_str();
    let code = error["code"].as_str();
    [kind, code]
        .into_iter()
        .flatten()
        .any(|code| CONTEXT_CODES.contains(&code))
        || (kind == Some("invalid_request_error")
            && error["message"].as_str().is_some_and(prompt_too_long))
}

fn prompt_too_long(message: &str) -> bool {
    message
        .get(..PROMPT_TOO_LONG.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(PROMPT_TOO_LONG))
}
