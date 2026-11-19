use crate::core::Core;
use crate::sidebar::{
    controller::{Sidebar, TextSearchStatus},
    geometry::ThreadPane,
    view::{self, ViewAction},
};
use crate::ui::kit::{Button, ButtonSize, ButtonTone, Space, Spaced};
use crate::ui::page;
use crate::ui::theme::ArtisanTheme;
use aqua::{Aqua, pct, px};
use aqua_ui::element::Element;
use std::rc::Rc;

pub(crate) fn view(
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
    let inbox_height: f32 = inbox_entries
        .iter()
        .map(editor_sidebar::geometry::Entry::height)
        .sum();
    let history_content: f32 = history_entries
        .iter()
        .map(editor_sidebar::geometry::Entry::height)
        .sum();
    let history_height = history_height(
        height,
        inbox_height,
        history_content,
        sidebar.history_fraction,
    );
    let visuals = sidebar.thread_visual_factory(&theme, reduced);
    let list = view::thread_entries_view(
        sidebar,
        core,
        &theme,
        height,
        inbox_entries,
        ThreadPane::Inbox,
        Rc::clone(&visuals),
        Rc::clone(&emit),
    );
    let inbox = Aqua::Box()
        .layout("column")
        .styles([Aqua::Style()
            .flex_grow(1.0)
            .min_width(pct(0.0))
            .min_height(pct(0.0))])
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
            .min_width(pct(0.0))
            .min_height(pct(0.0))])
        .child(inbox);
    if !sidebar.lists.history.is_empty() {
        column = column.child(
            Aqua::Box()
                .height(crate::sidebar::geometry::history_height())
                .styles([Aqua::Style().flex_shrink(0.0)])
                .child(view::history_toggle(
                    &theme,
                    sidebar.history_fraction,
                    &emit,
                ))
                .build(),
        );
    }
    if sidebar.history_visible() {
        let history = view::thread_entries_view(
            sidebar,
            core,
            &theme,
            history_height,
            history_entries,
            ThreadPane::History,
            visuals,
            Rc::clone(&emit),
        );
        column = column.child(
            Aqua::Box()
                .layout("column")
                .styles([Aqua::Style()
                    .height(px(history_height))
                    .min_height(pct(0.0))
                    .flex_shrink(0.0)
                    .clip(true)])
                .children(history)
                .build(),
        );
    }
    column = column.children(states(sidebar, core, &theme, &emit));
    Aqua::Box()
        .layout("column")
        .styles([Aqua::Style()
            .flex_grow(1.0)
            .min_width(pct(0.0))
            .min_height(pct(0.0))])
        .child(column.build().with_key("sidebar-threads-hover-surface"))
        .build()
        .with_key(view::ROOT_KEY)
}

fn button(
    key: &str,
    theme: &ArtisanTheme,
    label: &str,
    action: ViewAction,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    let emit = Rc::clone(emit);
    Button::text(key, *theme, ButtonTone::Neutral, label)
        .size(ButtonSize::Sm)
        .on_activate(move |_| emit(action.clone()))
        .build()
}

fn guidance(theme: &ArtisanTheme, text: String, action: Option<Element>) -> Element {
    aqua_ui::element::Box::new()
        .layout("column")
        .spaced(Space::S8)
        .padded(Space::S12)
        .child(page::note(theme, text).build())
        .children(action)
        .build()
}

pub fn states(
    sidebar: &Sidebar,
    core: &Core,
    theme: &ArtisanTheme,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Vec<Element> {
    let mut states = Vec::new();
    if let TextSearchStatus::Failed(cause) = sidebar.text_search_status() {
        states.push(
            page::notice(theme, format!("Couldn't search messages: {cause}"))
                .action(button(
                    "sidebar-search-retry",
                    theme,
                    "Try again",
                    ViewAction::Search,
                    emit,
                ))
                .build(),
        );
    }
    let host = sidebar.host_data(core).filter(|host| host.is_live());
    if let Some(message) = host.and_then(|host| host.rollback_notice().message()) {
        states.push(
            page::notice(&theme, message)
                .action(button(
                    "sidebar-rollback-dismiss",
                    theme,
                    "Dismiss",
                    ViewAction::DismissRollbackNotice,
                    emit,
                ))
                .build(),
        );
    }
    if let Some(error) = host.and_then(|host| host.thread_page(false).error()) {
        states.push(
            page::notice(theme, error)
                .action(button(
                    "sidebar-threads-retry",
                    theme,
                    "Try again",
                    ViewAction::RetryThreads,
                    emit,
                ))
                .build(),
        );
    }
    if host.is_some_and(crate::core::HostData::project_icons_failed) {
        states.push(
            page::notice(theme, "Some project icons didn't load.")
                .action(button(
                    "sidebar-metadata-retry",
                    theme,
                    "Try again",
                    ViewAction::RetryProjectIcons,
                    emit,
                ))
                .build(),
        );
    }
    if let Some(error) = &sidebar.error {
        states.push(
            page::notice(theme, error.clone())
                .action(button(
                    "sidebar-error-dismiss",
                    theme,
                    "Dismiss",
                    ViewAction::DismissError,
                    emit,
                ))
                .build(),
        );
    }
    let lists = &sidebar.lists;
    let empty = lists.inbox.is_empty() && lists.history.is_empty() && lists.hits.is_empty();
    let Some(host) = host.filter(|_| empty && states.is_empty()) else {
        return states;
    };
    let query = sidebar.search.borrow().text().trim().to_owned();
    states.push(if !query.is_empty() {
        match sidebar.text_search_status() {
            TextSearchStatus::Searching => guidance(theme, "Searching messages…".to_owned(), None),
            TextSearchStatus::Idle | TextSearchStatus::Failed(_) => guidance(
                theme,
                format!("No threads match “{query}”."),
                Some(button(
                    "sidebar-search-clear",
                    theme,
                    "Clear search",
                    ViewAction::ClearSearch,
                    emit,
                )),
            ),
        }
    } else if !host.thread_page(false).is_loaded() {
        guidance(theme, "Loading threads…".to_owned(), None)
    } else {
        guidance(
            theme,
            "No threads yet.".to_owned(),
            Some(button(
                "sidebar-empty-new-thread",
                theme,
                "New thread",
                ViewAction::NewThread(None),
                emit,
            )),
        )
    });
    states
}

pub fn history_height(viewport: f32, inbox: f32, history: f32, fraction: f32) -> f32 {
    let available = (viewport - crate::sidebar::geometry::history_height()).max(0.0);
    (available - inbox)
        .max(available * 0.5)
        .min(available)
        .min(history)
        * fraction.clamp(0.0, 1.0)
}
