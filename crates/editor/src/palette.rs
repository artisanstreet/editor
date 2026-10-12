//! Ctrl+K: jump to a thread, a screen or a host.

use editor_core::{Core, HostId, Screen, SettingsPage};
use editor_ui::text_editor::{TextEditor, TextEditorEvent};
use editor_ui::theme::DesktopTheme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, Focusable as _, InteractiveElement as _,
    IntoElement, KeyBinding, ParentElement as _, Render, StatefulInteractiveElement as _,
    Styled as _, Window, div, px,
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

pub struct Palette {
    core: Entity<Core>,
    query: Entity<TextEditor>,
    highlighted: usize,
}

impl EventEmitter<PaletteEvent> for Palette {}

impl Palette {
    pub fn new(core: Entity<Core>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query =
            cx.new(|cx| TextEditor::new("", "Search threads and commands", cx).single_line());
        cx.subscribe_in(&query, window, |this, _, event, _, cx| match event {
            TextEditorEvent::Changed => {
                this.highlighted = 0;
                cx.notify();
            }
            TextEditorEvent::Submitted => this.activate(cx),
        })
        .detach();
        window.focus(&query.read(cx).focus_handle(cx), cx);
        Self {
            core,
            query,
            highlighted: 0,
        }
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
            ("Settings", SettingsPage::Models),
            ("Global prompt", SettingsPage::GlobalPrompt),
            ("Threads", SettingsPage::Threads),
            ("Appearance", SettingsPage::Appearance),
            ("Notifications", SettingsPage::Notifications),
            ("Privacy", SettingsPage::Privacy),
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
        for (config, host) in core.hosts() {
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
            let host_data = host.read(cx);
            for thread in host_data.threads_by_activity(None) {
                if thread.archived || thread.parent_thread_id.is_some() {
                    continue;
                }
                let project = host_data
                    .projects
                    .get(&thread.project_id)
                    .map(|project| project.name.clone())
                    .unwrap_or_default();
                entries.push(Entry {
                    label: thread
                        .title
                        .clone()
                        .unwrap_or_else(|| "Untitled thread".into()),
                    detail: project,
                    screen: Screen::Thread {
                        host: HostId(config.id.0.clone()),
                        thread: thread.id.clone(),
                    },
                });
            }
        }
        if !query.is_empty() {
            entries.retain(|entry| {
                entry.label.to_lowercase().contains(&query)
                    || entry.detail.to_lowercase().contains(&query)
            });
        }
        entries.truncate(SHOWN);
        entries
    }

    fn activate(&mut self, cx: &mut Context<Self>) {
        if let Some(entry) = self.entries(cx).into_iter().nth(self.highlighted) {
            self.core
                .update(cx, |core, cx| core.navigate(entry.screen, cx));
        }
        cx.emit(PaletteEvent::Dismissed);
    }
}

impl Render for Palette {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = DesktopTheme::neutral_dark();
        let entries = self.entries(cx);
        self.highlighted = self.highlighted.min(entries.len().saturating_sub(1));
        let rows = entries.into_iter().enumerate().map(|(index, entry)| {
            div()
                .id(index)
                .px(px(12.0))
                .py(px(6.0))
                .rounded(px(6.0))
                .flex()
                .gap(px(8.0))
                .cursor_pointer()
                .when(index == self.highlighted, |row| row.bg(theme.selected))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.highlighted = index;
                    this.activate(cx);
                }))
                .child(div().flex_1().min_w(px(0.0)).truncate().child(entry.label))
                .child(div().text_color(theme.secondary).child(entry.detail))
        });
        // The backdrop dismisses; the card stops clicks from reaching it.
        div()
            .id("palette-backdrop")
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|_, _: &Dismiss, _, cx| cx.emit(PaletteEvent::Dismissed)))
            .on_action(cx.listener(|this, _: &Up, _, cx| {
                this.highlighted = this.highlighted.saturating_sub(1);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Down, _, cx| {
                this.highlighted += 1;
                cx.notify();
            }))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(PaletteEvent::Dismissed)))
            .absolute()
            .inset_0()
            .flex()
            .justify_center()
            .pt(px(96.0))
            .bg(gpui::hsla(0.0, 0.0, 0.0, 0.4))
            .child(
                div()
                    .id("palette")
                    .occlude()
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .w(px(560.0))
                    .max_h(px(480.0))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .p(px(8.0))
                    .rounded(px(12.0))
                    .border_1()
                    .border_color(theme.popover_line)
                    .bg(theme.sidebar)
                    .text_sm()
                    .child(div().px(px(4.0)).pb(px(4.0)).child(self.query.clone()))
                    .children(rows),
            )
    }
}
