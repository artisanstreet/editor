//! Ctrl+K: jump to a thread, a screen or a host.

use editor_core::{Core, Screen, SettingsPage};
use editor_ui::text_editor::{TextEditor, TextEditorEvent};
use futures::StreamExt as _;
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, Focusable as _, InteractiveElement as _,
    IntoElement, KeyBinding, ParentElement as _, Render, Styled as _, Window, div, px,
};

gpui::actions!(palette, [Dismiss, Up, Down]);

const KEY_CONTEXT: &str = "Palette";
const SHOWN: usize = 12;

pub fn bind_keys(cx: &mut App) {
    let context = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("escape", Dismiss, context),
        KeyBinding::new("up", Up, context),
        KeyBinding::new("down", Down, context),
    ]);
}

pub enum PaletteEvent {
    Dismissed,
}

#[derive(Clone)]
struct Entry {
    label: String,
    detail: String,
    screen: Screen,
}

// Sequences belong to one Forge. Across hosts, use activity time with a stable
// identity tie-break; only these bounded candidate rows are materialized.
type Recency = (
    artisan_api::Timestamp,
    editor_core::HostId,
    artisan_api::ThreadId,
);
fn insert_thread(
    entries: &mut Vec<(Recency, Entry)>,
    thread: &artisan_api::Thread,
    host: &editor_core::HostId,
    detail: &str,
    limit: usize,
) {
    let key = (thread.activity_at, host.clone(), thread.id.clone());
    let index = entries.partition_point(|(existing, _)| existing > &key);
    if index >= limit {
        return;
    }
    entries.insert(
        index,
        (
            key,
            Entry {
                label: thread
                    .title
                    .clone()
                    .unwrap_or_else(|| "Untitled thread".into()),
                detail: detail.to_owned(),
                screen: Screen::Thread {
                    host: host.clone(),
                    thread: thread.id.clone(),
                },
            },
        ),
    );
    entries.truncate(limit);
}

pub struct Palette {
    core: Entity<Core>,
    query: Entity<TextEditor>,
    highlighted: usize,
    entries: Vec<Entry>,
    _hosts: Vec<gpui::Subscription>,
    search: Option<Vec<Entry>>,
    search_generation: u64,
    search_task: gpui::Task<()>,
    search_error: Option<String>,
}

impl EventEmitter<PaletteEvent> for Palette {}

impl Palette {
    pub fn new(core: Entity<Core>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query =
            cx.new(|cx| TextEditor::new("", "Search threads and commands", cx).single_line());
        cx.subscribe_in(&query, window, |this, _, event, _, cx| match event {
            TextEditorEvent::Changed => {
                this.start_search(cx);
                this.refresh(cx);
                cx.notify();
            }
            TextEditorEvent::Submitted => this.activate(cx),
        })
        .detach();
        editor_ui::overlay::begin("command-palette", window, cx);
        window.focus(&query.read(cx).focus_handle(cx), cx);
        let hosts: Vec<_> = core
            .read(cx)
            .hosts()
            .map(|(_, host)| host.clone())
            .collect();
        let subscriptions = hosts
            .iter()
            .map(|host| {
                cx.subscribe(host, |this, _, event: &editor_core::HostEvent, cx| {
                    if matches!(
                        event,
                        editor_core::HostEvent::Changed
                            | editor_core::HostEvent::Reset
                            | editor_core::HostEvent::ThreadChanged { .. }
                    ) {
                        this.refresh(cx);
                        cx.notify();
                    }
                })
            })
            .collect();
        let mut palette = Self {
            core,
            query,
            highlighted: 0,
            entries: Vec::new(),
            _hosts: subscriptions,
            search: None,
            search_generation: 0,
            search_task: gpui::Task::ready(()),
            search_error: None,
        };
        palette.refresh(cx);
        palette
    }

    fn entries(&self, cx: &App) -> Vec<Entry> {
        let core = self.core.read(cx);
        let query = self.query.read(cx).text().trim().to_lowercase();
        let mut entries = Vec::new();
        let active = core.active_host().map(|host| host.read(cx).id.clone());
        if let Some(host) = active.clone() {
            entries.push(Entry {
                label: "New thread".into(),
                detail: "Ctrl+N".into(),
                screen: Screen::NewThread {
                    host,
                    project: None,
                },
            });
        }
        for (label, page) in [
            ("Settings", SettingsPage::Appearance),
            ("Global prompt", SettingsPage::GlobalPrompt),
            ("Threads", SettingsPage::Threads),
            ("Appearance", SettingsPage::Appearance),
            ("Notifications", SettingsPage::Notifications),
            ("About", SettingsPage::About),
            ("Engines", SettingsPage::Engines),
            ("Skills and MCP servers", SettingsPage::Extensions),
            ("Hosts and devices", SettingsPage::Hosts),
        ] {
            entries.push(Entry {
                label: label.into(),
                detail: "Settings".into(),
                screen: Screen::Settings(page),
            });
        }
        for (config, _) in core.hosts() {
            if Some(&config.id) != active.as_ref() {
                entries.push(Entry {
                    label: format!("Switch to {}", config.name),
                    detail: "Host".into(),
                    screen: Screen::NewThread {
                        host: config.id.clone(),
                        project: None,
                    },
                });
            }
        }
        entries.retain(|entry| {
            query.is_empty()
                || entry.label.to_lowercase().contains(&query)
                || entry.detail.to_lowercase().contains(&query)
        });
        if let Some(search) = &self.search {
            entries.extend(
                search
                    .iter()
                    .filter(|candidate| {
                        !entries.iter().any(|entry| entry.screen == candidate.screen)
                    })
                    .cloned()
                    .collect::<Vec<_>>(),
            );
            entries.truncate(SHOWN);
        }
        let room = SHOWN.saturating_sub(entries.len());
        let mut recent = Vec::with_capacity(room + 1);
        if room > 0 {
            for (config, host) in core.hosts() {
                let data = host.read(cx);
                for thread in data
                    .threads
                    .values()
                    .filter(|thread| !thread.archived && thread.parent_thread_id.is_none())
                {
                    if entries.iter().any(|entry| matches!(&entry.screen, Screen::Thread { host, thread: id } if host == &config.id && id == &thread.id)) {
                        continue;
                    }
                    let label = thread.title.as_deref().unwrap_or("Untitled thread");
                    let detail = data
                        .projects
                        .get(&thread.project_id)
                        .map_or("", |project| project.name.as_str());
                    if !query.is_empty()
                        && !label.to_lowercase().contains(&query)
                        && !detail.to_lowercase().contains(&query)
                    {
                        continue;
                    }
                    insert_thread(&mut recent, thread, &config.id, detail, room);
                }
            }
        }
        entries.extend(recent.into_iter().map(|(_, entry)| entry));
        entries.truncate(SHOWN);
        entries
    }

    fn start_search(&mut self, cx: &mut Context<Self>) {
        self.search_generation = self.search_generation.wrapping_add(1);
        self.search_task = gpui::Task::ready(());
        self.search = None;
        self.search_error = None;
        let query = self.query.read(cx).text().trim().to_owned();
        if query.is_empty() {
            return;
        }
        let generation = self.search_generation;
        let hosts: Vec<_> = self
            .core
            .read(cx)
            .hosts()
            .filter(|(_, host)| host.read(cx).is_live())
            .map(|(config, host)| {
                (
                    config.id.clone(),
                    config.name.clone(),
                    host.read(cx).client.clone(),
                )
            })
            .collect();
        if hosts.is_empty() {
            self.search_error = Some("Connect to a Forge to search older threads.".into());
            return;
        }
        self.search_task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(150))
                .await;
            let mut responses =
                futures::stream::iter(hosts.into_iter().map(|(host, name, client)| {
                    let query = query.clone();
                    async move {
                        let result = client
                            .threads(&artisan_api::ThreadsQuery {
                                query: Some(query),
                                limit: Some(12),
                                ..Default::default()
                            })
                            .await;
                        (host, name, result)
                    }
                }))
                .buffer_unordered(4);
            let mut results = Vec::new();
            let mut failures = 0;
            while let Some((host, name, result)) = responses.next().await {
                match result {
                    Ok(page) => {
                        for thread in page.threads {
                            insert_thread(&mut results, &thread, &host, &name, SHOWN);
                        }
                    }
                    Err(_) => failures += 1,
                }
                // Publish each Forge independently; one slow host does not hide other results.
                let entries = results.iter().map(|(_, entry)| entry.clone()).collect();
                if this
                    .update(cx, |this, cx| {
                        if this.search_generation != generation {
                            return;
                        }
                        this.search = Some(entries);
                        this.search_error = (failures > 0)
                            .then(|| format!("Search could not reach {failures} Forge(s)."));
                        this.refresh(cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
        });
    }
    fn refresh(&mut self, cx: &App) {
        let selected = self
            .entries
            .get(self.highlighted)
            .map(|entry| entry.screen.clone());
        self.entries = self.entries(cx);
        self.highlighted = selected
            .and_then(|screen| self.entries.iter().position(|entry| entry.screen == screen))
            .unwrap_or(0);
    }

    fn activate(&mut self, cx: &mut Context<Self>) {
        if let Some(entry) = self.entries.get(self.highlighted).cloned() {
            self.core
                .update(cx, |core, cx| core.navigate(entry.screen, cx));
        }
        cx.emit(PaletteEvent::Dismissed);
    }
}

impl Render for Palette {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = editor_ui::theme::ArtisanTheme::default();
        let entries = self.entries.clone();
        self.highlighted = self.highlighted.min(entries.len().saturating_sub(1));
        let rows = entries.into_iter().enumerate().map(|(index, entry)| {
            editor_ui::action_row::ActionRow::new(index, theme, entry.label)
                .secondary(entry.detail)
                .selected(index == self.highlighted)
                .on_activate(cx.listener(move |this, _, _, cx| {
                    this.core
                        .update(cx, |core, cx| core.navigate(entry.screen.clone(), cx));
                    cx.emit(PaletteEvent::Dismissed);
                }))
        });
        let content = div()
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|_, _: &Dismiss, _, cx| cx.emit(PaletteEvent::Dismissed)))
            .on_action(cx.listener(|this, _: &Up, _, cx| {
                this.highlighted = this.highlighted.saturating_sub(1);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Down, _, cx| {
                this.highlighted = (this.highlighted + 1).min(this.entries.len().saturating_sub(1));
                cx.notify();
            }))
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(self.query.clone())
            .children(
                self.search_error
                    .clone()
                    .map(|error| div().text_xs().child(error)),
            )
            .children(rows);
        editor_ui::dialog::Dialog::new(
            "command-palette",
            editor_ui::theme::ArtisanTheme::default(),
            "Search threads and commands",
            content,
        )
        .on_dismiss({
            let owner = cx.entity().downgrade();
            move |_, _, cx| {
                let _ = owner.update(cx, |_, cx| cx.emit(PaletteEvent::Dismissed));
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use artisan_api::{EngineConfig, EngineId, ProjectId, Thread, ThreadId};
    use editor_core::HostId;

    fn thread(id: &str, activity_at: i64, activity_seq: u64) -> Thread {
        Thread {
            id: ThreadId::from(id),
            project_id: ProjectId::from("project"),
            title: Some(format!("needle-{id}")),
            config: EngineConfig::new(EngineId::Codex),
            activity_seq,
            activity_at,
            read_seq: 0,
            marked_unread: false,
            settled_seq: 0,
            seq: activity_seq,
            created_at: 1,
            updated_at: 999_999,
            archived: false,
            parent_thread_id: None,
        }
    }

    #[test]
    fn search_merge_uses_activity_time_and_stable_host_identity() {
        let mut results = Vec::new();
        let busy = HostId("busy".into());
        let quiet = HostId("quiet".into());
        insert_thread(&mut results, &thread("old", 100, 900_000), &busy, "Busy", 2);
        insert_thread(&mut results, &thread("new", 200, 2), &quiet, "Quiet", 2);
        insert_thread(
            &mut results,
            &thread("oldest", 50, 9_000_000),
            &busy,
            "Busy",
            2,
        );
        assert_eq!(
            results
                .iter()
                .map(|(_, entry)| entry.label.as_str())
                .collect::<Vec<_>>(),
            ["needle-new", "needle-old"]
        );
        // Equal timestamps and IDs still have a total order across hosts.
        insert_thread(
            &mut results,
            &thread("new", 200, 99_000_000),
            &busy,
            "Busy",
            2,
        );
        assert_eq!(results[0].0.1, quiet);
        assert_eq!(results[1].0.1, busy);
    }

    #[cfg(feature = "capture")]
    #[gpui::test]
    fn local_palette_recency_ignores_another_forges_sequence_range(cx: &mut gpui::TestAppContext) {
        cx.executor().allow_parking();
        let dir = tempfile::tempdir().unwrap();
        let core = cx.new(|cx| {
            Core::new(
                editor_core::EditorFiles {
                    dir: dir.path().to_owned(),
                },
                cx,
            )
            .unwrap()
        });
        let fingerprint = core.read_with(cx, |core, _| {
            artisan_api::tls::fingerprint(core.identity.as_ref().unwrap().certificate_der().unwrap().as_ref())
        });
        for (host, item) in [
            ("busy", thread("old", 100, 900_000)),
            ("quiet", thread("new", 200, 2)),
        ] {
            let id = HostId(host.into());
            core.update(cx, |core, cx| {
                core.add_host(
                    editor_core::HostConfig {
                        id: id.clone(),
                        name: host.into(),
                        address: "127.0.0.1:1".into(),
                        fingerprint: fingerprint.clone(),
                        kind: editor_core::HostKind::Remote,
                    },
                    cx,
                )
                .unwrap()
            });
            let host = core.read_with(cx, |core, _| core.host(&id).unwrap().clone());
            host.update(cx, |host, _| {
                host.threads.insert(item.id.clone(), item);
            });
        }
        let query = cx.new(|cx| TextEditor::new("needle", "", cx));
        let palette = cx.new(|_| Palette {
            core,
            query,
            highlighted: 0,
            entries: Vec::new(),
            _hosts: Vec::new(),
            search: None,
            search_generation: 0,
            search_task: gpui::Task::ready(()),
            search_error: None,
        });
        let labels = palette.read_with(cx, |palette, cx| {
            palette
                .entries(cx)
                .into_iter()
                .map(|entry| entry.label)
                .collect::<Vec<_>>()
        });
        assert_eq!(labels, ["needle-new", "needle-old"]);
    }
}
