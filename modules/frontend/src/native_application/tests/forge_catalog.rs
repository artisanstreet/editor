//! Catalogs, readiness, and model selections as the Forge serves them: the
//! Forge applies each engine's readiness verdict to the runnable harnesses
//! of the catalogs it serves (the Editor reads the catalog again when a
//! verdict changes), and it resolves the Editor's model selections into the
//! configurations it runs.

use super::*;

/// The Codex configuration the Forge resolves for the fixture's `codex-sol`
/// defaults, with an optional context-window override.
pub(super) fn forge_codex_config(window: Option<u64>) -> artisan_domain::EngineRunConfig {
    use artisan_domain::{
        ApprovalMode, ByteLimit, CodexModelContextWindow, CodexReasoningEffort, CodexSelection,
        CodexServiceTier, CountLimit, EngineAgentId, EngineModelId, EnginePermissionPolicy,
        EngineProfileId, EngineRunConfig, EngineRuntimeControls, EngineRuntimeControlsInput,
        EngineSelection, FilesystemAccess, FiniteMillis, NetworkAccess, PermissionId,
        WebSearchAccess,
    };
    let millis = |value| FiniteMillis::new(value).expect("budget");
    let bytes = |value| ByteLimit::new(value).expect("capacity");
    let runtime = EngineRuntimeControls::new(EngineRuntimeControlsInput {
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
        max_header_count: CountLimit::new(64).expect("count"),
        max_http_buffer_bytes: bytes(64 * 1024),
        max_stderr_bytes: bytes(64 * 1024),
        observation_capacity: CountLimit::new(256).expect("count"),
    })
    .expect("runtime");
    let selection = CodexSelection::new(
        EngineProfileId::parse("default").expect("profile"),
        Some(EngineModelId::parse("gpt-5.6-sol").expect("model")),
        EnginePermissionPolicy::new(
            PermissionId::parse("autonomous").expect("permission"),
            EngineAgentId::parse("artisan-v1-codex-autonomous-offline-no-web").expect("agent"),
            ApprovalMode::OnRequest,
            FilesystemAccess::Workspace,
            NetworkAccess::Disabled,
            WebSearchAccess::Disabled,
        ),
        Some(CodexReasoningEffort::parse("low").expect("effort")),
        Some(CodexServiceTier::parse("standard").expect("tier")),
        window.map(|window| CodexModelContextWindow::new(window).expect("window")),
    )
    .expect("codex selection");
    EngineRunConfig::new(EngineSelection::Codex(selection), runtime)
}

/// Answers the pending selection resolution as the Forge would, with
/// `config`, through the real resolution handler.
pub(super) fn answer_resolution(
    application: &mut NativeApplication,
    cx: &mut Context<NativeApplication>,
    config: artisan_domain::EngineRunConfig,
) {
    let (thread_id, selection) = application
        .pending_resolution
        .clone()
        .expect("a selection resolution was requested");
    application.receive_selection_resolution(thread_id, selection, Ok(Ok(Box::new(config))), cx);
}

/// Delivers the Forge's typed refusal of the current send.
pub(super) fn refuse_send(
    application: &mut NativeApplication,
    cx: &mut Context<NativeApplication>,
    kind: artisan_domain::SubmissionRefusalKind,
    message: &str,
) {
    let flight = application
        .message_flight
        .as_ref()
        .expect("a send in flight");
    let (scope, request_id) = (flight.scope.clone(), flight.request_id.clone());
    application.handle_service_event(
        NativeTransportEvent::ForgeDecision(
            crate::native_transport_service::ForgeDecisionEvent::SendRefused {
                scope,
                request_id,
                refusal: artisan_domain::SubmissionRefusal::new(kind, message).expect("refusal"),
            },
        ),
        cx,
    );
}

/// The verdict the Forge serves with a report of this authentication.
pub(super) fn forge_readiness(
    display_name: &str,
    authentication: NativeUsageAuthentication,
) -> crate::native_profile_usage::EngineReadiness {
    use crate::native_profile_usage::{EngineReadiness, EngineReadinessVerdict};
    match authentication {
        NativeUsageAuthentication::Authenticated => EngineReadiness::ready(),
        NativeUsageAuthentication::Unauthenticated => EngineReadiness::new(
            EngineReadinessVerdict::NeedsSignIn,
            Some(format!("{display_name} account sign-in is required.")),
        )
        .expect("verdict"),
        NativeUsageAuthentication::Unknown => EngineReadiness::new(
            EngineReadinessVerdict::NotReady,
            Some(format!(
                "{display_name} account status is unavailable right now."
            )),
        )
        .expect("verdict"),
    }
}

/// Serves the current catalog again as the Forge does once `engines` have
/// proven their accounts: the Forge applies account readiness to the
/// runnable harnesses of every catalog it serves.
pub(super) fn serve_catalog_with_runnable(
    application: &mut NativeApplication,
    cx: &mut Context<NativeApplication>,
    engines: &[&str],
) {
    let mut catalog = application.served_catalog(cx);
    catalog
        .runnable_harness_ids
        .retain(|engine| !["codex", "claude"].contains(&engine.as_str()));
    for engine in engines {
        catalog.runnable_harness_ids.push((*engine).to_owned());
    }
    application.model_selector.update(cx, |selector, cx| {
        selector.set_snapshot(catalog, cx);
    });
    application.sync_composer_model_policy(cx);
}

pub(super) fn reported_usage_entry_with_auth(
    engine_id: &str,
    display_name: &str,
    authentication: NativeUsageAuthentication,
    windows: Vec<NativeUsageWindow>,
) -> NativeUsageEntry {
    NativeUsageEntry {
        engine_id: engine_id.to_owned(),
        display_name: display_name.to_owned(),
        report: Some(NativeUsageReport {
            engine_id: engine_id.to_owned(),
            display_name: display_name.to_owned(),
            authentication,
            account_email: None,
            quota_surface: NativeUsageQuotaSurface::Supported,
            windows,
            failure: None,
            readiness: forge_readiness(display_name, authentication),
        }),
        failure: None,
        fetched_at_ms: Some(super::profile_usage_now_ms().saturating_sub(60_000)),
    }
}

#[gpui::test]
fn signed_out_refresh_removes_admission_and_updates_settings(cx: &mut TestAppContext) {
    let thread_id = ThreadId::parse("settings-signout-task").expect("thread");
    let (view, cx) = cx.add_window_view(test_application);
    let (sink, commands) = command_sink([Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_ready_message_surface(application, cx, thread_id.clone(), "draft", sink);
            admit_probed_codex_usage(application, cx);
            assert!(
                application
                    .served_catalog(cx)
                    .selectability("codex-sol")
                    .is_available()
            );
            mount_settings_engine(application, cx, "codex");
        });
    });
    cx.run_until_parked();
    // The mounted Settings refresh button forces a probed re-read
    // through the real transport command; the reply is fed after the
    // click delivery flushes.
    let refresh = cx
        .debug_bounds("settings-installation-refresh")
        .expect("settings refresh mounted");
    cx.simulate_click(refresh.center(), gpui::Modifiers::default());
    cx.run_until_parked();
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            let forced = commands.borrow();
            assert!(
                forced.iter().any(|command| matches!(
                    command,
                    NativeTransportCommand::ReadAccountUsage { force: true, .. }
                )),
                "refresh action must force an account re-read"
            );
            drop(forced);
            // The signed-out reply removes the admission and updates the
            // mounted page through the real response handler.
            let generation = application.profile_usage_generation;
            let request_seq = application
                .profile_usage
                .pending_seq("codex")
                .expect("forced codex re-read admitted");
            application.handle_account_usage(
                "codex",
                generation,
                request_seq,
                reported_usage_entry_with_auth(
                    "codex",
                    "Codex",
                    NativeUsageAuthentication::Unauthenticated,
                    Vec::new(),
                ),
                cx,
            );
            // The new verdict makes the Editor read the catalog again; the
            // Forge serves it without Codex runnable.
            assert!(commands.borrow().iter().any(|command| matches!(
                command,
                NativeTransportCommand::ReadComposerCatalog { .. }
                    | NativeTransportCommand::ForgeDecision(_)
            )));
            serve_catalog_with_runnable(application, cx, &[]);
            assert!(
                !application
                    .served_catalog(cx)
                    .selectability("codex-sol")
                    .is_available()
            );
            let screen = application
                .settings_screen
                .clone()
                .expect("settings screen mounted");
            let snapshot = screen
                .read(cx)
                .engine_snapshot()
                .cloned()
                .expect("engine snapshot");
            assert_eq!(
                snapshot.readiness,
                crate::native_profile_usage::EngineReadinessVerdict::NeedsSignIn
            );
            assert_ne!(snapshot.saved_model.as_deref(), Some("codex-sol"));
        });
    });
}

#[gpui::test]
fn periodic_catalog_refresh_reads_the_forge_host_catalog(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(test_application);
    let (sink, commands) = command_sink([Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            // Without a thread the refresh asks the Forge for its host
            // catalog instead of reading a file the Forge published.
            application.refresh_model_catalog(cx);
            assert!(commands.borrow().iter().any(|command| matches!(
                command,
                NativeTransportCommand::ForgeDecision(
                    crate::native_transport_service::ForgeDecisionCommand::ReadHostCatalog
                )
            )));
            let mut catalog = application.host_model_catalog.clone().unwrap();
            catalog.catalog_revision = "refreshed-models".to_owned();
            catalog.manifest.models[0].name = "Newly discovered model".to_owned();
            application.receive_forge_decision(
                crate::native_transport_service::ForgeDecisionEvent::HostCatalog(Ok(Box::new(
                    catalog,
                ))),
                cx,
            );
            let snapshot = application.model_selector.read(cx).state().snapshot();
            assert_eq!(snapshot.catalog_revision, "refreshed-models");
            assert_eq!(snapshot.manifest.models[0].name, "Newly discovered model");
            // A failed read keeps the catalog already shown.
            application.receive_forge_decision(
                crate::native_transport_service::ForgeDecisionEvent::HostCatalog(Err(
                    super::super::invalid_service_failure(),
                )),
                cx,
            );
            assert_eq!(
                application
                    .model_selector
                    .read(cx)
                    .state()
                    .snapshot()
                    .catalog_revision,
                "refreshed-models"
            );
        });
    });
}

#[gpui::test]
fn periodic_catalog_refresh_requests_active_scope_once(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(test_application);
    let (sink, commands) = command_sink([Ok(()), Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            let thread = ThreadId::parse("catalog-refresh-thread").unwrap();
            install_ready_message_surface(application, cx, thread.clone(), "draft", sink);
            let selection = application.catalog_controller.select_scope(
                thread.clone(),
                artisan_domain::EngineProfileId::parse("default").unwrap(),
            ).unwrap();
            let scope = selection.scope().clone();
            assert!(application.catalog_controller.mark_catalog_admitted(&scope));
            assert!(application.catalog_controller.on_catalog_loaded(&scope));
            application.refresh_model_catalog(cx);
            application.refresh_model_catalog(cx);
            assert_eq!(commands.borrow().iter().filter(|command| matches!(
                command,
                NativeTransportCommand::ReadComposerCatalog { thread_id, .. } if *thread_id == thread
            )).count(), 1);
        });
    });
}

#[gpui::test]
fn periodic_catalog_refresh_recovers_missing_conversation_scope(cx: &mut TestAppContext) {
    let (view, cx) = cx.add_window_view(test_application);
    let (sink, commands) = command_sink([Ok(()), Ok(()), Ok(())]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            let thread = ThreadId::parse("catalog-recovery-thread").unwrap();
            install_ready_message_surface(application, cx, thread.clone(), "draft", sink);
            application.reset_composer_catalog(cx);
            assert!(application.catalog_controller.scope().is_none());
            application.refresh_model_catalog(cx);
            application.refresh_model_catalog(cx);
            assert_eq!(commands.borrow().iter().filter(|command| matches!(command,
                NativeTransportCommand::ReadComposerCatalog { thread_id, .. } if *thread_id == thread
            )).count(), 1);
        });
    });
}

// The Forge's preferences build on the Forge catalog fixtures above.
#[path = "forge_preferences.rs"]
mod forge_preferences;

// Pushed usage and titles build on the same fixtures.
#[path = "host_state_push.rs"]
mod host_state_push;

// The manual settings draft resolves through the Forge like a selection.
#[path = "manual_settings.rs"]
mod manual_settings;

// Managed engine installs build on the same fixtures.
#[path = "engine_installs.rs"]
mod engine_installs;
