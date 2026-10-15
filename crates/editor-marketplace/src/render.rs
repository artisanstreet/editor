//! Virtualized Marketplace rows.

use crate::presentation::{SkillRow, prompt_summary, sections};
use crate::{MARKETPLACE_SCREEN_SELECTOR, Marketplace};
use artisan_api::SkillOrigin;
use editor_core::{Screen, SettingsPage};
use editor_ui::AssetId;
use editor_ui::asset_seam::asset_glyph;
use editor_ui::button::{Button, ButtonVariant};
use editor_ui::card::{CardStyle, compact_card, compact_card_content};
use editor_ui::status_chip::{ChipTone, chips_row, tone_color};
use editor_ui::switch::{Switch, SwitchSize};
use editor_ui::theme::ArtisanTheme;
use editor_ui::wordmark_loader::WordmarkLoader;
use gpui::prelude::*;
use gpui::{AnyElement, Context, Div, FontWeight, SharedString, Window, div, list, px};

/// Content width, matching Settings.
const CONTENT_MAX_WIDTH_PX: f32 = 896.0;

pub(crate) enum Entry {
    Header,
    Prompt,
    Notices,
    Group {
        title: &'static str,
        intro: Option<&'static str>,
    },
    Empty(&'static str),
    Skill {
        row: SkillRow,
        first: bool,
        last: bool,
    },
}
pub(crate) fn entries(catalog: &[artisan_api::CatalogSkill]) -> Vec<Entry> {
    let sections = sections(catalog);
    let mut entries = vec![Entry::Header, Entry::Prompt, Entry::Notices];
    for (title, intro, rows, empty) in [
        (
            "Artisan skills",
            Some("Skills that teach every engine how Artisan works."),
            sections.artisan,
            Some("Nothing here yet."),
        ),
        (
            "Your skills",
            None,
            sections.adopted,
            Some("Adopt a skill below to keep it in every engine."),
        ),
        (
            "Found in your engines",
            Some(
                "Skills an engine has on its own. Adopting one moves each copy into Artisan's backups and keeps Artisan's copy in every engine.",
            ),
            sections.discovered,
            None,
        ),
    ] {
        if rows.is_empty() && empty.is_none() {
            continue;
        }
        entries.push(Entry::Group { title, intro });
        if rows.is_empty() {
            if let Some(text) = empty {
                entries.push(Entry::Empty(text));
            }
        }
        let count = rows.len();
        entries.extend(
            rows.into_iter()
                .enumerate()
                .map(|(index, row)| Entry::Skill {
                    row,
                    first: index == 0,
                    last: index + 1 == count,
                }),
        );
    }
    entries
}
impl Render for Marketplace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        div()
            .id("marketplace-screen")
            .debug_selector(|| MARKETPLACE_SCREEN_SELECTOR.to_owned())
            .size_full()
            .min_h_0()
            .bg(theme.colors.background.to_paint())
            .child(
                list(
                    self.list.clone(),
                    cx.processor(move |this, index, _, cx| {
                        let content = match &this.entries[index] {
                            Entry::Header => {
                                div().pt(px(48.0)).child(header(&theme)).into_any_element()
                            }
                            Entry::Prompt => this.prompt_section(&theme, cx).into_any_element(),
                            Entry::Notices => this.notices(&theme, cx).into_any_element(),
                            Entry::Group { title, intro } => div()
                                .pt(px(40.0))
                                .pb(px(12.0))
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(*title),
                                )
                                .children(intro.map(|text| muted(&theme, text)))
                                .into_any_element(),
                            Entry::Empty(text) => {
                                card(&theme, vec![muted(&theme, *text).py(px(14.0))])
                                    .into_any_element()
                            }
                            Entry::Skill { row, first, last } => {
                                let style = CardStyle::resolve(theme);
                                let mut band = div()
                                    .w_full()
                                    .bg(style.background)
                                    .text_color(style.foreground)
                                    .text_size(style.text_size)
                                    .px(style.content_horizontal_padding)
                                    .border_x_1()
                                    .border_color(style.ring_color)
                                    .child(this.skill_row(&theme, row, cx));
                                if *first {
                                    band = band.rounded_t(style.corner_radius).border_t_1();
                                } else {
                                    band = band
                                        .border_t_1()
                                        .border_color(theme.colors.border.to_paint());
                                }
                                if *last {
                                    band = band.rounded_b(style.corner_radius).border_b_1();
                                }
                                band.into_any_element()
                            }
                        };
                        div()
                            .w_full()
                            .flex()
                            .justify_center()
                            .child(
                                div()
                                    .w_full()
                                    .max_w(px(CONTENT_MAX_WIDTH_PX))
                                    .px(px(24.0))
                                    .child(content),
                            )
                            .into_any_element()
                    }),
                )
                .size_full(),
            )
    }
}

impl Marketplace {
    fn prompt_section(&self, theme: &ArtisanTheme, cx: &mut Context<Self>) -> Div {
        let prompt = self
            .core
            .read(cx)
            .active_host()
            .and_then(|host| host.read(cx).global_prompt.clone());
        let summary = prompt.as_deref().map(prompt_summary);
        let choice = prompt
            .as_ref()
            .is_some_and(|prompt| !prompt.candidates.is_empty());
        let this = cx.entity().downgrade();
        let action = Button::text(
            "marketplace-global-prompt-open",
            *theme,
            if choice { "Choose" } else { "Edit" },
        )
        .variant(if choice {
            ButtonVariant::Default
        } else {
            ButtonVariant::Outline
        })
        .debug_selector("marketplace-global-prompt-open")
        .on_activate(move |_, _, cx| {
            let _ = this.update(cx, |this, cx| {
                this.navigate(Screen::Settings(SettingsPage::GlobalPrompt), cx)
            });
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
            .child(action);
        section(
            theme,
            "global-prompt",
            "Global prompt",
            Some("Standing instructions every engine reads before your messages."),
            card(theme, vec![row]),
        )
    }

    fn notices(&self, theme: &ArtisanTheme, cx: &mut Context<Self>) -> Div {
        let Some(host) = self.core.read(cx).active_host().cloned() else {
            return div().child(muted(theme, "Pair a host to see its skills."));
        };
        let (catalog_error, prompt_error, loading) = {
            let host = host.read(cx);
            (
                host.catalog_load().error.clone(),
                host.prompt_load().error.clone(),
                host.catalog_load().loading,
            )
        };
        let retry = catalog_error.is_some() || prompt_error.is_some();
        let mut body = div().flex().flex_col();
        for error in [
            catalog_error.as_deref(),
            prompt_error.as_deref(),
            self.notice.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            body = body.child(notice_box(theme, error));
        }
        if retry {
            body = body.child(
                Button::text("marketplace-retry", *theme, "Retry")
                    .variant(ButtonVariant::Outline)
                    .on_activate(move |_, _, cx| {
                        host.update(cx, |host, cx| {
                            host.load_prompt(true, cx);
                            host.load_skill_catalog(true, cx);
                        });
                    }),
            );
        }
        if loading {
            body = body.child(WordmarkLoader::new("marketplace-loading", *theme));
        }
        body
    }

    fn skill_row(&self, theme: &ArtisanTheme, row: &SkillRow, cx: &mut Context<Self>) -> Div {
        let pending = self.pending.is_some();
        let selector = format!("marketplace-skill-{}", row.id);
        let control: Option<AnyElement> = if row.origin == SkillOrigin::Discovered {
            let id = row.id.clone();
            let this = cx.entity().downgrade();
            Some(
                Button::icon_text(
                    SharedString::from(format!("{selector}-adopt")),
                    *theme,
                    AssetId::TABLER_DOWNLOAD,
                    "Adopt",
                )
                .variant(ButtonVariant::Outline)
                .disabled(pending)
                .debug_selector(format!("{selector}-adopt"))
                .on_activate(move |_, _, cx| {
                    let _ = this.update(cx, |this, cx| {
                        this.skill_action(id.clone(), artisan_api::SkillAction::Adopt, cx)
                    });
                })
                .into_any_element(),
            )
        } else {
            let id = row.id.clone();
            let this = cx.entity().downgrade();
            Some(
                Switch::new(
                    SharedString::from(format!("{selector}-switch")),
                    *theme,
                    SwitchSize::Default,
                    row.enabled,
                    format!("Enable {}", row.name),
                )
                .disabled(pending)
                .debug_selector(format!("{selector}-switch"))
                .on_change(move |enabled, _, _, cx| {
                    let id = id.clone();
                    let _ = this.update(cx, |this, cx| {
                        this.skill_action(
                            id,
                            if enabled {
                                artisan_api::SkillAction::Enable
                            } else {
                                artisan_api::SkillAction::Disable
                            },
                            cx,
                        );
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
