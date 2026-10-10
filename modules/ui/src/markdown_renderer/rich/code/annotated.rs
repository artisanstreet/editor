//! `annotated-code`: code with numbered notes.
//!
//! The code sits on the card like any fence; the notes follow it as plain
//! numbered lines, with a badge beside the first line each covers.
//! Hovering a note highlights the lines it covers; hovering a covered line
//! highlights its note. Hover state is tracked per source (line or note),
//! so moving between two parts never clears the other's highlight out of
//! order.

use std::fmt::Write as _;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Div, Entity, IntoElement, ParentElement as _, RenderOnce, SharedString,
    Styled as _, Window, div, prelude::*, px,
};
use serde::Deserialize;

use super::super::actions::open_reference;
use super::super::kit::{self, LineSpan, Tone};
use super::super::{BlockContext, Chrome, RichBlock};
use super::{code_lines, code_panel, fence, gutter_width, hover_slot, line_number, path_link};
use crate::theme::ArtisanTheme;

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
    language: Option<String>,
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
    start: u32,
    language: Option<String>,
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
pub(super) struct AnnotatedCode(Rc<Model>);

impl From<Spec> for AnnotatedCode {
    fn from(spec: Spec) -> Self {
        let lines = code_lines(&spec.code);
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
        for (index, note) in notes.iter().enumerate() {
            for (line, slot) in owner.iter_mut().enumerate() {
                let number = start.saturating_add(u32::try_from(line).unwrap_or(u32::MAX));
                if note.span.contains(number) && slot.is_none() {
                    *slot = Some(index);
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
        Self(Rc::new(Model {
            lines,
            start,
            language: spec
                .language
                .map(|language| language.trim().to_owned())
                .filter(|language| !language.is_empty()),
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
    fn render(&self, cx: &BlockContext<'_>) -> AnyElement {
        AnnotatedView {
            key: cx.key("annotated"),
            theme: *cx.theme,
            model: Rc::clone(&self.0),
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

    fn markdown(&self) -> String {
        let model = &self.0;
        let mut out = String::new();
        if let Some(path) = &model.path {
            let _ = writeln!(out, "`{path}:{}`\n", model.start);
        }
        let body = model
            .lines
            .iter()
            .map(SharedString::as_ref)
            .collect::<Vec<_>>()
            .join("\n");
        out.push_str(&fence(model.language.as_deref(), &body));
        if !model.notes.is_empty() {
            out.push('\n');
            for (index, note) in model.notes.iter().enumerate() {
                let _ = writeln!(
                    out,
                    "{}. **{}** {}",
                    index + 1,
                    span_text(note.span),
                    note.text
                );
            }
        }
        out
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

#[derive(Default)]
struct HoverState {
    line: Option<usize>,
    note: Option<usize>,
}

#[derive(IntoElement)]
struct AnnotatedView {
    key: SharedString,
    theme: ArtisanTheme,
    model: Rc<Model>,
}

impl AnnotatedView {
    fn id(&self, part: impl std::fmt::Display) -> gpui::ElementId {
        gpui::ElementId::Name(SharedString::from(format!("{}-{part}", self.key)))
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
        .rounded(px(8.0))
        .text_size(px(10.0))
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
    fn code(&self, state: &Entity<HoverState>, active: Option<usize>) -> Div {
        let theme = &self.theme;
        let model = &self.model;
        let accent = Tone::Accent.color(theme);
        let gutter = gutter_width(model.number(model.lines.len().saturating_sub(1)));
        let has_notes = !model.notes.is_empty();
        let rows = model.lines.iter().enumerate().map(|(index, text)| {
            let owner = model.owner.get(index).copied().flatten();
            let lit = active
                .and_then(|note| model.notes.get(note))
                .is_some_and(|note| note.span.contains(model.number(index)));
            let marker = model.marker.get(index).copied().flatten();
            div()
                .id(self.id(format!("line-{index}")))
                .w_full()
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
                            .children(
                                marker.map(|note| badge(theme, note + 1, active == Some(note))),
                            ),
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
        });
        code_panel(theme).children(rows)
    }

    /// The numbered notes under the code; a note opens its first line on
    /// click.
    fn notes(&self, state: &Entity<HoverState>, active: Option<usize>) -> Div {
        let theme = self.theme;
        let model = &self.model;
        div()
            .w_full()
            .min_w_0()
            .pt(px(8.0))
            .border_t_1()
            .border_color(kit::hairline(&theme))
            .flex()
            .flex_col()
            .children(model.notes.iter().enumerate().map(|(index, entry)| {
                let on = active == Some(index);
                let path = model.path.clone();
                let line = entry.span.start;
                div()
                    .id(self.id(format!("note-{index}")))
                    .px(px(6.0))
                    .py(px(4.0))
                    .flex()
                    .items_start()
                    .gap(px(8.0))
                    .rounded(px(6.0))
                    .when(path.is_some(), gpui::Styled::cursor_pointer)
                    .when(on, |row| {
                        row.bg(Tone::Accent.color(&theme).with_alpha(0.08).to_paint())
                    })
                    .child(div().pt(px(2.0)).child(badge(&theme, index + 1, on)))
                    .child(
                        div()
                            .flex_shrink_0()
                            .min_w(px(36.0))
                            .text_size(px(13.0))
                            .line_height(px(20.0))
                            .font_family(theme.typography.mono.family)
                            .tracking(0.0)
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child(span_text(entry.span)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(14.0))
                            .line_height(px(20.0))
                            .text_color(theme.colors.foreground.to_paint())
                            .child(entry.text.clone()),
                    )
                    .on_hover(hover_slot(state.clone(), index, |state| &mut state.note))
                    .on_click(move |_, window, cx| {
                        if let Some(path) = &path {
                            open_reference(path, Some(line), window, cx);
                        }
                    })
            }))
    }

    /// The path, opening at the first line.
    fn head(&self) -> Option<gpui::Stateful<Div>> {
        let model = &self.model;
        model.path.as_ref().map(|path| {
            path_link(
                &self.theme,
                self.id("path"),
                path,
                Some(model.start),
                format!("{path}:{}", model.start),
            )
        })
    }
}

impl RenderOnce for AnnotatedView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = kit::keyed(window, cx, self.id("hover"), HoverState::default);
        let (line, note) = {
            let read = state.read(cx);
            (read.line, read.note)
        };
        let model = &self.model;
        let active =
            note.or_else(|| line.and_then(|line| model.owner.get(line).copied().flatten()));
        let notes = (!model.notes.is_empty()).then(|| self.notes(&state, active));
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
