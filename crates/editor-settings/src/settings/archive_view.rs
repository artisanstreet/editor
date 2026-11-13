use std::rc::Rc;

use aqua_ui::{
    AlignItems, JustifyContent,
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
    kit::{ButtonTone, Density, Row as KitRow, Space, Spaced, TextRole},
    select::{Select, SelectEntry, SelectMenu},
    theme::ArtisanTheme,
};

const AGE_WIDTH: f32 = 112.0;
const ACTION_WIDTH: f32 = 96.0;
const VISIBLE_ROWS: usize = 8;

fn row_height() -> f32 {
    KitRow::ListItem(Density::Tall).lines() + Space::S2.px() + 2.0 * KitRow::FormRow.pad().y.px()
}

fn column(width: f32, child: Element) -> Element {
    UiBox::new()
        .width(width)
        .style(Style::new().flex_shrink(0.0).justify(JustifyContent::END))
        .child(child)
        .build()
}


fn entries() -> Vec<SelectEntry<Option<u32>>> {
    let choices = artisan_api::setting_decl(THREAD_RETENTION_DAYS_KEY)
        .and_then(|decl| match decl.kind {
            artisan_api::SettingKind::Choice { values } => Some(values),
            _ => None,
        })
        .unwrap_or(&[]);
    choices
        .iter()
        .map(|days| {
            SelectEntry::item(
                *days,
                match days {
                    None => "Never".into(),
                    Some(365) => "After a year".into(),
                    Some(days) => format!("After {days} days"),
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
    theme: ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> SelectMenu<Option<u32>> {
    let change = emit.clone();
    let host_id = host.id.clone();
    let instance = host.instance();
    SelectMenu::new(
        theme,
        Some(selected(host)),
        entries(),
        controller.retention_select.clone(),
        ui::CONTROL_WIDTH,
    )
    .on_change(move |days| change(ViewAction::SetRetention(host_id.clone(), instance, days)))
    .on_open_change(move |open| emit(ViewAction::RetentionMenu(open)))
}

pub fn cleanup(
    controller: &SettingsController,
    host: &HostData,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let change = emit.clone();
    let host_id = host.id.clone();
    let instance = host.instance();
    let select = Select::new(
        "settings-thread-retention",
        *theme,
        Some(selected(host)),
        entries(),
        controller.retention_select.clone(),
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
        (left / 86_400_000, "day")
    } else if left >= 3_600_000 {
        (left / 3_600_000, "hour")
    } else {
        (left / 60_000, "minute")
    };
    if count == 1 {
        format!("1 {unit}")
    } else {
        format!("{count} {unit}s")
    }
}

#[derive(Clone)]
struct Row {
    id: ThreadId,
    title: String,
    project: String,
    age: i64,
    restoring: bool,
}

pub fn archived(
    controller: &SettingsController,
    host: &HostData,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let page = host.thread_page(true);
    let rows: Vec<Row> = controller
        .archive
        .ids
        .iter()
        .filter_map(|id| host.threads.get(id))
        .map(|thread| Row {
            id: thread.id.clone(),
            title: thread.title.clone().unwrap_or_else(|| "New thread".into()),
            project: host
                .projects
                .get(&thread.project_id)
                .map(|project| project.name.clone())
                .unwrap_or_default(),
            age: artisan_api::now() - thread.updated_at,
            restoring: controller.archive.restoring.contains(&thread.id),
        })
        .collect();
    let card = if rows.is_empty() {
        ui::card(
            theme,
            vec![ui::note(
                theme,
                if page.loading {
                    "Loading archived threads…"
                } else {
                    "No archived threads."
                },
            )],
        )
    } else {
        let header = UiBox::new()
            .spaced(Space::S16)
            .padded(KitRow::FormRow.pad().y)
            .style(Style::new().align(AlignItems::CENTER))
            .child(ui::grow(ui::line(theme, TextRole::Caption, Ink::Muted, "Thread")))
            .child(column(
                AGE_WIDTH,
                ui::grow(ui::line(theme, TextRole::Caption, Ink::Muted, "Archived")),
            ))
            .child(UiBox::new().width(ACTION_WIDTH).build())
            .build();
        let height = rows.len().min(VISIBLE_ROWS) as f32 * row_height();
        let theme = *theme;
        let row_emit = emit.clone();
        let list = List::new()
            .items(rows)
            .key(|row| format!("archived-{}", row.id))
            .row(move |row| {
                let id = row.id.clone();
                let send = row_emit.clone();
                let button = ui::button(
                    &theme,
                    format!("settings-archived-{}-unarchive", row.id),
                    if row.restoring {
                        "Restoring"
                    } else {
                        "Unarchive"
                    },
                    ButtonTone::Neutral,
                    row.restoring,
                    move || send(ViewAction::Unarchive(id.clone())),
                );
                UiBox::new()
                    .width(Length::Pct(100.0))
                    .spaced(Space::S16)
                    .padded(KitRow::FormRow.pad().y)
                    .style(Style::new().align(AlignItems::CENTER))
                    .child(ui::grow(
                        ui::stack(Space::S2)
                            .child(ui::line(&theme, TextRole::Label, Ink::Plain, row.title.clone()))
                            .child(ui::line(
                                &theme,
                                TextRole::Caption,
                                Ink::Muted,
                                row.project.clone(),
                            ))
                            .build(),
                    ))
                    .child(column(
                        AGE_WIDTH,
                        ui::grow(ui::line(
                            &theme,
                            TextRole::Label,
                            Ink::Muted,
                            format!("{} ago", span(row.age)),
                        )),
                    ))
                    .child(column(ACTION_WIDTH, button))
                    .build()
            })
            .style(
                Style::new()
                    .width(Length::Pct(100.0))
                    .height(Length::Px(height)),
            )
            .estimated_row_height(Length::Px(row_height()))
            .build()
            .with_key("archived-threads");
        ui::card(&theme, vec![header, list])
    };
    let mut body = ui::stack(Space::S8).child(card);
    if let Some(error) = &page.error {
        body = body.child(ui::error_text(theme, error));
    }
    if !page.at_latest || page.error.is_some() {
        let send = emit.clone();
        body = body.child(ui::button(
            theme,
            "archived-latest",
            "Latest archived threads",
            ButtonTone::Quiet,
            page.loading,
            move || send(ViewAction::ArchiveLatest),
        ));
    }
    if page.next.is_some() {
        let send = emit.clone();
        body = body.child(ui::button(
            theme,
            "archived-older",
            if page.loading {
                "Loading…"
            } else {
                "Older archived threads"
            },
            ButtonTone::Quiet,
            page.loading,
            move || send(ViewAction::ArchiveOlder),
        ));
    }
    if let Some(error) = &controller.archive.error {
        body = body.child(ui::error_text(theme, error));
    }
    ui::section(
        theme,
        "Archived",
        Some("Archived threads leave the sidebar. They stay here until you restore them."),
        None,
        body.build(),
    )
}
