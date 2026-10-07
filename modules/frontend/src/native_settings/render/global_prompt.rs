//! The global prompt page: setup choice, the editor with save and revert,
//! and every engine's file with the drift decisions.

use artisan_assets::AssetId;
use artisan_domain::{
    GlobalPromptCandidate, GlobalPromptEngineState, GlobalPromptEngineStatus,
    GlobalPromptResolution, GlobalPromptSetup,
};

use super::chrome::{settings_card, settings_header, settings_section_shell};
use super::*;
use crate::engine_status_chips::{chips_row, tone_color};
use crate::marketplace_presentation::{ChipTone, engine_name, prompt_chip};
use crate::native_settings::global_prompt::GlobalPromptPage;

/// Lines of a candidate or drifted text shown before it is cut.
const PREVIEW_LINES: usize = 8;

impl SettingsScreen {
    /// Renders the global prompt page.
    pub(super) fn render_global_prompt(
        &self,
        theme: &artisan_ui::theme::ArtisanTheme,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut page = div().flex().flex_col().child(settings_header(
            theme,
            SettingsSection::GlobalPrompt.title().to_owned(),
            SettingsSection::GlobalPrompt.description(),
        ));
        let Some(state) = &self.global_prompt else {
            return page;
        };
        if let Some(notice) = &state.notice {
            page = page.child(prompt_notice(theme, notice));
        }
        if let Some(prompt) = &state.prompt
            && let GlobalPromptSetup::ChoiceRequired(candidates) = prompt.setup()
        {
            page = page.child(self.choice_section(theme, state, candidates, cx));
        }
        page = page.child(self.editor_section(theme, state, cx));
        if let Some(prompt) = &state.prompt {
            page = page.child(self.engines_section(theme, state, prompt.engines(), cx));
        }
        page
    }

    fn choice_section(
        &self,
        theme: &artisan_ui::theme::ArtisanTheme,
        state: &GlobalPromptPage,
        candidates: &[GlobalPromptCandidate],
        cx: &mut Context<Self>,
    ) -> Div {
        let rows = candidates
            .iter()
            .map(|candidate| {
                let engines: Vec<&str> = candidate
                    .engine_ids
                    .iter()
                    .map(|engine| engine_name(engine))
                    .collect();
                let candidate_id = candidate.candidate_id.clone();
                let action = self
                    .prompt_button(
                        theme,
                        format!(
                            "settings-global-prompt-choose-{}",
                            &candidate.candidate_id[..12]
                        ),
                        "Use this prompt",
                        ButtonVariant::Default,
                        state.pending,
                    )
                    .map(|button| {
                        button.on_activate(cx.listener(move |screen, _, _, cx| {
                            screen.resolve_global_prompt(
                                GlobalPromptResolution::ChooseCandidate {
                                    candidate_id: candidate_id.clone(),
                                },
                                cx,
                            );
                        }))
                    });
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
                            .children(action),
                    )
                    .child(preview(theme, &candidate.text))
            })
            .collect();
        settings_section_shell(
            theme,
            "setup",
            "Choose your global prompt",
            Some(
                "Your engines had different prompts when Artisan took them over. Choose the one to keep in every engine, or write your own below. The others are backed up, never deleted.",
            ),
            None,
            settings_card(theme, rows),
        )
    }

    fn editor_section(
        &self,
        theme: &artisan_ui::theme::ArtisanTheme,
        state: &GlobalPromptPage,
        cx: &mut Context<Self>,
    ) -> Div {
        let dirty = state.dirty(cx);
        let status = if !state.loaded {
            "Reading the global prompt\u{2026}".to_owned()
        } else if state.pending {
            "Saving to every engine\u{2026}".to_owned()
        } else if state.newer {
            "A newer version was saved elsewhere. Load it to continue from there.".to_owned()
        } else if dirty {
            "Unsaved changes".to_owned()
        } else if state.base_revision.get() == 0 {
            "Not saved yet".to_owned()
        } else {
            format!("Saved \u{b7} version {}", state.base_revision.get())
        };
        let mut actions = div().flex().flex_row().gap(px(8.0)).flex_shrink_0();
        if state.newer {
            actions = actions.children(
                self.prompt_button(
                    theme,
                    "settings-global-prompt-load-latest".to_owned(),
                    "Load latest",
                    ButtonVariant::Outline,
                    state.pending,
                )
                .map(|button| {
                    button.on_activate(cx.listener(|screen, _, _, cx| {
                        screen.load_latest_global_prompt(cx);
                    }))
                }),
            );
        }
        actions = actions
            .children(
                self.prompt_button(
                    theme,
                    "settings-global-prompt-revert".to_owned(),
                    "Revert",
                    ButtonVariant::Ghost,
                    !dirty || state.pending,
                )
                .map(|button| {
                    button.on_activate(cx.listener(|screen, _, _, cx| {
                        screen.revert_global_prompt(cx);
                    }))
                }),
            )
            .children(
                self.prompt_button(
                    theme,
                    "settings-global-prompt-save".to_owned(),
                    "Save",
                    ButtonVariant::Default,
                    !dirty || state.pending || state.newer,
                )
                .map(|button| {
                    button.on_activate(cx.listener(|screen, _, _, cx| {
                        screen.save_global_prompt(cx);
                    }))
                }),
            );
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
            .child(state.editor.clone());
        settings_section_shell(
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
        &self,
        theme: &artisan_ui::theme::ArtisanTheme,
        state: &GlobalPromptPage,
        engines: &[GlobalPromptEngineState],
        cx: &mut Context<Self>,
    ) -> Div {
        let rows = engines
            .iter()
            .map(|engine| self.engine_row(theme, state, engine, cx))
            .collect();
        settings_section_shell(
            theme,
            "engines",
            "Engines",
            Some(
                "Where each engine keeps the prompt. A file edited outside Artisan is left alone until you decide.",
            ),
            None,
            settings_card(theme, rows),
        )
    }

    fn engine_row(
        &self,
        theme: &artisan_ui::theme::ArtisanTheme,
        state: &GlobalPromptPage,
        engine: &GlobalPromptEngineState,
        cx: &mut Context<Self>,
    ) -> Div {
        let chip = prompt_chip(engine);
        let mut row = div().flex().flex_col().gap(px(8.0)).py(px(14.0)).child(
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
                                .child(engine_name(&engine.engine_id)),
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
                    std::slice::from_ref(&chip),
                    "global-prompt-engine",
                )),
        );
        if engine.status == GlobalPromptEngineStatus::Drifted
            && let Some(external) = &engine.external_text
        {
            let actions = self.drift_actions(theme, state, &engine.engine_id, cx);
            row = row
                .child(
                    div()
                        .text_xs()
                        .text_color(tone_color(theme, ChipTone::Warning).to_paint())
                        .child(format!(
                            "{} was edited outside Artisan. It now says:",
                            engine_name(&engine.engine_id)
                        )),
                )
                .child(preview(theme, external))
                .child(actions);
        }
        row
    }

    fn drift_actions(
        &self,
        theme: &artisan_ui::theme::ArtisanTheme,
        state: &GlobalPromptPage,
        engine_id: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let import_id = engine_id.to_owned();
        let overwrite_id = engine_id.to_owned();
        div()
            .flex()
            .flex_row()
            .gap(px(8.0))
            .children(
                self.prompt_button(
                    theme,
                    format!("settings-global-prompt-import-{engine_id}"),
                    "Use this version everywhere",
                    ButtonVariant::Outline,
                    state.pending,
                )
                .map(|button| {
                    button.on_activate(cx.listener(move |screen, _, _, cx| {
                        screen.resolve_global_prompt(
                            GlobalPromptResolution::ImportEngine {
                                engine_id: import_id.clone(),
                            },
                            cx,
                        );
                    }))
                }),
            )
            .children(
                self.prompt_button(
                    theme,
                    format!("settings-global-prompt-overwrite-{engine_id}"),
                    "Restore Artisan's prompt",
                    ButtonVariant::Ghost,
                    state.pending,
                )
                .map(|button| {
                    button.on_activate(cx.listener(move |screen, _, _, cx| {
                        screen.resolve_global_prompt(
                            GlobalPromptResolution::OverwriteEngine {
                                engine_id: overwrite_id.clone(),
                            },
                            cx,
                        );
                    }))
                }),
            )
    }

    fn prompt_button(
        &self,
        theme: &artisan_ui::theme::ArtisanTheme,
        id: String,
        label: &'static str,
        variant: ButtonVariant,
        disabled: bool,
    ) -> Option<Button> {
        Button::new(
            SharedString::from(id.clone()),
            self.focus.control.clone(),
            *theme,
            MotionPolicy::Reduced,
            variant,
            ButtonSize::Small,
            ButtonContent::text(label),
        )
        .ok()
        .map(|button| button.disabled(disabled).debug_selector(id))
    }
}

/// The first lines of a prompt, in a quiet well.
fn preview(theme: &artisan_ui::theme::ArtisanTheme, text: &str) -> Div {
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

fn prompt_notice(theme: &artisan_ui::theme::ArtisanTheme, text: &str) -> Div {
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
            artisan_ui::asset_seam::asset_glyph(AssetId::TABLER_ALERT_TRIANGLE)
                .size(px(14.0))
                .text_color(tone.with_alpha(0.8).to_paint()),
        )
        .child(text.to_owned())
}
