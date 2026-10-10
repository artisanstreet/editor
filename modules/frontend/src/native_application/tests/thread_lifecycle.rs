//! Where threads live in the sidebar: reading a thread never moves it, the
//! row's context menu settles and archives it, and Settings lists
//! the archive and restores from it.

use super::*;
use crate::native_transport_service::{HoldKind, ThreadLifecycleCommand, ThreadLifecycleEvent};
use artisan_domain::{
    ArchivedThread, ArchivedThreadListing, RecentThread, RecentThreadListing, ThreadAttention,
    ThreadChange, ThreadPlacement, UpdateThread,
};
use gpui::{Entity, VisualTestContext};

const INBOX: &str = "artisan-sidebar-threads-unread";
const MENU: &str = "artisan-pointer-menu";

fn row(id: &str, title: &str, placement: ThreadPlacement) -> RecentThread {
    let mut summary = thread(id, "alpha", title);
    summary.last_message_at = Some(UnixMillis::from_millis(
        super::super::impl_sidebar_threads::wall_clock().as_millis() - 60_000,
    ));
    RecentThread {
        thread: summary,
        subtitle: DisplayName::parse("owner/editor").expect("subtitle"),
        project_icon: artisan_domain::RecentProjectIcon::default(),
        placement,
    }
}

fn push(
    application: &mut NativeApplication,
    rows: Vec<RecentThread>,
    cx: &mut Context<NativeApplication>,
) {
    application.handle_service_event(
        NativeTransportEvent::HostState(
            crate::native_transport_service::HostStateEvent::RecentThreads(
                RecentThreadListing::new(rows).expect("recent threads"),
            ),
        ),
        cx,
    );
}

fn mounted(
    cx: &mut TestAppContext,
) -> (
    Entity<NativeApplication>,
    &mut VisualTestContext,
    Rc<RefCell<Vec<NativeTransportCommand>>>,
) {
    let (view, cx) = cx.add_window_view(test_application);
    let (sink, commands) = command_sink([]);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            push(
                application,
                vec![
                    row("first", "First thread", ThreadPlacement::Inbox),
                    row("second", "Second thread", ThreadPlacement::Inbox),
                ],
                cx,
            );
        });
    });
    cx.simulate_resize(gpui::size(gpui::px(1400.0), gpui::px(800.0)));
    cx.run_until_parked();
    (view, cx, commands)
}

fn updates(commands: &RefCell<Vec<NativeTransportCommand>>) -> Vec<UpdateThread> {
    commands
        .borrow()
        .iter()
        .filter_map(|command| match command {
            NativeTransportCommand::ThreadLifecycle(ThreadLifecycleCommand::Update(update)) => {
                Some(update.clone())
            }
            _ => None,
        })
        .collect()
}

fn right_click(cx: &mut VisualTestContext, selector: &'static str) {
    let position = cx.debug_bounds(selector).expect("row mounted").center();
    cx.simulate_event(gpui::MouseDownEvent {
        button: gpui::MouseButton::Right,
        position,
        modifiers: gpui::Modifiers::default(),
        click_count: 1,
        first_mouse: false,
    });
    cx.simulate_event(gpui::MouseUpEvent {
        button: gpui::MouseButton::Right,
        position,
        modifiers: gpui::Modifiers::default(),
        click_count: 1,
    });
    cx.run_until_parked();
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let position = cx.debug_bounds(selector).expect("target mounted").center();
    cx.simulate_click(position, gpui::Modifiers::default());
    cx.run_until_parked();
    // Let a closing menu finish its fade.
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(500));
    cx.run_until_parked();
}

fn in_inbox(cx: &mut VisualTestContext, selector: &'static str) -> bool {
    let (Some(inbox), Some(row)) = (cx.debug_bounds(INBOX), cx.debug_bounds(selector)) else {
        return false;
    };
    inbox.contains(&row.center())
}

#[gpui::test]
fn reading_a_finished_thread_leaves_it_in_view(cx: &mut TestAppContext) {
    let (view, cx, _) = mounted(cx);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            let mut finished = row("first", "First thread", ThreadPlacement::Inbox);
            finished.thread.attention = ThreadAttention::Finished;
            push(application, vec![finished], cx);
        });
    });
    cx.run_until_parked();
    assert!(in_inbox(cx, "artisan-sidebar-thread-first"));
    // The Forge marks it read once the reader has opened and left it.
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            push(
                application,
                vec![row("first", "First thread", ThreadPlacement::Inbox)],
                cx,
            );
        });
    });
    cx.run_until_parked();
    assert!(
        in_inbox(cx, "artisan-sidebar-thread-first"),
        "a read thread stays until it is settled"
    );
    assert!(
        cx.debug_bounds("artisan-sidebar-threads-history-header")
            .is_none()
    );
}

#[gpui::test]
fn the_row_menu_settles_and_archives_threads(cx: &mut TestAppContext) {
    let (view, cx, commands) = mounted(cx);
    right_click(cx, "artisan-sidebar-thread-first");
    assert!(
        cx.debug_bounds(MENU).is_some(),
        "right-click opens the menu"
    );
    for item in [
        "artisan-pointer-menu-settle",
        "artisan-pointer-menu-separator",
        "artisan-pointer-menu-archive",
    ] {
        assert!(cx.debug_bounds(item).is_some(), "{item} is in the menu");
    }

    click(cx, "artisan-pointer-menu-settle");
    let sent = updates(&commands);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].thread_id.as_str(), "first");
    assert_eq!(sent[0].change, ThreadChange::Settle);
    assert_eq!(
        NativeTransportCommand::ThreadLifecycle(ThreadLifecycleCommand::Update(sent[0].clone()))
            .hold_kind(),
        Some(HoldKind::ThreadChange)
    );
    assert!(
        cx.debug_bounds("artisan-sidebar-threads-history-header")
            .is_some(),
        "the settled thread moves into History at once"
    );
    assert!(!in_inbox(cx, "artisan-sidebar-thread-first"));

    right_click(cx, "artisan-sidebar-thread-second");
    click(cx, "artisan-pointer-menu-archive");
    let sent = updates(&commands);
    assert_eq!(sent[1].change, ThreadChange::Archive);
    assert!(
        cx.debug_bounds("artisan-sidebar-thread-second").is_none(),
        "the archived thread leaves the sidebar"
    );

    // A refused change says so and reads the listing again.
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.handle_service_event(
                NativeTransportEvent::ThreadLifecycle(ThreadLifecycleEvent::Updated {
                    update: sent[1].clone(),
                    result: Err(super::super::invalid_service_failure()),
                }),
                cx,
            );
            assert_eq!(
                application.window_notices.front_message(),
                Some("Couldn't archive \u{201c}Second thread\u{201d}. Try again."),
                "the refusal is announced"
            );
        });
    });
    assert!(
        commands
            .borrow()
            .iter()
            .any(|command| matches!(command, NativeTransportCommand::ReadRecentThreads))
    );
}

#[gpui::test]
fn settling_the_open_thread_keeps_its_unread_outcome_in_view(cx: &mut TestAppContext) {
    let (view, cx, _) = mounted(cx);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            let mut finished = row("first", "First thread", ThreadPlacement::Inbox);
            finished.thread.attention = ThreadAttention::Finished;
            push(application, vec![finished], cx);
        });
    });
    cx.run_until_parked();
    // The Forge marks it read on opening, and its listing may land before
    // the sidebar paints the open thread; the row keeps its dot regardless.
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.navigate(
                NativeRoute::Thread {
                    project: ProjectId::parse("alpha").expect("project"),
                    thread: ThreadId::parse("first").expect("thread"),
                },
                cx,
            );
            push(
                application,
                vec![row("first", "First thread", ThreadPlacement::Inbox)],
                cx,
            );
        });
    });
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("artisan-sidebar-thread-first-finished")
            .is_some(),
        "the open thread keeps its unread dot"
    );

    right_click(cx, "artisan-sidebar-thread-first");
    click(cx, "artisan-pointer-menu-settle");
    assert!(
        in_inbox(cx, "artisan-sidebar-thread-first"),
        "an unread outcome keeps the settled thread in view"
    );
    assert!(
        cx.debug_bounds("artisan-sidebar-threads-history-header")
            .is_none()
    );
    assert!(
        cx.debug_bounds("artisan-sidebar-thread-first-finished")
            .is_some(),
        "the row keeps its dot"
    );
    // The Forge confirms the settle.
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            push(
                application,
                vec![row("first", "First thread", ThreadPlacement::History)],
                cx,
            );
        });
    });
    cx.run_until_parked();
    assert!(in_inbox(cx, "artisan-sidebar-thread-first"));
}

#[gpui::test]
fn settings_lists_the_archive_and_restores_from_it(cx: &mut TestAppContext) {
    let (view, cx, commands) = mounted(cx);
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.navigate(
                NativeRoute::Settings {
                    section: SettingsRoute::Threads,
                    engine: None,
                },
                cx,
            );
        });
    });
    cx.run_until_parked();
    assert!(
        commands.borrow().iter().any(|command| matches!(
            command,
            NativeTransportCommand::ThreadLifecycle(ThreadLifecycleCommand::ReadArchived)
        )),
        "the Threads page reads the archive"
    );
    let archived_at = super::super::impl_sidebar_threads::wall_clock();
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            let listing = ArchivedThreadListing::new(vec![ArchivedThread {
                thread: row("old", "Old thread", ThreadPlacement::History),
                archived_at,
            }])
            .expect("archive");
            application.handle_service_event(
                NativeTransportEvent::ThreadLifecycle(ThreadLifecycleEvent::Archived(Ok(
                    Box::new(listing),
                ))),
                cx,
            );
        });
    });
    cx.run_until_parked();
    assert!(cx.debug_bounds("settings-archived-old").is_some());
    click(cx, "settings-archived-old-unarchive");
    let sent = updates(&commands);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].thread_id.as_str(), "old");
    assert_eq!(sent[0].change, ThreadChange::Unarchive);
    assert!(
        cx.debug_bounds("settings-archived-old").is_none(),
        "the restored thread leaves the archive at once"
    );
}
