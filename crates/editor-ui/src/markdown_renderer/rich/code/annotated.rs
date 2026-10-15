//! `annotated-code`: code with numbered notes.
//!
//! The code sits on the card like any fence; the notes follow it as plain
//! numbered lines, with a badge beside the first line each covers.
//! Hovering a note highlights the lines it covers; hovering a covered line
//! highlights its note. Hover state is tracked per source (line or note),
//! so moving between two parts never clears the other's highlight out of
//! order.

use crate::markdown_renderer::rich::budget;
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens, TextStep, TypographyTokens};
use std::sync::Arc;

use gpui::{
    AnyElement, App, Div, Entity, IntoElement, ParentElement as _, RenderOnce, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::*, px,
};
use serde::Deserialize;

use super::super::actions::open_reference;
use super::super::kit::{self, LineSpan, Tone};
use super::super::{BlockContext, Chrome, RichBlock};
use super::{
    code_lines, code_panel, excerpt_width, gutter_width, hover_slot, line_number, path_link,
};
use crate::button::{Button, ButtonVariant};

#[derive(Debug, Deserialize)]
struct NoteSpec {
    lines: LineSpan,
    text: String,
}

const fn first_line() -> u32 {
    1
}

#[derive(Debug, Deserialize)]
struct Spec {
    code: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default = "first_line")]
    start_line: u32,
    #[serde(default)]
    notes: Vec<NoteSpec>,
}

#[derive(Debug)]
struct Note {
    /// The lines covered, in the excerpt's own numbering.
    span: LineSpan,
    text: SharedString,
}

#[derive(Debug)]
struct Model {
    lines: Vec<SharedString>,
    width: gpui::Pixels,
    start: u32,
    path: Option<SharedString>,
    notes: Vec<Note>,
    /// Per line: the first note covering it.
    owner: Vec<Option<usize>>,
    /// Per line: the note whose span starts on it, marked in the margin.
    marker: Vec<Option<usize>>,
}

impl Model {
    fn number(&self, index: usize) -> u32 {
        self.start
            .saturating_add(u32::try_from(index).unwrap_or(u32::MAX))
    }
}

/// The `annotated-code` spec, prepared once.
#[derive(Debug, Deserialize)]
#[serde(from = "Spec")]
pub(super) struct AnnotatedCode(Arc<Model>);

impl From<Spec> for AnnotatedCode {
    fn from(spec: Spec) -> Self {
        let lines = code_lines(&spec.code);
        let width = excerpt_width(&lines);
        let start = spec.start_line.max(1);
        let count = u32::try_from(lines.len()).unwrap_or(u32::MAX);
        let last = start.saturating_add(count.saturating_sub(1));
        let notes: Vec<Note> = spec
            .notes
            .into_iter()
            .filter(|note| !note.text.trim().is_empty())
            .map(|note| {
                let mut span = note.lines;
                // A span outside the excerpt that fits it counted from one
                // was written relative to the excerpt.
                let outside = span.end < start || span.start > last;
                if outside && start > 1 && span.start >= 1 && span.end <= count {
                    span = LineSpan {
                        start: span.start + start - 1,
                        end: span.end + start - 1,
                    };
                }
                Note {
                    span,
                    text: SharedString::from(note.text.trim().to_owned()),
                }
            })
            .collect();
        let mut owner = vec![None; lines.len()];
        let mut marker = vec![None; lines.len()];
        let mut next_free: Vec<usize> = (0..=lines.len()).collect();
        fn find(next: &mut [usize], mut index: usize) -> usize {
            let origin = index;
            while next[index] != index {
                index = next[index];
            }
            let root = index;
            index = origin;
            while next[index] != index {
                let parent = next[index];
                next[index] = root;
                index = parent;
            }
            root
        }
        for (index, note) in notes.iter().enumerate() {
            if note.span.end >= start {
                let first = usize::try_from(note.span.start.saturating_sub(start))
                    .unwrap_or(usize::MAX)
                    .min(lines.len());
                let end = usize::try_from(note.span.end.saturating_sub(start))
                    .unwrap_or(usize::MAX)
                    .saturating_add(1)
                    .min(lines.len());
                let mut at = find(&mut next_free, first);
                while at < end {
                    owner[at] = Some(index);
                    next_free[at] = find(&mut next_free, at + 1);
                    at = next_free[at];
                }
            }
            let first = note.span.start.max(start);
            let at = usize::try_from(first - start).unwrap_or(usize::MAX);
            if note.span.end >= start
                && let Some(slot) = marker.get_mut(at)
                && slot.is_none()
            {
                *slot = Some(index);
            }
        }
        Self(Arc::new(Model {
            lines,
            width,
            start,
            path: spec
                .path
                .map(|path| path.trim().to_owned())
                .filter(|path| !path.is_empty())
                .map(SharedString::from),
            notes,
            owner,
            marker,
        }))
    }
}

impl RichBlock for AnnotatedCode {
    fn resident_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + std::mem::size_of::<Model>()
            + budget::vector(&self.0.lines)
            + self.0.lines.iter().map(|line| line.len()).sum::<usize>()
            + self.0.path.as_ref().map_or(0, |text| text.len())
            + budget::vector(&self.0.notes)
            + self
                .0
                .notes
                .iter()
                .map(|note| note.text.len())
                .sum::<usize>()
            + budget::vector(&self.0.owner)
            + budget::vector(&self.0.marker)
    }

    fn render(&self, cx: &BlockContext<'_>) -> AnyElement {
        AnnotatedView {
            key: cx.key("annotated"),
            scope: cx.media.and_then(|provider| provider.resource_scope()),
            theme: *cx.theme,
            model: Arc::clone(&self.0),
        }
        .into_any_element()
    }

    fn chrome(&self) -> Chrome {
        Chrome::Card
    }

    fn copy(&self) -> Option<String> {
        Some(
            self.0
                .lines
                .iter()
                .map(SharedString::as_ref)
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }
}

/// `L4` or `L3–5`.
fn span_text(span: LineSpan) -> String {
    if span.start == span.end {
        format!("L{}", span.start)
    } else {
        format!("L{}–{}", span.start, span.end)
    }
}

struct HoverState {
    model: Arc<Model>,
    notes: gpui::ListState,
    line: Option<usize>,
    note: Option<usize>,
}

#[derive(Clone, IntoElement)]
struct AnnotatedView {
    scope: Option<super::super::actions::ResourceScope>,
    key: SharedString,
    theme: ArtisanTheme,
    model: Arc<Model>,
}

impl AnnotatedView {
    fn id(&self, part: impl std::fmt::Display) -> gpui::ElementId {
        kit::part(&self.key, part)
    }
}

/// A note's number in a small circle, filled when `active`.
fn badge(theme: &ArtisanTheme, number: usize, active: bool) -> Div {
    let color = Tone::Accent.color(theme);
    div()
        .size(px(16.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(RadiusTokens::value(RadiusStep::Md))
        .text_size(TypographyTokens::size(TextStep::X3s))
        .line_height(px(16.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .bg(color.with_alpha(if active { 0.9 } else { 0.18 }).to_paint())
        .text_color(if active {
            theme.colors.background.to_paint()
        } else {
            color.to_paint()
        })
        .child(SharedString::from(number.to_string()))
}

impl AnnotatedView {
    /// The code lines, lit where the active note covers them.
    fn code(&self, state: &Entity<HoverState>, active: Option<usize>) -> gpui::Stateful<Div> {
        let theme = &self.theme;
        let model = &self.model;
        let accent = Tone::Accent.color(theme);
        let gutter = gutter_width(model.number(model.lines.len().saturating_sub(1)));
        let has_notes = !model.notes.is_empty();
        let id = self.id("code-rows");
        let panel_id = self.id("code-scroll");
        let width = model.width + gutter + px(if has_notes { 24.0 } else { 0.0 });
        let count = model.lines.len();
        let view = self.clone();
        let state = state.clone();
        let rows =
            gpui::uniform_list(id, count, move |range, _, _| {
                let model = &view.model;
                let theme = &view.theme;
                range
                    .map(|index| {
                        let text = &model.lines[index];
                        let owner = model.owner.get(index).copied().flatten();
                        let lit = active
                            .and_then(|note| model.notes.get(note))
                            .is_some_and(|note| note.span.contains(model.number(index)));
                        let marker = model.marker.get(index).copied().flatten();
                        div()
                            .id(view.id(format!("line-{index}")))
                            .w_full()
                            .h(px(20.0))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .min_w_0()
                            .flex()
                            .border_l_2()
                            .border_color(match (lit, owner.is_some()) {
                                (true, _) => accent.to_paint(),
                                (false, true) => accent.with_alpha(0.3).to_paint(),
                                (false, false) => gpui::transparent_black(),
                            })
                            .when(lit, |row| row.bg(accent.with_alpha(0.1).to_paint()))
                            .when(has_notes, |row| {
                                row.child(
                                    div()
                                        .w(px(24.0))
                                        .flex_shrink_0()
                                        .flex()
                                        .justify_center()
                                        .pt(px(2.0))
                                        .children(marker.map(|note| {
                                            badge(theme, note + 1, active == Some(note))
                                        })),
                                )
                            })
                            .child(line_number(theme, Some(model.number(index)), gutter))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .min_h(px(20.0))
                                    .pr(px(12.0))
                                    .child(text.clone()),
                            )
                            .on_hover(hover_slot(state.clone(), index, |state| &mut state.line))
                    })
                    .collect::<Vec<_>>()
            })
            .w_full()
            .min_w(width)
            .h(px(count.min(18) as f32 * 20.0));
        code_panel(theme)
            .id(panel_id)
            .overflow_x_scroll()
            .child(rows)
    }

    /// The numbered notes under the code; a note opens its first line on
    /// click.
    fn notes(
        &self,
        state: &Entity<HoverState>,
        active: Option<usize>,
        rows: gpui::ListState,
    ) -> Div {
        let theme = self.theme;
        let model = self.model.clone();
        let view = self.clone();
        let state = state.clone();
        let height = px(model.notes.len().min(6) as f32 * 40.0);
        div()
            .w_full()
            .min_w_0()
            .pt(px(8.0))
            .border_t_1()
            .border_color(kit::hairline(&theme))
            .child(
                gpui::list(rows, move |index, _, _| {
                    let Some(entry) = model.notes.get(index) else {
                        return div().into_any_element();
                    };
                    let on = active == Some(index);
                    let path = model.path.clone();
                    let scope = view.scope.clone();
                    let line = entry.span.start;
                    div()
                        .id(view.id(format!("note-{index}")))
                        .px(px(6.0))
                        .py(px(4.0))
                        .flex()
                        .items_start()
                        .gap(px(8.0))
                        .rounded(RadiusTokens::value(RadiusStep::Sm))
                        .when(on, |row| {
                            row.bg(Tone::Accent.color(&theme).with_alpha(0.08).to_paint())
                        })
                        .child(div().pt(px(2.0)).child(badge(&theme, index + 1, on)))
                        .child(if let Some(path) = path {
                            Button::text(
                                view.id(format!("note-reference-{index}")),
                                theme,
                                span_text(entry.span),
                            )
                            .variant(ButtonVariant::Ghost)
                            .on_activate(move |_, window, cx| {
                                open_reference(&path, Some(line), scope.clone(), window, cx)
                            })
                            .into_any_element()
                        } else {
                            kit::label(&theme, span_text(entry.span)).into_any_element()
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(TypographyTokens::size(TextStep::Sm))
                                .line_height(px(20.0))
                                .text_color(theme.colors.foreground.to_paint())
                                .child(entry.text.clone()),
                        )
                        .on_hover(hover_slot(state.clone(), index, |state| &mut state.note))
                        .into_any_element()
                })
                .w_full()
                .h(height),
            )
    }

    /// The path, opening at the first line.
    fn head(&self) -> Option<Button> {
        let model = &self.model;
        model.path.as_ref().map(|path| {
            path_link(
                &self.theme,
                self.id("path"),
                path,
                Some(model.start),
                self.scope.clone(),
                format!("{path}:{}", model.start),
            )
        })
    }
}

impl RenderOnce for AnnotatedView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = kit::keyed(window, cx, self.id("hover"), || HoverState {
            model: self.model.clone(),
            notes: gpui::ListState::new(self.model.notes.len(), gpui::ListAlignment::Top, px(96.0)),
            line: None,
            note: None,
        });
        state.update(cx, |state, _| {
            if !Arc::ptr_eq(&state.model, &self.model) {
                state.model = self.model.clone();
                state.notes.reset(self.model.notes.len());
                state.line = None;
                state.note = None;
            }
        });
        let rows = state.read(cx).notes.clone();
        let (line, note) = {
            let read = state.read(cx);
            (read.line, read.note)
        };
        let model = &self.model;
        let active =
            note.or_else(|| line.and_then(|line| model.owner.get(line).copied().flatten()));
        let notes = (!model.notes.is_empty()).then(|| self.notes(&state, active, rows));
        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .children(self.head())
            .child(self.code(&state, active))
            .children(notes)
    }
}
