//! Global prompt (master's `render/global_prompt.rs`): the setup choice when
//! the engines held different prompts, the editor with its version, Revert
//! and Save, and every engine's file with what to do about one edited
//! outside Artisan. Forge keeps the versions and writes the engines' files.

use crate::Settings;
use crate::chrome::{card, header, section_shell};
use crate::page;
use artisan_api::{
    EngineId, GlobalPrompt, PromptCandidate, PromptEngineState, PromptEngineStatus, ResolvePrompt,
    SavePrompt,
};
use editor_core::{HostData, HostId, SettingsPage};
use editor_ui::AssetId;
use editor_ui::asset_seam::asset_glyph;
use editor_ui::button::ButtonVariant;
use editor_ui::status_chip::{ChipTone, EngineChip, chips_row, tone_color};
use editor_ui::text_editor::{TextEditor, TextEditorEvent};
use editor_ui::theme::ArtisanTheme;
use gpui::{
    AnyElement, AppContext as _, Context, Div, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Styled as _, Subscription, div, px,
};
use serde_json::Value;

pub(crate) const GLOBAL_PROMPT_KEY: &str = "global_prompt";

/// Lines of a candidate or edited text shown before it is cut.
const PREVIEW_LINES: usize = 8;

pub(crate) struct PromptForm {
    pub(crate) editor: Entity<TextEditor>,
    /// The host the page was loaded from.
    host: Option<HostId>,
    /// What Forge last said: the prompt, its version, and each engine.
    prompt: Option<GlobalPrompt>,
    /// The text the editor was loaded from; the editor is dirty while its
    /// text differs.
    saved: String,
    /// The version the edit started from; a save names it.
    base_version: u64,
    /// A newer version was saved elsewhere while the user was editing.
    newer: bool,
    /// A read is in flight.
    fetching: bool,
    /// A save or resolution is in flight.
    pending: bool,
    error: Option<String>,
    _changes: Subscription,
}

impl PromptForm {
    pub(crate) fn new(cx: &mut Context<Settings>) -> Self {
        let editor = cx.new(|cx| TextEditor::new("", "", cx));
        // Typing changes whether the prompt has unsaved changes.
        let changes = cx.subscribe(&editor, |_, _, _: &TextEditorEvent, cx| cx.notify());
        Self {
            editor,
            host: None,
            prompt: None,
            saved: String::new(),
            base_version: 0,
            newer: false,
            fetching: false,
            pending: false,
            error: None,
            _changes: changes,
        }
    }

    /// Reads the host's prompt when the host changes or its saved text
    /// moved on (a save here or on another device).
    pub(crate) fn sync(&mut self, host: Option<&Entity<HostData>>, cx: &mut Context<Settings>) {
        let Some(host) = host else {
            self.host = None;
            self.prompt = None;
            return;
        };
        let (id, stored) = {
            let data = host.read(cx);
            let stored = data
                .setting(GLOBAL_PROMPT_KEY)
                .and_then(Value::as_str)
                .map(str::to_owned);
            (data.id.clone(), stored)
        };
        let switched = self.host.as_ref() != Some(&id);
        if switched {
            self.host = Some(id);
            self.prompt = None;
            self.newer = false;
            self.error = None;
            self.base_version = 0;
            self.saved.clear();
            self.editor.update(cx, |editor, cx| editor.set_text("", cx));
        }
        let moved = self
            .prompt
            .as_ref()
            .is_some_and(|prompt| stored.is_some_and(|stored| stored != prompt.text));
        if (switched || moved) && !self.fetching && !self.pending {
            self.fetch(host, cx);
        }
    }

    fn fetch(&mut self, host: &Entity<HostData>, cx: &mut Context<Settings>) {
        let data = host.read(cx);
        let (id, client) = (data.id.clone(), data.client.clone());
        self.fetching = true;
        cx.spawn(async move |this, cx| {
            let result = client.global_prompt().await;
            let _ = this.update(cx, |settings, cx| {
                let form = &mut settings.prompt;
                form.fetching = false;
                if form.host.as_ref() != Some(&id) {
                    return;
                }
                match result {
                    Ok(prompt) => form.load(prompt, false, cx),
                    Err(error) => {
                        form.error = Some(format!("Couldn't read the global prompt: {error:#}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Shows what Forge answered; the editor follows it unless the user has
    /// unsaved changes (then a newer version is only announced).
    fn load(&mut self, prompt: GlobalPrompt, replace: bool, cx: &mut Context<Settings>) {
        if replace || !self.dirty(cx) {
            let text = prompt.text.clone();
            self.editor
                .update(cx, |editor, cx| editor.set_text(&text, cx));
            self.saved = text;
            self.base_version = prompt.version;
            self.newer = false;
        } else if prompt.version != self.base_version {
            self.newer = true;
        }
        self.prompt = Some(prompt);
    }

    fn dirty(&self, cx: &gpui::App) -> bool {
        self.editor.read(cx).text() != self.saved
    }
}

impl Settings {
    pub(crate) fn save_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(host) = self.selected_host(cx) else {
            return;
        };
        if self.prompt.prompt.is_none() {
            return;
        }
        let client = host.read(cx).client.clone();
        let save = SavePrompt {
            base_version: self.prompt.base_version,
            text: self.prompt.editor.read(cx).text().to_owned(),
        };
        self.prompt.pending = true;
        self.prompt.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = client.save_global_prompt(&save).await;
            let _ = this.update(cx, |this, cx| {
                let form = &mut this.prompt;
                form.pending = false;
                match result {
                    Ok(prompt) => form.load(prompt, true, cx),
                    Err(error) => {
                        form.error = Some(format!("Couldn't save the global prompt: {error:#}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn revert_prompt(&mut self, cx: &mut Context<Self>) {
        let saved = self.prompt.saved.clone();
        self.prompt
            .editor
            .update(cx, |editor, cx| editor.set_text(&saved, cx));
        self.prompt.error = None;
        cx.notify();
    }

    fn load_latest_prompt(&mut self, cx: &mut Context<Self>) {
        if let Some(prompt) = self.prompt.prompt.clone() {
            self.prompt.load(prompt, true, cx);
            cx.notify();
        }
    }

    fn resolve_prompt(&mut self, resolution: ResolvePrompt, cx: &mut Context<Self>) {
        let Some(host) = self.selected_host(cx) else {
            return;
        };
        let client = host.read(cx).client.clone();
        self.prompt.pending = true;
        self.prompt.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = client.resolve_global_prompt(&resolution).await;
            let _ = this.update(cx, |this, cx| {
                let form = &mut this.prompt;
                form.pending = false;
                match result {
                    Ok(prompt) => {
                        let replace = !form.dirty(cx);
                        form.load(prompt, replace, cx);
                    }
                    Err(error) => form.error = Some(format!("{error:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn prompt_button(
        &self,
        id: String,
        label: &'static str,
        variant: ButtonVariant,
        disabled: bool,
        resolution: Option<ResolvePrompt>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selector = id.clone();
        div()
            .debug_selector(move || selector.clone())
            .child(self.button(
                SharedString::from(id),
                label,
                variant,
                disabled,
                cx.listener(move |this, _, _, cx| {
                    if let Some(resolution) = resolution.clone() {
                        this.resolve_prompt(resolution, cx);
                    }
                }),
            ))
            .into_any_element()
    }
}

pub(crate) fn render(settings: &Settings, theme: &ArtisanTheme, cx: &mut Context<Settings>) -> Div {
    let form = &settings.prompt;
    let mut page = div().flex().flex_col().child(header(
        theme,
        page::label(SettingsPage::GlobalPrompt),
        page::description(SettingsPage::GlobalPrompt),
    ));
    if let Some(error) = &form.error {
        page = page.child(notice(theme, error));
    }
    if let Some(prompt) = &form.prompt
        && !prompt.candidates.is_empty()
    {
        page = page.child(choice_section(settings, theme, &prompt.candidates, cx));
    }
    page = page.child(editor_section(settings, theme, cx));
    if let Some(prompt) = &form.prompt {
        page = page.child(engines_section(settings, theme, &prompt.engines, cx));
    }
    page
}

fn choice_section(
    settings: &Settings,
    theme: &ArtisanTheme,
    candidates: &[PromptCandidate],
    cx: &mut Context<Settings>,
) -> Div {
    let rows = candidates
        .iter()
        .map(|candidate| {
            let engines: Vec<&str> = candidate
                .engines
                .iter()
                .map(|engine| page::engine_label(*engine))
                .collect();
            let short = candidate.id.get(..12).unwrap_or(&candidate.id);
            let action = settings.prompt_button(
                format!("settings-global-prompt-choose-{short}"),
                "Use this prompt",
                ButtonVariant::Default,
                settings.prompt.pending,
                Some(ResolvePrompt::Choose {
                    candidate_id: candidate.id.clone(),
                }),
                cx,
            );
            div()
                .flex()
                .flex_col()
                .gap(px(8.0))
                .py(px(14.0))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap(px(16.0))
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.colors.foreground.to_paint())
                                .child(format!("From {}", engines.join(", "))),
                        )
                        .child(action),
                )
                .child(preview(theme, &candidate.text))
                .into_any_element()
        })
        .collect();
    section_shell(
        theme,
        "setup",
        "Choose your global prompt",
        Some(
            "Your engines had different prompts when Artisan took them over. Choose the one to keep in every engine, or write your own below. The others are backed up, never deleted.",
        ),
        None,
        card(theme, rows),
    )
}

impl Settings {
    /// The line under the prompt editor.
    pub(crate) fn prompt_status(&self, cx: &gpui::App) -> String {
        let form = &self.prompt;
        let dirty = form.dirty(cx);
        if form.prompt.is_none() {
            "Reading the global prompt\u{2026}".to_owned()
        } else if form.pending {
            "Saving to every engine\u{2026}".to_owned()
        } else if form.newer {
            "A newer version was saved elsewhere. Load it to continue from there.".to_owned()
        } else if dirty {
            "Unsaved changes".to_owned()
        } else if form.base_version == 0 {
            "Not saved yet".to_owned()
        } else {
            format!("Saved \u{b7} version {}", form.base_version)
        }
    }
}

fn editor_section(settings: &Settings, theme: &ArtisanTheme, cx: &mut Context<Settings>) -> Div {
    let form = &settings.prompt;
    let loaded = form.prompt.is_some();
    let dirty = form.dirty(cx);
    let status = settings.prompt_status(cx);
    let mut actions = div().flex().flex_row().gap(px(8.0)).flex_shrink_0();
    if form.newer {
        actions = actions.child(settings.button(
            "settings-global-prompt-load-latest",
            "Load latest",
            ButtonVariant::Outline,
            form.pending,
            cx.listener(|this, _, _, cx| this.load_latest_prompt(cx)),
        ));
    }
    actions = actions
        .child(settings.button(
            "settings-global-prompt-revert",
            "Revert",
            ButtonVariant::Ghost,
            !dirty || form.pending,
            cx.listener(|this, _, _, cx| this.revert_prompt(cx)),
        ))
        .child(settings.button(
            "settings-global-prompt-save",
            "Save",
            ButtonVariant::Default,
            !loaded || !dirty || form.pending || form.newer,
            cx.listener(|this, _, _, cx| this.save_prompt(cx)),
        ));
    let footer = div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(px(16.0))
        .py(px(12.0))
        .child(
            div()
                .debug_selector(|| "settings-global-prompt-status".to_owned())
                .text_xs()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(status),
        )
        .child(actions);
    let editor = div()
        .pt(px(14.0))
        .debug_selector(|| "settings-global-prompt-editor".to_owned())
        .child(form.editor.clone());
    section_shell(
        theme,
        "prompt",
        "Prompt",
        Some(
            "Engines read this before every conversation, after their own system prompt and before your project's AGENTS.md or CLAUDE.md.",
        ),
        None,
        div().flex().flex_col().child(editor).child(footer),
    )
}

fn engines_section(
    settings: &Settings,
    theme: &ArtisanTheme,
    engines: &[PromptEngineState],
    cx: &mut Context<Settings>,
) -> Div {
    let rows = engines
        .iter()
        .map(|engine| engine_row(settings, theme, engine, cx).into_any_element())
        .collect();
    section_shell(
        theme,
        "engines",
        "Engines",
        Some(
            "Where each engine keeps the prompt. A file edited outside Artisan is left alone until you decide.",
        ),
        None,
        card(theme, rows),
    )
}

fn engine_row(
    settings: &Settings,
    theme: &ArtisanTheme,
    engine: &PromptEngineState,
    cx: &mut Context<Settings>,
) -> Div {
    let name = page::engine_label(engine.engine);
    let mut row = div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .py(px(14.0))
        .debug_selector({
            let selector = format!("settings-global-prompt-engine-{}", engine.engine);
            move || selector.clone()
        })
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(16.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.colors.foreground.to_paint())
                                .child(name),
                        )
                        .child(
                            div()
                                .text_xs()
                                .truncate()
                                .text_color(theme.colors.muted_foreground.to_paint())
                                .child(
                                    engine
                                        .reason
                                        .clone()
                                        .or_else(|| engine.file.clone())
                                        .unwrap_or_default(),
                                ),
                        ),
                )
                .child(chips_row(
                    theme,
                    std::slice::from_ref(&prompt_chip(engine)),
                    "global-prompt-engine",
                )),
        );
    if engine.status == PromptEngineStatus::Drifted
        && let Some(external) = &engine.external_text
    {
        row = row
            .child(
                div()
                    .text_xs()
                    .text_color(tone_color(theme, ChipTone::Warning).to_paint())
                    .debug_selector({
                        let selector = format!("settings-global-prompt-drift-{}", engine.engine);
                        move || selector.clone()
                    })
                    .child(format!("{name} was edited outside Artisan. It now says:")),
            )
            .child(preview(theme, external))
            .child(drift_actions(settings, engine.engine, cx));
    }
    row
}

fn drift_actions(settings: &Settings, engine: EngineId, cx: &mut Context<Settings>) -> Div {
    let pending = settings.prompt.pending;
    div()
        .flex()
        .flex_row()
        .gap(px(8.0))
        .child(settings.prompt_button(
            format!("settings-global-prompt-import-{engine}"),
            "Use this version everywhere",
            ButtonVariant::Outline,
            pending,
            Some(ResolvePrompt::UseEngine { engine }),
            cx,
        ))
        .child(settings.prompt_button(
            format!("settings-global-prompt-overwrite-{engine}"),
            "Restore Artisan's prompt",
            ButtonVariant::Ghost,
            pending,
            Some(ResolvePrompt::RestoreEngine { engine }),
            cx,
        ))
}

fn prompt_chip(state: &PromptEngineState) -> EngineChip {
    let (label, tone, icon) = match state.status {
        PromptEngineStatus::Synced => ("Synced", ChipTone::Success, AssetId::TABLER_CHECK),
        PromptEngineStatus::Drifted => {
            ("Edited", ChipTone::Warning, AssetId::TABLER_ALERT_TRIANGLE)
        }
        PromptEngineStatus::AwaitingChoice => {
            ("Waiting", ChipTone::Warning, AssetId::TABLER_ALERT_TRIANGLE)
        }
        PromptEngineStatus::EngineAbsent => {
            ("Not installed", ChipTone::Muted, AssetId::TABLER_MINUS)
        }
        PromptEngineStatus::Unavailable => ("Unavailable", ChipTone::Muted, AssetId::TABLER_MINUS),
        PromptEngineStatus::Failed => ("Failed", ChipTone::Danger, AssetId::TABLER_X),
    };
    let name = page::engine_label(state.engine);
    let reason = state.reason.clone().or_else(|| {
        state
            .file
            .as_ref()
            .filter(|_| state.status == PromptEngineStatus::Synced)
            .map(|file| format!("{name}: {file}"))
    });
    EngineChip::new(
        state.engine.as_str(),
        name,
        label,
        tone,
        icon,
        reason.as_deref(),
    )
}

/// The first lines of a prompt, in a quiet well.
fn preview(theme: &ArtisanTheme, text: &str) -> Div {
    let lines: Vec<&str> = text.lines().collect();
    let mut shown = lines
        .iter()
        .take(PREVIEW_LINES)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    if lines.len() > PREVIEW_LINES {
        shown.push_str("\n\u{2026}");
    }
    if shown.trim().is_empty() {
        "(empty)".clone_into(&mut shown);
    }
    div()
        .px(px(12.0))
        .py(px(10.0))
        .rounded(px(8.0))
        .bg(theme.colors.foreground.with_alpha(0.04).to_paint())
        .text_xs()
        .text_color(theme.colors.muted_foreground.to_paint())
        .whitespace_normal()
        .children(
            shown
                .lines()
                .map(|line| div().min_h(px(16.0)).child(line.to_owned()))
                .collect::<Vec<_>>(),
        )
}

fn notice(theme: &ArtisanTheme, text: &str) -> Div {
    let tone = tone_color(theme, ChipTone::Danger);
    div()
        .mt(px(24.0))
        .px(px(12.0))
        .py(px(10.0))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .rounded(px(10.0))
        .bg(tone.with_alpha(0.10).to_paint())
        .text_sm()
        .text_color(tone.to_paint())
        .debug_selector(|| "settings-global-prompt-notice".to_owned())
        .child(
            asset_glyph(AssetId::TABLER_ALERT_TRIANGLE)
                .size(px(14.0))
                .text_color(tone.with_alpha(0.8).to_paint()),
        )
        .child(text.to_owned())
}
