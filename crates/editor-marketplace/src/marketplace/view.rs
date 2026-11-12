use std::rc::Rc;

use aqua_core::Insets;
use aqua_text::{FontFamily, FontWeight, TextStyle};
use aqua_ui::{
    AlignItems, JustifyContent,
    collection::List,
    element::{Box as UiBox, Element, Text},
    style::{Length, Style},
};
use artisan_api::{EngineId, SkillAction, SkillOrigin};
use editor_assets::AssetId;

use super::{
    Action, Entry, Operation,
    controller::MarketplaceController,
    entries,
    presentation::{self, Item, Kind},
};
use editor_core::{Core, Screen, SettingsPage};
use editor_ui::{
    asset::source_asset_glyph,
    button::{Button, ButtonVariant},
    card::CardStyle,
    glass::{self, GlassStrength},
    page,
    status_chip::{ChipTone, tone_color},
    switch::Switch,
    text::{ListKey, Mode},
    theme::{ArtisanTheme, Opacity, RadiusStep, RadiusTokens, TextStep, TypographyTokens},
};

#[derive(Clone, Debug)]
pub enum ViewAction {
    SearchChanged,
    FocusSearch,
    PromptHover(bool),
    SearchKey(ListKey),
    ClearSearch,
    SetKind(Kind),
    ToggleFound,
    Retry,
    Navigate(Screen),
    Operate(Operation),
}

pub fn dispatch(
    controller: &mut MarketplaceController,
    core: &mut Core,
    action: ViewAction,
) -> bool {
    let before = controller.view_snapshot(core);
    match action {
        ViewAction::SearchChanged => controller.edited(core),
        ViewAction::FocusSearch => controller.page.focus_search = true,
        ViewAction::PromptHover(hovered) => controller.prompt_hovered = hovered,
        ViewAction::SearchKey(key) => controller.list_key(core, key),
        ViewAction::ClearSearch => controller.clear_search(core),
        ViewAction::SetKind(kind) => controller.page.set_kind(kind),
        ViewAction::ToggleFound => controller.page.found_open = !controller.page.found_open,
        ViewAction::Retry => controller.retry(core),
        ViewAction::Navigate(screen) => controller.act(core, Action::Navigate(screen)),
        ViewAction::Operate(operation) => controller.operate(core, operation),
    }
    before != controller.view_snapshot(core)
}

#[derive(Clone)]
struct Notices {
    host: bool,
    attention: Option<String>,
    errors: Vec<String>,
    retry: bool,
    loading: bool,
    loading_started: Option<std::time::Instant>,
    reduced_motion: bool,
}

pub fn build(
    controller: &MarketplaceController,
    core: &Core,
    theme: ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let page = &controller.page;
    let host = core.active_host();
    let notices = Notices {
        host: host.is_some(),
        attention: host
            .and_then(|host| host.global_prompt.as_deref())
            .and_then(presentation::prompt_attention),
        errors: host
            .into_iter()
            .flat_map(|host| {
                [
                    host.catalog_load().error.clone(),
                    host.prompt_load().error.clone(),
                ]
            })
            .chain([page.notice.clone()])
            .flatten()
            .collect(),
        loading_started: controller.loading_started(),
        reduced_motion: controller.reduced_motion,
        retry: host.is_some_and(|host| {
            host.catalog_load().error.is_some() || host.prompt_load().error.is_some()
        }),
        loading: host.is_some_and(|host| host.catalog_load().loading) && page.items.is_empty(),
    };
    let rows: Vec<(usize, Entry)> = entries(page).into_iter().enumerate().collect();
    let query = page.query.clone();
    let kind = page.kind;
    let found_open = page.found_open;
    let prompt_hovered = controller.prompt_hovered;
    let counts = [Kind::Skills, Kind::Servers].map(|kind| {
        page.items
            .iter()
            .filter(|item| item.managed() && kind.admits(item))
            .count()
    });
    let pending = page.pending.clone();
    let search = controller.search.clone();
    List::new()
        .items(rows)
        .key(|(index, _)| format!("marketplace-entry-{index}"))
        .row(move |(_, entry)| {
            let content = match entry {
                Entry::Hero => hero(
                    &theme,
                    &query,
                    kind,
                    counts,
                    &search,
                    prompt_hovered,
                    emit.clone(),
                ),
                Entry::Notices => notices_view(&theme, &notices, emit.clone()),
                Entry::Found(summary) => found(&theme, summary, found_open, emit.clone()),
                Entry::Group(title) => UiBox::new()
                    .style(Style::new().padding_insets(insets(4.0, 4.0, 32.0, 10.0)))
                    .child(label(
                        &theme,
                        title,
                        12.0,
                        theme.colors.text_muted.to_reference_text_color(),
                        true,
                    ))
                    .build(),
                Entry::Empty(text) => UiBox::new()
                    .style(
                        Style::new()
                            .padding_insets(insets(0.0, 0.0, 40.0, 0.0))
                            .justify(JustifyContent::CENTER),
                    )
                    .child(label(
                        &theme,
                        text,
                        14.0,
                        theme.colors.text_muted.to_reference_text_color(),
                        false,
                    ))
                    .build(),
                Entry::Item {
                    item,
                    first,
                    last,
                    selected,
                } => item_band(
                    &theme,
                    item,
                    *first,
                    *last,
                    *selected,
                    pending.as_deref(),
                    emit.clone(),
                ),
            };
            UiBox::new()
                .width(Length::Pct(100.0))
                .style(Style::new().justify(JustifyContent::CENTER))
                .child(
                    UiBox::new()
                        .layout("column")
                        .width(Length::Pct(100.0))
                        .style(
                            Style::new()
                                .max_width(Length::Px(page::CONTENT_MAX_WIDTH))
                                .padding_insets(insets(24.0, 24.0, 0.0, 0.0)),
                        )
                        .child(content)
                        .build(),
                )
                .build()
        })
        .style(
            Style::new()
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0)),
        )
        .estimated_row_height(Length::Px(256.0))
        .virtualize()
        .build()
        .with_key("marketplace-list")
        .debug_selector("route-marketplace")
}

fn insets(left: f32, right: f32, top: f32, bottom: f32) -> Insets {
    Insets {
        left,
        right,
        top,
        bottom,
    }
}

fn label(
    theme: &ArtisanTheme,
    text: impl Into<String>,
    size: f32,
    color: aqua_core::GpuiTextColor,
    medium: bool,
) -> Element {
    Text::new(text.into())
        .text_style(TextStyle {
            families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
            size_px: size,
            line_height_px: size * 1.25,
            weight: if medium {
                FontWeight::MEDIUM
            } else {
                FontWeight::NORMAL
            },
            ..TextStyle::default()
        })
        .gpui_source_color(color)
        .wrap(false)
        .build()
}

fn truncated_label(theme: &ArtisanTheme, text: impl Into<String>) -> Element {
    Text::new(text.into())
        .text_style(TextStyle {
            families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
            size_px: 12.0,
            line_height_px: 15.0,
            weight: FontWeight::NORMAL,
            ..TextStyle::default()
        })
        .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
        .wrap(false)
        .overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
        .style(Style::new().min_width(Length::Px(0.0)).flex_shrink(1.0))
        .build()
}

fn inherited_label(
    theme: &ArtisanTheme,
    text: impl Into<String>,
    size: f32,
    medium: bool,
) -> Element {
    Text::new(text.into())
        .text_style(TextStyle {
            families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
            size_px: size,
            line_height_px: size * 1.25,
            weight: if medium {
                FontWeight::MEDIUM
            } else {
                FontWeight::NORMAL
            },
            ..TextStyle::default()
        })
        .wrap(false)
        .build()
}

fn hero(
    theme: &ArtisanTheme,
    query: &str,
    kind: Kind,
    counts: [usize; 2],
    search: &editor_ui::text::mount::EditorController,
    prompt_hovered: bool,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let radius = RadiusTokens::value(RadiusStep::X2l);
    let mut field = UiBox::new()
        .width(Length::Pct(100.0))
        .height(56.0)
        .gap(12.0)
        .radius(radius)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .shadows(
                    glass::glass_card_shadows(theme, editor_ui::theme::Elevation::Floating)
                        .into_iter()
                        .map(Into::into)
                        .collect(),
                )
                .padding_insets(insets(20.0, 10.0, 0.0, 0.0)),
        )
        .child(glass::glass_layers(theme, GlassStrength::Quiet, radius))
        .child(source_asset_glyph(
            AssetId::TABLER_SEARCH,
            18.0,
            theme.colors.text_muted.to_reference_paint(),
        ))
        .child(
            UiBox::new()
                .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                .child(
                    search
                        .element_with_chrome(
                            Mode::SingleLine,
                            theme,
                            "Search skills and MCP servers",
                            true,
                            false,
                        )
                        .with_key("marketplace-search")
                        .debug_selector("marketplace-search"),
                )
                .build(),
        );
    if !query.is_empty() {
        let emit = emit.clone();
        field = field.child(
            Button::icon(
                "marketplace-search-clear",
                *theme,
                AssetId::TABLER_X,
                "Clear the search",
            )
            .variant(ButtonVariant::Ghost)
            .on_activate(move |_| emit(ViewAction::ClearSearch))
            .build(),
        );
    }
    let focus = emit.clone();
    let field = field.build().cursor(aqua_core::CursorIcon::Text).on_input(
        aqua_ui::input::InputKind::PointerDown,
        aqua_ui::input::EventPhase::Capture,
        move |_| {
            focus(ViewAction::FocusSearch);
            aqua_ui::input::InputResponse::default()
        },
    );
    let mut tabs = UiBox::new().gap(18.0);
    for (tab, text) in [
        (Kind::All, "All".to_owned()),
        (Kind::Skills, format!("Skills {}", counts[0])),
        (Kind::Servers, format!("MCP servers {}", counts[1])),
    ] {
        let emit = emit.clone();
        let color = if kind == tab {
            theme.colors.text.to_reference_text_color()
        } else {
            theme.colors.text_muted.to_reference_text_color()
        };
        tabs = tabs.child(
            aqua_ui::element::Button::new()
                .style(Style::new().gpui_source_text_color(color))
                .on_activate(move || emit(ViewAction::SetKind(tab)))
                .child(inherited_label(theme, text, 14.0, kind == tab))
                .build()
                .hover_style(
                    Style::new()
                        .gpui_source_text_color(theme.colors.text.to_reference_text_color()),
                )
                .cursor(aqua_core::CursorIcon::Pointer)
                .debug_selector(format!("marketplace-kind-{tab:?}")),
        );
    }
    let action = emit.clone();
    let hover = emit.clone();
    let prompt = aqua_ui::element::Button::new()
        .gap(2.0)
        .style(
            Style::new().gpui_source_text_color(theme.colors.text_muted.to_reference_text_color()),
        )
        .on_activate(move || {
            action(ViewAction::Navigate(Screen::Settings(
                SettingsPage::GlobalPrompt,
            )))
        })
        .child(inherited_label(theme, "Global prompt", 14.0, false))
        .child(source_asset_glyph(
            AssetId::TABLER_CHEVRON_RIGHT,
            14.0,
            if prompt_hovered {
                theme.colors.text
            } else {
                theme.colors.text_muted
            }
            .to_reference_paint(),
        ))
        .build()
        .hover_style(
            Style::new().gpui_source_text_color(theme.colors.text.to_reference_text_color()),
        )
        .cursor(aqua_core::CursorIcon::Pointer)
        .on_hover_change(move |hovered| hover(ViewAction::PromptHover(hovered)))
        .debug_selector("marketplace-global-prompt-open");
    UiBox::new()
        .layout("column")
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .padding_insets(insets(0.0, 0.0, 72.0, 8.0)),
        )
        .child(label(
            theme,
            "Marketplace",
            TypographyTokens::size(TextStep::Xl),
            theme.colors.text.to_reference_text_color(),
            true,
        ))
        .child(
            UiBox::new()
                .layout("column")
                .width(Length::Pct(100.0))
                .style(
                    Style::new()
                        .max_width(Length::Px(720.0))
                        .margin_insets(insets(0.0, 0.0, 20.0, 0.0)),
                )
                .child(field)
                .child(
                    UiBox::new()
                        .style(
                            Style::new()
                                .justify(JustifyContent::SPACE_BETWEEN)
                                .align(AlignItems::CENTER)
                                .padding_insets(insets(8.0, 8.0, 14.0, 0.0)),
                        )
                        .child(tabs.build())
                        .child(prompt)
                        .build(),
                )
                .build(),
        )
        .build()
}

fn notices_view(theme: &ArtisanTheme, state: &Notices, emit: Rc<dyn Fn(ViewAction)>) -> Element {
    if !state.host {
        return UiBox::new()
            .style(
                Style::new()
                    .justify(JustifyContent::CENTER)
                    .padding_insets(insets(0.0, 0.0, 24.0, 0.0)),
            )
            .child(label(
                theme,
                "Pair a host to see its skills.",
                14.0,
                theme.colors.text_muted.to_reference_text_color(),
                false,
            ))
            .build();
    }
    let mut body = UiBox::new()
        .layout("column")
        .gap(8.0)
        .style(Style::new().padding_insets(insets(0.0, 0.0, 16.0, 0.0)));
    if let Some(attention) = &state.attention {
        let emit = emit.clone();
        body = body.child(
            page::notice(theme, attention.clone())
                .action(
                    Button::text("marketplace-prompt-choose", *theme, "Open")
                        .variant(ButtonVariant::Outline)
                        .on_activate(move |_| {
                            emit(ViewAction::Navigate(Screen::Settings(
                                SettingsPage::GlobalPrompt,
                            )))
                        })
                        .build(),
                )
                .build(),
        );
    }
    for error in &state.errors {
        body = body.child(page::notice(theme, error.clone()).build());
    }
    if state.retry {
        let emit = emit.clone();
        body = body.child(
            Button::text("marketplace-retry", *theme, "Retry")
                .variant(ButtonVariant::Outline)
                .on_activate(move |_| emit(ViewAction::Retry))
                .build(),
        );
    }
    if state.loading {
        if let Some(started) = state.loading_started {
            body = body.child(
                UiBox::new()
                    .style(
                        Style::new()
                            .justify(JustifyContent::CENTER)
                            .padding_insets(insets(0.0, 0.0, 24.0, 0.0)),
                    )
                    .child(wordmark(theme, started, state.reduced_motion))
                    .build(),
            );
        }
    }
    body.build()
}

fn found(theme: &ArtisanTheme, summary: &str, open: bool, emit: Rc<dyn Fn(ViewAction)>) -> Element {
    let style = CardStyle::resolve(*theme);
    let content = UiBox::new()
        .gap(12.0)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .padding_insets(insets(0.0, 0.0, 10.0, 10.0)),
        )
        .child(source_asset_glyph(
            AssetId::TABLER_SHIELD_LOCK,
            16.0,
            theme.colors.text_muted.to_reference_paint(),
        ))
        .child(
            UiBox::new()
                .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                .child(label(
                    theme,
                    summary,
                    14.0,
                    theme.colors.text.to_reference_text_color(),
                    false,
                ))
                .build(),
        )
        .child(
            Button::text(
                "marketplace-found-review",
                *theme,
                if open { "Hide" } else { "Review" },
            )
            .variant(ButtonVariant::Outline)
            .on_activate(move |_| emit(ViewAction::ToggleFound))
            .build(),
        )
        .build();
    UiBox::new()
        .layout("column")
        .style(Style::new().padding_insets(insets(0.0, 0.0, 24.0, 0.0)))
        .child(
            UiBox::new()
                .layout("column")
                .radius(style.corner_radius)
                .fill(style.background)
                .border((1.0, style.ring_color))
                .style(Style::new().padding_insets(insets(
                    style.content_horizontal_padding,
                    style.content_horizontal_padding,
                    0.0,
                    0.0,
                )))
                .child(content)
                .build(),
        )
        .build()
}

fn item_band(
    theme: &ArtisanTheme,
    item: &Item,
    first: bool,
    last: bool,
    selected: bool,
    pending: Option<&str>,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let style = CardStyle::resolve(*theme);
    let radius = style.corner_radius;
    let corners = aqua_core::CornerRadii {
        top_left: if first { radius } else { 0.0 },
        top_right: if first { radius } else { 0.0 },
        bottom_left: if last { radius } else { 0.0 },
        bottom_right: if last { radius } else { 0.0 },
    };
    UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .fill(style.background)
        .style(
            Style::new()
                .corners(corners)
                .border_edges(
                    Insets {
                        left: 1.0,
                        right: 1.0,
                        top: 1.0,
                        bottom: if last { 1.0 } else { 0.0 },
                    },
                    if first {
                        style.ring_color
                    } else {
                        theme.colors.line.to_paint()
                    },
                )
                .padding_insets(insets(
                    style.content_horizontal_padding,
                    style.content_horizontal_padding,
                    0.0,
                    0.0,
                )),
        )
        .child(item_row(theme, item, selected, pending, emit))
        .build()
        .debug_selector(item.selector())
}

fn item_row(
    theme: &ArtisanTheme,
    item: &Item,
    selected: bool,
    pending: Option<&str>,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let (icon, meta, description, marks, attention) = match item {
        Item::Skill(row) => (
            if row.origin == SkillOrigin::FirstParty {
                AssetId::ARTISAN_STARBURST
            } else {
                AssetId::TABLER_SPARKLES
            },
            presentation::skill_meta(row),
            row.description.clone(),
            row.marks.clone(),
            row.attention.clone(),
        ),
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
    let text_color = if selected {
        theme.colors.link.to_reference_text_color()
    } else {
        theme.colors.text.to_reference_text_color()
    };
    let mut text = UiBox::new()
        .layout("column")
        .gap(2.0)
        .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
        .child(
            UiBox::new()
                .gap(8.0)
                .style(
                    Style::new()
                        .align(AlignItems::BASELINE)
                        .min_width(Length::Px(0.0)),
                )
                .child(
                    UiBox::new()
                        .style(Style::new().flex_shrink(0.0))
                        .child(label(theme, item.name(), 14.0, text_color, true))
                        .build(),
                )
                .child(truncated_label(theme, meta))
                .build(),
        );
    if !description.is_empty() {
        text = text.child(truncated_label(theme, description));
    }
    if let Some(attention) = attention {
        let color = tone_color(theme, ChipTone::Warning);
        text = text.child(
            UiBox::new()
                .gap(6.0)
                .style(
                    Style::new()
                        .margin_insets(insets(0.0, 0.0, 2.0, 0.0))
                        .align(AlignItems::CENTER),
                )
                .child(source_asset_glyph(
                    AssetId::TABLER_ALERT_TRIANGLE,
                    12.0,
                    color.at(Opacity::ToneIcon).to_reference_paint(),
                ))
                .child(label(
                    theme,
                    attention,
                    12.0,
                    color.to_reference_text_color(),
                    false,
                ))
                .build(),
        );
    }
    let tile = UiBox::new()
        .width(32.0)
        .height(32.0)
        .radius(RadiusTokens::value(RadiusStep::Lg))
        .fill(theme.colors.hover.to_paint())
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .justify(JustifyContent::CENTER)
                .flex_shrink(0.0),
        )
        .child(source_asset_glyph(
            icon,
            16.0,
            theme.colors.text_muted.to_reference_paint(),
        ))
        .build();
    let mut engines = UiBox::new()
        .gap(8.0)
        .style(Style::new().align(AlignItems::CENTER).flex_shrink(0.0));
    for mark in marks {
        let icon = match mark.engine {
            EngineId::Claude => AssetId::SVGL_CLAUDE_AI,
            EngineId::Codex => AssetId::SVGL_OPENAI,
            EngineId::OpenCode if theme.colors.mode.is_dark() => AssetId::BRANDS_OPENCODE,
            EngineId::OpenCode => AssetId::BRANDS_OPENCODE_LIGHT,
        };
        let selector = format!("{}-mark-{}", item.selector(), mark.engine.as_str());
        let tip = mark.label;
        let t = *theme;
        engines = engines.child(
            UiBox::new()
                .opacity(if mark.lit { 1.0 } else { 0.3 })
                .child(source_asset_glyph(
                    icon,
                    14.0,
                    theme.colors.text.to_reference_paint(),
                ))
                .build()
                .debug_selector(selector)
                .pointer_tooltip(move || glass_tooltip(&t, &tip)),
        );
    }
    let marker = selected.then(|| {
        UiBox::new()
            .width(2.0)
            .radius(1.0)
            .fill(theme.colors.link.to_paint())
            .style(Style::new().absolute_at(Some(-14.0), Some(12.0), None, Some(12.0)))
            .build()
    });
    let mut row = UiBox::new()
        .gap(14.0)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .padding_insets(insets(0.0, 0.0, 12.0, 12.0)),
        )
        .child(tile)
        .child(text.build())
        .child(engines.build())
        .child(
            UiBox::new()
                .width(148.0)
                .style(Style::new().justify(JustifyContent::END).flex_shrink(0.0))
                .child(control(theme, item, selected, pending, emit))
                .build(),
        );
    if let Some(marker) = marker {
        row = row.child(marker);
    }
    row.build()
}

fn control(
    theme: &ArtisanTheme,
    item: &Item,
    selected: bool,
    pending: Option<&str>,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let selector = item.selector();
    let disabled = pending.is_some();
    match item {
        Item::Skill(row) if row.origin == SkillOrigin::Discovered => {
            let id = row.id.clone();
            let discard_emit = emit.clone();
            let discard = Button::icon(
                format!("{selector}-discard"),
                *theme,
                AssetId::TABLER_TRASH,
                "Discard",
            )
            .variant(ButtonVariant::Ghost)
            .disabled(disabled)
            .on_activate(move |_| {
                discard_emit(ViewAction::Operate(Operation::Skill {
                    id: id.clone(),
                    action: SkillAction::Discard,
                }))
            })
            .build();
            let id = row.id.clone();
            let install = Button::icon_text(
                format!("{selector}-install"),
                *theme,
                AssetId::TABLER_DOWNLOAD,
                if selected { "Install ↵" } else { "Install" },
            )
            .variant(if selected {
                ButtonVariant::Primary
            } else {
                ButtonVariant::Outline
            })
            .disabled(disabled)
            .on_activate(move |_| {
                emit(ViewAction::Operate(Operation::Skill {
                    id: id.clone(),
                    action: SkillAction::Adopt,
                }))
            })
            .build();
            UiBox::new()
                .gap(4.0)
                .style(Style::new().align(AlignItems::CENTER))
                .child(discard)
                .child(install)
                .build()
        }
        Item::Skill(row) => {
            let id = row.id.clone();
            Switch::new(
                format!("{selector}-switch"),
                *theme,
                row.enabled,
                format!("Enable {}", row.name),
            )
            .disabled(disabled)
            .on_change(move |enabled| {
                emit(ViewAction::Operate(Operation::Skill {
                    id: id.clone(),
                    action: if enabled {
                        SkillAction::Enable
                    } else {
                        SkillAction::Disable
                    },
                }))
            })
            .build()
        }
        Item::Listing(row) => {
            let id = row.id.clone();
            let title = if pending == Some(row.id.as_str()) {
                "Installing…"
            } else if selected {
                "Install ↵"
            } else {
                "Install"
            };
            Button::icon_text(
                format!("{selector}-install"),
                *theme,
                AssetId::TABLER_DOWNLOAD,
                title,
            )
            .variant(if selected {
                ButtonVariant::Primary
            } else {
                ButtonVariant::Outline
            })
            .disabled(disabled)
            .on_activate(move |_| emit(ViewAction::Operate(Operation::Listing(id.clone()))))
            .build()
        }
        Item::Server(_) => Button::text(
            format!("{selector}-edit"),
            *theme,
            if selected { "Edit ↵" } else { "Edit" },
        )
        .variant(ButtonVariant::Outline)
        .on_activate(move |_| {
            emit(ViewAction::Navigate(Screen::Settings(
                SettingsPage::Extensions,
            )))
        })
        .build(),
    }
}

fn glass_tooltip(theme: &ArtisanTheme, text: &str) -> Element {
    use editor_ui::theme::Elevation;
    let radius = RadiusTokens::value(RadiusStep::X2l);
    let shadows = glass::glass_card_shadows(theme, Elevation::Overlay)
        .into_iter()
        .map(Into::into)
        .collect();
    let text = Text::new(text.to_owned())
        .text_style(TextStyle {
            families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
            size_px: TypographyTokens::size(TextStep::Xs),
            line_height_px: 16.0,
            weight: FontWeight::NORMAL,
            ..TextStyle::default()
        })
        .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
        .wrap(true)
        .build();
    UiBox::new()
        .layout("column")
        .radius(radius)
        .clip(true)
        .style(
            Style::new()
                .max_width(Length::Px(260.0))
                .shadows(shadows)
                .padding_insets(insets(12.0, 12.0, 8.0, 8.0)),
        )
        .child(glass::glass_layers(theme, GlassStrength::Strong, radius))
        .child(text)
        .build()
}

fn wordmark(theme: &ArtisanTheme, started: std::time::Instant, reduced_motion: bool) -> Element {
    use aqua_core::{DisplayItem, Rect, Role};
    use aqua_ui::element::Canvas;
    let height = 48.0 * 2.2515;
    let width = height * (136.675 / 225.15);
    let foreground = theme.colors.text;
    let muted = theme.colors.text_muted;
    let source: std::sync::Arc<str> = editor_assets::get(AssetId::ARTISAN_WORDMARK_STACKED)
        .source
        .into();
    Canvas::new()
        .width(width)
        .height(height)
        .on_paint(move |bounds| {
            let vector = |color| DisplayItem::GpuiSvg {
                bounds,
                source: source.clone(),
                tint: Some(color),
                opacity: 1.0,
            };
            if reduced_motion {
                return vec![vector(foreground.to_reference_paint())];
            }
            let progress = (started.elapsed().as_secs_f32() / 3.0).fract();
            let band_width = bounds.w * 0.6;
            let slice_width = band_width / 9.0;
            let leading = (bounds.w + band_width).mul_add(progress, -band_width);
            let mut items = vec![vector(muted.to_reference_paint())];
            for slice in 0..9 {
                let left = slice_width.mul_add(slice as f32, leading);
                let position = (slice as f32 + 0.5) / 9.0;
                let opacity = (position * std::f32::consts::TAU).cos().mul_add(-0.5, 0.5);
                items.push(DisplayItem::PushGpuiContentMask {
                    bounds: Rect {
                        x: bounds.x + left,
                        y: bounds.y,
                        w: slice_width,
                        h: bounds.h,
                    },
                });
                items.push(vector(foreground.fade(opacity).to_reference_paint()));
                items.push(DisplayItem::PopClip);
            }
            items
        })
        .build()
        .role(Role::ProgressIndicator)
        .aria_label("Loading")
}
