//! `stats`: a row of figures with deltas, sparklines, and hover hints.

use std::rc::Rc;

use editor_assets::AssetId;
use gpui::{
    AnyElement, App, Hsla, IntoElement, ParentElement as _, PathBuilder, RenderOnce, SharedString,
    Styled as _, Window, canvas, div, point, prelude::*, px,
};
use serde::Deserialize;

use super::super::kit::{self, Tone};
use super::super::kit::{part, shared};
use super::super::{BlockContext, RichBlock};
use crate::hover_card::{glass_hover_card, hover_card_text, hover_card_title};
use crate::theme::ArtisanTheme;

/// A figure as written: a number, formatted for reading, or text as is.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub(super) enum Figure {
    Number(f64),
    Text(String),
}

impl Figure {
    /// The figure for display; numbers group and compact, deltas sign.
    pub(super) fn text(&self, signed: bool) -> String {
        match self {
            Self::Number(value) if signed && *value > 0.0 => {
                format!("+{}", kit::format_number(*value))
            }
            Self::Number(value) if signed && *value < 0.0 => {
                format!("−{}", kit::format_number(value.abs()))
            }
            Self::Number(value) => kit::format_number(*value),
            Self::Text(text) => text.trim().to_owned(),
        }
    }

    /// The direction a delta points, read from its sign.
    fn direction(&self) -> Option<Trend> {
        match self {
            Self::Number(value) if *value > 0.0 => Some(Trend::Up),
            Self::Number(value) if *value < 0.0 => Some(Trend::Down),
            Self::Number(_) => Some(Trend::Flat),
            Self::Text(text) => match text.trim().chars().next()? {
                '+' | '↑' | '▲' => Some(Trend::Up),
                '-' | '−' | '↓' | '▼' => Some(Trend::Down),
                _ => None,
            },
        }
    }
}

/// Which way a figure moved.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(super) enum Trend {
    Up,
    Down,
    Flat,
}

/// The `stats` spec.
#[derive(Debug, Deserialize)]
pub(super) struct Stats {
    #[serde(deserialize_with = "shared")]
    tiles: Rc<[Tile]>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Tile {
    label: String,
    value: Figure,
    #[serde(default)]
    delta: Option<Figure>,
    #[serde(default)]
    trend: Option<Trend>,
    #[serde(default)]
    good: Option<Trend>,
    #[serde(default)]
    sparkline: Vec<f64>,
    #[serde(default)]
    hint: Option<String>,
}

impl Tile {
    /// The tile's trend: as given, else read from the delta's sign.
    fn trend(&self) -> Option<Trend> {
        self.trend
            .or_else(|| self.delta.as_ref().and_then(Figure::direction))
    }

    /// Whether the move is good: success, danger, or neutral when flat or
    /// unknown. `good` defaults to up.
    pub(super) fn tone(&self) -> Tone {
        let good = match self.good {
            Some(Trend::Down) => Trend::Down,
            _ => Trend::Up,
        };
        match self.trend() {
            None | Some(Trend::Flat) => Tone::Neutral,
            Some(trend) if trend == good => Tone::Success,
            Some(_) => Tone::Danger,
        }
    }

    fn hint(&self) -> Option<&str> {
        self.hint
            .as_deref()
            .map(str::trim)
            .filter(|hint| !hint.is_empty())
    }
}

impl RichBlock for Stats {
    fn render(&self, cx: &BlockContext<'_>) -> AnyElement {
        StatsView {
            id: cx.key("stats"),
            theme: *cx.theme,
            tiles: self.tiles.clone(),
        }
        .into_any_element()
    }

    fn markdown(&self) -> String {
        let hints = self.tiles.iter().any(|tile| tile.hint().is_some());
        let mut header = vec!["Metric".to_owned(), "Value".to_owned(), "Change".to_owned()];
        if hints {
            header.push("Note".to_owned());
        }
        let rows: Vec<Vec<String>> = self
            .tiles
            .iter()
            .map(|tile| {
                let mut row = vec![
                    tile.label.trim().to_owned(),
                    tile.value.text(false),
                    tile.delta
                        .as_ref()
                        .map_or_else(|| "–".to_owned(), |delta| delta.text(true)),
                ];
                if hints {
                    row.push(tile.hint().unwrap_or("").to_owned());
                }
                row
            })
            .collect();
        kit::markdown_table(&header, &rows)
    }
}

#[derive(IntoElement)]
struct StatsView {
    id: SharedString,
    theme: ArtisanTheme,
    tiles: Rc<[Tile]>,
}

#[derive(Default)]
struct StatsState {
    hovered: Option<usize>,
}

/// A small line through `values`, scaled to its own range.
fn sparkline(values: &[f64], color: Hsla) -> AnyElement {
    let values: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    let (low, high) = values
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), &v| {
            (low.min(v), high.max(v))
        });
    canvas(
        |_, _, _| {},
        move |bounds, (), window, _| {
            if values.len() < 2 {
                return;
            }
            let span = if high - low > f64::EPSILON {
                high - low
            } else {
                1.0
            };
            #[expect(
                clippy::cast_precision_loss,
                clippy::cast_possible_truncation,
                reason = "a sparkline needs no precision"
            )]
            let at = |index: usize, value: f64| {
                let x = index as f32 / (values.len() - 1) as f32;
                let y = ((value - low) / span) as f32;
                point(
                    bounds.origin.x + bounds.size.width * x,
                    bounds.origin.y + bounds.size.height * (1.0 - y),
                )
            };
            let mut path = PathBuilder::stroke(px(1.5));
            for (index, &value) in values.iter().enumerate() {
                if index == 0 {
                    path.move_to(at(index, value));
                } else {
                    path.line_to(at(index, value));
                }
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        },
    )
    .w(px(64.0))
    .h(px(20.0))
    .flex_shrink_0()
    .into_any_element()
}

impl StatsView {
    fn tile(
        &self,
        index: usize,
        tile: &Tile,
        state: &gpui::Entity<StatsState>,
        hovered: bool,
    ) -> gpui::Stateful<gpui::Div> {
        let theme = &self.theme;
        let tone = tile.tone();
        let color = if tone == Tone::Neutral {
            theme.colors.muted_foreground.to_paint()
        } else {
            tone.color(theme).to_paint()
        };
        let delta = tile.delta.as_ref().map(|delta| {
            let icon = match tile.trend() {
                Some(Trend::Up) => AssetId::TABLER_CHEVRON_UP,
                Some(Trend::Down) => AssetId::TABLER_CHEVRON_DOWN,
                _ => AssetId::TABLER_MINUS,
            };
            div()
                .flex()
                .items_center()
                .gap(px(2.0))
                .child(kit::glyph(icon, 12.0, color))
                .child(kit::label(theme, delta.text(true)).text_color(color))
        });
        let spark_color = if tone == Tone::Neutral {
            kit::series_color(theme, 0).to_paint()
        } else {
            color
        };
        let has_hint = tile.hint().is_some();
        let state = state.clone();
        // One cell of the strip: no tile, a hairline divider before every
        // cell but the first, the figure above its caption.
        div()
            .id(part(&self.id, format_args!("tile-{index}")))
            .relative()
            .flex_1()
            .min_w(px(120.0))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .when(index > 0, |cell| {
                cell.border_l_1()
                    .border_color(kit::hairline(theme))
                    .pl(px(16.0))
            })
            .pr(px(16.0))
            .when(has_hint, |cell| {
                cell.on_hover(move |over, _, cx| {
                    state.update(cx, |state, cx| {
                        if *over {
                            state.hovered = Some(index);
                        } else if state.hovered == Some(index) {
                            state.hovered = None;
                        }
                        cx.notify();
                    });
                })
            })
            .child(
                div()
                    .text_size(px(22.0))
                    .line_height(px(28.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.colors.foreground.to_paint())
                    .child(tile.value.text(false)),
            )
            .child(kit::label(theme, tile.label.clone()))
            .when(delta.is_some() || tile.sparkline.len() >= 2, |cell| {
                cell.child(
                    div()
                        .pt(px(4.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .children(delta)
                        .when(tile.sparkline.len() >= 2, |row| {
                            row.child(sparkline(&tile.sparkline, spark_color))
                        }),
                )
            })
            .when_some(tile.hint().filter(|_| hovered), |cell, hint| {
                cell.child(
                    glass_hover_card(
                        theme,
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .px(px(14.0))
                            .py(px(12.0))
                            .child(hover_card_title(theme, tile.label.trim().to_owned()))
                            .child(hover_card_text(theme, hint.to_owned(), &[])),
                    )
                    .absolute()
                    .top_full()
                    .mt(px(8.0))
                    .left(px(if index > 0 { 16.0 } else { 0.0 }))
                    .w(px(260.0)),
                )
            })
    }
}

impl RenderOnce for StatsView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = kit::keyed(window, cx, part(&self.id, "state"), StatsState::default);
        let hovered = state
            .read(cx)
            .hovered
            .filter(|&index| index < self.tiles.len());
        let tiles: Vec<_> = self
            .tiles
            .iter()
            .enumerate()
            .map(|(index, tile)| self.tile(index, tile, &state, hovered == Some(index)))
            .collect();
        div()
            .w_full()
            .flex()
            .flex_wrap()
            .gap_y(px(16.0))
            .children(tiles)
    }
}
