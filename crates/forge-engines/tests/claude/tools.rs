use super::tools::tool_kind;
use artisan_api::ToolKind;
use serde_json::json;

fn workflow_name(input: &serde_json::Value) -> Option<String> {
    let kind = tool_kind("Workflow", input).expect("a workflow call is valid");
    let ToolKind::Workflow { name, .. } = kind else {
        panic!("a workflow call decoded as {kind:?}");
    };
    name
}

#[test]
fn a_workflow_is_named_from_its_saved_name_or_its_script_meta() {
    assert_eq!(
        workflow_name(&json!({"name": "nightly-audit"})).as_deref(),
        Some("nightly-audit")
    );
    assert_eq!(
        workflow_name(&json!({
            "script": "export const meta = {\n  name: 'review-changes',\n  description: 'x',\n}"
        }))
        .as_deref(),
        Some("review-changes")
    );
    assert_eq!(
        workflow_name(&json!({"script": "export const meta = { description: \"x\" }"})),
        None
    );
}
