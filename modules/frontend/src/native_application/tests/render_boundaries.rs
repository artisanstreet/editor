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
            summary.last_message_at = Some(UnixMillis::from_millis(
                super::super::impl_sidebar_threads::wall_clock().as_millis() - 60_000,
            ));
            let listing = RecentThreadListing::new(vec![RecentThread {
                thread: summary,
                subtitle: DisplayName::parse("Varde").expect("subtitle"),
                project_icon: Default::default(),
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
