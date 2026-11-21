use std::{cell::RefCell, rc::Rc, sync::Arc};

use aqua_core::{GpuiBackground, Insets};
use aqua_ui::{
    AlignItems, JustifyContent,
    collection::List,
    element::{Box as UiBox, Element, Scroll, Text},
    style::{Length, Style},
};
use editor_assets::AssetId;

use super::{AnnotatedCode, Model};
use crate::ui::{
    kit::{
        Button, ButtonSize, ButtonTone, Corner, Density, Divider, Ellipsis, Hover, Label, Row,
        Space, Spaced, Surface, TextRole,
    },
    rich::actions::{OpenReference, ResourceScope},
    theme::{ArtisanTheme, Opacity},
};

const NOTE: Row = Row::ListItem(Density::Dense);
const BADGE: f32 = 16.0;
const MARKER: f32 = 24.0;
const EDGE: f32 = 2.0;

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

    fn update_model(&mut self, model: &AnnotatedCode) -> bool {
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

type Open = Rc<dyn Fn(OpenReference, Option<editor_core::HostInstance>)>;

impl AnnotatedCode {
    pub(crate) fn copy(&self) -> String {
        self.0.lines.join("\n")
    }

    #[expect(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "the annotated code view takes its reference scope, opener, state and invalidation"
    )]
    pub(crate) fn render(
        &self,
        theme: &ArtisanTheme,
        id: &str,
        scope: Option<ResourceScope>,
        instance: Option<editor_core::HostInstance>,
        open: Open,
        state: Rc<RefCell<AnnotatedState>>,
        invalidate: Rc<dyn Fn()>,
    ) -> Element {
        state.borrow_mut().update_model(self);
        let model = Arc::clone(&self.0);
        let active = state.borrow().active();
        let mut body = UiBox::new()
            .layout("column")
            .spaced(Space::S8)
            .width(Length::Pct(100.0))
            .style(Style::new().min_width(Length::Px(0.0)));
        if let Some(path) = &model.path {
            let action = OpenReference {
                scope: scope.clone(),
                path: path.clone(),
                line: Some(model.start),
            };
            let open = Rc::clone(&open);
            let owner = Rc::downgrade(&state);
            let rendered = Arc::downgrade(&model);
            body = body.child(
                Button::new(
                    format!("{id}-path"),
                    *theme,
                    ButtonTone::Quiet,
                    ButtonSize::Md,
                    Label::IconText(AssetId::TABLER_FILE_TEXT, format!("{path}:{}", model.start)),
                )
                .ellipsis(Ellipsis::Start)
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
        .mul_add(Space::S8.px(), Space::S8.px());
        let width = model.width
            + gutter
            + Space::S12.px()
            + if model.notes.is_empty() { 0.0 } else { MARKER };
        let height = crate::ui::rich::charts::scale::f32_of_count(count.min(18))
            * TextRole::Code.line_px(theme);
        let row_model = Arc::clone(&model);
        let row_state = Rc::clone(&state);
        let row_invalidate = Rc::clone(&invalidate);
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
            .estimated_row_height(Length::Px(TextRole::Code.line_px(theme)))
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
                    Rc::clone(&row_state),
                    Rc::clone(&row_invalidate),
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
            let note_height = 2.0f32.mul_add(NOTE.pad().y.px(), TextRole::Label.line_px(theme));
            let height = crate::ui::rich::charts::scale::f32_of_count(model.notes.len().min(6))
                * note_height;
            let note_model = Arc::clone(&model);
            let note_theme = *theme;
            let note_id = id.to_owned();
            let notes = List::new()
                .items(0..model.notes.len())
                .key(|index| *index)
                .virtualize()
                .estimated_row_height(Length::Px(note_height))
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
                        Rc::clone(&open),
                        Rc::clone(&state),
                        Rc::clone(&invalidate),
                    )
                })
                .build()
                .with_key(format!("{id}-notes-{:p}", Arc::as_ptr(&model)));
            body = body.child(Divider::Rule.build(theme, 0.0)).child(notes);
        }
        body.build()
    }
}

fn badge(theme: &ArtisanTheme, number: usize, active: bool) -> Element {
    let accent = theme.colors.link;
    UiBox::new()
        .width(BADGE)
        .height(BADGE)
        .radius(Corner::Full.resolve(BADGE))
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
                .text_style(TextRole::Caption.style(theme))
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

#[expect(
    clippy::too_many_arguments,
    reason = "a code row reads its model line, gutter, active note, state and invalidation"
)]
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
    let line = TextRole::Code.line_px(theme);
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
            left: EDGE,
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
        .height(line)
        .spaced(Space::S12)
        .clip(true)
        .style(style);
    if !model.notes.is_empty() {
        let mut marker = UiBox::new().width(MARKER).style(
            Style::new()
                .flex_shrink(0.0)
                .align(AlignItems::CENTER)
                .justify(JustifyContent::CENTER),
        );
        if let Some(note) = model.marker[index] {
            marker = marker.child(badge(theme, note + 1, active == Some(note)));
        }
        row = row.child(marker.build());
    }
    let style = TextRole::Code.style(theme);
    row = row
        .child(
            UiBox::new()
                .width(gutter)
                .style(Style::new().flex_shrink(0.0).justify(JustifyContent::END))
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
                        .min_height(Length::Px(line)),
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

#[expect(
    clippy::too_many_arguments,
    reason = "a note row reads its model note, active state, reference scope, opener and state"
)]
fn note_row(
    theme: &ArtisanTheme,
    id: &str,
    model: &Arc<Model>,
    index: usize,
    active: Option<usize>,
    scope: Option<ResourceScope>,
    instance: Option<editor_core::HostInstance>,
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
    let mut row = UiBox::new()
        .spaced(NOTE.gap())
        .padded(NOTE.pad().y)
        .radius(Row::radius_in(Surface::Card))
        .style(Style::new().align(AlignItems::FLEX_START));
    if on {
        row = row.style(Hover::Neutral.style(theme));
    }
    row = row.child(
        UiBox::new()
            .height(TextRole::Label.line_px(theme))
            .style(Style::new().align(AlignItems::CENTER))
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
            Button::text(
                format!("{id}-note-reference-{index}"),
                *theme,
                ButtonTone::Quiet,
                label,
            )
            .size(ButtonSize::Sm)
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
                .text_style(TextRole::Caption.style(theme))
                .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                .build(),
        );
    }
    row = row.child(
        UiBox::new()
            .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
            .child(
                Text::new(note.text.clone())
                    .text_style(TextRole::Label.style(theme))
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
