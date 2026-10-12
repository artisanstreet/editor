//! One thread row, its own entity so a live change re-renders only the row
//! it touches. Ported from the previous Editor's `desktop_sidebar_thread`:
//! the title on one line, the project's mark and repository beneath, and
//! a trailing dot when the thread needs the user.

use crate::Sidebar;
use crate::hover::HoverSurface;
use crate::list::{ProjectMark, RowData, RowState};
use editor_ui::AssetId;
use editor_ui::asset_seam::asset_glyph;
use editor_ui::icon::{IconSize, IconStyle, IconTint, icon};
use editor_ui::theme::ArtisanTheme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    Animation, AnimationExt as _, AnyElement, Context, InteractiveElement as _, IntoElement,
    MouseButton, ObjectFit, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, StyledImage as _, WeakEntity, Window, div, img,
    px,
};
use std::time::Duration;

/// Height of one two-line thread row.
pub(crate) const ROW_HEIGHT: f32 = 48.0;
/// The trailing state dot.
const STATE_DOT: f32 = 6.0;
/// One full fade of the working dot, out and back.
const WORKING_PULSE: Duration = Duration::from_millis(1_600);
/// The dimmest the working dot gets mid-pulse.
const WORKING_PULSE_FLOOR: f32 = 0.4;

pub(crate) struct ThreadRow {
    data: RowData,
    theme: ArtisanTheme,
    sidebar: WeakEntity<Sidebar>,
    /// The thread list's sliding pill.
    hover: HoverSurface,
}

impl ThreadRow {
    pub(crate) fn new(
        data: RowData,
        theme: ArtisanTheme,
        sidebar: WeakEntity<Sidebar>,
        hover: HoverSurface,
    ) -> Self {
        Self {
            data,
            theme,
            sidebar,
            hover,
        }
    }

    pub(crate) fn set(&mut self, data: RowData, cx: &mut Context<Self>) {
        if self.data != data {
            self.data = data;
            cx.notify();
        }
    }
}

/// The 14 px mark before a row's subtitle.
pub(crate) fn project_mark(mark: &ProjectMark, theme: &ArtisanTheme) -> AnyElement {
    let slot = div().size(px(14.0)).flex_shrink_0();
    match mark {
        ProjectMark::Icon(image) => slot
            .child(
                img(image.clone())
                    .size(px(14.0))
                    .rounded(px(3.0))
                    .object_fit(ObjectFit::Contain),
            )
            .into_any_element(),
        ProjectMark::Host(host) => {
            let (asset, monochrome) = host_mark(host.as_deref());
            let mut glyph = asset_glyph(asset).size(px(14.0));
            if monochrome {
                glyph = glyph.text_color(theme.colors.muted_foreground.to_paint());
            }
            slot.child(glyph).into_any_element()
        }
        ProjectMark::Folder => slot
            .child(icon(IconStyle::resolve(
                *theme,
                AssetId::TABLER_FOLDER,
                IconSize::Compact,
                IconTint::Muted,
            )))
            .into_any_element(),
    }
}

/// A repository host's mark, and whether it is one colour.
pub(crate) fn host_mark(host: Option<&str>) -> (AssetId, bool) {
    match host {
        Some("github") => (AssetId::SVGL_GITHUB, true),
        Some("gitlab") => (AssetId::SVGL_GITLAB, false),
        Some("azure") => (AssetId::SVGL_MICROSOFT_AZURE, false),
        Some("bitbucket") => (AssetId::SIMPLE_ICONS_BITBUCKET, false),
        Some("codeberg") => (AssetId::SIMPLE_ICONS_CODEBERG, false),
        Some("gitea") => (AssetId::SIMPLE_ICONS_GITEA, false),
        Some("sourcehut") => (AssetId::SIMPLE_ICONS_SOURCEHUT, false),
        _ => (AssetId::SVGL_GIT, false),
    }
}

impl Render for ThreadRow {
    #[allow(clippy::too_many_lines)]
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let colors = &theme.colors;
        let data = &self.data;
        // The open thread's title reads in the link colour; the hover fill
        // stays the pointer's alone.
        let title_color = if data.selected {
            colors.banner_info.to_paint()
        } else {
            colors.foreground.to_paint()
        };
        let dot = match data.state {
            RowState::Idle => None,
            RowState::Working => Some(colors.primary),
            RowState::Waiting => Some(colors.question_from),
            RowState::Unread => Some(colors.unread),
            RowState::Failed => Some(colors.destructive),
        };
        let selector = format!("sidebar-thread-{}", data.id);
        let title_selector = format!("{selector}-title");
        let subtitle_selector = format!("{selector}-subtitle");
        let (open_id, menu_id) = (data.id.clone(), data.id.clone());
        let hover_id = selector.clone();
        div()
            .id(SharedString::from(selector.clone()))
            .debug_selector(move || selector.clone())
            .relative()
            .w_full()
            .h(px(ROW_HEIGHT))
            .flex_none()
            .min_w(px(0.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(8.0))
            .rounded(px(6.0))
            .cursor_pointer()
            // The list's one pill flies to the row under the pointer; the
            // list measures the row, since a cached view's paint replays.
            .on_hover(cx.listener(move |row, hovered: &bool, _, cx| {
                row.hover.hovered(&hover_id, *hovered);
                row.sidebar.update(cx, |_, cx| cx.notify()).ok();
            }))
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
                            .debug_selector(move || title_selector.clone())
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
                            .text_color(colors.muted_foreground.to_paint())
                            .debug_selector(move || subtitle_selector.clone())
                            .child(project_mark(&data.mark, &theme))
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
            // reordering: purple is the user's turn, live work breathes, an
            // unread outcome holds still until the thread is opened.
            .when_some(dot, |row, tone| {
                let dot = div()
                    .size(px(STATE_DOT))
                    .flex_shrink_0()
                    .rounded_full()
                    .bg(tone.to_paint());
                if data.state == RowState::Working {
                    row.child(dot.with_animation(
                        "working-pulse",
                        Animation::new(WORKING_PULSE).repeat(),
                        |dot, progress| {
                            let swing = (progress * std::f32::consts::TAU).cos().mul_add(0.5, 0.5);
                            dot.opacity(
                                WORKING_PULSE_FLOOR + (1.0 - WORKING_PULSE_FLOOR) * swing,
                            )
                        },
                    ))
                } else {
                    row.child(dot)
                }
            })
            .on_click(cx.listener(move |row, _, _, cx| {
                let id = open_id.clone();
                row.sidebar
                    .update(cx, |sidebar, cx| sidebar.open_thread(id, cx))
                    .ok();
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |row, event: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let id = menu_id.clone();
                    let position = event.position;
                    row.sidebar
                        .update(cx, |sidebar, cx| sidebar.open_thread_menu(id, position, cx))
                        .ok();
                }),
            )
    }
}
