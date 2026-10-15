//! One thread row, its own cached entity so a live change re-renders only
//! the row it touches. The sidebar keeps entities for the rows its list
//! lays out, and drops them once they scroll away. Ported from the previous Editor's `desktop_sidebar_thread`:
//! the title on one line, the project's mark and repository beneath, and
//! a trailing dot when the thread needs the user.

use crate::Sidebar;
use crate::list::{RowData, RowState};
use editor_ui::AssetId;
use editor_ui::asset_seam::asset_glyph;
use editor_ui::icon::{IconSize, IconStyle, IconTint, icon};
use editor_ui::measure::Measured;
use editor_ui::picker_motion::HoverSurface;
use editor_ui::theme::{ArtisanTheme, RadiusStep, RadiusTokens};
use gpui::RenderImage;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, MouseButton, ObjectFit,
    ParentElement as _, PulseExt as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, StyledImage as _, WeakEntity, Window, div, img, px,
};
use std::sync::Arc;
use std::time::Duration;

/// Height of one two-line thread row.
pub(crate) const ROW_HEIGHT: f32 = 48.0;
/// The trailing state dot.
const STATE_DOT: f32 = 6.0;
/// One full fade of the working dot, out and back.
const WORKING_PULSE: Duration = Duration::from_millis(1_600);
/// The dimmest the working dot gets mid-pulse.
const WORKING_PULSE_FLOOR: f32 = 0.4;

/// The 14 px mark before a row's subtitle.
pub(crate) fn project_mark(
    image: Option<Arc<RenderImage>>,
    host: Option<Option<String>>,
    theme: &ArtisanTheme,
) -> AnyElement {
    let slot = div().size(px(14.0)).flex_shrink_0();
    if let Some(image) = image {
        slot.child(
            img(image)
                .size(px(14.0))
                .rounded(RadiusTokens::value(RadiusStep::X2sMid))
                .object_fit(ObjectFit::Contain),
        )
        .into_any_element()
    } else if let Some(host) = host {
        let (asset, monochrome) = host_mark(host.as_deref());
        let mut glyph = asset_glyph(asset).size(px(14.0));
        if monochrome {
            glyph = glyph.text_color(theme.colors.muted_foreground.to_paint());
        }
        slot.child(glyph).into_any_element()
    } else {
        slot.child(icon(IconStyle::resolve(
            *theme,
            AssetId::TABLER_FOLDER,
            IconSize::Compact,
            IconTint::Muted,
        )))
        .into_any_element()
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

/// What one row draws. The sidebar sets it every time the row is laid out;
/// an unchanged input leaves the cached paint alone.
#[derive(Clone)]
pub(crate) struct RowInputs {
    pub data: RowData,
    pub image: Option<Arc<RenderImage>>,
    pub host: Option<Option<String>>,
    pub reduced: bool,
}

impl PartialEq for RowInputs {
    fn eq(&self, other: &Self) -> bool {
        self.data == other.data
            && self.host == other.host
            && self.reduced == other.reduced
            && match (&self.image, &other.image) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

pub(crate) struct ThreadRow {
    inputs: RowInputs,
    theme: ArtisanTheme,
    sidebar: WeakEntity<Sidebar>,
    /// The thread list's sliding pill.
    hover: HoverSurface,
    /// The row's place, measured outside the cached view, for the
    /// keyboard-opened menu.
    bounds: Measured,
}

impl ThreadRow {
    pub(crate) fn new(
        inputs: RowInputs,
        theme: ArtisanTheme,
        sidebar: WeakEntity<Sidebar>,
        hover: HoverSurface,
        bounds: Measured,
    ) -> Self {
        Self {
            inputs,
            theme,
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
    #[allow(clippy::too_many_lines)]
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let RowInputs {
            data,
            image,
            host,
            reduced,
        } = self.inputs.clone();
        let theme = self.theme;
        let sidebar = self.sidebar.clone();
        let hover = self.hover.clone();
        let bounds = self.bounds.clone();
        let colors = &theme.colors;
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
        let state_label = match data.state {
            RowState::Idle => "",
            RowState::Working => "Working",
            RowState::Waiting => "Needs answer",
            RowState::Unread => "Unread",
            RowState::Failed => "Failed",
        };
        let hover_id = selector.clone();
        let keyboard_menu = menu_id.clone();
        let keyboard_owner = sidebar.clone();
        div()
            .id(SharedString::from(selector.clone()))
            .role(gpui::Role::Button).aria_label(data.title.clone()).aria_selected(data.selected)
            .aria_description(format!("{}; {state_label}; Thread actions: Shift+F10", data.subtitle))
            .focusable().tab_index(0)
            .on_key_down(move |event: &gpui::KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "contextmenu" | "menu") || (event.keystroke.key == "f10" && event.keystroke.modifiers.shift) {
                    let bounds = bounds.get().unwrap_or_default();
                    cx.stop_propagation();
                    let _ = keyboard_owner.update(cx, |sidebar, cx| sidebar.open_thread_menu(keyboard_menu.clone(), gpui::point(bounds.left(), bounds.bottom()), cx));
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
            .rounded(RadiusTokens::value(RadiusStep::Sm))
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
                            .child(project_mark(image, host, &theme))
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
                if data.state == RowState::Working && !reduced {
                    // The GPU breathes the dot: frames re-present the scene
                    // instead of rebuilding the sidebar.
                    row.child(dot.with_pulse("working-pulse", WORKING_PULSE, WORKING_PULSE_FLOOR))
                } else {
                    row.child(dot)
                }
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
