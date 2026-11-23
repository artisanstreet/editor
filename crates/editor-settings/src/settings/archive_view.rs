use std::rc::Rc;

use aqua_ui::{
    AlignItems,
    collection::List,
    element::{Box as UiBox, Element},
    style::{Length, Style},
};
use artisan_api::{THREAD_RETENTION_DAYS_KEY, ThreadId};

use super::{
    controller::SettingsController,
    ui::{self, Ink},
    view::ViewAction,
};
use editor_core::HostData;
use editor_ui::{
    kit::{
        ButtonTone, Density, Finish, IconButton, IconButtonSize, IconSize, Row as KitRow, RowKind,
        Space, Spaced, TextRole,
    },
    select::{Select, SelectEntry, SelectMenu},
    theme::ArtisanTheme,
};

const VISIBLE_ROWS: u8 = 8;

fn row_height(theme: &ArtisanTheme) -> f32 {
    2.0f32.mul_add(
        RowKind::Form.pad().y.px(),
        KitRow::ListItem(Density::Tall).lines(theme) + Space::S2.px(),
    )
}

fn entries() -> Vec<SelectEntry<Option<u32>>> {
    let choices = artisan_api::choice_values(THREAD_RETENTION_DAYS_KEY);
    choices
        .iter()
        .map(|days| {
            SelectEntry::item(
                *days,
                match days {
                    None => "Never".into(),
                    Some(365) => "After a year".into(),
                    Some(days) => {
                        format!("After {}", editor_ui::locale::count(*days, "day", "days"))
                    }
                },
            )
        })
        .collect()
}

fn selected(host: &HostData) -> Option<u32> {
    host.setting(THREAD_RETENTION_DAYS_KEY)
        .cloned()
        .or_else(|| {
            artisan_api::setting_decl(THREAD_RETENTION_DAYS_KEY)
                .map(artisan_api::SettingDecl::default_value)
        })
        .and_then(|value| value.as_u64().and_then(|days| u32::try_from(days).ok()))
}

pub fn retention_menu(
    controller: &SettingsController,
    host: &HostData,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> SelectMenu<Option<u32>> {
    let change = Rc::clone(&emit);
    let host_id = host.id.clone();
    let instance = host.instance();
    SelectMenu::new(
        *theme,
        Some(selected(host)),
        entries(),
        Rc::clone(&controller.retention_select),
        ui::CONTROL_WIDTH,
    )
    .on_change(move |days| change(ViewAction::SetRetention(host_id.clone(), instance, days)))
    .on_open_change(move |open| emit(ViewAction::RetentionMenu(open)))
}

pub(crate) fn cleanup(
    controller: &SettingsController,
    host: &HostData,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let change = Rc::clone(&emit);
    let host_id = host.id.clone();
    let instance = host.instance();
    let select = Select::new(
        "settings-thread-retention",
        *theme,
        Some(selected(host)),
        entries(),
        Rc::clone(&controller.retention_select),
    )
    .label("Delete inactive threads")
    .on_change(move |days| change(ViewAction::SetRetention(host_id.clone(), instance, days)))
    .on_open_change(move |open| emit(ViewAction::RetentionMenu(open)))
    .build();
    let control = ui::fixed(select);
    let row = ui::row(
        theme,
        "Delete inactive threads",
        "Threads nothing has happened in for this long are deleted with their messages and images, archived or not. Working threads, threads waiting on you and threads whose agents are still active are kept.",
        Some(control),
    );
    ui::section(theme, "Cleanup", None, None, ui::card(theme, vec![row]))
}

fn span(millis: i64) -> String {
    let left = millis.max(60_000);
    let (count, unit) = if left >= 86_400_000 {
        (left / 86_400_000, ("day", "days"))
    } else if left >= 3_600_000 {
        (left / 3_600_000, ("hour", "hours"))
    } else {
        (left / 60_000, ("minute", "minutes"))
    };
    editor_ui::locale::count(count, unit.0, unit.1)
}

#[derive(Clone)]
struct Row {
    id: ThreadId,
    title: String,
    icon: Option<std::sync::Arc<aqua::ImageHandle>>,
    detail: String,
    restoring: bool,
}

#[expect(
    clippy::too_many_lines,
    reason = "the archive page is one virtualized list with its header, empty and error states"
)]
pub(crate) fn archived(
    controller: &SettingsController,
    host: &HostData,
    theme: &ArtisanTheme,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    let page = host.thread_page(true);
    let retention = selected(host);
    let now = artisan_api::now();
    let rows: Vec<Row> = controller
        .archive
        .ids
        .iter()
        .filter_map(|id| host.threads.get(id))
        .map(|thread| {
            let age = now.saturating_sub(thread.updated_at).max(0);
            let age = format!("Last active {} ago", span(age));
            let mut detail = host.projects.get(&thread.project_id).map_or_else(
                || age.clone(),
                |project| format!("{} · {age}", project.name),
            );
            if retention.is_none() {
                detail.push_str(" · Kept indefinitely");
            }
            Row {
                id: thread.id.clone(),
                title: thread.title.clone().unwrap_or_else(|| "New thread".into()),
                icon: host.project_icon(&thread.project_id),
                detail,
                restoring: controller.archive.restoring.contains(&thread.id),
            }
        })
        .collect();
    let card = if rows.is_empty() {
        ui::card(
            theme,
            vec![ui::note(
                theme,
                if page.is_loading() {
                    "Loading archived threads…"
                } else {
                    "No archived threads."
                },
            )],
        )
    } else {
        let visible = u8::try_from(rows.len()).map_or(VISIBLE_ROWS, |rows| rows.min(VISIBLE_ROWS));
        let height = f32::from(visible) * row_height(theme);
        let theme = *theme;
        let row_emit = Rc::clone(emit);
        let list = List::new()
            .items(rows)
            .key(|row| format!("archived-{}", row.id))
            .row(move |row| {
                let id = row.id.clone();
                let send = Rc::clone(&row_emit);
                let label = if row.restoring {
                    "Restoring"
                } else {
                    "Unarchive"
                };
                let button = IconButton::new(
                    format!("settings-archived-{}-unarchive", row.id),
                    theme,
                    editor_assets::AssetId::TABLER_ARCHIVE,
                    label,
                    IconButtonSize::Md,
                    Finish::Glass,
                )
                .muted()
                .disabled(row.restoring)
                .on_activate(move |_| send(ViewAction::Unarchive(id.clone())))
                .build();
                let project = row.icon.as_ref().map_or_else(
                    || {
                        IconSize::Md.glyph(
                            editor_assets::AssetId::TABLER_FOLDER,
                            theme.colors.text_muted.to_reference_paint(),
                        )
                    },
                    |image| {
                        aqua_ui::element::Media::new(aqua_core::ImageRef::from((**image).clone()))
                            .width(20.0)
                            .height(20.0)
                            .fit(aqua_core::image::ImageFit::Cover)
                            .build()
                    },
                );
                UiBox::new()
                    .width(Length::Pct(100.0))
                    .spaced(Space::S16)
                    .padded(RowKind::Form.pad().y)
                    .style(Style::new().align(AlignItems::CENTER))
                    .child(project)
                    .child(ui::grow(
                        ui::stack(Space::S2)
                            .child(ui::line(
                                &theme,
                                TextRole::Label,
                                Ink::Plain,
                                row.title.clone(),
                            ))
                            .child(ui::line(
                                &theme,
                                TextRole::Caption,
                                Ink::Muted,
                                row.detail.clone(),
                            ))
                            .build(),
                    ))
                    .child(button)
                    .build()
            })
            .style(
                Style::new()
                    .width(Length::Pct(100.0))
                    .height(Length::Px(height)),
            )
            .estimated_row_height(Length::Px(row_height(&theme)))
            .build()
            .with_key("archived-threads");
        ui::card(&theme, vec![list])
    };
    let mut body = ui::stack(Space::S8).child(card);
    if let Some(problem) = page.error() {
        body = body.child(ui::problem(theme, "archived-page-error", problem).build());
    }
    if !page.at_latest() || page.error().is_some() {
        let send = Rc::clone(emit);
        body = body.child(ui::button(
            theme,
            "archived-latest",
            "Latest archived threads",
            ButtonTone::Quiet,
            page.is_loading(),
            move || send(ViewAction::ArchiveLatest),
        ));
    }
    if page.has_older() {
        let send = Rc::clone(emit);
        body = body.child(ui::button(
            theme,
            "archived-older",
            if page.is_loading() {
                "Loading…"
            } else {
                "Older archived threads"
            },
            ButtonTone::Quiet,
            page.is_loading(),
            move || send(ViewAction::ArchiveOlder),
        ));
    }
    if let Some(problem) = &controller.archive.error {
        body = body.child(ui::problem(theme, "archived-restore-error", problem).build());
    }
    ui::section(
        theme,
        "Archived",
        Some(
            "Archived threads leave the sidebar and stay here until you restore them, unless Cleanup deletes them first.",
        ),
        None,
        body.build(),
    )
}
