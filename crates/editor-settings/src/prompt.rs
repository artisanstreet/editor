//! Global prompt drafts and optimistic save/resolve forms. Canonical metadata belongs to HostData.

use crate::Settings;
use crate::page;
use artisan_api::{
    EngineId, GlobalPrompt, PromptCandidate, PromptEngineState, PromptEngineStatus, ResolvePrompt,
    SavePrompt,
};
use editor_core::{HostData, HostId, SettingsPage};
use editor_ui::button::ButtonVariant;
use editor_ui::page::{PageBlock, card, header, section_shell};
use editor_ui::status_chip::{ChipTone, chips_row, tone_color};
use editor_ui::text_editor::{TextEditor, TextEditorEvent};
use editor_ui::theme::ArtisanTheme;
use gpui::{
    AnyElement, AppContext as _, Context, Div, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Styled as _, Subscription, div, px,
};

#[cfg(test)]
pub(crate) const GLOBAL_PROMPT_KEY: &str = "global_prompt";

/// Lines of a candidate or edited text shown before it is cut.

pub(crate) struct PromptForm {
    pub(crate) editor: Entity<TextEditor>,
    /// The host the page was loaded from.
    host: Option<HostId>,
    /// What Forge last said: the prompt, its version, and each engine.
    loaded: bool,
    /// The text the editor was loaded from; the editor is dirty while its
    /// text differs.
    saved: String,
    /// The version the edit started from; a save names it.
    base_version: u64,
    /// A newer version was saved elsewhere while the user was editing.
    newer: bool,
    /// A save or resolution is in flight.
    pending: bool,
    error: Option<String>,
    generation: u64,
    edit_revision: u64,
    _changes: Subscription,
}

impl PromptForm {
    pub(crate) fn new(cx: &mut Context<Settings>) -> Self {
        let editor = cx.new(|cx| TextEditor::new("", "", cx));
        // Typing changes whether the prompt has unsaved changes.
        let changes = cx.subscribe(&editor, |settings, _, event: &TextEditorEvent, cx| {
            if *event == TextEditorEvent::Changed {
                settings.prompt.edit_revision = settings.prompt.edit_revision.wrapping_add(1);
            }
            cx.notify();
        });
        Self {
            editor,
            host: None,
            loaded: false,
            saved: String::new(),
            base_version: 0,
            newer: false,
            pending: false,
            error: None,
            generation: 0,
            edit_revision: 0,
            _changes: changes,
        }
    }

    /// Reads the host's prompt when the host changes or its saved text
    /// moved on (a save here or on another device).
    pub(crate) fn sync(&mut self, host: Option<&Entity<HostData>>, cx: &mut Context<Settings>) {
        let Some(host) = host else {
            self.host = None;
            self.loaded = false;
            self.generation = self.generation.wrapping_add(1);
            self.pending = false;
            return;
        };
        let id = host.read(cx).id.clone();
        let switched = self.host.as_ref() != Some(&id);
        if switched {
            self.generation = self.generation.wrapping_add(1);
            self.pending = false;
            self.host = Some(id);
            self.loaded = false;
            self.newer = false;
            self.error = None;
            self.base_version = 0;
            self.saved.clear();
            self.editor.update(cx, |editor, cx| editor.set_text("", cx));
        }
        if let Some(prompt) = host.read(cx).global_prompt.clone() {
            self.load(&prompt, false, cx);
        } else if !self.pending {
            host.update(cx, |host, cx| host.load_prompt(false, cx));
        }
    }

    /// Shows what Forge answered; the editor follows it unless the user has
    /// unsaved changes (then a newer version is only announced).
    fn load(&mut self, prompt: &GlobalPrompt, replace: bool, cx: &mut Context<Settings>) {
        if replace || !self.dirty(cx) {
            if self.editor.read(cx).text() != prompt.text {
                self.editor
                    .update(cx, |editor, cx| editor.set_text(&prompt.text, cx));
            }
            if self.saved != prompt.text {
                self.saved.clone_from(&prompt.text);
            }
            self.base_version = prompt.version;
            self.newer = false;
        } else if prompt.version != self.base_version {
            self.newer = true;
        }
        self.loaded = true;
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
        if !self.prompt.loaded || self.prompt.pending {
            return;
        }
        let client = host.read(cx).client.clone();
        let host_id = host.read(cx).id.clone();
        let revision = self.prompt.edit_revision;
        self.prompt.generation = self.prompt.generation.wrapping_add(1);
        let generation = self.prompt.generation;
        let save = SavePrompt {
            base_version: self.prompt.base_version,
            text: self.prompt.editor.read(cx).text().to_owned(),
        };
        self.prompt.pending = true;
        self.prompt.error = None;
        cx.notify();
        #[cfg(test)]
        let completion_gate = self.completion_gate.clone();
        cx.spawn(async move |this, cx| {
            let result = client.save_global_prompt(&save).await;
            #[cfg(test)]
            if let Some(gate) = completion_gate {
                gate.wait().await;
            }
            let _ = this.update(cx, |this, cx| {
                let form = &mut this.prompt;
                if form.host.as_ref() != Some(&host_id) || form.generation != generation {
                    return;
                }
                form.pending = false;
                match result {
                    Ok(prompt) => {
                        let saved_version = prompt.version;
                        host.update(cx, |host, cx| host.remember_prompt(prompt, cx));
                        let Some(prompt) = host.read(cx).global_prompt.clone() else {
                            form.newer = true;
                            return;
                        };
                        if prompt.version != saved_version {
                            form.load(&prompt, false, cx);
                        } else if form.edit_revision == revision {
                            form.load(&prompt, true, cx);
                        } else {
                            form.saved = prompt.text.clone();
                            form.base_version = prompt.version;
                            form.newer = false;
                            form.loaded = true;
                        }
                    }
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
        if let Some(prompt) = self
            .selected_host(cx)
            .and_then(|host| host.read(cx).global_prompt.clone())
        {
            self.prompt.load(&prompt, true, cx);
            cx.notify();
        }
    }

    fn resolve_prompt(&mut self, resolution: ResolvePrompt, cx: &mut Context<Self>) {
        let Some(host) = self.selected_host(cx) else {
            return;
        };
        if self.prompt.pending {
            return;
        }
        let client = host.read(cx).client.clone();
        let host_id = host.read(cx).id.clone();
        self.prompt.generation = self.prompt.generation.wrapping_add(1);
        let generation = self.prompt.generation;
        self.prompt.pending = true;
        self.prompt.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = client.resolve_global_prompt(&resolution).await;
            let _ = this.update(cx, |this, cx| {
                let form = &mut this.prompt;
                if form.host.as_ref() != Some(&host_id) || form.generation != generation {
                    return;
                }
                form.pending = false;
                match result {
                    Ok(prompt) => {
                        host.update(cx, |host, cx| host.remember_prompt(prompt, cx));
                        if let Some(prompt) = host.read(cx).global_prompt.clone() {
                            let replace = !form.dirty(cx);
                            form.load(&prompt, replace, cx);
                        }
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
    let host = settings.selected_host(cx);
    let load_error = host
        .as_ref()
        .and_then(|host| host.read(cx).prompt_load().error.clone());
    let fetching = host
        .as_ref()
        .is_some_and(|host| host.read(cx).prompt_load().loading);
    if let Some(error) = form.error.as_deref().or(load_error.as_deref()) {
        page = page.child(notice(theme, error)).child(settings.button(
            "prompt-retry",
            "Retry",
            ButtonVariant::Outline,
            form.pending || fetching,
            cx.listener(|this, _, _, cx| {
                if let Some(host) = this.selected_host(cx) {
                    this.prompt.error = None;
                    host.update(cx, |host, cx| host.load_prompt(true, cx));
                }
            }),
        ));
    }
    let projected = settings
        .selected_host(cx)
        .and_then(|host| host.read(cx).global_prompt.clone());
    if let Some(prompt) = &projected
        && !prompt.candidates.is_empty()
    {
        page = page.child(choice_section(settings, theme, &prompt.candidates, cx));
    }
    page = page.child(editor_section(settings, theme, cx));
    if let Some(prompt) = &projected {
        page = page.child(engines_section(settings, theme, &prompt.engines, cx));
    }
    page
}

fn choice_section(
    settings: &Settings,
    theme: &ArtisanTheme,
    candidates: &[PromptCandidate],
    cx: &mut Context<Settings>,
) -> PageBlock {
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
                .child(editor_ui::page::text_preview(theme, &candidate.text))
                .into_any_element()
        })
        .collect();
    section_shell(
        theme,
        "Choose your global prompt",
        Some(
            "Your engines had different prompts when Artisan took them over. Choose the one to keep in every engine, or write your own below. The others are backed up, never deleted.",
        ),
        None,
        card(theme, rows),
    )
.debug_selector("settings-section-setup")
}

impl Settings {
    /// The line under the prompt editor.
    pub(crate) fn prompt_status(&self, cx: &gpui::App) -> String {
        let form = &self.prompt;
        let dirty = form.dirty(cx);
        if !form.loaded {
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

fn editor_section(
    settings: &Settings,
    theme: &ArtisanTheme,
    cx: &mut Context<Settings>,
) -> PageBlock {
    let form = &settings.prompt;
    let loaded = form.loaded;
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
        "Prompt",
        Some(
            "Engines read this before every conversation, after their own system prompt and before your project's AGENTS.md or CLAUDE.md.",
        ),
        None,
        div().flex().flex_col().child(editor).child(footer),
    )
.debug_selector("settings-section-prompt")
}

fn engines_section(
    settings: &Settings,
    theme: &ArtisanTheme,
    engines: &[PromptEngineState],
    cx: &mut Context<Settings>,
) -> PageBlock {
    let rows = engines
        .iter()
        .map(|engine| engine_row(settings, theme, engine, cx).into_any_element())
        .collect();
    section_shell(
        theme,
        "Engines",
        Some(
            "Where each engine keeps the prompt. A file edited outside Artisan is left alone until you decide.",
        ),
        None,
        card(theme, rows),
    )
.debug_selector("settings-section-engines")
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
                    std::slice::from_ref(&editor_ui::status_chip::prompt_status(engine)),
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
            .child(editor_ui::page::text_preview(theme, external))
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

fn notice(theme: &ArtisanTheme, text: &str) -> editor_ui::page::Notice {
    editor_ui::page::notice(theme, text.to_owned()).debug_selector("settings-global-prompt-notice")
}

#[cfg(test)]
mod reform_tests {
    use super::*;
    use editor_core::{Core, EditorFiles};
    use gpui::TestAppContext;

    #[gpui::test]
    fn prompt_refresh_preserves_dirty_draft_and_reports_new_version(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let core = cx.new(|cx| {
            Core::new(
                EditorFiles {
                    dir: dir.path().join("client"),
                },
                cx,
            )
            .unwrap()
        });
        let (settings, cx) = cx.add_window_view(|window, cx| {
            Settings::new(core.clone(), crate::BuildInfo::default(), window, cx)
        });
        settings.update(cx, |settings, cx| {
            let mut prompt = GlobalPrompt {
                version: 1,
                text: "saved".into(),
                candidates: vec![],
                engines: vec![],
            };
            settings.prompt.load(&prompt, true, cx);
            settings
                .prompt
                .editor
                .update(cx, |editor, cx| editor.replace(0..5, "draft", cx));
            prompt.version = 2;
            prompt.text = "saved elsewhere".into();
            settings.prompt.load(&prompt, false, cx);
            assert_eq!(settings.prompt.editor.read(cx).text(), "draft");
            assert_eq!(settings.prompt.base_version, 1);
            assert!(settings.prompt.newer);
            settings.prompt.load(&prompt, true, cx);
            assert_eq!(settings.prompt.editor.read(cx).text(), "saved elsewhere");
            assert_eq!(settings.prompt.base_version, 2);
            assert!(!settings.prompt.newer);
        });
    }
}

#[cfg(test)]
mod delayed_save_tests {
    use crate::tests::{CompletionGate, SettingsFixture, finish_gate, wait_for, wait_gate};
    use gpui::{Focusable as _, TestAppContext};

    #[gpui::test]
    fn typing_during_prompt_save_preserves_newer_draft_and_undo(cx: &mut TestAppContext) {
        let fixture = SettingsFixture::new(cx);
        let (settings, cx) = fixture.open(cx, editor_core::SettingsPage::GlobalPrompt);
        wait_for(cx, "initial prompt", |cx| {
            settings.read(cx).prompt.loaded.then_some(())
        });
        let gate = CompletionGate::new();
        settings.update(cx, |settings, cx| {
            settings.completion_gate = Some(gate.clone());
            settings.prompt.editor.update(cx, |editor, cx| {
                editor.replace(0..editor.text().len(), "first saved", cx)
            });
            settings.save_prompt(cx);
        });
        wait_gate(cx, &gate);
        let editor = settings.read_with(cx, |settings, _| settings.prompt.editor.clone());
        editor.update(cx, |editor, cx| {
            editor.replace(editor.text().len()..editor.text().len(), " second edit", cx)
        });
        cx.run_until_parked();
        finish_gate(cx, &gate);
        settings.read_with(cx, |settings, cx| {
            assert_eq!(
                settings.prompt.editor.read(cx).text(),
                "first saved second edit"
            );
            assert_eq!(settings.prompt.saved, "first saved");
            assert!(settings.prompt.dirty(cx));
            assert!(!settings.prompt.pending);
            assert!(settings.prompt.base_version > 0);
        });
        cx.update(|window, cx| {
            window.focus(&editor.read(cx).focus_handle(cx), cx);
            window.dispatch_action(Box::new(editor_ui::text_editor::Undo), cx);
        });
        cx.run_until_parked();
        editor.read_with(cx, |editor, _| assert_eq!(editor.text(), "first saved"));
    }

    #[gpui::test]
    fn delayed_prompt_save_cannot_mutate_new_host(cx: &mut TestAppContext) {
        let fixture = SettingsFixture::new(cx);
        let (settings, cx) = fixture.open(cx, editor_core::SettingsPage::GlobalPrompt);
        wait_for(cx, "initial prompt", |cx| {
            settings.read(cx).prompt.loaded.then_some(())
        });
        let gate = CompletionGate::new();
        settings.update(cx, |settings, cx| {
            settings.completion_gate = Some(gate.clone());
            settings.prompt.editor.update(cx, |editor, cx| {
                editor.replace(0..editor.text().len(), "host A saved", cx)
            });
            settings.save_prompt(cx);
        });
        wait_gate(cx, &gate);
        fixture.select(1, editor_core::SettingsPage::GlobalPrompt, cx);
        wait_for(cx, "host B prompt", |cx| {
            settings.read(cx).prompt.loaded.then_some(())
        });
        settings.update(cx, |settings, cx| {
            settings.prompt.editor.update(cx, |editor, cx| {
                editor.replace(0..editor.text().len(), "host B draft", cx)
            });
            settings.prompt.error = Some("B owns this notice".into());
        });
        let baseline = settings.read_with(cx, |settings, _| {
            (settings.prompt.saved.clone(), settings.prompt.base_version)
        });
        finish_gate(cx, &gate);
        settings.read_with(cx, |settings, cx| {
            assert_eq!(settings.prompt.host.as_ref(), Some(&fixture.hosts[1]));
            assert_eq!(settings.prompt.editor.read(cx).text(), "host B draft");
            assert_eq!(
                (&settings.prompt.saved, settings.prompt.base_version),
                (&baseline.0, baseline.1)
            );
            assert_eq!(settings.prompt.error.as_deref(), Some("B owns this notice"));
            assert!(!settings.prompt.pending);
        });
    }
}
