//! Archived thread index, refreshed from host metadata and rendered as visible rows.

use crate::Settings;
use crate::page;
use crate::ui::{PageBlock, card, error_text, header, row, section_shell};
use artisan_api::{ThreadId, Timestamp, UpdateThread};
use editor_core::{HostData, HostId, SettingsPage};
use editor_ui::button::{Button, ButtonVariant};
use editor_ui::theme::ArtisanTheme;
use gpui::{
    AnyElement, App, Context, Div, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Styled as _, div, px, uniform_list,
};
use std::{collections::HashSet, rc::Rc};

/// Widths of the table's fixed columns.
const ARCHIVED_COLUMN_PX: f32 = 112.0;
const ACTION_COLUMN_PX: f32 = 96.0;

#[derive(Default)]
pub struct ArchiveForm {
    host: Option<HostId>,
    generation: u64,
    restoring: HashSet<ThreadId>,
    error: Option<String>,
    ids: Rc<[ThreadId]>,
    dirty: bool,
}

impl ArchiveForm {
    pub(crate) const fn invalidate(&mut self) {
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
        self.ids = Rc::from(
            host.map(|host| {
                let data = host.read(cx);
                data.thread_page(true)
                    .ids
                    .iter()
                    .filter(|id| data.threads.get(*id).is_some_and(|thread| thread.archived))
                    .cloned()
                    .collect::<Vec<_>>()
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
            this.update(cx, |this, cx| {
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
            })
            .ok();
        })
        .detach();
    }
}

pub fn render(
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
        .child(render_assistance(settings, host, theme, cx))
        .child(render_archived(settings, host, theme, cx))
}

/// The archive table.
#[expect(
    clippy::too_many_lines,
    reason = "the archive card, its virtual list and its empty states in one place"
)]
fn render_archived(
    settings: &Settings,
    host: &Entity<HostData>,
    theme: &ArtisanTheme,
    cx: &Context<Settings>,
) -> PageBlock {
    let muted = |text: &str| {
        div()
            .w_full()
            .py(px(16.0))
            .text_sm()
            .text_color(theme.colors.text_muted.to_paint())
            .child(text.to_owned())
            .into_any_element()
    };
    let ids = Rc::clone(&settings.archive.ids);
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
        .h(px(f32::from(u16::try_from(count.min(8)).unwrap_or(8) * 68)))
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
                        host.load_threads(true, editor_core::ThreadPageDirection::Latest, cx);
                    });
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
                    host.load_threads(true, editor_core::ThreadPageDirection::Older, cx);
                });
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

#[expect(
    clippy::too_many_arguments,
    reason = "one row's fields, passed straight from the archived thread"
)]
fn archived_row(
    settings: &Settings,
    host: &Entity<HostData>,
    theme: &ArtisanTheme,
    thread: ThreadId,
    title: String,
    project: String,
    age: Timestamp,
    cx: &Context<Settings>,
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
        .debug_selector(move || selector)
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
                        .text_color(theme.colors.text.to_paint())
                        .child(title),
                )
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(theme.colors.text_muted.to_paint())
                        .child(project),
                ),
        )
        .child(
            div()
                .w(px(ARCHIVED_COLUMN_PX))
                .flex_shrink_0()
                .text_sm()
                .text_color(theme.colors.text_muted.to_paint())
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
            .text_color(theme.colors.text_muted.to_paint())
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

/// What Forge asks a thread's engine on the side after each turn, and
/// whether threads take the titles their engines write. All are Forge
/// settings, on unless turned off; the side questions cost a little of the
/// engine's usage each time they run.
fn render_assistance(
    settings: &Settings,
    host: &Entity<HostData>,
    theme: &ArtisanTheme,
    cx: &mut Context<Settings>,
) -> PageBlock {
    let switch = |id: &'static str, key: &'static str, cx: &mut Context<Settings>| {
        let enabled = host
            .read(cx)
            .setting(key)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        let host = host.clone();
        settings.switch(
            id,
            enabled,
            Box::new(move |checked, _, cx| {
                save_switch(&host, key, checked, cx);
            }),
        )
    };
    let suggestions = switch("settings-switch-suggestions", "suggestions", cx);
    let notes = switch("settings-switch-you-should-know", "you_should_know", cx);
    let titles = switch("settings-switch-titles", "summary_titles", cx);
    section_shell(
        theme,
        "Assistance",
        None,
        None,
        card(
            theme,
            vec![
                row(
                    theme,
                    "Suggest my next message",
                    "After each turn, the composer offers what you would most likely send next. It is asked of the thread's own engine and reuses its cached conversation, so each one costs a little of that engine's usage.",
                    Some(suggestions),
                )
                .into_any_element(),
                row(
                    theme,
                    "You should know",
                    "After a turn that changed things \u{2014} edits, migrations, installs, decisions \u{2014} the engine is asked whether there is something you should know but might miss, and the thread shows it as a note. Turns that only looked around are not checked, and checks that find nothing make the next one rarer.",
                    Some(notes),
                )
                .into_any_element(),
                row(
                    theme,
                    "Summary titles",
                    "A thread takes the title its engine writes as it works. Off, it keeps the first line of its first message.",
                    Some(titles),
                )
                .into_any_element(),
            ],
        ),
    )
    .debug_selector("settings-section-assistance")
}

/// Shows the switch's new state at once and saves it to Forge, whose
/// setting event confirms it; a failed save puts it back.
pub(crate) fn save_switch(host: &Entity<HostData>, key: &'static str, checked: bool, cx: &mut App) {
    save_value(host, key, serde_json::Value::Bool(checked), cx)
}

/// Saves a setting to Forge, showing its new state at once; a failed save
/// puts it back.
pub(crate) fn save_value(
    host: &Entity<HostData>,
    key: &'static str,
    value: serde_json::Value,
    cx: &mut App,
) {
    let previous = host.read(cx).setting(key).cloned();
    host.update(cx, |host, cx| {
        host.settings.insert(key.to_owned(), value.clone());
        cx.notify();
    });
    let client = host.read(cx).client.clone();
    let host = host.downgrade();
    cx.spawn(async move |cx| {
        if let Err(error) = client.set_setting(key, &value).await {
            tracing::warn!(key, "saving a Forge setting: {error:#}");
            host.update(cx, |host, cx| {
                match previous {
                    Some(previous) => host.settings.insert(key.to_owned(), previous),
                    None => host.settings.remove(key),
                };
                cx.notify();
            })
            .ok();
        }
    })
    .detach();
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
