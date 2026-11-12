use crate::core::Core;
use crate::sidebar::{
    controller::Sidebar,
    view::{self, ViewAction},
};
use crate::ui::kit::TextRole;
use crate::ui::theme::ArtisanTheme;
use aqua::{Aqua, px};
use aqua_ui::element::Element;
use std::rc::Rc;

pub fn view(
    sidebar: &Sidebar,
    core: &Core,
    mut theme: ArtisanTheme,
    height: f32,
    reduced: bool,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    if !theme.colors.mode.is_dark() {
        theme.colors.text_muted = theme.colors.text_muted.animate_to(theme.colors.text, 0.08);
        theme.colors.thread_title_active = theme
            .colors
            .thread_title_active
            .animate_to(theme.colors.text, 0.25);
    }
    let entries = crate::sidebar::geometry::entries(&sidebar.lists, true);
    let inbox_entries = entries
        .iter()
        .take_while(|entry| {
            !matches!(
                entry,
                crate::sidebar::geometry::Entry::Spacer | crate::sidebar::geometry::Entry::History
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    let history_entries = entries
        .iter()
        .skip_while(|entry| !matches!(entry, crate::sidebar::geometry::Entry::History))
        .skip(1)
        .cloned()
        .collect::<Vec<_>>();
    let inbox_height: f32 = inbox_entries.iter().map(|entry| entry.height()).sum();
    let history_content: f32 = history_entries.iter().map(|entry| entry.height()).sum();
    let history_height = history_height(
        height,
        inbox_height,
        history_content,
        sidebar.history_fraction,
    );
    let visuals = sidebar.thread_visual_factory(theme, reduced);
    let list = view::thread_entries_view(
        sidebar,
        core,
        theme,
        height,
        inbox_entries,
        "sidebar-threads",
        visuals.clone(),
        Rc::clone(&emit),
    );
    let inbox = Aqua::Box()
        .layout("column")
        .styles([Aqua::Style()
            .flex_grow(1.0)
            .min_width(px(0.0))
            .min_height(px(0.0))])
        .children(list)
        .child(sidebar.threads_edges.band(
            "sidebar-threads-edge-top",
            crate::ui::scroll_edge::Edge::Top,
            theme.colors.frame,
        ))
        .child(sidebar.threads_edges.band(
            "sidebar-threads-edge-bottom",
            crate::ui::scroll_edge::Edge::Bottom,
            theme.colors.frame,
        ))
        .build();
    let mut column = Aqua::Box()
        .layout("column")
        .styles([Aqua::Style()
            .flex_grow(1.0)
            .min_width(px(0.0))
            .min_height(px(0.0))])
        .child(inbox);
    if !sidebar.lists.history.is_empty() {
        column = column.child(
            Aqua::Box()
                .height(crate::sidebar::geometry::history_height())
                .styles([Aqua::Style().flex_shrink(0.0)])
                .child(view::history_toggle(
                    theme,
                    sidebar.history_fraction,
                    Rc::clone(&emit),
                ))
                .build(),
        );
    }
    if sidebar.history_visible() {
        let history = view::thread_entries_view(
            sidebar,
            core,
            theme,
            history_height,
            history_entries,
            "sidebar-history-threads",
            visuals,
            Rc::clone(&emit),
        );
        column = column.child(
            Aqua::Box()
                .layout("column")
                .styles([Aqua::Style()
                    .height(px(history_height))
                    .min_height(px(0.0))
                    .flex_shrink(0.0)
                    .clip(true)])
                .children(history)
                .build(),
        );
    }
    if let Some(error) = sidebar.error.as_ref().or_else(|| {
        sidebar
            .host_data(core)
            .and_then(|host| host.thread_page(false).error.as_ref())
    }) {
        column = column.child(
            TextRole::Caption
                .text(&theme, error.clone())
                .gpui_source_color(theme.colors.destructive_text.to_reference_text_color())
                .build(),
        );
    }
    Aqua::Box()
        .layout("column")
        .styles([Aqua::Style()
            .flex_grow(1.0)
            .min_width(px(0.0))
            .min_height(px(0.0))])
        .child(column.build().with_key("sidebar-threads-hover-surface"))
        .build()
        .with_key(view::ROOT_KEY)
        .on_input(
            aqua_ui::input::InputKind::PointerLeave,
            aqua_ui::input::EventPhase::Bubble,
            move |event| {
                if event.current_target == event.target {
                    emit(ViewAction::ClearThreadHover);
                }
                aqua_ui::input::InputResponse::default()
            },
        )
}

pub fn history_height(viewport: f32, inbox: f32, history: f32, fraction: f32) -> f32 {
    let available = (viewport - 43.0).max(0.0);
    (available - inbox)
        .max(available * 0.5)
        .min(available)
        .min(history)
        * fraction.clamp(0.0, 1.0)
}
