//! The sidebar's recent threads across every project, as an inbox.
//!
//! Rows are the Forge's recent-threads listing (`impl_recent_threads.rs`):
//! the title, and beneath it the repository or project the thread works
//! in. Only threads with something unseen show outright; running threads
//! collapse into `Working` and read idle ones into `History`, which keeps
//! its age groups inside ([`crate::recent_thread_groups`]). Rows regroup
//! when one crosses an age boundary. Choosing a row opens the thread in its
//! own project.

use super::*;
use crate::desktop_shell::{DESKTOP_COLUMN_ROW_INSET_PX, desktop_section_label};
use crate::recent_thread_groups::{RecentThreadGroup, next_regrouping, section_recent_threads};
use artisan_domain::{
    RecentThread, RecentThreadListing, ThreadAttention, ThreadSummary, UnixMillis,
};
use gpui::prelude::FluentBuilder as _;
use gpui::{Animation, AnimationExt as _};
use std::time::Instant;

/// Height of one two-line thread row.
pub(super) const SIDEBAR_THREAD_ROW_HEIGHT_PX: f32 = 48.0;

/// Spoken after a working row's subtitle.
pub(super) const SIDEBAR_WORKING_LABEL: &str = "working";

/// One full fade of the working dot, out and back.
const SIDEBAR_WORKING_PULSE: Duration = Duration::from_millis(1_600);

/// The dimmest the working dot gets mid-pulse.
const SIDEBAR_WORKING_PULSE_FLOOR: f32 = 0.4;

/// A collapsible sidebar group.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum SidebarGroup {
    /// Threads the Forge is running with nothing waiting on the reader.
    Working,
    /// Read, idle threads, in age groups.
    History,
}

impl SidebarGroup {
    const fn label(self) -> &'static str {
        match self {
            Self::Working => "Working",
            Self::History => "History",
        }
    }

    const fn id(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::History => "history",
        }
    }
}

/// One collapsible group's disclosure: collapsed until the reader opens it.
#[derive(Default)]
struct SidebarGroupDisclosure {
    open: bool,
    /// When the reader last toggled it, for the chevron flip and reveal.
    toggled: Option<Instant>,
    focus: Option<FocusHandle>,
}

impl SidebarGroupDisclosure {
    /// Eased progress of the latest toggle, `None` once settled.
    fn progress(&self, now: Instant, reduced: bool) -> Option<f32> {
        if reduced {
            return None;
        }
        let elapsed = now.saturating_duration_since(self.toggled?);
        let duration = MotionDuration::Fast.as_duration();
        (elapsed < duration).then(|| {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the shared easing curve samples in f64 and feeds f32 rotation and opacity; the narrowing is the intended precision"
            )]
            let eased = MotionCurve::SmoothOut
                .sample(elapsed.as_secs_f64() / duration.as_secs_f64())
                as f32;
            eased
        })
    }
}

/// What a row's trailing dot says about its thread: whether it needs the
/// reader, never decoration. At most one state shows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SidebarThreadIndicator {
    /// An approval or question is open: the run waits on the reader.
    Awaiting,
    /// The Forge owns a live run.
    Working,
    /// The latest run failed or was interrupted and has not been read.
    Failed,
    /// The latest run finished and has not been read.
    Finished,
}

impl SidebarThreadIndicator {
    /// The state a thread's row shows, if any.
    ///
    /// Waiting on the reader outranks the work it blocks. An unread outcome
    /// never shows on the thread the reader has open: they are looking at
    /// it, and leaving it marks it read.
    pub(super) fn of(thread: &ThreadSummary, open: bool) -> Option<Self> {
        match thread.attention {
            ThreadAttention::AwaitingAnswer => Some(Self::Awaiting),
            _ if thread.has_active_work => Some(Self::Working),
            ThreadAttention::Failed if !open => Some(Self::Failed),
            ThreadAttention::Finished if !open => Some(Self::Finished),
            ThreadAttention::None | ThreadAttention::Failed | ThreadAttention::Finished => None,
        }
    }

    /// Spoken after the row's subtitle.
    const fn label(self) -> &'static str {
        match self {
            Self::Awaiting => "waiting for your answer",
            Self::Working => SIDEBAR_WORKING_LABEL,
            Self::Failed => "failed",
            Self::Finished => "finished",
        }
    }

    /// Suffix of the dot's selector.
    const fn id(self) -> &'static str {
        match self {
            Self::Awaiting => "awaiting",
            Self::Working => "working",
            Self::Failed => "failed",
            Self::Finished => "finished",
        }
    }
}

#[derive(Default)]
pub(super) struct SidebarThreadsState {
    focus: HashMap<ThreadId, FocusHandle>,
    project_icons: super::project_icon::ProjectIconCache,
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
    /// The `Working` and `History` disclosures.
    groups: HashMap<SidebarGroup, SidebarGroupDisclosure>,
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

/// Mixes `from` toward `to` by `weight` in `[0, 1]`, channel by channel.
///
/// GPUI's `ColorExt::blend` is not this: it scales the base by the other
/// colour's alpha, so a zero weight returns a transparent colour instead of
/// `from`, which made every unselected title vanish.
fn mix_colors(from: gpui::Hsla, to: gpui::Hsla, weight: f32) -> gpui::Hsla {
    let weight = weight.clamp(0.0, 1.0);
    let mut mixed = gpui::hsla_to_rgba(from);
    let to = gpui::hsla_to_rgba(to);
    mixed.red += (to.red - mixed.red) * weight;
    mixed.green += (to.green - mixed.green) * weight;
    mixed.blue += (to.blue - mixed.blue) * weight;
    mixed.alpha += (to.alpha - mixed.alpha) * weight;
    gpui::rgb_to_hsla(mixed)
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
        self.sidebar_threads.project_icons.retain(&listing);
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
        let container = div()
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
        self.desktop_sidebar_sections(container, &listing, now, window, cx)
    }

    /// Appends the unread rows, then the `Working` and `History` groups,
    /// each only when it holds a row.
    fn desktop_sidebar_sections(
        &mut self,
        mut container: Stateful<Div>,
        listing: &RecentThreadListing,
        now: UnixMillis,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let sections = section_recent_threads(listing, now);
        if !sections.unread.is_empty() {
            let mut unread = div()
                .w_full()
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .debug_selector(|| "artisan-sidebar-threads-unread".to_owned());
            for thread in &sections.unread {
                unread = unread.child(self.desktop_sidebar_thread(thread, cx));
            }
            container = container.child(unread);
        }
        if !sections.working.is_empty() {
            let rows = if self.sidebar_group_open(SidebarGroup::Working) {
                sections
                    .working
                    .iter()
                    .map(|thread| self.desktop_sidebar_thread(thread, cx).into_any_element())
                    .collect()
            } else {
                Vec::new()
            };
            container = container.child(self.desktop_sidebar_collapsible(
                SidebarGroup::Working,
                sections.working.len(),
                rows,
                window,
                cx,
            ));
        }
        if !sections.history.is_empty() {
            let groups = if self.sidebar_group_open(SidebarGroup::History) {
                sections
                    .history
                    .iter()
                    .map(|group| {
                        self.desktop_sidebar_thread_group(group, cx)
                            .into_any_element()
                    })
                    .collect()
            } else {
                Vec::new()
            };
            container = container.child(self.desktop_sidebar_collapsible(
                SidebarGroup::History,
                sections.history_len(),
                groups,
                window,
                cx,
            ));
        }
        container
    }

    fn sidebar_group_open(&self, group: SidebarGroup) -> bool {
        self.sidebar_threads
            .groups
            .get(&group)
            .is_some_and(|disclosure| disclosure.open)
    }

    fn toggle_sidebar_group(&mut self, group: SidebarGroup, cx: &mut Context<Self>) {
        let disclosure = self.sidebar_threads.groups.entry(group).or_default();
        disclosure.open = !disclosure.open;
        disclosure.toggled = Some(Instant::now());
        cx.notify();
    }

    /// One collapsible group: a header in the age-group label style with a
    /// count and a chevron that turns down when open, then its content.
    ///
    /// Opening flips the chevron and fades the content in over the fast
    /// motion token; closing flips the chevron and removes the content.
    fn desktop_sidebar_collapsible(
        &mut self,
        group: SidebarGroup,
        count: usize,
        content: Vec<AnyElement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let reduced = cx.reduce_motion();
        let disclosure = self.sidebar_threads.groups.entry(group).or_default();
        let open = disclosure.open;
        let progress = disclosure.progress(Instant::now(), reduced);
        let focus = disclosure
            .focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        if progress.is_some() {
            window.request_animation_frame();
        }
        let settled = if open { 1.0 } else { 0.0 };
        let turn = progress.map_or(settled, |eased| if open { eased } else { 1.0 - eased });
        let selector = format!("artisan-sidebar-threads-{}", group.id());
        let header_selector = format!("{selector}-header");
        let chevron_selector = format!("{selector}-chevron");
        let muted = self.theme.colors.muted_foreground.to_paint();
        let focus_fill = self.theme.colors.muted.to_paint();
        let label = SharedString::from(format!("{}, {count} threads", group.label()));
        let header = div()
            .id(SharedString::from(header_selector.clone()))
            .track_focus(&focus)
            .tab_index(0)
            .role(gpui::Role::Button)
            .aria_label(label)
            .aria_expanded(open)
            .w_full()
            .flex()
            .items_center()
            .gap(px(4.0))
            .px(px(DESKTOP_COLUMN_ROW_INSET_PX))
            .pb(self.theme.spacing.steps(1.0))
            .rounded(px(6.0))
            .text_size(self.theme.typography.label_text)
            .text_color(muted)
            .cursor_pointer()
            .focus_visible(move |style| style.bg(focus_fill))
            .debug_selector(move || header_selector.clone())
            .child(div().flex_none().child(group.label()))
            .child(div().flex_none().opacity(0.7).child(count.to_string()))
            .child(
                gpui::svg()
                    .path(AssetId::TABLER_CHEVRON_RIGHT.as_str())
                    .size(px(12.0))
                    .flex_shrink_0()
                    .text_color(muted)
                    .with_transformation(gpui::Transformation::rotate(gpui::radians(
                        turn * std::f32::consts::FRAC_PI_2,
                    )))
                    .debug_selector(move || chevron_selector.clone()),
            )
            .on_click(cx.listener(move |app, _, window, cx| {
                window.focus(&focus, cx);
                app.toggle_sidebar_group(group, cx);
            }))
            .on_key_down(cx.listener(move |app, event: &gpui::KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    cx.stop_propagation();
                    app.toggle_sidebar_group(group, cx);
                }
            }));
        let mut section = div()
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .debug_selector(move || selector.clone())
            .child(header);
        if open && !content.is_empty() {
            let body = div()
                .w_full()
                .flex()
                .flex_col()
                .gap(self.theme.spacing.steps(3.0))
                .children(content);
            section = section.child(match progress {
                Some(eased) => body.opacity(eased),
                None => body,
            });
        }
        section
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
            .debug_selector(move || selector.clone());
        if group.age != crate::recent_thread_groups::RecentThreadAge::LastDay {
            rows = rows.child(
                desktop_section_label(&self.theme, group.age.label())
                    .debug_selector(move || header_selector.clone()),
            );
        }
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
        // The open thread's title reads in the link colour, fading with the
        // selection. The hover fill stays the pointer's alone.
        let title_color = mix_colors(
            self.desktop_theme.foreground,
            self.theme.colors.banner_info.to_paint(),
            weight,
        );
        let title = SharedString::from(thread.title.as_str().to_owned());
        let subtitle = SharedString::from(row.subtitle.as_str().to_owned());
        let project_icon = self.sidebar_threads.project_icons.render(
            &thread.project_id,
            &row.project_icon,
            &self.theme,
        );
        let open = self.sidebar_threads.selection.target.as_ref() == Some(&thread.thread_id);
        let indicator = SidebarThreadIndicator::of(thread, open);
        let description = match indicator {
            Some(indicator) => SharedString::from(format!("{subtitle}, {}", indicator.label())),
            None => subtitle.clone(),
        };
        let selector = format!("artisan-sidebar-thread-{}", thread.thread_id.as_str());
        let title_selector = format!("{selector}-title");
        let subtitle_selector = format!("{selector}-subtitle");
        let dot_selector = indicator.map(|indicator| format!("{selector}-{}", indicator.id()));
        let pulse = !cx.reduce_motion();
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
                            .text_color(title_color)
                            .debug_selector(move || title_selector.clone())
                            .child(title),
                    )
                    .child(
                        div()
                            .w_full()
                            .min_w(px(0.0))
                            .flex()
                            .items_center()
                            .gap(px(5.0))
                            .text_size(self.theme.typography.label_text)
                            .text_color(self.theme.colors.muted_foreground.to_paint())
                            .debug_selector(move || subtitle_selector.clone())
                            .child(project_icon)
                            .child(div().flex_1().min_w(px(0.0)).truncate().child(subtitle)),
                    ),
            )
            // A thread that needs the reader keeps its prominence without
            // reordering the chronological groups: the trailing state dot.
            // Purple is the reader's turn; live work breathes; an unread
            // outcome holds still until the thread is opened.
            .when_some(indicator, |row, indicator| {
                let colors = &self.theme.colors;
                let tone = match indicator {
                    SidebarThreadIndicator::Awaiting => colors.question_from,
                    SidebarThreadIndicator::Working => colors.primary,
                    SidebarThreadIndicator::Failed => colors.destructive,
                    SidebarThreadIndicator::Finished => colors.unread,
                };
                let dot_selector = dot_selector.unwrap_or_default();
                let dot = div()
                    .size(px(crate::shell::LEGACY_RAIL_STATE_DOT_PX))
                    .flex_shrink_0()
                    .rounded_full()
                    .bg(tone.to_paint())
                    .debug_selector({
                        let dot_selector = dot_selector.clone();
                        move || dot_selector.clone()
                    });
                if indicator == SidebarThreadIndicator::Working && pulse {
                    row.child(dot.with_animation(
                        SharedString::from(dot_selector),
                        Animation::new(SIDEBAR_WORKING_PULSE).repeat(),
                        |dot, progress| {
                            let swing = (progress * std::f32::consts::TAU).cos().mul_add(0.5, 0.5);
                            dot.opacity(
                                SIDEBAR_WORKING_PULSE_FLOOR
                                    + (1.0 - SIDEBAR_WORKING_PULSE_FLOOR) * swing,
                            )
                        },
                    ))
                } else {
                    row.child(dot)
                }
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
    fn indicator_names_what_the_thread_needs_from_the_reader() {
        let thread = |working: bool, attention| ThreadSummary {
            has_started_response: true,
            has_active_work: working,
            attention,
            last_message_at: None,
            thread_id: ThreadId::parse("thread").unwrap(),
            project_id: ProjectId::parse("project").unwrap(),
            title: artisan_domain::ThreadTitle::parse("Thread").unwrap(),
            created_at: UnixMillis::from_millis(1),
            updated_at: UnixMillis::from_millis(1),
        };
        let of = SidebarThreadIndicator::of;
        assert_eq!(of(&thread(false, ThreadAttention::None), false), None);
        assert_eq!(
            of(&thread(true, ThreadAttention::None), false),
            Some(SidebarThreadIndicator::Working)
        );
        // The reader's turn outranks the work it blocks, open or not.
        for open in [false, true] {
            assert_eq!(
                of(&thread(true, ThreadAttention::AwaitingAnswer), open),
                Some(SidebarThreadIndicator::Awaiting)
            );
        }
        assert_eq!(
            of(&thread(false, ThreadAttention::Finished), false),
            Some(SidebarThreadIndicator::Finished)
        );
        assert_eq!(
            of(&thread(false, ThreadAttention::Failed), false),
            Some(SidebarThreadIndicator::Failed)
        );
        // The thread on screen is being read: no unread outcome on it.
        assert_eq!(of(&thread(false, ThreadAttention::Finished), true), None);
        assert_eq!(of(&thread(false, ThreadAttention::Failed), true), None);
        // A new run after an unread outcome reads as working.
        assert_eq!(
            of(&thread(true, ThreadAttention::Finished), false),
            Some(SidebarThreadIndicator::Working)
        );
    }

    #[test]
    fn title_colour_rests_on_the_foreground_and_reaches_the_link_colour() {
        let foreground = gpui::rgb_to_hsla(gpui::rgb(0x00ff_ffff));
        let link = gpui::rgb_to_hsla(gpui::rgb(0x0060_a5fa));
        let close = |left: gpui::Hsla, right: gpui::Hsla| {
            let (left, right) = (gpui::hsla_to_rgba(left), gpui::hsla_to_rgba(right));
            [
                left.red - right.red,
                left.green - right.green,
                left.blue - right.blue,
                left.alpha - right.alpha,
            ]
            .iter()
            .all(|delta| delta.abs() < 0.01)
        };
        // An unselected title is the plain, fully opaque foreground.
        assert!(close(mix_colors(foreground, link, 0.0), foreground));
        assert!(close(mix_colors(foreground, link, 1.0), link));
        let half = gpui::hsla_to_rgba(mix_colors(foreground, link, 0.5));
        assert!(
            (half.alpha - 1.0).abs() < 0.01,
            "the title never goes translucent"
        );
    }

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
