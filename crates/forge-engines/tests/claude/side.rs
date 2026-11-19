use super::*;
use artisan_api::Permissions;

#[test]
fn a_fork_launches_as_its_thread_does() {
    for permissions in [Permissions::Auto, Permissions::Full] {
        let mut config = EngineConfig::new(artisan_api::EngineId::Claude);
        config.permissions = permissions;
        config.model = Some("claude-opus-5-5".to_owned());
        let readable = [PathBuf::from("/data/blobs")];
        let thread = launch_args(&config, Some("s1"), &readable);
        let fork = side_args(&config, Some("s1"), &readable);
        assert_eq!(fork[..thread.len()], thread[..]);
        assert_eq!(
            fork[thread.len()..],
            ["--fork-session", "--max-turns", "1"].map(str::to_owned)
        );
    }
}

#[test]
fn only_a_context_terminal_reason_reports_a_context_limit() {
    let result = |terminal_reason: &str, text: &str| {
        json!({
            "type": "result", "subtype": "success", "is_error": true, "api_error_status": 400,
            "result": text, "terminal_reason": terminal_reason, "session_id": "s1",
            "usage": { "input_tokens": 0, "output_tokens": 0, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0 },
        })
    };
    for (terminal_reason, text) in [
        ("prompt_too_long", "Prompt is too long"),
        ("blocking_limit", "Prompt is too long"),
    ] {
        let failure = reply_of(&result(terminal_reason, text)).unwrap_err();
        assert!(failure.exceeds_context(), "{failure}");
    }
    for (terminal_reason, text) in [
        (
            "api_error",
            r#"API Error: 400 {"type":"error","error":{"type":"invalid_request_error","message":"context_management: Extra inputs are not permitted"}}"#,
        ),
        (
            "image_error",
            "Image was too large. Double press esc to go back and try again with a smaller image.",
        ),
    ] {
        let failure = reply_of(&result(terminal_reason, text)).unwrap_err();
        assert!(!failure.exceeds_context(), "{failure}");
        assert!(
            matches!(failure, SideQueryError::Failed { .. }),
            "{failure}"
        );
    }
}
