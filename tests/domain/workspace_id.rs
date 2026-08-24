use artisan_domain::{WorkspaceId, WorkspaceIdError};

#[test]
fn accepts_a_non_empty_workspace_id() {
    let id = WorkspaceId::parse("workspace-42").expect("the fixture is valid");
    assert_eq!(id.as_str(), "workspace-42");
}

#[test]
fn rejects_an_empty_workspace_id() {
    assert_eq!(WorkspaceId::parse("  "), Err(WorkspaceIdError::Empty));
}
