use super::*;

#[test]
fn only_the_forks_compaction_is_accepted_and_usage_includes_cache() {
    let data = json!({ "type": "session.compaction.ended", "data": {
        "sessionID": "fork", "text": "## Objective\n- Continue",
        "tokens": { "input": 10, "output": 20, "reasoning": 5, "cache": { "read": 100, "write": 30 } },
    }}).to_string();
    assert!(compact_event(&data, "source").unwrap().is_none());
    let result = compact_event(&data, "fork").unwrap().unwrap();
    assert_eq!(
        result.usage,
        SideUsage {
            input: 140,
            cached_input: 100,
            output: 25
        }
    );
    assert!(crate::is_compaction_summary(&result.text));
    assert!(compact_event(&json!({ "type": "session.compaction.failed", "data": { "sessionID": "fork", "error": "overflow" }}).to_string(), "fork").is_err());
}
