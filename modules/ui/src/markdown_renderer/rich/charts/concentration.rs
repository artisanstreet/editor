//! `concentration`: how lopsided a total is across items. Items are ranked
//! largest first; the curve rises through the cumulative share of the total
//! against the share of items, beside the diagonal an even spread would
//! draw. A highlighted cutoff reads the headline ("the top 10% of installs
//! send 65% of turns"), and hovering reads any point on the curve.

use std::fmt::Write as _;

use gpui::{
    AnyElement, App, ElementId, IntoElement, MouseMoveEvent, ParentElement as _, RenderOnce,
    SharedString, Styled as _, Window, div, prelude::*,
};
use serde::Deserialize;

use super::super::{BlockContext, RichBlock, kit};
use super::frame::{self, Frame, PlotLabel, ReadoutRow, Tick};
use super::scale::{Domain, f32_of, f64_of, index_of, percent};
use super::scene::{Probe, Shape, fractions, hrule, plot_canvas, probe, vrule};
use crate::theme::ArtisanTheme;

/// Most points drawn; larger inputs are sampled evenly along the curve.
const MAX_POINTS: usize = 400;

/// Shares of items listed in the copied table.
const TABLE_SHARES: [f64; 5] = [0.01, 0.05, 0.1, 0.2, 0.5];

#[derive(Clone, Debug, Deserialize)]
pub(super) struct Concentration {
    /// One value per item: turns per install, bytes per file, and so on.
    values: Vec<f64>,
    /// What the items are, plural ("installs").
    #[serde(default)]
    items: Option<String>,
    /// What the values add up to, plural ("turns").
    #[serde(default)]
    total: Option<String>,
    /// The share of items to call out, `0..1`; default `0.1`.
    #[serde(default)]
    highlight: Option<f64>,
}

impl Concentration {
    fn items(&self) -> &str {
        self.items.as_deref().map_or("items", str::trim)
    }

    fn total(&self) -> &str {
        self.total.as_deref().map_or("the total", str::trim)
    }

    fn highlight(&self) -> f64 {
        self.highlight
            .filter(|share| share.is_finite() && *share > 0.0 && *share < 1.0)
            .unwrap_or(0.1)
    }

    /// The curve: `(share of items, share of total)` from `(0, 0)` to
    /// `(1, 1)`, items ranked largest first. `None` without a positive total.
    pub(super) fn curve(&self) -> Option<Vec<(f64, f64)>> {
        let mut values: Vec<f64> = self
            .values
            .iter()
            .copied()
            .filter(|value| value.is_finite() && *value >= 0.0)
            .collect();
        let total: f64 = values.iter().sum();
        if values.is_empty() || total <= 0.0 {
            return None;
        }
        values.sort_by(|a, b| b.total_cmp(a));
        let count = f64_of(values.len());
        let mut running = 0.0;
        let mut curve = Vec::with_capacity(values.len() + 1);
        curve.push((0.0, 0.0));
        for (index, value) in values.iter().enumerate() {
            running += value;
            curve.push((f64_of(index + 1) / count, running / total));
        }
        Some(curve)
    }

    /// The share of the total held by the top `share` of items, read off the
    /// curve with linear interpolation between items.
    pub(super) fn share_of_total(curve: &[(f64, f64)], share: f64) -> f64 {
        let share = share.clamp(0.0, 1.0);
        let after = curve
            .iter()
            .position(|(x, _)| *x >= share)
            .unwrap_or(curve.len() - 1);
        if after == 0 {
            return curve[0].1;
        }
        let (x0, y0) = curve[after - 1];
        let (x1, y1) = curve[after];
        if x1 - x0 <= f64::EPSILON {
            y1
        } else {
            y0 + (y1 - y0) * (share - x0) / (x1 - x0)
        }
    }
}

impl RichBlock for Concentration {
    fn render(&self, cx: &BlockContext<'_>) -> AnyElement {
        ConcentrationView {
            id: cx.key("concentration"),
            theme: *cx.theme,
            spec: self.clone(),
        }
        .into_any_element()
    }

    fn markdown(&self) -> String {
        let Some(curve) = self.curve() else {
            return "_No values._".to_owned();
        };
        let header = vec![
            format!("Top share of {}", self.items()),
            format!("Share of {}", self.total()),
        ];
        let rows: Vec<Vec<String>> = TABLE_SHARES
            .iter()
            .map(|share| {
                vec![
                    percent(*share),
                    percent(Self::share_of_total(&curve, *share)),
                ]
            })
            .collect();
        let mut out = kit::markdown_table(&header, &rows);
        let _ = write!(out, "\n_{} {}._\n", self.values.len(), self.items());
        out
    }
}

#[derive(IntoElement)]
struct ConcentrationView {
    id: SharedString,
    theme: ArtisanTheme,
    spec: Concentration,
}

struct ConcentrationState {
    /// The hovered share of items, `0..1`.
    hover: Option<f64>,
    probe: Probe,
}

impl RenderOnce for ConcentrationView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let Some(curve) = self.spec.curve() else {
            return frame::empty(&theme, "No values to rank");
        };
        let state = kit::keyed(window, cx, ElementId::Name(self.id.clone()), || {
            ConcentrationState {
                hover: None,
                probe: probe(),
            }
        });
        let (hover, plot_probe) = {
            let current = state.read(cx);
            (current.hover, current.probe.clone())
        };
        let axis = Domain::new(0.0, 1.0);
        let quarters = [0.0, 0.25, 0.5, 0.75, 1.0];
        let y_ticks: Vec<Tick> = quarters
            .iter()
            .map(|share| Tick {
                at: axis.fraction_down(*share),
                label: SharedString::from(percent(*share)),
            })
            .collect();
        let x_ticks: Vec<Tick> = quarters
            .iter()
            .map(|share| Tick {
                at: axis.fraction(*share),
                label: SharedString::from(percent(*share)),
            })
            .collect();
        let (shapes, labels) = self.shapes(&curve, &y_ticks, hover);
        let readout = hover.map(|share| {
            let held = Concentration::share_of_total(&curve, share);
            frame::readout(
                &theme,
                f32_of(share),
                Some(format!("Top {} of {}", percent(share), self.spec.items()).into()),
                vec![ReadoutRow::new(
                    Some(kit::series_color(&theme, 0).to_paint()),
                    format!("Share of {}", self.spec.total()),
                    percent(held),
                )],
            )
        });
        let plot = self.plot(&state, shapes, labels, readout, plot_probe);
        Frame {
            end_labels: Vec::new(),
            y_ticks,
            x_ticks,
            y_title: Some(format!("Share of {}", self.spec.total()).into()),
            x_title: Some(format!("Share of {}, largest first", self.spec.items()).into()),
            y_axis_width: frame::NUMBER_AXIS_WIDTH,
            height: frame::PLOT_HEIGHT,
        }
        .wrap(&theme, plot)
        .into_any_element()
    }
}

impl ConcentrationView {
    /// The grid, the even-spread diagonal, the area under the curve, the
    /// curve, the highlighted cutoff, and the hover marker.
    fn shapes(
        &self,
        curve: &[(f64, f64)],
        y_ticks: &[Tick],
        hover: Option<f64>,
    ) -> (Vec<Shape>, Vec<PlotLabel>) {
        let theme = &self.theme;
        let color = kit::series_color(theme, 0);
        let at = |(x, y): (f64, f64)| (f32_of(x), 1.0 - f32_of(y));
        let sampled = sample(curve, MAX_POINTS);
        let mut shapes = frame::grid(theme, y_ticks, &[]);
        shapes.push(Shape::Line {
            points: vec![(0.0, 1.0), (1.0, 0.0)],
            width: 1.0,
            color: theme.colors.muted_foreground.with_alpha(0.5).to_paint(),
            dash: true,
        });
        let mut area: Vec<(f32, f32)> = sampled.iter().copied().map(at).collect();
        area.push((1.0, 1.0));
        area.push((0.0, 1.0));
        shapes.push(Shape::Area {
            points: area,
            color: color.with_alpha(0.14).to_paint(),
        });
        shapes.push(Shape::Line {
            points: sampled.iter().copied().map(at).collect(),
            width: 2.0,
            color: color.to_paint(),
            dash: false,
        });
        let cutoff = self.spec.highlight();
        let held = Concentration::share_of_total(curve, cutoff);
        let (x, y) = at((cutoff, held));
        let marker = theme.colors.foreground.with_alpha(0.55).to_paint();
        shapes.push(vrule(x, marker, true));
        shapes.push(hrule(y, marker, true));
        shapes.push(Shape::Dot {
            x,
            y,
            radius: 4.0,
            color: theme.colors.foreground.to_paint(),
        });
        let labels = vec![PlotLabel {
            x,
            y,
            below: true,
            text: SharedString::from(format!(
                "Top {} of {} \u{2192} {} of {}",
                percent(cutoff),
                self.spec.items(),
                percent(held),
                self.spec.total()
            )),
            color: theme.colors.foreground.to_paint(),
        }];
        if let Some(share) = hover {
            let (hx, hy) = at((share, Concentration::share_of_total(curve, share)));
            shapes.push(vrule(
                hx,
                theme.colors.foreground.with_alpha(0.3).to_paint(),
                false,
            ));
            shapes.push(Shape::Dot {
                x: hx,
                y: hy,
                radius: 4.0,
                color: color.to_paint(),
            });
        }
        (shapes, labels)
    }

    /// The plot surface with its labels, readout, and hover tracking.
    fn plot(
        &self,
        state: &gpui::Entity<ConcentrationState>,
        shapes: Vec<Shape>,
        labels: Vec<PlotLabel>,
        readout: Option<gpui::Div>,
        plot_probe: Probe,
    ) -> AnyElement {
        let moving = state.clone();
        let leaving = state.clone();
        let move_probe = plot_probe.clone();
        div()
            .id(ElementId::Name(format!("{}-plot", self.id).into()))
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .cursor_crosshair()
            .child(plot_canvas(shapes, plot_probe))
            .children(
                labels
                    .into_iter()
                    .map(|label| frame::plot_label(&self.theme, label)),
            )
            .children(readout)
            .on_mouse_move(move |event: &MouseMoveEvent, _, cx| {
                let hover = fractions(&move_probe, event.position)
                    .filter(|(fx, fy)| (0.0..=1.0).contains(fx) && (0.0..=1.0).contains(fy))
                    .map(|(fx, _)| f64::from(fx));
                moving.update(cx, |state, cx| {
                    if state.hover != hover {
                        state.hover = hover;
                        cx.notify();
                    }
                });
            })
            .on_hover(move |hovered, _, cx| {
                if !*hovered {
                    leaving.update(cx, |state, cx| {
                        if state.hover.take().is_some() {
                            cx.notify();
                        }
                    });
                }
            })
            .into_any_element()
    }
}

/// At most `limit` points spread evenly along `curve`, keeping both ends.
fn sample(curve: &[(f64, f64)], limit: usize) -> Vec<(f64, f64)> {
    if curve.len() <= limit || limit < 2 {
        return curve.to_vec();
    }
    let last = curve.len() - 1;
    (0..limit)
        .map(|step| curve[index_of(f64_of(step) * f64_of(last) / f64_of(limit - 1)).min(last)])
        .collect()
}
