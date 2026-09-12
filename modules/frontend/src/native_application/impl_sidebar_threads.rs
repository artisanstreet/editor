//! Flat sidebar thread groups and navigation-independent catalog refreshes.

use super::*;
use artisan_domain::ThreadSummary;
use gpui::ColorExt as _;
use std::time::Instant;

#[derive(Default)]
pub(super) struct SidebarThreadsState {
    focus: HashMap<ThreadId, FocusHandle>,
    hover: Rc<RefCell<SlidingHoverState>>,
    bounds: Rc<RefCell<Option<Bounds<gpui::Pixels>>>>,
    generation: u64,
    pending: Option<(ProjectId, u64)>,
    next_read: Option<Instant>,
    selection: SidebarSelectionFade,
    selection_frame_pending: bool,
}

/// Two sequential halves of the transitions-dev icon-swap duration.
#[derive(Default)]
struct SidebarSelectionFade {
    target: Option<ThreadId>,
    origins: HashMap<ThreadId, f32>,
    started: Option<Instant>,
}

impl SidebarSelectionFade {
    fn weight(&self, id: &ThreadId, now: Instant) -> f32 {
        let Some(started) = self.started else {
            return if self.target.as_ref() == Some(id) {
                1.0
            } else {
                0.0
            };
        };
        let elapsed = now.saturating_duration_since(started).as_secs_f32();
        let half = selection_duration().as_secs_f32() / 2.0;
        if elapsed < half {
            self.origins.get(id).copied().unwrap_or(0.0) * (1.0 - fade_curve(elapsed / half))
        } else if self.target.as_ref() == Some(id) {
            fade_curve(((elapsed - half) / half).min(1.0))
        } else {
            0.0
        }
    }

    fn select(&mut self, target: Option<ThreadId>, ids: &[ThreadSummary], reduced: bool) -> bool {
        let now = Instant::now();
        if self.target != target {
            self.origins = ids
                .iter()
                .map(|row| (row.thread_id.clone(), self.weight(&row.thread_id, now)))
                .collect();
            let initial = self.target.is_none() && self.started.is_none();
            self.target = target;
            self.started = if initial || reduced { None } else { Some(now) };
        }
        if reduced {
            self.started = None;
        }
        self.started
            .is_some_and(|at| now.saturating_duration_since(at) < selection_duration())
    }
}

fn selection_duration() -> Duration {
    use artisan_ui::motion::{MotionPlan, MotionPolicy, MotionRecipe};
    match MotionPolicy::Full.resolve(MotionRecipe::IconSwap) {
        MotionPlan::Animate(animation) => animation.duration(),
        MotionPlan::Immediate => Duration::ZERO,
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "the easing result is bounded to the unit interval"
)]
fn fade_curve(progress: f32) -> f32 {
    artisan_ui::motion::MotionCurve::EaseInOut.sample(f64::from(progress)) as f32
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

    fn animate_sidebar_selection(&mut self, rows: &[ThreadSummary], cx: &mut Context<Self>) {
        let target = match self.route() {
            NativeRoute::Thread { thread, .. } | NativeRoute::Editor { thread, .. } => {
                Some(thread.clone())
            }
            _ => None,
        };
        if self
            .sidebar_threads
            .selection
            .select(target, rows, cx.reduce_motion())
            && !self.sidebar_threads.selection_frame_pending
        {
            self.sidebar_threads.selection_frame_pending = true;
            cx.spawn(async move |entity, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let _ = entity.update(cx, |app, cx| {
                    app.sidebar_threads.selection_frame_pending = false;
                    cx.notify();
                });
            })
            .detach();
        }
    }

    pub(super) fn desktop_sidebar_threads(&mut self, cx: &mut Context<Self>) -> Stateful<Div> {
        let listing = self
            .thread_listing
            .clone()
            .unwrap_or_else(empty_thread_listing);
        self.animate_sidebar_selection(listing.threads(), cx);
        self.sidebar_threads.focus.retain(|id, _| {
            listing
                .threads()
                .iter()
                .any(|thread| &thread.thread_id == id)
        });
        let hover_ids = listing
            .threads()
            .iter()
            .map(|thread| thread.thread_id.as_str().to_owned())
            .collect::<Vec<_>>();
        self.sidebar_threads
            .hover
            .borrow_mut()
            .clear_if_missing(&hover_ids);
        let bounds_state = Rc::clone(&self.sidebar_threads.bounds);
        let probe = canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                if *bounds_state.borrow() != Some(bounds) {
                    *bounds_state.borrow_mut() = Some(bounds);
                    window.defer(cx, |window, _| window.refresh());
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let (working, settled) = thread_groups(&listing);
        let show_separator = !working.is_empty() && !settled.is_empty();
        let mut groups = div()
            .id("artisan-sidebar-threads")
            .relative()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .debug_selector(|| "artisan-sidebar-threads".to_owned())
            .on_hover(cx.listener(|app, hovered: &bool, _, cx| {
                if *hovered {
                    app.sidebar_hover.borrow_mut().hide();
                } else {
                    app.sidebar_threads.hover.borrow_mut().clear();
                }
                cx.notify();
            }))
            .child(probe)
            .child(render_picker_hover_pill(
                &self.theme,
                &self.sidebar_threads.hover,
                "sidebar-threads",
                px(6.0),
                cx.reduce_motion(),
            ));
        for (name, rows) in [("working", working), ("settled", settled)] {
            if rows.is_empty() {
                continue;
            }
            if name == "settled" && show_separator {
                groups = groups.child(
                    div()
                        .h(px(1.0))
                        .flex_none()
                        .mx(px(12.0))
                        .bg(self.theme.colors.border.to_paint())
                        .debug_selector(|| "artisan-sidebar-thread-group-separator".to_owned()),
                );
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
        let weight = self
            .sidebar_threads
            .selection
            .weight(&thread.thread_id, Instant::now());
        let glyph_color = self
            .desktop_theme
            .secondary
            .blend(&self.desktop_theme.foreground.opacity(weight));
        let title = self
            .listed_thread_display_title(&thread.thread_id, cx)
            .unwrap_or_else(|| thread.title.as_str().to_owned());
        let selector = format!("artisan-sidebar-thread-{}", thread.thread_id.as_str());
        let click_thread = thread.thread_id.clone();
        let key_thread = thread.thread_id.clone();
        let color = self.theme.colors.muted.to_paint();
        let hover_id = thread.thread_id.as_str().to_owned();
        div()
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
            .relative()
            .on_hover(cx.listener(move |app, hovered: &bool, _, cx| {
                let mut hover = app.sidebar_threads.hover.borrow_mut();
                if *hovered {
                    hover.set_active(hover_id.clone());
                } else if hover.active_id() == Some(hover_id.as_str()) {
                    hover.hide();
                }
                cx.notify();
            }))
            .child(sidebar_hover_probe(
                Rc::clone(&self.sidebar_threads.hover),
                Rc::clone(&self.sidebar_threads.bounds),
                thread.thread_id.as_str().to_owned(),
            ))
            .focus_visible(move |style| style.bg(color))
            .debug_selector(move || selector.clone())
            .child(
                desktop_nav_glyph(
                    if thread.has_active_work {
                        AssetId::TABLER_LOADER_2
                    } else {
                        AssetId::TABLER_MESSAGE_CIRCLE
                    },
                    self.desktop_theme,
                )
                .text_color(glyph_color),
            )
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
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use artisan_domain::{ThreadTitle, UnixMillis};

    #[test]
    fn selected_foreground_fades_out_before_next_fades_in() {
        let old = ThreadId::parse("old").unwrap();
        let next = ThreadId::parse("next").unwrap();
        let start = Instant::now();
        let fade = SidebarSelectionFade {
            target: Some(next.clone()),
            origins: HashMap::from([(old.clone(), 1.0)]),
            started: Some(start),
        };
        assert!(fade.weight(&old, start + Duration::from_millis(60)) > 0.0);
        assert!(fade.weight(&next, start + Duration::from_millis(60)).abs() < f32::EPSILON);
        assert!(fade.weight(&old, start + Duration::from_millis(125)).abs() < f32::EPSILON);
        assert!(fade.weight(&next, start + Duration::from_millis(180)) > 0.0);
        assert!(
            (fade.weight(&next, start + Duration::from_millis(250)) - 1.0).abs() < f32::EPSILON
        );
    }

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
    fn sidebar_groups_have_an_inset_separator_and_rows_open_threads(cx: &mut gpui::TestAppContext) {
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
        let separator = cx
            .debug_bounds("artisan-sidebar-thread-group-separator")
            .expect("separator between populated groups");
        assert_eq!(separator.size.height, px(1.0));
        assert_eq!(separator.top() - working.bottom(), px(8.0));
        assert_eq!(settled.top() - separator.bottom(), px(8.0));
        assert_eq!(separator.left() - working.left(), px(12.0));
        assert_eq!(working.right() - separator.right(), px(12.0));
        let row = cx
            .debug_bounds("artisan-sidebar-thread-idle-thread")
            .expect("thread row");
        cx.simulate_mouse_move(row.center(), None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, app| {
            let hover = view.read(app).sidebar_threads.hover.borrow();
            assert_eq!(hover.active_id(), Some("idle-thread"));
            assert!(hover.visible());
        });
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
