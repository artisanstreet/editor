//! The chart widget: legend toggles, hover readout, drag-to-zoom, and
//! threshold sliders over the plot.

use std::rc::Rc;
use std::sync::Arc;

use editor_assets::AssetId;
use gpui::{
    App, ElementId, Entity, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement as _, RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, div, prelude::*, px,
};

use super::super::super::kit;
use super::super::frame::{self, Frame, LegendEntry, ReadoutRow};
use super::super::scale::{Domain, f64_of, percent, reading};
use super::super::scene::{Probe, fractions, plot_canvas, probe, probed_width};
use super::ChartStyle;
use super::model::Model;
use super::plot::{self, Geometry, Snapshot};
use crate::slider::{Slider, SliderRange, SliderState};
use crate::theme::ArtisanTheme;

/// Drags shorter than this many pixels are clicks, not zooms.
const MIN_ZOOM_DRAG: f32 = 8.0;

#[derive(IntoElement)]
pub(super) struct ChartView {
    pub(super) id: SharedString,
    pub(super) theme: ArtisanTheme,
    pub(super) spec: Arc<ChartStyle>,
    pub(super) model: Arc<Model>,
}

#[derive(PartialEq)]
struct GeometryKey {
    hidden: Vec<bool>,
    zoom: Option<Domain>,
    thresholds: Vec<(SharedString, f64)>,
    width: f32,
    mode: crate::theme::ThemeMode,
}
struct ChartState {
    spec: Arc<ChartStyle>,
    model: Arc<Model>,
    geometry: Option<(GeometryKey, Rc<Geometry>)>,
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
                spec: self.spec.clone(),
                model: self.model.clone(),
                geometry: None,
                hidden: vec![false; count],
                hover: None,
                zoom: None,
                drag: None,
                probe: probe(),
            }
        });
        let replaced = state.update(cx, |state, _| {
            if !Arc::ptr_eq(&state.spec, &self.spec) || !Arc::ptr_eq(&state.model, &self.model) {
                let changed = *state.spec != *self.spec || !Arc::ptr_eq(&state.model, &self.model);
                state.spec = self.spec.clone();
                state.model = self.model.clone();
                if changed {
                    state.geometry = None;
                    state.hidden = vec![false; count];
                    state.hover = None;
                    state.zoom = None;
                    state.drag = None;
                }
                changed
            } else {
                false
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
                let entity = SliderState::keyed(
                    window,
                    cx,
                    format!("{}-param-{key}", self.id),
                    SliderRange::new(min, max, step),
                    value,
                );
                if replaced {
                    entity.update(cx, |state, _| {
                        state.reset(SliderRange::new(min, max, step), value)
                    });
                }
                (label, entity)
            })
            .collect();
        let current = state.read(cx);
        let hidden = current.hidden.clone();
        let hover = current.hover;
        let zoom = current.zoom;
        let drag = current.drag;
        let plot_probe = current.probe.clone();
        let snapshot = Snapshot {
            hidden: &hidden,
            hover,
            zoom,
            drag,
            thresholds: sliders
                .iter()
                .map(|(label, slider)| (label.clone(), slider.read(cx).value()))
                .collect(),
            width: probed_width(&plot_probe, 480.0),
        };
        let key = GeometryKey {
            hidden: hidden.clone(),
            zoom,
            thresholds: snapshot.thresholds.clone(),
            width: snapshot.width,
            mode: theme.mode,
        };
        let geometry = state.update(cx, |state, _| {
            if let Some((known, geometry)) = &state.geometry {
                if *known == key {
                    return geometry.clone();
                }
            }
            let stable = Snapshot {
                hidden: &hidden,
                hover: None,
                zoom,
                drag: None,
                thresholds: snapshot.thresholds.clone(),
                width: snapshot.width,
            };
            let geometry = Rc::new(plot::layout(&theme, &self.spec, &self.model, &stable));
            state.geometry = Some((key, geometry.clone()));
            geometry
        });
        let interaction = plot::interaction(&theme, &self.spec, &self.model, &geometry, &snapshot);
        let (readout, selection_label) = snapshot
            .hover
            .map(|hover| self.readout(&geometry, &snapshot, hover))
            .unzip();
        let visible = snapshot.visible(count);
        let zoomed = zoom.is_some();
        let header = self.header(&state, zoomed, cx);
        let slider_rows = self.slider_rows(&sliders, &visible, cx);
        let keyboard = state.clone();
        let first = self.model.xs.partition_point(|x| *x < geometry.view.lo);
        let end = self.model.xs.partition_point(|x| *x <= geometry.view.hi);
        let mut plot = crate::chart_navigation::ChartNavigation::new(
            kit::part(&self.id, "navigation"),
            theme,
            "Chart. Arrows explore values; Home and End select endpoints; Escape resets zoom.",
            first..end,
            self.plot(&geometry, interaction, readout, &state, plot_probe),
            move |command, _, cx| {
                keyboard.update(cx, |state, cx| {
                    match command {
                        crate::chart_navigation::ChartCommand::Select(index)
                        | crate::chart_navigation::ChartCommand::Activate(index) => {
                            state.hover = Some(index)
                        }
                        crate::chart_navigation::ChartCommand::Parent
                        | crate::chart_navigation::ChartCommand::Reset => {
                            state.zoom = None;
                            state.drag = None;
                        }
                    }
                    cx.notify();
                })
            },
        )
        .selected(snapshot.hover);
        if let Some(label) = selection_label {
            plot = plot.selection_label(label);
        }

        let framed = Frame {
            end_labels: geometry.end_labels.clone(),
            y_ticks: geometry.y_ticks.clone(),
            x_ticks: geometry.x_ticks.clone(),
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
        Slider::follow_drag(root, &entities).into_any_element()
    }
}

impl ChartView {
    /// One row per threshold: its slider and the share at or above it.
    fn slider_rows(
        &self,
        sliders: &[(SharedString, Entity<SliderState>)],
        visible: &[bool],
        cx: &App,
    ) -> Vec<Slider> {
        let unit = plot::effective_axis(&self.spec).unit;
        sliders
            .iter()
            .enumerate()
            .map(|(index, (label, entity))| {
                let value = entity.read(cx).value();
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
                Slider::new(
                    format!("{}-slider-{index}", self.id),
                    self.theme,
                    label.clone(),
                    text,
                    entity.clone(),
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
                    crate::button::Button::icon_text(
                        kit::part(&self.id, "reset"),
                        self.theme,
                        AssetId::TABLER_ZOOM_OUT,
                        "Reset zoom",
                    )
                    .variant(crate::button::ButtonVariant::Ghost)
                    .on_activate(move |_, _, cx| {
                        resetting.update(cx, |state, cx| {
                            state.zoom = None;
                            cx.notify();
                        });
                    }),
                )
            })
    }

    /// The readout panel for the hovered x.
    fn readout(
        &self,
        geometry: &Geometry,
        snapshot: &Snapshot<'_>,
        hover: usize,
    ) -> (gpui::Div, SharedString) {
        let model = &self.model;
        let x = model.xs.get(hover).copied().unwrap_or_default();
        let unit = self.spec.y.unit.as_deref();
        let visible = snapshot.visible(model.series.len());
        let values: Vec<(usize, f64)> = (0..model.series.len())
            .filter(|index| visible[*index])
            .filter_map(|index| model.value_at(index, x).map(|value| (index, value)))
            .collect();
        let scale = values
            .iter()
            .map(|(_, value)| value.abs())
            .fold(0.0, f64::max);
        let total: f64 = if scale > 0.0 {
            values.iter().map(|(_, value)| value.abs() / scale).sum()
        } else {
            0.0
        };
        let mut rows: Vec<ReadoutRow> = values
            .iter()
            .map(|(index, value)| {
                let mut text = reading(*value, unit);
                if self.spec.normalize && total > 0.0 {
                    text = format!("{text} · {}", percent((value.abs() / scale) / total));
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
            rows.push(ReadoutRow::new(
                None,
                "Total",
                if sum.is_finite() {
                    reading(sum, unit)
                } else {
                    "Outside numeric range".to_owned()
                },
            ));
        }
        let heading = if model.is_categorical() {
            model.x_label(x)
        } else {
            reading(x, self.spec.x.unit.as_deref())
        };
        let selection_label = frame::reading_label(Some(&heading), &rows);
        (
            frame::readout(
                &self.theme,
                geometry.view.fraction(x),
                Some(heading.into()),
                rows,
            ),
            selection_label,
        )
    }

    /// The plot surface: canvas, labels, readout, and the pointer handlers.
    fn plot(
        &self,
        geometry: &Geometry,
        interaction: Vec<super::super::scene::Shape>,
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
            Arc::clone(&self.model),
            Arc::clone(&self.model),
            Arc::clone(&self.model),
        );
        let down_probe = probe.clone();
        let move_probe = probe.clone();
        div()
            .id(kit::part(&self.id, "plot"))
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .cursor_crosshair()
            .child(plot_canvas(geometry.shapes.clone(), probe.clone()))
            .child(plot_canvas(interaction, probe))
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

#[cfg(test)]
mod native_replacement_tests {
    use super::super::ChartSpec;
    use super::*;
    use gpui::{Context, Render, TestAppContext};
    struct Fixture {
        style: Arc<ChartStyle>,
        model: Arc<Model>,
    }
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(640.0)).child(ChartView {
                id: "replacement-chart".into(),
                theme: ArtisanTheme::default(),
                spec: self.style.clone(),
                model: self.model.clone(),
            })
        }
    }
    fn data(source: &str) -> (Arc<ChartStyle>, Arc<Model>) {
        let spec: ChartSpec = serde_json::from_str(source).unwrap();
        let model = Arc::new(Model::from_spec(&spec));
        (Arc::new(spec.into()), model)
    }
    fn bars(cx: &mut gpui::VisualTestContext) -> usize {
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            let scale = window.scale_factor();
            let data_fill: gpui::Background = kit::series_color(&ArtisanTheme::default(), 0)
                .to_paint()
                .into();
            window
                .painted_quads()
                .into_iter()
                .filter(|quad| {
                    quad.background == data_fill
                        && quad.bounds.size.width.as_f32() / scale > 20.0
                        && quad.bounds.size.height.as_f32() / scale > 30.0
                })
                .count()
        })
    }
    #[gpui::test]
    fn unchanged_style_replacement_updates_actual_painted_bar_count(cx: &mut TestAppContext) {
        let (style, model) = data(r#"{"mark":"bar","series":[{"name":"a","values":[2,4]}]}"#);
        let (view, cx) = cx.add_window_view(|_, _| Fixture { style, model });
        assert_eq!(bars(cx), 2);
        let (style, model) = data(r#"{"mark":"bar","series":[{"name":"a","values":[7]}]}"#);
        view.update(cx, |view, cx| {
            assert_eq!(*view.style, *style);
            view.model = model;
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(
            bars(cx),
            1,
            "a new immutable model must invalidate geometry even when style identity is unchanged"
        );
    }
}
