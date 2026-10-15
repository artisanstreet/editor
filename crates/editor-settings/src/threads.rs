//! Archived thread index, refreshed from host metadata and rendered as visible rows.

use crate::Settings;
use crate::page;
use artisan_api::{ThreadId, Timestamp, UpdateThread};
use editor_core::{HostData, HostId, SettingsPage};
use editor_ui::button::{Button, ButtonVariant};
use editor_ui::page::{PageBlock, card, error_text, header, row, section_shell};
use editor_ui::theme::ArtisanTheme;
use gpui::{
    AnyElement, App, Context, Div, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Styled as _, div, px, uniform_list,
};
use std::{collections::HashSet, rc::Rc};

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
    host: Option<HostId>,
    generation: u64,
    restoring: HashSet<ThreadId>,
    error: Option<String>,
    ids: Rc<Vec<ThreadId>>,
    dirty: bool,
}

impl ArchiveForm {
    pub(crate) fn invalidate(&mut self) {
        self.dirty = true;
    }
    pub(crate) fn sync(&mut self, host: Option<&Entity<HostData>>, cx: &App) {
        let host_id = host.map(|host| host.read(cx).id.clone());
        if self.host != host_id {
            self.host = host_id;
            self.dirty = true;
            self.generation += 1;
            self.restoring.clear();
            self.error = None;
        }
        if !self.dirty {
            return;
        }
        self.dirty = false;
        self.ids = Rc::new(
            host.map(|host| {
                let data = host.read(cx);
                data.thread_page(true)
                    .ids
                    .iter()
                    .filter(|id| data.threads.get(*id).is_some_and(|thread| thread.archived))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default(),
        );
    }
}

impl Settings {
    fn unarchive(&mut self, host: &Entity<HostData>, thread: ThreadId, cx: &mut Context<Self>) {
        if self.archive.restoring.contains(&thread) {
            return;
        }
        let client = host.read(cx).client.clone();
        let host_id = host.read(cx).id.clone();
        let generation = self.archive.generation;
        self.archive.restoring.insert(thread.clone());
        self.archive.error = None;
        cx.notify();
        #[cfg(test)]
        let completion_gate = self.completion_gate.clone();
        cx.spawn(async move |this, cx| {
            let update = UpdateThread {
                archived: Some(false),
                ..UpdateThread::default()
            };
            let result = client.update_thread(&thread, &update).await;
            #[cfg(test)]
            if let Some(gate) = completion_gate {
                gate.wait().await;
            }
            let _ = this.update(cx, |this, cx| {
                if this.archive.host.as_ref() != Some(&host_id)
                    || this.archive.generation != generation
                {
                    return;
                }
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
) -> PageBlock {
    let muted = |text: &str| {
        div()
            .w_full()
            .py(px(16.0))
            .text_sm()
            .text_color(theme.colors.muted_foreground.to_paint())
            .child(text.to_owned())
            .into_any_element()
    };
    let ids = settings.archive.ids.clone();
    let body = if ids.is_empty() {
        card(
            theme,
            vec![muted(if host.read(cx).thread_page(true).loading {
                "Loading archived threads…"
            } else {
                "No archived threads."
            })],
        )
    } else {
        let this = cx.entity().downgrade();
        let host = host.clone();
        let theme = *theme;
        let count = ids.len();
        let list = uniform_list("archived-threads", count, move |range, _, cx| {
            this.update(cx, |settings, cx| {
                let now = artisan_api::now();
                range
                    .filter_map(|index| {
                        let data = host.read(cx);
                        let thread = data.threads.get(&ids[index])?;
                        let id = thread.id.clone();
                        let title = thread
                            .title
                            .clone()
                            .unwrap_or_else(|| "New thread".to_owned());
                        let project = data
                            .projects
                            .get(&thread.project_id)
                            .map(|project| project.name.clone())
                            .unwrap_or_default();
                        let age = now - thread.updated_at;
                        Some(archived_row(
                            settings, &host, &theme, id, title, project, age, cx,
                        ))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
        })
        .h(px((count.min(8) * 68) as f32))
        .w_full();
        card(
            &theme,
            vec![
                archive_header(&theme).into_any_element(),
                list.into_any_element(),
            ],
        )
    };
    let mut body = div().flex().flex_col().child(body);
    let page = host.read(cx).thread_page(true);
    let loading = page.loading;
    let next = page.next.is_some();
    let older = !page.at_latest;
    let error = page.error.clone();
    if let Some(error) = error {
        body = body.child(error_text(theme, error));
    }
    if older || host.read(cx).thread_page(true).error.is_some() {
        let host = host.clone();
        body = body.child(
            Button::text("archived-latest", *theme, "Latest archived threads")
                .disabled(loading)
                .variant(ButtonVariant::Ghost)
                .on_activate(move |_, _, cx| {
                    host.update(cx, |host, cx| {
                        host.load_threads(true, editor_core::ThreadPageDirection::Latest, cx)
                    })
                }),
        );
    }
    if next {
        let host = host.clone();
        body = body.child(
            Button::text(
                "archived-older",
                *theme,
                if loading {
                    "Loading…"
                } else {
                    "Older archived threads"
                },
            )
            .disabled(loading)
            .variant(ButtonVariant::Ghost)
            .on_activate(move |_, _, cx| {
                host.update(cx, |host, cx| {
                    host.load_threads(true, editor_core::ThreadPageDirection::Older, cx)
                })
            }),
        );
    }
    if let Some(error) = &settings.archive.error {
        body = body.child(error_text(theme, error.clone()));
    }
    section_shell(
        theme,
        "Archived",
        Some("Archived threads leave the sidebar. They stay here until you restore them."),
        None,
        body,
    )
    .debug_selector("settings-section-archived")
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
fn render_retention(settings: &Settings, theme: &ArtisanTheme) -> PageBlock {
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
        "Retention",
        Some("Retention controls are not yet connected in the native app."),
        None,
        body,
    )
    .debug_selector("settings-section-retention")
}

fn render_titles(settings: &Settings, theme: &ArtisanTheme) -> PageBlock {
    section_shell(
        theme,
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
.debug_selector("settings-section-thread-titles")
}

fn render_usage_recovery(settings: &Settings, theme: &ArtisanTheme) -> PageBlock {
    section_shell(
        theme,
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
.debug_selector("settings-section-usage-recovery")
}

fn render_agent_names(_settings: &Settings, theme: &ArtisanTheme) -> PageBlock {
    let select = Some(
        editor_ui::select::Select::new(
            "settings-select-agent-names",
            *theme,
            Some(AGENT_NAME_DATASETS[0].0),
            AGENT_NAME_DATASETS
                .iter()
                .map(|(id, label, description)| {
                    editor_ui::select::SelectEntry::item(*id, format!("{label} — {description}"))
                })
                .collect(),
        )
        .disabled(true)
        .label("Agent name set")
        .debug_selector("settings-select-agent-names")
        .into_any_element(),
    );
    section_shell(
        theme,
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
    .debug_selector("settings-section-agents")
}

#[cfg(test)]
mod delayed_restore_tests {
    use super::*;
    use crate::tests::{CompletionGate, SettingsFixture, finish_gate, wait_gate};
    use gpui::TestAppContext;
    #[gpui::test]
    fn old_archive_failure_cannot_clear_new_hosts_restoring_state(cx: &mut TestAppContext) {
        let fixture = SettingsFixture::new(cx);
        let (settings, cx) = fixture.open(cx, SettingsPage::Threads);
        let gate = CompletionGate::new();
        let thread = ThreadId::new();
        settings.update(cx, |settings, cx| {
            settings.completion_gate = Some(gate.clone());
            let host = settings.selected_host(cx).unwrap();
            settings.unarchive(&host, thread.clone(), cx);
        });
        wait_gate(cx, &gate);
        fixture.select(1, SettingsPage::Threads, cx);
        settings.update(cx, |settings, _| {
            settings.archive.restoring.insert(thread.clone());
            settings.archive.error = Some("B owns this error".into());
        });
        finish_gate(cx, &gate);
        settings.read_with(cx, |settings, _| {
            assert_eq!(settings.archive.host.as_ref(), Some(&fixture.hosts[1]));
            assert!(settings.archive.restoring.contains(&thread));
            assert_eq!(settings.archive.error.as_deref(), Some("B owns this error"));
        });
    }
}
