//! Coverage retained for the pre-seed [`WorkspaceId`](artisan_domain::WorkspaceId)
//! placeholder.
//!
//! The placeholder itself survives in
//! `modules/domain/src/legacy_workspace_id.rs` because `modules/protocol` and
//! `modules/database` still re-export it; these tests keep its behavior
//! pinned until the controller retires both sides together.

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

#[test]
fn round_trips_display_and_from_str() {
    let parsed: WorkspaceId = "workspace-42".parse().expect("the fixture is valid");

    assert_eq!(parsed.to_string(), "workspace-42");
    assert_eq!(
        parsed,
        WorkspaceId::parse("workspace-42").expect("the fixture is valid")
    );
}
