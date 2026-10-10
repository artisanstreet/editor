use super::*;

#[gpui::test]
fn host_crash_reconnects_automatically_without_losing_view_draft_or_selection(
    cx: &mut TestAppContext,
) {
    let (service, _commands, finished) = NativeTransportService::pending_for_test();
    let service = Arc::new(service);
    let (next, _next_commands, _) = NativeTransportService::pending_for_test();
    let next = Arc::new(next);
    let (workspace, cx) = connected_workspace(cx, &service, vec![next.clone()]);
    let view = selected(&workspace, cx);
    set_draft(&view, "draft survives WSL restart", cx);
    let thread = ThreadId::parse("interrupted-thread").unwrap();
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.selected_thread = Some(thread.clone());
            app.handle_service_stopped(ServiceStopStatus::Failed, cx);
        })
    });
    finished.store(true, std::sync::atomic::Ordering::Release);
    cx.executor().advance_clock(Duration::from_secs(3));
    cx.run_until_parked();
    assert_eq!(selected(&workspace, cx), view);
    assert_eq!(draft(&view, cx), "draft survives WSL restart");
    cx.update(|_, cx| {
        let app = view.read(cx);
        assert!(Arc::ptr_eq(app.service.as_ref().unwrap(), &next));
        assert_eq!(app.selected_thread.as_ref(), Some(&thread));
        assert!(!app.service_stopped);
        assert!(app.project_picker_action_is_admissible());
    });
}

#[gpui::test]
fn automatic_recovery_does_not_override_integrity_failure_or_quit(cx: &mut TestAppContext) {
    let (service, _commands, finished) = NativeTransportService::pending_for_test();
    let service = Arc::new(service);
    let (workspace, cx) = connected_workspace(cx, &service, vec![]);
    let view = selected(&workspace, cx);
    finished.store(true, std::sync::atomic::Ordering::Release);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            app.set_failure(
                ServiceFailure {
                    stage: ServiceFailureStage::Request,
                    category: ServiceFailureCategory::Integrity,
                },
                cx,
            )
        });
        workspace.update(cx, NativeWorkspace::recover_connection);
        assert!(Arc::ptr_eq(
            view.read(cx).service.as_ref().unwrap(),
            &service
        ));
        view.update(cx, |app, cx| {
            app.set_failure(command_failure(CommandSendError::Stopped), cx);
            app.shutdown_prepared = true;
        });
        workspace.update(cx, NativeWorkspace::recover_connection);
        assert!(Arc::ptr_eq(
            view.read(cx).service.as_ref().unwrap(),
            &service
        ));
    });
}

#[gpui::test]
fn retiring_a_thread_without_a_host_leaves_navigation_admissible(cx: &mut TestAppContext) {
    let (service, mut commands, _) = NativeTransportService::pending_for_test();
    let service = Arc::new(service);
    let (workspace, cx) = connected_workspace(cx, &service, vec![]);
    let view = selected(&workspace, cx);
    let source = ThreadId::parse("recovered-thread").unwrap();
    let mounted = ThreadId::parse("next-thread").unwrap();
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            // Failure recovery released the host but the Forge subscription
            // is still open.
            app.selected_thread = Some(source.clone());
            app.retire_host(cx);
            assert!(app.selected_thread.is_none());
            assert!(app.ordinary_unsubscribe_thread.is_none());
            assert!(app.project_picker_action_is_admissible());

            // Another thread mounts before the Forge acknowledges the stop.
            app.selected_thread = Some(mounted.clone());
            app.handle_service_event(
                NativeTransportEvent::ConversationSubscriptionStopped {
                    thread_id: source.clone(),
                    request_id: RequestId::parse("late-stop-ack").unwrap(),
                    stopped: artisan_protocol::ConversationSubscriptionStopped {
                        thread_id: source.clone(),
                    },
                },
                cx,
            );
            assert_eq!(app.selected_thread.as_ref(), Some(&mounted));
            assert!(app.ordinary_unsubscribe_thread.is_none());
            assert!(app.project_picker_action_is_admissible());
        })
    });
    assert!(
        commands.try_recv().is_ok(),
        "the Forge subscription is still stopped"
    );
}

#[gpui::test]
fn connection_loss_releases_navigation_waits_and_ignores_old_receipts(cx: &mut TestAppContext) {
    let (service, _commands, _) = NativeTransportService::pending_for_test();
    let service = Arc::new(service);
    let (workspace, cx) = connected_workspace(cx, &service, vec![]);
    let view = selected(&workspace, cx);
    set_draft(&view, "keep this draft", cx);
    cx.update(|_, cx| {
        view.update(cx, |app, cx| {
            let source = ThreadId::parse("source-thread").unwrap();
            let receipt = RequestId::parse("lost-switch-receipt").unwrap();
            app.thread_switch_flight = Some(ThreadSwitchFlight {
                #[cfg(feature = "flight-recorder")]
                trace: artisan_tracing::span!("navigation", "test.thread_switch"),
                source_thread: source.clone(),
                target_thread: Some(ThreadId::parse("target-thread").unwrap()),
                generation: 1,
                carry_draft: false,
                phase: ThreadSwitchPhase::AwaitingUnsubscribeStop {
                    request_id: Some(receipt.clone()),
                },
            });
            app.ordinary_unsubscribe_thread = Some(source);
            app.intake_stage = Some(NativeProjectIntakeStage::RefreshingThreads);
            app.handle_delivery_lost(command_failure(CommandSendError::Stopped), cx);
            assert!(app.thread_switch_flight.is_none());
            assert!(app.ordinary_unsubscribe_thread.is_none());
            assert!(app.intake_stage.is_none());
            assert!(app.retained_switch_request_ids.contains(&receipt));
            assert!(app.project_picker_action_is_admissible());
        })
    });
    assert_eq!(draft(&view, cx), "keep this draft");
}
