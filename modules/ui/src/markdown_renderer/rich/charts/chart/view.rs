//! The chart widget: legend toggles, hover readout, drag-to-zoom, and
//! threshold sliders over the plot.

use std::rc::Rc;

use artisan_assets::AssetId;
use gpui::{
    App, ElementId, Entity, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement as _, RenderOnce, SharedString, Styled as _, Window, div, prelude::*, px,
};

use super::super::super::kit;
use super::super::frame::{self, Frame, LegendEntry, ReadoutRow};
use super::super::scale::{Domain, f64_of, percent, reading};
use super::super::scene::{Probe, fractions, plot_canvas, probe, probed_width};
use super::super::slider::{self, SliderState};
use super::ChartSpec;
use super::model::Model;
use super::plot::{self, Geometry, Snapshot};
use crate::theme::ArtisanTheme;

/// Drags shorter than this many pixels are clicks, not zooms.
const MIN_ZOOM_DRAG: f32 = 8.0;

#[derive(IntoElement)]
pub(super) struct ChartView {
    pub(super) id: SharedString,
    pub(super) theme: ArtisanTheme,
    pub(super) spec: Rc<ChartSpec>,
    pub(super) model: Rc<Model>,
}

struct ChartState {
    hidden: Vec<bool>,
    hover: Option<usize>,
    zoom: Option<Domain>,
    drag: Option<(f64, f64)>,
    probe: Probe,
}

impl ChartState {
    /// Ends a drag at its last position; zooms when it spanned enough of
    /// the plot and covers at least one x.
    fn finish_drag(&mut self, model: &Model, view: Domain) -> bool {
        let Some((from, to)) = self.drag.take() else {
            return false;
        };
        let width = probed_width(&self.probe, 0.0);
        let span_px = (view.fraction(to) - view.fraction(from)).abs() * width;
        if span_px < MIN_ZOOM_DRAG {
            return true;
        }
        let (lo, hi) = (from.min(to), from.max(to));
        let zoom = if model.is_categorical() {
            let (first, last) = (lo.ceil(), hi.floor());
            (first <= last).then(|| Domain::new(first - 0.5, last + 0.5))
        } else {
            (hi - lo > view.span() * 1e-6).then(|| Domain::new(lo, hi))
        };
        if zoom.is_some() {
            self.zoom = zoom;
            self.hover = None;
        }
        true
    }
}

impl RenderOnce for ChartView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        if !self.model.has_values() {
            return frame::empty(&theme, "No data to chart");
        }
        let count = self.model.series.len();
        let state = kit::keyed(window, cx, ElementId::Name(self.id.clone()), || {
            ChartState {
                hidden: vec![false; count],
                hover: None,
                zoom: None,
                drag: None,
                probe: probe(),
            }
        });
        let sliders: Vec<(SharedString, Entity<SliderState>)> = self
            .spec
            .thresholds()
            .map(|(index, param)| {
                let label = param.label.trim();
                let label = SharedString::from(
                    if label.is_empty() { "Threshold" } else { label }.to_owned(),
                );
                let (min, max, step, value) = (param.min, param.max, param.step, param.value);
                let key = if param.id.trim().is_empty() {
                    index.to_string()
                } else {
                    param.id.trim().to_owned()
                };
                let entity = slider::keyed_slider(
                    window,
                    cx,
                    format!("{}-param-{key}", self.id).into(),
                    || SliderState::new(min, max, step, value),
                );
                (label, entity)
            })
            .collect();
        let current = state.read(cx);
        let snapshot = Snapshot {
            hidden: &current.hidden,
            hover: current.hover,
            zoom: current.zoom,
            drag: current.drag,
            thresholds: sliders
                .iter()
                .map(|(label, slider)| (label.clone(), slider.read(cx).value))
                .collect(),
            width: probed_width(&current.probe, 480.0),
        };
        let geometry = plot::layout(&theme, &self.spec, &self.model, &snapshot);
        let readout = snapshot
            .hover
            .map(|hover| self.readout(&geometry, &snapshot, hover));
        let visible = snapshot.visible(count);
        let zoomed = current.zoom.is_some();
        let plot_probe = current.probe.clone();
        let header = self.header(&state, zoomed, cx);
        let slider_rows = self.slider_rows(&sliders, &visible, cx);
        let plot = self.plot(&geometry, readout, &state, plot_probe);
        let framed = Frame {
            end_labels: geometry.end_labels.clone(),
            y_ticks: geometry.y_ticks,
            x_ticks: geometry.x_ticks,
            y_title: frame::axis_title(self.spec.y.label.as_deref(), self.spec.y.unit.as_deref()),
            x_title: frame::axis_title(self.spec.x.label.as_deref(), self.spec.x.unit.as_deref()),
            y_axis_width: frame::NUMBER_AXIS_WIDTH,
            height: frame::PLOT_HEIGHT,
        }
        .wrap(&theme, plot);
        let entities: Vec<Entity<SliderState>> =
            sliders.into_iter().map(|(_, entity)| entity).collect();
        let root = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(header)
            .child(framed)
            .children(slider_rows);
        slider::follow_drag(root, &entities).into_any_element()
    }
}

impl ChartView {
    /// One row per threshold: its slider and the share at or above it.
    fn slider_rows(
        &self,
        sliders: &[(SharedString, Entity<SliderState>)],
        visible: &[bool],
        cx: &App,
    ) -> Vec<gpui::Div> {
        let unit = plot::effective_axis(&self.spec).unit;
        sliders
            .iter()
            .enumerate()
            .map(|(index, (label, entity))| {
                let value = entity.read(cx).value;
                let (above, all) = self.model.at_or_above(visible, value);
                let share = if all == 0 {
                    "–".to_owned()
                } else {
                    percent(f64_of(above) / f64_of(all))
                };
                let text = format!(
                    "{} · {share} of points at or above ({above} of {all})",
                    reading(value, unit.as_deref())
                );
                slider::slider(
                    &self.theme,
                    format!("{}-slider-{index}", self.id).into(),
                    label.clone(),
                    text.into(),
                    entity,
                    cx,
                )
            })
            .collect()
    }

    /// The legend, and a reset control while zoomed.
    fn header(&self, state: &Entity<ChartState>, zoomed: bool, cx: &App) -> gpui::Div {
        let hidden = &state.read(cx).hidden;
        let entries = self
            .model
            .series
            .iter()
            .enumerate()
            .map(|(index, series)| LegendEntry {
                name: SharedString::from(series.name.clone()),
                color: kit::series_color(&self.theme, index).to_paint(),
                hidden: hidden.get(index).copied().unwrap_or(false),
            })
            .collect();
        let toggling = state.clone();
        let toggle: frame::Toggle = Rc::new(move |index, cx: &mut App| {
            toggling.update(cx, |state, cx| {
                if let Some(hidden) = state.hidden.get_mut(index) {
                    *hidden = !*hidden;
                }
                cx.notify();
            });
        });
        let resetting = state.clone();
        div()
            .w_full()
            .flex()
            .items_center()
            .gap(px(12.0))
            .child(frame::legend(&self.theme, &self.id, entries, Some(&toggle)))
            .child(div().flex_1())
            .when(zoomed, |header| {
                header.child(
                    div()
                        .id(ElementId::Name(format!("{}-reset", self.id).into()))
                        .cursor_pointer()
                        .on_click(move |_, _, cx| {
                            resetting.update(cx, |state, cx| {
                                state.zoom = None;
                                cx.notify();
                            });
                        })
                        .child(
                            kit::chip(&self.theme, "Reset zoom", kit::Tone::Neutral).child(
                                kit::glyph(
                                    AssetId::TABLER_ZOOM_OUT,
                                    12.0,
                                    self.theme.colors.muted_foreground.to_paint(),
                                ),
                            ),
                        ),
                )
            })
    }

    /// The readout panel for the hovered x.
    fn readout(&self, geometry: &Geometry, snapshot: &Snapshot<'_>, hover: usize) -> gpui::Div {
        let model = &self.model;
        let x = model.xs.get(hover).copied().unwrap_or_default();
        let unit = self.spec.y.unit.as_deref();
        let visible = snapshot.visible(model.series.len());
        let values: Vec<(usize, f64)> = (0..model.series.len())
            .filter(|index| visible[*index])
            .filter_map(|index| model.value_at(index, x).map(|value| (index, value)))
            .collect();
        let total: f64 = values.iter().map(|(_, value)| value.abs()).sum();
        let mut rows: Vec<ReadoutRow> = values
            .iter()
            .map(|(index, value)| {
                let mut text = reading(*value, unit);
                if self.spec.normalize && total > 0.0 {
                    text = format!("{text} · {}", percent(value.abs() / total));
                }
                ReadoutRow::new(
                    Some(kit::series_color(&self.theme, *index).to_paint()),
                    model.series[*index].name.clone(),
                    text,
                )
            })
            .collect();
        if rows.is_empty() {
            rows.push(ReadoutRow::new(None, "No values", ""));
        } else if self.spec.stacked && !self.spec.normalize && values.len() > 1 {
            let sum: f64 = values.iter().map(|(_, value)| value).sum();
            rows.push(ReadoutRow::new(None, "Total", reading(sum, unit)));
        }
        let heading = if model.is_categorical() {
            model.x_label(x)
        } else {
            reading(x, self.spec.x.unit.as_deref())
        };
        frame::readout(
            &self.theme,
            geometry.view.fraction(x),
            Some(heading.into()),
            rows,
        )
    }

    /// The plot surface: canvas, labels, readout, and the pointer handlers.
    fn plot(
        &self,
        geometry: &Geometry,
        readout: Option<gpui::Div>,
        state: &Entity<ChartState>,
        probe: Probe,
    ) -> gpui::Stateful<gpui::Div> {
        let view = geometry.view;
        let labels = geometry
            .labels
            .iter()
            .cloned()
            .map(|label| frame::plot_label(&self.theme, label));
        let (moving, pressing, releasing, leaving, unhovering) = (
            state.clone(),
            state.clone(),
            state.clone(),
            state.clone(),
            state.clone(),
        );
        let (move_model, up_model, out_model) = (
            Rc::clone(&self.model),
            Rc::clone(&self.model),
            Rc::clone(&self.model),
        );
        let down_probe = probe.clone();
        let move_probe = probe.clone();
        div()
            .id(ElementId::Name(format!("{}-plot", self.id).into()))
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .cursor_crosshair()
            .child(plot_canvas(geometry.shapes.clone(), probe))
            .children(labels)
            .children(readout)
            .on_mouse_move(move |event: &MouseMoveEvent, _, cx| {
                let Some((fx, _)) = fractions(&move_probe, event.position) else {
                    return;
                };
                let x = view.value_at(fx);
                moving.update(cx, |state, cx| {
                    let hover = move_model.nearest(x, view);
                    let mut changed = hover != state.hover;
                    state.hover = hover;
                    if let Some((from, _)) = state.drag {
                        if event.pressed_button == Some(MouseButton::Left) {
                            state.drag = Some((from, x.clamp(view.lo, view.hi)));
                        } else {
                            state.finish_drag(&move_model, view);
                        }
                        changed = true;
                    }
                    if changed {
                        cx.notify();
                    }
                });
            })
            .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, _, cx| {
                let Some((fx, _)) = fractions(&down_probe, event.position) else {
                    return;
                };
                let x = view.value_at(fx);
                pressing.update(cx, |state, cx| {
                    if event.click_count >= 2 {
                        state.zoom = None;
                        state.drag = None;
                    } else {
                        state.drag = Some((x, x));
                    }
                    cx.notify();
                });
            })
            .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                releasing.update(cx, |state, cx| {
                    if state.finish_drag(&up_model, view) {
                        cx.notify();
                    }
                });
            })
            .on_mouse_up_out(MouseButton::Left, move |_, _, cx| {
                leaving.update(cx, |state, cx| {
                    if state.finish_drag(&out_model, view) {
                        cx.notify();
                    }
                });
            })
            .on_hover(move |hovered, _, cx| {
                if !*hovered {
                    unhovering.update(cx, |state, cx| {
                        if state.hover.take().is_some() {
                            cx.notify();
                        }
                    });
                }
            })
    }
}
