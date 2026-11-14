use std::rc::Rc;

use aqua_core::Role;
use aqua_text::overflow::OverflowMode;
use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Element, Scroll},
    style::{Length, Style},
};
use editor_assets::AssetId;
use editor_ui::{
    kit::{
        Button, ButtonTone, Density, Divider, Finish, HitArea, Hover, IconButton, IconButtonSize,
        IconSize, Inset, Key, KeyFace, KeySize, Lead, Link, LinkTone, Picker, Radius, Row, RowItem,
        Space, Spaced, Surface, TextRole, Tooltip, Trigger, TriggerSize, live,
    },
    theme::{ArtisanTheme, Opacity},
};

const MODEL_TRIGGER_WIDTH: f32 = 360.0;
const MENTION_THREAD_DETAIL_WIDTH: f32 = 180.0;
const MENTION_ROWS_SHOWN: f32 = 9.5;
const PROJECT_MENU_WIDTH: f32 = 320.0;
const PROJECT_TAB_WIDTH: f32 = 240.0;
const PROJECT_ROWS_SHOWN: usize = 8;
const MENU_VIEWPORT_MARGIN: f32 = 32.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposerAction {
    Editor(editor_ui::text::mount::EditorEvent),
    DroppedPaths {
        paths: Vec<std::path::PathBuf>,
        instance: editor_core::HostInstance,
        generation: u64,
    },
    DroppedThread(editor_core::DraggedThread),
    TooltipHover {
        instance: editor_core::HostInstance,
        generation: u64,
        anchor: String,
        content: Option<super::picker::TooltipContent>,
        hovered: bool,
    },
    Send,
    Stop,
    Continue,
    ToggleModel,
    ToggleProject,
    JumpEngine(artisan_api::EngineId),
    OpenEngineSettings(artisan_api::EngineId),
    ToggleModelGroup {
        engine: artisan_api::EngineId,
        name: String,
        open: bool,
    },
    SelectPane(Option<super::picker::Axis>),
    AxisPointer {
        axis: super::picker::Axis,
        index: usize,
    },
    AxisKey {
        axis: super::picker::Axis,
        key: super::picker::AxisKey,
    },
    ChooseAxis {
        axis: super::picker::Axis,
        index: usize,
    },
    ClosePicker,
    DismissMentions,
    ChooseModel {
        engine: artisan_api::EngineId,
        id: String,
    },
    ChooseProject(artisan_api::ProjectId),
    NewProject,
    SelectMention(usize),
    HoverMention(usize),
    RemoveImage(u64),
    OpenImage(u64),
    RetryImage(u64),
    RemoveFile(u64),
    RetryDraft,
    KeepDraft,
    LoadOtherDraft,
    RetryThreadSettings,
    CloseNote,
    ClearGoal,
    Unpark,
    DismissQuestionnaire,
    ActivateQuestionnaire(artisan_api::ItemId),
    ChooseQuestionOption {
        item: artisan_api::ItemId,
        question: String,
        step: usize,
        id: String,
        multi: bool,
    },
    ContinueQuestionnaire,
    RetryQuestionnaire,
    RetryQuestionPage,
    OpenQuestionSource(artisan_api::ContentRef),
}

pub type Dispatch = Rc<dyn Fn(ComposerAction)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Primary {
    Send,
    Stop,
    Continue,
}

pub fn primary(theme: ArtisanTheme, face: Primary, disabled: bool, dispatch: Dispatch) -> Element {
    let (icon, label, id, action) = match face {
        Primary::Stop => (
            AssetId::TABLER_PLAYER_STOP_FILLED,
            "Stop",
            "composer-stop",
            ComposerAction::Stop,
        ),
        Primary::Continue => (
            AssetId::TABLER_PLAYER_PLAY_FILLED,
            "Continue where it stopped",
            "composer-continue",
            ComposerAction::Continue,
        ),
        Primary::Send => (
            AssetId::TABLER_ARROW_UP,
            "Send message",
            "composer-send",
            ComposerAction::Send,
        ),
    };
    Key::new(id, theme, label, KeySize::Md, KeyFace::Glyph(icon))
        .primary()
        .disabled(disabled)
        .on_activate(move |_| dispatch(action.clone()))
        .build()
        .debug_selector(id)
}

pub fn frame(theme: ArtisanTheme, nested: bool, content: Element) -> Element {
    editor_ui::glass::ComposerFrame::new(theme, content)
        .nested(nested)
        .build()
}

pub fn model_trigger(
    theme: ArtisanTheme,
    config: Option<&artisan_api::EngineConfig>,
    model: Option<&artisan_api::Model>,
    dispatch: Dispatch,
) -> Element {
    let migrated = model.and_then(|model| config.and_then(|config| model.migrate_config(config)));
    let config = migrated.as_ref().or(config);
    let name = config.map_or_else(
        || "No model".to_owned(),
        |config| {
            model.map_or_else(
                || config.model.clone().unwrap_or_else(|| "No model".into()),
                |model| model.name.clone(),
            )
        },
    );
    let details = config.map(|config| model_details(config, model)).unwrap_or_default();
    let mut trigger = Trigger::new("composer-model", theme, TriggerSize::Sm, name)
        .on_activate(move || dispatch(ComposerAction::ToggleModel));
    if !details.is_empty() {
        trigger = trigger.detail(details.join(" "));
    }
    if let Some(config) = config {
        trigger = trigger.lead(Lead::Glyph(engine_icon(&theme, config.engine)));
    }
    UiBox::new()
        .style(
            Style::new()
                .min_width(Length::Pct(0.0))
                .max_width(Length::Px(MODEL_TRIGGER_WIDTH)),
        )
        .child(trigger.build())
        .build()
}

fn model_details(
    config: &artisan_api::EngineConfig,
    model: Option<&artisan_api::Model>,
) -> Vec<String> {
    let context = model
        .and_then(|model| config.context_window_override().or(model.context_window))
        .map(|context| match context >= 1_000_000 && context.is_multiple_of(1_000_000) {
            true => format!("{}M", context / 1_000_000),
            false => format!("{}K", context / 1_000),
        });
    let compaction = config
        .compaction_window_override()
        .map(|compaction| format!("{}K compact", compaction / 1_000));
    let effort = config
        .effort
        .as_ref()
        .filter(|_| config.engine != artisan_api::EngineId::OpenCode)
        .map(|effort| {
            artisan_api::ThinkingLevel::from_native(effort)
                .map_or_else(|| effort.clone(), |level| level.label().into())
        });
    let variant = model
        .and_then(|model| {
            model
                .variant_for(config.variant.as_deref())
                .and_then(|id| model.variants.iter().find(|variant| variant.id == id))
        })
        .map(|variant| {
            artisan_api::ThinkingLevel::from_native(&variant.id)
                .map_or_else(|| variant.label.clone(), |level| level.label().into())
        });
    let speed = model
        .and_then(|model| {
            config.speed.as_ref().and_then(|id| {
                model
                    .speeds
                    .iter()
                    .find(|speed| &speed.id == id && !speed.default)
            })
        })
        .map(|speed| {
            artisan_api::SpeedLevel::from_native(&speed.id)
                .map_or_else(|| speed.label.clone(), |level| level.label().into())
        });
    [context, compaction, effort, variant, speed]
        .into_iter()
        .flatten()
        .collect()
}

pub fn controls(primary: Element) -> Element {
    UiBox::new()
        .height(KeySize::Md.edge())
        .spaced(Space::S8)
        .style(
            Style::new()
                .flex_shrink(0.0)
                .align(AlignItems::CENTER)
                .justify(JustifyContent::END),
        )
        .child(primary)
        .build()
        .with_key("composer-controls")
}







pub fn mention_menu(
    theme: ArtisanTheme,
    mentions: &super::mentions::Mentions,
    viewport: aqua_core::Size,
    width: f32,
    dispatch: Dispatch,
) -> Option<Element> {
    use super::mentions::Kind;
    use editor_ui::popover::PopoverPanel;
    if mentions.rows.is_empty() {
        return None;
    }
    let surface = Surface::Menu;
    let muted = theme.colors.text_muted;
    let mut entries = UiBox::new().layout("column");
    let mut previous = None;
    for (index, catalog_index) in mentions.rows.iter().copied().enumerate() {
        let Some(row) = mentions.catalog.get(catalog_index) else {
            continue;
        };
        if previous != Some(row.kind) {
            if previous.is_some() {
                entries = entries.child(Divider::Rule.build(&theme, surface.padding_px()));
            }
            let heading = match row.kind {
                Kind::Command => "Commands",
                Kind::Skill => "Skills",
                Kind::Connector => "Connectors",
                Kind::Thread => "Threads",
            };
            entries = entries.child(
                RowItem::new(
                    format!("composer-mention-heading-{index}"),
                    theme,
                    Row::Heading,
                    surface,
                    heading,
                )
                .build(),
            );
            previous = Some(row.kind);
        }
        let icon = match row.kind {
            Kind::Command => AssetId::TABLER_TERMINAL_2,
            Kind::Skill => AssetId::TABLER_FILE_SPARK,
            Kind::Connector => AssetId::TABLER_PLUG_CONNECTED,
            Kind::Thread => AssetId::TABLER_MESSAGE_CIRCLE,
        };
        let action = Rc::clone(&dispatch);
        let hover = Rc::clone(&dispatch);
        let description_box = match row.kind {
            Kind::Thread => Style::new()
                .flex_shrink(0.0)
                .max_width(Length::Px(MENTION_THREAD_DETAIL_WIDTH)),
            _ => Style::new().flex_grow(1.0).min_width(Length::Pct(0.0)),
        };
        let description = UiBox::new()
            .style(description_box)
            .child(
                TextRole::Label
                    .text(&theme, row.description.clone())
                    .gpui_source_color(muted.to_reference_text_color())
                    .overflow(OverflowMode::EndEllipsis)
                    .style(Style::new().min_width(Length::Pct(0.0)).text_wrap(false))
                    .build(),
            )
            .build();
        entries = entries.child(
            RowItem::new(
                format!("composer-mention-row-{index}"),
                theme,
                Row::MenuItem,
                surface,
                row.title.clone(),
            )
            .leading(IconSize::Md.glyph(icon, muted.to_reference_paint()))
            .trailing(description)
            .selected(index == mentions.highlighted)
            .on_activate(move || action(ComposerAction::SelectMention(index)))
            .on_hover(move |hovered| {
                if hovered {
                    hover(ComposerAction::HoverMention(index));
                }
            })
            .build(),
        );
    }
    let cap = Row::MenuItem
        .height()
        .unwrap_or_default()
        .mul_add(MENTION_ROWS_SHOWN, Row::Heading.height().unwrap_or_default());
    let list = Scroll::new()
        .layout("column")
        .style(Style::new().max_height(Length::Px(cap)))
        .child(entries.build())
        .build();
    let panel = PopoverPanel::new("composer-mentions-panel", theme, list, viewport.w, viewport.h).build();
    Some(
        UiBox::new()
            .width(width)
            .child(panel)
            .build()
            .with_key("composer-mention-menu"),
    )
}

pub fn questionnaire(
    theme: ArtisanTheme,
    panel: &super::questionnaire::QuestionnairePanel,
    question: Option<&artisan_api::Question>,
    stored_options: Option<Element>,
    load_error: Option<&str>,
    source_view: Option<Element>,
    dispatch: Dispatch,
) -> Option<Element> {
    let active = panel.active()?;
    let muted = theme.colors.text_muted;
    let surface = Surface::Composer;
    let Some(question) = question else {
        let mut loading = UiBox::new().layout("column").spaced(Space::S8).child(
            TextRole::Label
                .text(&theme, load_error.unwrap_or("Loading question…").to_owned())
                .wrap(true)
                .build(),
        );
        if load_error.is_some() {
            let action = Rc::clone(&dispatch);
            loading = loading.child(
                Button::text("question-page-retry", theme, ButtonTone::Neutral, "Retry")
                    .within(surface)
                    .on_activate(move |_| action(ComposerAction::RetryQuestionPage))
                    .build(),
            );
        }
        return Some(loading.build());
    };
    let sending = panel.sending.is_some();
    let action = Rc::clone(&dispatch);
    let dismiss = IconButton::new(
        "composer-question-dismiss",
        theme,
        AssetId::TABLER_X,
        "Dismiss questionnaire",
        IconButtonSize::Sm,
        Finish::Plain,
    )
    .muted()
    .within(surface)
    .disabled(sending)
    .on_activate(move |_| action(ComposerAction::DismissQuestionnaire))
    .build();
    let header = UiBox::new()
        .spaced(Space::S8)
        .style(Style::new().align(AlignItems::CENTER))
        .child(IconSize::Sm.glyph(AssetId::TABLER_MESSAGE_CIRCLE, muted.to_reference_paint()))
        .child(
            UiBox::new()
                .style(Style::new().flex_grow(1.0).min_width(Length::Pct(0.0)))
                .child(
                    TextRole::Caption
                        .text(
                            &theme,
                            question.header.clone().unwrap_or_else(|| "Question".into()),
                        )
                        .gpui_source_color(muted.to_reference_text_color())
                        .wrap(false)
                        .build(),
                )
                .build(),
        )
        .child(dismiss)
        .build();
    let text = match &question.text {
        artisan_api::TextContent::Inline(text) => {
            TextRole::Label.text(&theme, text.clone()).wrap(true).build()
        }
        artisan_api::TextContent::Stored(source) => {
            let source = source.clone();
            let action = Rc::clone(&dispatch);
            Button::text("question-source", theme, ButtonTone::Quiet, "Read full question")
                .within(surface)
                .on_activate(move |_| action(ComposerAction::OpenQuestionSource(source.clone())))
                .build()
        }
    };
    let mut card = UiBox::new()
        .layout("column")
        .spaced(Space::S8)
        .child(header)
        .child(text);
    let mut rows = UiBox::new().layout("column").spaced(Space::S2);
    if let Some(options) = stored_options {
        rows = rows.child(options);
    }
    if let artisan_api::Collection::Inline(options) = &question.options {
        for (index, option) in options.iter().enumerate() {
            let selected = panel.ticked.contains(&option.id);
            let marker = match question.multi_select && selected {
                true => IconSize::Md.glyph(AssetId::TABLER_CHECK, theme.colors.text.to_reference_paint()),
                false => TextRole::Label
                    .text(&theme, (index + 1).to_string())
                    .gpui_source_color(muted.to_reference_text_color())
                    .wrap(false)
                    .build(),
            };
            let mut text = UiBox::new()
                .layout("column")
                .style(Style::new().flex_grow(1.0).min_width(Length::Pct(0.0)))
                .child(TextRole::Label.text(&theme, option.label.clone()).wrap(true).build());
            if let Some(description) = &option.description {
                let value = match description {
                    artisan_api::TextContent::Inline(text) => text.clone(),
                    artisan_api::TextContent::Stored(_) => {
                        "Full description available below".into()
                    }
                };
                text = text.child(
                    TextRole::Caption
                        .text(&theme, value)
                        .gpui_source_color(muted.to_reference_text_color())
                        .wrap(true)
                        .build(),
                );
            }
            let item = active.item.clone();
            let question_id = question.id.clone();
            let id = option.id.clone();
            let multi = question.multi_select;
            let step = panel.step;
            let action = Rc::clone(&dispatch);
            let body = UiBox::new()
                .padded(Space::S8)
                .spaced(Space::S8)
                .style(Style::new().align(AlignItems::FLEX_START))
                .child(
                    UiBox::new()
                        .width(IconSize::Md.px())
                        .style(Style::new().flex_shrink(0.0).align(AlignItems::CENTER))
                        .child(marker)
                        .build(),
                )
                .child(text.build())
                .build();
            let mut row = HitArea::new(
                format!("composer-question-option-{index}"),
                Role::Button,
                option.label.clone(),
            )
            .child(live::layer(
                Hover::Neutral,
                &theme,
                Radius::Field.px(),
                selected && !sending,
            ))
            .child(body);
            if !sending {
                row = row.on_activate(move || {
                    action(ComposerAction::ChooseQuestionOption {
                        item: item.clone(),
                        question: question_id.clone(),
                        step,
                        id: id.clone(),
                        multi,
                    })
                });
            }
            let row = choice(row.build(), selected, sending);
            rows = rows.child(row);
            if let Some(artisan_api::TextContent::Stored(source)) = &option.description {
                let source = source.clone();
                let action = Rc::clone(&dispatch);
                rows = rows.child(
                    Button::text(
                        format!("question-description-{index}"),
                        theme,
                        ButtonTone::Quiet,
                        "Read description",
                    )
                    .within(surface)
                    .on_activate(move |_| {
                        action(ComposerAction::OpenQuestionSource(source.clone()))
                    })
                    .build(),
                );
            }
        }
    }
    if let Some(source) = source_view {
        rows = rows.child(source);
    }
    if question.multi_select && !panel.ticked.is_empty() && !sending {
        let text = if (panel.step as u64) + 1 < active.questions.len() {
            "Next"
        } else {
            "Send answers"
        };
        let action = Rc::clone(&dispatch);
        rows = rows.child(
            Button::text("composer-question-continue", theme, ButtonTone::Primary, text)
                .within(surface)
                .on_activate(move |_| action(ComposerAction::ContinueQuestionnaire))
                .build(),
        );
    }
    card = card.child(rows.build());
    if sending {
        card = card.child(
            TextRole::Caption
                .text(&theme, "Sending your answer…")
                .gpui_source_color(muted.to_reference_text_color())
                .build(),
        );
    } else if let Some(error) = &panel.error {
        let action = Rc::clone(&dispatch);
        card = card
            .child(
                TextRole::Label
                    .text(&theme, error.clone())
                    .gpui_source_color(theme.colors.destructive_text.to_reference_text_color())
                    .wrap(true)
                    .build(),
            )
            .child(
                Button::text("composer-question-retry", theme, ButtonTone::Neutral, "Retry answer")
                    .within(surface)
                    .on_activate(move |_| action(ComposerAction::RetryQuestionnaire))
                    .build(),
            );
    }
    let mut stack = UiBox::new().layout("column").spaced(Space::S2);
    for (position, other) in panel
        .visible()
        .filter(|other| other.item != active.item)
        .enumerate()
    {
        let title = other
            .questions
            .inline()
            .and_then(|questions| questions.first())
            .map_or_else(
                || "Questionnaire".to_owned(),
                |question| match &question.text {
                    artisan_api::TextContent::Inline(text) => text.clone(),
                    artisan_api::TextContent::Stored(_) => question
                        .header
                        .clone()
                        .unwrap_or_else(|| "Questionnaire".into()),
                },
            );
        let item = other.item.clone();
        let action = Rc::clone(&dispatch);
        let mut row = RowItem::new(
            format!("composer-questionnaire-{position}"),
            theme,
            Row::ListItem(Density::Dense),
            surface,
            title,
        )
        .leading(IconSize::Md.glyph(AssetId::TABLER_CHEVRON_RIGHT, muted.to_reference_paint()))
        .disabled(sending)
        .on_activate(move || action(ComposerAction::ActivateQuestionnaire(item.clone())));
        if other.questions.len() > 1 {
            row = row.trailing(
                TextRole::Caption
                    .text(&theme, format!("{} questions", other.questions.len()))
                    .gpui_source_color(muted.to_reference_text_color())
                    .wrap(false)
                    .build(),
            );
        }
        stack = stack.child(row.build());
    }
    Some(
        UiBox::new()
            .layout("column")
            .width(Length::Pct(100.0))
            .spaced(Space::S8)
            .child(card.build())
            .child(stack.build())
            .build()
            .with_key("composer-questionnaire"),
    )
}

fn choice(row: Element, selected: bool, sending: bool) -> Element {
    let row = row
        .style(Style::new().height(Length::Auto))
        .style(Radius::Field.style())
        .aria_selected(selected)
        .disabled(sending)
        .focusable(!sending);
    match sending {
        true => row,
        false => live::track(row),
    }
}

pub struct CardParts {
    pub prompt: Element,
    pub primary: Element,
    pub questionnaires: Option<Element>,
    pub attachments: Option<Element>,
    pub mention_overlay: Option<Element>,
    pub thread_error: Option<Element>,
    pub draft_error: Option<Element>,
}

pub fn composer_card(theme: ArtisanTheme, nested: bool, parts: CardParts) -> Element {
    let mut card = UiBox::new().layout("column").width(Length::Pct(100.0));
    if let Some(error) = parts.thread_error {
        card = card.child(error);
    }
    if let Some(error) = parts.draft_error {
        card = card.child(error);
    }
    if let Some(questionnaires) = parts.questionnaires {
        card = card.child(questionnaires);
    }
    if let Some(attachments) = parts.attachments {
        card = card.child(attachments);
    }
    card = card.child(
        UiBox::new()
            .width(Length::Pct(100.0))
            .spaced(Space::S8)
            .style(Style::new().align(AlignItems::END))
            .child(
                UiBox::new()
                    .layout("column")
                    .style(Style::new().flex_grow(1.0).min_width(Length::Pct(0.0)))
                    .child(parts.prompt)
                    .build(),
            )
            .child(controls(parts.primary))
            .build(),
    );
    if let Some(mentions) = parts.mention_overlay {
        card = card.child(mentions);
    }
    frame(theme, nested, card.build()).with_key("composer")
}

pub fn draft_error(
    theme: ArtisanTheme,
    error: &str,
    conflict: bool,
    dispatch: Dispatch,
) -> Element {
    let surface = Surface::Composer;
    let choice = |key: &'static str, label: &'static str, action: ComposerAction| {
        let dispatch = Rc::clone(&dispatch);
        Button::text(key, theme, ButtonTone::Neutral, label)
            .within(surface)
            .on_activate(move |_| dispatch(action.clone()))
            .build()
    };
    let actions = match conflict {
        true => UiBox::new()
            .spaced(Space::S8)
            .child(choice("draft-keep-local", "Keep my draft", ComposerAction::KeepDraft))
            .child(choice(
                "draft-use-remote",
                "Load other draft",
                ComposerAction::LoadOtherDraft,
            ))
            .build(),
        false => choice("draft-retry", "Retry", ComposerAction::RetryDraft),
    };
    UiBox::new()
        .layout("column")
        .padded(Space::S8)
        .spaced(Space::S8)
        .child(
            TextRole::Label
                .text(&theme, error.to_owned())
                .gpui_source_color(theme.colors.destructive_text.to_reference_text_color())
                .wrap(true)
                .build(),
        )
        .child(actions)
        .build()
}

pub fn goal_label(
    theme: ArtisanTheme,
    frame: &super::goal_strip::GoalStripFrame,
    ticker: Element,
    dispatch: Dispatch,
) -> Element {
    use super::goal_strip::ComposerGoalState as State;
    use aqua_core::Transform2D;
    let goal = &frame.goal;
    let safety = matches!(
        goal.state,
        State::SafetyHolding | State::SafetyRerouted | State::SafetyRefused
    );
    let note = matches!(goal.state, State::Note | State::HeadsUp);
    let (icon, tone) = match goal.state {
        State::Blocked => (
            AssetId::TABLER_TARGET_ARROW,
            theme.colors.banner_warning.text,
        ),
        State::SafetyHolding | State::SafetyRerouted => (
            AssetId::TABLER_SHIELD_LOCK,
            theme.colors.banner_warning.text,
        ),
        State::SafetyRefused => (AssetId::TABLER_SHIELD_LOCK, theme.colors.banner_error.text),
        State::Parked => (AssetId::TABLER_RUN_SPRINT, theme.colors.banner_warning.text),
        State::Note => (
            AssetId::TABLER_BELL_EXCLAMATION,
            theme.colors.banner_info.text,
        ),
        State::HeadsUp => (
            AssetId::TABLER_BELL_EXCLAMATION,
            theme.colors.banner_warning.text,
        ),
        _ => (AssetId::TABLER_TARGET_ARROW, theme.colors.text_muted),
    };
    let muted = theme.colors.text_muted.to_reference_text_color();
    let mut style = Style::new()
        .align(AlignItems::CENTER)
        .opacity(frame.label_opacity)
        .transform(Transform2D::translate(0.0, frame.label_offset));
    let blur = (frame.label_offset.abs() / 2.0).min(2.0);
    if blur > 0.0 {
        style = style.content_blur(blur);
    }
    let mut row = UiBox::new()
        .height(Length::Pct(100.0))
        .padded(Space::S6)
        .spaced(Space::S6)
        .style(style)
        .child(IconSize::Md.glyph(icon, tone.to_reference_paint()));
    if note {
        row = row.child(
            UiBox::new()
                .style(Style::new().flex_grow(1.0).min_width(Length::Pct(0.0)))
                .child(ticker)
                .build(),
        );
    } else {
        let headline = match safety {
            true => tone,
            false => theme.colors.text,
        };
        row = row.child(
            UiBox::new()
                .style(Style::new().min_width(Length::Pct(0.0)))
                .child(
                    TextRole::Label
                        .text(&theme, goal.objective.clone())
                        .gpui_source_color(headline.to_reference_text_color())
                        .wrap(false)
                        .build(),
                )
                .build(),
        );
        if matches!(
            goal.state,
            State::Active | State::SafetyHolding | State::SafetyRerouted | State::SafetyRefused
        ) && let Some(detail) = &goal.detail
        {
            row = row.child(
                TextRole::Label
                    .text(&theme, detail.clone())
                    .gpui_source_color(muted)
                    .wrap(false)
                    .build(),
            );
        }
        let status = match goal.state {
            State::Paused => Some(("Paused", theme.colors.banner_info.text)),
            State::Blocked => Some(("Blocked", theme.colors.banner_warning.text)),
            State::UsageLimited => Some(("Usage limit reached", theme.colors.banner_warning.text)),
            State::BudgetLimited => Some(("Budget spent", theme.colors.banner_warning.text)),
            _ => None,
        };
        if let Some((text, tone)) = status {
            row = row.child(
                TextRole::Caption
                    .text(&theme, text)
                    .gpui_source_color(tone.to_reference_text_color())
                    .wrap(false)
                    .build(),
            );
        }
        row = row.child(UiBox::new().style(Style::new().flex_grow(1.0)).build());
    }
    if !safety {
        let (id, label, action) = if note {
            ("composer-note-close", "Close note", ComposerAction::CloseNote)
        } else if goal.state == State::Parked {
            ("composer-park-cancel", "Cancel the parked run", ComposerAction::Unpark)
        } else {
            ("composer-goal-clear", "Clear goal", ComposerAction::ClearGoal)
        };
        row = row.child(
            IconButton::new(
                id,
                theme,
                AssetId::TABLER_X,
                label,
                IconButtonSize::Sm,
                Finish::Plain,
            )
            .muted()
            .on_activate(move |_| dispatch(action.clone()))
            .build(),
        );
    }
    row.build()
}

pub struct ComposerOwner {
    pub controller: Rc<std::cell::RefCell<super::controller::Composer>>,
    pub editor: editor_ui::text::mount::EditorController,
    dispatch: Dispatch,
    answers: editor_core::delivery::Delivery<AnswerCompletion>,
    reduced_motion: bool,
    model_popup: Option<editor_ui::popover::AnimatedPickerHost>,
    mention_popup: Option<editor_ui::popover::AnimatedPickerHost>,
    project_popup: Option<editor_ui::popover::AnimatedPickerHost>,
    engine_light: std::cell::RefCell<super::engine_light::EngineLight>,
    engine_light_image: std::cell::RefCell<super::engine_light::EngineLightImage>,
    tab_geometry: Rc<std::cell::RefCell<editor_ui::tab_card::GeometryCache>>,
    strip_moving: std::cell::Cell<bool>,
    note_ticker: std::cell::RefCell<editor_ui::marquee::Marquee>,
    model_menu_serial: std::cell::Cell<u64>,
    model_scroll: std::cell::RefCell<super::model_list::ModelScroll>,
    model_tab: std::cell::RefCell<super::sliding_tab::SlidingTab>,
    last_visible: std::cell::RefCell<ComposerVisible>,
    tooltip: std::cell::RefCell<super::picker::PickerTooltip>,
}

struct AnswerCompletion {
    host: editor_core::HostId,
    instance: editor_core::HostInstance,
    generation: u64,
    item: artisan_api::ItemId,
    result: Result<(), String>,
}

#[derive(Clone, Debug)]
pub enum ComposerEffect {
    EditorChanged,
    FocusEditor,
    NewProject,
    OpenImage(u64),
    OpenQuestionSource(artisan_api::ContentRef),
    RetryQuestionPage,
    CloseNote,
    RetryThreadSettings,
}

pub struct ComposerDrain {
    pub events: Vec<super::persistence::SessionEvent>,
    pub effects: Vec<ComposerEffect>,
    pub changed: bool,
}

pub struct PickerTooltipPresentation {
    pub anchor: String,
    pub side: aqua_ui::overlay::Side,
    pub snap_to_window: bool,
    pub content: Element,
}

pub struct PickerTooltipSurface {
    pub anchor: String,
    pub placement: editor_ui::hover_tooltip::TooltipPlacement,
    pub surface: Rc<dyn Fn(f32, bool) -> Element>,
}

fn picker_tooltip_surface(
    theme: &ArtisanTheme,
    tooltip: &super::picker::TooltipContent,
    width: f32,
    fixed: bool,
) -> Element {
    use aqua_text::StyledSpan;
    let style = TextRole::Caption.style(theme);
    let mut spans = Vec::new();
    if let Some(advisory) = tooltip.advisory.as_ref().filter(|text| !text.is_empty()) {
        let color = if advisory == "Additional info:" {
            theme.colors.banner_info.text
        } else {
            theme.colors.destructive_text
        }
        .to_reference_text_color();
        spans.push(StyledSpan::new(advisory.clone(), style.clone()).gpui_source_color([
            color.hue_degrees,
            color.saturation,
            color.lightness,
            color.alpha,
        ]));
        if !tooltip.text.is_empty() {
            spans.push(StyledSpan::new(" ", style.clone()));
        }
    }
    if !tooltip.text.is_empty() {
        spans.push(StyledSpan::new(tooltip.text.clone(), style));
    }
    let mut layout = Style::new().max_width(Length::Px(width));
    if fixed {
        layout = layout.width(Length::Px(width));
    }
    Surface::Tooltip(Tooltip::Brief)
        .build(
            theme,
            vec![
                TextRole::Caption
                    .text(theme, "")
                    .gpui_source_color(theme.colors.text_muted_glass.to_reference_text_color())
                    .spans(spans)
                    .wrap(true)
                    .build(),
            ],
        )
        .style(layout)
        .with_key("composer-picker-tooltip")
}

#[derive(PartialEq)]
struct ComposerRest {
    picker: super::picker::PickerState,
    mention: (Option<super::mentions::Trigger>, Vec<usize>, usize),
    question: (
        Option<artisan_api::ItemId>,
        usize,
        Vec<String>,
        Option<artisan_api::ItemId>,
        Option<String>,
        Option<String>,
    ),
    strip: super::goal_strip::StripState,
    config: Option<artisan_api::EngineConfig>,
    project: Option<artisan_api::ProjectId>,
    attachments: Option<(
        Vec<(
            u64,
            super::attachment::State,
            Option<String>,
            bool,
            bool,
            Option<std::time::Instant>,
        )>,
        Option<(u64, std::time::Instant)>,
        Option<String>,
    )>,
    files: Vec<(u64, std::path::PathBuf, bool)>,
    file_shake: Option<(u64, std::time::Instant)>,
    notice: Option<String>,
    session: Option<(bool, bool, Option<String>, bool)>,
    cancelling: bool,
    continuing: bool,
}

#[derive(PartialEq)]
struct ComposerVisible {
    rest: ComposerRest,
    editor: EditorVisible,
}

impl ComposerVisible {
    fn same_chrome(&self, other: &Self) -> bool {
        self.rest == other.rest && self.editor.same_chrome(&other.editor)
    }
}

#[derive(Clone, PartialEq)]
struct EditorVisible {
    text: String,
    selection: std::ops::Range<usize>,
    reversed: bool,
    marked: Option<std::ops::Range<usize>>,
    disabled: bool,
    limit_notice: bool,
    suggestion: Option<String>,
    tokens: Vec<editor_ui::text::editor::StyledToken>,
}

impl EditorVisible {
    fn capture(editor: &editor_ui::text::editor::Editor) -> Self {
        Self {
            text: editor.text().to_owned(),
            selection: editor.selection(),
            reversed: editor.selection_reversed(),
            marked: editor.marked_range(),
            disabled: editor.is_disabled(),
            limit_notice: editor.limit_notice(),
            suggestion: editor.shown_suggestion().map(str::to_owned),
            tokens: editor.styled_tokens(),
        }
    }

    fn same_chrome(&self, other: &Self) -> bool {
        self.text.is_empty() == other.text.is_empty()
            && self.text.trim().is_empty() == other.text.trim().is_empty()
            && self.disabled == other.disabled
            && self.limit_notice == other.limit_notice
            && self.suggestion == other.suggestion
            && self.tokens == other.tokens
    }
}

impl ComposerVisible {
    fn capture(composer: &super::controller::Composer) -> Self {
        let editor = composer.editor.borrow();
        Self {
            editor: EditorVisible::capture(&editor),
            rest: ComposerRest::capture(composer),
        }
    }
}

impl ComposerRest {
    fn capture(composer: &super::controller::Composer) -> Self {
        let mut picker = composer.picker.clone();
        picker.axis_cursor = picker.axis_cursor.filter(|cursor| {
            matches!(cursor, super::picker::AxisCursor::Keys(_))
        });
        Self {
            picker,
            mention: (
                composer.mentions.current.clone(),
                composer.mentions.rows.clone(),
                composer.mentions.highlighted,
            ),
            question: (
                composer
                    .questionnaire
                    .active()
                    .map(|entry| entry.item.clone()),
                composer.questionnaire.step,
                composer.questionnaire.ticked.clone(),
                composer.questionnaire.sending.clone(),
                composer.questionnaire.error.clone(),
                composer.questionnaire.aside_text().map(str::to_owned),
            ),
            strip: composer.strip.clone(),
            config: composer.config.clone(),
            project: composer.project.clone(),
            attachments: composer.attachments.as_ref().map(|attachments| {
                (
                    attachments
                        .items
                        .iter()
                        .map(|item| {
                            (
                                item.key,
                                item.state.clone(),
                                item.preview_error.clone(),
                                item.shown,
                                item.preview.is_some(),
                                item.revealed,
                            )
                        })
                        .collect(),
                    attachments.shake,
                    attachments.notice.clone(),
                )
            }),
            files: composer
                .files
                .iter()
                .map(|file| (file.key, file.path.clone(), file.folder))
                .collect(),
            file_shake: composer.file_shake,
            notice: composer.notice.clone(),
            session: composer.session.as_ref().map(|session| {
                session.read(|state| {
                    (
                        state.loaded,
                        state.conflict,
                        state.error.clone(),
                        state.sending(),
                    )
                })
            }),
            cancelling: composer.cancelling,
            continuing: composer.continuing,
        }
    }
}

impl ComposerOwner {
    pub fn new(
        core: &editor_core::Core,
        sessions: &mut super::persistence::DraftSessions,
        target: super::controller::ComposerTarget,
        dispatch: Dispatch,
    ) -> Self {
        let controller = Rc::new(std::cell::RefCell::new(super::controller::Composer::new(
            core, sessions, target,
        )));
        let send = Rc::clone(&dispatch);
        let editor = controller
            .borrow()
            .mount_editor(move |event| send(ComposerAction::Editor(event)));
        let last_visible = ComposerVisible::capture(&controller.borrow());
        Self {
            controller,
            editor,
            dispatch,
            answers: editor_core::delivery::Delivery::new(std::num::NonZeroUsize::new(16).unwrap()),
            reduced_motion: false,
            model_popup: None,
            mention_popup: None,
            project_popup: None,
            engine_light: std::cell::RefCell::new(super::engine_light::EngineLight::default()),
            engine_light_image: std::cell::RefCell::new(
                super::engine_light::EngineLightImage::default(),
            ),
            tab_geometry: Rc::new(std::cell::RefCell::new(
                editor_ui::tab_card::GeometryCache::default(),
            )),
            strip_moving: std::cell::Cell::new(false),
            note_ticker: std::cell::RefCell::new(editor_ui::marquee::Marquee::new(
                "composer-note-ticker",
                "",
            )),
            model_menu_serial: std::cell::Cell::new(0),
            model_scroll: std::cell::RefCell::default(),
            model_tab: std::cell::RefCell::new(super::sliding_tab::SlidingTab::new(
                0.0,
                MODEL_ITEM_HEIGHT,
            )),
            last_visible: std::cell::RefCell::new(last_visible),
            tooltip: std::cell::RefCell::new(super::picker::PickerTooltip::default()),
        }
    }

    pub fn set_target(
        &self,
        core: &editor_core::Core,
        sessions: &mut super::persistence::DraftSessions,
        target: super::controller::ComposerTarget,
    ) {
        let changed = {
            let controller = self.controller.borrow();
            controller.target != target
                || core
                    .host(target.host())
                    .map(editor_core::HostData::instance)
                    != controller.host_instance()
        };
        if changed {
            self.tooltip.borrow_mut().clear();
            *self.engine_light.borrow_mut() = super::engine_light::EngineLight::default();
            *self.tab_geometry.borrow_mut() = editor_ui::tab_card::GeometryCache::default();
            self.strip_moving.set(false);
            *self.note_ticker.borrow_mut() =
                editor_ui::marquee::Marquee::new("composer-note-ticker", "");
        }
        self.controller
            .borrow_mut()
            .set_target(core, sessions, target);
        *self.last_visible.borrow_mut() = ComposerVisible::capture(&self.controller.borrow());
    }

    pub fn refresh_mentions(&self, host: &editor_core::HostData, theme: &ArtisanTheme) {
        let tokenizer = {
            let mut controller = self.controller.borrow_mut();
            controller.refresh_mentions(host);
            controller.mentions.tokenizer(theme)
        };
        self.editor.set_tokenizer(Some(tokenizer));
    }

    pub fn set_reduced_motion(&mut self, reduced: bool) {
        self.reduced_motion = reduced;
    }

    pub fn tooltip_target(&self) -> Option<(editor_core::HostInstance, u64)> {
        let composer = self.controller.borrow();
        composer
            .host_instance()
            .map(|instance| (instance, composer.generation()))
    }

    pub fn tooltip_deadline(&self) -> Option<std::time::Instant> {
        self.tooltip.borrow().deadline()
    }

    pub fn advance_tooltip(&self, now: std::time::Instant) -> bool {
        self.tooltip.borrow_mut().show_due(now)
    }

    pub fn shown_tooltip(&self) -> Option<super::picker::TooltipContent> {
        self.tooltip.borrow().shown().cloned()
    }

    pub fn tooltip_surface_factory(&self, theme: ArtisanTheme) -> Option<PickerTooltipSurface> {
        use super::picker::TooltipPlacement;
        let tooltip = self.shown_tooltip()?;
        let placement = match tooltip.placement {
            TooltipPlacement::Above => {
                editor_ui::hover_tooltip::TooltipPlacement::Above { max_width: 240.0 }
            }
            TooltipPlacement::Beside => {
                editor_ui::hover_tooltip::TooltipPlacement::Beside { max_width: 320.0 }
            }
        };
        Some(PickerTooltipSurface {
            anchor: tooltip.anchor.clone(),
            placement,
            surface: Rc::new(move |width, fixed| {
                picker_tooltip_surface(&theme, &tooltip, width, fixed)
            }),
        })
    }

    pub fn tooltip_presentation(
        &self,
        theme: ArtisanTheme,
        anchor: aqua_core::Rect,
        viewport: aqua_core::Size,
    ) -> Option<PickerTooltipPresentation> {
        use super::picker::TooltipPlacement;
        use aqua_ui::overlay::Side;
        let tooltip = self.shown_tooltip()?;
        let (side, width, fixed, snap_to_window) = match tooltip.placement {
            TooltipPlacement::Above => {
                (Side::Above, 240.0_f32.min(viewport.w.max(1.0)), false, true)
            }
            TooltipPlacement::Beside => {
                let right = viewport.w.max(1.0) - anchor.right() - 8.0;
                let left = anchor.x - 8.0;
                let use_right = right >= 320.0 || right >= left;
                let available = if use_right { right } else { left };
                (
                    if use_right { Side::After } else { Side::Before },
                    320.0_f32.min(available.max(1.0)),
                    true,
                    false,
                )
            }
        };
        Some(PickerTooltipPresentation {
            anchor: tooltip.anchor.clone(),
            side,
            snap_to_window,
            content: picker_tooltip_surface(&theme, &tooltip, width, fixed),
        })
    }


    pub fn dispose_popups(&mut self, app: &mut aqua::UiApp) {
        self.tooltip.borrow_mut().clear();
        if let Some(popup) = self.mention_popup.take() {
            popup.dispose(app);
        }
        if let Some(popup) = self.model_popup.take() {
            popup.dispose(app);
        }
        if let Some(popup) = self.project_popup.take() {
            popup.dispose(app);
        }
    }

    pub fn open_animated_popup(
        &mut self,
        app: &mut aqua::UiApp,
        anchor: aqua_core::NodeId,
        kind: PopupKind,
        factory: impl Fn() -> Element + 'static,
        now: f64,
    ) -> Result<(), String> {
        use aqua_ui::popup::PopupFocus;
        use editor_ui::popover::{AnimatedPickerHost, PickerEntrance};
        let entrance = PickerEntrance::Scale {
            origin: aqua_core::Point::new(0.0, 1.0),
        };
        let open_duration = editor_ui::picker_motion::DROPDOWN_OPEN;
        let close_duration = editor_ui::picker_motion::DROPDOWN_CLOSE;
        let action = Rc::clone(&self.dispatch);
        let popup = AnimatedPickerHost::open_above(
            app,
            anchor,
            factory,
            kind.gap(),
            PopupFocus::Preserve,
            entrance,
            if self.reduced_motion {
                std::time::Duration::ZERO
            } else {
                open_duration
            },
            if self.reduced_motion {
                std::time::Duration::ZERO
            } else {
                close_duration
            },
            now,
            move |_| {
                action(match kind {
                    PopupKind::Mentions => ComposerAction::DismissMentions,
                    _ => ComposerAction::ClosePicker,
                })
            },
        )?;
        match kind {
            PopupKind::Model => self.model_popup = Some(popup),
            PopupKind::Mentions => self.mention_popup = Some(popup),
            PopupKind::Project => self.project_popup = Some(popup),
        }
        Ok(())
    }

    pub fn open_popup_content(
        &mut self,
        app: &mut aqua::UiApp,
        anchor: aqua_core::NodeId,
        kind: PopupKind,
        initial: Element,
        now: f64,
    ) -> Result<(), String> {
        use aqua_ui::popup::PopupFocus;
        use editor_ui::popover::{AnimatedPickerHost, PickerEntrance};
        if self.popup_is_open(kind, app) {
            return self.refresh_popup_content(kind, app, initial, now);
        }
        let entrance = PickerEntrance::Scale {
            origin: aqua_core::Point::new(0.0, 1.0),
        };
        let open_duration = editor_ui::picker_motion::DROPDOWN_OPEN;
        let close_duration = editor_ui::picker_motion::DROPDOWN_CLOSE;
        let action = Rc::clone(&self.dispatch);
        let popup = AnimatedPickerHost::open_content(
            app,
            anchor,
            initial,
            kind.gap(),
            PopupFocus::Preserve,
            entrance,
            if self.reduced_motion {
                std::time::Duration::ZERO
            } else {
                open_duration
            },
            if self.reduced_motion {
                std::time::Duration::ZERO
            } else {
                close_duration
            },
            now,
            move |_| {
                action(match kind {
                    PopupKind::Mentions => ComposerAction::DismissMentions,
                    _ => ComposerAction::ClosePicker,
                })
            },
        )?;
        let slot = match kind {
            PopupKind::Model => &mut self.model_popup,
            PopupKind::Mentions => &mut self.mention_popup,
            PopupKind::Project => &mut self.project_popup,
        };
        if let Some(old) = slot.replace(popup) {
            old.dispose(app);
        }
        Ok(())
    }

    pub fn popup_root(&self, kind: PopupKind, app: &aqua::UiApp) -> Option<aqua_core::NodeId> {
        match kind {
            PopupKind::Model => self.model_popup.as_ref().and_then(|popup| popup.root(app)),
            PopupKind::Mentions => self
                .mention_popup
                .as_ref()
                .and_then(|popup| popup.root(app)),
            PopupKind::Project => self
                .project_popup
                .as_ref()
                .and_then(|popup| popup.root(app)),
        }
    }

    pub fn popup_is_open(&self, kind: PopupKind, app: &aqua::UiApp) -> bool {
        self.popup_root(kind, app).is_some()
    }

    pub fn refresh_popup_content(
        &mut self,
        kind: PopupKind,
        app: &mut aqua::UiApp,
        content: Element,
        now: f64,
    ) -> Result<(), String> {
        match kind {
            PopupKind::Model => self
                .model_popup
                .as_mut()
                .filter(|popup| popup.is_open(app))
                .map_or(Ok(()), |popup| {
                    if popup.is_closing() {
                        popup.begin_open(app, now)?;
                    }
                    popup.refresh_content(app, content)
                }),
            PopupKind::Mentions => self
                .mention_popup
                .as_mut()
                .filter(|popup| popup.is_open(app))
                .map_or(Ok(()), |popup| {
                    if popup.is_closing() {
                        popup.begin_open(app, now)?;
                    }
                    popup.refresh_content(app, content)
                }),
            PopupKind::Project => self
                .project_popup
                .as_mut()
                .filter(|popup| popup.is_open(app))
                .map_or(Ok(()), |popup| {
                    if popup.is_closing() {
                        popup.begin_open(app, now)?;
                    }
                    popup.refresh_content(app, content)
                }),
        }
    }

    pub fn open_project_popup(
        &mut self,
        app: &mut aqua::UiApp,
        anchor: aqua_core::NodeId,
        content: Element,
    ) -> Result<(), String> {
        self.open_popup_content(app, anchor, PopupKind::Project, content, 0.0)
    }

    pub fn close_popup(
        &mut self,
        app: &mut aqua::UiApp,
        kind: PopupKind,
        now: f64,
    ) -> Result<(), String> {
        match kind {
            PopupKind::Model => {
                if let Some(popup) = &mut self.model_popup {
                    popup.begin_close(app, now)?;
                }
            }
            PopupKind::Mentions => {
                if let Some(popup) = &mut self.mention_popup {
                    popup.begin_close(app, now)?;
                }
            }
            PopupKind::Project => {
                if let Some(popup) = &mut self.project_popup {
                    popup.begin_close(app, now)?;
                }
            }
        }
        Ok(())
    }

    pub fn needs_animation(&self, app: &aqua::UiApp) -> bool {
        self.strip_moving.get()
            || self.note_ticker.borrow().is_animating()
            || (self.controller.borrow().picker.menu == Some(super::picker::Menu::Model)
                && (self.engine_light.borrow().animating()
                    || !self.model_tab.borrow().settled()
                    || self.model_scroll.borrow().jumping()))
            || self
                .model_popup
                .as_ref()
                .is_some_and(|popup| popup.is_animating(app))
            || self
                .mention_popup
                .as_ref()
                .is_some_and(|popup| popup.is_animating(app))
            || self
                .project_popup
                .as_ref()
                .is_some_and(|popup| popup.is_animating(app))
    }

    pub fn advance_project_edges(
        &self,
        app: &mut aqua::UiApp,
        theme: &ArtisanTheme,
        reduced: bool,
        now: std::time::Instant,
    ) -> bool {
        let controller = self.controller.borrow();
        let picker = &controller.picker;
        match picker.menu {
            Some(super::picker::Menu::Project) => editor_ui::scroll_edge::EdgeSurface {
                edges: &picker.project_edges,
                scroller: PROJECT_SCROLL,
                top: PROJECT_EDGE_TOP,
                bottom: PROJECT_EDGE_BOTTOM,
                top_color: theme.colors.gray.step(7),
                bottom_color: theme.colors.gray.step(4),
                paint: editor_ui::scroll_edge::EdgePaint::Gradient,
            }
            .advance(app, reduced, now),
            Some(super::picker::Menu::Model) if picker.axis.is_none() => {
                editor_ui::scroll_edge::EdgeSurface {
                    edges: &picker.model_edges,
                    scroller: editor_ui::scroll::MODEL_MENU,
                    top: MODEL_EDGE_TOP,
                    bottom: MODEL_EDGE_BOTTOM,
                    top_color: theme.colors.background,
                    bottom_color: theme.colors.background,
                    paint: editor_ui::scroll_edge::EdgePaint::InsetShadow,
                }
                .advance(app, reduced, now)
            }
            _ => false,
        }
    }

    pub fn advance_note_ticker(&self, app: &mut aqua::UiApp, now: f64) {
        let key = aqua_ui::tree::hash_key(aqua_ui::element::Key::from("composer-note-ticker"));
        let node = app.tree().nodes_with_key(key).into_iter().next();
        if let Some(node) = node {
            self.note_ticker
                .borrow_mut()
                .advance(app, node, now, self.reduced_motion);
        } else {
            *self.note_ticker.borrow_mut() =
                editor_ui::marquee::Marquee::new("composer-note-ticker", "");
        }
    }

    pub fn advance_popups(&mut self, app: &mut aqua::UiApp, now: f64) -> Result<(), String> {
        for popup in [
            &mut self.model_popup,
            &mut self.mention_popup,
            &mut self.project_popup,
        ] {
            if let Some(popup) = popup {
                if popup.is_open(app) {
                    popup.advance(app, now)?;
                }
            }
        }
        Ok(())
    }

    pub fn refresh_model_popup(
        &mut self,
        app: &mut aqua::UiApp,
        content: Element,
    ) -> Result<(), String> {
        if let Some(popup) = &mut self.model_popup {
            if popup.is_open(app) {
                popup.refresh_content(app, content)?;
            }
        }
        Ok(())
    }

    pub fn dismiss_popups_for_event(
        &mut self,
        app: &mut aqua::UiApp,
        event: &aqua_core::InputEvent,
    ) -> Result<bool, String> {
        for (popup, close) in [
            (&mut self.model_popup, ComposerAction::ClosePicker),
            (&mut self.mention_popup, ComposerAction::DismissMentions),
            (&mut self.project_popup, ComposerAction::ClosePicker),
        ] {
            if let Some(popup) = popup {
                if popup.is_open(app) && !popup.is_closing() && popup.request_dismissal(app, event)? {
                    (self.dispatch)(close);
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    pub fn answer_wake_mailbox(&self) -> Option<&aqua::mailbox::Mailbox<()>> {
        self.answers.wake_mailbox()
    }

    fn send_answer(
        &self,
        host: &editor_core::HostData,
        answer: Option<(artisan_api::ItemId, artisan_api::Answer)>,
        generation: u64,
    ) {
        let Some((item, answer)) = answer else {
            return;
        };
        let client = host.client.clone();
        let host_id = host.id.clone();
        let instance = host.instance();
        let sender = self.answers.sender();
        editor_core::runtime::runtime().spawn(async move {
            let result =
                super::questionnaire::QuestionnairePanel::send(&client, &item, &answer).await;
            let _ = sender
                .send(AnswerCompletion {
                    host: host_id,
                    instance,
                    generation,
                    item,
                    result,
                })
                .await;
        });
    }

    pub fn drain_answers(&mut self, core: &editor_core::Core) -> Vec<ComposerEffect> {
        let mut effects = Vec::new();
        for completion in self.answers.drain() {
            let mut composer = self.controller.borrow_mut();
            if composer.target.host() != &completion.host
                || composer.generation() != completion.generation
                || composer.questionnaire.sending.as_ref() != Some(&completion.item)
                || core
                    .host_if_instance(&completion.host, completion.instance)
                    .is_none()
            {
                continue;
            }
            let mut text = composer.editor.borrow().text().to_owned();
            composer
                .questionnaire
                .settled(&completion.item, completion.result.err(), &mut text);
            composer.editor.borrow_mut().set_text(&text);
            effects.push(ComposerEffect::EditorChanged);
        }
        effects
    }

    pub fn drain_async(
        &mut self,
        core: &editor_core::Core,
        assets: &aqua::images::ImageAssets,
    ) -> ComposerDrain {
        let revision = self.controller.borrow().editor.borrow().paint_revision();
        let events = {
            let mut composer = self.controller.borrow_mut();
            composer.drain_turn_actions(core);
            composer.drain_attachments(core, assets);
            composer.sync(core)
        };
        let effects = self.drain_answers(core);
        let edited_here = self.controller.borrow().editor.borrow().paint_revision() != revision;
        let visible = ComposerVisible::capture(&self.controller.borrow());
        let changed = edited_here || !self.last_visible.borrow().same_chrome(&visible);
        *self.last_visible.borrow_mut() = visible;
        ComposerDrain {
            events,
            effects,
            changed,
        }
    }

    pub fn dispatch_with_change(
        &mut self,
        core: &mut editor_core::Core,
        action: ComposerAction,
        question: Option<&artisan_api::Question>,
    ) -> (Vec<ComposerEffect>, bool) {
        let valid = {
            let composer = self.controller.borrow();
            core.host(composer.target.host())
                .is_some_and(|host| composer.matches_host(host))
        };
        if !valid {
            return (Vec::new(), false);
        }
        let before = ComposerVisible::capture(&self.controller.borrow());
        let tooltip_before = self.tooltip.borrow().revision();
        let typed = matches!(
            action,
            ComposerAction::Editor(
                editor_ui::text::mount::EditorEvent::Changed
                    | editor_ui::text::mount::EditorEvent::SelectionChanged { .. }
                    | editor_ui::text::mount::EditorEvent::PresentationChanged
            )
        );
        let effects = self.dispatch(core, action, question);
        let after = ComposerVisible::capture(&self.controller.borrow());
        let changed = match typed {
            true => {
                !self.last_visible.borrow().same_chrome(&after) || !before.same_chrome(&after)
            }
            false => before != after || *self.last_visible.borrow() != after,
        } || tooltip_before != self.tooltip.borrow().revision()
            || self.model_scroll.borrow().jumping();
        *self.last_visible.borrow_mut() = after;
        (effects, changed)
    }

    pub fn dispatch(
        &mut self,
        core: &mut editor_core::Core,
        action: ComposerAction,
        question: Option<&artisan_api::Question>,
    ) -> Vec<ComposerEffect> {
        use editor_ui::text::mount::EditorEvent;
        let mut effects = Vec::new();
        let mut composer = self.controller.borrow_mut();
        let Some(host) = core.host(composer.target.host()) else {
            return effects;
        };
        if !composer.matches_host(host) {
            return effects;
        }
        composer.picker.refresh_settings(core, host);
        match action {
            ComposerAction::TooltipHover {
                instance,
                generation,
                anchor,
                content,
                hovered,
            } => {
                if composer.host_instance() == Some(instance)
                    && composer.generation() == generation
                    && composer.picker.menu == Some(super::picker::Menu::Model)
                {
                    let mut tooltip = self.tooltip.borrow_mut();
                    if hovered {
                        if let Some(content) = content {
                            tooltip.begin(content, std::time::Instant::now());
                        } else {
                            tooltip.clear();
                        }
                    } else {
                        tooltip.clear_for(&anchor);
                    }
                }
            }
            ComposerAction::DroppedPaths {
                paths,
                instance,
                generation,
            } => {
                if composer.host_instance() == Some(instance)
                    && composer.generation() == generation
                    && composer.draft_editable()
                {
                    composer.add_paths(paths);
                }
            }
            ComposerAction::DroppedThread(thread) => {
                if &thread.host == composer.target.host()
                    && artisan_mentions::valid_thread_title(&thread.title)
                {
                    let (at, lead) = {
                        let editor = composer.editor.borrow();
                        let text = editor.text();
                        let at = editor.caret().unwrap_or(text.len());
                        let spaced = text[..at]
                            .chars()
                            .next_back()
                            .is_none_or(char::is_whitespace);
                        (at, if spaced { "" } else { " " })
                    };
                    let mention = format!("{lead}@{} ", thread.title);
                    composer.editor.borrow_mut().replace(at..at, &mention);
                    composer.edited();
                    composer.sync_mentions();
                    effects.push(ComposerEffect::EditorChanged);
                    effects.push(ComposerEffect::FocusEditor);
                }
            }
            ComposerAction::Editor(EditorEvent::Changed) => {
                if !composer.questionnaire.writing_answer() {
                    composer.edited();
                }
                composer.sync_mentions();
            }
            ComposerAction::Editor(EditorEvent::SelectionChanged { .. }) => {
                composer.sync_mentions()
            }
            ComposerAction::Editor(EditorEvent::PresentationChanged) => {}
            ComposerAction::Editor(EditorEvent::List(key)) => {
                composer.mention_key(key);
                effects.push(ComposerEffect::EditorChanged);
            }
            ComposerAction::Editor(EditorEvent::PastedFiles { images, paths }) => {
                for image in images {
                    composer.add_image(
                        super::attachment::Source::Clipboard(image),
                        "Pasted image".into(),
                    );
                }
                composer.add_paths(paths);
            }
            ComposerAction::Editor(EditorEvent::Submitted) | ComposerAction::Send => {
                if composer.questionnaire.writing_answer() {
                    let mut text = composer.editor.borrow().text().to_owned();
                    let answer_text = text.trim().to_owned();
                    if !answer_text.is_empty() && composer.questionnaire.sending.is_none() {
                        let answer = composer.questionnaire.answer(
                            artisan_api::QuestionAnswer::Text { text: answer_text },
                            question.is_some(),
                            &mut text,
                        );
                        composer.editor.borrow_mut().set_text(&text);
                        self.send_answer(host, answer, composer.generation());
                        effects.push(ComposerEffect::EditorChanged);
                    }
                } else {
                    composer.send(host);
                }
            }
            ComposerAction::Stop => composer.stop(host),
            ComposerAction::Continue => composer.continue_turn(host),
            ComposerAction::ClearGoal => composer.clear_goal(host),
            ComposerAction::Unpark => composer.unpark(host),
            ComposerAction::ToggleModel => {
                self.tooltip.borrow_mut().clear();
                if composer.picker.menu == Some(super::picker::Menu::Model) {
                    composer.picker.menu = None;
                } else {
                    composer.open_picker(super::picker::Menu::Model, host);
                }
            }
            ComposerAction::ClosePicker => {
                self.tooltip.borrow_mut().clear();
                composer.picker.menu = None;
                composer.picker.axis = None;
            }
            ComposerAction::DismissMentions => {
                composer.mentions.dismiss();
                composer.sync_mentions();
            }
            ComposerAction::ToggleProject => {
                self.tooltip.borrow_mut().clear();
                if composer.picker.menu == Some(super::picker::Menu::Project) {
                    composer.picker.menu = None;
                } else {
                    composer.open_picker(super::picker::Menu::Project, host);
                }
            }
            ComposerAction::ChooseModel { engine, id } => {
                self.tooltip.borrow_mut().clear();
                let config = composer.choose_model(host, engine, &id);
                if let Some(config) = &config {
                    composer.apply_permissions(host, config.permissions);
                }
                let host_id = host.id.clone();
                let instance = host.instance();
                if let (Some(config), Some(host)) =
                    (config, core.host_mut_if_instance(&host_id, instance))
                {
                    host.remember_model_config(config);
                }
            }
            ComposerAction::OpenEngineSettings(engine) => {
                composer.picker.menu = None;
                core.navigate(editor_core::Screen::Settings(
                    editor_core::SettingsPage::Engine(engine),
                ));
            }
            ComposerAction::ToggleModelGroup { engine, name, open } => {
                composer.picker.toggle_group(engine, name.clone(), open);
                composer.refresh_picker(host);
                let host_id = host.id.clone();
                let instance = host.instance();
                if let Some(host) = core.host_mut_if_instance(&host_id, instance) {
                    host.remember_model_group(engine, name, !open);
                }
            }
            ComposerAction::JumpEngine(engine) => {
                self.tooltip.borrow_mut().clear();
                let layout = super::model_list::ListLayout::new(&composer.picker.models);
                if let Some(to) = layout.anchor(&composer.picker.models, engine, MODEL_LIST_HEIGHT)
                {
                    self.model_scroll.borrow_mut().jump(engine, to);
                }
            }
            ComposerAction::SelectPane(pane) => {
                self.tooltip.borrow_mut().clear();
                composer.picker.axis = pane;
                composer.picker.axis_cursor = None;
            }
            ComposerAction::AxisPointer { axis, index } => {
                if composer.picker.axis == Some(axis) {
                    composer.picker.axis_cursor = Some(super::picker::AxisCursor::Pointer(index));
                }
            }
            ComposerAction::AxisKey { axis, key } => {
                if composer.picker.axis == Some(axis) {
                    if let Some(preview) = composer
                        .picker
                        .pane_config(host, composer.effective_config(host))
                    {
                        let model = host.engines.get(&preview.engine).and_then(|status| {
                            preview
                                .model
                                .as_ref()
                                .and_then(|id| status.models.iter().find(|model| &model.id == id))
                        });
                        let options = composer.picker.axis_options(axis, &preview, model);
                        let selected = options
                            .iter()
                            .position(|option| option.selected)
                            .unwrap_or(0);
                        match composer.picker.axis_key(key, selected, options.len()) {
                            super::picker::AxisKeyResult::Choose(index) => {
                                self.tooltip.borrow_mut().clear();
                                if let Some(option) = options.into_iter().nth(index) {
                                    if composer.metadata_ready(host) && composer.draft_editable() {
                                        let config = option.apply;
                                        composer.apply_permissions(host, config.permissions);
                                        composer.config = Some(config.clone());
                                        composer.edited();
                                        let host_id = host.id.clone();
                                        let instance = host.instance();
                                        if let Some(host) =
                                            core.host_mut_if_instance(&host_id, instance)
                                        {
                                            host.remember_model_config(config);
                                        }
                                    }
                                }
                            }
                            super::picker::AxisKeyResult::Dismiss => {
                                self.tooltip.borrow_mut().clear();
                                composer.picker.axis = None;
                            }
                            _ => {}
                        }
                    }
                }
            }
            ComposerAction::ChooseAxis { axis, index } => {
                if !composer.metadata_ready(host) || !composer.draft_editable() {
                    return effects;
                }
                if let Some(preview) = composer
                    .picker
                    .pane_config(host, composer.effective_config(host))
                {
                    let model = host.engines.get(&preview.engine).and_then(|status| {
                        preview
                            .model
                            .as_ref()
                            .and_then(|id| status.models.iter().find(|model| &model.id == id))
                    });
                    if let Some(option) = composer.picker.axis_options(axis, &preview, model)
                        .into_iter()
                        .nth(index)
                    {
                        self.tooltip.borrow_mut().clear();
                        let config = option.apply;
                        composer.apply_permissions(host, config.permissions);
                        composer.config = Some(config.clone());
                        composer.edited();
                        let host_id = host.id.clone();
                        let instance = host.instance();
                        if let Some(host) = core.host_mut_if_instance(&host_id, instance) {
                            host.remember_model_config(config);
                        }
                    }
                }
            }
            ComposerAction::ChooseProject(id) => composer.choose_project(id),
            ComposerAction::NewProject => {
                composer.picker.menu = None;
                effects.push(ComposerEffect::NewProject);
            }
            ComposerAction::HoverMention(index) => {
                if index < composer.mentions.rows.len() {
                    composer.mentions.highlighted = index;
                }
            }
            ComposerAction::SelectMention(index) => {
                let replacement = {
                    let editor = composer.editor.borrow();
                    composer.mentions.replacement(editor.text(), index)
                };
                if let Some((range, text)) = replacement {
                    composer.editor.borrow_mut().replace(range, &text);
                    composer.edited();
                    composer.sync_mentions();
                    effects.push(ComposerEffect::EditorChanged);
                    effects.push(ComposerEffect::FocusEditor);
                }
            }
            ComposerAction::RemoveImage(key) => composer.remove_image(key),
            ComposerAction::RetryImage(key) => {
                if let Some(attachments) = &mut composer.attachments {
                    attachments.retry_preview(key);
                }
            }
            ComposerAction::RemoveFile(key) => composer.remove_file(key),
            ComposerAction::OpenImage(key) => effects.push(ComposerEffect::OpenImage(key)),
            ComposerAction::RetryDraft => {
                if let Some(session) = &composer.session {
                    session.retry();
                }
            }
            ComposerAction::KeepDraft => {
                if let Some(session) = &composer.session {
                    session.keep_local();
                }
            }
            ComposerAction::LoadOtherDraft => {
                if let Some(session) = &composer.session {
                    session.use_remote();
                }
            }
            ComposerAction::RetryThreadSettings => {
                effects.push(ComposerEffect::RetryThreadSettings)
            }
            ComposerAction::CloseNote => {
                composer
                    .strip
                    .close_note(std::time::Instant::now(), self.reduced_motion);
            }
            ComposerAction::OpenQuestionSource(source) => {
                effects.push(ComposerEffect::OpenQuestionSource(source))
            }
            ComposerAction::RetryQuestionPage => effects.push(ComposerEffect::RetryQuestionPage),
            ComposerAction::ActivateQuestionnaire(item) => {
                let mut text = composer.editor.borrow().text().to_owned();
                composer.questionnaire.activate(item, &mut text);
                composer.editor.borrow_mut().set_text(&text);
                effects.push(ComposerEffect::EditorChanged);
            }
            ComposerAction::DismissQuestionnaire => {
                let mut text = composer.editor.borrow().text().to_owned();
                let answer = composer.questionnaire.dismiss(&mut text);
                composer.editor.borrow_mut().set_text(&text);
                self.send_answer(host, answer, composer.generation());
                effects.push(ComposerEffect::EditorChanged);
            }
            ComposerAction::RetryQuestionnaire => {
                let answer = composer.questionnaire.retry();
                self.send_answer(host, answer, composer.generation());
            }
            ComposerAction::ChooseQuestionOption {
                item,
                question: expected,
                step,
                id,
                multi,
            } => {
                if composer.questionnaire.sending.is_none()
                    && composer.questionnaire.step == step
                    && composer
                        .questionnaire
                        .active()
                        .is_some_and(|active| active.item == item)
                    && question.is_some_and(|question| question.id == expected)
                {
                    if multi {
                        composer.questionnaire.toggle(id);
                    } else {
                        let mut text = composer.editor.borrow().text().to_owned();
                        let answer = composer.questionnaire.answer(
                            artisan_api::QuestionAnswer::Choices {
                                ids: vec![id].into(),
                            },
                            true,
                            &mut text,
                        );
                        composer.editor.borrow_mut().set_text(&text);
                        self.send_answer(host, answer, composer.generation());
                        effects.push(ComposerEffect::EditorChanged);
                    }
                }
            }
            ComposerAction::ContinueQuestionnaire => {
                if !composer.questionnaire.ticked.is_empty() {
                    let ids = std::mem::take(&mut composer.questionnaire.ticked);
                    let mut text = composer.editor.borrow().text().to_owned();
                    let answer = composer.questionnaire.answer(
                        artisan_api::QuestionAnswer::Choices { ids: ids.into() },
                        question.is_some(),
                        &mut text,
                    );
                    composer.editor.borrow_mut().set_text(&text);
                    self.send_answer(host, answer, composer.generation());
                    effects.push(ComposerEffect::EditorChanged);
                }
            }
        }
        effects
    }

    pub fn model_menu(
        &self,
        theme: ArtisanTheme,
        host: &editor_core::HostData,
        viewport: aqua_core::Size,
        kept_scroll: f32,
        assets: &aqua::images::ImageAssets,
        now: std::time::Instant,
    ) -> Result<Element, aqua::images::ImageAssetError> {
        use super::model_list::ListLayout;
        let controller = self.controller.borrow();
        let picker = &controller.picker;
        let current = controller.effective_config(host);
        let engines =
            super::picker::offered_engines(host, current.as_ref().map(|config| config.engine));
        let layout = ListLayout::new(&picker.models);
        let items = model_menu_items(host, picker, current.clone());
        let pane = picker
            .axis
            .filter(|axis| items.iter().any(|item| item.pane == Some(*axis)));
        let opening = self.model_menu_serial.replace(picker.open_serial) != picker.open_serial;
        let max = layout.max_scroll(MODEL_LIST_HEIGHT);
        let mut scrolling = self.model_scroll.borrow_mut();
        let kept = if opening {
            let centred = current
                .as_ref()
                .and_then(|config| {
                    layout.centred(
                        &picker.models,
                        config.engine,
                        config.model.as_deref()?,
                        MODEL_LIST_HEIGHT,
                    )
                })
                .unwrap_or(0.0);
            scrolling.open(centred);
            centred
        } else {
            kept_scroll
        };
        let scroll = if pane.is_none() {
            scrolling.offset(kept, max, now, self.reduced_motion)
        } else {
            scrolling.hide();
            0.0
        };
        if opening && scroll > 0.0 {
            picker.model_edges.restore(editor_ui::scroll_edge::Hidden {
                above: scroll,
                below: max - scroll,
            });
        }
        let spied = scrolling
            .target()
            .or_else(|| layout.spied(&picker.models, scroll, MODEL_LIST_HEIGHT))
            .filter(|engine| engines.iter().any(|status| status.engine == *engine));
        drop(scrolling);
        let active = items
            .iter()
            .position(|item| item.pane == pane)
            .unwrap_or_default() as f32;
        let tab = {
            let mut tab = self.model_tab.borrow_mut();
            let height = model_item_height(items.len());
            tab.resize(height);
            tab.aim(active * height, now);
            if opening || self.reduced_motion {
                tab.snap();
            } else {
                tab.advance(now);
            }
            *tab
        };
        let engine_light = if let Some((index, engine)) = spied.and_then(|engine| {
            engines
                .iter()
                .position(|status| status.engine == engine)
                .map(|index| (index, engine))
        }) {
            let mut light = self.engine_light.borrow_mut();
            light.measure(
                engine,
                index as f32 * (MODEL_BAR_HEIGHT + MODEL_BAR_GAP.px()),
                MODEL_BAR_HEIGHT,
                now,
            );
            match light.frame(now) {
                Some((left, _)) => Some(
                    self.engine_light_image
                        .borrow_mut()
                        .element(&theme, assets, left)?,
                ),
                None => None,
            }
        } else {
            None
        };
        Ok(model_menu(
            theme,
            host,
            picker,
            current,
            viewport,
            ModelMenuFrame {
                items,
                pane,
                scroll,
                spied,
                tab,
                engine_light,
            },
            (host.instance(), controller.generation()),
            Rc::clone(&self.dispatch),
        ))
    }

    pub fn surface(
        &self,
        theme: ArtisanTheme,
        host: &editor_core::HostData,
        placeholder: &str,
        questionnaires: Option<Element>,
        attachments: Option<Element>,
        mention_overlay: Option<Element>,
        thread_error: Option<Element>,
        now: std::time::Instant,
    ) -> Element {
        let suggestion = {
            let composer = self.controller.borrow();
            match &composer.target {
                super::controller::ComposerTarget::Thread { thread, .. }
                    if composer.matches_host(host)
                        && !composer.questionnaire.writing_answer()
                        && !host.is_working(thread) =>
                {
                    host.suggestion(thread).map(str::to_owned)
                }
                _ => None,
            }
        };
        self.editor.set_suggestion(suggestion);
        let (tab, strip) = {
            let mut controller = self.controller.borrow_mut();
            let project = match &controller.target {
                super::controller::ComposerTarget::NewThread { .. } => {
                    project_tab(theme, &controller, host, Rc::clone(&self.dispatch))
                }
                super::controller::ComposerTarget::Thread { .. } => None,
            };
            let tab = Some(
                UiBox::new()
                    .height(TriggerSize::Sm.height())
                    .spaced(Space::S4)
                    .style(Style::new().align(AlignItems::CENTER))
                    .child(self.model(theme, &controller, host))
                    .children(
                        project
                            .map(|project| {
                                [
                                    editor_ui::kit::Divider::vertical(&theme, Space::S16.px()),
                                    project,
                                ]
                            })
                            .into_iter()
                            .flatten(),
                    )
                    .build(),
            );
            let strip = if tab.is_some() {
                match controller.strip.motion.frame(now, self.reduced_motion) {
                    Some(frame) => {
                        self.strip_moving.set(frame.moving);
                        let mut ticker = self.note_ticker.borrow_mut();
                        ticker.set_text(frame.goal.objective.clone());
                        let content = goal_label(
                            theme,
                            &frame,
                            UiBox::new()
                                .style(TextRole::Label.inherit(&theme))
                                .child(ticker.build(TextRole::Label.spec().line))
                                .build(),
                            Rc::clone(&self.dispatch),
                        );
                        Some(editor_ui::tab_card::TabCardStrip {
                            reveal: frame.reveal,
                            content,
                        })
                    }
                    None => {
                        self.strip_moving.set(false);
                        None
                    }
                }
            } else {
                self.strip_moving.set(false);
                None
            };
            (tab, strip)
        };
        let nested = tab.is_some();
        let card = self.card(
            theme,
            host,
            nested,
            placeholder,
            questionnaires,
            attachments,
            mention_overlay,
            thread_error,
        );
        match tab {
            Some(tab) => editor_ui::tab_card::TabCard {
                id: "composer-tab-card".into(),
                tab,
                body: card,
                strip,
            }
            .build(theme, Rc::clone(&self.tab_geometry)),
            None => card,
        }
    }

    fn model(
        &self,
        theme: ArtisanTheme,
        controller: &super::controller::Composer,
        host: &editor_core::HostData,
    ) -> Element {
        let config = controller.effective_config(host);
        let model = config.as_ref().and_then(|config| {
            host.engines.get(&config.engine).and_then(|status| {
                status
                    .models
                    .iter()
                    .find(|model| Some(&model.id) == config.model.as_ref())
            })
        });
        model_trigger(theme, config.as_ref(), model, Rc::clone(&self.dispatch))
    }

    pub fn card(
        &self,
        theme: ArtisanTheme,
        host: &editor_core::HostData,
        nested: bool,
        placeholder: &str,
        questionnaires: Option<Element>,
        attachments: Option<Element>,
        mention_overlay: Option<Element>,
        thread_error: Option<Element>,
    ) -> Element {
        let controller = self.controller.borrow();
        let (face, disabled) = controller.primary(host);
        let instance = controller.host_instance();
        let generation = controller.generation();
        let host_id = controller.target.host().clone();
        let dispatch = Rc::clone(&self.dispatch);
        let draft_error = controller
            .session
            .as_ref()
            .and_then(|session| {
                session.read(|state| state.error.clone().map(|error| (error, state.conflict)))
            })
            .map(|(error, conflict)| {
                draft_error(theme, &error, conflict, Rc::clone(&self.dispatch))
            });
        composer_card(
            theme,
            nested,
            CardParts {
                prompt: self.editor.element_with_notice_keyed(
                    "composer-prompt",
                    editor_ui::text::Mode::Prompt,
                    &theme,
                    placeholder,
                    false,
                    false,
                ),
                primary: primary(theme, face, disabled, Rc::clone(&self.dispatch)),
                questionnaires,
                attachments,
                mention_overlay,
                thread_error,
                draft_error,
            },
        )
        .on_input(
            aqua_ui::input::InputKind::DragOver,
            aqua_ui::input::EventPhase::Bubble,
            {
                let host = host_id.clone();
                move |event| {
                    use aqua_ui::input::{InputResponse, RoutedInput};
                    let acceptable = matches!(
                        &event.input,
                        RoutedInput::DragOver { payload, .. }
                            if payload.get::<editor_core::DraggedThread>().is_some_and(|thread| {
                                thread.host == host
                                    && artisan_mentions::valid_thread_title(&thread.title)
                            })
                    );
                    InputResponse {
                        accept_drop: acceptable,
                        ..InputResponse::default()
                    }
                }
            },
        )
        .on_input(
            aqua_ui::input::InputKind::Drop,
            aqua_ui::input::EventPhase::Bubble,
            {
                let dispatch = Rc::clone(&self.dispatch);
                move |event| {
                    use aqua_ui::input::{InputResponse, RoutedInput};
                    match &event.input {
                        RoutedInput::Drop { payload, .. } => {
                            match payload.get::<editor_core::DraggedThread>() {
                                Some(thread) => {
                                    dispatch(ComposerAction::DroppedThread(thread.clone()));
                                    InputResponse::handled()
                                }
                                None => InputResponse::default(),
                            }
                        }
                        _ => InputResponse::default(),
                    }
                }
            },
        )
        .on_input(
            aqua_ui::input::InputKind::FilesDropped,
            aqua_ui::input::EventPhase::Bubble,
            move |event| {
                use aqua_ui::input::{InputResponse, RoutedInput};
                if let (
                    Some(instance),
                    RoutedInput::FilesDropped {
                        paths,
                        at: Some(at),
                    },
                ) = (instance, &event.input)
                {
                    if !paths.is_empty() && event.bounds.contains(*at) {
                        dispatch(ComposerAction::DroppedPaths {
                            paths: paths.to_vec(),
                            instance,
                            generation,
                        });
                        return InputResponse::handled();
                    }
                }
                InputResponse::default()
            },
        )
    }
}

pub fn attachments(
    theme: ArtisanTheme,
    controller: &super::controller::Composer,
    now: std::time::Instant,
    reduced_motion: bool,
    dispatch: Dispatch,
) -> Option<Element> {
    use editor_ui::attachment_tile::AttachmentTile;
    if controller.questionnaire.writing_answer() {
        return None;
    }
    let images = controller.attachments.as_ref();
    if images.is_none_or(|images| images.items.iter().all(|image| !image.shown))
        && controller.files.is_empty()
    {
        return None;
    }
    let editable = controller
        .session
        .as_ref()
        .is_some_and(|session| session.read(|state| !state.sending()));
    let mut row = UiBox::new()
        .padded(Space::S4)
        .spaced(Space::S8)
        .style(Style::new().align(AlignItems::CENTER));
    if let Some(images) = images {
        for image in images.items.iter().filter(|image| image.shown) {
            let key = image.key;
            let remove = Rc::clone(&dispatch);
            let open = Rc::clone(&dispatch);
            let mut tile = AttachmentTile::new(
                format!("composer-attachment-{key}"),
                theme,
                image.name.clone(),
            )
            .preview(
                image.preview.clone(),
                image
                    .revealed
                    .map(|instant| now.saturating_duration_since(instant)),
            )
            .preparing(matches!(
                image.state,
                super::attachment::State::Encoding | super::attachment::State::Uploading
            ))
            .error(
                image
                    .state
                    .failure()
                    .or(image.preview_error.as_deref())
                    .map(str::to_owned),
            )
            .shaking(
                images
                    .shake
                    .filter(|(shaken, _)| *shaken == key)
                    .map(|(_, instant)| now.saturating_duration_since(instant)),
            )
            .reduced_motion(reduced_motion)
            .removable(editable)
            .on_remove(move || remove(ComposerAction::RemoveImage(key)))
            .on_activate(move || open(ComposerAction::OpenImage(key)));
            if image.preview_error.is_some() {
                let retry = Rc::clone(&dispatch);
                tile = tile.on_retry(move || retry(ComposerAction::RetryImage(key)));
            }
            row = row.child(tile.build());
        }
    }
    for file in &controller.files {
        let key = file.key;
        let name = file.path.file_name().map_or_else(
            || file.path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let glyph = if file.folder {
            AssetId::TABLER_FOLDER
        } else {
            AssetId::TABLER_FILE_TEXT
        };
        let remove = Rc::clone(&dispatch);
        row = row.child(
            AttachmentTile::new(format!("composer-file-{key}"), theme, name)
                .file(glyph)
                .shaking(
                    controller
                        .file_shake
                        .filter(|(shaken, _)| *shaken == key)
                        .map(|(_, instant)| now.saturating_duration_since(instant)),
                )
                .reduced_motion(reduced_motion)
                .removable(editable)
                .on_remove(move || remove(ComposerAction::RemoveFile(key)))
                .build(),
        );
    }
    Some(
        Scroll::new()
            .width(Length::Pct(100.0))
            .child(row.build())
            .build()
            .with_key("composer-attachments"),
    )
}

const PROJECT_SCROLL: &str = "composer-project-scroll";
const PROJECT_EDGE_TOP: &str = "composer-project-edge-top";
const PROJECT_EDGE_BOTTOM: &str = "composer-project-edge-bottom";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupKind {
    Model,
    Project,
    Mentions,
}
impl PopupKind {
    pub const fn gap(self) -> f32 {
        match self {
            Self::Project | Self::Model | Self::Mentions => Space::S8.px(),
        }
    }
}

pub fn open_popup(
    app: &mut aqua::UiApp,
    owner: aqua_core::NodeId,
    content: Element,
    kind: PopupKind,
    dispatch: Dispatch,
) -> Result<editor_ui::popover::PickerPopupHost, String> {
    use aqua_ui::{overlay::DismissPolicy, popup::PopupFocus};
    editor_ui::popover::PickerPopupHost::open_above(
        app,
        owner,
        content,
        kind.gap(),
        PopupFocus::Preserve,
        DismissPolicy::LIGHT,
        move |_| {
            dispatch(match kind {
                PopupKind::Mentions => ComposerAction::DismissMentions,
                PopupKind::Model | PopupKind::Project => ComposerAction::ClosePicker,
            });
        },
    )
}

pub fn project_menu(
    theme: ArtisanTheme,
    host: &editor_core::HostData,
    picker: &super::picker::PickerState,
    selected: Option<&artisan_api::ProjectId>,
    viewport: aqua_core::Size,
    dispatch: Dispatch,
) -> Element {
    let surface = Surface::Picker(Picker::List);
    let row = Row::MenuItem;
    let muted = theme.colors.text_muted.to_reference_paint();
    let item =
        |key: String, title: String, leading: Element, chosen: bool, action: ComposerAction| {
            let dispatch = Rc::clone(&dispatch);
            let mut item = RowItem::new(key, theme, row, surface, title)
                .leading(leading)
                .on_activate(move || dispatch(action.clone()));
            if chosen {
                item = item.trailing(IconSize::Md.glyph(AssetId::TABLER_CHECK, muted));
            }
            item.build().aria_selected(chosen)
        };
    let mut rows = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .style(Style::new().min_width(Length::Pct(0.0)));
    for id in &picker.projects {
        let Some(project) = host.projects.get(id) else {
            continue;
        };
        rows = rows.child(item(
            format!("composer-project-{id}"),
            project.name.clone(),
            editor_ui::project_mark::Mark::of(host, id).view(theme.colors.text_muted),
            selected == Some(id),
            ComposerAction::ChooseProject(id.clone()),
        ));
    }
    let count = picker.projects.len();
    let mut list = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .style(Style::new().min_width(Length::Pct(0.0)));
    if count > 0 {
        list = list
            .child(
                UiBox::new()
                    .width(Length::Pct(100.0))
                    .clip(true)
                    .child(
                        Scroll::new()
                            .width(Length::Pct(100.0))
                            .height((count.min(PROJECT_ROWS_SHOWN) as f32) * row.height().unwrap_or_default())
                            .child(rows.build())
                            .build()
                            .with_key(PROJECT_SCROLL),
                    )
                    .child(picker.project_edges.band(
                        PROJECT_EDGE_TOP,
                        editor_ui::scroll_edge::Edge::Top,
                        theme.colors.gray.step(7),
                    ))
                    .child(picker.project_edges.band(
                        PROJECT_EDGE_BOTTOM,
                        editor_ui::scroll_edge::Edge::Bottom,
                        theme.colors.gray.step(4),
                    ))
                    .build(),
            )
            .child(Divider::Rule.build(&theme, surface.padding_px()));
    }
    let scratch = artisan_api::ProjectId::scratch();
    if host
        .projects
        .get(&scratch)
        .is_some_and(|project| !project.removed)
    {
        list = list.child(item(
            "composer-project-No project".to_owned(),
            "No project".to_owned(),
            IconSize::Md.glyph(AssetId::TABLER_MESSAGE_CIRCLE, muted),
            selected == Some(&scratch),
            ComposerAction::ChooseProject(scratch),
        ));
    }
    list = list.child(item(
        "composer-project-New project".to_owned(),
        "New project".to_owned(),
        IconSize::Md.glyph(AssetId::TABLER_FOLDER_PLUS, muted),
        false,
        ComposerAction::NewProject,
    ));
    let width = PROJECT_MENU_WIDTH.min((viewport.w - MENU_VIEWPORT_MARGIN).max(0.0));
    surface
        .build(&theme, vec![list.build()])
        .style(
            Style::new()
                .width(Length::Px(width))
                .min_width(Length::Px(width))
                .max_width(Length::Px(width))
                .flex_shrink(0.0),
        )
        .with_key("composer-project-menu")
}

fn engine_icon(theme: &ArtisanTheme, engine: artisan_api::EngineId) -> AssetId {
    match engine {
        artisan_api::EngineId::Claude => AssetId::SVGL_CLAUDE_AI,
        artisan_api::EngineId::Codex => AssetId::SVGL_OPENAI,
        artisan_api::EngineId::OpenCode if theme.colors.mode.is_dark() => AssetId::BRANDS_OPENCODE,
        artisan_api::EngineId::OpenCode => AssetId::BRANDS_OPENCODE_LIGHT,
    }
}

const MODEL_MENU_WIDTH: f32 = 528.0;
const MODEL_PANE_HEIGHT: f32 = 328.0;
const MODEL_SIDE_WIDTH: f32 = 164.0;
const MODEL_ITEM_HEIGHT: f32 = match Row::ListItem(Density::Tall).height() {
    Some(height) => height,
    None => 0.0,
};
const MODEL_ITEM_GUTTER: f32 = Inset::Menu.px();
const MODEL_PANE_INSET: f32 = Inset::Menu.px();
const MODEL_PANE_PADDING: Space = super::model_list::PADDING;
const MODEL_ENGINE_TAB: IconButtonSize = IconButtonSize::Md;
const MODEL_BAR_HEIGHT: f32 = MODEL_ENGINE_TAB.edge();
const MODEL_BAR_GAP: Space = Space::S4;
const MODEL_LIST_HEIGHT: f32 = MODEL_PANE_HEIGHT - MODEL_BAR_HEIGHT - 2.0 * MODEL_PANE_INSET;
const MODEL_EDGE_TOP: &str = "composer-model-edge-top";
const MODEL_EDGE_BOTTOM: &str = "composer-model-edge-bottom";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelMenuItem {
    pub pane: Option<super::picker::Axis>,
    pub label: &'static str,
    pub value: String,
}

pub struct ModelMenuFrame {
    pub items: Vec<ModelMenuItem>,
    pub pane: Option<super::picker::Axis>,
    pub scroll: f32,
    pub spied: Option<artisan_api::EngineId>,
    pub tab: super::sliding_tab::SlidingTab,
    pub engine_light: Option<Element>,
}

fn model_menu_items(
    host: &editor_core::HostData,
    picker: &super::picker::PickerState,
    current: Option<artisan_api::EngineConfig>,
) -> Vec<ModelMenuItem> {
    let config = picker.pane_config(host, current);
    let model = config.as_ref().and_then(|config| {
        host.engines
            .get(&config.engine)?
            .models
            .iter()
            .find(|model| config.model.as_ref() == Some(&model.id))
    });
    let mut items = vec![ModelMenuItem {
        pane: None,
        label: "Model",
        value: model.map_or_else(
            || {
                config
                    .as_ref()
                    .and_then(|config| config.model.clone())
                    .unwrap_or_default()
            },
            |model| model.name.clone(),
        ),
    }];
    if let Some(config) = &config {
        for axis in super::picker::Axis::PICKER {
            let options = picker.axis_options(axis, config, model);
            if !axis.shown(options.len()) {
                continue;
            }
            let value = options
                .iter()
                .find(|option| option.selected)
                .map(|option| option.label.clone())
                .unwrap_or_else(|| match axis {
                    super::picker::Axis::Special => "None".into(),
                    super::picker::Axis::Activation => "On".into(),
                    super::picker::Axis::Thinking => "Special mode".into(),
                    _ => "Not set".into(),
                });
            items.push(ModelMenuItem {
                pane: Some(axis),
                label: axis.title(),
                value,
            });
        }
    }
    items
}

#[expect(
    clippy::disallowed_types,
    reason = "the sliding pane tab is a path-traced silhouette painted on a canvas, like TabCard; its fill and rim are path paints, not box styles"
)]
fn model_menu_tab(
    theme: &ArtisanTheme,
    tab: super::sliding_tab::SlidingTab,
    radius: f32,
    header: f32,
) -> Element {
    use aqua_core::{
        DisplayItem, GpuiBackground, Transform2D,
        vector::{FillRule, GpuiPath, GpuiPathStyle, LineCap, LineJoin},
    };
    use aqua_ui::element::Canvas;
    let fill = GpuiBackground::Solid(theme.colors.background.to_reference_paint());
    let rim = GpuiBackground::Solid(theme.colors.text.at(Opacity::Hairline).to_reference_paint());
    Canvas::new()
        .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
        .on_paint(move |bounds| {
            let shape = super::sliding_tab::Bounds {
                width: bounds.w,
                height: bounds.h,
                split: MODEL_SIDE_WIDTH,
                radius,
            };
            let item = |commands, style, background| DisplayItem::GpuiPath {
                transformed_window_logical_bounds: bounds,
                path: std::sync::Arc::new(GpuiPath {
                    commands_local: commands,
                    origin_window_logical: bounds.origin(),
                    element_transform: Transform2D::IDENTITY,
                    style,
                    background,
                }),
            };
            vec![
                item(
                    tab.cutout(shape, header, MODEL_PANE_INSET),
                    GpuiPathStyle::Fill { rule: FillRule::EvenOdd },
                    fill,
                ),
                item(
                    tab.outline(shape),
                    GpuiPathStyle::Stroke {
                        width: 1.0,
                        cap: LineCap::Butt,
                        join: LineJoin::Round,
                        dash: Vec::new(),
                        dash_offset: 0.0,
                    },
                    rim,
                ),
            ]
        })
        .build()
        .exclude_from_hit_testing()
}

fn model_engine_bar(
    theme: ArtisanTheme,
    engines: &[&artisan_api::EngineStatus],
    spied: Option<artisan_api::EngineId>,
    engine_light: Option<Element>,
    tooltip_target: (editor_core::HostInstance, u64),
    dispatch: &Dispatch,
) -> Element {
    let mut bar = UiBox::new()
        .height(MODEL_BAR_HEIGHT)
        .spaced(MODEL_BAR_GAP)
        .style(Style::new().flex_shrink(0.0).margin(Length::Px(MODEL_PANE_INSET)));
    if let Some(engine_light) = engine_light {
        bar = bar.child(engine_light);
    }
    for status in engines {
        let engine = status.engine;
        let action = Rc::clone(dispatch);
        let tooltip_action = Rc::clone(dispatch);
        let tooltip_anchor = format!("composer-engine-{engine}");
        let gated = super::picker::gate(status);
        let reason = gated.map(|gate| {
            let name = engine.display_name();
            match gate {
                "Not installed" => format!("{name} isn't installed on this Forge."),
                "Sign in" => format!("Sign in to {name} to use its models."),
                "Signing in…" => format!("Signing in to {name}…"),
                "Checking…" => format!("Checking {name}…"),
                "No models" => format!("{name} hasn't reported any models."),
                "Discovering models…" | "Discovery failed" => status.discovery.description(artisan_api::now()),
                _ => unreachable!(),
            }
        });
        let tab = IconButton::new(
            format!("composer-engine-{engine}"),
            theme,
            engine_icon(&theme, engine),
            engine.display_name(),
            MODEL_ENGINE_TAB,
            Finish::Plain,
        )
        .nested(Radius::Panel.px(), MODEL_PANE_INSET)
        .selected(spied == Some(engine))
        .on_activate(move |_| action(ComposerAction::JumpEngine(engine)))
        .build()
        .aria_selected(spied == Some(engine))
        .on_hover_change(move |hovered| {
            if let Some(reason) = &reason {
                tooltip_action(ComposerAction::TooltipHover {
                    instance: tooltip_target.0,
                    generation: tooltip_target.1,
                    anchor: tooltip_anchor.clone(),
                    content: Some(super::picker::TooltipContent {
                        anchor: tooltip_anchor.clone(),
                        advisory: None,
                        text: reason.clone(),
                        placement: super::picker::TooltipPlacement::Above,
                    }),
                    hovered,
                });
            }
        });
        bar = bar.child(match gated {
            Some(_) => tab.style(Style::new().opacity(Opacity::Unavailable.alpha())),
            None => tab,
        });
    }
    bar.build()
}

fn model_gate_row(
    theme: &ArtisanTheme,
    status: &artisan_api::EngineStatus,
    dispatch: &Dispatch,
) -> Element {
    let engine = status.engine;
    let reason = super::picker::gate(status).unwrap_or_default();
    let mut body = UiBox::new()
        .layout("column")
        .height(super::model_list::GATE_ROW_HEIGHT)
        .padded(Space::S12)
        .spaced(Space::S4)
        .style(Style::new().flex_shrink(0.0))
        .child(
            TextRole::Label
                .text(
                    theme,
                    format!("{} is not ready: {}.", engine.display_name(), reason.to_lowercase()),
                )
                .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                .wrap(true)
                .build(),
        );
    if reason == "Not installed" || reason == "Sign in" {
        let action_label = if reason == "Sign in" {
            format!("Sign in to {}", engine.display_name())
        } else {
            format!("Install {}", engine.display_name())
        };
        let action = Rc::clone(dispatch);
        body = body.child(
            Link::new(
                format!("composer-engine-gate-{engine}"),
                *theme,
                LinkTone::Link,
                action_label,
            )
            .on_activate(move || action(ComposerAction::OpenEngineSettings(engine)))
            .build(),
        );
    }
    body.build()
}

fn model_list_pane(
    theme: ArtisanTheme,
    host: &editor_core::HostData,
    picker: &super::picker::PickerState,
    current: Option<&artisan_api::EngineConfig>,
    scroll: f32,
    dispatch: &Dispatch,
) -> Element {
    use super::model_list::{GROUP_ROW_HEIGHT, ROW_GAP, SECTION_SPACER};
    use super::picker::ModelRow;
    let muted = theme.colors.text_muted;
    let mut rows = UiBox::new()
        .layout("column")
        .padded(MODEL_PANE_PADDING)
        .spaced(ROW_GAP)
        .style(Style::new().flex_shrink(0.0));
    let mut section = None;
    for entry in &picker.models {
        let engine = entry.engine();
        if section.is_some_and(|previous| previous != engine) {
            rows = rows.child(
                UiBox::new()
                    .height(SECTION_SPACER)
                    .style(Style::new().flex_shrink(0.0))
                    .build(),
            );
        }
        section = Some(engine);
        match entry {
            ModelRow::Engine(_) => {
                rows = rows.child(
                    RowItem::new(
                        format!("composer-model-engine-{engine}"),
                        theme,
                        Row::Heading,
                        Surface::Menu,
                        engine.display_name(),
                    )
                    .build(),
                );
            }
            ModelRow::Gate(_) => {
                if let Some(status) = host.engines.get(&engine) {
                    rows = rows.child(model_gate_row(&theme, status, dispatch));
                }
            }
            ModelRow::Group { name, open, .. } => {
                let name = name.clone();
                let is_open = *open;
                let group_label = if name.is_empty() {
                    "Other".to_owned()
                } else {
                    name.clone()
                };
                let action = Rc::clone(dispatch);
                let heading = RowItem::new(
                    format!("composer-model-group-{engine}-{group_label}"),
                    theme,
                    Row::Heading,
                    Surface::Menu,
                    group_label.clone(),
                )
                .trailing(IconSize::Sm.glyph(
                    if is_open {
                        AssetId::TABLER_CHEVRON_DOWN
                    } else {
                        AssetId::TABLER_CHEVRON_RIGHT
                    },
                    muted.to_reference_paint(),
                ))
                .build();
                rows = rows.child(
                    HitArea::new(
                        format!("composer-model-group-toggle-{engine}-{group_label}"),
                        Role::Button,
                        group_label,
                    )
                    .child(heading)
                    .on_activate(move || {
                        action(ComposerAction::ToggleModelGroup {
                            engine,
                            name: name.clone(),
                            open: is_open,
                        })
                    })
                    .build()
                    .style(
                        Style::new()
                            .height(Length::Px(GROUP_ROW_HEIGHT))
                            .flex_shrink(0.0),
                    )
                    .aria_expanded(is_open),
                );
            }
            ModelRow::Model { id, index, .. } => {
                let Some(model) = host
                    .engines
                    .get(&engine)
                    .and_then(|status| status.models.get(*index))
                    .filter(|model| &model.id == id)
                else {
                    continue;
                };
                let chosen = current.is_some_and(|config| {
                    config.engine == engine && config.model.as_ref() == Some(id)
                });
                let model_id = id.clone();
                let action = Rc::clone(dispatch);
                let mut row = RowItem::new(
                    format!("composer-model-{engine}-{id}"),
                    theme,
                    Row::MenuItem,
                    Surface::Menu,
                    model.name.clone(),
                )
                .nested(Radius::Panel.px(), MODEL_PANE_INSET)
                .leading(
                    IconSize::Md.glyph(
                        model
                            .lab
                            .as_deref()
                            .and_then(lab_icon)
                            .unwrap_or_else(|| engine_icon(&theme, engine)),
                        theme.colors.text.to_reference_paint(),
                    ),
                )
                .on_activate(move || {
                    action(ComposerAction::ChooseModel {
                        engine,
                        id: model_id.clone(),
                    })
                });
                if chosen {
                    row = row.trailing(
                        IconSize::Md.glyph(AssetId::TABLER_CHECK, muted.to_reference_paint()),
                    );
                }
                rows = rows.child(row.build().aria_selected(chosen));
            }
        }
    }
    UiBox::new()
        .layout("column")
        .height(MODEL_LIST_HEIGHT)
        .clip(true)
        .style(Style::new().flex_shrink(0.0))
        .child(
            Scroll::new()
                .height(MODEL_LIST_HEIGHT)
                .offset(aqua_core::Point::new(0.0, scroll))
                .style(Style::new().column())
                .child(rows.build())
                .build()
                .with_key(editor_ui::scroll::MODEL_MENU),
        )
        .child(picker.model_edges.inset_band(
            MODEL_EDGE_TOP,
            editor_ui::scroll_edge::Edge::Top,
            theme.colors.background,
            0.0,
        ))
        .child(picker.model_edges.inset_band(
            MODEL_EDGE_BOTTOM,
            editor_ui::scroll_edge::Edge::Bottom,
            theme.colors.background,
            Radius::Panel.px(),
        ))
        .build()
}

fn model_axis_pane(
    theme: ArtisanTheme,
    axis: super::picker::Axis,
    options: &[super::picker::AxisOption],
    keyboard_cursor: Option<usize>,
    dispatch: &Dispatch,
) -> Element {
    let mut rows = UiBox::new()
        .layout("column")
        .padded(MODEL_PANE_PADDING)
        .spaced(Space::S2)
        .style(Style::new().flex_shrink(0.0));
    let mut special = false;
    for (index, option) in options.iter().enumerate() {
        if option.special && !special && axis != super::picker::Axis::Special {
            special = true;
            rows = rows
                .child(Divider::Rule.build(&theme, MODEL_PANE_INSET))
                .child(
                    RowItem::new(
                        format!("composer-axis-{axis:?}-special"),
                        theme,
                        Row::Heading,
                        Surface::Menu,
                        "Special efforts",
                    )
                    .build(),
                );
        }
        let advisory = option.advisory.as_deref().filter(|advisory| {
            !option
                .description
                .as_deref()
                .is_some_and(|description| description.contains(advisory))
        });
        let detail = [option.description.as_deref(), advisory]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
        let detail = Some(detail).filter(|detail| {
            !detail.is_empty()
                && !matches!(
                    axis,
                    super::picker::Axis::Speed
                        | super::picker::Axis::Thinking
                        | super::picker::Axis::Permission
                )
        });
        let action = Rc::clone(dispatch);
        let pointer_action = Rc::clone(dispatch);
        let shape = match detail {
            Some(_) => Row::ListItem(Density::Tall),
            None => Row::MenuItem,
        };
        let mut row = RowItem::new(
            format!("composer-axis-{axis:?}-{index}"),
            theme,
            shape,
            Surface::Menu,
            option.label.clone(),
        )
        .role(Role::MenuItem)
        .nested(Radius::Panel.px(), MODEL_PANE_INSET)
        .selected(keyboard_cursor == Some(index))
        .on_activate(move || action(ComposerAction::ChooseAxis { axis, index }))
        .on_hover(move |hovered| {
            if hovered {
                pointer_action(ComposerAction::AxisPointer { axis, index });
            }
        });
        if let Some(detail) = detail {
            row = row.detail(detail);
        }
        if option.selected {
            row = row.trailing(IconSize::Md.glyph(
                AssetId::TABLER_CHECK,
                theme.colors.text_muted.to_reference_paint(),
            ));
        }
        rows = rows.child(row.build().aria_selected(option.selected));
    }
    Scroll::new()
        .height(MODEL_PANE_HEIGHT)
        .style(Style::new().column())
        .child(rows.build())
        .build()
        .with_key(format!("composer-axis-pane-{axis:?}"))
}

pub fn model_menu(
    theme: ArtisanTheme,
    host: &editor_core::HostData,
    picker: &super::picker::PickerState,
    current: Option<artisan_api::EngineConfig>,
    viewport: aqua_core::Size,
    frame: ModelMenuFrame,
    tooltip_target: (editor_core::HostInstance, u64),
    dispatch: Dispatch,
) -> Element {
    let ModelMenuFrame {
        items,
        pane,
        scroll,
        spied,
        tab,
        engine_light,
    } = frame;
    let sheet = Surface::Picker(Picker::Sheet);
    let pane_radius = Radius::Panel.px();
    let engines = super::picker::offered_engines(host, current.as_ref().map(|config| config.engine));
    let model_icon = picker.pane_config(host, current.clone()).map(|config| {
        host.engines
            .get(&config.engine)
            .and_then(|status| {
                status
                    .models
                    .iter()
                    .find(|model| config.model.as_ref() == Some(&model.id))
            })
            .and_then(|model| model.lab.as_deref())
            .and_then(lab_icon)
            .unwrap_or_else(|| engine_icon(&theme, config.engine))
    });
    let item_height = model_item_height(items.len());
    let mut buttons = UiBox::new()
        .layout("column")
        .width(MODEL_SIDE_WIDTH - MODEL_ITEM_GUTTER)
        .style(Style::new().flex_shrink(0.0));
    for (position, item) in items.iter().enumerate() {
        let target = item.pane;
        let action = Rc::clone(&dispatch);
        let mut side = RowItem::new(
            format!("composer-pane-{position}"),
            theme,
            Row::ListItem(Density::Tall),
            sheet,
            item.value.clone(),
        )
        .detail(item.label)
        .role(Role::Tab)
        .nested(pane_radius, 0.0)
        .on_activate(move || action(ComposerAction::SelectPane(target)));
        if let Some(icon) = model_icon.filter(|_| target.is_none()) {
            side = side.leading(IconSize::Md.glyph(icon, theme.colors.text.to_reference_paint()));
        }
        buttons = buttons.child(
            side.build()
                .style(Style::new().height(Length::Px(item_height)).flex_shrink(0.0))
                .aria_label(format!("{}: {}", item.label, item.value))
                .aria_selected(target == pane),
        );
    }
    let mut pane_body = UiBox::new()
        .layout("column")
        .height(MODEL_PANE_HEIGHT)
        .clip(true)
        .style(Radius::Panel.style())
        .style(Style::new().flex_grow(1.0).min_width(Length::Pct(0.0)));
    match pane {
        None => {
            pane_body = pane_body
                .child(model_engine_bar(
                    theme,
                    &engines,
                    spied,
                    engine_light,
                    tooltip_target,
                    &dispatch,
                ))
                .child(model_list_pane(
                    theme,
                    host,
                    picker,
                    current.as_ref(),
                    scroll,
                    &dispatch,
                ));
        }
        Some(axis) => {
            let config = picker.pane_config(host, current.clone());
            let model = config.as_ref().and_then(|config| {
                host.engines
                    .get(&config.engine)?
                    .models
                    .iter()
                    .find(|model| config.model.as_ref() == Some(&model.id))
            });
            let options = config.as_ref().map_or_else(Vec::new, |config| {
                picker.axis_options(axis, config, model)
            });
            pane_body = pane_body.child(model_axis_pane(
                theme,
                axis,
                &options,
                picker.keyboard_axis_cursor(),
                &dispatch,
            ));
        }
    }
    let key_dispatch = Rc::clone(&dispatch);
    let resolution_note = current.as_ref().and_then(|config| {
        let model = host.engines.get(&config.engine)?.models.iter().find(|model| config.model.as_ref() == Some(&model.id))?;
        let changes = model.resolve_config(config, &picker.customizations).changes;
        (!changes.is_empty()).then(|| changes.into_iter().map(|change| format!("{}: {}", change.axis.title(), change.reason)).collect::<Vec<_>>().join(" "))
    });
    let mut children = vec![
        UiBox::new()
            .height(MODEL_PANE_HEIGHT)
            .child(model_menu_tab(
                &theme,
                tab,
                pane_radius,
                if pane.is_none() { MODEL_BAR_HEIGHT + MODEL_PANE_INSET } else { 0.0 },
            ))
            .child(buttons.build())
            .child(UiBox::new().width(MODEL_ITEM_GUTTER).style(Style::new().flex_shrink(0.0)).build())
            .child(pane_body.build())
            .build(),
    ];
    if let Some(note) = resolution_note {
        children.push(
            UiBox::new()
                .layout("column")
                .padded(MODEL_PANE_PADDING)
                .style(Style::new().min_width(Length::Pct(0.0)))
                .child(TextRole::Caption.text(&theme, note).wrap(true).build())
                .build(),
        );
    }
    sheet
        .build(&theme, children)
        .style(Style::new().width(Length::Px(MODEL_MENU_WIDTH.min(viewport.w - MENU_VIEWPORT_MARGIN))))
        .with_key("composer-model-menu")
        .on_input(
            aqua_ui::input::InputKind::KeyDown,
            aqua_ui::input::EventPhase::Bubble,
            move |event| {
                use aqua_core::{InputEvent, Key, NamedKey};
                use aqua_ui::input::{InputResponse, RoutedInput};
                let Some(axis) = pane else {
                    return InputResponse::default();
                };
                let key = match &event.input {
                    RoutedInput::Native(InputEvent::KeyDown(event)) => match event.key {
                        Key::Named(NamedKey::Home) => Some(super::picker::AxisKey::Home),
                        Key::Named(NamedKey::End) => Some(super::picker::AxisKey::End),
                        Key::Named(NamedKey::Up) => Some(super::picker::AxisKey::Up),
                        Key::Named(NamedKey::Down) => Some(super::picker::AxisKey::Down),
                        Key::Named(NamedKey::Enter) | Key::Char(' ') => {
                            Some(super::picker::AxisKey::Accept)
                        }
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(key) = key {
                    key_dispatch(ComposerAction::AxisKey { axis, key });
                    InputResponse {
                        stop_propagation: true,
                        ..InputResponse::handled()
                    }
                } else {
                    InputResponse::default()
                }
            },
        )
}

fn model_item_height(count: usize) -> f32 {
    MODEL_ITEM_HEIGHT.min(MODEL_PANE_HEIGHT / count.max(1) as f32)
}


fn lab_icon(lab: &str) -> Option<AssetId> {
    Some(match lab {
        "Alibaba" => AssetId::SVGL_QWEN,
        "Anthropic" => AssetId::SVGL_CLAUDE_AI,
        "DeepSeek" => AssetId::SVGL_DEEPSEEK,
        "Google" => AssetId::LOBE_GOOGLE,
        "Meituan" => AssetId::SIMPLE_ICONS_MEITUAN,
        "Meta" => AssetId::SVGL_META,
        "MiniMax" => AssetId::LOBE_MINIMAX,
        "Mistral" => AssetId::LOBE_MISTRAL,
        "Moonshot AI" => AssetId::BRANDS_KIMI,
        "NVIDIA" => AssetId::LOBE_NVIDIA,
        "OpenAI" => AssetId::SVGL_OPENAI,
        "Reflection AI" => AssetId::BRANDS_REFLECTION,
        "StepFun" => AssetId::LOBE_STEPFUN,
        "Tencent" => AssetId::LOBE_TENCENT,
        "xAI" => AssetId::LOBE_XAI,
        "Xiaomi" => AssetId::SIMPLE_ICONS_XIAOMI,
        "Z.ai" => AssetId::BRANDS_ZAI,
        _ => return None,
    })
}

pub fn project_tab(
    theme: ArtisanTheme,
    controller: &super::controller::Composer,
    host: &editor_core::HostData,
    dispatch: Dispatch,
) -> Option<Element> {
    let fitted = |element: Element| {
        UiBox::new()
            .style(
                Style::new()
                    .min_width(Length::Pct(0.0))
                    .max_width(Length::Px(PROJECT_TAB_WIDTH)),
            )
            .child(element)
            .build()
    };
    match &controller.target {
        super::controller::ComposerTarget::Thread { thread, .. } => {
            let thread = host.threads.get(thread)?;
            let project = host.projects.get(&thread.project_id)?;
            Some(fitted(
                RowItem::new(
                    "composer-project",
                    theme,
                    Row::Heading,
                    Surface::Composer,
                    project.name.clone(),
                )
                .leading(
                    editor_ui::project_mark::Mark::of(host, &thread.project_id)
                        .view(theme.colors.text_muted),
                )
                .build(),
            ))
        }
        super::controller::ComposerTarget::NewThread { .. } => {
            let project = controller
                .project(host)
                .filter(|id| host.projects.contains_key(id));
            let name = project
                .as_ref()
                .and_then(|id| host.projects.get(id))
                .map_or_else(
                    || "Choose a project".to_owned(),
                    |project| project.name.clone(),
                );
            let lead = project
                .as_ref()
                .map_or(Lead::Glyph(AssetId::TABLER_FOLDER), |id| {
                    Lead::Avatar(
                        editor_ui::project_mark::Mark::of(host, id).view(theme.colors.text_muted),
                    )
                });
            let open = controller.picker.menu == Some(super::picker::Menu::Project);
            Some(fitted(
                Trigger::new("composer-project", theme, TriggerSize::Sm, name)
                    .lead(lead)
                    .muted()
                    .open(open)
                    .on_activate(move || dispatch(ComposerAction::ToggleProject))
                    .build(),
            ))
        }
    }
}
