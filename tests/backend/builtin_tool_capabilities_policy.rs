#[allow(dead_code)]
#[path = "../../modules/backend/src/builtin_tool_capabilities_policy.rs"]
mod builtin_tool_capabilities_policy;

use builtin_tool_capabilities_policy::{
    BuiltInToolCapabilityPolicy, CapabilityState, FILE_STORE_TOOL_IDS, FILESYSTEM_TOOL_IDS,
    GIT_TOOL_IDS, LANGUAGE_SERVICE_UNAVAILABLE_REASON, PREVIEW_UNAVAILABLE_REASON, RegistryKind,
    RegistryOutcome, RegistryOutcomes, ToolCapabilityResolution, ToolId,
    WORKSPACE_CAPABILITY_UNAVAILABLE_REASON, WORKSPACE_REQUIRED_REASON, required_registry, resolve,
    selected_registry,
};

const WORKSPACE_ID: Option<&str> = Some("workspace-1");

fn id(value: &str) -> ToolId {
    ToolId::from(value)
}

fn assert_available(result: ToolCapabilityResolution, expected_id: &str) {
    assert_eq!(result.state, CapabilityState::Available);
    assert_eq!(result.tool_id.as_str(), expected_id);
    assert_eq!(result.unavailable_reason, None);
    assert!(result.is_available());
}

fn assert_unavailable(
    result: ToolCapabilityResolution,
    expected_id: &str,
    expected_reason: &'static str,
) {
    assert_eq!(result.state, CapabilityState::Unavailable);
    assert_eq!(result.tool_id.as_str(), expected_id);
    assert_eq!(result.unavailable_reason, Some(expected_reason));
    assert_eq!(result.unavailable_reason(), Some(expected_reason));
    assert!(!result.is_available());
}

fn outcomes_for(registry: RegistryKind, selected: RegistryOutcome) -> RegistryOutcomes {
    match registry {
        RegistryKind::FileStore => {
            RegistryOutcomes::new(selected, RegistryOutcome::Failed, RegistryOutcome::Missing)
        }
        RegistryKind::Filesystem => {
            RegistryOutcomes::new(RegistryOutcome::Failed, selected, RegistryOutcome::Missing)
        }
        RegistryKind::Git => {
            RegistryOutcomes::new(RegistryOutcome::Failed, RegistryOutcome::Missing, selected)
        }
    }
}

#[test]
fn every_workspace_tool_selects_the_exact_registry() {
    let expected = [
        ("workspace.file.read", RegistryKind::FileStore),
        ("workspace.file.write", RegistryKind::FileStore),
        ("workspace.file.list", RegistryKind::Filesystem),
        ("terminal.open", RegistryKind::Filesystem),
        ("git.status.read", RegistryKind::Git),
        ("git.diff.read", RegistryKind::Git),
        ("git.index.stage", RegistryKind::Git),
        ("git.index.unstage", RegistryKind::Git),
    ];

    for (tool_id, registry) in expected {
        assert_eq!(selected_registry(&id(tool_id)), Some(registry), "{tool_id}");
        assert_eq!(required_registry(&id(tool_id)), Some(registry), "{tool_id}");
    }
}

#[test]
fn exported_tool_sets_are_exact_and_complete() {
    assert_eq!(
        FILE_STORE_TOOL_IDS,
        &["workspace.file.read", "workspace.file.write"]
    );
    assert_eq!(
        FILESYSTEM_TOOL_IDS,
        &["workspace.file.list", "terminal.open"]
    );
    assert_eq!(
        GIT_TOOL_IDS,
        &[
            "git.status.read",
            "git.diff.read",
            "git.index.stage",
            "git.index.unstage",
        ]
    );
}

#[test]
fn each_registry_present_missing_and_failed_outcome_is_resolved() {
    let cases = [
        ("workspace.file.read", RegistryKind::FileStore, "file_store"),
        (
            "workspace.file.write",
            RegistryKind::FileStore,
            "file_store",
        ),
        (
            "workspace.file.list",
            RegistryKind::Filesystem,
            "filesystem",
        ),
        ("terminal.open", RegistryKind::Filesystem, "filesystem"),
        ("git.status.read", RegistryKind::Git, "git"),
        ("git.diff.read", RegistryKind::Git, "git"),
        ("git.index.stage", RegistryKind::Git, "git"),
        ("git.index.unstage", RegistryKind::Git, "git"),
    ];

    for (tool_id, registry, label) in cases {
        for outcome in [
            RegistryOutcome::Present,
            RegistryOutcome::Missing,
            RegistryOutcome::Failed,
        ] {
            let result = resolve(id(tool_id), WORKSPACE_ID, outcomes_for(registry, outcome));

            match outcome {
                RegistryOutcome::Present => assert_available(result, tool_id),
                RegistryOutcome::Missing | RegistryOutcome::Failed => {
                    assert_unavailable(result, tool_id, WORKSPACE_CAPABILITY_UNAVAILABLE_REASON)
                }
            }
            assert_eq!(selected_registry(&id(tool_id)), Some(registry), "{label}");
        }
    }
}

#[test]
fn missing_workspace_precedes_every_registry_outcome() {
    let workspace_tools = [
        "workspace.file.read",
        "workspace.file.write",
        "workspace.file.list",
        "terminal.open",
        "git.status.read",
        "git.diff.read",
        "git.index.stage",
        "git.index.unstage",
    ];
    let outcomes = [
        RegistryOutcome::Present,
        RegistryOutcome::Missing,
        RegistryOutcome::Failed,
    ];

    for tool_id in workspace_tools {
        for outcome in outcomes {
            let result = resolve(id(tool_id), None, RegistryOutcomes::all(outcome));
            assert_unavailable(result, tool_id, WORKSPACE_REQUIRED_REASON);
        }
    }

    // A defined but empty JavaScript string is still a present workspace ID.
    assert_available(
        resolve(
            id("workspace.file.read"),
            Some(""),
            RegistryOutcomes::new(
                RegistryOutcome::Present,
                RegistryOutcome::Missing,
                RegistryOutcome::Failed,
            ),
        ),
        "workspace.file.read",
    );
}

#[test]
fn non_workspace_tools_are_unconditionally_available() {
    let tool_ids = [
        "question.ask",
        "assumption.record",
        "terminal.read",
        "terminal.write",
        "terminal.restart",
        "terminal.stop",
        "approval.request",
        "engine.native_action.record",
        "ordinary.future.tool",
    ];
    let outcomes = [
        RegistryOutcomes::all(RegistryOutcome::Present),
        RegistryOutcomes::all(RegistryOutcome::Missing),
        RegistryOutcomes::all(RegistryOutcome::Failed),
    ];

    for tool_id in tool_ids {
        for registries in outcomes {
            assert_available(resolve(id(tool_id), None, registries), tool_id);
        }
    }
}

#[test]
fn preview_prefix_is_unconditionally_unavailable_and_precedes_workspace_rules() {
    for tool_id in [
        "preview.open",
        "preview.inspect",
        "preview.stop",
        "preview.future",
    ] {
        for workspace_id in [None, Some("workspace-1")] {
            let result = resolve(
                id(tool_id),
                workspace_id,
                RegistryOutcomes::all(RegistryOutcome::Present),
            );
            assert_unavailable(result, tool_id, PREVIEW_UNAVAILABLE_REASON);
        }
    }

    // `startsWith("preview.")` is intentionally narrower than `startsWith("preview")`.
    assert_available(
        resolve(
            id("preview"),
            None,
            RegistryOutcomes::all(RegistryOutcome::Failed),
        ),
        "preview",
    );
}

#[test]
fn language_service_is_unconditionally_unavailable_and_has_exact_reason() {
    for workspace_id in [None, Some("workspace-1")] {
        let result = resolve(
            id("workspace.language.status"),
            workspace_id,
            RegistryOutcomes::all(RegistryOutcome::Present),
        );
        assert_unavailable(
            result,
            "workspace.language.status",
            LANGUAGE_SERVICE_UNAVAILABLE_REASON,
        );
    }
}

#[test]
fn selected_registry_outcome_is_the_only_registry_that_matters() {
    let policy = BuiltInToolCapabilityPolicy::new(RegistryOutcomes::new(
        RegistryOutcome::Present,
        RegistryOutcome::Failed,
        RegistryOutcome::Missing,
    ));
    assert_available(
        policy.get(id("workspace.file.read"), WORKSPACE_ID),
        "workspace.file.read",
    );

    let policy = BuiltInToolCapabilityPolicy::new(RegistryOutcomes::new(
        RegistryOutcome::Failed,
        RegistryOutcome::Present,
        RegistryOutcome::Missing,
    ));
    assert_available(
        policy.resolve(id("workspace.file.list"), WORKSPACE_ID),
        "workspace.file.list",
    );

    let policy = BuiltInToolCapabilityPolicy::new(RegistryOutcomes::new(
        RegistryOutcome::Failed,
        RegistryOutcome::Missing,
        RegistryOutcome::Present,
    ));
    assert_available(
        policy.resolve(id("git.diff.read"), WORKSPACE_ID),
        "git.diff.read",
    );
}

#[test]
fn exact_tool_id_is_carried_through_without_registry_custody() {
    let raw_id = String::from("ordinary.tool.with/unusual-custody/🦀");
    let expected_id = raw_id.clone();
    let result = resolve(
        ToolId::new(raw_id),
        None,
        RegistryOutcomes::all(RegistryOutcome::Failed),
    );

    assert_available(result.clone(), &expected_id);
    assert_eq!(result.tool_id.into_string(), expected_id);
}
