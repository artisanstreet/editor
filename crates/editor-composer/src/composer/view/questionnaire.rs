use super::{ComposerAction, Dispatch};
use aqua_core::Role;
use aqua_ui::{
    AlignItems,
    element::{Box as UiBox, Element, Scroll},
    style::{Length, Style},
};
use editor_assets::AssetId;
use editor_ui::{
    kit::{
        Button, ButtonTone, Density, Finish, HitArea, Hover, IconButton, IconButtonSize, IconSize,
        Radius, Row, RowItem, Space, Spaced, Surface, TextRole, live,
    },
    theme::ArtisanTheme,
};
use std::rc::Rc;

pub fn questionnaire(
    theme: ArtisanTheme,
    panel: &super::super::questionnaire::QuestionnairePanel,
    question: Option<&artisan_api::Question>,
    stored_options: Option<Element>,
    load_error: Option<&str>,
    source_view: Option<Element>,
    dispatch: Dispatch,
) -> Option<Element> {
    let active = panel.active()?;
    let muted = theme.colors.text_muted;
    let surface = Surface::Composer;
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
                            question
                                .and_then(|question| question.header.clone())
                                .unwrap_or_else(|| "Question".into()),
                        )
                        .gpui_source_color(muted.to_reference_text_color())
                        .wrap(false)
                        .build(),
                )
                .build(),
        )
        .child(dismiss)
        .build();
    let card = match question {
        Some(question) => question_card(
            theme,
            panel,
            active,
            question,
            stored_options,
            source_view,
            header,
            &dispatch,
        ),
        None => {
            let mut loading = UiBox::new()
                .layout("column")
                .spaced(Space::S8)
                .child(header)
                .child(
                    TextRole::Label
                        .text(&theme, load_error.unwrap_or("Loading question…").to_owned())
                        .wrap(true)
                        .build(),
                );
            if load_error.is_some() {
                let action = Rc::clone(&dispatch);
                loading = loading.child(
                    Button::text(
                        "question-page-retry",
                        theme,
                        ButtonTone::Neutral,
                        "Try again",
                    )
                    .within(surface)
                    .on_activate(move |_| action(ComposerAction::RetryQuestionPage))
                    .build(),
                );
            }
            loading.build()
        }
    };
    Some(
        Scroll::new()
            .layout("column")
            .width(Length::Pct(100.0))
            .style(Style::new().max_height(Length::Px(QUESTIONNAIRE_MAX_HEIGHT)))
            .child(
                UiBox::new()
                    .layout("column")
                    .width(Length::Pct(100.0))
                    .spaced(Space::S8)
                    .child(card)
                    .child(other_questionnaires(
                        theme, panel, active, sending, &dispatch,
                    ))
                    .build(),
            )
            .build()
            .with_key("composer-questionnaire"),
    )
}

pub(super) const QUESTIONNAIRE_MAX_HEIGHT: f32 = 280.0;

#[expect(
    clippy::too_many_arguments,
    reason = "a loaded question card needs the panel, its question and the parts mounted around it"
)]
pub(super) fn question_card(
    theme: ArtisanTheme,
    panel: &super::super::questionnaire::QuestionnairePanel,
    active: &super::super::questionnaire::ComposerQuestionnaire,
    question: &artisan_api::Question,
    stored_options: Option<Element>,
    source_view: Option<Element>,
    header: Element,
    dispatch: &Dispatch,
) -> Element {
    let muted = theme.colors.text_muted;
    let surface = Surface::Composer;
    let sending = panel.sending.is_some();
    let text = match &question.text {
        artisan_api::TextContent::Inline(text) => TextRole::Label
            .text(&theme, text.clone())
            .wrap(true)
            .build(),
        artisan_api::TextContent::Stored(source) => {
            let source = source.clone();
            let action = Rc::clone(dispatch);
            Button::text(
                "question-source",
                theme,
                ButtonTone::Quiet,
                "Read full question",
            )
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
                true => IconSize::Md.glyph(
                    AssetId::TABLER_CHECK,
                    theme.colors.text.to_reference_paint(),
                ),
                false => TextRole::Label
                    .text(&theme, (index + 1).to_string())
                    .gpui_source_color(muted.to_reference_text_color())
                    .wrap(false)
                    .build(),
            };
            let mut text = UiBox::new()
                .layout("column")
                .style(Style::new().flex_grow(1.0).min_width(Length::Pct(0.0)))
                .child(
                    TextRole::Label
                        .text(&theme, option.label.clone())
                        .wrap(true)
                        .build(),
                );
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
            let action = Rc::clone(dispatch);
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
                &theme,
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
                let action = Rc::clone(dispatch);
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
        let action = Rc::clone(dispatch);
        rows = rows.child(
            Button::text(
                "composer-question-continue",
                theme,
                ButtonTone::Primary,
                text,
            )
            .within(surface)
            .on_activate(move |_| action(ComposerAction::ContinueQuestionnaire))
            .build(),
        );
    }
    card = card.child(rows.build());
    if panel.can_go_back() {
        let action = Rc::clone(dispatch);
        card = card.child(
            Button::text("composer-question-back", theme, ButtonTone::Quiet, "Back")
                .within(surface)
                .on_activate(move |_| action(ComposerAction::QuestionBack))
                .build(),
        );
    }
    if sending {
        card = card.child(
            TextRole::Caption
                .text(&theme, "Sending your answer…")
                .gpui_source_color(muted.to_reference_text_color())
                .build(),
        );
    } else if let Some(error) = &panel.error {
        let action = Rc::clone(dispatch);
        card = card
            .child(
                TextRole::Label
                    .text(&theme, error.clone())
                    .gpui_source_color(theme.colors.destructive_text.to_reference_text_color())
                    .wrap(true)
                    .build(),
            )
            .child(
                Button::text(
                    "composer-question-retry",
                    theme,
                    ButtonTone::Neutral,
                    "Send the answer again",
                )
                .within(surface)
                .on_activate(move |_| action(ComposerAction::RetryQuestionnaire))
                .build(),
            );
    }
    card.build()
}

pub(super) fn other_questionnaires(
    theme: ArtisanTheme,
    panel: &super::super::questionnaire::QuestionnairePanel,
    active: &super::super::questionnaire::ComposerQuestionnaire,
    sending: bool,
    dispatch: &Dispatch,
) -> Element {
    let muted = theme.colors.text_muted;
    let surface = Surface::Composer;
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
        let action = Rc::clone(dispatch);
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
                    .text(
                        &theme,
                        editor_ui::locale::count(other.questions.len(), "question", "questions"),
                    )
                    .gpui_source_color(muted.to_reference_text_color())
                    .wrap(false)
                    .build(),
            );
        }
        stack = stack.child(row.build());
    }
    stack.build()
}

pub(super) fn choice(row: Element, selected: bool, sending: bool) -> Element {
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
