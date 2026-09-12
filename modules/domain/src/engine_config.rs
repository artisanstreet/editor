//! Validated, durable configuration for the native engine run path.
//!
//! The configuration types deliberately keep their representations private.
//! Callers must construct a complete value through the checked constructors,
//! which gives the database and protocol layers one place to enforce the
//! runtime bounds without carrying secrets or host paths in diagnostics.

mod identity;
mod providers;
mod runtime;

pub use identity::{
    ByteLimit, CountLimit, EngineConfigError, EngineConfigReason, EngineId, EngineSelection,
    FiniteMillis, OpenCode2Selection,
};
pub use providers::{
    ClaudeEffort, ClaudePermissionMode, ClaudeSelection, CodexModelContextWindow,
    CodexReasoningEffort, CodexSelection, CodexServiceTier, CursorPermissionMode,
    CursorReasoningEffort, CursorSelection, CursorSpeed, GrokPermissionMode, GrokReasoningEffort,
    GrokSelection,
};
pub use runtime::{
    ApprovalMode, EngineConfigRevision, EngineConfigUpdatePrecondition, EnginePermissionPolicy,
    EngineRunConfig, EngineRuntimeControls, EngineRuntimeControlsInput, FilesystemAccess,
    NetworkAccess, WebSearchAccess,
};

#[cfg(test)]
use crate::identifiers::{
    EngineAgentId, EngineModelId, EngineProfileId, EngineRouteId, PermissionId,
};
#[cfg(test)]
mod tests {
    use super::*;

    fn runtime(attempt: u64) -> Result<EngineRuntimeControls, EngineConfigError> {
        let one = FiniteMillis::new(1)?;
        EngineRuntimeControls::new(EngineRuntimeControlsInput {
            attempt_budget: FiniteMillis::new(attempt)?,
            readiness_budget: one,
            health_budget: one,
            prompt_budget: one,
            stream_budget: one,
            close_budget: one,
            max_json_body_bytes: ByteLimit::new(1)?,
            max_sse_line_bytes: ByteLimit::new(1)?,
            max_sse_event_bytes: ByteLimit::new(1)?,
            max_readiness_line_bytes: ByteLimit::new(1)?,
            max_header_count: CountLimit::new(1)?,
            max_http_buffer_bytes: ByteLimit::new(1)?,
            max_stderr_bytes: ByteLimit::new(1)?,
            observation_capacity: CountLimit::new(1)?,
        })
    }

    fn complete_config(profile: &str) -> EngineRunConfig {
        let permission = EnginePermissionPolicy::new(
            PermissionId::parse("permission-test").expect("permission id is valid"),
            EngineAgentId::parse("agent-test").expect("agent id is valid"),
            ApprovalMode::Never,
            FilesystemAccess::None,
            NetworkAccess::Disabled,
            WebSearchAccess::Disabled,
        );
        EngineRunConfig::new(
            EngineSelection::OpenCode2(OpenCode2Selection::new(
                EngineProfileId::parse(profile).expect("profile id is valid"),
                EngineModelId::parse("model-test").expect("model id is valid"),
                EngineRouteId::parse("route-test").expect("route id is valid"),
                None,
                permission,
            )),
            runtime(5).expect("runtime is valid"),
        )
    }

    #[test]
    fn bounds_are_checked_at_the_private_value_boundaries() {
        assert!(FiniteMillis::new(0).is_err());
        assert!(FiniteMillis::new(86_400_001).is_err());
        assert!(ByteLimit::new(0).is_err());
        assert!(ByteLimit::new(24 * 1024 * 1024 + 1).is_err());
        assert!(CountLimit::new(0).is_err());
        assert!(CountLimit::new(4_097).is_err());
        assert!(EngineConfigRevision::new(0).is_err());
        assert!(EngineConfigRevision::new(i64::MAX as u64 + 1).is_err());
        assert!(EngineConfigRevision::new(1).is_ok());
    }

    #[test]
    fn revision_sqlite_conversion_preserves_boundary_and_checked_overflow() {
        let maximum =
            EngineConfigRevision::new(i64::MAX as u64).expect("SQLite maximum is a valid revision");
        assert_eq!(maximum.as_i64(), i64::MAX);
        assert!(maximum.checked_next().is_err());
    }

    #[test]
    fn phase_and_containing_buffer_relationships_are_checked() {
        let one = FiniteMillis::new(1).expect("one millisecond is valid");
        let bytes = |value| ByteLimit::new(value).expect("byte limit is valid");
        let count = |value| CountLimit::new(value).expect("count limit is valid");
        assert!(
            EngineRuntimeControls::new(EngineRuntimeControlsInput {
                attempt_budget: FiniteMillis::new(4).expect("attempt budget is valid"),
                readiness_budget: one,
                health_budget: one,
                prompt_budget: one,
                stream_budget: one,
                close_budget: one,
                max_json_body_bytes: bytes(1),
                max_sse_line_bytes: bytes(2),
                max_sse_event_bytes: bytes(1),
                max_readiness_line_bytes: bytes(1),
                max_header_count: count(1),
                max_http_buffer_bytes: bytes(1),
                max_stderr_bytes: bytes(1),
                observation_capacity: count(1),
            })
            .is_err()
        );
        assert!(
            EngineRuntimeControls::new(EngineRuntimeControlsInput {
                attempt_budget: FiniteMillis::new(5).expect("attempt budget is valid"),
                readiness_budget: one,
                health_budget: one,
                prompt_budget: one,
                stream_budget: one,
                close_budget: one,
                max_json_body_bytes: bytes(1),
                max_sse_line_bytes: bytes(2),
                max_sse_event_bytes: bytes(1),
                max_readiness_line_bytes: bytes(1),
                max_header_count: count(1),
                max_http_buffer_bytes: bytes(1),
                max_stderr_bytes: bytes(1),
                observation_capacity: count(1),
            })
            .is_err()
        );
    }

    #[test]
    fn missing_variant_is_explicit_and_default_is_an_ordinary_profile_id() {
        let config = complete_config("default");
        let selection = config
            .selection()
            .as_opencode2()
            .expect("fixture is an OpenCode2 selection");
        assert_eq!(selection.profile_id().as_str(), "default");
        assert!(selection.variant_id().is_none());
    }

    fn test_permission() -> EnginePermissionPolicy {
        EnginePermissionPolicy::new(
            PermissionId::parse("permission-test").expect("permission id is valid"),
            EngineAgentId::parse("agent-test").expect("agent id is valid"),
            ApprovalMode::Never,
            FilesystemAccess::None,
            NetworkAccess::Disabled,
            WebSearchAccess::Disabled,
        )
    }

    fn writable_permission() -> EnginePermissionPolicy {
        EnginePermissionPolicy::new(
            PermissionId::parse("permission-test").expect("permission id is valid"),
            EngineAgentId::parse("agent-test").expect("agent id is valid"),
            ApprovalMode::OnRequest,
            FilesystemAccess::Workspace,
            NetworkAccess::Enabled,
            WebSearchAccess::Disabled,
        )
    }

    fn test_profile() -> EngineProfileId {
        EngineProfileId::parse("profile-test").expect("profile id is valid")
    }

    fn test_model() -> EngineModelId {
        EngineModelId::parse("model-test").expect("model id is valid")
    }

    #[test]
    fn engine_id_spellings_parse_and_reject_unknown() {
        for engine in EngineId::ALL {
            assert_eq!(EngineId::parse(engine.as_str()), Ok(engine));
        }
        assert_eq!(
            EngineId::parse("acp"),
            Err(EngineConfigError::new(
                "engine",
                EngineConfigReason::Unsupported
            ))
        );
        assert_eq!(
            EngineId::parse("opencode"),
            Err(EngineConfigError::new(
                "engine",
                EngineConfigReason::Unsupported
            ))
        );
        assert_eq!(
            EngineId::parse(""),
            Err(EngineConfigError::new(
                "engine",
                EngineConfigReason::Unsupported
            ))
        );
    }

    #[test]
    fn only_opencode2_has_an_execution_runtime() {
        for engine in EngineId::ALL {
            assert_eq!(
                engine.has_execution_runtime(),
                engine == EngineId::OpenCode2,
                "unexpected runtime flag for {}",
                engine.as_str()
            );
        }
    }

    #[test]
    fn opencode2_accessor_rejects_other_engines_without_coercion() {
        let codex = EngineSelection::Codex(
            CodexSelection::new(
                test_profile(),
                Some(test_model()),
                test_permission(),
                None,
                None,
                None,
            )
            .expect("codex selection is valid"),
        );
        assert_eq!(codex.engine_id(), EngineId::Codex);
        assert_eq!(
            codex.as_opencode2(),
            Err(EngineConfigError::new(
                "engine",
                EngineConfigReason::Unsupported
            ))
        );
        assert!(!codex.is_execution_supported());
        assert_eq!(codex.profile_id().as_str(), "profile-test");
        assert_eq!(
            codex.model_id().map(EngineModelId::as_str),
            Some("model-test")
        );
        assert!(codex.as_codex().is_ok());
        assert!(codex.as_claude().is_err());
    }

    #[test]
    fn codex_permission_relationships_reject_adapter_violations() {
        let hostile = EnginePermissionPolicy::new(
            PermissionId::parse("permission-test").expect("permission id is valid"),
            EngineAgentId::parse("agent-test").expect("agent id is valid"),
            ApprovalMode::Always,
            FilesystemAccess::Workspace,
            NetworkAccess::Enabled,
            WebSearchAccess::Disabled,
        );
        assert!(
            CodexSelection::new(test_profile(), None, hostile, None, None, None).is_err(),
            "codex rejects always approval"
        );
        let offline_host = EnginePermissionPolicy::new(
            PermissionId::parse("permission-test").expect("permission id is valid"),
            EngineAgentId::parse("agent-test").expect("agent id is valid"),
            ApprovalMode::Never,
            FilesystemAccess::Host,
            NetworkAccess::Disabled,
            WebSearchAccess::Disabled,
        );
        assert!(
            CodexSelection::new(test_profile(), None, offline_host, None, None, None).is_err(),
            "codex rejects host scope without network"
        );
        assert!(CodexModelContextWindow::new(0).is_err());
        assert!(CodexModelContextWindow::new(1).is_ok());
        assert!(CodexReasoningEffort::parse("ultra").is_ok());
        assert!(CodexReasoningEffort::parse("turbo").is_err());
        assert!(CodexServiceTier::parse("fast").is_ok());
        assert!(CodexServiceTier::parse("priority").is_err());
    }

    #[test]
    fn claude_selection_requires_write_and_network_access() {
        assert!(
            ClaudeSelection::new(
                test_profile(),
                None,
                test_permission(),
                None,
                None,
                false,
                false,
            )
            .is_err(),
            "claude rejects read-only policy"
        );
        assert!(
            ClaudeSelection::new(
                test_profile(),
                Some(test_model()),
                writable_permission(),
                Some(ClaudeEffort::High),
                Some(ClaudePermissionMode::Plan),
                true,
                false,
            )
            .is_ok()
        );
        assert!(ClaudeEffort::parse("max").is_ok());
        assert!(ClaudeEffort::parse("ultra").is_err());
        assert!(ClaudePermissionMode::parse("acceptEdits").is_ok());
        assert!(ClaudePermissionMode::parse("yes").is_err());
    }

    #[test]
    fn open_ended_effort_labels_reject_empty_and_unbounded_values() {
        assert!(GrokReasoningEffort::parse("medium").is_ok());
        assert!(GrokReasoningEffort::parse("").is_err());
        assert!(GrokReasoningEffort::parse("has space").is_err());
        assert!(CursorReasoningEffort::parse("high").is_ok());
        assert!(CursorReasoningEffort::parse("").is_err());
        assert_eq!(
            GrokPermissionMode::parse("always-approve"),
            Ok(GrokPermissionMode::AlwaysApprove)
        );
        assert!(GrokPermissionMode::parse("yolo").is_err());
        assert_eq!(
            CursorPermissionMode::parse("force"),
            Ok(CursorPermissionMode::Force)
        );
        assert!(CursorPermissionMode::parse("ask").is_err());
        assert_eq!(CursorSpeed::parse("fast"), Ok(CursorSpeed::Fast));
        assert!(CursorSpeed::parse("slow").is_err());
    }

    #[test]
    fn storage_codec_version_is_legacy_only_for_opencode2() {
        let legacy = complete_config("default");
        assert_eq!(legacy.engine_id(), EngineId::OpenCode2);
        assert_eq!(legacy.storage_codec_version(), 1);
        let codex = EngineRunConfig::new(
            EngineSelection::Codex(
                CodexSelection::new(test_profile(), None, test_permission(), None, None, None)
                    .expect("codex selection is valid"),
            ),
            runtime(5).expect("runtime is valid"),
        );
        assert_eq!(codex.engine_id(), EngineId::Codex);
        assert_eq!(codex.storage_codec_version(), 2);
    }
}
