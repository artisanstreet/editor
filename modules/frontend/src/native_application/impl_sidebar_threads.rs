//! The sidebar's recent threads across every project, grouped by age.
//!
//! Rows are the Forge's recent-threads listing (`impl_recent_threads.rs`):
//! the title, and beneath it the repository or project the thread works
//! in. The Editor only sorts them into age groups
//! ([`crate::recent_thread_groups`]) and regroups them when a row crosses an
//! age boundary. Choosing a row opens the thread in its own project.

use super::*;
use crate::recent_thread_groups::{RecentThreadGroup, group_recent_threads, next_regrouping};
use artisan_domain::{RecentThread, RecentThreadListing, UnixMillis};
use gpui::ColorExt as _;
use gpui::prelude::FluentBuilder as _;
use std::time::Instant;

/// Height of one two-line thread row.
pub(super) const SIDEBAR_THREAD_ROW_HEIGHT_PX: f32 = 48.0;

/// Spoken after a working row's subtitle.
pub(super) const SIDEBAR_WORKING_LABEL: &str = "working";

#[derive(Default)]
pub(super) struct SidebarThreadsState {
    focus: HashMap<ThreadId, FocusHandle>,
    hover: Rc<RefCell<SlidingHoverState>>,
    bounds: Rc<RefCell<Option<Bounds<gpui::Pixels>>>>,
    selection: SidebarSelectionFade,
    selection_frame_pending: bool,
    /// The recent threads the Forge served or last pushed.
    pub(super) recent: Option<RecentThreadListing>,
    /// Advances with every new recent-threads list.
    pub(super) recent_revision: u64,
    /// The list the selected project's listing was last checked against.
    pub(super) refreshed_revision: u64,
    /// The instant the next row changes age group, and the repaint waiting
    /// for it.
    regroup: Option<(UnixMillis, Task<()>)>,
    /// Background reads of the selected project's listing.
    pub(super) generation: u64,
    pub(super) pending: Option<(ProjectId, u64)>,
    /// A recent thread of the selected project to open once its listing
    /// names it.
    pub(super) open_after_refresh: Option<ThreadId>,
    /// A chosen recent thread waiting for its project or a settled view.
    pub(super) awaited_open: Option<super::impl_recent_threads::AwaitedOpen>,
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

    fn select(&mut self, target: Option<ThreadId>, ids: &[ThreadId], reduced: bool) -> bool {
        let now = Instant::now();
        if self.target != target {
            self.origins = ids
                .iter()
                .map(|id| (id.clone(), self.weight(id, now)))
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

/// The wall clock the age groups are measured against.
pub(super) fn wall_clock() -> UnixMillis {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        });
    UnixMillis::from_millis(millis)
}

impl NativeApplication {
    fn animate_sidebar_selection(
        &mut self,
        ids: &[ThreadId],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = match self.route() {
            NativeRoute::Thread { thread, .. } | NativeRoute::Editor { thread, .. } => {
                Some(thread.clone())
            }
            _ => None,
        };
        if self
            .sidebar_threads
            .selection
            .select(target, ids, cx.reduce_motion())
            && !self.sidebar_threads.selection_frame_pending
        {
            self.sidebar_threads.selection_frame_pending = true;
            cx.on_next_frame(window, |app, _, cx| {
                app.sidebar_threads.selection_frame_pending = false;
                cx.notify();
            });
        }
    }

    /// Repaints when the next row changes age group, so rows move between
    /// groups as time passes without anything else changing.
    fn schedule_regroup(
        &mut self,
        listing: &RecentThreadListing,
        now: UnixMillis,
        cx: &mut Context<Self>,
    ) {
        let next = next_regrouping(listing, now);
        if self.sidebar_threads.regroup.as_ref().map(|(at, _)| *at) == next {
            return;
        }
        self.sidebar_threads.regroup = next.map(|at| {
            let delay = u64::try_from(at.as_millis().saturating_sub(now.as_millis()))
                .map_or(Duration::ZERO, Duration::from_millis);
            let task = cx.spawn(async move |app, cx| {
                cx.background_executor().timer(delay).await;
                let _ = app.update(cx, |app, cx| {
                    app.sidebar_threads.regroup = None;
                    cx.notify();
                });
            });
            (at, task)
        });
    }

    pub(super) fn desktop_sidebar_threads(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let listing = self.sidebar_threads.recent.clone().unwrap_or_default();
        let now = wall_clock();
        self.schedule_regroup(&listing, now, cx);
        let ids = listing
            .threads()
            .iter()
            .map(|row| row.thread.thread_id.clone())
            .collect::<Vec<_>>();
        self.animate_sidebar_selection(&ids, window, cx);
        self.sidebar_threads.focus.retain(|id, _| ids.contains(id));
        let hover_ids = ids
            .iter()
            .map(|id| id.as_str().to_owned())
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
        let mut container = div()
            .id("artisan-sidebar-threads")
            .relative()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(self.theme.spacing.steps(3.0))
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
        for group in &group_recent_threads(&listing, now) {
            container = container.child(self.desktop_sidebar_thread_group(group, cx));
        }
        container
    }

    fn desktop_sidebar_thread_group(
        &mut self,
        group: &RecentThreadGroup<'_>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let selector = format!("artisan-sidebar-threads-{}", group.age.id());
        let header_selector = format!("{selector}-header");
        let mut rows = div()
            .id(SharedString::from(selector.clone()))
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .debug_selector(move || selector.clone())
            .child(
                div()
                    .w_full()
                    .px(px(8.0))
                    .pb(self.theme.spacing.steps(1.0))
                    .text_size(self.theme.typography.label_text)
                    .text_color(self.theme.colors.muted_foreground.to_paint())
                    .truncate()
                    .debug_selector(move || header_selector.clone())
                    .child(group.age.label()),
            );
        for thread in &group.threads {
            rows = rows.child(self.desktop_sidebar_thread(thread, cx));
        }
        rows
    }

    fn desktop_sidebar_thread(
        &mut self,
        row: &RecentThread,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let thread = &row.thread;
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
            .foreground
            .blend(&self.desktop_theme.secondary.opacity(weight));
        let title = SharedString::from(thread.title.as_str().to_owned());
        let subtitle = SharedString::from(row.subtitle.as_str().to_owned());
        let description = if thread.has_active_work {
            SharedString::from(format!("{subtitle}, {SIDEBAR_WORKING_LABEL}"))
        } else {
            subtitle.clone()
        };
        let selector = format!("artisan-sidebar-thread-{}", thread.thread_id.as_str());
        let title_selector = format!("{selector}-title");
        let subtitle_selector = format!("{selector}-subtitle");
        let working_selector = format!("{selector}-working");
        let working_tone = self.theme.colors.primary.to_paint();
        let click_target = (thread.project_id.clone(), thread.thread_id.clone());
        let key_target = click_target.clone();
        let color = self.theme.colors.muted.to_paint();
        let hover_id = thread.thread_id.as_str().to_owned();
        div()
            .id(SharedString::from(selector.clone()))
            .track_focus(&focus)
            .tab_index(0)
            .role(gpui::Role::Button)
            .aria_label(title.clone())
            .aria_description(description)
            .w_full()
            .h(px(SIDEBAR_THREAD_ROW_HEIGHT_PX))
            .flex_none()
            .min_w(px(0.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(8.0))
            .rounded(px(6.0))
            .cursor_pointer()
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
                desktop_nav_glyph(AssetId::TABLER_MESSAGE_CIRCLE, self.desktop_theme)
                    .text_color(glyph_color),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .w_full()
                            .truncate()
                            .text_size(self.theme.typography.control_text)
                            .text_color(self.desktop_theme.foreground)
                            .debug_selector(move || title_selector.clone())
                            .child(title),
                    )
                    .child(
                        div()
                            .w_full()
                            .truncate()
                            .text_size(self.theme.typography.label_text)
                            .text_color(self.theme.colors.muted_foreground.to_paint())
                            .debug_selector(move || subtitle_selector.clone())
                            .child(subtitle),
                    ),
            )
            // Live work keeps its prominence without reordering the
            // chronological groups: the rail's trailing state dot.
            .when(thread.has_active_work, |row| {
                row.child(
                    div()
                        .size(px(crate::shell::LEGACY_RAIL_STATE_DOT_PX))
                        .flex_shrink_0()
                        .rounded_full()
                        .bg(working_tone)
                        .debug_selector(move || working_selector.clone()),
                )
            })
            .on_click(cx.listener(move |app, _, window, cx| {
                window.focus(&focus, cx);
                let (project, thread) = click_target.clone();
                app.open_recent_thread(project, thread, cx);
            }))
            .on_key_down(cx.listener(move |app, event: &gpui::KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    cx.stop_propagation();
                    let (project, thread) = key_target.clone();
                    app.open_recent_thread(project, thread, cx);
                }
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
