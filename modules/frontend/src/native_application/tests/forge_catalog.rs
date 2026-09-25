//! Catalogs and account readiness as the Forge serves them: the Forge
//! applies each engine's readiness verdict to the runnable harnesses of the
//! catalogs it serves, and the Editor reads the catalog again when a verdict
//! changes.

use super::*;

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
