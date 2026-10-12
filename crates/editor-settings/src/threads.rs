//! Threads (`threads.svelte` with `archived_threads`, `thread-titles`,
//! `usage-recovery` and `agent-names`), as the previous Editor painted it.
//!
//! The archive is live: the host's archived threads, most recently touched
//! first, each with a button that brings it back to the sidebar. Forge
//! never erases an archived thread, so the table has no deletion column.
//! Retention, titles, usage recovery and the agent name set are painted
//! disabled with the previous Editor's values.

use crate::Settings;
use crate::chrome::{card, error_text, header, row, section_shell};
use crate::page;
use artisan_api::{ThreadId, Timestamp, UpdateThread};
use editor_core::{HostData, SettingsPage};
use editor_ui::button::ButtonVariant;
use editor_ui::native_select::{NativeSelect, NativeSelectOption};
use editor_ui::theme::ArtisanTheme;
use gpui::{
    AnyElement, Context, Div, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Styled as _, div, px,
};
use std::collections::HashSet;

/// Widths of the table's fixed columns.
const ARCHIVED_COLUMN_PX: f32 = 112.0;
const ACTION_COLUMN_PX: f32 = 96.0;
/// The previous Editor's painted inactivity threshold.
const RETENTION_DAYS: u16 = 30;
const AGENT_NAME_DATASETS: [(&str, &str, &str); 2] = [
    ("norwegian", "Norwegian", "Norwegian feminine given names."),
    ("british", "British", "British feminine given names."),
];

#[derive(Default)]
pub(crate) struct ArchiveForm {
    restoring: HashSet<ThreadId>,
    error: Option<String>,
}

impl Settings {
    fn unarchive(&mut self, host: &Entity<HostData>, thread: ThreadId, cx: &mut Context<Self>) {
        let client = host.read(cx).client.clone();
        self.archive.restoring.insert(thread.clone());
        self.archive.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let update = UpdateThread {
                archived: Some(false),
                ..UpdateThread::default()
            };
            let result = client.update_thread(&thread, &update).await;
            let _ = this.update(cx, |this, cx| {
                this.archive.restoring.remove(&thread);
                if let Err(error) = result {
                    this.archive.error = Some(format!("Couldn't unarchive the thread: {error:#}"));
                }
                cx.notify();
            });
        })
        .detach();
    }
}

pub(crate) fn render(
    settings: &Settings,
    host: &Entity<HostData>,
    theme: &ArtisanTheme,
    cx: &mut Context<Settings>,
) -> Div {
    div()
        .flex()
        .flex_col()
        .child(header(
            theme,
            page::label(SettingsPage::Threads),
            page::description(SettingsPage::Threads),
        ))
        .child(render_archived(settings, host, theme, cx))
        .child(render_retention(settings, theme))
        .child(render_titles(settings, theme))
        .child(render_usage_recovery(settings, theme))
        .child(render_agent_names(settings, theme))
}

/// The archive table.
fn render_archived(
    settings: &Settings,
    host: &Entity<HostData>,
    theme: &ArtisanTheme,
    cx: &mut Context<Settings>,
) -> Div {
    let muted = |text: &str| {
        div()
            .w_full()
            .py(px(16.0))
            .text_sm()
            .text_color(theme.colors.muted_foreground.to_paint())
            .child(text.to_owned())
            .into_any_element()
    };
    let data = host.read(cx);
    let mut archived: Vec<(ThreadId, String, String, Timestamp)> = data
        .threads
        .values()
        .filter(|thread| thread.archived)
        .map(|thread| {
            let title = thread
                .title
                .clone()
                .unwrap_or_else(|| "New thread".to_owned());
            let project = data
                .projects
                .get(&thread.project_id)
                .map(|project| project.name.clone())
                .unwrap_or_default();
            (thread.id.clone(), title, project, thread.updated_at)
        })
        .collect();
    archived.sort_by(|a, b| b.3.cmp(&a.3).then_with(|| a.0.cmp(&b.0)));
    let body = if archived.is_empty() {
        card(theme, vec![muted("No archived threads.")])
    } else {
        let now = artisan_api::now();
        let mut blocks = vec![archive_header(theme).into_any_element()];
        for (id, title, project, archived_at) in archived {
            blocks.push(archived_row(
                settings,
                host,
                theme,
                id,
                title,
                project,
                now - archived_at,
                cx,
            ));
        }
        card(theme, blocks)
    };
    let mut section = section_shell(
        theme,
        "archived",
        "Archived",
        Some("Archived threads leave the sidebar. They stay here until you restore them."),
        None,
        body,
    );
    if let Some(error) = &settings.archive.error {
        section = section.child(error_text(theme, error.clone()));
    }
    section
}

#[allow(clippy::too_many_arguments)]
fn archived_row(
    settings: &Settings,
    host: &Entity<HostData>,
    theme: &ArtisanTheme,
    thread: ThreadId,
    title: String,
    project: String,
    age: Timestamp,
    cx: &mut Context<Settings>,
) -> AnyElement {
    let selector = format!("settings-archived-{thread}");
    let restoring = settings.archive.restoring.contains(&thread);
    let host = host.clone();
    let button = settings.button(
        SharedString::from(format!("{selector}-unarchive")),
        if restoring { "Restoring" } else { "Unarchive" },
        ButtonVariant::Outline,
        restoring,
        cx.listener(move |this, _, _, cx| this.unarchive(&host, thread.clone(), cx)),
    );
    div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(16.0))
        .py(px(10.0))
        .debug_selector(move || selector.clone())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .text_color(theme.colors.foreground.to_paint())
                        .child(title),
                )
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(theme.colors.muted_foreground.to_paint())
                        .child(project),
                ),
        )
        .child(
            div()
                .w(px(ARCHIVED_COLUMN_PX))
                .flex_shrink_0()
                .text_sm()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(format!("{} ago", span(age))),
        )
        .child(
            div()
                .w(px(ACTION_COLUMN_PX))
                .flex_shrink_0()
                .flex()
                .justify_end()
                .child(button),
        )
        .into_any_element()
}

/// The table's column headings.
fn archive_header(theme: &ArtisanTheme) -> Div {
    let heading = |text: &'static str| {
        div()
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.colors.muted_foreground.to_paint())
            .child(text)
    };
    div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(16.0))
        .py(px(8.0))
        .debug_selector(|| "settings-archived-header".to_owned())
        .child(div().flex_1().min_w_0().child(heading("Thread")))
        .child(
            div()
                .w(px(ARCHIVED_COLUMN_PX))
                .flex_shrink_0()
                .child(heading("Archived")),
        )
        .child(div().w(px(ACTION_COLUMN_PX)).flex_shrink_0())
}

/// A span in its largest whole unit, at least a minute: "3 days".
fn span(millis: Timestamp) -> String {
    const MINUTE: i64 = 60 * 1_000;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    let left = millis.max(MINUTE);
    let (count, unit) = if left >= DAY {
        (left / DAY, "day")
    } else if left >= HOUR {
        (left / HOUR, "hour")
    } else {
        (left / MINUTE, "minute")
    };
    if count == 1 {
        format!("1 {unit}")
    } else {
        format!("{count} {unit}s")
    }
}

/// Retention: Forge never erases a thread, so erasing stays off.
fn render_retention(settings: &Settings, theme: &ArtisanTheme) -> Div {
    let body = card(
        theme,
        vec![
            row(
                theme,
                "Erase inactive threads",
                "Permanently erases a thread \u{2014} conversation, checkpoints, and lineage \u{2014} once it has been untouched for the configured number of days. This is deletion, not archival.",
                Some(settings.switch("settings-switch-retention", false, None)),
            )
            .into_any_element(),
            row(
                theme,
                "Inactivity threshold",
                "Days a thread must be untouched before it is erased. Between 1 and 3650.",
                Some(
                    div()
                        .text_sm()
                        .text_color(theme.colors.muted_foreground.to_paint())
                        .child(format!("{RETENTION_DAYS} days"))
                        .into_any_element(),
                ),
            )
            .into_any_element(),
        ],
    );
    section_shell(
        theme,
        "retention",
        "Retention",
        Some("Retention controls are not yet connected in the native app."),
        None,
        body,
    )
}

fn render_titles(settings: &Settings, theme: &ArtisanTheme) -> Div {
    section_shell(
        theme,
        "thread-titles",
        "Titles",
        Some("Title choices are not yet configurable in the native app."),
        None,
        card(
            theme,
            vec![
                row(
                    theme,
                    "Summary titles",
                    "Name threads with the harness's own generated summary. When off, a thread is named by the latest message you sent. A rename of your own always wins, and threads without a summary keep the message title.",
                    Some(settings.switch("settings-switch-titles", true, None)),
                )
                .into_any_element(),
            ],
        ),
    )
}

fn render_usage_recovery(settings: &Settings, theme: &ArtisanTheme) -> Div {
    section_shell(
        theme,
        "usage-recovery",
        "Usage recovery",
        Some("Usage-recovery choices are not yet configurable in the native app."),
        None,
        card(
            theme,
            vec![
                row(
                    theme,
                    "Automatically continue after usage resets",
                    "New turns interrupted by a provider limit continue once Forge verifies the usage window has reset. You can still change this on each interruption card.",
                    Some(settings.switch("settings-switch-recovery", false, None)),
                )
                .into_any_element(),
            ],
        ),
    )
}

fn render_agent_names(settings: &Settings, theme: &ArtisanTheme) -> Div {
    let options = AGENT_NAME_DATASETS
        .into_iter()
        .filter_map(|(id, label, description)| {
            NativeSelectOption::new(id, format!("{label} \u{2014} {description}")).ok()
        })
        .collect::<Vec<_>>();
    let select = NativeSelect::new(
        "settings-select-agent-names",
        settings.control_focus.clone(),
        *theme,
        AGENT_NAME_DATASETS[0].0,
        options,
    )
    .ok()
    .map(|select| {
        select
            .disabled(true)
            .semantic_label("Agent name set")
            .debug_selector("settings-select-agent-names")
            .into_any_element()
    });
    section_shell(
        theme,
        "agents",
        "Agents",
        Some("The name set is not yet configurable in the native app."),
        None,
        card(
            theme,
            vec![
                row(
                    theme,
                    "Name set",
                    "The catalog Artisan uses when it names a new delegated agent.",
                    select,
                )
                .into_any_element(),
            ],
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::span;

    #[test]
    fn spans_read_in_their_largest_whole_unit() {
        let minute = 60 * 1_000;
        let hour = 60 * minute;
        let day = 24 * hour;
        for (left, expected) in [
            (0, "1 minute"),
            (5 * minute, "5 minutes"),
            (hour, "1 hour"),
            (3 * day + hour, "3 days"),
        ] {
            assert_eq!(span(left), expected);
        }
    }
}
