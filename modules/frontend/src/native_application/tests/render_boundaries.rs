//! The frame's cached regions: a live transcript frame does not rebuild the
//! sidebar, a sidebar frame does not rebuild the transcript, and a sidebar
//! state change still reaches the sidebar.
//!
//! These tests opt in to the production cached views
//! ([`crate::view_boundary::CachedViewsInTests`]) and count renders through
//! the sidebar's and the conversation host's test seams.

use super::*;
use artisan_domain::{RecentThread, RecentThreadListing};

/// Mounts the thread `message-project`/`thread-a` with its transcript and
/// history delivered, in a window wide enough to seat the inspector.
fn mount_live_thread(
    cx: &mut TestAppContext,
) -> (
    gpui::Entity<NativeApplication>,
    &mut gpui::VisualTestContext,
) {
    cx.update(|app| app.set_global(crate::view_boundary::CachedViewsInTests));
    let (view, cx) = cx.add_window_view(test_application);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            install_project_option(application, "Varde");
            let thread = ThreadId::parse("thread-a").expect("thread");
            let (sink, _commands) = command_sink([]);
            install_ready_message_surface(application, cx, thread.clone(), "", sink);
            application
                .handle_service_event(NativeTransportEvent::Snapshot(snapshot_for(&thread, 1)), cx);
            application.handle_service_event(history_current_event(&thread), cx);
        });
    });
    cx.simulate_resize(gpui::size(gpui::px(1800.0), gpui::px(900.0)));
    cx.run_until_parked();
    (view, cx)
}

/// Renders so far, as `(sidebar, transcript host)`.
fn render_counts(
    view: &gpui::Entity<NativeApplication>,
    cx: &mut gpui::VisualTestContext,
) -> (usize, usize) {
    cx.update(|_, app| {
        let application = view.read(app);
        let sidebar = application.sidebar.read(app).renders();
        let host = application
            .conversation_host
            .as_ref()
            .expect("the thread's host is mounted")
            .read(app)
            .renders();
        (sidebar, host)
    })
}

#[gpui::test]
fn transcript_frames_reuse_the_sidebar(cx: &mut TestAppContext) {
    let (view, cx) = mount_live_thread(cx);
    let (sidebar_before, host_before) = render_counts(&view, cx);
    assert!(sidebar_before > 0, "the sidebar painted");
    assert!(host_before > 0, "the transcript painted");

    // What a live transcript line's animation frame does: the surface
    // notifies itself, dirtying its ancestors but none of its siblings.
    for _ in 0..3 {
        cx.update(|_, app| {
            let host = view
                .read(app)
                .conversation_host
                .clone()
                .expect("host mounted");
            let surface = host.read(app).surface().clone();
            surface.update(app, |_, cx| cx.notify());
        });
        cx.run_until_parked();
    }

    let (sidebar_after, host_after) = render_counts(&view, cx);
    assert!(
        host_after >= host_before + 3,
        "every transcript frame renders the transcript ({host_before} -> {host_after})"
    );
    assert_eq!(
        sidebar_after, sidebar_before,
        "transcript frames reuse the sidebar's previous paint"
    );
}

#[gpui::test]
fn sidebar_frames_reuse_the_transcript(cx: &mut TestAppContext) {
    let (view, cx) = mount_live_thread(cx);
    let (sidebar_before, host_before) = render_counts(&view, cx);

    // What the working dot's pulse does: the sidebar asks for a frame.
    cx.update(|_, app| {
        let sidebar = view.read(app).sidebar.clone();
        sidebar.update(app, |_, cx| cx.notify());
    });
    cx.run_until_parked();

    let (sidebar_after, host_after) = render_counts(&view, cx);
    assert!(
        sidebar_after > sidebar_before,
        "the sidebar rendered its frame"
    );
    assert_eq!(
        host_after, host_before,
        "sidebar frames reuse the transcript's previous paint"
    );
}

#[gpui::test]
fn a_recent_threads_push_renders_the_sidebar(cx: &mut TestAppContext) {
    let (view, cx) = mount_live_thread(cx);
    let (sidebar_before, _) = render_counts(&view, cx);
    assert!(
        cx.debug_bounds("artisan-sidebar-thread-thread-b").is_none(),
        "the row is not listed yet"
    );

    cx.update(|_, app| {
        view.update(app, |application, cx| {
            let mut summary = thread("thread-b", "message-project", "Pulse the dot");
            // Unread rows show outright; read idle ones fold into History.
            summary.attention = artisan_domain::ThreadAttention::Finished;
            summary.last_message_at = Some(UnixMillis::from_millis(
                super::super::impl_sidebar_threads::wall_clock().as_millis() - 60_000,
            ));
            let listing = RecentThreadListing::new(vec![RecentThread {
                thread: summary,
                subtitle: DisplayName::parse("Varde").expect("subtitle"),
                project_icon: Default::default(),
                placement: artisan_domain::ThreadPlacement::Inbox,
            }])
            .expect("recent threads");
            application.handle_service_event(
                NativeTransportEvent::HostState(
                    crate::native_transport_service::HostStateEvent::RecentThreads(listing),
                ),
                cx,
            );
        });
    });
    cx.run_until_parked();

    let (sidebar_after, _) = render_counts(&view, cx);
    assert!(
        sidebar_after > sidebar_before,
        "a listing change re-renders the sidebar"
    );
    assert!(
        cx.debug_bounds("artisan-sidebar-thread-thread-b").is_some(),
        "the pushed row paints"
    );
}

#[gpui::test]
fn a_delivered_message_renders_the_transcript(cx: &mut TestAppContext) {
    let (view, cx) = mount_live_thread(cx);
    let thread = ThreadId::parse("thread-a").expect("thread");
    // Frames elsewhere (the sidebar's working dot) reuse the transcript's
    // previous paint before anything is delivered.
    for _ in 0..3 {
        cx.update(|_, app| {
            let sidebar = view.read(app).sidebar.clone();
            sidebar.update(app, |_, cx| cx.notify());
        });
        cx.run_until_parked();
    }
    let (_, host_before) = render_counts(&view, cx);

    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.handle_service_event(
                NativeTransportEvent::PatchBatch(echo_batch(
                    &thread,
                    1,
                    "item-user",
                    None,
                    "turn-live",
                    0,
                    1,
                    "hello",
                )),
                cx,
            );
        });
    });
    cx.run_until_parked();

    let (_, host_after) = render_counts(&view, cx);
    assert!(
        host_after > host_before,
        "a delivered message re-renders the cached transcript ({host_before} -> {host_after})"
    );
    let row_renders = cx.update(|_, app| {
        let host = view.read(app).conversation_host.clone().expect("host");
        let surface = host.read(app).surface().clone();
        surface
            .read(app)
            .turn_row_renders(&TurnId::parse("turn-live").expect("turn"))
    });
    assert!(
        row_renders.is_some_and(|renders| renders > 0),
        "the delivered turn's row painted"
    );
}

#[gpui::test]
fn reopening_the_same_thread_paints_the_new_host(cx: &mut TestAppContext) {
    let (view, cx) = mount_live_thread(cx);
    let thread = ThreadId::parse("thread-a").expect("thread");

    // Reopening the thread retires its host and mounts a fresh one in the
    // same pass (`try_mount_pending_thread`): no frame ever sees the thread
    // without a host.
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.retire_host(cx);
            let (sink, _commands) = command_sink([]);
            install_ready_message_surface(application, cx, thread.clone(), "", sink);
            // The fresh subscription's start carries the snapshot.
            let host = application.conversation_host.clone().expect("new host");
            application.dispatch_snapshot(&host, snapshot_for(&thread, 1), cx);
            application.handle_service_event(history_current_event(&thread), cx);
        });
    });
    cx.run_until_parked();

    let (live, painted) = cx.update(|_, app| {
        let application = view.read(app);
        let live = application.conversation_host.clone().expect("live host");
        let painted = application
            .thread_screen
            .as_ref()
            .expect("thread screen")
            .read(app)
            .host()
            .clone();
        (live, painted)
    });
    assert_eq!(
        painted, live,
        "the thread screen paints the host that receives the thread's patches"
    );

    // A message sent after reopening paints on that host.
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.handle_service_event(
                NativeTransportEvent::PatchBatch(echo_batch(
                    &thread,
                    1,
                    "item-user",
                    None,
                    "turn-live",
                    0,
                    1,
                    "hello",
                )),
                cx,
            );
        });
    });
    cx.run_until_parked();
    let row_renders = cx.update(|_, app| {
        let application = view.read(app);
        let screen = application.thread_screen.as_ref().expect("thread screen");
        let surface = screen.read(app).host().read(app).surface().clone();
        surface
            .read(app)
            .turn_row_renders(&TurnId::parse("turn-live").expect("turn"))
    });
    assert!(
        row_renders.is_some_and(|renders| renders > 0),
        "the sent message paints in the reopened thread"
    );
}

#[gpui::test]
fn a_refused_surface_gesture_leaves_the_thread_live(cx: &mut TestAppContext) {
    let (view, cx) = mount_live_thread(cx);
    let thread = ThreadId::parse("thread-a").expect("thread");

    // A toggle on a section the scene no longer holds: the click raced the
    // scene that removed it, and the controller refuses it.
    cx.update(|_, app| {
        let host = view.read(app).conversation_host.clone().expect("host");
        let surface = host.read(app).surface().clone();
        surface.update(app, |surface, cx| {
            assert!(surface.enqueue_action_for_test(
                crate::conversation_surface::ConversationSurfaceAction::DisclosureToggleRequested {
                    id: crate::conversation_scene::SceneId::parse("gone-section").expect("id"),
                    requested_open: true,
                },
                cx,
            ));
        });
    });
    cx.run_until_parked();
    // What the application's host observation does on every host notify.
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            let host = application.conversation_host.clone().expect("host");
            application.pump_host_boundary(&host, cx);
        });
    });
    cx.update(|_, app| {
        assert!(
            !matches!(view.read(app).state, NativeViewState::Failure(_)),
            "a stale gesture is not a broken thread"
        );
    });

    // The thread keeps taking deliveries.
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.handle_service_event(
                NativeTransportEvent::PatchBatch(echo_batch(
                    &thread,
                    1,
                    "item-user",
                    None,
                    "turn-live",
                    0,
                    1,
                    "hello",
                )),
                cx,
            );
        });
    });
    cx.run_until_parked();
    let row_renders = cx.update(|_, app| {
        let host = view.read(app).conversation_host.clone().expect("host");
        let surface = host.read(app).surface().clone();
        surface
            .read(app)
            .turn_row_renders(&TurnId::parse("turn-live").expect("turn"))
    });
    assert!(row_renders.is_some_and(|renders| renders > 0));
}
