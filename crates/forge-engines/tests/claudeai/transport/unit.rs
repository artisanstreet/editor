use super::*;
use reqwest::StatusCode;

#[test]
fn the_body_carries_the_fields_claude_ai_reads() {
    let request = Request {
        prompt: "hello".to_owned(),
        model: "claude-opus-5-5".to_owned(),
        thinking_mode: "auto".to_owned(),
        effort: Some("high".to_owned()),
        tools: Vec::new(),
        tool_states: Vec::new(),
        files: vec!["f1".to_owned()],
        parent: Some("m1".to_owned()),
    };
    let body = request.body();
    assert_eq!(body["prompt"], "hello");
    assert_eq!(body["parent_message_uuid"], "m1");
    assert_eq!(body["files"], json!(["f1"]));
    assert_eq!(body["rendering_mode"], "messages");
}

#[test]
fn a_cloudflare_page_is_a_browser_check_not_a_wall_of_html() {
    let page = r#"<!DOCTYPE html><html><head><title>Just a moment...</title></head><body><script src="/cdn-cgi/challenge-platform/h/b"></script></body></html>"#;
    let refused = refusal(StatusCode::FORBIDDEN, false, page);
    assert_eq!(refused, Refusal::Challenge);
    assert!(!refused.to_string().contains('<'));
    assert_eq!(
        refusal(StatusCode::FORBIDDEN, true, ""),
        Refusal::Challenge,
        "Cloudflare's header says so too"
    );
}

#[test]
fn a_missing_conversation_is_told_apart() {
    let body = r#"{"type":"error","error":{"type":"not_found_error","message":"This conversation could not be found.","details":{"error_code":"chat_conversation_not_found"}}}"#;
    assert_eq!(
        refusal(StatusCode::NOT_FOUND, false, body),
        Refusal::ConversationGone
    );
}

#[test]
fn a_usage_limit_says_when_it_resets() {
    let body = r#"{"type":"error","error":{"type":"rate_limit_error","message":"{\"type\":\"exceeded_limit\",\"representativeClaim\":\"five_hour\",\"resetsAt\":1792620000}"}}"#;
    assert_eq!(
        refusal(StatusCode::TOO_MANY_REQUESTS, false, body),
        Refusal::UsageLimit {
            resets_at: Some(1_792_620_000_000),
            message: "session usage limit".to_owned()
        }
    );
}

#[test]
fn a_concurrency_limit_is_not_exhausted_usage() {
    let body = json!({"error":{"type":"rate_limit_error", "message":json!({"type":"exceeded_limit", "representativeClaim":"concurrents"}).to_string()}});
    assert_eq!(
        refusal(StatusCode::TOO_MANY_REQUESTS, false, &body.to_string()),
        Refusal::RateLimited(
            "Claude.ai has too many active responses. Try again shortly.".to_owned()
        )
    );
}

#[test]
fn an_unclassified_429_keeps_the_reason_and_does_not_claim_spent_usage() {
    let body = r#"{"error":{"type":"rate_limit_error","message":"Please slow down"}}"#;
    assert_eq!(
        refusal(StatusCode::TOO_MANY_REQUESTS, false, body),
        Refusal::RateLimited("Claude.ai temporarily limited this request (HTTP 429). Provider message: Please slow down".to_owned())
    );
}

#[test]
fn an_unknown_answer_is_quoted_short() {
    let long = "x".repeat(2000);
    let Refusal::Other(message) = refusal(StatusCode::BAD_REQUEST, false, &long) else {
        panic!("an other refusal");
    };
    assert!(message.chars().count() < 400);
}
