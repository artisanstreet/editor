//! Painting the Marketplace (master's `marketplace_screen/render.rs`).

use crate::presentation::{SkillRow, prompt_summary, sections};
use crate::{MARKETPLACE_SCREEN_SELECTOR, Marketplace};
use artisan_api::SkillOrigin;
use editor_core::{Screen, SettingsPage};
use editor_ui::AssetId;
use editor_ui::asset_seam::asset_glyph;
use editor_ui::button::{Button, ButtonContent, ButtonSize, ButtonVariant};
use editor_ui::card::{CardStyle, compact_card, compact_card_content};
use editor_ui::motion::MotionPolicy;
use editor_ui::status_chip::{ChipTone, chips_row, tone_color};
use editor_ui::switch::{Switch, SwitchSize};
use editor_ui::theme::ArtisanTheme;
use editor_ui::wordmark_loader::WordmarkLoader;
use gpui::prelude::*;
use gpui::{AnyElement, Context, Div, FontWeight, SharedString, Window, div, px};

/// Content width, matching Settings.
const CONTENT_MAX_WIDTH_PX: f32 = 896.0;

impl Render for Marketplace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync(cx);
        let theme = self.theme;
        let mut column = div()
            .w_full()
            .max_w(px(CONTENT_MAX_WIDTH_PX))
            .flex()
            .flex_col()
            .px(px(24.0))
            .py(px(48.0))
            .child(header(&theme));
        if self.host.is_none() {
            column = column.child(
                div()
                    .mt(px(40.0))
                    .child(muted(&theme, "Pair a host to see its skills.")),
            );
        } else {
            match &self.catalog {
                None => {
                    column = column.child(
                        div()
                            .mt(px(96.0))
                            .flex()
                            .justify_center()
                            .child(WordmarkLoader::new("marketplace-loading", theme)),
                    );
                }
                Some(catalog) => {
                    let catalog: &[artisan_api::CatalogSkill] = match catalog {
                        Ok(catalog) => catalog,
                        Err(error) => {
                            column = column.child(notice_box(&theme, error));
                            &[]
                        }
                    };
                    if let Some(notice) = &self.notice {
                        column = column.child(notice_box(&theme, notice));
                    }
                    column = column.child(self.prompt_section(&theme, cx));
                    let sections = sections(catalog);
                    column = column.child(self.skill_section(
                        &theme,
                        "artisan",
                        "Artisan skills",
                        Some("Skills that teach every engine how Artisan works."),
                        &sections.artisan,
                        None,
                        cx,
                    ));
                    column = column.child(self.skill_section(
                        &theme,
                        "adopted",
                        "Your skills",
                        None,
                        &sections.adopted,
                        Some("Adopt a skill below to keep it in every engine."),
                        cx,
                    ));
                    if !sections.discovered.is_empty() {
                        column = column.child(self.skill_section(
                            &theme,
                            "discovered",
                            "Found in your engines",
                            Some(
                                "Skills an engine has on its own. Adopting one moves each copy into Artisan's backups and keeps Artisan's copy in every engine.",
                            ),
                            &sections.discovered,
                            None,
                            cx,
                        ));
                    }
                }
            }
        }
        div()
            .id("marketplace-screen")
            .track_focus(&self.focus)
            .debug_selector(|| MARKETPLACE_SCREEN_SELECTOR.to_owned())
            .size_full()
            .overflow_y_scroll()
            .bg(theme.colors.background.to_paint())
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .justify_center()
                    .child(column),
            )
    }
}

impl Marketplace {
    fn prompt_section(&self, theme: &ArtisanTheme, cx: &mut Context<Self>) -> Div {
        let summary = self.prompt.as_ref().map(prompt_summary);
        let choice = self
            .prompt
            .as_ref()
            .is_some_and(|prompt| !prompt.candidates.is_empty());
        let action = Button::new(
            "marketplace-global-prompt-open",
            self.focus.clone(),
            *theme,
            MotionPolicy::Reduced,
            if choice {
                ButtonVariant::Default
            } else {
                ButtonVariant::Outline
            },
            ButtonSize::Small,
            ButtonContent::text(if choice { "Choose" } else { "Edit" }),
        )
        .ok()
        .map(|button| {
            let this = cx.entity().downgrade();
            button
                .debug_selector("marketplace-global-prompt-open")
                .on_activate(move |_, _, cx| {
                    let _ = this.update(cx, |this, cx| {
                        this.navigate(Screen::Settings(SettingsPage::GlobalPrompt), cx);
                    });
                })
        });
        let mut text = div().flex_1().min_w_0().flex().flex_col().gap(px(6.0));
        match &summary {
            None => {
                text = text.child(muted(theme, "Reading the global prompt\u{2026}"));
            }
            Some(summary) => {
                text = text.child(
                    div()
                        .text_sm()
                        .truncate()
                        .text_color(if summary.empty {
                            theme.colors.muted_foreground.to_paint()
                        } else {
                            theme.colors.foreground.to_paint()
                        })
                        .child(summary.preview.clone()),
                );
                if let Some(attention) = &summary.attention {
                    text = text.child(attention_line(theme, attention));
                }
                text = text.child(chips_row(theme, &summary.chips, "global-prompt"));
            }
        }
        let row = div()
            .flex()
            .flex_row()
            .items_start()
            .gap(px(16.0))
            .py(px(14.0))
            .child(text)
            .children(action);
        section(
            theme,
            "global-prompt",
            "Global prompt",
            Some("Standing instructions every engine reads before your messages."),
            card(theme, vec![row]),
        )
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "one section painter shared by the three skill groups"
    )]
    fn skill_section(
        &self,
        theme: &ArtisanTheme,
        anchor: &'static str,
        title: &str,
        intro: Option<&str>,
        rows: &[SkillRow],
        empty: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Div {
        let body = if rows.is_empty() {
            card(
                theme,
                vec![
                    div()
                        .py(px(14.0))
                        .child(muted(theme, empty.unwrap_or("Nothing here yet."))),
                ],
            )
        } else {
            card(
                theme,
                rows.iter()
                    .map(|row| self.skill_row(theme, row, cx))
                    .collect(),
            )
        };
        section(theme, anchor, title, intro, body)
    }

    fn skill_row(&self, theme: &ArtisanTheme, row: &SkillRow, cx: &mut Context<Self>) -> Div {
        let pending = self.pending.contains(&row.id);
        let selector = format!("marketplace-skill-{}", row.id);
        let control: Option<AnyElement> = if row.origin == SkillOrigin::Discovered {
            let id = row.id.clone();
            let this = cx.entity().downgrade();
            Button::new(
                SharedString::from(format!("{selector}-adopt")),
                self.focus.clone(),
                *theme,
                MotionPolicy::Reduced,
                ButtonVariant::Outline,
                ButtonSize::Small,
                ButtonContent::icon_text(AssetId::TABLER_DOWNLOAD, "Adopt"),
            )
            .ok()
            .map(|button| {
                button
                    .disabled(pending)
                    .debug_selector(format!("{selector}-adopt"))
                    .on_activate(move |_, _, cx| {
                        let _ = this.update(cx, |this, cx| {
                            this.skill_action(id.clone(), "adopt", cx);
                        });
                    })
                    .into_any_element()
            })
        } else {
            let id = row.id.clone();
            let this = cx.entity().downgrade();
            Some(
                Switch::new(
                    SharedString::from(format!("{selector}-switch")),
                    self.focus.clone(),
                    *theme,
                    SwitchSize::Default,
                    row.enabled,
                )
                .disabled(pending)
                .debug_selector(format!("{selector}-switch"))
                .on_change(move |enabled, _, _, cx| {
                    let id = id.clone();
                    let _ = this.update(cx, |this, cx| {
                        this.skill_action(id, if enabled { "enable" } else { "disable" }, cx);
                    });
                })
                .into_any_element(),
            )
        };
        let mut meta = row.size.clone();
        if let Some(found_in) = &row.found_in {
            meta = format!("{found_in} \u{b7} {meta}");
        }
        let mut text = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_baseline()
                    .gap(px(8.0))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.colors.foreground.to_paint())
                            .child(row.name.clone()),
                    )
                    .child(muted_xs(theme, &meta)),
            );
        if !row.description.is_empty() {
            text = text.child(
                div()
                    .text_xs()
                    .line_clamp(2)
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(row.description.clone()),
            );
        }
        if let Some(attention) = &row.attention {
            text = text.child(attention_line(theme, attention));
        }
        text = text.child(chips_row(theme, &row.chips, &row.id));
        div()
            .flex()
            .flex_row()
            .items_start()
            .gap(px(16.0))
            .py(px(14.0))
            .debug_selector(move || selector.clone())
            .child(text)
            .children(control.map(|control| div().flex_shrink_0().pt(px(2.0)).child(control)))
    }
}

fn header(theme: &ArtisanTheme) -> Div {
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .text_size(px(20.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.colors.foreground.to_paint())
                .child("Marketplace"),
        )
        .child(
            div()
                .mt(px(6.0))
                .text_sm()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(
                    "Skills and the global prompt Artisan keeps in every engine. Changes reach each engine on its next session.",
                ),
        )
}

fn section(
    theme: &ArtisanTheme,
    anchor: &'static str,
    title: &str,
    intro: Option<&str>,
    body: Div,
) -> Div {
    let mut head = div().flex().flex_col().child(
        div()
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.colors.foreground.to_paint())
            .child(title.to_owned()),
    );
    if let Some(intro) = intro {
        head = head.child(div().mt(px(4.0)).child(muted(theme, intro)));
    }
    div()
        .flex()
        .flex_col()
        .mt(px(40.0))
        .debug_selector(move || format!("marketplace-section-{anchor}"))
        .child(head)
        .child(div().mt(px(12.0)).child(body))
}

fn card(theme: &ArtisanTheme, rows: Vec<Div>) -> Div {
    let style = CardStyle::resolve(*theme);
    let border = theme.colors.border.to_paint();
    let mut card = compact_card(style).w_full().gap(px(0.0)).py(px(0.0));
    for (index, row) in rows.into_iter().enumerate() {
        let mut band = compact_card_content(style).w_full().child(row);
        if index > 0 {
            band = band.border_t_1().border_color(border);
        }
        card = card.child(band);
    }
    card
}

fn attention_line(theme: &ArtisanTheme, text: &str) -> Div {
    let tone = tone_color(theme, ChipTone::Warning);
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .text_xs()
        .text_color(tone.to_paint())
        .child(
            asset_glyph(AssetId::TABLER_ALERT_TRIANGLE)
                .size(px(12.0))
                .text_color(tone.with_alpha(0.8).to_paint()),
        )
        .child(text.to_owned())
}

fn notice_box(theme: &ArtisanTheme, text: &str) -> Div {
    let tone = tone_color(theme, ChipTone::Danger);
    div()
        .mt(px(24.0))
        .px(px(12.0))
        .py(px(10.0))
        .rounded(px(10.0))
        .bg(tone.with_alpha(0.10).to_paint())
        .text_sm()
        .text_color(tone.to_paint())
        .debug_selector(|| "marketplace-notice".to_owned())
        .child(text.to_owned())
}

fn muted(theme: &ArtisanTheme, text: &str) -> Div {
    div()
        .text_sm()
        .text_color(theme.colors.muted_foreground.to_paint())
        .child(text.to_owned())
}

fn muted_xs(theme: &ArtisanTheme, text: &str) -> Div {
    div()
        .text_xs()
        .text_color(theme.colors.muted_foreground.to_paint())
        .child(text.to_owned())
}
