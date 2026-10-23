//! `compare`: options against criteria, drawn as a plain table on the
//! transcript. The recommended option is named in the accent above its
//! column; hovering a column highlights it.

use crate::markdown_renderer::rich::budget;
use crate::theme::{ArtisanTheme, TextStep, TypographyTokens};
use gpui::{
    AnyElement, App, Div, Entity, Hsla, IntoElement, ParentElement as _, RenderOnce, SharedString,
    Stateful, Styled as _, Window, div, prelude::*, px,
};
use serde::Deserialize;
use std::sync::Arc;

use super::super::kit::{self, Tone};
use super::super::{BlockContext, RichBlock};
use super::part;

/// The `compare` spec.
#[derive(Debug, Deserialize)]
struct CompareSpec {
    options: Vec<String>,
    rows: Vec<RowSpec>,
    #[serde(default)]
    recommended: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct RowSpec {
    criterion: String,
    #[serde(default)]
    values: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(try_from = "CompareSpec")]
pub(super) struct Compare(Arc<CompareData>);
#[derive(Debug)]
struct CompareData {
    options: Vec<SharedString>,
    rows: Vec<Row>,
    recommended: Option<usize>,
}
#[derive(Debug)]
struct Row {
    criterion: SharedString,
    values: Vec<SharedString>,
}
impl TryFrom<CompareSpec> for Compare {
    type Error = String;
    fn try_from(spec: CompareSpec) -> Result<Self, String> {
        if spec.options.is_empty() || spec.options.len() > 16 || spec.rows.len() > 4096 {
            return Err("comparison preview needs 1–16 options and at most 4096 criteria".into());
        }
        let recommended = spec.recommended.as_ref().and_then(|wanted| {
            spec.options
                .iter()
                .position(|option| option.trim().eq_ignore_ascii_case(wanted.trim()))
        });
        let text = |text: String| SharedString::from(text.trim().to_owned());
        Ok(Self(Arc::new(CompareData {
            options: spec.options.into_iter().map(text).collect(),
            rows: spec
                .rows
                .into_iter()
                .map(|row| Row {
                    criterion: text(row.criterion),
                    values: row.values.into_iter().map(text).collect(),
                })
                .collect(),
            recommended,
        })))
    }
}

impl RichBlock for Compare {
    fn resident_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + std::mem::size_of::<CompareData>()
            + budget::vector(&self.0.options)
            + self.0.options.iter().map(|text| text.len()).sum::<usize>()
            + budget::vector(&self.0.rows)
            + self
                .0
                .rows
                .iter()
                .map(|row| {
                    row.criterion.len()
                        + budget::vector(&row.values)
                        + row.values.iter().map(|text| text.len()).sum::<usize>()
                })
                .sum::<usize>()
    }
    fn render(&self, cx: &BlockContext<'_>) -> AnyElement {
        CompareView {
            id: cx.key("compare"),
            theme: *cx.theme,
            data: Arc::clone(&self.0),
        }
        .into_any_element()
    }
}

#[derive(IntoElement)]
struct CompareView {
    id: SharedString,
    theme: ArtisanTheme,
    data: Arc<CompareData>,
}

struct HoverState {
    column: Option<usize>,
    data: Arc<CompareData>,
    rows: gpui::ListState,
}

/// What every cell needs to draw and report hover.
#[derive(Clone)]
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
        (self.hovered == Some(column)).then(|| {
            self.theme
                .colors
                .text
                .at(crate::theme::Opacity::HoverBottom)
                .to_paint()
        })
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
    #[expect(
        clippy::too_many_lines,
        reason = "one element tree; splitting it would only thread its locals through helpers"
    )]
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let data = self.data;
        let state = kit::keyed(window, cx, part(&self.id, "state"), || HoverState {
            column: None,
            data: Arc::clone(&data),
            rows: gpui::ListState::new(data.rows.len(), gpui::ListAlignment::Top, px(96.0)),
        });
        state.update(cx, |state, _| {
            if !Arc::ptr_eq(&state.data, &data) {
                state.data = Arc::clone(&data);
                state.column = None;
                state.rows.reset(data.rows.len());
            }
        });
        let rows = state.read(cx).rows.clone();
        let hovered = state.read(cx).column;
        let grid = Grid {
            id: self.id,
            theme,
            state,
            hovered,
            recommended: data.recommended,
        };
        let columns = data.options.len();
        let criterion = move |text: SharedString| {
            div()
                .w(gpui::relative(0.24))
                .flex_shrink_0()
                .px(px(10.0))
                .py(px(7.0))
                .child(kit::muted(&theme, text))
        };
        let header = div()
            .flex()
            .items_end()
            .child(criterion(SharedString::default()))
            .children(data.options.iter().enumerate().map(|(column, option)| {
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
                            kit::title(&theme, option.clone())
                                .when(recommended, |title| title.text_color(accent)),
                        ),
                )
            }));
        let minimum = px(crate::f32_of_count(columns).mul_add(96.0, 128.0));
        let row_grid = grid.clone();
        let height = px(crate::f32_of_count(data.rows.len().min(6)) * 40.0);
        let selection = data.options.get(hovered.unwrap_or(usize::MAX)).cloned();
        let body = gpui::list(rows, move |index, _, _| {
            let Some(row) = data.rows.get(index) else {
                return div().into_any_element();
            };
            div()
                .w_full()
                .min_w(minimum)
                .flex()
                .border_t_1()
                .border_color(kit::hairline(&theme))
                .child(criterion(row.criterion.clone()))
                .children((0..columns).map(|column| {
                    let value = row.values.get(column).cloned().unwrap_or_default();
                    row_grid
                        .cell(&format!("row{index}"), column)
                        .child(if value.is_empty() {
                            kit::muted(&theme, "–")
                        } else {
                            kit::body(&theme, value)
                                .text_size(TypographyTokens::size(TextStep::XsMid))
                                .line_height(px(20.0))
                        })
                }))
                .into_any_element()
        })
        .w_full()
        .min_w(minimum)
        .h(height);
        let table = div()
            .id(part(&grid.id, "viewport"))
            .w_full()
            .min_w_0()
            .overflow_x_scroll()
            .child(
                div()
                    .min_w(minimum)
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(body),
            );
        let keyboard = grid.state.clone();
        let mut navigation = crate::chart_navigation::ChartNavigation::new(
            part(&grid.id, "navigation"),
            theme,
            "Comparison. Arrows select options; Escape clears the highlight.",
            0..columns,
            table,
            move |command, _, cx| {
                keyboard.update(cx, |state, cx| {
                    state.column = match command {
                        crate::chart_navigation::ChartCommand::Select(index)
                        | crate::chart_navigation::ChartCommand::Activate(index) => Some(index),
                        crate::chart_navigation::ChartCommand::Parent
                        | crate::chart_navigation::ChartCommand::Reset => None,
                    };
                    cx.notify();
                });
            },
        )
        .selected(hovered)
        .content_height();
        if let Some(label) = selection {
            navigation = navigation.selection_label(label);
        }
        navigation
    }
}
