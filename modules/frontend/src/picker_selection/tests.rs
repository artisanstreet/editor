use artisan_domain::{
    ApprovalMode, ByteLimit, ClaudeEffort, ClaudePermissionMode, ClaudeSelection,
    CodexReasoningEffort, CodexSelection, CodexServiceTier, CountLimit, EngineAgentId,
    EngineModelId, EnginePermissionPolicy, EngineRouteId, EngineRuntimeControls,
    EngineRuntimeControlsInput, FilesystemAccess, FiniteMillis, NetworkAccess, OpenCode2Selection,
    PermissionId, WebSearchAccess,
};

use super::*;

fn fixture() -> NativeModelCatalog {
    NativeModelCatalog::from_manifest_json(include_str!(
        "../../../../tests/fixtures/model_catalog.json"
    ))
    .unwrap()
}

fn runtime() -> EngineRuntimeControls {
    let millis = |value| FiniteMillis::new(value).unwrap();
    let bytes = |value| ByteLimit::new(value).unwrap();
    EngineRuntimeControls::new(EngineRuntimeControlsInput {
        attempt_budget: millis(3_660_000),
        readiness_budget: millis(15_000),
        health_budget: millis(5_000),
        prompt_budget: millis(30_000),
        stream_budget: millis(3_600_000),
        close_budget: millis(10_000),
        max_json_body_bytes: bytes(24 * 1024 * 1024),
        max_sse_line_bytes: bytes(64 * 1024),
        max_sse_event_bytes: bytes(1024 * 1024),
        max_readiness_line_bytes: bytes(8192),
        max_header_count: CountLimit::new(64).unwrap(),
        max_http_buffer_bytes: bytes(64 * 1024),
        max_stderr_bytes: bytes(64 * 1024),
        observation_capacity: CountLimit::new(256).unwrap(),
    })
    .unwrap()
}

fn permission(
    id: &str,
    filesystem: FilesystemAccess,
    network: NetworkAccess,
) -> EnginePermissionPolicy {
    EnginePermissionPolicy::new(
        PermissionId::parse(id).unwrap(),
        EngineAgentId::parse(format!("artisan-v1-{id}")).unwrap(),
        ApprovalMode::OnRequest,
        filesystem,
        network,
        WebSearchAccess::Disabled,
    )
}

fn profile() -> EngineProfileId {
    EngineProfileId::parse("default").unwrap()
}

/// A Codex configuration as the Forge saves `codex-sol` with its defaults.
fn codex_sol() -> EngineRunConfig {
    EngineRunConfig::new(
        EngineSelection::Codex(
            CodexSelection::new(
                profile(),
                Some(EngineModelId::parse("gpt-5.6-sol").unwrap()),
                permission(
                    "autonomous",
                    FilesystemAccess::Workspace,
                    NetworkAccess::Disabled,
                ),
                Some(CodexReasoningEffort::parse("low").unwrap()),
                Some(CodexServiceTier::parse("standard").unwrap()),
                None,
            )
            .unwrap(),
        ),
        runtime(),
    )
}

#[test]
fn a_saved_codex_configuration_displays_as_its_catalog_row() {
    let catalog = fixture();
    let mut shown = catalog.selection_policy_for_model("codex-sol").unwrap();
    shown.profile_id = Some("default".to_owned());
    let projected = saved_config_policy(&catalog, &codex_sol()).expect("displayable");
    assert_eq!(projected.model_id, "codex-sol");
    assert_eq!(projected.reasoning_effort, shown.reasoning_effort);
    assert_eq!(projected.speed, shown.speed);
    assert_eq!(projected.permission, shown.permission);
    assert_eq!(projected.profile_id.as_deref(), Some("default"));
    // An unsaved window is the base window, never an extended default.
    assert_eq!(
        projected
            .context_window
            .as_ref()
            .map(|window| window.native_suffix.as_str()),
        Some("")
    );
}

#[test]
fn a_saved_claude_model_splits_its_context_suffix_back_off() {
    let catalog = fixture();
    let saved = EngineRunConfig::new(
        EngineSelection::Claude(
            ClaudeSelection::new(
                profile(),
                Some(EngineModelId::parse("claude-fable-5[1m]").unwrap()),
                permission("autonomous", FilesystemAccess::Host, NetworkAccess::Enabled),
                Some(ClaudeEffort::parse("high").unwrap()),
                Some(ClaudePermissionMode::Auto),
                false,
                false,
            )
            .unwrap(),
        ),
        runtime(),
    );
    let projected = saved_config_policy(&catalog, &saved).expect("displayable");
    assert_eq!(projected.model_id, "claude-fable");
    assert_eq!(
        projected
            .context_window
            .as_ref()
            .map(|window| window.native_suffix.as_str()),
        Some("[1m]")
    );
}

#[test]
fn a_saved_claude_window_cap_displays_as_its_capped_option() {
    let mut catalog = fixture();
    let model = catalog
        .manifest
        .models
        .iter_mut()
        .find(|model| model.id == "claude-fable")
        .unwrap();
    let context = model.capabilities.context_window.as_mut().unwrap();
    let mut capped = context.options[0].clone();
    capped.id = "capped".to_owned();
    capped.tokens = 400_000;
    capped.native_config = Some(crate::native_model_catalog::NativeContextConfig {
        model_context_window: 400_000,
    });
    context.options.insert(0, capped);
    let saved = |window| {
        EngineRunConfig::new(
            EngineSelection::Claude(
                ClaudeSelection::new(
                    profile(),
                    Some(EngineModelId::parse("claude-fable-5").unwrap()),
                    permission("autonomous", FilesystemAccess::Host, NetworkAccess::Enabled),
                    Some(ClaudeEffort::parse("high").unwrap()),
                    Some(ClaudePermissionMode::Auto),
                    false,
                    false,
                )
                .unwrap()
                .with_auto_compact_window(window),
            ),
            runtime(),
        )
    };
    let shown = |config: &EngineRunConfig| {
        saved_config_policy(&catalog, config)
            .and_then(|policy| policy.context_window)
            .map(|window| window.id)
    };
    let window = artisan_domain::ClaudeAutoCompactWindow::new(400_000).unwrap();
    assert_eq!(shown(&saved(Some(window))).as_deref(), Some("capped"));
    assert_eq!(shown(&saved(None)).as_deref(), Some("standard"));
    // A window no option carries belongs to no row.
    let unknown = artisan_domain::ClaudeAutoCompactWindow::new(123_000).unwrap();
    assert_eq!(shown(&saved(Some(unknown))), None);
}

#[test]
fn a_configuration_the_catalog_cannot_display_has_no_policy() {
    let mut catalog = fixture();
    catalog
        .manifest
        .models
        .retain(|model| model.id != "codex-sol");
    assert_eq!(saved_config_policy(&catalog, &codex_sol()), None);
    // An `OpenCode` configuration displays only in its own profile's scope.
    let opencode = EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            profile(),
            EngineModelId::parse("model-test").unwrap(),
            EngineRouteId::parse("route-test").unwrap(),
            None,
            permission(
                "autonomous",
                FilesystemAccess::Workspace,
                NetworkAccess::Disabled,
            ),
        )),
        runtime(),
    );
    assert_eq!(saved_config_policy(&fixture(), &opencode), None);
}

#[test]
fn a_displayed_policy_is_named_by_its_catalog_identities() {
    let catalog = fixture();
    let mut shown = catalog.selection_policy_for_model("codex-sol").unwrap();
    shown.profile_id = Some("default".to_owned());
    let selection = selection_for_policy(&shown).expect("nameable");
    assert_eq!(selection.model_id.as_str(), "codex-sol");
    assert_eq!(
        selection.profile_id.as_ref().map(EngineProfileId::as_str),
        Some("default")
    );
    assert_eq!(
        selection.permission.as_ref().map(CatalogOptionId::as_str),
        shown.permission.as_ref().map(|value| value.id.as_str())
    );
    assert_eq!(
        selection
            .reasoning_effort
            .as_ref()
            .map(CatalogOptionId::as_str),
        shown
            .reasoning_effort
            .as_ref()
            .map(|value| value.id.as_str())
    );
}
