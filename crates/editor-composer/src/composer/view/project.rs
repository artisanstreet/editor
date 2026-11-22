use super::{
    ComposerAction, Dispatch, MENU_VIEWPORT_MARGIN, PROJECT_EDGE_BOTTOM, PROJECT_EDGE_TOP,
    PROJECT_MENU_WIDTH, PROJECT_ROWS_SHOWN, PROJECT_SCROLL, PROJECT_TAB_WIDTH,
};
use aqua_ui::{
    element::{Box as UiBox, Element, Scroll},
    style::{Length, Style},
};
use editor_assets::AssetId;
use editor_ui::{
    kit::{Divider, IconSize, Lead, Picker, Row, RowItem, Surface, Trigger, TriggerSize},
    theme::ArtisanTheme,
};
use std::rc::Rc;

pub fn project_menu(
    theme: &ArtisanTheme,
    host: &editor_core::HostData,
    picker: &super::super::picker::PickerState,
    selected: Option<&artisan_api::ProjectId>,
    viewport: aqua_core::Size,
    dispatch: &Dispatch,
) -> Element {
    let surface = Surface::Picker(Picker::List);
    let row = Row::MenuItem;
    let muted = theme.colors.text_muted.to_reference_paint();
    let item =
        |key: String, title: String, leading: Element, chosen: bool, action: ComposerAction| {
            let dispatch = Rc::clone(dispatch);
            let mut item = RowItem::new(key, *theme, row, surface, title)
                .leading(leading)
                .on_activate(move || dispatch(action.clone()));
            if chosen {
                item = item.trailing(IconSize::Md.glyph(AssetId::TABLER_CHECK, muted));
            }
            item.build().aria_selected(chosen)
        };
    let mut rows = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .style(Style::new().min_width(Length::Pct(0.0)));
    for id in &picker.projects {
        let Some(project) = host.projects.get(id) else {
            continue;
        };
        rows = rows.child(item(
            format!("composer-project-{id}"),
            project.name.clone(),
            editor_ui::project_mark::Mark::of(host, id).view(theme.colors.text_muted),
            selected == Some(id),
            ComposerAction::ChooseProject(id.clone()),
        ));
    }
    let count = picker.projects.len();
    let mut list = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .style(Style::new().min_width(Length::Pct(0.0)));
    if count > 0 {
        list = list
            .child(
                UiBox::new()
                    .width(Length::Pct(100.0))
                    .clip(true)
                    .child(
                        Scroll::new()
                            .width(Length::Pct(100.0))
                            .height(super::rows(count.min(PROJECT_ROWS_SHOWN)) * row.height(theme))
                            .style(Style::new().column())
                            .child(rows.build())
                            .build()
                            .with_key(PROJECT_SCROLL),
                    )
                    .child(picker.project_edges.band(
                        PROJECT_EDGE_TOP,
                        editor_ui::scroll_edge::Edge::Top,
                        theme.colors.gray.step(7),
                    ))
                    .child(picker.project_edges.band(
                        PROJECT_EDGE_BOTTOM,
                        editor_ui::scroll_edge::Edge::Bottom,
                        theme.colors.gray.step(4),
                    ))
                    .build(),
            )
            .child(Divider::Rule.build(theme, surface.padding_px()));
    }
    let scratch = artisan_api::ProjectId::scratch();
    if host
        .projects
        .get(&scratch)
        .is_some_and(|project| !project.removed)
    {
        list = list.child(item(
            "composer-project-No project".to_owned(),
            "No project".to_owned(),
            IconSize::Md.glyph(AssetId::TABLER_MESSAGE_CIRCLE, muted),
            selected == Some(&scratch),
            ComposerAction::ChooseProject(scratch),
        ));
    }
    list = list.child(item(
        "composer-project-New project".to_owned(),
        "New project".to_owned(),
        IconSize::Md.glyph(AssetId::TABLER_FOLDER_PLUS, muted),
        false,
        ComposerAction::NewProject,
    ));
    let width = PROJECT_MENU_WIDTH.min((viewport.w - MENU_VIEWPORT_MARGIN).max(0.0));
    surface
        .build(theme, vec![list.build()])
        .style(
            Style::new()
                .width(Length::Px(width))
                .min_width(Length::Px(width))
                .max_width(Length::Px(width))
                .flex_shrink(0.0),
        )
        .with_key("composer-project-menu")
}

pub(crate) fn project_tab(
    theme: &ArtisanTheme,
    controller: &super::super::controller::Composer,
    host: &editor_core::HostData,
    dispatch: Dispatch,
) -> Option<Element> {
    let fitted = |element: Element| {
        UiBox::new()
            .style(
                Style::new()
                    .min_width(Length::Pct(0.0))
                    .max_width(Length::Px(PROJECT_TAB_WIDTH)),
            )
            .child(element)
            .build()
    };
    match &controller.target {
        super::super::controller::ComposerTarget::Thread { thread, .. } => {
            let thread = host.threads.get(thread)?;
            let project = host.projects.get(&thread.project_id)?;
            Some(fitted(
                RowItem::new(
                    "composer-project",
                    *theme,
                    Row::Heading,
                    Surface::Composer,
                    project.name.clone(),
                )
                .leading(
                    editor_ui::project_mark::Mark::of(host, &thread.project_id)
                        .view(theme.colors.text_muted),
                )
                .build(),
            ))
        }
        super::super::controller::ComposerTarget::NewThread { .. } => {
            let project = controller
                .project(host)
                .filter(|id| host.projects.contains_key(id));
            let name = project
                .as_ref()
                .and_then(|id| host.projects.get(id))
                .map_or_else(
                    || "Choose a project".to_owned(),
                    |project| project.name.clone(),
                );
            let lead = project
                .as_ref()
                .map_or(Lead::Glyph(AssetId::TABLER_FOLDER), |id| {
                    Lead::Avatar(
                        editor_ui::project_mark::Mark::of(host, id).view(theme.colors.text_muted),
                    )
                });
            let open = controller.picker.menu == Some(super::super::picker::Menu::Project);
            Some(fitted(
                Trigger::new("composer-project", *theme, TriggerSize::Sm, name)
                    .lead(lead)
                    .muted()
                    .open(open)
                    .on_activate(move || dispatch(ComposerAction::ToggleProject))
                    .build(),
            ))
        }
    }
}
