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
