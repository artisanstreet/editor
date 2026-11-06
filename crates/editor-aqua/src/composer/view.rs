use std::rc::Rc;

use crate::ui::{
    button::{Button, ButtonShape, ButtonVariant},
    theme::{ArtisanTheme, RadiusStep, RadiusTokens},
};
use aqua_ui::{
    element::{Box as UiBox, Element},
    style::{Length, Style},
};
use editor_assets::AssetId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposerAction {
    Editor(crate::ui::text::mount::EditorEvent),
    DroppedPaths {
        paths: Vec<std::path::PathBuf>,
        instance: crate::core::HostInstance,
        generation: u64,
    },
    DroppedThread(crate::core::DraggedThread),
    TooltipHover {
        instance: crate::core::HostInstance,
        generation: u64,
        anchor: String,
        content: Option<super::picker::TooltipContent>,
        hovered: bool,
        axis: Option<super::picker::Axis>,
    },
    Send,
    Stop,
    Continue,
    ToggleModel,
    ToggleProject,
    BrowseEngine(artisan_api::EngineId),
    OpenEngineSettings(artisan_api::EngineId),
    PreviewModel {
        engine: artisan_api::EngineId,
        id: String,
    },
    ToggleModelGroup {
        engine: artisan_api::EngineId,
        name: String,
        open: bool,
    },
    ToggleAxis(super::picker::Axis),
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
    CloseAxis,
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
    Button::icon(id, theme, icon, label)
        .debug_selector(id)
        .variant(ButtonVariant::Accent)
        .shape(ButtonShape::Rounded)
        .disabled(disabled)
        .on_activate(move |_| dispatch(action.clone()))
        .build()
}

pub fn frame(theme: ArtisanTheme, nested: bool, content: Element) -> Element {
    crate::ui::glass::ComposerFrame::new(theme, content)
        .nested(nested)
        .build()
}

pub fn model_trigger(
    theme: ArtisanTheme,
    config: Option<&artisan_api::EngineConfig>,
    model: Option<&artisan_api::Model>,
    dispatch: Dispatch,
) -> Element {
    use crate::ui::asset::source_asset_glyph;
    use aqua_core::{Insets, Role};
    use aqua_text::{FontFamily, FontWeight, StyledSpan, TextStyle};
    use aqua_ui::{
        element::{Button as UiButton, Text},
        style::Style,
    };
    let style = TextStyle {
        families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
        size_px: 14.0,
        line_height_px: 20.0,
        weight: FontWeight::NORMAL,
        ..TextStyle::default()
    };
    let muted = theme.colors.text_muted.to_reference_text_color();
    let mut body = UiBox::new()
        .gap(4.0)
        .style(Style::new().min_width(Length::Px(0.0)));
    let token = |label: String, color| {
        Text::new(label)
            .text_style(style.clone())
            .gpui_source_color(color)
            .wrap(false)
            .build()
    };
    if let Some(config) = config {
        let name = model.map_or_else(
            || config.model.clone().unwrap_or_else(|| "No model".into()),
            |model| model.name.clone(),
        );
        body = body.child(token(name, theme.colors.text.to_reference_text_color()));
        if let Some(context) =
            model.and_then(|model| config.context_window_override().or(model.context_window))
        {
            let label = if context >= 1_000_000 && context.is_multiple_of(1_000_000) {
                format!("{}M", context / 1_000_000)
            } else {
                format!("{}K", context / 1_000)
            };
            body = body.child(token(label, muted));
        }
        if config.engine != artisan_api::EngineId::OpenCode
            && let Some(effort) = &config.effort
        {
            let label = if effort == "xhigh" {
                "Extra High".to_owned()
            } else {
                effort
                    .split(['_', '-'])
                    .enumerate()
                    .map(|(index, part)| {
                        let mut chars = part.chars();
                        chars.next().map_or_else(String::new, |first| {
                            if index == 0 {
                                first.to_uppercase().chain(chars).collect()
                            } else {
                                std::iter::once(first).chain(chars).collect()
                            }
                        })
                    })
                    .collect::<Vec<String>>()
                    .join(" ")
            };
            body = body.child(token(label, muted));
        }
        if let Some(model) = model {
            if let Some(variant) = model
                .variant_for(config.variant.as_deref())
                .and_then(|id| model.variants.iter().find(|variant| variant.id == id))
            {
                body = body.child(token(variant.label.clone(), muted));
            }
            if let Some(speed) = config.speed.as_ref().and_then(|id| {
                model
                    .speeds
                    .iter()
                    .find(|speed| &speed.id == id && !speed.default)
            }) {
                let gradient = if speed.id == "superfast" {
                    Some((theme.colors.palette[7], theme.colors.palette[3]))
                } else if speed.id == "fast" || speed.label.eq_ignore_ascii_case("fast") {
                    Some((theme.colors.palette[1], theme.colors.palette[4]))
                } else {
                    None
                };
                let speed_element = if let Some((start, end)) = gradient {
                    let count = speed.label.chars().count();
                    let spans = speed
                        .label
                        .chars()
                        .enumerate()
                        .map(|(index, character)| {
                            let progress = if count > 1 {
                                index as f32 / (count - 1) as f32
                            } else {
                                0.0
                            };
                            let color = start.animate_to(end, progress).to_reference_text_color();
                            StyledSpan::new(character.to_string(), style.clone()).gpui_source_color(
                                [
                                    color.hue_degrees,
                                    color.saturation,
                                    color.lightness,
                                    color.alpha,
                                ],
                            )
                        })
                        .collect();
                    Text::new("")
                        .spans(spans)
                        .text_style(style.clone())
                        .wrap(false)
                        .build()
                } else {
                    token(
                        speed.label.clone(),
                        theme.colors.text.to_reference_text_color(),
                    )
                };
                body = body.child(speed_element);
            }
        }
    } else {
        body = body.child(token(
            "No model".into(),
            theme.colors.text.to_reference_text_color(),
        ));
    }
    let mut trigger = UiButton::new()
        .height(32.0)
        .gap(8.0)
        .radius(RadiusTokens::value(RadiusStep::Lg))
        .style(
            Style::new()
                .align(aqua_ui::AlignItems::CENTER)
                .max_width(Length::Px(360.0))
                .padding_insets(Insets {
                    left: 8.0,
                    right: 8.0,
                    top: 0.0,
                    bottom: 0.0,
                }),
        );
    if let Some(config) = config {
        let icon = match config.engine {
            artisan_api::EngineId::Claude => AssetId::SVGL_CLAUDE_AI,
            artisan_api::EngineId::Codex => AssetId::SVGL_OPENAI,
            artisan_api::EngineId::OpenCode if theme.colors.mode.is_dark() => {
                AssetId::BRANDS_OPENCODE
            }
            artisan_api::EngineId::OpenCode => AssetId::BRANDS_OPENCODE_LIGHT,
        };
        trigger = trigger.child(source_asset_glyph(
            icon,
            16.0,
            theme.colors.text.to_reference_paint(),
        ));
    }
    trigger
        .child(body.build())
        .child(source_asset_glyph(
            AssetId::TABLER_SELECTOR,
            14.0,
            theme.colors.text_muted.to_reference_paint(),
        ))
        .on_activate(move || dispatch(ComposerAction::ToggleModel))
        .build()
        .with_key("composer-model")
        .role(Role::Button)
        .aria_label("Choose a model")
        .focusable(true)
}

pub fn controls(model: Element, primary: Element) -> Element {
    use aqua_ui::{AlignItems, JustifyContent};
    UiBox::new()
        .width(Length::Pct(100.0))
        .height(32.0)
        .gap(8.0)
        .style(
            Style::new()
                .flex_shrink(0.0)
                .align(AlignItems::CENTER)
                .justify(JustifyContent::SPACE_BETWEEN),
        )
        .child(
            UiBox::new()
                .gap(2.0)
                .style(
                    Style::new()
                        .min_width(Length::Px(0.0))
                        .align(AlignItems::CENTER),
                )
                .child(model)
                .build(),
        )
        .child(primary)
        .build()
        .with_key("composer-controls")
}

fn label(
    theme: &ArtisanTheme,
    text: String,
    size: f32,
    line_height: f32,
    color: aqua::GpuiTextColor,
) -> Element {
    use aqua_text::{FontFamily, FontWeight, TextStyle};
    use aqua_ui::element::Text;
    Text::new(text)
        .text_style(TextStyle {
            families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
            size_px: size,
            line_height_px: line_height,
            weight: FontWeight::NORMAL,
            ..TextStyle::default()
        })
        .gpui_source_color(color)
        .wrap(false)
        .build()
}

fn semibold(
    theme: &ArtisanTheme,
    text: String,
    size: f32,
    line_height: f32,
    color: aqua::GpuiTextColor,
) -> Element {
    use aqua_text::{FontFamily, FontWeight, TextStyle};
    aqua_ui::element::Text::new(text)
        .text_style(TextStyle {
            families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
            size_px: size,
            line_height_px: line_height,
            weight: FontWeight(600),
            ..TextStyle::default()
        })
        .gpui_source_color(color)
        .wrap(false)
        .build()
}

fn paragraph(
    theme: &ArtisanTheme,
    text: String,
    size: f32,
    line_height: f32,
    color: aqua::GpuiTextColor,
) -> Element {
    use aqua_text::{FontFamily, FontWeight, TextStyle};
    aqua_ui::element::Text::new(text)
        .text_style(TextStyle {
            families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
            size_px: size,
            line_height_px: line_height,
            weight: FontWeight::NORMAL,
            ..TextStyle::default()
        })
        .gpui_source_color(color)
        .wrap(true)
        .build()
}

pub fn mention_menu(
    theme: ArtisanTheme,
    mentions: &super::mentions::Mentions,
    viewport: aqua_core::Size,
    width: f32,
    dispatch: Dispatch,
) -> Option<Element> {
    use super::mentions::Kind;
    use crate::ui::{
        asset::source_asset_glyph, glass::glass_menu_separator, popover::PopoverPanel,
    };
    use aqua_core::{Insets, Role};
    use aqua_ui::{AlignItems, element::Button as UiButton};
    if mentions.rows.is_empty() {
        return None;
    }
    let mut entries = UiBox::new().layout("column");
    let mut previous = None;
    let mut content_height = 0.0_f32;
    for (index, catalog_index) in mentions.rows.iter().copied().enumerate() {
        let Some(row) = mentions.catalog.get(catalog_index) else {
            continue;
        };
        if previous != Some(row.kind) {
            if previous.is_some() {
                entries = entries.child(glass_menu_separator(&theme));
                content_height += 9.0;
            }
            content_height += 30.0;
            let heading = match row.kind {
                Kind::Command => "Commands",
                Kind::Skill => "Skills",
                Kind::Connector => "Connectors",
                Kind::Thread => "Threads",
            };
            entries = entries.child(
                UiBox::new()
                    .height(30.0)
                    .style(
                        Style::new()
                            .align(AlignItems::CENTER)
                            .padding_insets(Insets {
                                left: 10.0,
                                right: 10.0,
                                top: 0.0,
                                bottom: 0.0,
                            }),
                    )
                    .child(label(
                        &theme,
                        heading.into(),
                        12.0,
                        16.0,
                        theme.colors.text_muted.to_reference_text_color(),
                    ))
                    .build(),
            );
            previous = Some(row.kind);
        }
        content_height += 36.0;
        let icon = match row.kind {
            Kind::Command => AssetId::TABLER_TERMINAL_2,
            Kind::Skill => AssetId::TABLER_FILE_SPARK,
            Kind::Connector => AssetId::TABLER_PLUG_CONNECTED,
            Kind::Thread => AssetId::TABLER_MESSAGE_CIRCLE,
        };
        let action = Rc::clone(&dispatch);
        let hover = Rc::clone(&dispatch);
        let title = label(
            &theme,
            row.title.clone(),
            14.0,
            20.0,
            theme.colors.text.to_reference_text_color(),
        );
        let description = label(
            &theme,
            row.description.clone(),
            14.0,
            20.0,
            theme.colors.text_muted.to_reference_text_color(),
        );
        let (title_style, description_style) = if row.kind == Kind::Thread {
            (
                Style::new().flex_grow(1.0).min_width(Length::Px(0.0)),
                Style::new().flex_shrink(0.0).max_width(Length::Px(180.0)),
            )
        } else {
            (
                Style::new().flex_shrink(0.0).max_width(Length::Px(260.0)),
                Style::new().flex_grow(1.0).min_width(Length::Px(0.0)),
            )
        };
        let item = UiButton::new()
            .width(Length::Pct(100.0))
            .height(36.0)
            .gap(10.0)
            .radius(RadiusTokens::value(RadiusStep::LgMid))
            .style(
                Style::new()
                    .align(AlignItems::CENTER)
                    .padding_insets(Insets {
                        left: 10.0,
                        right: 10.0,
                        top: 0.0,
                        bottom: 0.0,
                    }),
            )
            .child(source_asset_glyph(
                icon,
                16.0,
                theme.colors.text_muted.to_reference_paint(),
            ))
            .child(UiBox::new().style(title_style).child(title).build())
            .child(
                UiBox::new()
                    .style(description_style)
                    .child(description)
                    .build(),
            )
            .on_activate(move || action(ComposerAction::SelectMention(index)))
            .build()
            .with_key(format!("composer-mention-row-{index}"))
            .role(Role::MenuItem)
            .aria_label(row.title.clone())
            .aria_selected(index == mentions.highlighted)
            .focusable(true)
            .on_input(
                aqua_ui::input::InputKind::PointerEnter,
                aqua_ui::input::EventPhase::Bubble,
                move |_| {
                    hover(ComposerAction::HoverMention(index));
                    aqua_ui::input::InputResponse::default()
                },
            );
        entries = entries.child(item);
    }
    let list = aqua_ui::element::Scroll::new()
        .height(content_height.min(36.0_f32.mul_add(9.5, 30.0)))
        .child(entries.build())
        .build();
    let panel = PopoverPanel::new("composer-mentions-panel", theme, list, viewport.w, viewport.h)
        .quiet_glass_menu()
        .build();
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
    use crate::ui::asset::source_asset_glyph;
    use aqua_core::{Insets, Role};
    use aqua_ui::{AlignItems, element::Button as UiButton};
    let active = panel.active()?;
    let Some(question) = question else {
        let mut loading = UiBox::new().layout("column").child(label(
            &theme,
            load_error.unwrap_or("Loading question…").into(),
            14.0,
            20.0,
            theme.colors.text.to_reference_text_color(),
        ));
        if load_error.is_some() {
            let action = Rc::clone(&dispatch);
            loading = loading.child(
                Button::text("question-page-retry", theme, "Retry")
                    .on_activate(move |_| action(ComposerAction::RetryQuestionPage))
                    .build(),
            );
        }
        return Some(loading.build());
    };
    let sending = panel.sending.is_some();
    let action = Rc::clone(&dispatch);
    let dismiss = UiButton::new()
        .width(20.0)
        .height(20.0)
        .radius(RadiusTokens::value(RadiusStep::Sm))
        .child(source_asset_glyph(
            AssetId::TABLER_X,
            14.0,
            theme.colors.text_muted.to_reference_paint(),
        ))
        .on_activate(move || action(ComposerAction::DismissQuestionnaire))
        .build()
        .with_key("composer-question-dismiss")
        .role(Role::Button)
        .aria_label("Dismiss questionnaire")
        .disabled(sending)
        .focusable(!sending);
    let header = UiBox::new()
        .gap(8.0)
        .style(Style::new().align(AlignItems::CENTER))
        .child(source_asset_glyph(
            AssetId::TABLER_MESSAGE_CIRCLE,
            14.0,
            theme.colors.text_muted.to_reference_paint(),
        ))
        .child(
            UiBox::new()
                .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                .child(label(
                    &theme,
                    question.header.clone().unwrap_or_else(|| "Question".into()),
                    12.0,
                    16.0,
                    theme.colors.text_muted.to_reference_text_color(),
                ))
                .build(),
        )
        .child(dismiss)
        .build();
    let text = match &question.text {
        artisan_api::TextContent::Inline(text) => paragraph(
            &theme,
            text.clone(),
            15.0,
            22.0,
            theme.colors.text.to_reference_text_color(),
        ),
        artisan_api::TextContent::Stored(source) => {
            let source = source.clone();
            let action = Rc::clone(&dispatch);
            Button::text("question-source", theme, "Read full question")
                .on_activate(move |_| action(ComposerAction::OpenQuestionSource(source.clone())))
                .build()
        }
    };
    let mut card = UiBox::new()
        .layout("column")
        .gap(8.0)
        .padding(4.0)
        .child(header)
        .child(
            UiBox::new()
                .style(Style::new().padding_insets(Insets {
                    left: 2.0,
                    right: 2.0,
                    top: 0.0,
                    bottom: 0.0,
                }))
                .child(text)
                .build(),
        );
    let mut rows = UiBox::new().layout("column").gap(2.0);
    if let Some(options) = stored_options {
        rows = rows.child(options);
    }
    if let artisan_api::Collection::Inline(options) = &question.options {
        for (index, option) in options.iter().enumerate() {
            let selected = panel.ticked.contains(&option.id);
            let marker = if question.multi_select {
                let mut mark = UiBox::new()
                    .width(16.0)
                    .height(16.0)
                    .radius(RadiusTokens::value(RadiusStep::Xs))
                    .border((1.0, theme.colors.line.to_paint()));
                if selected {
                    mark = mark.child(source_asset_glyph(
                        AssetId::TABLER_CHECK,
                        12.0,
                        theme.colors.text.to_reference_paint(),
                    ));
                }
                mark.build()
            } else {
                label(
                    &theme,
                    (index + 1).to_string(),
                    14.0,
                    20.0,
                    theme.colors.text_muted.to_reference_text_color(),
                )
            };
            let mut text = UiBox::new()
                .layout("column")
                .style(Style::new().min_width(Length::Px(0.0)))
                .child(label(
                    &theme,
                    option.label.clone(),
                    14.0,
                    20.0,
                    theme.colors.text.to_reference_text_color(),
                ));
            if let Some(description) = &option.description {
                let value = match description {
                    artisan_api::TextContent::Inline(text) => text.clone(),
                    artisan_api::TextContent::Stored(_) => {
                        "Full description available below".into()
                    }
                };
                text = text.child(label(
                    &theme,
                    value,
                    12.0,
                    16.0,
                    theme.colors.text_muted.to_reference_text_color(),
                ));
            }
            let item = active.item.clone();
            let question_id = question.id.clone();
            let id = option.id.clone();
            let multi = question.multi_select;
            let step = panel.step;
            let action = Rc::clone(&dispatch);
            let mut row = UiButton::new()
                .gap(10.0)
                .radius(RadiusTokens::value(RadiusStep::Lg))
                .style(
                    Style::new()
                        .align(AlignItems::FLEX_START)
                        .padding_insets(Insets {
                            left: 10.0,
                            right: 10.0,
                            top: 6.0,
                            bottom: 6.0,
                        }),
                )
                .child(marker)
                .child(text.build());
            if selected {
                row = row.fill(theme.colors.selected.to_paint());
            }
            let row = row
                .on_activate(move || {
                    action(ComposerAction::ChooseQuestionOption {
                        item: item.clone(),
                        question: question_id.clone(),
                        step,
                        id: id.clone(),
                        multi,
                    })
                })
                .build()
                .with_key(format!("composer-question-option-{index}"))
                .role(Role::Button)
                .aria_label(option.label.clone())
                .aria_selected(selected)
                .disabled(sending)
                .focusable(!sending);
            rows = rows.child(row);
            if let Some(artisan_api::TextContent::Stored(source)) = &option.description {
                let source = source.clone();
                let action = Rc::clone(&dispatch);
                rows = rows.child(
                    Button::text(
                        format!("question-description-{index}"),
                        theme,
                        "Read description",
                    )
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
            Button::text("composer-question-continue", theme, text)
                .on_activate(move |_| action(ComposerAction::ContinueQuestionnaire))
                .build(),
        );
    }
    card = card.child(rows.build());
    if sending {
        card = card.child(label(
            &theme,
            "Sending your answer…".into(),
            12.0,
            16.0,
            theme.colors.text_muted.to_reference_text_color(),
        ));
    } else if let Some(error) = &panel.error {
        let action = Rc::clone(&dispatch);
        card = card
            .child(label(
                &theme,
                error.clone(),
                14.0,
                20.0,
                theme.colors.destructive_text.to_reference_text_color(),
            ))
            .child(
                Button::text("composer-question-retry", theme, "Retry answer")
                    .on_activate(move |_| action(ComposerAction::RetryQuestionnaire))
                    .build(),
            );
    }
    let mut result = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .child(card.build());
    let mut stack = UiBox::new().layout("column").gap(2.0);
    for other in panel.visible().filter(|other| other.item != active.item) {
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
        let mut row = UiButton::new()
            .gap(8.0)
            .radius(RadiusTokens::value(RadiusStep::Md))
            .style(
                Style::new()
                    .align(AlignItems::CENTER)
                    .padding_insets(Insets {
                        left: 10.0,
                        right: 10.0,
                        top: 4.0,
                        bottom: 4.0,
                    }),
            )
            .child(source_asset_glyph(
                AssetId::TABLER_CHEVRON_RIGHT,
                14.0,
                theme.colors.text_muted.to_reference_paint(),
            ))
            .child(label(
                &theme,
                title.clone(),
                13.0,
                18.0,
                theme.colors.text_muted.to_reference_text_color(),
            ));
        if other.questions.len() > 1 {
            row = row.child(label(
                &theme,
                format!("{} questions", other.questions.len()),
                13.0,
                18.0,
                theme.colors.text_muted.to_reference_text_color(),
            ));
        }
        stack = stack.child(
            row.on_activate(move || action(ComposerAction::ActivateQuestionnaire(item.clone())))
                .build()
                .role(Role::Button)
                .aria_label(title)
                .disabled(sending)
                .focusable(!sending),
        );
    }
    result = result.child(stack.build());
    Some(result.build().with_key("composer-questionnaire"))
}

pub struct CardParts {
    pub prompt: Element,
    pub model: Element,
    pub primary: Element,
    pub questionnaires: Option<Element>,
    pub attachments: Option<Element>,
    pub mention_overlay: Option<Element>,
    pub thread_error: Option<Element>,
}

pub fn composer_card(theme: ArtisanTheme, nested: bool, parts: CardParts) -> Element {
    let mut card = UiBox::new().layout("column").width(Length::Pct(100.0));
    if let Some(error) = parts.thread_error {
        card = card.child(error);
    }
    if let Some(questionnaires) = parts.questionnaires {
        card = card.child(questionnaires);
    }
    if let Some(attachments) = parts.attachments {
        card = card.child(attachments);
    }
    card = card.child(parts.prompt);
    if let Some(mentions) = parts.mention_overlay {
        card = card.child(mentions);
    }
    card = card.child(controls(parts.model, parts.primary));
    frame(theme, nested, card.build()).with_key("composer")
}

pub fn draft_error(
    theme: ArtisanTheme,
    error: &str,
    conflict: bool,
    dispatch: Dispatch,
) -> Element {
    use aqua_core::Insets;
    let text = UiBox::new()
        .style(Style::new().padding_insets(Insets {
            left: 12.0,
            right: 12.0,
            top: 0.0,
            bottom: 0.0,
        }))
        .child(label(
            &theme,
            error.to_owned(),
            13.0,
            16.25,
            theme.colors.destructive_text.to_reference_text_color(),
        ))
        .build();
    let mut root = UiBox::new().layout("column").gap(8.0).child(text);
    if conflict {
        let local = Rc::clone(&dispatch);
        let remote = Rc::clone(&dispatch);
        root = root.child(
            UiBox::new()
                .gap(8.0)
                .child(
                    Button::text("draft-keep-local", theme, "Keep my draft")
                        .on_activate(move |_| local(ComposerAction::KeepDraft))
                        .build(),
                )
                .child(
                    Button::text("draft-use-remote", theme, "Load other draft")
                        .on_activate(move |_| remote(ComposerAction::LoadOtherDraft))
                        .build(),
                )
                .build(),
        );
    } else {
        root = root.child(
            Button::text("draft-retry", theme, "Retry")
                .on_activate(move |_| dispatch(ComposerAction::RetryDraft))
                .build(),
        );
    }
    root.build()
}

pub fn goal_label(
    theme: ArtisanTheme,
    frame: &super::goal_strip::GoalStripFrame,
    ticker: Element,
    dispatch: Dispatch,
) -> Element {
    use super::goal_strip::ComposerGoalState as State;
    use crate::ui::{asset::source_asset_glyph, glass::tinted_card_shadows};
    use aqua_core::{Insets, Transform2D};
    use aqua_ui::{AlignItems, element::Button as UiButton};
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
    let mut style = Style::new()
        .align(AlignItems::CENTER)
        .padding_insets(Insets {
            left: 6.0,
            right: 12.0,
            top: 0.0,
            bottom: 0.0,
        })
        .opacity(frame.label_opacity)
        .transform(Transform2D::translate(0.0, frame.label_offset));
    let blur = (frame.label_offset.abs() / 2.0).min(2.0);
    if blur > 0.0 {
        style = style.content_blur(blur);
    }
    let mut row = UiBox::new()
        .height(Length::Pct(100.0))
        .gap(6.0)
        .style(style)
        .child(source_asset_glyph(icon, 15.0, tone.to_reference_paint()));
    if note {
        row = row.child(
            UiBox::new()
                .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                .child(ticker)
                .build(),
        );
    } else {
        let headline = if safety {
            tone.to_reference_text_color()
        } else {
            theme.colors.text.to_reference_text_color()
        };
        row = row.child(
            UiBox::new()
                .style(Style::new().min_width(Length::Px(0.0)))
                .child(label(&theme, goal.objective.clone(), 13.0, 18.0, headline))
                .build(),
        );
        if matches!(
            goal.state,
            State::Active | State::SafetyHolding | State::SafetyRerouted | State::SafetyRefused
        ) && let Some(detail) = &goal.detail
        {
            row = row.child(label(
                &theme,
                detail.clone(),
                13.0,
                18.0,
                theme.colors.text_muted.to_reference_text_color(),
            ));
        }
        let pill = match goal.state {
            State::Paused => Some(("Paused", theme.colors.banner_info)),
            State::Blocked => Some(("Blocked", theme.colors.banner_warning)),
            State::UsageLimited => Some(("Usage limit reached", theme.colors.banner_warning)),
            State::BudgetLimited => Some(("Budget spent", theme.colors.banner_warning)),
            _ => None,
        };
        if let Some((text, tone)) = pill {
            row = row.child(
                UiBox::new()
                    .height(18.0)
                    .radius(5.0)
                    .fill(tone.bg.to_paint())
                    .style(
                        Style::new()
                            .align(AlignItems::CENTER)
                            .padding_insets(Insets {
                                left: 6.0,
                                right: 6.0,
                                top: 0.0,
                                bottom: 0.0,
                            })
                            .shadows(
                                tinted_card_shadows(&theme, tone.text)
                                    .into_iter()
                                    .map(Into::into)
                                    .collect(),
                            ),
                    )
                    .child(label(
                        &theme,
                        text.into(),
                        12.0,
                        16.0,
                        tone.text.to_reference_text_color(),
                    ))
                    .build(),
            );
        }
        row = row.child(UiBox::new().style(Style::new().flex_grow(1.0)).build());
    }
    if !safety {
        let (id, action) = if note {
            ("composer-note-close", ComposerAction::CloseNote)
        } else if goal.state == State::Parked {
            ("composer-park-cancel", ComposerAction::Unpark)
        } else {
            ("composer-goal-clear", ComposerAction::ClearGoal)
        };
        row = row.child(
            UiButton::new()
                .width(15.0)
                .height(15.0)
                .child(source_asset_glyph(
                    AssetId::TABLER_X,
                    15.0,
                    theme.colors.text_muted.to_reference_paint(),
                ))
                .on_activate(move || dispatch(action.clone()))
                .build()
                .with_key(id)
                .focusable(true),
        );
    }
    row.build()
}

pub struct ComposerOwner {
    pub controller: Rc<std::cell::RefCell<super::controller::Composer>>,
    pub editor: crate::ui::text::mount::EditorController,
    dispatch: Dispatch,
    answers: crate::core::delivery::Delivery<AnswerCompletion>,
    reduced_motion: bool,
    model_popup: Option<crate::ui::popover::AnimatedPickerHost>,
    mention_popup: Option<crate::ui::popover::AnimatedPickerHost>,
    project_popup: Option<crate::ui::popover::PickerPopupHost>,
    axis_popup: Option<crate::ui::popover::PickerPopupHost>,
    axis_animated: Option<crate::ui::popover::AnimatedPickerHost>,
    engine_light: std::cell::RefCell<super::engine_light::EngineLight>,
    engine_light_image: std::cell::RefCell<super::engine_light::EngineLightImage>,
    tab_geometry: Rc<std::cell::RefCell<crate::ui::tab_card::GeometryCache>>,
    strip_moving: std::cell::Cell<bool>,
    note_ticker: std::cell::RefCell<crate::ui::marquee::Marquee>,
    model_hover: Rc<std::cell::RefCell<crate::ui::picker_motion::HoverSurface>>,
    project_hover: Rc<std::cell::RefCell<crate::ui::picker_motion::HoverSurface>>,
    axis_hover: Rc<std::cell::RefCell<crate::ui::picker_motion::HoverSurface>>,
    last_visible: std::cell::RefCell<ComposerVisible>,
    tooltip: std::cell::RefCell<super::picker::PickerTooltip>,
}

struct AnswerCompletion {
    host: crate::core::HostId,
    instance: crate::core::HostInstance,
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
    pub placement: crate::ui::hover_tooltip::TooltipPlacement,
    pub surface: Rc<dyn Fn(f32, bool) -> Element>,
}

fn picker_tooltip_surface(
    theme: &ArtisanTheme,
    tooltip: &super::picker::TooltipContent,
    width: f32,
    fixed: bool,
) -> Element {
    use crate::ui::{
        glass::{GlassStrength, glass_card_shadows, glass_layers},
        theme::Elevation,
    };
    use aqua_text::{FontFamily, FontWeight, StyledSpan, TextStyle};
    use aqua_ui::element::Text;
    let style = TextStyle {
        families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
        size_px: 12.0,
        line_height_px: 16.0,
        weight: FontWeight::NORMAL,
        ..TextStyle::default()
    };
    let mut spans = Vec::new();
    if let Some(advisory) = tooltip.advisory.as_ref().filter(|text| !text.is_empty()) {
        let color = if advisory == "Additional info:" {
            theme.colors.banner_info.text
        } else {
            theme.colors.destructive_text
        }
        .to_reference_text_color();
        let mut highlighted = style.clone();
        highlighted.weight = FontWeight(500);
        spans.push(
            StyledSpan::new(advisory.clone(), highlighted).gpui_source_color([
                color.hue_degrees,
                color.saturation,
                color.lightness,
                color.alpha,
            ]),
        );
        if !tooltip.text.is_empty() {
            spans.push(StyledSpan::new(" ", style.clone()));
        }
    }
    if !tooltip.text.is_empty() {
        spans.push(StyledSpan::new(tooltip.text.clone(), style.clone()));
    }
    let radius = RadiusTokens::value(RadiusStep::X2l);
    let mut layout = Style::new()
        .max_width(Length::Px(width))
        .padding_insets(aqua_core::Insets {
            left: 12.0,
            right: 12.0,
            top: 8.0,
            bottom: 8.0,
        })
        .gpui_shadows(
            glass_card_shadows(theme, Elevation::Overlay)
                .into_iter()
                .map(Into::into)
                .collect(),
        );
    if fixed {
        layout = layout.width(Length::Px(width));
    }
    UiBox::new()
        .radius(radius)
        .clip(true)
        .style(layout)
        .child(glass_layers(theme, GlassStrength::Strong, radius))
        .child(
            Text::new("")
                .text_style(style)
                .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                .spans(spans)
                .wrap(true)
                .build(),
        )
        .build()
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
    tokens: Vec<crate::ui::text::editor::StyledToken>,
}

impl EditorVisible {
    fn capture(editor: &crate::ui::text::editor::Editor) -> Self {
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
        Self {
            picker: composer.picker.clone(),
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
        core: &crate::core::Core,
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
            answers: crate::core::delivery::Delivery::new(std::num::NonZeroUsize::new(16).unwrap()),
            reduced_motion: false,
            model_popup: None,
            mention_popup: None,
            project_popup: None,
            axis_popup: None,
            axis_animated: None,
            engine_light: std::cell::RefCell::new(super::engine_light::EngineLight::default()),
            engine_light_image: std::cell::RefCell::new(
                super::engine_light::EngineLightImage::default(),
            ),
            tab_geometry: Rc::new(std::cell::RefCell::new(
                crate::ui::tab_card::GeometryCache::default(),
            )),
            strip_moving: std::cell::Cell::new(false),
            note_ticker: std::cell::RefCell::new(crate::ui::marquee::Marquee::new(
                "composer-note-ticker",
                "",
            )),
            model_hover: Rc::new(std::cell::RefCell::new(
                crate::ui::picker_motion::HoverSurface::new("composer-model-hover"),
            )),
            project_hover: Rc::new(std::cell::RefCell::new(
                crate::ui::picker_motion::HoverSurface::new("composer-project-hover"),
            )),
            axis_hover: Rc::new(std::cell::RefCell::new(
                crate::ui::picker_motion::HoverSurface::new("composer-axis-hover"),
            )),
            last_visible: std::cell::RefCell::new(last_visible),
            tooltip: std::cell::RefCell::new(super::picker::PickerTooltip::default()),
        }
    }

    pub fn set_target(
        &self,
        core: &crate::core::Core,
        sessions: &mut super::persistence::DraftSessions,
        target: super::controller::ComposerTarget,
    ) {
        let changed = {
            let controller = self.controller.borrow();
            controller.target != target
                || core
                    .host(target.host())
                    .map(crate::core::HostData::instance)
                    != controller.host_instance()
        };
        if changed {
            self.tooltip.borrow_mut().clear();
            *self.engine_light.borrow_mut() = super::engine_light::EngineLight::default();
            *self.tab_geometry.borrow_mut() = crate::ui::tab_card::GeometryCache::default();
            self.strip_moving.set(false);
            *self.note_ticker.borrow_mut() =
                crate::ui::marquee::Marquee::new("composer-note-ticker", "");
        }
        self.controller
            .borrow_mut()
            .set_target(core, sessions, target);
        *self.last_visible.borrow_mut() = ComposerVisible::capture(&self.controller.borrow());
    }

    pub fn refresh_mentions(&self, host: &crate::core::HostData, theme: &ArtisanTheme) {
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

    pub fn project_hover(&self) -> Rc<std::cell::RefCell<crate::ui::picker_motion::HoverSurface>> {
        Rc::clone(&self.project_hover)
    }

    pub fn axis_hover(&self) -> Rc<std::cell::RefCell<crate::ui::picker_motion::HoverSurface>> {
        Rc::clone(&self.axis_hover)
    }

    pub fn tooltip_target(&self) -> Option<(crate::core::HostInstance, u64)> {
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
                crate::ui::hover_tooltip::TooltipPlacement::Above { max_width: 240.0 }
            }
            TooltipPlacement::Beside => {
                crate::ui::hover_tooltip::TooltipPlacement::Beside { max_width: 320.0 }
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

    pub fn adopt_axis_popup(
        &mut self,
        app: &mut aqua::UiApp,
        popup: crate::ui::popover::PickerPopupHost,
    ) {
        if let Some(old) = self.axis_popup.replace(popup) {
            old.close(app);
        }
    }

    pub fn dispose_popups(&mut self, app: &mut aqua::UiApp) {
        self.tooltip.borrow_mut().clear();
        self.model_hover.borrow_mut().dispose(app);
        self.project_hover.borrow_mut().dispose(app);
        self.axis_hover.borrow_mut().dispose(app);
        if let Some(popup) = self.axis_popup.take() {
            popup.close(app);
        }
        if let Some(popup) = self.axis_animated.take() {
            popup.dispose(app);
        }
        if let Some(popup) = self.mention_popup.take() {
            popup.dispose(app);
        }
        if let Some(popup) = self.model_popup.take() {
            popup.dispose(app);
        }
        if let Some(popup) = self.project_popup.take() {
            popup.close(app);
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
        use crate::ui::popover::{AnimatedPickerHost, PickerEntrance};
        use aqua_ui::popup::PopupFocus;
        let (entrance, open_ms, close_ms) = match kind {
            PopupKind::Model | PopupKind::Axis => (PickerEntrance::Slide, 100, 100),
            PopupKind::Mentions => (
                PickerEntrance::Scale {
                    origin: aqua_core::Point::new(0.5, 1.0),
                },
                250,
                150,
            ),
            PopupKind::Project => return Err("project menu is not animated".into()),
        };
        let action = Rc::clone(&self.dispatch);
        let popup = AnimatedPickerHost::open_above(
            app,
            anchor,
            factory,
            kind.gap(),
            if kind == PopupKind::Axis {
                PopupFocus::Root
            } else {
                PopupFocus::Preserve
            },
            entrance,
            std::time::Duration::from_millis(if self.reduced_motion { 0 } else { open_ms }),
            std::time::Duration::from_millis(if self.reduced_motion { 0 } else { close_ms }),
            now,
            move |_| {
                action(match kind {
                    PopupKind::Mentions => ComposerAction::DismissMentions,
                    PopupKind::Axis => ComposerAction::CloseAxis,
                    _ => ComposerAction::ClosePicker,
                })
            },
        )?;
        match kind {
            PopupKind::Model => self.model_popup = Some(popup),
            PopupKind::Mentions => self.mention_popup = Some(popup),
            PopupKind::Axis => self.axis_animated = Some(popup),
            PopupKind::Project => {}
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
        use crate::ui::popover::{AnimatedPickerHost, PickerEntrance};
        use aqua_ui::popup::PopupFocus;
        if kind == PopupKind::Project {
            if self.popup_is_open(kind, app) {
                return self.refresh_popup_content(kind, app, initial, now);
            }
            return self.open_project_popup(app, anchor, initial);
        }
        if self.popup_is_open(kind, app) {
            return self.refresh_popup_content(kind, app, initial, now);
        }
        let (entrance, open_ms, close_ms) = match kind {
            PopupKind::Model | PopupKind::Axis => (PickerEntrance::Slide, 100, 100),
            PopupKind::Mentions => (
                PickerEntrance::Scale {
                    origin: aqua_core::Point::new(0.5, 1.0),
                },
                250,
                150,
            ),
            PopupKind::Project => unreachable!(),
        };
        let action = Rc::clone(&self.dispatch);
        let popup = AnimatedPickerHost::open_content(
            app,
            anchor,
            initial,
            kind.gap(),
            if kind == PopupKind::Axis {
                PopupFocus::Root
            } else {
                PopupFocus::Preserve
            },
            entrance,
            std::time::Duration::from_millis(if self.reduced_motion { 0 } else { open_ms }),
            std::time::Duration::from_millis(if self.reduced_motion { 0 } else { close_ms }),
            now,
            move |_| {
                action(match kind {
                    PopupKind::Mentions => ComposerAction::DismissMentions,
                    PopupKind::Axis => ComposerAction::CloseAxis,
                    _ => ComposerAction::ClosePicker,
                })
            },
        )?;
        let slot = match kind {
            PopupKind::Model => &mut self.model_popup,
            PopupKind::Mentions => &mut self.mention_popup,
            PopupKind::Axis => &mut self.axis_animated,
            PopupKind::Project => unreachable!(),
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
            PopupKind::Axis => self
                .axis_animated
                .as_ref()
                .and_then(|popup| popup.root(app))
                .or_else(|| self.axis_popup.as_ref().and_then(|popup| popup.root(app))),
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
            PopupKind::Axis => {
                if let Some(popup) = self
                    .axis_animated
                    .as_mut()
                    .filter(|popup| popup.is_open(app))
                {
                    if popup.is_closing() {
                        popup.begin_open(app, now)?;
                    }
                    popup.refresh_content(app, content)
                } else if let Some(popup) =
                    self.axis_popup.as_ref().filter(|popup| popup.is_open(app))
                {
                    popup.refresh(app, content)
                } else {
                    Ok(())
                }
            }
            PopupKind::Project => self
                .project_popup
                .as_ref()
                .filter(|popup| popup.is_open(app))
                .map_or(Ok(()), |popup| popup.refresh(app, content)),
        }
    }

    pub fn open_project_popup(
        &mut self,
        app: &mut aqua::UiApp,
        anchor: aqua_core::NodeId,
        content: Element,
    ) -> Result<(), String> {
        self.project_popup = Some(open_popup(
            app,
            anchor,
            content,
            PopupKind::Project,
            Rc::clone(&self.dispatch),
        )?);
        Ok(())
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
            PopupKind::Axis => {
                if let Some(popup) = &mut self.axis_animated {
                    popup.begin_close(app, now)?;
                }
                if let Some(popup) = self.axis_popup.take() {
                    popup.close(app);
                }
            }
            PopupKind::Project => {
                if let Some(popup) = self.project_popup.take() {
                    popup.close(app);
                }
            }
        }
        Ok(())
    }

    pub fn needs_animation(&self, app: &aqua::UiApp) -> bool {
        self.strip_moving.get()
            || self.note_ticker.borrow().is_animating()
            || self.model_hover.borrow().is_animating()
            || self.project_hover.borrow().is_animating()
            || self.axis_hover.borrow().is_animating()
            || self
                .axis_animated
                .as_ref()
                .is_some_and(|popup| popup.is_animating(app))
            || (self.controller.borrow().picker.menu == Some(super::picker::Menu::Model)
                && self.engine_light.borrow().animating())
            || self
                .model_popup
                .as_ref()
                .is_some_and(|popup| popup.is_animating(app))
            || self
                .mention_popup
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
        if controller.picker.menu != Some(super::picker::Menu::Project) {
            return false;
        }
        crate::ui::scroll_edge::EdgeSurface {
            edges: &controller.picker.project_edges,
            scroller: PROJECT_SCROLL,
            top: PROJECT_EDGE_TOP,
            bottom: PROJECT_EDGE_BOTTOM,
            top_color: theme.colors.gray.step(7),
            bottom_color: theme.colors.gray.step(4),
        }
        .advance(app, reduced, now)
    }

    pub fn advance_picker_hover(
        &self,
        app: &mut aqua::UiApp,
        theme: &ArtisanTheme,
        now: f64,
        roots: [Option<aqua_core::NodeId>; 3],
    ) -> Result<bool, String> {
        let mut changed = false;
        for (index, (root, hover)) in roots
            .into_iter()
            .zip([&self.model_hover, &self.project_hover, &self.axis_hover])
            .enumerate()
        {
            let surface = root.and_then(|root| {
                if index != 0 {
                    return Some(root);
                }
                let key =
                    aqua_ui::tree::hash_key(aqua_ui::element::Key::from("composer-model-viewport"));
                let mut stack = vec![root];
                while let Some(node) = stack.pop() {
                    if let Some(record) = app.tree().get(node) {
                        if record.key == Some(key) {
                            return Some(node);
                        }
                        stack.extend(record.children.iter().copied());
                    }
                }
                None
            });
            if let Some(surface) = surface {
                let frame = hover.borrow_mut().advance_frame(
                    app,
                    surface,
                    theme,
                    false,
                    now,
                    self.reduced_motion,
                )?;
                changed |= frame.changed || frame.animating;
            } else {
                hover.borrow_mut().dispose(app);
            }
        }
        Ok(changed)
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
                crate::ui::marquee::Marquee::new("composer-note-ticker", "");
        }
    }

    pub fn advance_popups(&mut self, app: &mut aqua::UiApp, now: f64) -> Result<(), String> {
        for popup in [
            &mut self.axis_animated,
            &mut self.model_popup,
            &mut self.mention_popup,
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
        for popup in [
            &mut self.axis_animated,
            &mut self.model_popup,
            &mut self.mention_popup,
        ] {
            if let Some(popup) = popup {
                if popup.is_open(app) && popup.request_dismissal(app, event)? {
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
        host: &crate::core::HostData,
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
        crate::core::runtime::runtime().spawn(async move {
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

    pub fn drain_answers(&mut self, core: &crate::core::Core) -> Vec<ComposerEffect> {
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
        core: &crate::core::Core,
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
        core: &mut crate::core::Core,
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
                crate::ui::text::mount::EditorEvent::Changed
                    | crate::ui::text::mount::EditorEvent::SelectionChanged { .. }
                    | crate::ui::text::mount::EditorEvent::PresentationChanged
            )
        );
        let effects = self.dispatch(core, action, question);
        let after = ComposerVisible::capture(&self.controller.borrow());
        let changed = match typed {
            true => {
                !self.last_visible.borrow().same_chrome(&after) || !before.same_chrome(&after)
            }
            false => before != after || *self.last_visible.borrow() != after,
        } || tooltip_before != self.tooltip.borrow().revision();
        *self.last_visible.borrow_mut() = after;
        (effects, changed)
    }

    pub fn dispatch(
        &mut self,
        core: &mut crate::core::Core,
        action: ComposerAction,
        question: Option<&artisan_api::Question>,
    ) -> Vec<ComposerEffect> {
        use crate::ui::text::mount::EditorEvent;
        let mut effects = Vec::new();
        let mut composer = self.controller.borrow_mut();
        let Some(host) = core.host(composer.target.host()) else {
            return effects;
        };
        if !composer.matches_host(host) {
            return effects;
        }
        match action {
            ComposerAction::TooltipHover {
                instance,
                generation,
                anchor,
                content,
                hovered,
                axis,
            } => {
                if composer.host_instance() == Some(instance)
                    && composer.generation() == generation
                    && composer.picker.menu == Some(super::picker::Menu::Model)
                    && composer.picker.axis == axis
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
                composer.picker.preview_model = None;
            }
            ComposerAction::CloseAxis => {
                self.tooltip.borrow_mut().clear();
                composer.picker.axis = None;
                composer.picker.axis_cursor = None;
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
                core.navigate(crate::core::Screen::Settings(
                    crate::core::SettingsPage::Engine(engine),
                ));
            }
            ComposerAction::BrowseEngine(engine) => {
                self.tooltip.borrow_mut().clear();
                let current = composer.effective_config(host);
                let mut changed = None;
                if current
                    .as_ref()
                    .is_none_or(|config| config.engine != engine)
                {
                    if let Some(config) = super::controller::remembered_harness_config(host, engine)
                    {
                        if composer.metadata_ready(host) && composer.draft_editable() {
                            composer.apply_permissions(host, config.permissions);
                            composer.config = Some(config.clone());
                            composer.edited();
                            changed = Some(config);
                        }
                    }
                }
                composer.picker.browsing = Some(engine);
                composer.picker.axis = None;
                composer.refresh_picker(host);
                if let Some(config) = changed {
                    let host_id = host.id.clone();
                    let instance = host.instance();
                    if let Some(host) = core.host_mut_if_instance(&host_id, instance) {
                        host.remember_model_config(config);
                    }
                }
            }
            ComposerAction::PreviewModel { engine, id } => {
                if composer.picker.menu == Some(super::picker::Menu::Model)
                    && composer.picker.axis.is_none()
                {
                    composer.picker.preview_model = Some((engine, id));
                }
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
            ComposerAction::ToggleAxis(axis) => {
                self.tooltip.borrow_mut().clear();
                composer.picker.axis_cursor = None;
                composer.picker.axis = if composer.picker.axis == Some(axis) {
                    None
                } else {
                    Some(axis)
                };
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
                        .preview_config(host, composer.effective_config(host))
                    {
                        let model = host.engines.get(&preview.engine).and_then(|status| {
                            preview
                                .model
                                .as_ref()
                                .and_then(|id| status.models.iter().find(|model| &model.id == id))
                        });
                        let options = super::picker::axis_options(axis, &preview, model);
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
                                        composer.picker.axis = None;
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
                    .preview_config(host, composer.effective_config(host))
                {
                    let model = host.engines.get(&preview.engine).and_then(|status| {
                        preview
                            .model
                            .as_ref()
                            .and_then(|id| status.models.iter().find(|model| &model.id == id))
                    });
                    if let Some(option) = super::picker::axis_options(axis, &preview, model)
                        .into_iter()
                        .nth(index)
                    {
                        self.tooltip.borrow_mut().clear();
                        let config = option.apply;
                        composer.apply_permissions(host, config.permissions);
                        composer.config = Some(config.clone());
                        composer.edited();
                        composer.picker.axis = None;
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
        host: &crate::core::HostData,
        viewport: aqua_core::Size,
        assets: &aqua::images::ImageAssets,
        now: std::time::Instant,
    ) -> Result<Element, aqua::images::ImageAssetError> {
        let controller = self.controller.borrow();
        let current = controller.effective_config(host);
        let engines: Vec<_> = host
            .engines
            .values()
            .filter(|status| {
                gate(status).is_none()
                    || current
                        .as_ref()
                        .is_some_and(|config| config.engine == status.engine)
            })
            .collect();
        let browsing = controller
            .picker
            .browsing
            .filter(|engine| engines.iter().any(|status| status.engine == *engine))
            .or_else(|| current.as_ref().map(|config| config.engine))
            .or_else(|| engines.first().map(|status| status.engine));
        let light = if let Some((index, engine)) = browsing.and_then(|engine| {
            engines
                .iter()
                .position(|status| status.engine == engine)
                .map(|index| (index, engine))
        }) {
            let mut light = self.engine_light.borrow_mut();
            light.measure(engine, 4.0 + index as f32 * 36.0, 32.0, now);
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
        let axis = axis_controls(
            theme,
            host,
            &controller.picker,
            current.clone(),
            Rc::clone(&self.dispatch),
        );
        Ok(model_menu(
            theme,
            host,
            &controller.picker,
            current,
            viewport,
            light,
            axis,
            Rc::clone(&self.model_hover),
            (host.instance(), controller.generation()),
            Rc::clone(&self.dispatch),
        ))
    }

    pub fn surface(
        &self,
        theme: ArtisanTheme,
        host: &crate::core::HostData,
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
            let tab = project_tab(theme, &controller, host, Rc::clone(&self.dispatch));
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
                                .style(
                                    Style::new()
                                        .gpui_source_text_color(
                                            theme.colors.text.to_reference_text_color(),
                                        )
                                        .text_style(aqua_text::TextStyle {
                                            families: vec![aqua_text::FontFamily::Named(
                                                theme.typography.sans.family.to_owned(),
                                            )],
                                            size_px: 13.0,
                                            line_height_px: 18.0,
                                            weight: aqua_text::FontWeight::NORMAL,
                                            ..aqua_text::TextStyle::default()
                                        }),
                                )
                                .child(ticker.build(18.0))
                                .build(),
                            Rc::clone(&self.dispatch),
                        );
                        Some(crate::ui::tab_card::TabCardStrip {
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
            Some(tab) => crate::ui::tab_card::TabCard {
                id: "composer-tab-card".into(),
                tab,
                body: card,
                strip,
            }
            .build(theme, Rc::clone(&self.tab_geometry)),
            None => card,
        }
    }

    pub fn card(
        &self,
        theme: ArtisanTheme,
        host: &crate::core::HostData,
        nested: bool,
        placeholder: &str,
        questionnaires: Option<Element>,
        attachments: Option<Element>,
        mention_overlay: Option<Element>,
        thread_error: Option<Element>,
    ) -> Element {
        let controller = self.controller.borrow();
        let config = controller.effective_config(host);
        let model = config.as_ref().and_then(|config| {
            host.engines.get(&config.engine).and_then(|status| {
                status
                    .models
                    .iter()
                    .find(|model| Some(&model.id) == config.model.as_ref())
            })
        });
        let (face, disabled) = controller.primary(host);
        let instance = controller.host_instance();
        let generation = controller.generation();
        let host_id = controller.target.host().clone();
        let dispatch = Rc::clone(&self.dispatch);
        composer_card(
            theme,
            nested,
            CardParts {
                prompt: self.editor.element_with_notice_keyed(
                    "composer-prompt",
                    crate::ui::text::Mode::Prompt,
                    &theme,
                    placeholder,
                    false,
                    false,
                ),
                model: model_trigger(theme, config.as_ref(), model, Rc::clone(&self.dispatch)),
                primary: primary(theme, face, disabled, Rc::clone(&self.dispatch)),
                questionnaires,
                attachments,
                mention_overlay,
                thread_error,
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
                            if payload.get::<crate::core::DraggedThread>().is_some_and(|thread| {
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
                            match payload.get::<crate::core::DraggedThread>() {
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
    use crate::ui::attachment_tile::AttachmentTile;
    use aqua_core::Insets;
    use aqua_ui::{AlignItems, element::Scroll};
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
    let mut row =
        UiBox::new()
            .gap(8.0)
            .style(
                Style::new()
                    .align(AlignItems::CENTER)
                    .padding_insets(Insets {
                        left: 4.0,
                        right: 4.0,
                        top: 4.0,
                        bottom: 8.0,
                    }),
            );
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
    Axis,
}
impl PopupKind {
    pub const fn gap(self) -> f32 {
        match self {
            Self::Project => 10.0,
            Self::Model | Self::Mentions => 8.0,
            Self::Axis => 4.0,
        }
    }
}

pub fn open_popup(
    app: &mut aqua::UiApp,
    owner: aqua_core::NodeId,
    content: Element,
    kind: PopupKind,
    dispatch: Dispatch,
) -> Result<crate::ui::popover::PickerPopupHost, String> {
    use aqua_ui::{overlay::DismissPolicy, popup::PopupFocus};
    crate::ui::popover::PickerPopupHost::open_above(
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
                PopupKind::Axis => ComposerAction::CloseAxis,
            });
        },
    )
}

fn project_mark(
    theme: &ArtisanTheme,
    host: &crate::core::HostData,
    project: &artisan_api::ProjectId,
) -> Element {
    use aqua_core::image::ImageFit;
    use aqua_ui::element::Media;
    if let Some(image) = host.project_icon(project) {
        return Media::new(image.as_ref().clone())
            .width(16.0)
            .height(16.0)
            .fit(ImageFit::Contain)
            .radius(RadiusTokens::value(RadiusStep::X2sMid))
            .build();
    }
    let icon = match host.repository(project) {
        None => AssetId::TABLER_FOLDER,
        Some(repository) => match repository.host.as_deref() {
            Some("github") => AssetId::SVGL_GITHUB,
            Some("gitlab") => AssetId::SVGL_GITLAB,
            Some("azure") => AssetId::SVGL_MICROSOFT_AZURE,
            Some("bitbucket") => AssetId::SIMPLE_ICONS_BITBUCKET,
            Some("codeberg") => AssetId::SIMPLE_ICONS_CODEBERG,
            Some("gitea") => AssetId::SIMPLE_ICONS_GITEA,
            Some("sourcehut") => AssetId::SIMPLE_ICONS_SOURCEHUT,
            _ => AssetId::SVGL_GIT,
        },
    };
    crate::ui::asset::source_asset_glyph(icon, 16.0, theme.colors.text_muted.to_reference_paint())
}

pub fn project_menu(
    theme: ArtisanTheme,
    host: &crate::core::HostData,
    picker: &super::picker::PickerState,
    selected: Option<&artisan_api::ProjectId>,
    viewport: aqua_core::Size,
    hover_surface: Rc<std::cell::RefCell<crate::ui::picker_motion::HoverSurface>>,
    dispatch: Dispatch,
) -> Element {
    use crate::ui::{
        asset::source_asset_glyph,
        glass::{GlassStrength, glass_card_shadows, glass_layers, glass_menu_separator},
        theme::Elevation,
    };
    use aqua_core::{Insets, Role};
    use aqua_ui::{
        AlignItems,
        element::{Button as UiButton, Scroll},
    };
    let mut rows = UiBox::new().layout("column");
    for id in &picker.projects {
        let Some(project) = host.projects.get(id) else {
            continue;
        };
        let chosen = selected == Some(id);
        let action = Rc::clone(&dispatch);
        let id = id.clone();
        let row_key = format!("composer-project-{id}");
        let hover = Rc::clone(&hover_surface);
        let mut row = UiButton::new()
            .width(Length::Pct(100.0))
            .height(36.0)
            .gap(10.0)
            .radius(RadiusTokens::value(RadiusStep::Sm))
            .style(
                Style::new()
                    .align(AlignItems::CENTER)
                    .padding_insets(Insets {
                        left: 8.0,
                        right: 8.0,
                        top: 6.0,
                        bottom: 6.0,
                    }),
            )
            .child(project_mark(&theme, host, &id))
            .child(
                UiBox::new()
                    .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                    .child(label(
                        &theme,
                        project.name.clone(),
                        14.0,
                        20.0,
                        theme.colors.text.to_reference_text_color(),
                    ))
                    .build(),
            );
        if chosen {
            row = row.child(label(
                &theme,
                "✓".into(),
                16.0,
                20.0,
                theme.colors.text_muted.to_reference_text_color(),
            ));
        }
        rows = rows.child(
            row.on_activate(move || action(ComposerAction::ChooseProject(id.clone())))
                .build()
                .with_key(row_key.clone())
                .on_hover_change(move |hovered| {
                    if hovered {
                        hover.borrow().hovered(
                            &row_key,
                            true,
                            crate::ui::hover::HoverTone::Neutral,
                        );
                    }
                })
                .role(Role::MenuItem)
                .aria_label(project.name.clone())
                .aria_selected(chosen)
                .focusable(true),
        );
    }
    let count = picker.projects.len();
    let mut list = UiBox::new().layout("column").padding(8.0);
    if count > 0 {
        list = list
            .child(
                UiBox::new()
                    .width(Length::Pct(100.0))
                    .clip(true)
                    .child(
                        Scroll::new()
                            .height((count.min(8) as f32) * 36.0)
                            .child(rows.build())
                            .build()
                            .with_key(PROJECT_SCROLL),
                    )
                    .child(picker.project_edges.band(
                        PROJECT_EDGE_TOP,
                        crate::ui::scroll_edge::Edge::Top,
                        theme.colors.gray.step(7),
                    ))
                    .child(picker.project_edges.band(
                        PROJECT_EDGE_BOTTOM,
                        crate::ui::scroll_edge::Edge::Bottom,
                        theme.colors.gray.step(4),
                    ))
                    .build(),
            )
            .child(glass_menu_separator(&theme));
    }
    let scratch = artisan_api::ProjectId::scratch();
    let intake = |label_text: &'static str, glyph, selected: bool, action: ComposerAction| {
        let action_dispatch = Rc::clone(&dispatch);
        let hover = Rc::clone(&hover_surface);
        let row_key = format!("composer-project-{label_text}");
        let mut row = UiButton::new()
            .width(Length::Pct(100.0))
            .height(36.0)
            .gap(10.0)
            .radius(RadiusTokens::value(RadiusStep::Sm))
            .style(
                Style::new()
                    .align(AlignItems::CENTER)
                    .padding_insets(Insets {
                        left: 8.0,
                        right: 8.0,
                        top: 6.0,
                        bottom: 6.0,
                    }),
            )
            .child(source_asset_glyph(
                glyph,
                16.0,
                theme.colors.text_muted.to_reference_paint(),
            ))
            .child(
                UiBox::new()
                    .style(Style::new().flex_grow(1.0))
                    .child(label(
                        &theme,
                        label_text.into(),
                        14.0,
                        20.0,
                        theme.colors.text.to_reference_text_color(),
                    ))
                    .build(),
            );
        if selected {
            row = row.child(label(
                &theme,
                "✓".into(),
                16.0,
                20.0,
                theme.colors.text_muted.to_reference_text_color(),
            ));
        }
        row.on_activate(move || action_dispatch(action.clone()))
            .build()
            .with_key(row_key.clone())
            .on_hover_change(move |hovered| {
                if hovered {
                    hover
                        .borrow()
                        .hovered(&row_key, true, crate::ui::hover::HoverTone::Neutral);
                }
            })
            .role(Role::MenuItem)
            .aria_label(label_text)
            .aria_selected(selected)
            .focusable(true)
    };
    if host
        .projects
        .get(&scratch)
        .is_some_and(|project| !project.removed)
    {
        list = list.child(intake(
            "No project",
            AssetId::TABLER_MESSAGE_CIRCLE,
            selected == Some(&scratch),
            ComposerAction::ChooseProject(scratch),
        ));
    }
    list = list.child(intake(
        "New project",
        AssetId::TABLER_FOLDER_PLUS,
        false,
        ComposerAction::NewProject,
    ));
    let radius = RadiusTokens::value(RadiusStep::Lg);
    UiBox::new()
        .layout("column")
        .width(320.0_f32.min(viewport.w - 32.0))
        .radius(radius)
        .style(
            Style::new().shadows(
                glass_card_shadows(&theme, Elevation::Overlay)
                    .into_iter()
                    .map(Into::into)
                    .collect(),
            ),
        )
        .child(glass_layers(&theme, GlassStrength::Quiet, radius))
        .child(
            hover_surface
                .borrow()
                .pill(&theme, RadiusTokens::value(RadiusStep::Sm), false),
        )
        .child(list.build())
        .build()
        .with_key("composer-project-menu")
        .on_hover_change({
            let hover = Rc::clone(&hover_surface);
            move |hovered| {
                if !hovered {
                    hover.borrow().hide();
                }
            }
        })
}

fn engine_icon(theme: &ArtisanTheme, engine: artisan_api::EngineId) -> AssetId {
    match engine {
        artisan_api::EngineId::Claude => AssetId::SVGL_CLAUDE_AI,
        artisan_api::EngineId::Codex => AssetId::SVGL_OPENAI,
        artisan_api::EngineId::OpenCode if theme.colors.mode.is_dark() => AssetId::BRANDS_OPENCODE,
        artisan_api::EngineId::OpenCode => AssetId::BRANDS_OPENCODE_LIGHT,
    }
}

pub fn model_menu(
    theme: ArtisanTheme,
    host: &crate::core::HostData,
    picker: &super::picker::PickerState,
    current: Option<artisan_api::EngineConfig>,
    viewport: aqua_core::Size,
    engine_light: Option<Element>,
    axis_controls: Element,
    hover_surface: Rc<std::cell::RefCell<crate::ui::picker_motion::HoverSurface>>,
    tooltip_target: (crate::core::HostInstance, u64),
    dispatch: Dispatch,
) -> Element {
    use crate::ui::{
        asset::source_asset_glyph,
        glass::{
            GlassStrength, card_shadows, glass_card_shadows, glass_control_fill, glass_layers,
        },
        theme::{Elevation, Opacity},
    };
    use aqua_core::{Insets, Role};
    use aqua_ui::{
        AlignItems,
        element::{Button as UiButton, Scroll},
        input::{EventPhase, InputKind, InputResponse},
    };
    let engines: Vec<_> = host
        .engines
        .values()
        .filter(|status| {
            gate(status).is_none()
                || current
                    .as_ref()
                    .is_some_and(|config| config.engine == status.engine)
        })
        .collect();
    let browsing = picker
        .browsing
        .filter(|engine| engines.iter().any(|status| status.engine == *engine))
        .or_else(|| current.as_ref().map(|config| config.engine))
        .or_else(|| engines.first().map(|status| status.engine));
    let gradient = glass_control_fill(&theme);
    let mut tabs = UiBox::new()
        .height(40.0)
        .gap(4.0)
        .padding(4.0)
        .radius(RadiusTokens::value(RadiusStep::Lg))
        .clip(true)
        .style(
            Style::new()
                .gpui_background(aqua::GpuiBackground::LinearGradient {
                    angle: gradient.angle_degrees,
                    color_space: aqua::GpuiGradientColorSpace::Oklab,
                    stops: [
                        aqua::GpuiGradientStop {
                            percentage: 0.0,
                            color: gradient.start.to_reference_paint(),
                        },
                        aqua::GpuiGradientStop {
                            percentage: gradient.end_position,
                            color: gradient.end.to_reference_paint(),
                        },
                    ],
                })
                .shadows(card_shadows(&theme).into_iter().map(Into::into).collect()),
        );
    if let Some(engine_light) = engine_light {
        tabs = tabs.child(engine_light);
    }
    for status in &engines {
        let engine = status.engine;
        let selected = browsing == Some(engine);
        let action = Rc::clone(&dispatch);
        let tooltip_action = Rc::clone(&dispatch);
        let tooltip_anchor = format!("composer-engine-{engine}");
        let reason = gate(status).map(|gate| {
            let name = engine.display_name();
            match gate {
                "Not installed" => format!("{name} isn't installed on this Forge."),
                "Sign in" => format!("Sign in to {name} to use its models."),
                "Signing in…" => format!("Signing in to {name}…"),
                "Checking…" => format!("Checking {name}…"),
                "No models" => format!("{name} hasn't reported any models."),
                _ => unreachable!(),
            }
        });
        let mut tab = UiButton::new()
            .width(32.0)
            .height(32.0)
            .radius(RadiusTokens::value(RadiusStep::Xl))
            .style(
                Style::new()
                    .align(AlignItems::CENTER)
                    .justify(aqua_ui::JustifyContent::CENTER)
                    .flex_shrink(0.0),
            )
            .child(source_asset_glyph(
                engine_icon(&theme, engine),
                16.0,
                theme.colors.text.to_reference_paint(),
            ));
        if gate(status).is_some() {
            tab = tab.opacity(Opacity::Unavailable.alpha());
        }
        tabs = tabs.child(
            tab.on_activate(move || action(ComposerAction::BrowseEngine(engine)))
                .build()
                .with_key(format!("composer-engine-{engine}"))
                .role(Role::Button)
                .aria_label(engine.display_name())
                .aria_selected(selected)
                .focusable(true)
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
                            axis: None,
                        });
                    }
                }),
        );
    }
    let mut list = UiBox::new()
        .layout("column")
        .height(192.0)
        .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
        .child(
            hover_surface
                .borrow()
                .pill(&theme, RadiusTokens::value(RadiusStep::Xl), false),
        );
    if let Some(status) = browsing.and_then(|engine| {
        engines
            .iter()
            .find(|status| status.engine == engine)
            .copied()
    }) {
        let unavailable = gate(status);
        let mut rows = UiBox::new().layout("column").gap(3.0);
        if let Some(reason) = unavailable {
            if reason == "Not installed" || reason == "Sign in" {
                let action_label = if reason == "Sign in" {
                    format!("Sign in to {}", status.engine.display_name())
                } else {
                    format!("Install {}", status.engine.display_name())
                };
                let action = Rc::clone(&dispatch);
                let engine = status.engine;
                rows = rows.child(
                    UiBox::new()
                        .layout("column")
                        .padding(12.0)
                        .child(label(
                            &theme,
                            format!(
                                "{} is not ready: {}.",
                                engine.display_name(),
                                reason.to_lowercase()
                            ),
                            14.0,
                            20.0,
                            theme.colors.text_muted.to_reference_text_color(),
                        ))
                        .child(
                            UiButton::new()
                                .on_activate(move || {
                                    action(ComposerAction::OpenEngineSettings(engine))
                                })
                                .child(label(
                                    &theme,
                                    action_label,
                                    14.0,
                                    20.0,
                                    theme.colors.text.to_reference_text_color(),
                                ))
                                .build(),
                        )
                        .build(),
                );
            } else if status.models.is_empty() {
                rows = rows.child(
                    UiBox::new()
                        .padding(12.0)
                        .child(label(
                            &theme,
                            reason.into(),
                            14.0,
                            20.0,
                            theme.colors.text_muted.to_reference_text_color(),
                        ))
                        .build(),
                );
            }
        }
        let show_rows = unavailable
            .is_none_or(|reason| reason != "Not installed" && reason != "Sign in")
            && (unavailable.is_none() || !status.models.is_empty());
        for entry in picker.models.iter().filter(|_| show_rows) {
            match entry {
                super::picker::ModelRow::Group { name, open } => {
                    let name = name.clone();
                    let is_open = *open;
                    let group_label = if name.is_empty() {
                        "Other".to_owned()
                    } else {
                        name.clone()
                    };
                    let action = Rc::clone(&dispatch);
                    let header_hover = Rc::clone(&hover_surface);
                    let engine = status.engine;
                    let row = UiButton::new()
                        .width(Length::Pct(100.0))
                        .height(28.0)
                        .gap(8.0)
                        .style(
                            Style::new()
                                .flex_shrink(0.0)
                                .align(AlignItems::CENTER)
                                .padding_insets(Insets {
                                    left: 10.0,
                                    right: 10.0,
                                    top: 0.0,
                                    bottom: 0.0,
                                }),
                        )
                        .child(
                            UiBox::new()
                                .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                                .child(label(
                                    &theme,
                                    group_label.clone(),
                                    12.0,
                                    16.0,
                                    theme.colors.text_muted.to_reference_text_color(),
                                ))
                                .build(),
                        )
                        .child(source_asset_glyph(
                            if is_open {
                                AssetId::TABLER_CHEVRON_DOWN
                            } else {
                                AssetId::TABLER_CHEVRON_RIGHT
                            },
                            14.0,
                            theme.colors.text_muted.to_reference_paint(),
                        ))
                        .on_activate(move || {
                            action(ComposerAction::ToggleModelGroup {
                                engine,
                                name: name.clone(),
                                open: is_open,
                            })
                        })
                        .build()
                        .role(Role::Button)
                        .aria_label(group_label)
                        .focusable(true)
                        .on_hover_change(move |hovered| {
                            if hovered {
                                header_hover.borrow().hide();
                            }
                        });
                    rows = rows.child(row);
                }
                super::picker::ModelRow::Model { id, index } => {
                    let Some(model) = status.models.get(*index).filter(|model| &model.id == id)
                    else {
                        continue;
                    };
                    let engine = status.engine;
                    let model_id = id.clone();
                    let hover_id = id.clone();
                    let action = Rc::clone(&dispatch);
                    let hover = Rc::clone(&dispatch);
                    let pill_hover = Rc::clone(&hover_surface);
                    let pill_key = format!("composer-model-{engine}-{id}");
                    let lab = model.lab.clone().unwrap_or_else(|| {
                        match engine {
                            artisan_api::EngineId::Claude => "Anthropic",
                            artisan_api::EngineId::Codex => "OpenAI",
                            artisan_api::EngineId::OpenCode => "OpenCode",
                        }
                        .into()
                    });
                    let text = UiBox::new()
                        .layout("column")
                        .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                        .child(semibold(
                            &theme,
                            model.name.clone(),
                            14.0,
                            20.0,
                            theme.colors.text.to_reference_text_color(),
                        ))
                        .child(label(
                            &theme,
                            lab,
                            12.0,
                            16.0,
                            theme.colors.text_muted.to_reference_text_color(),
                        ))
                        .build();
                    let row = UiButton::new()
                        .width(Length::Pct(100.0))
                        .height(48.0)
                        .gap(8.0)
                        .radius(RadiusTokens::value(RadiusStep::Xl))
                        .style(
                            Style::new()
                                .flex_shrink(0.0)
                                .align(AlignItems::CENTER)
                                .padding_insets(Insets {
                                    left: 10.0,
                                    right: 10.0,
                                    top: 0.0,
                                    bottom: 0.0,
                                }),
                        )
                        .child(source_asset_glyph(
                            model
                                .lab
                                .as_deref()
                                .and_then(lab_icon)
                                .unwrap_or_else(|| engine_icon(&theme, engine)),
                            20.0,
                            theme.colors.text.to_reference_paint(),
                        ))
                        .child(text)
                        .on_activate(move || {
                            action(ComposerAction::ChooseModel {
                                engine,
                                id: model_id.clone(),
                            })
                        })
                        .build()
                        .with_key(format!("composer-model-{engine}-{id}"))
                        .role(Role::MenuItem)
                        .aria_label(model.name.clone())
                        .focusable(true)
                        .on_hover_change(move |hovered| {
                            if hovered {
                                pill_hover.borrow().hovered(
                                    &pill_key,
                                    true,
                                    crate::ui::hover::HoverTone::Neutral,
                                );
                            }
                        })
                        .on_input(InputKind::PointerEnter, EventPhase::Bubble, move |_| {
                            hover(ComposerAction::PreviewModel {
                                engine,
                                id: hover_id.clone(),
                            });
                            InputResponse::default()
                        });
                    rows = rows.child(row);
                }
            }
        }
        list = list.child(
            Scroll::new()
                .height(192.0)
                .child(rows.build())
                .build()
                .with_key(crate::shell::wheel::MODEL_MENU),
        );
    } else {
        list = list.child(label(
            &theme,
            "Forge reported no engines.".into(),
            14.0,
            20.0,
            theme.colors.text_muted.to_reference_text_color(),
        ));
    }
    let list = list
        .build()
        .with_key("composer-model-viewport")
        .on_hover_change({
            let hover = Rc::clone(&hover_surface);
            move |hovered| {
                if !hovered {
                    hover.borrow().hide();
                }
            }
        });
    let preview_config = picker.preview_config(host, current);
    let mut preview = UiBox::new()
        .layout("column")
        .width(224.0)
        .height(192.0)
        .gap(8.0)
        .padding(10.0)
        .style(Style::new().flex_shrink(0.0).min_width(Length::Px(0.0)));
    if let Some(config) = preview_config {
        let model = host.engines.get(&config.engine).and_then(|status| {
            config
                .model
                .as_ref()
                .and_then(|id| status.models.iter().find(|model| &model.id == id))
        });
        let name = model.map_or_else(
            || config.model.clone().unwrap_or_default(),
            |model| model.name.clone(),
        );
        let line = model
            .and_then(|model| model.description.clone())
            .unwrap_or_else(|| {
                let lab = match config.engine {
                    artisan_api::EngineId::Claude => "Anthropic",
                    artisan_api::EngineId::Codex => "OpenAI",
                    artisan_api::EngineId::OpenCode => "OpenCode",
                };
                let mut facts = vec![format!(
                    "{} by {}",
                    model
                        .and_then(|model| model.group.as_deref())
                        .unwrap_or_else(|| config.engine.display_name()),
                    model.and_then(|model| model.lab.as_deref()).unwrap_or(lab)
                )];
                if let Some(context) = model
                    .and_then(|model| config.context_window_override().or(model.context_window))
                {
                    let value = if context >= 1_000_000 && context.is_multiple_of(1_000_000) {
                        format!("{}M", context / 1_000_000)
                    } else {
                        format!("{}K", context / 1_000)
                    };
                    facts.push(format!("{value} context"));
                }
                if model.is_some_and(|model| model.supports_images) {
                    facts.push("Reads images".into());
                }
                facts.join(" · ")
            });
        preview = preview
            .child(
                UiBox::new()
                    .layout("column")
                    .gap(4.0)
                    .style(Style::new().flex_grow(1.0).min_height(Length::Px(0.0)))
                    .child(semibold(
                        &theme,
                        name,
                        14.0,
                        20.0,
                        theme.colors.text.to_reference_text_color(),
                    ))
                    .child(paragraph(
                        &theme,
                        line,
                        12.0,
                        16.0,
                        theme.colors.text_muted.to_reference_text_color(),
                    ))
                    .build(),
            )
            .child(axis_controls);
    }
    let radius = RadiusTokens::value(RadiusStep::X3l);
    UiBox::new()
        .layout("column")
        .width(480.0_f32.min(viewport.w - 32.0))
        .padding(8.0)
        .gap(8.0)
        .radius(radius)
        .style(
            Style::new().max_height(Length::Px(320.0)).shadows(
                glass_card_shadows(&theme, Elevation::Overlay)
                    .into_iter()
                    .map(Into::into)
                    .collect(),
            ),
        )
        .child(glass_layers(&theme, GlassStrength::Quiet, radius))
        .child(tabs.build())
        .child(
            UiBox::new()
                .height(192.0)
                .gap(8.0)
                .child(list)
                .child(preview.build())
                .build(),
        )
        .build()
        .with_key("composer-model-menu")
}

fn gate(status: &artisan_api::EngineStatus) -> Option<&'static str> {
    use artisan_api::{EngineId, InstallState, SignInState};
    if !matches!(status.install, InstallState::Installed { .. }) {
        return Some("Not installed");
    }
    match &status.sign_in {
        SignInState::SignedOut
            if status.engine == EngineId::OpenCode && !status.models.is_empty() =>
        {
            None
        }
        SignInState::SignedOut | SignInState::Failed { .. } => Some("Sign in"),
        SignInState::LoggingIn { .. } => Some("Signing in…"),
        SignInState::Unknown if status.models.is_empty() => Some("Checking…"),
        SignInState::SignedIn { .. } if status.models.is_empty() => Some("No models"),
        SignInState::Unknown | SignInState::SignedIn { .. } => None,
    }
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

pub fn axis_controls(
    theme: ArtisanTheme,
    host: &crate::core::HostData,
    picker: &super::picker::PickerState,
    current: Option<artisan_api::EngineConfig>,
    dispatch: Dispatch,
) -> Element {
    use crate::ui::{
        asset::source_asset_glyph,
        glass::{card_shadows, glass_control_fill},
    };
    use aqua_core::{Insets, Role};
    use aqua_ui::{AlignItems, element::Button as UiButton};
    let mut controls = UiBox::new().layout("column").gap(6.0);
    let Some(config) = picker.preview_config(host, current) else {
        return controls.build();
    };
    let model = host.engines.get(&config.engine).and_then(|status| {
        config
            .model
            .as_ref()
            .and_then(|id| status.models.iter().find(|model| &model.id == id))
    });
    for axis in super::picker::Axis::ALL {
        let options = super::picker::axis_options(axis, &config, model);
        if !axis.shown(options.len()) {
            continue;
        }
        let value = options
            .iter()
            .find(|option| option.selected)
            .or_else(|| options.first())
            .map(|option| option.label.clone())
            .unwrap_or_default();
        let icon = match axis {
            super::picker::Axis::Variant | super::picker::Axis::Thinking => AssetId::TABLER_BRAIN,
            super::picker::Axis::Speed => AssetId::TABLER_BOLT_FILLED,
            super::picker::Axis::Context => AssetId::TABLER_ARROWS_HORIZONTAL,
            super::picker::Axis::Permission => AssetId::TABLER_LOCK,
        };
        let gradient = glass_control_fill(&theme);
        let action = Rc::clone(&dispatch);
        let control = UiButton::new()
            .height(24.0)
            .gap(7.0)
            .radius(RadiusTokens::value(RadiusStep::Md))
            .style(
                Style::new()
                    .align(AlignItems::CENTER)
                    .padding_insets(Insets {
                        left: 8.0,
                        right: 8.0,
                        top: 0.0,
                        bottom: 0.0,
                    })
                    .gpui_background(aqua::GpuiBackground::LinearGradient {
                        angle: gradient.angle_degrees,
                        color_space: aqua::GpuiGradientColorSpace::Oklab,
                        stops: [
                            aqua::GpuiGradientStop {
                                percentage: 0.0,
                                color: gradient.start.to_reference_paint(),
                            },
                            aqua::GpuiGradientStop {
                                percentage: gradient.end_position,
                                color: gradient.end.to_reference_paint(),
                            },
                        ],
                    })
                    .shadows(card_shadows(&theme).into_iter().map(Into::into).collect()),
            )
            .child(source_asset_glyph(
                icon,
                14.0,
                theme.colors.text_muted.to_reference_paint(),
            ))
            .child(
                UiBox::new()
                    .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                    .child(label(
                        &theme,
                        value.clone(),
                        12.0,
                        16.0,
                        theme.colors.text.to_reference_text_color(),
                    ))
                    .build(),
            )
            .child(source_asset_glyph(
                AssetId::TABLER_SELECTOR,
                14.0,
                theme.colors.text_muted.to_reference_paint(),
            ))
            .on_activate(move || action(ComposerAction::ToggleAxis(axis)))
            .build()
            .with_key(format!("composer-axis-{axis:?}"))
            .role(Role::Button)
            .aria_label(format!("{}: {value}", axis.title()))
            .focusable(true);
        controls = controls.child(control);
    }
    controls.build()
}

pub fn axis_menu(
    theme: ArtisanTheme,
    axis: super::picker::Axis,
    options: &[super::picker::AxisOption],
    keyboard_cursor: Option<usize>,
    hover_surface: Rc<std::cell::RefCell<crate::ui::picker_motion::HoverSurface>>,
    tooltip_target: Option<(crate::core::HostInstance, u64)>,
    dispatch: Dispatch,
) -> Element {
    use crate::ui::{
        asset::source_asset_glyph,
        glass::{GlassStrength, glass_card_shadows, glass_layers, glass_menu_separator},
        theme::Elevation,
    };
    use aqua_core::{Insets, Role};
    use aqua_ui::{AlignItems, element::Button as UiButton};
    let header = |text: &str| {
        UiBox::new()
            .style(Style::new().padding_insets(Insets {
                left: 12.0,
                right: 12.0,
                top: 4.0,
                bottom: 4.0,
            }))
            .child(label(
                &theme,
                text.to_owned(),
                12.0,
                16.0,
                theme.colors.text_muted.to_reference_text_color(),
            ))
            .build()
    };
    let mut rows = UiBox::new().layout("column").child(header(axis.title()));
    let mut special = false;
    for (index, option) in options.iter().enumerate() {
        if option.special && !special {
            special = true;
            rows = rows
                .child(glass_menu_separator(&theme))
                .child(header("Special Efforts"));
        }
        let action = Rc::clone(&dispatch);
        let mut row = UiButton::new()
            .width(Length::Pct(100.0))
            .gap(10.0)
            .radius(RadiusTokens::value(RadiusStep::Xl))
            .style(
                Style::new()
                    .align(AlignItems::CENTER)
                    .padding_insets(Insets {
                        left: 12.0,
                        right: 8.0,
                        top: 8.0,
                        bottom: 8.0,
                    }),
            )
            .child(
                UiBox::new()
                    .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                    .child(label(
                        &theme,
                        option.label.clone(),
                        14.0,
                        20.0,
                        theme.colors.text.to_reference_text_color(),
                    ))
                    .build(),
            );
        if keyboard_cursor == Some(index) {
            row = row.fill(theme.colors.selected.to_paint());
        }
        if option.selected {
            row = row.child(source_asset_glyph(
                AssetId::TABLER_CHECK,
                14.0,
                theme.colors.text_muted.to_reference_paint(),
            ));
        }
        let pointer_action = Rc::clone(&dispatch);
        let tooltip_action = Rc::clone(&dispatch);
        let description = option.description.clone();
        let advisory = option.advisory.clone();
        let hover = Rc::clone(&hover_surface);
        let row_key = format!("composer-axis-{axis:?}-{index}");
        rows = rows.child(
            row.on_activate(move || action(ComposerAction::ChooseAxis { axis, index }))
                .build()
                .with_key(row_key.clone())
                .on_hover_change(move |hovered| {
                    if hovered {
                        hover.borrow().hovered(
                            &row_key,
                            true,
                            crate::ui::hover::HoverTone::Neutral,
                        );
                    }
                    if let Some((instance, generation)) = tooltip_target {
                        tooltip_action(ComposerAction::TooltipHover {
                            instance,
                            generation,
                            anchor: row_key.clone(),
                            content: (advisory.is_some() || description.is_some()).then(|| {
                                super::picker::TooltipContent {
                                    anchor: row_key.clone(),
                                    advisory: advisory.clone(),
                                    text: description.clone().unwrap_or_default(),
                                    placement: super::picker::TooltipPlacement::Beside,
                                }
                            }),
                            hovered,
                            axis: Some(axis),
                        });
                    }
                })
                .role(Role::MenuItem)
                .aria_label(option.label.clone())
                .aria_selected(option.selected)
                .focusable(true)
                .on_input(
                    aqua_ui::input::InputKind::PointerEnter,
                    aqua_ui::input::EventPhase::Bubble,
                    move |_| {
                        pointer_action(ComposerAction::AxisPointer { axis, index });
                        aqua_ui::input::InputResponse::default()
                    },
                ),
        );
    }
    let radius = RadiusTokens::value(RadiusStep::X2l);
    let key_dispatch = Rc::clone(&dispatch);
    UiBox::new()
        .layout("column")
        .style(
            Style::new().min_width(Length::Px(144.0)).shadows(
                glass_card_shadows(&theme, Elevation::Overlay)
                    .into_iter()
                    .map(Into::into)
                    .collect(),
            ),
        )
        .radius(radius)
        .child(glass_layers(&theme, GlassStrength::Quiet, radius))
        .child(
            hover_surface
                .borrow()
                .pill(&theme, RadiusTokens::value(RadiusStep::Xl), false),
        )
        .child(rows.build().on_hover_change({
            let hover = Rc::clone(&hover_surface);
            move |hovered| {
                if !hovered {
                    hover.borrow().clear();
                }
            }
        }))
        .build()
        .with_key(format!("composer-axis-options-{axis:?}"))
        .on_input(
            aqua_ui::input::InputKind::KeyDown,
            aqua_ui::input::EventPhase::Bubble,
            move |event| {
                use aqua_core::{InputEvent, Key, NamedKey};
                use aqua_ui::input::{InputResponse, RoutedInput};
                let key = match &event.input {
                    RoutedInput::Native(InputEvent::KeyDown(event)) => match event.key {
                        Key::Named(NamedKey::Home) => Some(super::picker::AxisKey::Home),
                        Key::Named(NamedKey::End) => Some(super::picker::AxisKey::End),
                        Key::Named(NamedKey::Up) => Some(super::picker::AxisKey::Up),
                        Key::Named(NamedKey::Down) => Some(super::picker::AxisKey::Down),
                        Key::Named(NamedKey::Enter) | Key::Char(' ') => {
                            Some(super::picker::AxisKey::Accept)
                        }
                        Key::Named(NamedKey::Escape) => Some(super::picker::AxisKey::Dismiss),
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

pub fn project_tab(
    theme: ArtisanTheme,
    controller: &super::controller::Composer,
    host: &crate::core::HostData,
    dispatch: Dispatch,
) -> Option<Element> {
    use crate::ui::asset::source_asset_glyph;
    use aqua_core::{Insets, Role};
    use aqua_ui::{AlignItems, element::Button as UiButton};
    let padding = Style::new()
        .align(AlignItems::CENTER)
        .padding_insets(Insets {
            left: 8.0,
            right: 8.0,
            top: 0.0,
            bottom: 0.0,
        });
    let folder = || {
        source_asset_glyph(
            AssetId::TABLER_FOLDER,
            15.0,
            theme.colors.text_muted.to_reference_paint(),
        )
    };
    match &controller.target {
        super::controller::ComposerTarget::Thread { thread, .. } => {
            let thread = host.threads.get(thread)?;
            let project = host.projects.get(&thread.project_id)?;
            Some(
                UiBox::new()
                    .height(28.0)
                    .gap(6.0)
                    .style(padding)
                    .child(folder())
                    .child(
                        UiBox::new()
                            .style(Style::new().max_width(Length::Px(240.0)))
                            .child(label(
                                &theme,
                                project.name.clone(),
                                13.0,
                                18.0,
                                theme.colors.text_muted.to_reference_text_color(),
                            ))
                            .build(),
                    )
                    .build(),
            )
        }
        super::controller::ComposerTarget::NewThread { .. } => {
            let name = controller
                .project(host)
                .as_ref()
                .and_then(|id| host.projects.get(id))
                .map_or_else(
                    || "Choose a project".to_owned(),
                    |project| project.name.clone(),
                );
            let open = controller.picker.menu == Some(super::picker::Menu::Project);
            let foreground = if open {
                theme.colors.text.to_reference_paint()
            } else {
                theme.colors.text_muted.to_reference_paint()
            };
            Some(
                UiButton::new()
                    .height(28.0)
                    .gap(6.0)
                    .radius(RadiusTokens::value(RadiusStep::Lg))
                    .style(padding)
                    .child(source_asset_glyph(AssetId::TABLER_FOLDER, 15.0, foreground))
                    .child(
                        UiBox::new()
                            .style(Style::new().max_width(Length::Px(240.0)))
                            .child(label(
                                &theme,
                                name.clone(),
                                13.0,
                                18.0,
                                if open {
                                    theme.colors.text.to_reference_text_color()
                                } else {
                                    theme.colors.text_muted.to_reference_text_color()
                                },
                            ))
                            .build(),
                    )
                    .child(source_asset_glyph(
                        AssetId::TABLER_SELECTOR,
                        14.0,
                        foreground,
                    ))
                    .on_activate(move || dispatch(ComposerAction::ToggleProject))
                    .build()
                    .with_key("composer-project")
                    .role(Role::Button)
                    .aria_label(name)
                    .focusable(true),
            )
        }
    }
}
