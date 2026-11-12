use super::super::kit::{self, Tone};
use serde::Deserialize;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use crate::ui::kit::TextRole;
use aqua_core::geometry::CornerRadii;
use aqua_core::{DisplayItem, Insets};
use aqua_ui::element::{Element, ui};
use aqua_ui::style::{Length, Style};
use editor_assets::AssetId;

use crate::ui::rich::charts::scene::{self, Shape};
use crate::ui::rich::narrative::callout::{GlyphFactory, text};
use crate::ui::theme::{ArtisanTheme, Opacity};

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum Figure {
    Number(f64),
    Text(String),
}

impl Figure {
    pub(crate) fn text(&self, signed: bool) -> String {
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

fn tiles<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Arc<[Tile]>, D::Error> {
    let tiles = Vec::<Tile>::deserialize(deserializer)?;
    if tiles.len() > 64 {
        return Err(serde::de::Error::custom(
            "stats preview supports at most 64 tiles",
        ));
    }
    Ok(tiles.into())
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Trend {
    Up,
    Down,
    Flat,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Stats {
    #[serde(deserialize_with = "tiles")]
    tiles: Arc<[Tile]>,
}

#[derive(Clone, Default)]
pub(crate) struct StatsState(Rc<RefCell<HashMap<String, usize>>>);

impl StatsState {
    pub(crate) fn remove(&self, id: &str) {
        self.0.borrow_mut().remove(id);
    }
}

pub(crate) type HintFactory = Rc<dyn Fn(&ArtisanTheme, &str, &str, f32) -> Element>;

impl Stats {
    pub(crate) fn resident_bytes(&self) -> usize {
        let figure = |value: &Figure| match value {
            Figure::Text(text) => text.capacity(),
            Figure::Number(_) => 0,
        };
        std::mem::size_of::<Self>()
            + std::mem::size_of_val(self.tiles.as_ref())
            + self
                .tiles
                .iter()
                .map(|tile| {
                    tile.label.capacity()
                        + figure(&tile.value)
                        + tile.delta.as_ref().map_or(0, &figure)
                        + std::mem::size_of_val(tile.sparkline.0.as_ref())
                        + crate::ui::rich::kit::retained_optional(tile.hint.as_ref())
                })
                .sum::<usize>()
    }
    pub(crate) fn view(
        &self,
        id: String,
        theme: &ArtisanTheme,
        states: StatsState,
        glyph: GlyphFactory,
        hint: HintFactory,
        invalidate: Rc<dyn Fn()>,
    ) -> Element {
        let hovered = states
            .0
            .borrow()
            .get(&id)
            .copied()
            .filter(|index| *index < self.tiles.len());
        let mut strip = ui::Box::new().layout("row").style(
            Style::new()
                .width(Length::Pct(100.0))
                .wrap(true)
                .row_gap(16.0),
        );
        for (index, tile) in self.tiles.iter().enumerate() {
            let tone = tile.tone();
            let color = match tone {
                Tone::Success => theme.colors.banner_success.text,
                Tone::Danger => theme.colors.banner_error.text,
                _ => theme.colors.text_muted,
            };
            let edge = theme.colors.text.at(Opacity::Rule).to_reference_paint();
            let mut cell = ui::Canvas::new()
                .layout("column")
                .gap(2.0)
                .style(
                    Style::new()
                        .flex_grow(1.0)
                        .min_width(Length::Px(120.0))
                        .padding_insets(Insets {
                            left: if index > 0 { 16.0 } else { 0.0 },
                            right: 16.0,
                            top: 0.0,
                            bottom: 0.0,
                        }),
                )
                .scroll_stable()
                .on_paint(move |bounds| {
                    if index > 0 {
                        vec![DisplayItem::GpuiBorder {
                            bounds,
                            corners: CornerRadii::uniform(0.0),
                            widths: Insets {
                                left: 1.0,
                                top: 0.0,
                                right: 0.0,
                                bottom: 0.0,
                            },
                            color: edge,
                            dashed: false,
                        }]
                    } else {
                        Vec::new()
                    }
                })
                .child(text(
                    theme,
                    &tile.value.text(false),
                    TextRole::Heading,
                    theme.colors.text,
                ))
                .child(text(
                    theme,
                    &tile.label,
                    TextRole::Caption,
                    theme.colors.text_muted,
                ));
            if tile.delta.is_some() || tile.sparkline.len() >= 2 {
                let mut row = ui::Box::new().layout("row").gap(8.0).style(
                    Style::new()
                        .align(aqua_ui::AlignItems::CENTER)
                        .padding_insets(Insets {
                            left: 0.0,
                            right: 0.0,
                            top: 4.0,
                            bottom: 0.0,
                        }),
                );
                if let Some(delta) = &tile.delta {
                    let icon = match tile.trend() {
                        Some(Trend::Up) => AssetId::TABLER_CHEVRON_UP,
                        Some(Trend::Down) => AssetId::TABLER_CHEVRON_DOWN,
                        _ => AssetId::TABLER_MINUS,
                    };
                    row = row.child(
                        ui::Box::new()
                            .layout("row")
                            .gap(2.0)
                            .style(Style::new().align(aqua_ui::AlignItems::CENTER))
                            .child(glyph(icon, 12.0, color.to_reference_paint()))
                            .child(text(theme, &delta.text(true), TextRole::Caption, color))
                            .build(),
                    );
                }
                if tile.sparkline.len() >= 2 {
                    #[expect(
                        clippy::cast_precision_loss,
                        reason = "sparkline samples fit the preview limit"
                    )]
                    let points = tile
                        .sparkline
                        .0
                        .iter()
                        .enumerate()
                        .map(|(index, value)| {
                            (
                                index as f32 / (tile.sparkline.len() - 1) as f32,
                                1.0 - value,
                            )
                        })
                        .collect();
                    let spark_color = if tone == Tone::Neutral {
                        theme.colors.palette[0]
                    } else {
                        color
                    };
                    let shapes = vec![Shape::Line {
                        points,
                        width: 1.5,
                        color: spark_color.to_reference_paint(),
                        dash: false,
                    }];
                    let spark = ui::Canvas::new()
                        .width(64.0)
                        .height(20.0)
                        .style(Style::new().flex_shrink(0.0))
                        .on_paint(move |bounds| {
                            let mut items = scene::source_scene(bounds, &shapes);
                            let _ = items.pop();
                            items.remove(0);
                            items
                        })
                        .build();
                    row = row.child(spark);
                }
                cell = cell.child(row.build());
            }
            if hovered == Some(index)
                && let Some(content) = tile.hint()
            {
                cell = cell.child(hint(
                    theme,
                    tile.label.trim(),
                    content,
                    if index > 0 { 16.0 } else { 0.0 },
                ));
            }
            let mut cell = cell.build().with_key(format!("{id}-tile-{index}"));
            if tile.hint().is_some() {
                let state = states.clone();
                let key = id.clone();
                let invalidate = invalidate.clone();
                cell = cell.on_hover_change(move |over| {
                    let mut state = state.0.borrow_mut();
                    if over {
                        state.insert(key.clone(), index);
                    } else if state.get(&key) == Some(&index) {
                        state.remove(&key);
                    }
                    drop(state);
                    invalidate();
                });
            }
            strip = strip.child(cell);
        }
        strip.build().with_key(id)
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct Tile {
    label: String,
    value: Figure,
    #[serde(default)]
    delta: Option<Figure>,
    #[serde(default)]
    trend: Option<Trend>,
    #[serde(default)]
    good: Option<Trend>,
    #[serde(default)]
    sparkline: Sparkline,
    #[serde(default)]
    hint: Option<String>,
}

impl Tile {
    fn trend(&self) -> Option<Trend> {
        self.trend
            .or_else(|| self.delta.as_ref().and_then(Figure::direction))
    }

    pub(crate) fn tone(&self) -> Tone {
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

#[derive(Debug, Default)]
struct Sparkline(Arc<[f32]>);

impl Sparkline {
    fn len(&self) -> usize {
        self.0.len()
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "0..=1 fractions fit an f32"
    )]
    fn normalize(values: &[f64]) -> Self {
        let scale = values
            .iter()
            .copied()
            .filter(|value| value.is_finite())
            .fold(0.0_f64, |scale, value| scale.max(value.abs()))
            .max(f64::MIN_POSITIVE);
        let (low, high) = values
            .iter()
            .copied()
            .filter(|value| value.is_finite())
            .map(|value| value / scale)
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), value| {
                (low.min(value), high.max(value))
            });
        let span = high - low;
        Self(
            values
                .iter()
                .copied()
                .filter(|value| value.is_finite())
                .map(|value| {
                    if span > 0.0 {
                        ((value / scale - low) / span) as f32
                    } else {
                        0.0
                    }
                })
                .collect(),
        )
    }
}

impl<'de> Deserialize<'de> for Sparkline {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let values = Vec::<f64>::deserialize(deserializer)?;
        if values.len() > 4096 {
            return Err(serde::de::Error::custom(
                "sparkline preview supports at most 4096 samples",
            ));
        }
        Ok(Self::normalize(&values))
    }
}
