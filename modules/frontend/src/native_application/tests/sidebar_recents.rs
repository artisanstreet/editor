//! The sidebar's recent threads across every project: age groups, the
//! two-line rows, opening a thread in its own project, pushes instead of
//! polling; and the new-task project picker's menu that remains.

use super::*;
use crate::home_project_picker::HomeProjectPickerView;
use artisan_domain::{RecentThread, RecentThreadListing};
use gpui::{AppContext as _, ParentElement as _, Styled as _};

fn recent_row(id: &str, project: &str, title: &str, subtitle: &str, age_ms: i64) -> RecentThread {
    let mut summary = thread(id, project, title);
    summary.last_message_at = Some(UnixMillis::from_millis(
        super::super::super::impl_sidebar_threads::wall_clock().as_millis() - age_ms,
    ));
    RecentThread {
        thread: summary,
        subtitle: DisplayName::parse(subtitle).expect("subtitle"),
    }
}

fn recent_listing(rows: Vec<RecentThread>) -> RecentThreadListing {
    RecentThreadListing::new(rows).expect("recent threads")
}

fn push_recent(
    application: &mut NativeApplication,
    listing: RecentThreadListing,
    cx: &mut Context<NativeApplication>,
) {
    application.handle_service_event(
        NativeTransportEvent::HostState(
            crate::native_transport_service::HostStateEvent::RecentThreads(listing),
        ),
        cx,
    );
}

const HOUR_MS: i64 = 60 * 60 * 1_000;

#[gpui::test]
fn sidebar_lists_recent_threads_by_age_without_a_project_switcher(cx: &mut TestAppContext) {
    use gpui::px;

    let (view, cx) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.project_options = ["alpha", "beta"]
                .map(|id| ProjectOption {
                    id: ProjectId::parse(id).expect("fixture project"),
                    name: id.into(),
                })
                .to_vec();
            application.selected_project = Some(application.project_options[0].id.clone());
            application.sync_project_pickers(cx);
            push_recent(
                application,
                recent_listing(vec![
                    recent_row("fresh", "alpha", "Fix the sidebar", "owner/editor", HOUR_MS),
                    recent_row("aging", "beta", "Plan the release", "beta", 50 * HOUR_MS),
                    recent_row(
                        "stale",
                        "alpha",
                        "Old idea",
                        "owner/editor",
                        40 * 24 * HOUR_MS,
                    ),
                ]),
                cx,
            );
        });
    });
    cx.simulate_resize(gpui::size(px(1000.0), px(800.0)));
    cx.run_until_parked();

    for removed in [
        "artisan-sidebar-project-trigger",
        "sidebar-previous-project",
        "sidebar-next-project",
    ] {
        assert!(
            cx.debug_bounds(removed).is_none(),
            "{removed} no longer paints"
        );
    }
    let marketplace = cx
        .debug_bounds("artisan-marketplace-navigation")
        .expect("marketplace paints");
    let day = cx
        .debug_bounds("artisan-sidebar-threads-last-24-hours")
        .expect("last day group");
    let days = cx
        .debug_bounds("artisan-sidebar-threads-last-3-days")
        .expect("last three days group");
    let older = cx
        .debug_bounds("artisan-sidebar-threads-older")
        .expect("older group");
    assert!(
        cx.debug_bounds("artisan-sidebar-threads-last-30-days")
            .is_none(),
        "an empty group is hidden"
    );
    assert!(
        marketplace.bottom() < day.top(),
        "threads follow the navigation"
    );
    assert_eq!(days.top() - day.bottom(), px(12.0));
    assert_eq!(older.top() - days.bottom(), px(12.0));
    let header = cx
        .debug_bounds("artisan-sidebar-threads-last-24-hours-header")
        .expect("group header");
    let fresh = cx
        .debug_bounds("artisan-sidebar-thread-fresh")
        .expect("fresh row");
    assert!(header.bottom() <= fresh.top());
    assert_eq!(
        fresh.size.height,
        px(super::super::super::impl_sidebar_threads::SIDEBAR_THREAD_ROW_HEIGHT_PX)
    );
    let title = cx
        .debug_bounds("artisan-sidebar-thread-fresh-title")
        .expect("title line");
    let subtitle = cx
        .debug_bounds("artisan-sidebar-thread-fresh-subtitle")
        .expect("subtitle line");
    assert!(
        title.bottom() <= subtitle.top(),
        "the subtitle sits beneath"
    );
    assert!(subtitle.size.height < title.size.height, "in smaller text");
    assert!(
        cx.debug_bounds("artisan-sidebar-thread-aging-subtitle")
            .is_some()
    );

    cx.simulate_resize(gpui::size(px(1000.0), px(200.0)));
    cx.run_until_parked();
    let sidebar = cx
        .debug_bounds(DESKTOP_SIDEBAR_SELECTOR)
        .expect("short sidebar paints");
    let profile = cx
        .debug_bounds("artisan-desktop-profile-trigger")
        .expect("profile footer paints");
    let navigation = cx
        .debug_bounds("artisan-sidebar-navigation-scroll")
        .expect("navigation scroll area paints");
    assert!(profile.top() >= sidebar.top());
    assert_eq!(profile.bottom(), sidebar.bottom() - px(10.0));
    assert!(navigation.bottom() < profile.top());
}

#[gpui::test]
fn choosing_a_recent_thread_opens_it_in_its_own_project(cx: &mut TestAppContext) {
    use gpui::px;

    let (view, cx) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let (sink, commands) = command_sink([]);
    let here = ProjectId::parse("here").expect("fixture project");
    let there = ProjectId::parse("there").expect("fixture project");
    let listed = ThreadId::parse("listed-here").expect("fixture thread");
    let target = ThreadId::parse("over-there").expect("fixture thread");
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            application.project_options = [&here, &there]
                .map(|id| ProjectOption {
                    id: id.clone(),
                    name: id.as_str().to_owned().into(),
                })
                .to_vec();
            application.selected_project = Some(here.clone());
            application.thread_listing = Some(
                ThreadListing::new(vec![thread(listed.as_str(), "here", "Listed")])
                    .expect("listing"),
            );
            push_recent(
                application,
                recent_listing(vec![
                    recent_row(listed.as_str(), "here", "Listed", "owner/here", 60_000),
                    recent_row(
                        target.as_str(),
                        "there",
                        "Elsewhere",
                        "owner/there",
                        HOUR_MS,
                    ),
                ]),
                cx,
            );
        });
    });
    cx.simulate_resize(gpui::size(px(1000.0), px(800.0)));
    cx.run_until_parked();

    // A thread of the selected project opens directly.
    let row = cx
        .debug_bounds("artisan-sidebar-thread-listed-here")
        .expect("row of the selected project");
    cx.simulate_click(row.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        assert_eq!(
            view.read(app).route(),
            &NativeRoute::Thread {
                project: here.clone(),
                thread: listed.clone(),
            }
        );
    });

    // A thread of another project opens that project on it.
    let row = cx
        .debug_bounds("artisan-sidebar-thread-over-there")
        .expect("row of another project");
    cx.simulate_click(row.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            assert_eq!(application.selected_project.as_ref(), Some(&there));
            assert!(commands.borrow().iter().any(|command| matches!(
                command,
                NativeTransportCommand::SelectProject(project) if project == &there
            )));
            let threads = ThreadListing::new(vec![
                thread("first-there", "there", "First"),
                thread(target.as_str(), "there", "Elsewhere"),
            ])
            .expect("listing");
            application.handle_threads(&there, &threads, cx);
            let opened = application
                .thread_switch_flight
                .as_ref()
                .and_then(|flight| flight.target_thread.clone())
                .or_else(|| application.pending_thread.clone())
                .or_else(|| application.selected_thread.clone());
            assert_eq!(opened, Some(target.clone()), "the chosen thread opens");
        });
    });
}

#[gpui::test]
fn pushed_recent_threads_refresh_the_selected_listing_once_without_polling(
    cx: &mut TestAppContext,
) {
    let (view, cx) = cx.add_window_view(|window, view_cx| test_application(window, view_cx));
    let (sink, commands) = command_sink([]);
    let project = ProjectId::parse("selected").expect("fixture project");
    let refreshes = |commands: &[NativeTransportCommand]| {
        commands
            .iter()
            .filter(|command| matches!(command, NativeTransportCommand::RefreshThreads { .. }))
            .count()
    };
    cx.update(|_, app| {
        view.update(app, |application, cx| {
            application.test_command_sink = Some(sink);
            application.selected_project = Some(project.clone());
            application.thread_listing = Some(ThreadListing::new(Vec::new()).expect("listing"));
            for _ in 0..3 {
                application.refresh_project_threads_if_stale();
            }
            assert!(commands.borrow().is_empty(), "nothing is read on its own");

            // Another project's thread leaves the selected listing alone.
            push_recent(
                application,
                recent_listing(vec![recent_row("elsewhere", "other", "Other", "other", 1)]),
                cx,
            );
            assert_eq!(refreshes(&commands.borrow()), 0);

            let pushed = recent_listing(vec![
                recent_row("new", "selected", "New", "owner/repo", 1),
                recent_row("elsewhere", "other", "Other", "other", 1),
            ]);
            push_recent(application, pushed.clone(), cx);
            application.refresh_project_threads_if_stale();
            assert_eq!(refreshes(&commands.borrow()), 1, "one read per change");
            let (pending, generation) = application
                .sidebar_threads
                .pending
                .clone()
                .expect("refresh in flight");
            assert_eq!(pending, project);
            let refreshed =
                ThreadListing::new(vec![pushed.threads()[0].thread.clone()]).expect("listing");
            application.handle_service_event(
                NativeTransportEvent::ThreadsRefreshed {
                    project_id: project.clone(),
                    generation,
                    result: Ok(refreshed.clone()),
                },
                cx,
            );
            assert_eq!(application.thread_listing, Some(refreshed));
            application.refresh_project_threads_if_stale();
            push_recent(application, pushed, cx);
            assert_eq!(
                refreshes(&commands.borrow()),
                1,
                "an unchanged push reads nothing"
            );

            commands.borrow_mut().clear();
            application.handle_service_event(NativeTransportEvent::Reconnected, cx);
            assert!(
                commands
                    .borrow()
                    .iter()
                    .any(|command| matches!(command, NativeTransportCommand::ReadRecentThreads)),
                "a reconnected connection reads the recent threads again"
            );
        });
    });
}

/// Paints the new-task project picker's inline trigger at the bottom of a
/// window, so its menu opens above it as on the home surface.
struct HomePickerHost {
    picker: gpui::Entity<HomeProjectPickerView>,
}

impl gpui::Render for HomePickerHost {
    fn render(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> impl gpui::IntoElement {
        let trigger = self.picker.update(cx, |picker, cx| {
            picker.render_inline_trigger("alpha", window, cx)
        });
        gpui::div()
            .size_full()
            .flex()
            .flex_col()
            .justify_end()
            .child(trigger)
    }
}

#[gpui::test]
fn project_menu_shares_sliding_hover_with_keyboard_navigation(cx: &mut TestAppContext) {
    use crate::home_project_picker::{HOME_MENU_SELECTOR, HOME_TRIGGER_SELECTOR};

    cx.update(|app| app.set_reduce_motion(true));
    let options = ["alpha", "beta"]
        .map(|name| ProjectOption {
            id: ProjectId::parse(format!("hover-{name}")).expect("fixture project"),
            name: name.into(),
        })
        .to_vec();
    let current = Some(options[0].id.clone());
    let (host, cx) = cx.add_window_view(|_, cx| HomePickerHost {
        picker: cx.new(|cx| {
            HomeProjectPickerView::new(
                options,
                current,
                artisan_ui::theme::DesktopTheme::neutral_dark(),
                cx,
            )
        }),
    });
    let picker = cx.update(|_, app| host.read(app).picker.clone());
    cx.simulate_resize(gpui::size(gpui::px(800.0), gpui::px(600.0)));
    cx.run_until_parked();
    let trigger = cx.debug_bounds(HOME_TRIGGER_SELECTOR).unwrap();
    cx.simulate_click(trigger.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let picker = picker.read(app);
        assert_eq!(
            picker.state().highlighted_row(),
            Some(PickerRow::Project(0))
        );
        assert_eq!(
            picker.menu_hover_state().active_id(),
            Some("project:hover-alpha")
        );
    });

    let alpha = cx.debug_bounds("artisan-home-project-row-0").unwrap();
    let beta = cx.debug_bounds("artisan-home-project-row-1").unwrap();
    let new_project = cx.debug_bounds("artisan-home-project-row-new").unwrap();
    cx.update(|_, app| app.set_reduce_motion(false));
    cx.simulate_mouse_move(alpha.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.simulate_mouse_move(beta.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let picker = picker.read(app);
        let hover = picker.menu_hover_state();
        assert_eq!(
            picker.state().highlighted_row(),
            Some(PickerRow::Project(1))
        );
        assert_eq!(hover.active_id(), Some("project:hover-beta"));
        let transition = hover
            .transition()
            .expect("moving between rows slides the same pill");
        assert!(transition.to.top > transition.from.top);
    });

    cx.simulate_mouse_move(new_project.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let picker = picker.read(app);
        assert_eq!(
            picker.state().highlighted_row(),
            Some(PickerRow::NewProject)
        );
        assert_eq!(picker.menu_hover_state().active_id(), Some("new-project"));
    });

    // Keyboard navigation takes over while the pointer stays on New project.
    for (key, row, hover_id) in [
        ("up", PickerRow::Project(1), "project:hover-beta"),
        ("up", PickerRow::Project(0), "project:hover-alpha"),
        ("down", PickerRow::Project(1), "project:hover-beta"),
    ] {
        cx.simulate_keystrokes(key);
        cx.run_until_parked();
        cx.update(|_, app| {
            let picker = picker.read(app);
            let hover = picker.menu_hover_state();
            assert_eq!(picker.state().highlighted_row(), Some(row));
            assert_eq!(hover.active_id(), Some(hover_id));
            assert!(hover.visible());
            assert!(
                hover.transition().is_none(),
                "keyboard highlighting is immediate"
            );
        });
    }

    cx.simulate_keystrokes("escape");
    cx.run_until_parked();
    assert!(cx.debug_bounds(HOME_MENU_SELECTOR).is_none());
    cx.update(|_, app| {
        let picker = picker.read(app);
        let hover = picker.menu_hover_state();
        assert!(!picker.state().is_open());
        assert!(!hover.visible());
        assert_eq!(hover.active_id(), None);
        assert!(hover.transition().is_none());
    });
}
