//! Virtualized Marketplace rows: the search hero, then what it finds.

use crate::presentation::{EngineMark, Item, Kind, SkillRow, found_summary, prompt_attention};
use crate::{MARKETPLACE_SCREEN_SELECTOR, MARKETPLACE_SEARCH_SELECTOR, Marketplace, Remote};
use artisan_api::{SkillAction, SkillOrigin};
use editor_core::{Screen, SettingsPage};
use editor_ui::AssetId;
use editor_ui::asset_seam::asset_glyph;
use editor_ui::button::{Button, ButtonVariant};
use editor_ui::card::CardStyle;
use editor_ui::glass::{
    GlassStrength, glass_blur_radius, glass_card_shadows, glass_foreground_base,
    glass_highlight_layer, glass_material_layer,
};
use editor_ui::hover_card::{glass_tooltip, tooltip};
use editor_ui::page;
use editor_ui::status_chip::{ChipTone, tone_color};
use editor_ui::switch::{Switch, SwitchSize};
use editor_ui::theme::{
    ActiveTheme as _, ArtisanTheme, Elevation, Opacity, RadiusStep, RadiusTokens, TextStep,
    TypographyTokens,
};
use editor_ui::wordmark_loader::WordmarkLoader;
use gpui::prelude::*;
use gpui::{
    AnyElement, Context, Div, Focusable as _, FontWeight, SharedString, Window, div, list, px,
};

/// The search box's width; results run the page's full width.
const SEARCH_MAX_WIDTH: f32 = 720.0;
/// The widest row control: Discard beside "Install ↵".
const CONTROL_WIDTH: f32 = 148.0;

pub enum Entry {
    Hero,
    Notices,
    /// Skills the engines have on their own: one line until opened.
    Found(String),
    Group(String),
    Empty(String),
    Item {
        item: Item,
        first: bool,
        last: bool,
        selected: bool,
    },
}

pub fn entries(page: &Marketplace) -> Vec<Entry> {
    let mut entries = vec![Entry::Hero, Entry::Notices];
    let results = page.results();
    let group = |entries: &mut Vec<Entry>, items: Vec<&Item>, selected: Option<usize>| {
        let count = items.len();
        entries.extend(
            items
                .into_iter()
                .enumerate()
                .map(|(index, item)| Entry::Item {
                    item: item.clone(),
                    first: index == 0,
                    last: index + 1 == count,
                    selected: selected == Some(index),
                }),
        );
    };
    if !page.query.trim().is_empty() {
        // One highlight runs through both groups: the host's, then skills.sh's.
        let selected = page.selected.min(results.len().saturating_sub(1));
        let (local, remote): (Vec<&Item>, Vec<&Item>) = results
            .iter()
            .partition(|item| !matches!(item, Item::Listing(_)));
        let local_count = local.len();
        if !local.is_empty() {
            entries.push(Entry::Group("On this host".to_owned()));
            group(&mut entries, local, Some(selected));
        }
        match &page.listings {
            Remote::Searching => {
                entries.push(Entry::Group("Searching skills.sh\u{2026}".to_owned()));
            }
            Remote::Failed(error) => entries.push(Entry::Group(error.clone())),
            Remote::Found(_) if !remote.is_empty() => {
                entries.push(Entry::Group("skills.sh".to_owned()));
                group(&mut entries, remote, selected.checked_sub(local_count));
            }
            Remote::Found(_) | Remote::Idle => {}
        }
        if results.is_empty() && !matches!(page.listings, Remote::Searching) {
            entries.push(Entry::Empty(format!(
                "Nothing called \u{201c}{}\u{201d}.",
                page.query.trim()
            )));
        }
        return entries;
    }
    let (installed, found): (Vec<&Item>, Vec<&Item>) =
        results.iter().partition(|item| item.managed());
    if let Some(summary) = found_summary(&page.items).filter(|_| page.kind != Kind::Servers) {
        entries.push(Entry::Found(summary));
        if page.found_open {
            group(&mut entries, found, None);
        }
    }
    entries.push(Entry::Group(format!(
        "Installed \u{b7} {}",
        installed.len()
    )));
    if installed.is_empty() {
        entries.push(Entry::Empty(
            match page.kind {
                Kind::Servers => "No MCP servers yet.",
                Kind::All | Kind::Skills => "Nothing installed yet.",
            }
            .to_owned(),
        ));
    } else {
        group(&mut entries, installed, None);
    }
    entries
}

impl Render for Marketplace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.theme = *cx.theme();
        if self.focus_search {
            self.focus_search = false;
            window.focus(&self.search.focus_handle(cx), cx);
        }
        let theme = self.theme;
        div()
            .id("marketplace-screen")
            .debug_selector(|| MARKETPLACE_SCREEN_SELECTOR.to_owned())
            .size_full()
            .min_h_0()
            .child(
                list(
                    self.list.clone(),
                    cx.processor(move |this, index, _, cx| {
                        let content = match &this.entries[index] {
                            Entry::Hero => this.hero(&theme, cx).into_any_element(),
                            Entry::Notices => this.notices(&theme, cx).into_any_element(),
                            Entry::Found(summary) => {
                                this.found_banner(&theme, summary, cx).into_any_element()
                            }
                            Entry::Group(title) => div()
                                .pt(px(32.0))
                                .pb(px(10.0))
                                .px(px(4.0))
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.colors.text_muted.to_paint())
                                .child(title.clone())
                                .into_any_element(),
                            Entry::Empty(text) => div()
                                .pt(px(40.0))
                                .flex()
                                .justify_center()
                                .child(muted(&theme, text))
                                .debug_selector(|| "marketplace-empty".to_owned())
                                .into_any_element(),
                            Entry::Item {
                                item,
                                first,
                                last,
                                selected,
                            } => band(
                                &theme,
                                *first,
                                *last,
                                this.item_row(&theme, item, *selected, cx),
                            )
                            .into_any_element(),
                        };
                        div()
                            .w_full()
                            .flex()
                            .justify_center()
                            .child(
                                div()
                                    .w_full()
                                    .max_w(px(page::CONTENT_MAX_WIDTH))
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
    /// The title, the search and the kind filter.
    #[expect(
        clippy::too_many_lines,
        reason = "the hero's title, search field and filter chips build one element tree"
    )]
    fn hero(&self, theme: &ArtisanTheme, cx: &Context<Self>) -> Div {
        let radius = RadiusTokens::value(RadiusStep::X2l);
        let search = self.search.clone();
        let this = cx.entity().downgrade();
        let clear = (!self.query.is_empty()).then(|| {
            let this = this.clone();
            Button::icon(
                "marketplace-search-clear",
                *theme,
                AssetId::TABLER_X,
                "Clear the search",
            )
            .variant(ButtonVariant::Ghost)
            .debug_selector("marketplace-search-clear")
            .on_activate(move |_, _, cx| {
                this.update(cx, Self::clear_search).ok();
            })
        });
        let field = div()
            .id("marketplace-search-field")
            .debug_selector(|| MARKETPLACE_SEARCH_SELECTOR.to_owned())
            .relative()
            .w_full()
            .h(px(56.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.0))
            .pl(px(20.0))
            .pr(px(10.0))
            .rounded(radius)
            .backdrop_blur(glass_blur_radius(GlassStrength::Quiet))
            .bg(glass_foreground_base(theme))
            .shadow(glass_card_shadows(theme, Elevation::Floating))
            .cursor_text()
            .on_click(move |_, window, cx| window.focus(&search.focus_handle(cx), cx))
            .child(glass_material_layer(theme, GlassStrength::Quiet, radius))
            .child(glass_highlight_layer(theme, GlassStrength::Quiet, radius))
            .child(
                asset_glyph(AssetId::TABLER_SEARCH)
                    .size(px(18.0))
                    .text_color(theme.colors.text_muted.to_paint()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(TypographyTokens::size(TextStep::Lg))
                    .text_color(theme.colors.text.to_paint())
                    .child(self.search.clone()),
            )
            .children(clear);
        let counts = |kind: Kind| {
            self.items
                .iter()
                .filter(|item| item.managed() && kind.admits(item))
                .count()
        };
        let tabs = [
            (Kind::All, "All".to_owned()),
            (Kind::Skills, format!("Skills {}", counts(Kind::Skills))),
            (
                Kind::Servers,
                format!("MCP servers {}", counts(Kind::Servers)),
            ),
        ]
        .into_iter()
        .map(|(kind, label)| {
            let chosen = self.kind == kind;
            let this = this.clone();
            let text = theme.colors.text.to_paint();
            div()
                .id(SharedString::from(format!("marketplace-kind-{kind:?}")))
                .debug_selector(move || format!("marketplace-kind-{kind:?}"))
                .text_sm()
                .cursor_pointer()
                .text_color(if chosen {
                    text
                } else {
                    theme.colors.text_muted.to_paint()
                })
                .when(chosen, |tab| tab.font_weight(FontWeight::MEDIUM))
                .hover(move |style| style.text_color(text))
                .on_click(move |_, _, cx| {
                    this.update(cx, |this, cx| this.set_kind(kind, cx)).ok();
                })
                .child(label)
        });
        let prompt = {
            let this = this.clone();
            let text = theme.colors.text.to_paint();
            div()
                .id("marketplace-global-prompt-open")
                .debug_selector(|| "marketplace-global-prompt-open".to_owned())
                .flex()
                .flex_row()
                .items_center()
                .gap(px(2.0))
                .text_sm()
                .cursor_pointer()
                .text_color(theme.colors.text_muted.to_paint())
                .hover(move |style| style.text_color(text))
                .on_click(move |_, _, cx| {
                    this.update(cx, |this, cx| {
                        this.navigate(Screen::Settings(SettingsPage::GlobalPrompt), cx);
                    })
                    .ok();
                })
                .child("Global prompt")
                .child(asset_glyph(AssetId::TABLER_CHEVRON_RIGHT).size(px(14.0)))
        };
        div()
            .pt(px(72.0))
            .pb(px(8.0))
            .flex()
            .flex_col()
            .items_center()
            .child(
                div()
                    .text_size(TypographyTokens::size(TextStep::Xl))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.colors.text.to_paint())
                    .child("Marketplace"),
            )
            .child(
                div()
                    .mt(px(20.0))
                    .w_full()
                    .max_w(px(SEARCH_MAX_WIDTH))
                    .child(field)
                    .child(
                        div()
                            .mt(px(14.0))
                            .px(px(8.0))
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(div().flex().flex_row().gap(px(18.0)).children(tabs))
                            .child(prompt),
                    ),
            )
    }

    fn notices(&self, theme: &ArtisanTheme, cx: &Context<Self>) -> Div {
        let Some(host) = self.core.read(cx).active_host().cloned() else {
            return div()
                .pt(px(24.0))
                .flex()
                .justify_center()
                .child(muted(theme, "Pair a host to see its skills."));
        };
        let (catalog_error, prompt_error, loading, prompt) = {
            let host = host.read(cx);
            (
                host.catalog_load().error.clone(),
                host.prompt_load().error.clone(),
                host.catalog_load().loading,
                host.global_prompt.clone(),
            )
        };
        let retry = catalog_error.is_some() || prompt_error.is_some();
        let mut body = div().flex().flex_col().gap(px(8.0));
        if let Some(attention) = prompt.as_deref().and_then(prompt_attention) {
            let this = cx.entity().downgrade();
            body = body.child(
                page::notice(theme, attention)
                    .action(
                        Button::text("marketplace-prompt-choose", *theme, "Open")
                            .variant(ButtonVariant::Outline)
                            .on_activate(move |_, _, cx| {
                                this.update(cx, |this, cx| {
                                    this.navigate(Screen::Settings(SettingsPage::GlobalPrompt), cx);
                                })
                                .ok();
                            }),
                    )
                    .debug_selector("marketplace-prompt-attention"),
            );
        }
        for error in [
            catalog_error.as_deref(),
            prompt_error.as_deref(),
            self.notice.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            body = body
                .child(page::notice(theme, error.to_owned()).debug_selector("marketplace-notice"));
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
        if loading && self.items.is_empty() {
            body = body.child(
                div()
                    .pt(px(24.0))
                    .flex()
                    .justify_center()
                    .child(WordmarkLoader::new("marketplace-loading", *theme)),
            );
        }
        body.pt(px(16.0))
    }

    /// One line about the skills Forge quarantined, with a way to list them
    /// to install or discard.
    fn found_banner(&self, theme: &ArtisanTheme, summary: &str, cx: &Context<Self>) -> Div {
        let this = cx.entity().downgrade();
        let open = self.found_open;
        let style = CardStyle::resolve(*theme);
        div().pt(px(24.0)).child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(12.0))
                .px(style.content_horizontal_padding)
                .py(px(10.0))
                .rounded(style.corner_radius)
                .border_1()
                .border_color(style.ring_color)
                .bg(style.background)
                .debug_selector(|| "marketplace-found".to_owned())
                .child(
                    asset_glyph(AssetId::TABLER_SHIELD_LOCK)
                        .size(px(16.0))
                        .text_color(theme.colors.text_muted.to_paint()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .text_color(theme.colors.text.to_paint())
                        .child(summary.to_owned()),
                )
                .child(
                    Button::text(
                        "marketplace-found-review",
                        *theme,
                        if open { "Hide" } else { "Review" },
                    )
                    .variant(ButtonVariant::Outline)
                    .debug_selector("marketplace-found-review")
                    .on_activate(move |_, _, cx| {
                        this.update(cx, |this, cx| {
                            this.found_open = !this.found_open;
                            this.rebuild(true);
                            cx.notify();
                        })
                        .ok();
                    }),
                ),
        )
    }

    fn item_row(
        &self,
        theme: &ArtisanTheme,
        item: &Item,
        selected: bool,
        cx: &Context<Self>,
    ) -> Div {
        let selector = item.selector();
        let (icon, meta, description, marks, attention) = match item {
            Item::Skill(row) => {
                let icon = match row.origin {
                    SkillOrigin::FirstParty => AssetId::ARTISAN_STARBURST,
                    SkillOrigin::User | SkillOrigin::Discovered => AssetId::TABLER_SPARKLES,
                };
                (
                    icon,
                    skill_meta(row),
                    row.description.clone(),
                    row.marks.clone(),
                    row.attention.clone(),
                )
            }
            Item::Server(row) => (
                AssetId::TABLER_PLUG_CONNECTED,
                "MCP server".to_owned(),
                row.target.clone(),
                Vec::new(),
                None,
            ),
            Item::Listing(row) => (
                AssetId::TABLER_WORLD,
                row.meta.clone(),
                String::new(),
                Vec::new(),
                None,
            ),
        };
        let title_color = if selected {
            theme.colors.link.to_paint()
        } else {
            theme.colors.text.to_paint()
        };
        let mut text = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_baseline()
                    .gap(px(8.0))
                    .min_w_0()
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(title_color)
                            .child(item.name().to_owned()),
                    )
                    .child(muted_xs(theme, &meta).truncate()),
            );
        if !description.is_empty() {
            text = text.child(muted_xs(theme, &description).truncate());
        }
        if let Some(attention) = &attention {
            text = text.child(attention_line(theme, attention));
        }
        let accent = theme.colors.link.to_paint();
        div()
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(14.0))
            .py(px(12.0))
            .debug_selector({
                let selector = selector.clone();
                move || selector
            })
            .when(selected, |row| {
                row.child(
                    div()
                        .absolute()
                        .left(px(-14.0))
                        .top(px(12.0))
                        .bottom(px(12.0))
                        .w(px(2.0))
                        .rounded_full()
                        .bg(accent),
                )
            })
            .child(icon_tile(theme, icon))
            .child(text)
            .child(engine_marks(theme, &selector, &marks))
            .child(
                // One width for switch and buttons keeps the marks in a column.
                div()
                    .flex_shrink_0()
                    .w(px(CONTROL_WIDTH))
                    .flex()
                    .justify_end()
                    .child(self.control(theme, item, selected, cx)),
            )
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one control per item kind and state, kept side by side"
    )]
    fn control(
        &self,

        theme: &ArtisanTheme,
        item: &Item,
        selected: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let pending = self.pending.is_some();
        let selector = item.selector();
        let this = cx.entity().downgrade();
        match item {
            Item::Skill(row) if row.origin == SkillOrigin::Discovered => {
                let install = {
                    let id = row.id.clone();
                    let this = this.clone();
                    Button::icon_text(
                        SharedString::from(format!("{selector}-install")),
                        *theme,
                        AssetId::TABLER_DOWNLOAD,
                        if selected {
                            "Install \u{21b5}"
                        } else {
                            "Install"
                        },
                    )
                    .variant(if selected {
                        ButtonVariant::Default
                    } else {
                        ButtonVariant::Outline
                    })
                    .disabled(pending)
                    .debug_selector(format!("{selector}-install"))
                    .on_activate(move |_, _, cx| {
                        this.update(cx, |this, cx| {
                            this.skill_action(id.clone(), SkillAction::Adopt, cx);
                        })
                        .ok();
                    })
                };
                let id = row.id.clone();
                let discard = Button::icon(
                    SharedString::from(format!("{selector}-discard")),
                    *theme,
                    AssetId::TABLER_TRASH,
                    "Discard",
                )
                .variant(ButtonVariant::Ghost)
                .disabled(pending)
                .debug_selector(format!("{selector}-discard"))
                .on_activate(move |_, _, cx| {
                    this.update(cx, |this, cx| {
                        this.skill_action(id.clone(), SkillAction::Discard, cx);
                    })
                    .ok();
                });
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .child(discard)
                    .child(install)
                    .into_any_element()
            }
            Item::Skill(row) => {
                let id = row.id.clone();
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
                    this.update(cx, |this, cx| {
                        let action = if enabled {
                            SkillAction::Enable
                        } else {
                            SkillAction::Disable
                        };
                        this.skill_action(id, action, cx);
                    })
                    .ok();
                })
                .into_any_element()
            }
            Item::Listing(row) => {
                let id = row.id.clone();
                let installing = self.pending.as_deref() == Some(row.id.as_str());
                Button::icon_text(
                    SharedString::from(format!("{selector}-install")),
                    *theme,
                    AssetId::TABLER_DOWNLOAD,
                    if installing {
                        "Installing\u{2026}"
                    } else if selected {
                        "Install \u{21b5}"
                    } else {
                        "Install"
                    },
                )
                .variant(if selected {
                    ButtonVariant::Default
                } else {
                    ButtonVariant::Outline
                })
                .disabled(pending)
                .debug_selector(format!("{selector}-install"))
                .on_activate(move |_, _, cx| {
                    this.update(cx, |this, cx| this.install_listing(id.clone(), cx))
                        .ok();
                })
                .into_any_element()
            }
            Item::Server(_) => Button::text(
                SharedString::from(format!("{selector}-edit")),
                *theme,
                if selected { "Edit \u{21b5}" } else { "Edit" },
            )
            .variant(ButtonVariant::Outline)
            .debug_selector(format!("{selector}-edit"))
            .on_activate(move |_, _, cx| {
                this.update(cx, |this, cx| {
                    this.navigate(Screen::Settings(SettingsPage::Extensions), cx);
                })
                .ok();
            })
            .into_any_element(),
        }
    }
}

/// `"Artisan · 2 files · 22 KB"`, `"Quarantined from Codex · 1 file · 13 KB"`.
fn skill_meta(row: &SkillRow) -> String {
    let source = match row.origin {
        SkillOrigin::FirstParty => "Artisan".to_owned(),
        SkillOrigin::User => "Your skill".to_owned(),
        SkillOrigin::Discovered => row
            .found_in
            .clone()
            .unwrap_or_else(|| "Quarantined".to_owned()),
    };
    format!("{source} \u{b7} {}", row.size)
}

/// A row's card band: the first and last round the card's corners.
fn band(theme: &ArtisanTheme, first: bool, last: bool, row: Div) -> Div {
    let style = CardStyle::resolve(*theme);
    let mut band = div()
        .w_full()
        .bg(style.background)
        .text_color(style.foreground)
        .text_size(style.text_size)
        .px(style.content_horizontal_padding)
        .border_x_1()
        .border_color(style.ring_color)
        .child(row);
    if first {
        band = band.rounded_t(style.corner_radius).border_t_1();
    } else {
        band = band.border_t_1().border_color(theme.colors.line.to_paint());
    }
    if last {
        band = band.rounded_b(style.corner_radius).border_b_1();
    }
    band
}

fn icon_tile(theme: &ArtisanTheme, icon: AssetId) -> Div {
    div()
        .flex_shrink_0()
        .size(px(32.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(RadiusTokens::value(RadiusStep::Lg))
        .bg(theme.colors.hover.to_paint())
        .child(
            asset_glyph(icon)
                .size(px(16.0))
                .text_color(theme.colors.text_muted.to_paint()),
        )
}

/// Each engine's logo, dimmed where the engine doesn't have the item; the
/// tooltip says why.
fn engine_marks(theme: &ArtisanTheme, selector: &str, marks: &[EngineMark]) -> Div {
    let theme = *theme;
    div()
        .flex_shrink_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .children(marks.iter().map(|mark| {
            let label = mark.label.clone();
            let id = format!("{selector}-mark-{}", mark.engine.as_str());
            div()
                .id(SharedString::from(id.clone()))
                .debug_selector(move || id)
                .when(!mark.lit, |mark| mark.opacity(0.3))
                .tooltip(move |_, cx| {
                    let label = label.clone();
                    tooltip(
                        move |_, _| {
                            glass_tooltip("marketplace-mark-tip", &theme, px(260.0), false)
                                .text_color(theme.colors.text.to_paint())
                                .child(label.clone())
                                .into_any_element()
                        },
                        cx,
                    )
                })
                .child(asset_glyph(editor_ui::engine_icon(&theme, mark.engine)).size(px(14.0)))
        }))
}

fn attention_line(theme: &ArtisanTheme, text: &str) -> Div {
    let tone = tone_color(theme, ChipTone::Warning);
    div()
        .mt(px(2.0))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .text_xs()
        .text_color(tone.to_paint())
        .child(
            asset_glyph(AssetId::TABLER_ALERT_TRIANGLE)
                .size(px(12.0))
                .text_color(tone.at(Opacity::ToneIcon).to_paint()),
        )
        .child(text.to_owned())
}

fn muted(theme: &ArtisanTheme, text: &str) -> Div {
    div()
        .text_sm()
        .text_color(theme.colors.text_muted.to_paint())
        .child(text.to_owned())
}

fn muted_xs(theme: &ArtisanTheme, text: &str) -> Div {
    div()
        .text_xs()
        .text_color(theme.colors.text_muted.to_paint())
        .child(text.to_owned())
}
