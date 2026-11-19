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
        let Err(_) = tool_kind(name, &serde_json::json!({})) else {
            panic!("{name} without its required input is refused");
        };
    }
    tool_kind(
        "Edit",
        &serde_json::json!({"file_path":"file","old_string":"","new_string":""}),
    )
    .expect("an edit with empty strings still names its file");
}
