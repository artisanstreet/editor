//! Flat sidebar thread groups and navigation-independent catalog refreshes.

use super::*;
use artisan_domain::ThreadSummary;
use std::time::Instant;

#[derive(Default)]
pub(super) struct SidebarThreadsState {
    focus: HashMap<ThreadId, FocusHandle>,
    generation: u64,
    pending: Option<(ProjectId, u64)>,
    next_read: Option<Instant>,
}

/// Electron orders each group by the most recent sent message, falling back
/// to creation for empty threads. Stable sorting preserves ties.
fn thread_groups(listing: &ThreadListing) -> (Vec<ThreadSummary>, Vec<ThreadSummary>) {
    let mut rows = listing.threads().to_vec();
    rows.sort_by_key(|thread| {
        std::cmp::Reverse(
            thread
                .last_message_at
                .unwrap_or(thread.created_at)
                .as_millis(),
        )
    });
    rows.into_iter().partition(|thread| thread.has_active_work)
}

impl NativeApplication {
    pub(super) fn refresh_sidebar_threads(&mut self) {
        if self.service_stopped
            || self.shutdown_prepared
            || self.sidebar_collapsed
            || self.thread_switch_flight.is_some()
            || self.intake_stage.is_some()
            || self.thread_listing.is_none()
        {
            return;
        }
        let Some(project_id) = self.selected_project.clone() else {
            return;
        };
        if self.sidebar_threads.pending.is_some()
            || self
                .sidebar_threads
                .next_read
                .is_some_and(|at| Instant::now() < at)
        {
            return;
        }
        let Some(generation) = self.sidebar_threads.generation.checked_add(1) else {
            return;
        };
        self.sidebar_threads.generation = generation;
        self.sidebar_threads.next_read = Some(Instant::now() + Duration::from_millis(1500));
        if self
            .submit_command(NativeTransportCommand::ReadSidebarThreads {
                project_id: project_id.clone(),
                generation,
            })
            .is_ok()
        {
            self.sidebar_threads.pending = Some((project_id, generation));
        }
    }

    pub(super) fn receive_sidebar_threads(
        &mut self,
        project_id: &ProjectId,
        generation: u64,
        result: Result<ThreadListing, ServiceFailure>,
        cx: &mut Context<Self>,
    ) {
        if self.sidebar_threads.pending.as_ref() != Some(&(project_id.clone(), generation)) {
            return;
        }
        self.sidebar_threads.pending = None;
        self.sidebar_threads.next_read = Some(Instant::now() + Duration::from_millis(1500));
        if self.selected_project.as_ref() != Some(project_id)
            || self.thread_switch_flight.is_some()
            || self.intake_stage.is_some()
        {
            return;
        }
        let Ok(listing) = result else {
            return;
        };
        if listing
            .threads()
            .iter()
            .any(|thread| &thread.project_id != project_id)
            || self.thread_listing.as_ref() == Some(&listing)
        {
            return;
        }
        if self.selected_thread.as_ref().is_some_and(|selected| {
            !listing
                .threads()
                .iter()
                .any(|thread| &thread.thread_id == selected)
        }) {
            self.handle_threads(project_id, &listing, cx);
            return;
        }
        // Background reads must not auto-open a thread or disturb an in-flight
        // snapshot, draft, route, or selected conversation.
        self.thread_listing = Some(listing.clone());
        self.update_thread_picker(listing, self.selected_thread.clone(), cx);
        self.sync_command_menu_groups(cx);
        cx.notify();
    }

    pub(super) fn desktop_sidebar_threads(&mut self, cx: &mut Context<Self>) -> Stateful<Div> {
        let listing = self
            .thread_listing
            .clone()
            .unwrap_or_else(empty_thread_listing);
        self.sidebar_threads.focus.retain(|id, _| {
            listing
                .threads()
                .iter()
                .any(|thread| &thread.thread_id == id)
        });
        let (working, settled) = thread_groups(&listing);
        let mut groups = div()
            .id("artisan-sidebar-threads")
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .debug_selector(|| "artisan-sidebar-threads".to_owned())
            .on_hover(cx.listener(|app, hovered: &bool, _, cx| {
                if *hovered {
                    app.sidebar_hover.borrow_mut().hide();
                    cx.notify();
                }
            }));
        for (name, rows) in [("working", working), ("settled", settled)] {
            if rows.is_empty() {
                continue;
            }
            let selector = format!("artisan-sidebar-threads-{name}");
            let mut group = div()
                .id(SharedString::from(selector.clone()))
                .w_full()
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .debug_selector(move || selector.clone());
            for thread in rows {
                group = group.child(self.desktop_sidebar_thread(&thread, cx));
            }
            groups = groups.child(group);
        }
        groups
    }

    fn desktop_sidebar_thread(
        &mut self,
        thread: &ThreadSummary,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let focus = self
            .sidebar_threads
            .focus
            .entry(thread.thread_id.clone())
            .or_insert_with(|| cx.focus_handle())
            .clone();
        let selected = matches!(self.route(), NativeRoute::Thread { thread: id, .. }
            | NativeRoute::Editor { thread: id, .. } if id == &thread.thread_id);
        let title = self
            .listed_thread_display_title(&thread.thread_id, cx)
            .unwrap_or_else(|| thread.title.as_str().to_owned());
        let selector = format!("artisan-sidebar-thread-{}", thread.thread_id.as_str());
        let click_thread = thread.thread_id.clone();
        let key_thread = thread.thread_id.clone();
        let color = self.theme.colors.muted.to_paint();
        let mut row = div()
            .id(SharedString::from(selector.clone()))
            .track_focus(&focus)
            .tab_index(0)
            .w_full()
            .h(px(34.0))
            .flex_none()
            .min_w(px(0.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(8.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .text_size(px(14.0))
            .text_color(self.desktop_theme.foreground)
            .hover(move |style| style.bg(color))
            .focus_visible(move |style| style.bg(color))
            .debug_selector(move || selector.clone())
            .child(desktop_nav_glyph(
                if thread.has_active_work {
                    AssetId::TABLER_LOADER_2
                } else {
                    AssetId::TABLER_MESSAGE_CIRCLE
                },
                self.desktop_theme,
            ))
            .child(div().flex_1().min_w(px(0.0)).truncate().child(title))
            .on_click(cx.listener(move |app, _, window, cx| {
                window.focus(&focus, cx);
                app.open_thread_from_sidebar(click_thread.clone(), cx);
            }))
            .on_key_down(cx.listener(move |app, event: &gpui::KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    cx.stop_propagation();
                    app.open_thread_from_sidebar(key_thread.clone(), cx);
                }
            }));
        if selected {
            row = row.bg(color);
        }
        row
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use artisan_domain::{ThreadTitle, UnixMillis};

    #[test]
    fn sidebar_groups_work_first_and_sort_by_sent_message_not_projection_updates() {
        let make = |id: &str, working, created, message: Option<i64>, updated| ThreadSummary {
            thread_id: ThreadId::parse(id).unwrap(),
            project_id: ProjectId::parse("project").unwrap(),
            title: ThreadTitle::parse(id).unwrap(),
            has_active_work: working,
            last_message_at: message.map(UnixMillis::from_millis),
            created_at: UnixMillis::from_millis(created),
            updated_at: UnixMillis::from_millis(updated),
        };
        let listing = ThreadListing::new(vec![
            make("old", false, 1, Some(2), 999),
            make("working-old", true, 1, Some(3), 900),
            make("empty", false, 6, None, 6),
            make("working-new", true, 1, Some(5), 5),
            make("recent", false, 1, Some(8), 8),
        ])
        .unwrap();
        let (working, settled) = thread_groups(&listing);
        let ids = |rows: Vec<ThreadSummary>| {
            rows.into_iter()
                .map(|row| row.thread_id.as_str().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(working), ["working-new", "working-old"]);
        assert_eq!(ids(settled), ["recent", "empty", "old"]);
        let mut rows = listing.threads().to_vec();
        rows[3].has_active_work = false;
        let (working, settled) = thread_groups(&ThreadListing::new(rows).unwrap());
        assert_eq!(ids(working), ["working-old"]);
        assert_eq!(ids(settled), ["recent", "empty", "working-new", "old"]);
    }
    #[gpui::test]
    fn sidebar_rows_have_a_gap_and_open_the_thread(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let project = ProjectId::parse("sidebar-project").unwrap();
        let active = ThreadId::parse("active-thread").unwrap();
        let idle = ThreadId::parse("idle-thread").unwrap();
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                application.selected_project = Some(project.clone());
                application.selected_thread = Some(idle.clone());
                application.thread_listing = Some(
                    ThreadListing::new(vec![
                        ThreadSummary {
                            thread_id: active.clone(),
                            project_id: project.clone(),
                            title: ThreadTitle::parse("Active thread").unwrap(),
                            has_active_work: true,
                            last_message_at: None,
                            created_at: UnixMillis::EPOCH,
                            updated_at: UnixMillis::EPOCH,
                        },
                        ThreadSummary {
                            thread_id: idle.clone(),
                            project_id: project.clone(),
                            title: ThreadTitle::parse("Idle thread").unwrap(),
                            has_active_work: false,
                            last_message_at: None,
                            created_at: UnixMillis::EPOCH,
                            updated_at: UnixMillis::EPOCH,
                        },
                    ])
                    .unwrap(),
                );
                cx.notify();
            })
        });
        cx.run_until_parked();
        let working = cx
            .debug_bounds("artisan-sidebar-threads-working")
            .expect("working group");
        let settled = cx
            .debug_bounds("artisan-sidebar-threads-settled")
            .expect("settled group");
        assert_eq!(settled.top() - working.bottom(), px(16.0));
        let row = cx
            .debug_bounds("artisan-sidebar-thread-idle-thread")
            .expect("thread row");
        cx.simulate_click(row.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, app| {
            assert_eq!(
                view.read(app).route(),
                &NativeRoute::Thread {
                    project,
                    thread: idle
                }
            );
        });
    }

    #[gpui::test]
    fn sidebar_refresh_preserves_draft_route_and_rejects_stale_project(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let project = ProjectId::parse("sidebar-project").unwrap();
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                application.selected_project = Some(project.clone());
                let route = application.route().clone();
                let rows = ThreadListing::new(vec![ThreadSummary {
                    thread_id: ThreadId::parse("background-thread").unwrap(),
                    project_id: project.clone(),
                    title: ThreadTitle::parse("Background work").unwrap(),
                    has_active_work: true,
                    last_message_at: None,
                    created_at: UnixMillis::EPOCH,
                    updated_at: UnixMillis::EPOCH,
                }])
                .unwrap();
                application.sidebar_threads.pending = Some((project.clone(), 1));
                application.receive_sidebar_threads(&project, 1, Ok(rows.clone()), cx);
                assert_eq!(application.thread_listing, Some(rows.clone()));
                assert_eq!(application.route(), &route);
                assert!(application.selected_thread.is_none());
                assert!(application.pending_thread.is_none());
                application.sidebar_threads.pending = Some((project.clone(), 2));
                application.receive_sidebar_threads(&project, 1, Ok(empty_thread_listing()), cx);
                assert_eq!(application.thread_listing, Some(rows.clone()));
                application.selected_project = Some(ProjectId::parse("other-project").unwrap());
                application.receive_sidebar_threads(&project, 2, Ok(empty_thread_listing()), cx);
                assert_eq!(application.thread_listing, Some(rows));
                assert!(application.sidebar_threads.pending.is_none());
            })
        });
    }
}
