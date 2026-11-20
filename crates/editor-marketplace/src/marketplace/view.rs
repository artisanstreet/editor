use std::rc::Rc;

use aqua_text::overflow::OverflowMode;
use aqua_ui::{
    AlignItems, JustifyContent,
    collection::List,
    element::{Box as UiBox, Element},
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
use editor_core::{Core, Problem, Screen, SettingsPage};
use editor_ui::{
    kit::{
        Button, ButtonSize, ButtonTone, Finish, IconButton, IconButtonSize, IconSize, Label, Link,
        LinkTone, ProblemNotice, Row, RowItem, Space, Spaced, Surface, TextRole, Tooltip,
        surface::band::Band,
    },
    page,
    status_chip::{ChipTone, tone_color},
    switch::Switch,
    text::{ListKey, Mode},
    theme::{ArtisanTheme, Opacity},
};

const SEARCH_MAX_WIDTH: f32 = 720.0;
const TOOLTIP_MAX_WIDTH: f32 = 260.0;
const CONTROL_WIDTH: f32 = 148.0;
const CARD_PADDING: Space = Space::S8;
const ESTIMATED_ROW_HEIGHT: f32 = 256.0;

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
    RetrySearch,
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
        ViewAction::Retry => MarketplaceController::retry(core),
        ViewAction::RetrySearch => controller.retry_search(core),
        ViewAction::Navigate(screen) => controller.act(core, Action::Navigate(screen)),
        ViewAction::Operate(operation) => controller.operate(core, operation),
    }
    before != controller.view_snapshot(core)
}

#[derive(Clone, Copy)]
struct LoadingMark {
    started: std::time::Instant,
    reduced_motion: bool,
}

#[derive(Clone)]
enum Notices {
    NoHost,
    Host {
        attention: Option<String>,
        errors: Vec<(Problem, bool)>,
        loading: Option<LoadingMark>,
    },
}

impl Notices {
    fn of(controller: &MarketplaceController, host: Option<&editor_core::HostData>) -> Self {
        let Some(host) = host else {
            return Self::NoHost;
        };
        let page = &controller.page;
        let loading = host.catalog_load().loading && page.items.is_empty();
        Self::Host {
            attention: host
                .global_prompt
                .as_deref()
                .and_then(presentation::prompt_attention),
            errors: [
                host.catalog_load()
                    .error
                    .clone()
                    .map(|problem| (problem, true)),
                host.prompt_load()
                    .error
                    .clone()
                    .map(|problem| (problem, true)),
                host.mcp_servers().err().map(|error| {
                    (
                        Problem::caused_by("Couldn't read the MCP servers.", error),
                        false,
                    )
                }),
                page.notice.clone().map(|problem| (problem, false)),
            ]
            .into_iter()
            .flatten()
            .collect(),
            loading: controller
                .loading_started()
                .filter(|_| loading)
                .map(|started| LoadingMark {
                    started,
                    reduced_motion: controller.reduced_motion,
                }),
        }
    }
}

pub fn build(
    controller: &MarketplaceController,
    core: &Core,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let theme = *theme;
    let page = &controller.page;
    let notices = Notices::of(controller, core.active_host());
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
    let list = List::new()
        .items(rows)
        .key(|(index, _)| format!("marketplace-entry-{index}"))
        .row(move |(index, entry)| {
            let content = match entry {
                Entry::Hero => hero(&theme, &query, kind, counts, &search, prompt_hovered, &emit),
                Entry::Notices => notices_view(&theme, &notices, &emit),
                Entry::Found(summary) => found(&theme, summary, found_open, Rc::clone(&emit)),
                Entry::Group(title) => UiBox::new()
                    .padded(CARD_PADDING)
                    .child(
                        RowItem::new(
                            format!("marketplace-group-{index}"),
                            theme,
                            Row::Heading,
                            Surface::Card,
                            title.clone(),
                        )
                        .build(),
                    )
                    .build(),
                Entry::SearchFailed(error) => search_failed(&theme, error, &emit),
                Entry::Empty(text) => centered(muted(&theme, TextRole::Label, text.clone())),
                Entry::Item {
                    item,
                    first,
                    last,
                    selected,
                } => Band::of(*first, *last)
                    .build(
                        &theme,
                        *selected,
                        item_row(&theme, item, *selected, pending.as_ref(), Rc::clone(&emit)),
                    )
                    .debug_selector(item.selector()),
            };
            UiBox::new()
                .width(Length::Pct(100.0))
                .style(Style::new().justify(JustifyContent::CENTER))
                .child(
                    UiBox::new()
                        .layout("column")
                        .width(Length::Pct(100.0))
                        .style(Style::new().max_width(Length::Px(page::CONTENT_MAX_WIDTH)))
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
        .estimated_row_height(Length::Px(ESTIMATED_ROW_HEIGHT))
        .virtualize()
        .build()
        .with_key("marketplace-list")
        .debug_selector("route-marketplace");
    UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .padded(Space::S24)
        .child(list)
        .build()
}

fn search_failed(
    theme: &ArtisanTheme,
    problem: &Problem,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    let retry = Rc::clone(emit);
    UiBox::new()
        .padded(CARD_PADDING)
        .child(
            ProblemNotice::new("marketplace-search-error", *theme, problem)
                .action(
                    Button::text(
                        "marketplace-search-retry",
                        *theme,
                        ButtonTone::Neutral,
                        "Try again",
                    )
                    .on_activate(move |_| retry(ViewAction::RetrySearch))
                    .build(),
                )
                .build(),
        )
        .build()
}

fn muted(theme: &ArtisanTheme, role: TextRole, words: String) -> Element {
    role.text(theme, words)
        .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
        .wrap(true)
        .build()
}

fn caption(theme: &ArtisanTheme, words: String) -> Element {
    TextRole::Caption
        .text(theme, words)
        .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
        .wrap(false)
        .overflow(OverflowMode::EndEllipsis)
        .style(Style::new().min_width(Length::Pct(0.0)).flex_shrink(1.0))
        .build()
}

fn centered(child: Element) -> Element {
    UiBox::new()
        .padded(Space::S24)
        .style(Style::new().justify(JustifyContent::CENTER))
        .child(child)
        .build()
}

fn spacer(space: Space) -> Element {
    UiBox::new().height(space.px()).build()
}

fn hero(
    theme: &ArtisanTheme,
    query: &str,
    kind: Kind,
    counts: [usize; 2],
    search: &editor_ui::text::mount::EditorController,
    prompt_hovered: bool,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    let field = search_field(theme, query, search, emit);
    let tabs = kind_tabs(theme, kind, counts, emit);
    let prompt = prompt_link(theme, prompt_hovered, emit);
    let controls = UiBox::new()
        .padded(Space::S8)
        .style(
            Style::new()
                .justify(JustifyContent::SPACE_BETWEEN)
                .align(AlignItems::CENTER),
        )
        .child(tabs)
        .child(prompt)
        .build();
    UiBox::new()
        .layout("column")
        .padded(Space::S32)
        .spaced(Space::S20)
        .style(Style::new().align(AlignItems::CENTER))
        .child(
            TextRole::Heading
                .text(theme, "Marketplace")
                .wrap(false)
                .build(),
        )
        .child(
            UiBox::new()
                .layout("column")
                .width(Length::Pct(100.0))
                .spaced(Space::S12)
                .style(Style::new().max_width(Length::Px(SEARCH_MAX_WIDTH)))
                .child(field)
                .child(controls)
                .build(),
        )
        .build()
}

fn search_field(
    theme: &ArtisanTheme,
    query: &str,
    search: &editor_ui::text::mount::EditorController,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    let well = Surface::Composer;
    let slot = IconButtonSize::Md.edge();
    let lead = UiBox::new()
        .width(slot)
        .height(slot)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .justify(JustifyContent::CENTER)
                .flex_shrink(0.0),
        )
        .child(IconSize::Lg.glyph(
            AssetId::TABLER_SEARCH,
            theme.colors.text_muted.to_reference_paint(),
        ))
        .build();
    let editor = UiBox::new()
        .style(Style::new().flex_grow(1.0).min_width(Length::Pct(0.0)))
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
        .build();
    let clear = (!query.is_empty()).then(|| {
        let emit = Rc::clone(emit);
        IconButton::new(
            "marketplace-search-clear",
            *theme,
            AssetId::TABLER_X,
            "Clear the search",
            IconButtonSize::Md,
            Finish::Plain,
        )
        .muted()
        .within(well)
        .on_activate(move |_| emit(ViewAction::ClearSearch))
        .build()
    });
    let line = UiBox::new()
        .height(slot)
        .spaced(Space::S8)
        .style(Style::new().align(AlignItems::CENTER))
        .child(lead)
        .child(editor)
        .children(clear)
        .build();
    let focus = Rc::clone(emit);

    well.build(theme, vec![line])
        .style(Style::new().width(Length::Pct(100.0)))
        .cursor(aqua_core::CursorIcon::Text)
        .on_input(
            aqua_ui::input::InputKind::PointerDown,
            aqua_ui::input::EventPhase::Capture,
            move |_| {
                focus(ViewAction::FocusSearch);
                aqua_ui::input::InputResponse::default()
            },
        )
}

fn kind_tabs(
    theme: &ArtisanTheme,
    kind: Kind,
    counts: [usize; 2],
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    let tabs = [
        (Kind::All, "All".to_owned()),
        (Kind::Skills, format!("Skills {}", counts[0])),
        (Kind::Servers, format!("MCP servers {}", counts[1])),
    ]
    .into_iter()
    .map(|(tab, text)| {
        let emit = Rc::clone(emit);
        Link::new(
            format!("marketplace-kind-{tab:?}"),
            *theme,
            LinkTone::Crumb,
            text,
        )
        .current(kind == tab)
        .on_activate(move || emit(ViewAction::SetKind(tab)))
        .build()
        .role(aqua_core::Role::Tab)
        .aria_selected(kind == tab)
        .debug_selector(format!("marketplace-kind-{tab:?}"))
    });

    UiBox::new()
        .spaced(Space::S16)
        .children(tabs)
        .build()
        .role(aqua_core::Role::TabList)
}

fn prompt_link(
    theme: &ArtisanTheme,
    prompt_hovered: bool,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    let action = Rc::clone(emit);
    let hover = Rc::clone(emit);
    let prompt = Link::new(
        "marketplace-global-prompt",
        *theme,
        LinkTone::Crumb,
        "Global prompt",
    )
    .on_activate(move || {
        action(ViewAction::Navigate(Screen::Settings(
            SettingsPage::GlobalPrompt,
        )));
    })
    .build()
    .on_hover_change(move |hovered| hover(ViewAction::PromptHover(hovered)))
    .debug_selector("marketplace-global-prompt-open");
    let chevron = IconSize::Sm.glyph(
        AssetId::TABLER_CHEVRON_RIGHT,
        if prompt_hovered {
            theme.colors.text
        } else {
            theme.colors.text_muted
        }
        .to_reference_paint(),
    );

    UiBox::new()
        .spaced(Space::S2)
        .style(Style::new().align(AlignItems::CENTER))
        .child(prompt)
        .child(chevron)
        .build()
}

fn notices_view(theme: &ArtisanTheme, state: &Notices, emit: &Rc<dyn Fn(ViewAction)>) -> Element {
    let Notices::Host {
        attention,
        errors,
        loading,
    } = state
    else {
        return centered(muted(
            theme,
            TextRole::Label,
            "Pair a Forge to see its skills.".to_owned(),
        ));
    };
    let mut body: Vec<Element> = Vec::new();
    if let Some(attention) = attention {
        let emit = Rc::clone(emit);
        body.push(
            ProblemNotice::new(
                "marketplace-prompt-attention",
                *theme,
                &Problem::new(attention.clone()),
            )
            .action(
                Button::text(
                    "marketplace-prompt-choose",
                    *theme,
                    ButtonTone::Quiet,
                    "Open",
                )
                .on_activate(move |_| {
                    emit(ViewAction::Navigate(Screen::Settings(
                        SettingsPage::GlobalPrompt,
                    )));
                })
                .build(),
            )
            .build(),
        );
    }
    let first_retry = errors.iter().position(|(_, retryable)| *retryable);
    body.extend(errors.iter().enumerate().map(|(index, (problem, _))| {
        let notice = ProblemNotice::new(format!("marketplace-error-{index}"), *theme, problem);
        if first_retry == Some(index) {
            let emit = Rc::clone(emit);
            notice
                .action(
                    Button::text(
                        "marketplace-retry",
                        *theme,
                        ButtonTone::Neutral,
                        "Try again",
                    )
                    .on_activate(move |_| emit(ViewAction::Retry))
                    .build(),
                )
                .build()
        } else {
            notice.build()
        }
    }));
    if let Some(mark) = loading {
        body.push(centered(wordmark(theme, mark.started, mark.reduced_motion)));
    }
    if body.is_empty() {
        return UiBox::new().build();
    }
    UiBox::new()
        .layout("column")
        .spaced(Space::S8)
        .children(body)
        .child(spacer(Space::S8))
        .build()
}

fn found(theme: &ArtisanTheme, summary: &str, open: bool, emit: Rc<dyn Fn(ViewAction)>) -> Element {
    let content = UiBox::new()
        .spaced(Space::S12)
        .style(Style::new().align(AlignItems::CENTER))
        .child(IconSize::Md.glyph(
            AssetId::TABLER_SHIELD_LOCK,
            theme.colors.text_muted.to_reference_paint(),
        ))
        .child(
            UiBox::new()
                .style(Style::new().flex_grow(1.0).min_width(Length::Pct(0.0)))
                .child(
                    TextRole::Label
                        .text(theme, summary.to_owned())
                        .wrap(false)
                        .build(),
                )
                .build(),
        )
        .child(
            Button::text(
                "marketplace-found-review",
                *theme,
                ButtonTone::Quiet,
                if open { "Hide" } else { "Review" },
            )
            .on_activate(move |_| emit(ViewAction::ToggleFound))
            .build(),
        )
        .build();
    UiBox::new()
        .layout("column")
        .child(Band::Only.build(theme, false, content))
        .child(spacer(Space::S24))
        .build()
}

fn attention_line(theme: &ArtisanTheme, attention: String) -> Element {
    let color = tone_color(theme, ChipTone::Warning);
    UiBox::new()
        .spaced(Space::S4)
        .style(Style::new().align(AlignItems::CENTER))
        .child(IconSize::Sm.glyph(
            AssetId::TABLER_ALERT_TRIANGLE,
            color.at(Opacity::ToneIcon).to_reference_paint(),
        ))
        .child(
            TextRole::Caption
                .text(theme, attention)
                .gpui_source_color(color.to_reference_text_color())
                .wrap(false)
                .build(),
        )
        .build()
}

fn engine_marks(
    theme: &ArtisanTheme,
    item: &Item,
    marks: Vec<presentation::EngineMark>,
) -> Element {
    let marks = marks.into_iter().map(|mark| {
        let icon = match mark.engine {
            EngineId::Claude => AssetId::SVGL_CLAUDE_AI,
            EngineId::Codex => AssetId::SVGL_OPENAI,
            EngineId::OpenCode if theme.colors.mode.is_dark() => AssetId::BRANDS_OPENCODE,
            EngineId::OpenCode => AssetId::BRANDS_OPENCODE_LIGHT,
        };
        let selector = format!("{}-mark-{}", item.selector(), mark.engine.as_str());
        let tip = mark.label;
        let tooltip_theme = *theme;
        UiBox::new()
            .opacity(if mark.lit {
                1.0
            } else {
                Opacity::Unavailable.alpha()
            })
            .child(IconSize::Md.glyph(icon, theme.colors.text.to_reference_paint()))
            .build()
            .debug_selector(selector)
            .pointer_tooltip(move || tooltip(&tooltip_theme, &tip))
    });
    UiBox::new()
        .spaced(Space::S8)
        .style(Style::new().align(AlignItems::CENTER).flex_shrink(0.0))
        .children(marks)
        .build()
}

fn item_row(
    theme: &ArtisanTheme,
    item: &Item,
    selected: bool,
    pending: Option<&Operation>,
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
    let heading = UiBox::new()
        .spaced(Space::S8)
        .style(
            Style::new()
                .align(AlignItems::BASELINE)
                .min_width(Length::Pct(0.0)),
        )
        .child(name(theme, item.name()))
        .child(caption(theme, meta))
        .build();
    let description = (!description.is_empty()).then(|| caption(theme, description));
    let attention = attention.map(|attention| attention_line(theme, attention));
    let text = UiBox::new()
        .layout("column")
        .spaced(Space::S2)
        .style(Style::new().flex_grow(1.0).min_width(Length::Pct(0.0)))
        .child(heading)
        .children(description)
        .children(attention)
        .build();
    let tile = UiBox::new()
        .style(Style::new().flex_shrink(0.0))
        .child(Surface::Field.build(
            theme,
            vec![IconSize::Md.glyph(icon, theme.colors.text_muted.to_reference_paint())],
        ))
        .build();
    let engines = engine_marks(theme, item, marks);
    UiBox::new()
        .spaced(Space::S12)
        .style(Style::new().align(AlignItems::CENTER))
        .child(tile)
        .child(text)
        .child(engines)
        .child(
            UiBox::new()
                .width(CONTROL_WIDTH)
                .style(Style::new().justify(JustifyContent::END).flex_shrink(0.0))
                .child(control(theme, item, selected, pending, emit))
                .build(),
        )
        .build()
}

fn name(theme: &ArtisanTheme, name: &str) -> Element {
    let full = name.to_owned();
    let tooltip_theme = *theme;
    TextRole::LabelStrong
        .text(theme, name.to_owned())
        .wrap(false)
        .overflow(OverflowMode::EndEllipsis)
        .style(
            Style::new()
                .min_width(Length::Pct(0.0))
                .flex_shrink(1.0)
                .max_width(Length::Pct(60.0)),
        )
        .build()
        .pointer_tooltip(move || tooltip(&tooltip_theme, &full))
}

fn install(
    theme: &ArtisanTheme,
    key: String,
    title: &str,
    selected: bool,
    disabled: bool,
    activate: impl Fn() + 'static,
) -> Element {
    let tone = if selected {
        ButtonTone::Primary
    } else {
        ButtonTone::Neutral
    };
    Button::new(
        key,
        *theme,
        tone,
        ButtonSize::Md,
        Label::IconText(AssetId::TABLER_DOWNLOAD, title.to_owned()),
    )
    .disabled(disabled)
    .on_activate(move |_| activate())
    .build()
}

fn discovered(
    theme: &ArtisanTheme,
    row: &presentation::SkillRow,
    selector: &str,
    selected: bool,
    pending: Option<&Operation>,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    let disabled = pending.is_some();
    let id = row.id.clone();
    let discard_emit = Rc::clone(emit);
    let discard = IconButton::new(
        format!("{selector}-discard"),
        *theme,
        AssetId::TABLER_TRASH,
        "Discard",
        IconButtonSize::Md,
        Finish::Plain,
    )
    .muted()
    .disabled(disabled)
    .on_activate(move |_| {
        discard_emit(ViewAction::Operate(Operation::Skill {
            id: id.clone(),
            action: SkillAction::Discard,
        }));
    })
    .build();
    let id = row.id.clone();
    let adopt_emit = Rc::clone(emit);
    let title = match pending {
        Some(Operation::Skill {
            id: pending,
            action: SkillAction::Adopt,
        }) if pending == &row.id => "Installing…",
        Some(Operation::Skill {
            id: pending,
            action: SkillAction::Discard,
        }) if pending == &row.id => "Discarding…",
        _ if selected => "Install ↵",
        _ => "Install",
    };
    let adopt = install(
        theme,
        format!("{selector}-install"),
        title,
        selected,
        disabled,
        move || {
            adopt_emit(ViewAction::Operate(Operation::Skill {
                id: id.clone(),
                action: SkillAction::Adopt,
            }));
        },
    );
    UiBox::new()
        .spaced(Space::S4)
        .style(Style::new().align(AlignItems::CENTER))
        .child(discard)
        .child(adopt)
        .build()
}

fn control(
    theme: &ArtisanTheme,
    item: &Item,
    selected: bool,
    pending: Option<&Operation>,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let selector = item.selector();
    let disabled = pending.is_some();
    match item {
        Item::Skill(row) if row.origin == SkillOrigin::Discovered => {
            discovered(theme, row, &selector, selected, pending, &emit)
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
                }));
            })
            .build()
        }
        Item::Listing(row) => {
            let id = row.id.clone();
            let title = if pending == Some(&Operation::Listing(row.id.clone())) {
                "Installing…"
            } else if selected {
                "Install ↵"
            } else {
                "Install"
            };
            install(
                theme,
                format!("{selector}-install"),
                title,
                selected,
                disabled,
                move || emit(ViewAction::Operate(Operation::Listing(id.clone()))),
            )
        }
        Item::Server(_) => Button::text(
            format!("{selector}-edit"),
            *theme,
            ButtonTone::Quiet,
            if selected { "Edit ↵" } else { "Edit" },
        )
        .on_activate(move |_| {
            emit(ViewAction::Navigate(Screen::Settings(
                SettingsPage::Extensions,
            )));
        })
        .build(),
    }
}

fn tooltip(theme: &ArtisanTheme, text: &str) -> Element {
    Surface::Tooltip(Tooltip::Brief)
        .build(
            theme,
            vec![
                TextRole::Caption
                    .text(theme, text.to_owned())
                    .gpui_source_color(theme.colors.text_muted_glass.to_reference_text_color())
                    .wrap(true)
                    .build(),
            ],
        )
        .style(Style::new().max_width(Length::Px(TOOLTIP_MAX_WIDTH)))
}

pub const LOADING_KEY: &str = "marketplace-loading";

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
                source: std::sync::Arc::clone(&source),
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
            for slice in 0..9_u8 {
                let left = slice_width.mul_add(f32::from(slice), leading);
                let position = (f32::from(slice) + 0.5) / 9.0;
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
        .with_key(LOADING_KEY)
        .role(Role::ProgressIndicator)
        .aria_label("Loading")
}
