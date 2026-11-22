use super::*;
use serde_json::json;

#[test]
fn an_unset_or_unknown_setting_means_claude_code() {
    assert_eq!(transport_of(None), CLAUDE_CODE);
    assert_eq!(transport_of(Some(&json!("claude_code"))), CLAUDE_CODE);
    assert_eq!(transport_of(Some(&json!("something else"))), CLAUDE_CODE);
    assert_eq!(transport_of(Some(&json!(true))), CLAUDE_CODE);
}

#[test]
fn the_claude_ai_transport_is_named_by_its_setting() {
    assert_eq!(transport_of(Some(&json!(CLAUDE_AI))), CLAUDE_AI);
}
