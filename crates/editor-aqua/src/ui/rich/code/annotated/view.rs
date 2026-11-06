use std::{cell::RefCell, rc::Rc, sync::Arc};

use aqua_core::{GpuiBackground, Insets};
use aqua_text::{FontFamily, FontWeight, TextStyle};
use aqua_ui::{
    AlignItems, JustifyContent,
    collection::List,
    element::{Box as UiBox, Element, Scroll, Text},
    style::{Length, Style},
};
use editor_assets::AssetId;

use super::{AnnotatedCode, Model};
use crate::ui::{
    button::{Button, ButtonVariant, LabelOverflow},
    rich::actions::{OpenReference, ResourceScope},
    theme::{ArtisanTheme, Opacity, RadiusStep, RadiusTokens, TextStep, TypographyTokens},
};

const LINE_HEIGHT: f32 = 20.0;

pub(crate) struct AnnotatedState {
    model: Arc<Model>,
    line: Option<usize>,
    note: Option<usize>,
}

impl AnnotatedState {
    pub(crate) fn new(model: &AnnotatedCode) -> Self {
        Self {
            model: Arc::clone(&model.0),
            line: None,
            note: None,
        }
    }

    pub(crate) fn update_model(&mut self, model: &AnnotatedCode) -> bool {
        if Arc::ptr_eq(&self.model, &model.0) {
            return false;
        }
        self.model = Arc::clone(&model.0);
        self.line = None;
        self.note = None;
        true
    }

    fn active(&self) -> Option<usize> {
        self.note.or_else(|| {
            self.line
                .and_then(|line| self.model.owner.get(line).copied().flatten())
        })
    }

    fn hover(&mut self, note: bool, index: usize, over: bool) -> bool {
        let slot = if note { &mut self.note } else { &mut self.line };
        if over {
            if *slot == Some(index) {
                return false;
            }
            *slot = Some(index);
        } else if *slot == Some(index) {
            *slot = None;
        } else {
            return false;
        }
        true
    }
}

type Open = Rc<dyn Fn(OpenReference, Option<crate::core::HostInstance>)>;

impl AnnotatedCode {
    pub(crate) fn copy(&self) -> String {
        self.0.lines.join("\n")
    }

    pub(crate) fn render(
        &self,
        theme: &ArtisanTheme,
        id: &str,
        scope: Option<ResourceScope>,
        instance: Option<crate::core::HostInstance>,
        open: Open,
        state: Rc<RefCell<AnnotatedState>>,
        invalidate: Rc<dyn Fn()>,
    ) -> Element {
        state.borrow_mut().update_model(self);
        let model = Arc::clone(&self.0);
        let active = state.borrow().active();
        let mut body = UiBox::new()
            .layout("column")
            .gap(8.0)
            .width(Length::Pct(100.0))
            .style(Style::new().min_width(Length::Px(0.0)));
        if let Some(path) = &model.path {
            let action = OpenReference {
                scope: scope.clone(),
                path: path.clone(),
                line: Some(model.start),
            };
            let open = open.clone();
            let owner = Rc::downgrade(&state);
            let rendered = Arc::downgrade(&model);
            body = body.child(
                Button::icon_text(
                    format!("{id}-path"),
                    *theme,
                    AssetId::TABLER_FILE_TEXT,
                    format!("{path}:{}", model.start),
                )
                .variant(ButtonVariant::Ghost)
                .label_overflow(LabelOverflow::Start)
                .on_activate(move |_| {
                    if owner
                        .upgrade()
                        .zip(rendered.upgrade())
                        .is_some_and(|(state, model)| Arc::ptr_eq(&state.borrow().model, &model))
                    {
                        open(action.clone(), instance);
                    }
                })
                .build(),
            );
        }
        let count = model.lines.len();
        let gutter = f32::from(
            u16::try_from(model.number(count.saturating_sub(1)).max(1).ilog10() + 1)
                .unwrap_or(u16::MAX),
        )
        .mul_add(8.0, 20.0);
        let width = model.width + gutter + if model.notes.is_empty() { 0.0 } else { 24.0 };
        let height = crate::ui::rich::charts::scale::f32_of_count(count.min(18)) * LINE_HEIGHT;
        let row_model = Arc::clone(&model);
        let row_state = state.clone();
        let row_invalidate = invalidate.clone();
        let row_theme = *theme;
        let row_id = id.to_owned();
        let widest = model
            .lines
            .iter()
            .enumerate()
            .max_by_key(|(_, line)| unicode_width::UnicodeWidthStr::width(line.as_str()))
            .map_or(0, |(index, _)| index);
        let rows = List::new()
            .items(0..count)
            .key(|index| *index)
            .virtualize()
            .width_probe(widest)
            .estimated_row_height(Length::Px(LINE_HEIGHT))
            .style(
                Style::new()
                    .width(Length::Pct(100.0))
                    .min_width(Length::Px(width))
                    .height(Length::Px(height)),
            )
            .row(move |index| {
                code_row(
                    &row_theme,
                    &row_id,
                    &row_model,
                    *index,
                    gutter,
                    active,
                    row_state.clone(),
                    row_invalidate.clone(),
                )
            })
            .build()
            .with_key(format!("{id}-code-rows"));
        body = body.child(
            Scroll::new()
                .width(Length::Pct(100.0))
                .height(height)
                .style(Style::new().min_width(Length::Px(0.0)))
                .child(rows)
                .build()
                .with_key(format!("{id}-code-scroll")),
        );
        if !model.notes.is_empty() {
            let height =
                crate::ui::rich::charts::scale::f32_of_count(model.notes.len().min(6)) * 40.0;
            let note_model = Arc::clone(&model);
            let note_theme = *theme;
            let note_id = id.to_owned();
            let notes = List::new()
                .items(0..model.notes.len())
                .key(|index| *index)
                .virtualize()
                .estimated_row_height(Length::Px(40.0))
                .style(
                    Style::new()
                        .width(Length::Pct(100.0))
                        .height(Length::Px(height)),
                )
                .row(move |index| {
                    note_row(
                        &note_theme,
                        &note_id,
                        &note_model,
                        *index,
                        active,
                        scope.clone(),
                        instance,
                        open.clone(),
                        state.clone(),
                        invalidate.clone(),
                    )
                })
                .build()
                .with_key(format!("{id}-notes-{:p}", Arc::as_ptr(&model)));
            body = body.child(
                UiBox::new()
                    .width(Length::Pct(100.0))
                    .style(
                        Style::new()
                            .min_width(Length::Px(0.0))
                            .padding_insets(Insets {
                                top: 8.0,
                                ..Insets::default()
                            })
                            .gpui_border(
                                Insets {
                                    top: 1.0,
                                    ..Insets::default()
                                },
                                theme.colors.text.at(Opacity::Rule).to_reference_paint(),
                                false,
                            ),
                    )
                    .child(notes)
                    .build(),
            );
        }
        body.build()
    }
}

fn text_style(
    theme: &ArtisanTheme,
    mono: bool,
    size: f32,
    line: f32,
    weight: FontWeight,
) -> TextStyle {
    TextStyle {
        families: vec![FontFamily::Named(
            if mono {
                theme.typography.mono.family
            } else {
                theme.typography.sans.family
            }
            .to_owned(),
        )],
        size_px: size,
        line_height_px: line,
        weight,
        letter_spacing_em: mono.then_some(0.0),
        ..TextStyle::default()
    }
}

fn badge(theme: &ArtisanTheme, number: usize, active: bool) -> Element {
    let accent = theme.colors.link;
    UiBox::new()
        .width(16.0)
        .height(16.0)
        .radius(RadiusTokens::value(RadiusStep::Md))
        .style(
            Style::new()
                .flex_shrink(0.0)
                .align(AlignItems::CENTER)
                .justify(JustifyContent::CENTER)
                .gpui_background(GpuiBackground::Solid(
                    accent
                        .at(if active {
                            Opacity::BadgeActive
                        } else {
                            Opacity::BadgeTint
                        })
                        .to_reference_paint(),
                )),
        )
        .child(
            Text::new(number.to_string())
                .text_style(text_style(
                    theme,
                    false,
                    TypographyTokens::size(TextStep::X3s),
                    16.0,
                    FontWeight(600),
                ))
                .gpui_source_color(
                    if active {
                        theme.colors.background
                    } else {
                        accent
                    }
                    .to_reference_text_color(),
                )
                .build(),
        )
        .build()
}

fn hover(
    element: Element,
    state: Rc<RefCell<AnnotatedState>>,
    model: Arc<Model>,
    index: usize,
    note: bool,
    invalidate: Rc<dyn Fn()>,
) -> Element {
    element.on_hover_change(move |over| {
        let mut state = state.borrow_mut();
        if Arc::ptr_eq(&state.model, &model) && state.hover(note, index, over) {
            drop(state);
            invalidate();
        }
    })
}

fn code_row(
    theme: &ArtisanTheme,
    id: &str,
    model: &Arc<Model>,
    index: usize,
    gutter: f32,
    active: Option<usize>,
    state: Rc<RefCell<AnnotatedState>>,
    invalidate: Rc<dyn Fn()>,
) -> Element {
    let accent = theme.colors.link;
    let lit = active
        .and_then(|note| model.notes.get(note))
        .is_some_and(|note| note.span.contains(model.number(index)));
    let edge = if lit {
        accent
    } else if model.owner[index].is_some() {
        accent.at(Opacity::AnnotationEdge)
    } else {
        accent.at(Opacity::Clear)
    };
    let mut style = Style::new().min_width(Length::Px(0.0)).gpui_border(
        Insets {
            left: 2.0,
            ..Insets::default()
        },
        edge.to_reference_paint(),
        false,
    );
    if lit {
        style = style.gpui_background(GpuiBackground::Solid(
            accent.at(Opacity::ToneTint).to_reference_paint(),
        ));
    }
    let mut row = UiBox::new()
        .width(Length::Pct(100.0))
        .height(LINE_HEIGHT)
        .clip(true)
        .style(style);
    if !model.notes.is_empty() {
        let mut marker = UiBox::new().width(24.0).style(
            Style::new()
                .flex_shrink(0.0)
                .justify(JustifyContent::CENTER)
                .padding_insets(Insets {
                    top: 2.0,
                    ..Insets::default()
                }),
        );
        if let Some(note) = model.marker[index] {
            marker = marker.child(badge(theme, note + 1, active == Some(note)));
        }
        row = row.child(marker.build());
    }
    let style = text_style(
        theme,
        true,
        TypographyTokens::size(TextStep::XsMid),
        LINE_HEIGHT,
        FontWeight::NORMAL,
    );
    row = row
        .child(
            UiBox::new()
                .width(gutter)
                .style(
                    Style::new()
                        .flex_shrink(0.0)
                        .justify(JustifyContent::END)
                        .padding_insets(Insets {
                            right: 12.0,
                            ..Insets::default()
                        }),
                )
                .child(
                    Text::new(model.number(index).to_string())
                        .text_style(style.clone())
                        .gpui_source_color(theme.colors.text_faint.to_reference_text_color())
                        .wrap(false)
                        .build(),
                )
                .build(),
        )
        .child(
            UiBox::new()
                .style(
                    Style::new()
                        .flex_grow(1.0)
                        .min_width(Length::Px(0.0))
                        .min_height(Length::Px(LINE_HEIGHT))
                        .padding_insets(Insets {
                            right: 12.0,
                            ..Insets::default()
                        }),
                )
                .child(
                    Text::new(model.lines[index].clone())
                        .text_style(style)
                        .gpui_source_color(theme.colors.text.to_reference_text_color())
                        .wrap(false)
                        .build(),
                )
                .build(),
        );
    hover(
        row.build().with_key(format!("{id}-line-{index}")),
        state,
        Arc::clone(model),
        index,
        false,
        invalidate,
    )
}

fn note_row(
    theme: &ArtisanTheme,
    id: &str,
    model: &Arc<Model>,
    index: usize,
    active: Option<usize>,
    scope: Option<ResourceScope>,
    instance: Option<crate::core::HostInstance>,
    open: Open,
    state: Rc<RefCell<AnnotatedState>>,
    invalidate: Rc<dyn Fn()>,
) -> Element {
    let note = &model.notes[index];
    let on = active == Some(index);
    let label = if note.span.start == note.span.end {
        format!("L{}", note.span.start)
    } else {
        format!("L{}–{}", note.span.start, note.span.end)
    };
    let mut style = Style::new()
        .align(AlignItems::FLEX_START)
        .padding_insets(Insets {
            left: 6.0,
            right: 6.0,
            top: 4.0,
            bottom: 4.0,
        });
    if on {
        style = style.gpui_background(GpuiBackground::Solid(
            theme
                .colors
                .link
                .at(Opacity::NoteHover)
                .to_reference_paint(),
        ));
    }
    let mut row = UiBox::new()
        .gap(8.0)
        .radius(RadiusTokens::value(RadiusStep::Sm))
        .style(style)
        .child(
            UiBox::new()
                .style(Style::new().padding_insets(Insets {
                    top: 2.0,
                    ..Insets::default()
                }))
                .child(badge(theme, index + 1, on))
                .build(),
        );
    if let Some(path) = &model.path {
        let action = OpenReference {
            scope,
            path: path.clone(),
            line: Some(note.span.start),
        };
        let owner = Rc::downgrade(&state);
        let rendered = Arc::downgrade(model);
        row = row.child(
            Button::text(format!("{id}-note-reference-{index}"), *theme, label)
                .variant(ButtonVariant::Ghost)
                .on_activate(move |_| {
                    if owner
                        .upgrade()
                        .zip(rendered.upgrade())
                        .is_some_and(|(state, model)| Arc::ptr_eq(&state.borrow().model, &model))
                    {
                        open(action.clone(), instance);
                    }
                })
                .build(),
        );
    } else {
        row = row.child(
            Text::new(label)
                .text_style(text_style(
                    theme,
                    false,
                    TypographyTokens::size(TextStep::Xs),
                    16.0,
                    FontWeight::MEDIUM,
                ))
                .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                .build(),
        );
    }
    row = row.child(
        UiBox::new()
            .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
            .child(
                Text::new(note.text.clone())
                    .text_style(text_style(
                        theme,
                        false,
                        TypographyTokens::size(TextStep::Sm),
                        20.0,
                        FontWeight::NORMAL,
                    ))
                    .gpui_source_color(theme.colors.text.to_reference_text_color())
                    .wrap(true)
                    .build(),
            )
            .build(),
    );
    hover(
        row.build().with_key(format!("{id}-note-{index}")),
        state,
        Arc::clone(model),
        index,
        true,
        invalidate,
    )
}

#[cfg(test)]
#[path = "../../../../../tests/ui/rich/code/annotated.rs"]
mod tests;
