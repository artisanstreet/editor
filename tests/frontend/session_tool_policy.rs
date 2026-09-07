//! Dependency-free parity coverage for the native session tool policy.
//!
//! The production module is included directly so these tests do not require
//! frontend crate registration or any runtime dependency.

#[path = "../../modules/frontend/src/session_tool_policy.rs"]
mod session_tool_policy;

use session_tool_policy::{
    ApprovalMode, ArtisanToolPermissionPolicy, PermissionMode, SandboxMode, SessionPolicyView,
    make_session_tool_policy,
};

const FAIL_CLOSED: ArtisanToolPermissionPolicy = ArtisanToolPermissionPolicy {
    approval: ApprovalMode::Never,
    allow_engine_observation: true,
    allow_git_index_write: false,
    allow_preview_control: false,
    allow_process_control: false,
    allow_workspace_read: true,
    allow_workspace_write: false,
};

fn assert_all_fields(actual: &ArtisanToolPermissionPolicy, expected: &ArtisanToolPermissionPolicy) {
    assert_eq!(&actual.approval, &expected.approval);
    assert_eq!(
        actual.allow_engine_observation,
        expected.allow_engine_observation
    );
    assert_eq!(actual.allow_git_index_write, expected.allow_git_index_write);
    assert_eq!(actual.allow_preview_control, expected.allow_preview_control);
    assert_eq!(actual.allow_process_control, expected.allow_process_control);
    assert_eq!(actual.allow_workspace_read, expected.allow_workspace_read);
    assert_eq!(actual.allow_workspace_write, expected.allow_workspace_write);
    assert_eq!(actual, expected);
}

fn expected(approval: ApprovalMode, allow_mutation: bool) -> ArtisanToolPermissionPolicy {
    ArtisanToolPermissionPolicy {
        approval,
        allow_engine_observation: true,
        allow_git_index_write: allow_mutation,
        allow_preview_control: allow_mutation,
        allow_process_control: allow_mutation,
        allow_workspace_read: true,
        allow_workspace_write: allow_mutation,
    }
}

#[test]
fn missing_policy_and_missing_fields_fail_closed_for_mutations() {
    let cases = [
        (None, FAIL_CLOSED),
        (Some(SessionPolicyView::from_raw(None, None)), FAIL_CLOSED),
        (
            Some(SessionPolicyView::from_raw(Some("on_request"), None)),
            ArtisanToolPermissionPolicy {
                approval: ApprovalMode::OnRequest,
                allow_engine_observation: true,
                allow_git_index_write: false,
                allow_preview_control: false,
                allow_process_control: false,
                allow_workspace_read: true,
                allow_workspace_write: false,
            },
        ),
    ];

    for (policy, expected) in cases {
        let actual = make_session_tool_policy(policy);
        assert_all_fields(&actual, &expected);
    }
}

#[test]
fn exhaustive_permission_and_sandbox_table_maps_every_output_field() {
    let cases = [
        (
            PermissionMode::Never,
            SandboxMode::ReadOnly,
            ApprovalMode::Never,
            false,
        ),
        (
            PermissionMode::Never,
            SandboxMode::WorkspaceWrite,
            ApprovalMode::Never,
            true,
        ),
        (
            PermissionMode::OnRequest,
            SandboxMode::ReadOnly,
            ApprovalMode::OnRequest,
            false,
        ),
        (
            PermissionMode::OnRequest,
            SandboxMode::WorkspaceWrite,
            ApprovalMode::OnRequest,
            true,
        ),
    ];

    assert_eq!(
        PermissionMode::ALL,
        [PermissionMode::Never, PermissionMode::OnRequest]
    );
    assert_eq!(
        SandboxMode::ALL,
        [SandboxMode::ReadOnly, SandboxMode::WorkspaceWrite]
    );

    for (permission_mode, sandbox_mode, approval, allow_mutation) in cases {
        let policy = SessionPolicyView::new(permission_mode, sandbox_mode);
        let actual = make_session_tool_policy(Some(policy));
        let expected = expected(approval, allow_mutation);
        assert_all_fields(&actual, &expected);
    }
}

#[test]
fn only_exact_workspace_write_enables_mutating_capabilities() {
    let read_only = make_session_tool_policy(Some(SessionPolicyView::new(
        PermissionMode::OnRequest,
        SandboxMode::ReadOnly,
    )));
    assert!(!read_only.allow_git_index_write);
    assert!(!read_only.allow_preview_control);
    assert!(!read_only.allow_process_control);
    assert!(!read_only.allow_workspace_write);

    let workspace_write = make_session_tool_policy(Some(SessionPolicyView::new(
        PermissionMode::Never,
        SandboxMode::WorkspaceWrite,
    )));
    assert!(workspace_write.allow_git_index_write);
    assert!(workspace_write.allow_preview_control);
    assert!(workspace_write.allow_process_control);
    assert!(workspace_write.allow_workspace_write);
}

#[test]
fn unknown_future_and_malformed_sandbox_values_fail_closed() {
    let raw_values = [
        "unknown",
        "future_workspace_write",
        "workspace-write",
        "WORKSPACE_WRITE",
        " workspace_write",
        "workspace_write ",
        "",
    ];

    for raw_sandbox_mode in raw_values {
        let actual = make_session_tool_policy(Some(SessionPolicyView::from_raw(
            Some("on_request"),
            Some(raw_sandbox_mode),
        )));
        let expected = expected(ApprovalMode::OnRequest, false);
        assert_all_fields(&actual, &expected);
        assert_eq!(actual.approval.as_str(), "on_request");
    }
}

#[test]
fn permission_mode_is_passed_through_exactly_and_defaults_to_never() {
    for raw_permission_mode in ["never", "on_request", "future_permission", ""] {
        let actual = make_session_tool_policy(Some(SessionPolicyView::from_raw(
            Some(raw_permission_mode),
            Some("read_only"),
        )));
        assert_eq!(actual.approval.as_str(), raw_permission_mode);
        assert!(!actual.allow_git_index_write);
        assert!(!actual.allow_preview_control);
        assert!(!actual.allow_process_control);
        assert!(!actual.allow_workspace_write);
    }

    assert_eq!(make_session_tool_policy(None).approval.as_str(), "never");
    assert_eq!(
        make_session_tool_policy(Some(SessionPolicyView::from_raw(None, Some("read_only"))))
            .approval
            .as_str(),
        "never"
    );
}

#[test]
fn observation_and_reads_are_unconditionally_allowed() {
    let policies = [
        None,
        Some(SessionPolicyView::from_raw(None, None)),
        Some(SessionPolicyView::from_raw(
            Some("never"),
            Some("read_only"),
        )),
        Some(SessionPolicyView::from_raw(
            Some("on_request"),
            Some("workspace_write"),
        )),
        Some(SessionPolicyView::from_raw(
            Some("future_permission"),
            Some("future_sandbox"),
        )),
    ];

    for policy in policies {
        let output = make_session_tool_policy(policy);
        assert!(output.allow_engine_observation);
        assert!(output.allow_workspace_read);
    }
}

#[test]
fn raw_vocabulary_is_exact_and_unknown_values_are_retained() {
    assert_eq!(PermissionMode::from_raw("never"), PermissionMode::Never);
    assert_eq!(
        PermissionMode::from_raw("on_request"),
        PermissionMode::OnRequest
    );
    assert_eq!(PermissionMode::from_raw(" future ").as_str(), " future ");
    assert_eq!(SandboxMode::from_raw("read_only"), SandboxMode::ReadOnly);
    assert_eq!(
        SandboxMode::from_raw("workspace_write"),
        SandboxMode::WorkspaceWrite
    );
    assert_eq!(SandboxMode::from_raw("future").as_str(), "future");
    assert_eq!(ApprovalMode::from_raw("never"), ApprovalMode::Never);
    assert_eq!(
        ApprovalMode::from_raw("on_request"),
        ApprovalMode::OnRequest
    );
    assert_eq!(ApprovalMode::from_raw("always"), ApprovalMode::Always);
    assert_eq!(ApprovalMode::from_raw("future").as_str(), "future");
    assert_eq!(
        ApprovalMode::ALL,
        [
            ApprovalMode::Never,
            ApprovalMode::OnRequest,
            ApprovalMode::Always
        ]
    );
}
