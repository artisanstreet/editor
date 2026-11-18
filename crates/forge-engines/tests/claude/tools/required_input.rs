use super::*;
#[test]
fn known_native_tools_reject_missing_fields_but_allow_empty_replacement_text() {
    for name in [
        "Bash",
        "Read",
        "Edit",
        "Write",
        "Grep",
        "Glob",
        "WebFetch",
        "WebSearch",
        "SendMessage",
    ] {
        assert!(tool_kind(name, &serde_json::json!({})).is_err(), "{name}");
    }
    assert!(
        tool_kind(
            "Edit",
            &serde_json::json!({"file_path":"file","old_string":"","new_string":""})
        )
        .is_ok()
    );
}
