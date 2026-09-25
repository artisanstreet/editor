//! Thread-bound engine settings state.
//!
//! The controller owns no runtime or GPUI entity. It models the
//! application-visible lifecycle for one selected real thread and
//! retains only redacted diagnostics. The Forge builds and validates every
//! configuration: the manual draft is resolved by the Forge before it is
//! saved.

#![forbid(unsafe_code)]

use artisan_domain::{
    EngineConfigError, EngineConfigReason, EngineConfigRevision, EngineProfileId, EngineRunConfig,
    ThreadId,
};
use artisan_protocol::{
    RegisteredEngineProfilesResult, SetThreadEngineConfigResult, ThreadEngineSettingsResult,
};

use crate::native_transport_service::{
    ServiceFailure, ServiceFailureCategory, ServiceFailureStage, SettingsLoadGeneration,
};

// Phase-1 split submodules (see engine_settings/).

#[path = "engine_settings/draft.rs"]
mod draft;

#[path = "engine_settings/controller.rs"]
mod controller;

pub use controller::*;
pub use draft::*;

#[cfg(test)]
mod tests {
    use super::*;
    use artisan_domain::{
        EngineConfigRevision, EngineProfileId, EngineRunConfig, RequestId, ThreadId,
    };
    use artisan_protocol::{
        RegisteredEngineProfilesResult, SetThreadEngineConfigResult, ThreadEngineSettingsResult,
    };

    fn thread_id(value: &str) -> ThreadId {
        ThreadId::parse(value).expect("thread id")
    }

    fn profile_id(value: &str) -> EngineProfileId {
        EngineProfileId::parse(value).expect("profile id")
    }

    fn request_id(value: &str) -> RequestId {
        RequestId::parse(value).expect("request id")
    }

    fn revision(value: u64) -> EngineConfigRevision {
        EngineConfigRevision::new(value).expect("revision")
    }

    fn sample_draft(profile: &str) -> EngineSettingsDraft {
        EngineSettingsDraft {
            profile_id: profile.to_owned(),
            model_id: "model-test".to_owned(),
            route_id: "route-test".to_owned(),
            variant_id: String::new(),
            permission_id: "permission-test".to_owned(),
            agent_id: "agent-test".to_owned(),
            approval: "never".to_owned(),
            filesystem: "none".to_owned(),
            network: "disabled".to_owned(),
            web_search: "disabled".to_owned(),
            attempt_budget: "5".to_owned(),
            readiness_budget: "1".to_owned(),
            health_budget: "1".to_owned(),
            prompt_budget: "1".to_owned(),
            stream_budget: "1".to_owned(),
            close_budget: "1".to_owned(),
            max_json_body_bytes: "1".to_owned(),
            max_sse_line_bytes: "1".to_owned(),
            max_sse_event_bytes: "1".to_owned(),
            max_readiness_line_bytes: "1".to_owned(),
            max_header_count: "1".to_owned(),
            max_http_buffer_bytes: "1".to_owned(),
            max_stderr_bytes: "1".to_owned(),
            observation_capacity: "1".to_owned(),
        }
    }

    /// The configuration the Forge builds from `sample_draft(profile)` with
    /// `model`.
    fn config_with(profile: &str, model: &str) -> EngineRunConfig {
        use artisan_domain::{
            ApprovalMode, ByteLimit, CountLimit, EngineAgentId, EngineModelId,
            EnginePermissionPolicy, EngineRouteId, EngineRuntimeControls,
            EngineRuntimeControlsInput, EngineSelection, FilesystemAccess, FiniteMillis,
            NetworkAccess, OpenCode2Selection, PermissionId, WebSearchAccess,
        };
        let millis = |value| FiniteMillis::new(value).expect("millis");
        let bytes = |value| ByteLimit::new(value).expect("bytes");
        let count = |value| CountLimit::new(value).expect("count");
        let runtime = EngineRuntimeControls::new(EngineRuntimeControlsInput {
            attempt_budget: millis(5),
            readiness_budget: millis(1),
            health_budget: millis(1),
            prompt_budget: millis(1),
            stream_budget: millis(1),
            close_budget: millis(1),
            max_json_body_bytes: bytes(1),
            max_sse_line_bytes: bytes(1),
            max_sse_event_bytes: bytes(1),
            max_readiness_line_bytes: bytes(1),
            max_header_count: count(1),
            max_http_buffer_bytes: bytes(1),
            max_stderr_bytes: bytes(1),
            observation_capacity: count(1),
        })
        .expect("runtime");
        EngineRunConfig::new(
            EngineSelection::OpenCode2(OpenCode2Selection::new(
                profile_id(profile),
                EngineModelId::parse(model).expect("model"),
                EngineRouteId::parse("route-test").expect("route"),
                None,
                EnginePermissionPolicy::new(
                    PermissionId::parse("permission-test").expect("permission"),
                    EngineAgentId::parse("agent-test").expect("agent"),
                    ApprovalMode::Never,
                    FilesystemAccess::None,
                    NetworkAccess::Disabled,
                    WebSearchAccess::Disabled,
                ),
            )),
            runtime,
        )
    }

    fn sample_config(profile: &str) -> EngineRunConfig {
        config_with(profile, "model-test")
    }

    fn registered_present(ids: &[&str]) -> RegisteredEngineProfilesResult {
        RegisteredEngineProfilesResult::RegistryPresent {
            profile_ids: ids.iter().map(|id| profile_id(id)).collect(),
        }
    }

    fn admit_settings_load(
        controller: &mut EngineSettingsController,
        thread_id: &ThreadId,
    ) -> SettingsLoadGeneration {
        let generation = controller.prepare_settings_load().expect("generation");
        assert!(controller.mark_settings_load_admitted(thread_id, generation));
        generation
    }

    fn load_unconfigured(controller: &mut EngineSettingsController, thread_id: &ThreadId) {
        let generation = admit_settings_load(controller, thread_id);
        controller.on_settings_loaded(
            generation,
            ThreadEngineSettingsResult::Unconfigured {
                thread_id: thread_id.clone(),
            },
        );
    }

    fn load_configured(
        controller: &mut EngineSettingsController,
        thread_id: &ThreadId,
        revision_value: u64,
        config: EngineRunConfig,
    ) {
        let generation = admit_settings_load(controller, thread_id);
        controller.on_settings_loaded(
            generation,
            ThreadEngineSettingsResult::Configured {
                thread_id: thread_id.clone(),
                revision: revision(revision_value),
                config: Box::new(config),
            },
        );
    }

    fn ready_controller() -> (EngineSettingsController, ThreadId, EngineRunConfig) {
        let mut controller = EngineSettingsController::new();
        let thread = thread_id("thread-a");
        controller.select_thread(Some(&thread));
        controller.on_registry_loaded(registered_present(&["default"]));
        let config = sample_config("default");
        load_configured(&mut controller, &thread, 7, config.clone());
        (controller, thread, config)
    }

    /// Edits the draft's model and returns the configuration the Forge
    /// builds from it.
    fn make_dirty_config(controller: &mut EngineSettingsController) -> EngineRunConfig {
        controller.draft_mut().model_id = "model-next".to_owned();
        config_with("default", "model-next")
    }

    fn bridge_busy() -> ServiceFailure {
        ServiceFailure {
            stage: ServiceFailureStage::EventBridge,
            category: ServiceFailureCategory::Backpressure,
        }
    }

    fn bridge_stopped() -> ServiceFailure {
        ServiceFailure {
            stage: ServiceFailureStage::EventBridge,
            category: ServiceFailureCategory::ChannelClosed,
        }
    }

    #[test]
    fn direct_save_uses_unconfigured_precondition_before_first_save() {
        let mut controller = EngineSettingsController::new();
        let tid = thread_id("thread-a");
        controller.select_thread(Some(&tid));
        controller.on_registry_loaded(registered_present(&["default"]));
        load_unconfigured(&mut controller, &tid);
        let config = sample_config("default");
        let command = controller
            .build_direct_save_command(request_id("save-1"), config.clone())
            .expect("direct save builds without a draft");
        assert_eq!(command.thread_id(), &tid);
        assert_eq!(command.config(), &config);
        assert_eq!(
            command.precondition(),
            artisan_domain::EngineConfigUpdatePrecondition::Unconfigured
        );
    }

    #[test]
    fn direct_save_uses_exact_revision_after_configuration() {
        let (controller, thread, config) = ready_controller();
        let next = sample_config("default");
        let command = controller
            .build_direct_save_command(request_id("save-2"), next.clone())
            .expect("direct save builds on a configured thread");
        assert_eq!(command.thread_id(), &thread);
        assert_eq!(command.config(), &next);
        assert_eq!(
            command.precondition(),
            artisan_domain::EngineConfigUpdatePrecondition::Exact(revision(7))
        );
        assert_eq!(
            &config,
            controller.authoritative_config().expect("authoritative")
        );
    }

    #[test]
    fn direct_save_refuses_without_thread_or_during_flight() {
        let controller = EngineSettingsController::new();
        assert!(
            controller
                .build_direct_save_command(request_id("save-x"), sample_config("default"))
                .is_none()
        );
        let (mut controller, thread, config) = ready_controller();
        assert!(controller.begin_direct_save(thread, request_id("save-flight"), config));
        assert!(
            controller
                .build_direct_save_command(request_id("save-y"), sample_config("default"))
                .is_none()
        );
    }

    #[test]
    fn configured_load_becomes_ready_with_authoritative_config() {
        let mut controller = EngineSettingsController::new();
        let tid = thread_id("thread-a");
        controller.select_thread(Some(&tid));
        controller.on_registry_loaded(registered_present(&["default", "work"]));
        let config = sample_config("default");
        load_configured(&mut controller, &tid, 7, config.clone());
        assert_eq!(controller.status(), EngineSettingsStatus::Ready);
        assert_eq!(controller.revision(), Some(revision(7)));
        assert_eq!(controller.authoritative_config(), Some(&config));
        assert!(!controller.is_dirty());
        assert!(!controller.can_save());
        controller.draft_mut().model_id = "model-next".to_owned();
        assert!(controller.can_save());
    }

    #[test]
    fn unconfigured_load_becomes_unconfigured_and_requires_explicit_values() {
        let mut controller = EngineSettingsController::new();
        let tid = thread_id("thread-a");
        controller.select_thread(Some(&tid));
        controller.on_registry_loaded(registered_present(&["default"]));
        load_unconfigured(&mut controller, &tid);
        assert_eq!(controller.status(), EngineSettingsStatus::Unconfigured);
        assert_eq!(controller.revision(), None);
        assert!(!controller.can_save());
        assert_eq!(controller.draft().profile_id, "");
        assert_eq!(controller.draft().model_id, "");
    }

    #[test]
    fn registry_missing_present_empty_and_exact_ids_are_distinct() {
        let mut controller = EngineSettingsController::new();
        controller.select_thread(Some(&thread_id("thread-a")));
        controller.on_registry_loaded(RegisteredEngineProfilesResult::RegistryMissing);
        assert_eq!(controller.registry_view(), RegistryView::Missing);
        assert_eq!(controller.status(), EngineSettingsStatus::RegistryMissing);

        controller.on_registry_loaded(RegisteredEngineProfilesResult::RegistryPresent {
            profile_ids: Vec::new(),
        });
        assert_eq!(controller.registry_view(), RegistryView::PresentEmpty);
        assert_eq!(
            controller.status(),
            EngineSettingsStatus::RegistryPresentEmpty
        );

        controller.on_registry_loaded(registered_present(&["alpha", "beta"]));
        assert!(matches!(controller.registry_view(), RegistryView::Present(ids) if ids.len()==2));
        load_unconfigured(&mut controller, &thread_id("thread-a"));
        assert_eq!(controller.status(), EngineSettingsStatus::Unconfigured);
    }

    #[test]
    fn edits_are_local_dirty_and_cancel_does_not_emit_save() {
        let mut controller = EngineSettingsController::new();
        let tid = thread_id("thread-a");
        controller.select_thread(Some(&tid));
        controller.on_registry_loaded(registered_present(&["default"]));
        load_unconfigured(&mut controller, &tid);
        controller.draft_mut().profile_id = "default".to_owned();
        controller.draft_mut().model_id = "model-test".to_owned();
        // Dirty; the Forge decides whether the draft is complete.
        assert!(controller.is_dirty());
        assert_eq!(controller.status(), EngineSettingsStatus::Dirty);
        assert!(controller.can_save());
        controller.cancel();
        assert!(!controller.is_dirty());
        assert_eq!(controller.draft().profile_id, "");
    }

    #[test]
    fn saving_asks_the_forge_to_build_the_draft_and_shows_its_refusal() {
        let mut controller = EngineSettingsController::new();
        let tid = thread_id("thread-a");
        controller.select_thread(Some(&tid));
        controller.on_registry_loaded(registered_present(&["default"]));
        load_unconfigured(&mut controller, &tid);
        assert_eq!(controller.draft().profile_id, "");
        assert!(!controller.can_save());
        assert!(controller.begin_resolution().is_none());
        let valid = sample_config("default");
        controller
            .draft_mut()
            .clone_from(&EngineSettingsDraft::from_config(&valid));
        // Empty variant is explicit None, not synthesized.
        assert_eq!(controller.draft().variant_id, "");
        let query = controller.begin_resolution().expect("resolution asked");
        assert_eq!(query.thread_id, tid);
        assert_eq!(&query.configuration, controller.draft());
        assert_eq!(controller.status(), EngineSettingsStatus::Saving);
        assert!(
            controller.begin_resolution().is_none(),
            "one resolution at a time"
        );
        // Another draft's late answer is not this one's.
        let mut other = controller.draft().clone();
        other.model_id = "model-other".to_owned();
        assert!(!controller.finish_resolution(&tid, &other));
        assert!(controller.finish_resolution(&tid, &query.configuration));
        controller.on_configuration_refused("The approval value is unsupported.".to_owned());
        assert_eq!(
            controller.refusal(),
            Some("The approval value is unsupported.")
        );
        assert_eq!(
            controller.failure_operation(),
            Some(EngineSettingsFailureOperation::Input)
        );
        controller.draft_mut().approval = "never".to_owned();
        assert!(
            controller
                .apply_manual_configuration(&valid_document())
                .is_ok()
        );
        assert_eq!(controller.refusal(), None);
    }

    fn valid_document() -> String {
        sample_draft("default").to_document()
    }

    #[test]
    fn save_success_applies_returned_revision_plus_retained_config() {
        let (mut controller, tid, _) = ready_controller();
        let config = make_dirty_config(&mut controller);
        let request = request_id("request-a");
        assert!(controller.begin_direct_save(tid.clone(), request.clone(), config.clone()));
        assert_eq!(controller.status(), EngineSettingsStatus::Saving);
        let result = SetThreadEngineConfigResult {
            request_id: request,
            thread_id: tid.clone(),
            revision: revision(1),
            disposition: artisan_domain::ReceiptDisposition::Accepted,
        };
        controller.on_save_succeeded(&result, config.clone());
        assert_eq!(controller.status(), EngineSettingsStatus::Ready);
        assert_eq!(controller.revision(), Some(revision(1)));
        assert_eq!(controller.authoritative_config(), Some(&config));
        assert!(!controller.is_dirty());
    }

    #[test]
    fn conflict_emits_exactly_one_authoritative_refresh() {
        let mut controller = EngineSettingsController::new();
        let tid = thread_id("thread-a");
        controller.select_thread(Some(&tid));
        controller.on_registry_loaded(registered_present(&["default"]));
        let config = sample_config("default");
        load_configured(&mut controller, &tid, 3, config);
        let new_config = make_dirty_config(&mut controller);
        let request = request_id("request-a");
        assert!(controller.begin_direct_save(tid.clone(), request.clone(), new_config));
        controller.on_conflict(tid.clone(), &request);
        assert_eq!(
            controller.status(),
            EngineSettingsStatus::ConflictRefreshing
        );
        assert_eq!(controller.pending_reload_thread(), Some(&tid));
        let generation = admit_settings_load(&mut controller, &tid);
        assert!(controller.pending_reload_thread().is_none());
        controller.on_settings_loaded(
            generation,
            ThreadEngineSettingsResult::Unconfigured {
                thread_id: tid.clone(),
            },
        );
        assert_eq!(controller.status(), EngineSettingsStatus::Unconfigured);
        // Second conflict without pending should be ignored.
        controller.on_conflict(tid, &request_id("request-a"));
        assert_eq!(controller.status(), EngineSettingsStatus::Unconfigured);
    }

    #[test]
    fn stale_load_responses_are_fenced_by_thread_and_generation() {
        let mut controller = EngineSettingsController::new();
        let first = thread_id("thread-a");
        let second = thread_id("thread-b");
        controller.select_thread(Some(&first));
        controller.on_registry_loaded(registered_present(&["default"]));
        let first_generation = admit_settings_load(&mut controller, &first);
        controller.select_thread(Some(&second));
        let second_generation = admit_settings_load(&mut controller, &second);
        controller.select_thread(Some(&first));
        let current_generation = admit_settings_load(&mut controller, &first);
        let stale_first = ThreadEngineSettingsResult::Configured {
            thread_id: first.clone(),
            revision: revision(5),
            config: Box::new(sample_config("default")),
        };
        controller.on_settings_loaded(first_generation, stale_first);
        controller.on_settings_load_failed(first.clone(), first_generation, bridge_busy());
        controller.on_settings_load_failed(second.clone(), second_generation, bridge_busy());
        controller.on_settings_loaded(
            second_generation,
            ThreadEngineSettingsResult::Unconfigured { thread_id: second },
        );
        assert!(controller.authoritative_settings().is_none());
        controller.on_settings_loaded(
            current_generation,
            ThreadEngineSettingsResult::Unconfigured { thread_id: first },
        );
        assert!(matches!(
            controller.authoritative_settings(),
            Some(ThreadEngineSettingsResult::Unconfigured { .. })
        ));
    }

    #[test]
    fn stale_save_responses_are_fenced_by_exact_request_identity() {
        let (mut controller, tid, _) = ready_controller();
        let retained = make_dirty_config(&mut controller);
        let request = request_id("request-a");
        assert!(controller.begin_direct_save(tid.clone(), request.clone(), retained.clone()));
        let stale_request = request_id("request-b");
        controller.on_save_failed(&tid, &stale_request, bridge_busy());
        controller.on_conflict(tid.clone(), &stale_request);
        controller.on_save_succeeded(
            &SetThreadEngineConfigResult {
                request_id: stale_request,
                thread_id: tid.clone(),
                revision: revision(9),
                disposition: artisan_domain::ReceiptDisposition::Accepted,
            },
            retained.clone(),
        );
        assert_eq!(controller.status(), EngineSettingsStatus::Saving);
        assert_eq!(controller.pending_save_request_id(), Some(&request));
        controller.on_save_succeeded(
            &SetThreadEngineConfigResult {
                request_id: request,
                thread_id: tid,
                revision: revision(9),
                disposition: artisan_domain::ReceiptDisposition::Accepted,
            },
            retained,
        );
        assert_eq!(controller.status(), EngineSettingsStatus::Ready);
    }

    #[test]
    fn cancel_is_noop_while_an_admitted_save_is_pending() {
        let (mut controller, tid, _) = ready_controller();
        let retained = make_dirty_config(&mut controller);
        let draft_before = controller.draft().clone();
        assert!(controller.begin_direct_save(tid, request_id("request-a"), retained,));
        assert!(!controller.can_cancel());
        controller.cancel();
        assert_eq!(controller.draft(), &draft_before);
        assert_eq!(controller.status(), EngineSettingsStatus::Saving);
    }

    #[test]
    fn bridge_admission_refusals_rearm_without_losing_draft_or_claiming_progress() {
        let mut controller = EngineSettingsController::new();
        let tid = thread_id("thread-a");
        controller.select_thread(Some(&tid));
        controller.on_registry_load_admission_failed(bridge_busy());
        assert!(controller.needs_registry_load());
        assert_eq!(
            controller.failure_operation(),
            Some(EngineSettingsFailureOperation::Registry)
        );

        controller.on_registry_loaded(registered_present(&["default"]));
        controller.on_settings_load_admission_failed(tid.clone(), bridge_stopped());
        assert!(controller.needs_settings_load());
        assert!(controller.active_settings_generation().is_none());
        assert_eq!(
            controller.failure_operation(),
            Some(EngineSettingsFailureOperation::SettingsRead)
        );

        load_unconfigured(&mut controller, &tid);
        let retained = sample_config("default");
        controller
            .draft_mut()
            .clone_from(&EngineSettingsDraft::from_config(&retained));
        controller.draft_mut().model_id = "model-next".to_owned();
        let draft_before = controller.draft().clone();
        controller.on_save_admission_failed(bridge_stopped());
        assert!(controller.pending_save_request_id().is_none());
        assert_eq!(controller.draft(), &draft_before);
        assert_eq!(
            controller.failure_operation(),
            Some(EngineSettingsFailureOperation::Save)
        );
    }

    #[test]
    fn save_admission_failure_clears_defensive_pending_state() {
        let (mut controller, tid, _) = ready_controller();
        let retained = make_dirty_config(&mut controller);
        let draft_before = controller.draft().clone();
        assert!(controller.begin_direct_save(tid, request_id("request-a"), retained));
        controller.on_save_admission_failed(bridge_busy());
        assert!(controller.pending_save_request_id().is_none());
        assert_eq!(controller.draft(), &draft_before);
        assert_eq!(
            controller.failure_operation(),
            Some(EngineSettingsFailureOperation::Save)
        );
    }

    #[test]
    fn generation_exhaustion_fails_closed_without_rearming_the_read() {
        let mut controller = EngineSettingsController::new();
        let tid = thread_id("thread-a");
        controller.select_thread(Some(&tid));
        controller.next_settings_load_generation =
            Some(SettingsLoadGeneration::from_raw_for_test(u64::MAX));
        let failure = controller.prepare_settings_load().expect_err("exhausted");
        assert_eq!(failure.category, ServiceFailureCategory::Integrity);
        controller.on_settings_load_admission_failed(tid, failure);
        assert!(!controller.needs_settings_load());
        assert!(controller.active_settings_generation().is_none());
    }

    #[test]
    fn certified_profile_selection_is_checked_against_the_authoritative_registry() {
        let mut controller = EngineSettingsController::new();
        let tid = thread_id("thread-a");
        controller.select_thread(Some(&tid));
        controller.on_registry_loaded(registered_present(&["default"]));
        load_unconfigured(&mut controller, &tid);
        assert!(!controller.select_profile(&profile_id("unregistered")));
        assert_eq!(controller.draft().profile_id, "");
        assert!(controller.select_profile(&profile_id("default")));
        assert_eq!(controller.draft().profile_id, "default");
    }

    #[test]
    fn manual_configuration_is_complete_bounded_and_does_not_retain_rejected_text() {
        let template = manual_configuration_template();
        assert_eq!(template.lines().count(), MANUAL_CONFIGURATION_KEYS.len());
        assert_eq!(
            parse_manual_configuration(&template),
            Ok(EngineSettingsDraft::default())
        );

        let expected = sample_draft("default");
        let document = expected.to_document();
        assert_eq!(parse_manual_configuration(&document), Ok(expected.clone()));

        let unknown = template.replacen("profile_id=", "unknown=", 1);
        let unknown_error = parse_manual_configuration(&unknown).expect_err("unknown key");
        assert_eq!(unknown_error.field(), "configuration");
        assert!(parse_manual_configuration("profile_id=\nprofile_id=\n").is_err());
        assert!(parse_manual_configuration("profile_id=has=extra\n").is_err());
        let missing = template
            .lines()
            .take(MANUAL_CONFIGURATION_KEYS.len() - 1)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(parse_manual_configuration(&missing).is_err());
        assert!(
            parse_manual_configuration(&"x".repeat(MAX_MANUAL_CONFIGURATION_BYTES + 1)).is_err()
        );

        let mut controller = EngineSettingsController::new();
        let tid = thread_id("thread-a");
        controller.select_thread(Some(&tid));
        controller.on_registry_loaded(registered_present(&["default"]));
        load_unconfigured(&mut controller, &tid);
        let rejected = "profile_id=secret-profile\n";
        assert!(controller.apply_manual_configuration(rejected).is_err());
        assert!(!format!("{:?}", controller.input_error()).contains("secret-profile"));
        assert_eq!(controller.draft(), &EngineSettingsDraft::default());
        assert!(!controller.can_save());
    }

    #[test]
    fn failure_is_redacted_and_does_not_reveal_source_values() {
        let failure = ServiceFailure {
            stage: ServiceFailureStage::Request,
            category: ServiceFailureCategory::Peer,
        };
        let display = failure.to_string();
        assert!(!display.contains("model-test"));
        assert!(!display.contains("route-test"));
        assert!(!display.contains("default"));
    }

    #[test]
    fn registry_is_cached_for_application_lifetime() {
        let mut controller = EngineSettingsController::new();
        controller.select_thread(Some(&thread_id("thread-a")));
        controller.on_registry_loaded(registered_present(&["default"]));
        assert!(!controller.needs_registry_load());
        controller.select_thread(Some(&thread_id("thread-b")));
        assert!(!controller.needs_registry_load());
        assert_eq!(
            controller.registry_view(),
            RegistryView::Present(vec![profile_id("default")])
        );
    }
}
