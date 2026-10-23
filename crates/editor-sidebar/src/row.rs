//! One thread row, its own cached entity so a live change re-renders only
//! the row it touches. The sidebar keeps entities for the rows its list
//! lays out, and drops them once they scroll away. Ported from the previous Editor's `desktop_sidebar_thread`:
//! the title on one line, the project's mark and repository beneath, and
//! a trailing dot when the thread needs the user.

use crate::Sidebar;
use crate::list::{RowData, RowState};
use editor_core::DraggedThread;
use editor_ui::AssetId;
use editor_ui::asset_seam::asset_glyph;
use editor_ui::measure::Measured;
use editor_ui::picker_motion::HoverSurface;
use editor_ui::project_mark::project_mark;
use editor_ui::theme::{ActiveTheme as _, RadiusStep, RadiusTokens, ThemeMode};
use gpui::RenderImage;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Context, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, PulseExt as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, WeakEntity, Window, div, px,
};
use std::sync::Arc;
use std::time::Duration;

/// Height of one two-line thread row.
pub const ROW_HEIGHT: f32 = 48.0;
/// The trailing state dot.
pub const STATE_DOT: f32 = 6.0;
/// One full fade of the working dot, out and back.
pub const WORKING_PULSE: Duration = Duration::from_millis(1_600);
/// The dimmest the working dot gets mid-pulse.
pub const WORKING_PULSE_FLOOR: f32 = 0.4;

/// What one row draws. The sidebar sets it every time the row is laid out;
/// an unchanged input leaves the cached paint alone.
#[derive(Clone)]
pub struct RowInputs {
    pub data: RowData,
    pub image: Option<Arc<RenderImage>>,
    #[expect(
        clippy::option_option,
        reason = "editor_ui::project_mark takes this shape: no repository vs a repository without a known host"
    )]
    pub host: Option<Option<String>>,
    pub reduced: bool,
    /// The theme mode the row paints in: a flip re-renders the cached row.
    pub mode: ThemeMode,
    /// What dragging the row into a composer mentions; `None` for a thread
    /// no `@title` can name.
    pub drag: Option<DraggedThread>,
}

impl PartialEq for RowInputs {
    fn eq(&self, other: &Self) -> bool {
        self.data == other.data
            && self.host == other.host
            && self.reduced == other.reduced
            && self.mode == other.mode
            && self.drag == other.drag
            && match (&self.image, &other.image) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

pub struct ThreadRow {
    inputs: RowInputs,
    sidebar: WeakEntity<Sidebar>,
    /// The thread list's sliding pill.
    hover: HoverSurface,
    /// The row's place, measured outside the cached view, for the
    /// keyboard-opened menu.
    bounds: Measured,
}

impl ThreadRow {
    pub(crate) const fn new(
        inputs: RowInputs,
        sidebar: WeakEntity<Sidebar>,
        hover: HoverSurface,
        bounds: Measured,
    ) -> Self {
        Self {
            inputs,
            sidebar,
            hover,
            bounds,
        }
    }

    pub(crate) fn set(&mut self, inputs: RowInputs, cx: &mut Context<Self>) {
        if self.inputs != inputs {
            self.inputs = inputs;
            cx.notify();
        }
    }
}

impl Render for ThreadRow {
    #[expect(
        clippy::too_many_lines,
        reason = "the row is one element tree, ported from the old Editor"
    )]
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let RowInputs {
            data,
            image,
            host,
            reduced,
            mode: _,
            drag,
        } = self.inputs.clone();
        let theme = *cx.theme();
        let sidebar = self.sidebar.clone();
        let hover = self.hover.clone();
        let bounds = self.bounds.clone();
        let colors = &theme.colors;
        let title_color = if data.selected {
            colors.thread_title_active.to_paint()
        } else {
            colors.text.to_paint()
        };
        let dot = match data.state {
            RowState::Idle => None,
            RowState::Working => Some(colors.text),
            RowState::Waiting => Some(colors.primary),
            RowState::Unread => Some(colors.unread),
            RowState::Failed => Some(colors.red.solid),
            RowState::Parked => Some(colors.amber.solid),
        };
        let selector = format!("sidebar-thread-{}", data.id);
        let title_selector = format!("{selector}-title");
        let subtitle_selector = format!("{selector}-subtitle");
        let (open_id, menu_id) = (data.id.clone(), data.id.clone());
        let state_label = match data.state {
            RowState::Idle => "",
            RowState::Working => "Working",
            RowState::Waiting => "Needs answer",
            RowState::Unread => "Unread",
            RowState::Failed => "Failed",
            RowState::Parked => "Waiting to pick up",
        };
        let hover_id = selector.clone();
        let keyboard_menu = menu_id.clone();
        let keyboard_owner = sidebar.clone();
        div()
            .id(SharedString::from(selector))
            .role(gpui::Role::Button).aria_label(data.title.clone()).aria_selected(data.selected)
            .aria_description(format!("{}; {state_label}; Thread actions: Shift+F10", data.subtitle))
            .focusable().tab_index(0)
            .on_key_down(move |event: &gpui::KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "contextmenu" | "menu") || (event.keystroke.key == "f10" && event.keystroke.modifiers.shift) {
                    let bounds = bounds.get().unwrap_or_default();
                    cx.stop_propagation();
                    keyboard_owner.update(cx, |sidebar, cx| sidebar.open_thread_menu(keyboard_menu.clone(), gpui::point(bounds.left(), bounds.bottom()), cx)).ok();
                }
            })
            .relative()
            .w_full()
            .h(px(ROW_HEIGHT))
            .flex_none()
            .min_w(px(0.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(8.0))
            .rounded(RadiusTokens::value(RadiusStep::Md))
            .cursor_pointer()
            // The list's one pill flies to the row under the pointer; the
            // list measures the row outside this cached view, whose paint
            // replays.
            .on_hover({ let owner = sidebar.clone(); move |hovered: &bool, _, cx| {
                hover.hovered(&hover_id, *hovered);
                owner.update(cx, |_, cx| cx.notify()).ok();
            }})
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
                            .text_size(theme.typography.control_text)
                            .text_color(title_color)
                            .debug_selector(move || title_selector)
                            .child(data.title.clone()),
                    )
                    .child(
                        div()
                            .w_full()
                            .min_w(px(0.0))
                            .flex()
                            .items_center()
                            .gap(px(5.0))
                            .text_size(theme.typography.label_text)
                            .text_color(colors.text_muted.to_paint())
                            .debug_selector(move || subtitle_selector)
                            .child(project_mark(
                                image,
                                host,
                                14.0,
                                colors.text_muted.to_paint(),
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .truncate()
                                    .child(data.subtitle.clone()),
                            ),
                    ),
            )
            // A thread that needs the user keeps its prominence without
            // reordering: the accent is the user's turn, live work breathes, an
            // unread outcome holds still until the thread is opened.
            .when_some(dot, |row, tone| {
                let dot = div()
                    .size(px(STATE_DOT))
                    .flex_shrink_0()
                    .rounded_full()
                    .bg(tone.to_paint());
                if data.state == RowState::Working && !reduced {
                    // The GPU breathes the dot: frames re-present the scene
                    // instead of rebuilding the sidebar.
                    row.child(dot.with_pulse("working-pulse", WORKING_PULSE, WORKING_PULSE_FLOOR))
                } else {
                    row.child(dot)
                }
            })
            .when_some(drag, |row, drag| {
                row.on_drag(drag, |drag, _, _, cx| {
                    let title = SharedString::from(drag.title.clone());
                    cx.new(|_| DragPreview { title })
                })
            })
            .on_click({ let owner = sidebar.clone(); move |_, _, cx| {
                owner.update(cx, |sidebar, cx| sidebar.open_thread(open_id.clone(), cx)).ok();
            }})
            .on_mouse_down(MouseButton::Right, move |event: &gpui::MouseDownEvent, _, cx| {
                cx.stop_propagation();
                sidebar.update(cx, |sidebar, cx| sidebar.open_thread_menu(menu_id.clone(), event.position, cx)).ok();
            })
    }
}

/// The pill that follows the pointer while a thread is dragged toward a
/// composer.
struct DragPreview {
    title: SharedString,
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        editor_ui::popover::glass_menu(
            "sidebar-thread-drag",
            &theme,
            RadiusTokens::value(RadiusStep::LgMid),
        )
        .child(
            div()
                .h(px(32.0))
                .max_w(px(260.0))
                .px(px(10.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_sm()
                .text_color(theme.colors.text.to_paint())
                .child(
                    asset_glyph(AssetId::TABLER_MESSAGE_CIRCLE)
                        .size(px(14.0))
                        .flex_shrink_0()
                        .text_color(theme.colors.text_muted.to_paint()),
                )
                .child(div().min_w_0().truncate().child(self.title.clone())),
        )
    }
}
