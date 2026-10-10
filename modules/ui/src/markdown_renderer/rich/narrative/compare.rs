//! `compare`: options against criteria, drawn as a plain table on the
//! transcript. The recommended option is named in the accent above its
//! column; hovering a column highlights it.

use gpui::{
    AnyElement, App, Div, Entity, Hsla, IntoElement, ParentElement as _, RenderOnce, SharedString,
    Stateful, Styled as _, Window, div, prelude::*, px,
};
use serde::Deserialize;

use super::super::kit::{self, Tone};
use super::super::{BlockContext, RichBlock};
use super::part;
use crate::theme::ArtisanTheme;

/// The `compare` spec.
#[derive(Debug, Deserialize)]
pub(super) struct Compare {
    options: Vec<String>,
    rows: Vec<Row>,
    #[serde(default)]
    recommended: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct Row {
    criterion: String,
    #[serde(default)]
    values: Vec<String>,
}

impl Compare {
    /// The recommended option's column, matched without regard to case.
    fn recommended(&self) -> Option<usize> {
        let wanted = self.recommended.as_deref()?.trim();
        self.options
            .iter()
            .position(|option| option.trim().eq_ignore_ascii_case(wanted))
    }
}

impl RichBlock for Compare {
    fn render(&self, cx: &BlockContext<'_>) -> AnyElement {
        CompareView {
            id: cx.key("compare"),
            theme: *cx.theme,
            options: self.options.clone(),
            rows: self.rows.clone(),
            recommended: self.recommended(),
        }
        .into_any_element()
    }

    fn markdown(&self) -> String {
        let recommended = self.recommended();
        let mut header = vec![String::new()];
        header.extend(self.options.iter().enumerate().map(|(index, option)| {
            if Some(index) == recommended {
                format!("{} (recommended)", option.trim())
            } else {
                option.trim().to_owned()
            }
        }));
        let rows: Vec<Vec<String>> = self
            .rows
            .iter()
            .map(|row| {
                let mut cells = vec![format!("**{}**", row.criterion.trim())];
                cells.extend((0..self.options.len()).map(|index| {
                    row.values
                        .get(index)
                        .map_or_else(String::new, |value| value.trim().to_owned())
                }));
                cells
            })
            .collect();
        kit::markdown_table(&header, &rows)
    }
}

#[derive(IntoElement)]
struct CompareView {
    id: SharedString,
    theme: ArtisanTheme,
    options: Vec<String>,
    rows: Vec<Row>,
    recommended: Option<usize>,
}

#[derive(Default)]
struct HoverState {
    column: Option<usize>,
}

/// What every cell needs to draw and report hover.
struct Grid {
    id: SharedString,
    theme: ArtisanTheme,
    state: Entity<HoverState>,
    hovered: Option<usize>,
    recommended: Option<usize>,
}

impl Grid {
    /// The tint of `column` while it is hovered. A column spans many
    /// cells, so it takes the hover fill's lower stop as one flat tint
    /// instead of a gradient per cell.
    fn tint(&self, column: usize) -> Option<Hsla> {
        (self.hovered == Some(column)).then(|| self.theme.interaction.hover_fill_bottom.to_paint())
    }

    fn cell(&self, row: &str, column: usize) -> Stateful<Div> {
        let state = self.state.clone();
        div()
            .id(part(&self.id, format!("{row}-{column}")))
            .flex_1()
            .flex_basis(px(0.0))
            .min_w_0()
            .px(px(10.0))
            .py(px(7.0))
            .when_some(self.tint(column), gpui::Styled::bg)
            .on_hover(move |hovered: &bool, _, cx| {
                state.update(cx, |state, cx| {
                    if *hovered {
                        state.column = Some(column);
                    } else if state.column == Some(column) {
                        state.column = None;
                    }
                    cx.notify();
                });
            })
    }
}

impl RenderOnce for CompareView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let state = kit::keyed(window, cx, part(&self.id, "state"), HoverState::default);
        let hovered = state.read(cx).column;
        let grid = Grid {
            id: self.id,
            theme,
            state,
            hovered,
            recommended: self.recommended,
        };
        let columns = self.options.len();
        let criterion = |text: SharedString| {
            div()
                .w(px(140.0))
                .flex_shrink_0()
                .px(px(10.0))
                .py(px(7.0))
                .child(kit::muted(&theme, text))
        };
        let header = div()
            .flex()
            .items_end()
            .child(criterion(SharedString::default()))
            .children(self.options.iter().enumerate().map(|(column, option)| {
                let recommended = grid.recommended == Some(column);
                let accent = Tone::Accent.color(&theme).to_paint();
                grid.cell("option", column).child(
                    div()
                        .flex()
                        .flex_col()
                        .items_start()
                        .when(recommended, |head| {
                            head.child(kit::label(&theme, "Recommended").text_color(accent))
                        })
                        .child(
                            kit::title(&theme, option.trim().to_owned())
                                .when(recommended, |title| title.text_color(accent)),
                        ),
                )
            }));
        div().w_full().flex().flex_col().child(header).children(
            self.rows.into_iter().enumerate().map(|(index, row)| {
                div()
                    .flex()
                    .border_t_1()
                    .border_color(kit::hairline(&theme))
                    .child(criterion(SharedString::from(
                        row.criterion.trim().to_owned(),
                    )))
                    .children((0..columns).map(|column| {
                        let value = row.values.get(column).map_or("", |value| value.trim());
                        grid.cell(&format!("row{index}"), column)
                            .child(if value.is_empty() {
                                kit::muted(&theme, "–")
                            } else {
                                kit::body(&theme, value.to_owned())
                                    .text_size(px(13.0))
                                    .line_height(px(20.0))
                            })
                    }))
            }),
        )
    }
}
