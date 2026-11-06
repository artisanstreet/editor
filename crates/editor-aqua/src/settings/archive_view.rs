use std::rc::Rc;

use aqua_core::Insets;
use aqua_text::{FontFamily, FontWeight, TextStyle};
use aqua_ui::{
    AlignItems, JustifyContent,
    collection::List,
    element::{Box as UiBox, Element, Text},
    style::{Length, Style},
};
use artisan_api::{THREAD_RETENTION_DAYS_KEY, ThreadId};

use super::{controller::SettingsController, ui, view::ViewAction};
use crate::{
    core::HostData,
    ui::{
        button::{Button, ButtonVariant},
        select::{Select, SelectEntry, SelectMenu},
        theme::ArtisanTheme,
    },
};

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
        144.0,
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
    let control = UiBox::new()
        .width(144.0)
        .style(Style::new().flex_shrink(0.0))
        .child(select)
        .build();
    let row = ui::row(
        theme,
        "Delete inactive threads",
        "Threads nothing has happened in for this long are deleted with their messages and images, archived or not. Working threads, threads waiting on you and threads whose agents are still active are kept.",
        Some(control),
    );
    ui::section(theme, "Cleanup", None, None, ui::card(theme, vec![row]))
}

fn label(theme: &ArtisanTheme, text: impl Into<String>, size: f32, muted: bool) -> Element {
    Text::new(text.into())
        .text_style(TextStyle {
            families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
            size_px: size,
            ..TextStyle::default()
        })
        .gpui_source_color(if muted {
            theme.colors.text_muted.to_reference_text_color()
        } else {
            theme.colors.text.to_reference_text_color()
        })
        .wrap(false)
        .build()
}

fn heading(theme: &ArtisanTheme, text: &str) -> Element {
    Text::new(text)
        .text_style(TextStyle {
            families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
            size_px: 12.0,
            line_height_px: 16.0,
            weight: FontWeight::MEDIUM,
            ..TextStyle::default()
        })
        .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
        .build()
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
            .gap(16.0)
            .style(
                Style::new()
                    .align(AlignItems::CENTER)
                    .padding_insets(Insets {
                        top: 8.0,
                        bottom: 8.0,
                        ..Insets::default()
                    }),
            )
            .child(
                UiBox::new()
                    .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                    .child(heading(theme, "Thread"))
                    .build(),
            )
            .child(
                UiBox::new()
                    .width(112.0)
                    .child(heading(theme, "Archived"))
                    .build(),
            )
            .child(UiBox::new().width(96.0).build())
            .build();
        let height = (rows.len().min(8) * 68) as f32;
        let theme = *theme;
        let row_emit = emit.clone();
        let list = List::new()
            .items(rows)
            .key(|row| format!("archived-{}", row.id))
            .row(move |row| {
                let id = row.id.clone();
                let send = row_emit.clone();
                let button = Button::text(
                    format!("settings-archived-{}-unarchive", row.id),
                    theme,
                    if row.restoring {
                        "Restoring"
                    } else {
                        "Unarchive"
                    },
                )
                .variant(ButtonVariant::Outline)
                .disabled(row.restoring)
                .on_activate(move |_| send(ViewAction::Unarchive(id.clone())))
                .build();
                UiBox::new()
                    .width(Length::Pct(100.0))
                    .gap(16.0)
                    .style(
                        Style::new()
                            .align(AlignItems::CENTER)
                            .padding_insets(Insets {
                                top: 10.0,
                                bottom: 10.0,
                                ..Insets::default()
                            }),
                    )
                    .child(
                        UiBox::new()
                            .layout("column")
                            .gap(2.0)
                            .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                            .child(label(&theme, row.title.clone(), 14.0, false))
                            .child(label(&theme, row.project.clone(), 12.0, true))
                            .build(),
                    )
                    .child(
                        UiBox::new()
                            .width(112.0)
                            .style(Style::new().flex_shrink(0.0))
                            .child(label(&theme, format!("{} ago", span(row.age)), 14.0, true))
                            .build(),
                    )
                    .child(
                        UiBox::new()
                            .width(96.0)
                            .style(Style::new().flex_shrink(0.0).justify(JustifyContent::END))
                            .child(button)
                            .build(),
                    )
                    .build()
            })
            .style(
                Style::new()
                    .width(Length::Pct(100.0))
                    .height(Length::Px(height)),
            )
            .estimated_row_height(Length::Px(68.0))
            .build()
            .with_key("archived-threads");
        ui::card(&theme, vec![header, list])
    };
    let mut body = UiBox::new().layout("column").child(card);
    if let Some(error) = &page.error {
        body = body.child(ui::error_text(theme, error));
    }
    if !page.at_latest || page.error.is_some() {
        let send = emit.clone();
        body = body.child(
            Button::text("archived-latest", *theme, "Latest archived threads")
                .disabled(page.loading)
                .variant(ButtonVariant::Ghost)
                .on_activate(move |_| send(ViewAction::ArchiveLatest))
                .build(),
        );
    }
    if page.next.is_some() {
        let send = emit.clone();
        body = body.child(
            Button::text(
                "archived-older",
                *theme,
                if page.loading {
                    "Loading…"
                } else {
                    "Older archived threads"
                },
            )
            .disabled(page.loading)
            .variant(ButtonVariant::Ghost)
            .on_activate(move |_| send(ViewAction::ArchiveOlder))
            .build(),
        );
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
