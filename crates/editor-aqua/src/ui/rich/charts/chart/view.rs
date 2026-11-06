use super::{
    ChartStyle,
    model::Model,
    plot::{self, EndLabel, Geometry, PlotLabel, Snapshot, Tick},
};
use crate::ui::rich::charts::scale::{Domain, f64_of, percent, reading};
use crate::ui::rich::charts::scene::{self, Probe, Shape};
use crate::ui::slider::{Slider, SliderRange, SliderState};
use crate::ui::theme::{ArtisanTheme, ThemeMode};
use aqua_core::{CursorIcon, GpuiHsla, InputEvent, Key, Modifiers, NamedKey, Point, Role};
use aqua_ui::element::{Element, ui};
use aqua_ui::input::{CaptureRequest, EventPhase, InputKind, InputResponse, RoutedInput};
use aqua_ui::style::{Length, Style};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

pub(crate) type PressHandler = Rc<dyn Fn(Point, usize)>;
pub(crate) type PressFactory = Rc<dyn Fn(Element, PressHandler) -> Element>;
pub(crate) type FrameFactory = Rc<
    dyn Fn(Element, Vec<Tick>, Vec<Tick>, Vec<EndLabel>, Option<String>, Option<String>) -> Element,
>;
pub(crate) type LabelFactory = Rc<dyn Fn(PlotLabel) -> Element>;
pub(crate) type ReadoutFactory =
    Rc<dyn Fn(f32, &str, Vec<(Option<GpuiHsla>, String, String)>) -> Element>;
pub(crate) type LegendFactory =
    Rc<dyn Fn(Vec<(String, GpuiHsla, bool)>, Rc<dyn Fn(usize)>) -> Element>;
pub(crate) type ResetFactory = Rc<dyn Fn(Rc<dyn Fn()>) -> Element>;
pub(crate) type EmptyFactory = Rc<dyn Fn(&str) -> Element>;
pub(crate) type NavigationFrame = Rc<dyn Fn(Element, GpuiHsla) -> Element>;
pub(crate) struct Services {
    pub press: PressFactory,
    pub frame: FrameFactory,
    pub label: LabelFactory,
    pub readout: ReadoutFactory,
    pub legend: LegendFactory,
    pub reset: ResetFactory,
    pub empty: EmptyFactory,
    pub navigation: NavigationFrame,
}

#[derive(PartialEq)]
struct GeometryKey {
    mode: ThemeMode,
    hidden: Vec<bool>,
    zoom: Option<Domain>,
    thresholds: Vec<(String, f64)>,
    width: f32,
}
struct Entry {
    spec: Arc<ChartStyle>,
    model: Arc<Model>,
    geometry: Option<(GeometryKey, Rc<Geometry>)>,
    hidden: Vec<bool>,
    hover: Option<usize>,
    zoom: Option<Domain>,
    drag: Option<(f64, f64)>,
    probe: Probe,
    sliders: HashMap<String, Rc<RefCell<SliderState>>>,
}
#[derive(Clone, Default)]
pub(crate) struct ChartState(Rc<RefCell<HashMap<String, Entry>>>);
impl ChartState {
    fn sync(&self, id: &str, spec: &Arc<ChartStyle>, model: &Arc<Model>) {
        let mut entries = self.0.borrow_mut();
        let entry = entries.entry(id.to_owned()).or_insert_with(|| Entry {
            spec: spec.clone(),
            model: model.clone(),
            geometry: None,
            hidden: vec![false; model.series.len()],
            hover: None,
            zoom: None,
            drag: None,
            probe: Rc::new(Cell::new(None)),
            sliders: HashMap::new(),
        });
        let replaced = (!Arc::ptr_eq(&entry.spec, spec) || !Arc::ptr_eq(&entry.model, model))
            && (*entry.spec != **spec || !Arc::ptr_eq(&entry.model, model));
        if !Arc::ptr_eq(&entry.spec, spec) || !Arc::ptr_eq(&entry.model, model) {
            entry.spec = spec.clone();
            entry.model = model.clone();
        }
        if replaced {
            entry.geometry = None;
            entry.hidden = vec![false; model.series.len()];
            entry.hover = None;
            entry.zoom = None;
            entry.drag = None;
        }
        for (index, param) in spec.thresholds() {
            let key = if param.id.trim().is_empty() {
                index.to_string()
            } else {
                param.id.trim().to_owned()
            };
            let range = SliderRange::new(param.min, param.max, param.step);
            let slider = entry
                .sliders
                .entry(key)
                .or_insert_with(|| Rc::new(RefCell::new(SliderState::new(range, param.value))));
            if replaced {
                slider.borrow_mut().reset(range, param.value);
            }
        }
    }
    fn finish(entry: &mut Entry, view: Domain) -> bool {
        let Some((from, to)) = entry.drag.take() else {
            return false;
        };
        let width = entry.probe.get().map_or(0.0, |bounds| bounds.w);
        if (view.fraction(to) - view.fraction(from)).abs() * width < 8.0 {
            return true;
        }
        let (lo, hi) = (from.min(to), from.max(to));
        let zoom = if entry.model.is_categorical() {
            let (first, last) = (lo.ceil(), hi.floor());
            (first <= last).then(|| Domain::new(first - 0.5, last + 0.5))
        } else {
            (hi - lo > view.span() * 1e-6).then(|| Domain::new(lo, hi))
        };
        if let Some(zoom) = zoom {
            entry.zoom = Some(zoom);
            entry.hover = None;
        }
        true
    }
    fn key(&self, id: &str, first: usize, end: usize, code: NamedKey, alt: bool) -> bool {
        let mut entries = self.0.borrow_mut();
        let Some(entry) = entries.get_mut(id) else {
            return false;
        };
        if matches!(code, NamedKey::Escape | NamedKey::Backspace) || code == NamedKey::Left && alt {
            entry.zoom = None;
            entry.drag = None;
            return true;
        }
        if first >= end {
            return false;
        }
        let current = entry.hover.filter(|n| (*n >= first) && (*n < end));
        let next = match code {
            NamedKey::Home => first,
            NamedKey::End => end - 1,
            NamedKey::Left | NamedKey::Up => {
                current.map_or(first, |n| n.saturating_sub(1).max(first))
            }
            NamedKey::Right | NamedKey::Down => current.map_or(first, |n| (n + 1).min(end - 1)),
            NamedKey::Enter => current.unwrap_or(first),
            _ => return false,
        };
        entry.hover = Some(next);
        true
    }
    pub(crate) fn remove(&self, id: &str) {
        self.0.borrow_mut().remove(id);
    }
}
fn axis_title(label: Option<&str>, unit: Option<&str>) -> Option<String> {
    let label = label.map(str::trim).filter(|s| !s.is_empty());
    let unit = unit.map(str::trim).filter(|s| !s.is_empty());
    match (label, unit) {
        (Some(l), Some(u)) => Some(format!("{l} ({u})")),
        (Some(l), None) => Some(l.to_owned()),
        (None, Some(u)) => Some(u.to_owned()),
        _ => None,
    }
}
fn canvas(shapes: Rc<[Shape]>, probe: Probe) -> Element {
    ui::Canvas::new()
        .style(
            Style::new()
                .absolute_at(Some(0.0), Some(0.0), None, None)
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0)),
        )
        .on_paint(move |bounds| {
            probe.set(Some(bounds));
            scene::source_scene(bounds, &shapes)
        })
        .build()
}

pub(crate) fn view(
    id: String,
    theme: &ArtisanTheme,
    spec: Arc<ChartStyle>,
    model: Arc<Model>,
    states: ChartState,
    services: &Services,
    invalidate: Rc<dyn Fn()>,
) -> Element {
    if !model.has_values() {
        return (services.empty)("No data to chart");
    }
    states.sync(&id, &spec, &model);
    let theme = *theme;
    let (hidden, hover, zoom, drag, probe, sliders) = {
        let entries = states.0.borrow();
        let entry = &entries[&id];
        let sliders = spec
            .thresholds()
            .map(|(index, param)| {
                let label = if param.label.trim().is_empty() {
                    "Threshold"
                } else {
                    param.label.trim()
                }
                .to_owned();
                let key = if param.id.trim().is_empty() {
                    index.to_string()
                } else {
                    param.id.trim().to_owned()
                };
                (label, entry.sliders[&key].clone())
            })
            .collect::<Vec<_>>();
        (
            entry.hidden.clone(),
            entry.hover,
            entry.zoom,
            entry.drag,
            entry.probe.clone(),
            sliders,
        )
    };
    let thresholds = sliders
        .iter()
        .map(|(label, slider)| (label.clone(), slider.borrow().value()))
        .collect::<Vec<_>>();
    let width = probe.get().map_or(480.0, |rect| rect.w);
    let snapshot = Snapshot {
        hidden: &hidden,
        hover,
        zoom,
        drag,
        thresholds: thresholds.clone(),
        width,
    };
    let key = GeometryKey {
        mode: theme.colors.mode,
        hidden: hidden.clone(),
        zoom,
        thresholds: thresholds.clone(),
        width,
    };
    let geometry = {
        let mut entries = states.0.borrow_mut();
        let entry = entries.get_mut(&id).unwrap();
        if let Some((known, geometry)) = &entry.geometry
            && *known == key
        {
            geometry.clone()
        } else {
            let stable = Snapshot {
                hidden: &hidden,
                hover: None,
                zoom,
                drag: None,
                thresholds: thresholds.clone(),
                width,
            };
            let geometry = Rc::new(plot::layout(&theme, &spec, &model, &stable));
            entry.geometry = Some((key, geometry.clone()));
            geometry
        }
    };
    let interaction: Rc<[Shape]> =
        plot::interaction(&theme, &spec, &model, &geometry, &snapshot).into();
    let visible = snapshot.visible(model.series.len());
    let mut selection = None;
    let mut plot = ui::Box::new()
        .style(
            Style::new()
                .absolute_at(Some(0.0), Some(0.0), None, None)
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0)),
        )
        .child(canvas(geometry.shapes.clone(), probe.clone()))
        .child(canvas(interaction, probe.clone()));
    for label in &geometry.labels {
        plot = plot.child((services.label)(label.clone()));
    }
    if let Some(hover) = hover {
        let x = model.xs.get(hover).copied().unwrap_or_default();
        let values: Vec<(usize, f64)> = (0..model.series.len())
            .filter(|i| visible[*i])
            .filter_map(|i| model.value_at(i, x).map(|value| (i, value)))
            .collect();
        let scale = values
            .iter()
            .map(|(_, value)| value.abs())
            .fold(0.0, f64::max);
        let total = if scale > 0.0 {
            values
                .iter()
                .map(|(_, value)| value.abs() / scale)
                .sum::<f64>()
        } else {
            0.0
        };
        let mut rows = values
            .iter()
            .map(|(index, value)| {
                let mut text = reading(*value, spec.y.unit.as_deref());
                if spec.normalize && total > 0.0 {
                    text = format!("{text} · {}", percent((value.abs() / scale) / total));
                }
                (
                    Some(theme.colors.palette[index % 8].to_reference_paint()),
                    model.series[*index].name.clone(),
                    text,
                )
            })
            .collect::<Vec<_>>();
        if rows.is_empty() {
            rows.push((None, "No values".to_owned(), String::new()));
        } else if spec.stacked && !spec.normalize && values.len() > 1 {
            let sum = values.iter().map(|(_, value)| value).sum::<f64>();
            rows.push((
                None,
                "Total".to_owned(),
                if sum.is_finite() {
                    reading(sum, spec.y.unit.as_deref())
                } else {
                    "Outside numeric range".to_owned()
                },
            ));
        }
        let heading = if model.is_categorical() {
            model.x_label(x)
        } else {
            reading(x, spec.x.unit.as_deref())
        };
        let mut pieces = vec![heading.clone()];
        pieces.extend(
            rows.iter()
                .map(|(_, label, value)| format!("{label}: {value}")),
        );
        selection = Some(pieces.join("; "));
        plot = plot.child((services.readout)(
            geometry.view.fraction(x),
            &heading,
            rows,
        ));
    }
    let view = geometry.view;
    let moving = states.clone();
    let moving_id = id.clone();
    let moving_probe = probe.clone();
    let moving_invalidate = invalidate.clone();
    let releasing = states.clone();
    let releasing_id = id.clone();
    let releasing_invalidate = invalidate.clone();
    let leaving = states.clone();
    let leaving_id = id.clone();
    let leaving_invalidate = invalidate.clone();
    let plot = plot
        .build()
        .with_key(format!("{id}-plot"))
        .cursor(CursorIcon::Crosshair)
        .on_input(InputKind::PointerMove, EventPhase::Bubble, move |event| {
            let RoutedInput::Native(InputEvent::PointerMove(pointer)) = &event.input else {
                return InputResponse::default();
            };
            let Some((fx, _)) = scene::fractions(&moving_probe, pointer.position) else {
                return InputResponse::default();
            };
            let x = view.value_at(fx);
            let mut entries = moving.0.borrow_mut();
            let Some(entry) = entries.get_mut(&moving_id) else {
                return InputResponse::default();
            };
            let hover = entry.model.nearest(x, view);
            let mut changed = hover != entry.hover;
            entry.hover = hover;
            if let Some((from, _)) = entry.drag {
                if pointer.buttons & 1 != 0 {
                    entry.drag = Some((from, x.clamp(view.lo, view.hi)));
                } else {
                    ChartState::finish(entry, view);
                }
                changed = true;
            }
            drop(entries);
            if changed {
                moving_invalidate();
            }
            InputResponse::default()
        })
        .on_input(InputKind::PointerUp, EventPhase::Bubble, move |_| {
            let mut entries = releasing.0.borrow_mut();
            let changed = entries
                .get_mut(&releasing_id)
                .is_some_and(|entry| ChartState::finish(entry, view));
            drop(entries);
            if changed {
                releasing_invalidate();
            }
            InputResponse {
                pointer_capture: CaptureRequest::Release,
                ..InputResponse::default()
            }
        })
        .on_hover_change(move |over| {
            if !over {
                let mut entries = leaving.0.borrow_mut();
                let changed = entries
                    .get_mut(&leaving_id)
                    .is_some_and(|entry| entry.hover.take().is_some());
                drop(entries);
                if changed {
                    leaving_invalidate();
                }
            }
        });
    let pressing = states.clone();
    let pressing_id = id.clone();
    let pressing_probe = probe.clone();
    let pressing_invalidate = invalidate.clone();
    let plot = (services.press)(
        plot,
        Rc::new(move |position, click_count| {
            let Some((fx, _)) = scene::fractions(&pressing_probe, position) else {
                return;
            };
            let x = view.value_at(fx);
            let mut entries = pressing.0.borrow_mut();
            if let Some(entry) = entries.get_mut(&pressing_id) {
                if click_count >= 2 {
                    entry.zoom = None;
                    entry.drag = None;
                } else {
                    entry.drag = Some((x, x));
                }
            }
            drop(entries);
            pressing_invalidate();
        }),
    );
    let first = model.xs.partition_point(|x| *x < geometry.view.lo);
    let end = model.xs.partition_point(|x| *x <= geometry.view.hi);
    let mut label =
        "Chart. Arrows explore values; Home and End select endpoints; Escape resets zoom."
            .to_owned();
    if let Some(value) = &selection {
        label.push(' ');
        label.push_str(value);
    }
    let keyboard = states.clone();
    let keyboard_id = id.clone();
    let keyboard_invalidate = invalidate.clone();
    let mut nav = ui::Box::new()
        .style(
            Style::new()
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0)),
        )
        .child(plot)
        .build()
        .with_key(format!("{id}-navigation"))
        .role(Role::Group)
        .focusable(true)
        .tab_index(0)
        .aria_label(label)
        .on_input(InputKind::KeyDown, EventPhase::Bubble, move |event| {
            let RoutedInput::Native(InputEvent::KeyDown(key_event)) = &event.input else {
                return InputResponse::default();
            };
            let Key::Named(code) = key_event.key else {
                return InputResponse::default();
            };
            if !keyboard.key(
                &keyboard_id,
                first,
                end,
                code,
                key_event.modifiers.contains(Modifiers::ALT),
            ) {
                return InputResponse::default();
            }
            keyboard_invalidate();
            InputResponse {
                stop_propagation: true,
                ..InputResponse::default()
            }
        });
    if let Some(value) = selection {
        nav = nav.aria_value(value);
    }
    let nav = (services.navigation)(nav, theme.colors.ring.to_reference_paint());
    let framed = (services.frame)(
        nav,
        geometry.y_ticks.clone(),
        geometry.x_ticks.clone(),
        geometry.end_labels.clone(),
        axis_title(spec.y.label.as_deref(), spec.y.unit.as_deref()),
        axis_title(spec.x.label.as_deref(), spec.x.unit.as_deref()),
    );
    let toggling = states.clone();
    let toggle_id = id.clone();
    let toggle_invalidate = invalidate.clone();
    let toggle = Rc::new(move |index: usize| {
        let mut entries = toggling.0.borrow_mut();
        if let Some(value) = entries
            .get_mut(&toggle_id)
            .and_then(|entry| entry.hidden.get_mut(index))
        {
            *value = !*value;
        }
        drop(entries);
        toggle_invalidate();
    });
    let entries = model
        .series
        .iter()
        .enumerate()
        .map(|(i, series)| {
            (
                series.name.clone(),
                theme.colors.palette[i % 8].to_reference_paint(),
                hidden.get(i).copied().unwrap_or(false),
            )
        })
        .collect();
    let mut header = ui::Box::new()
        .layout("row")
        .style(
            Style::new()
                .width(Length::Pct(100.0))
                .align(aqua_ui::AlignItems::CENTER)
                .gap(12.0),
        )
        .child((services.legend)(entries, toggle))
        .child(ui::Box::new().style(Style::new().flex_grow(1.0)).build());
    if zoom.is_some() {
        let resetting = states.clone();
        let reset_id = id.clone();
        let reset_invalidate = invalidate.clone();
        header = header.child((services.reset)(Rc::new(move || {
            if let Some(entry) = resetting.0.borrow_mut().get_mut(&reset_id) {
                entry.zoom = None;
            }
            reset_invalidate();
        })));
    }
    let mut root = ui::Box::new()
        .layout("column")
        .style(Style::new().width(Length::Pct(100.0)).gap(10.0))
        .child(header.build())
        .child(framed);
    for (index, (label, slider)) in sliders.into_iter().enumerate() {
        let value = slider.borrow().value();
        let (above, all) = model.at_or_above(&visible, value);
        let share = if all == 0 {
            "–".to_owned()
        } else {
            percent(f64_of(above) / f64_of(all))
        };
        let unit = plot::effective_axis(&spec).unit;
        let readout = format!(
            "{} · {share} of points at or above ({above} of {all})",
            reading(value, unit.as_deref())
        );
        let refresh = invalidate.clone();
        root = root.child(
            Slider::new(
                format!("{id}-slider-{index}"),
                theme,
                label,
                readout,
                slider,
            )
            .on_change(move |_| refresh())
            .build(),
        );
    }
    root.build()
}

#[cfg(feature = "test-support")]
pub struct ChartFixture {
    spec: Arc<ChartStyle>,
    model: Arc<Model>,
    state: ChartState,
}
#[cfg(feature = "test-support")]
impl ChartFixture {
    pub fn new(json: &str) -> Self {
        let value = serde_json::from_str(json).expect("JSON");
        let (spec, model) = super::prepare(&value).expect("chart");
        let (spec, model) = (Arc::new(spec), Arc::new(model));
        let state = ChartState::default();
        state.sync("test", &spec, &model);
        Self { spec, model, state }
    }
    pub fn replace(&mut self, json: &str) {
        let value = serde_json::from_str(json).expect("JSON");
        let (spec, model) = super::prepare(&value).expect("chart");
        self.spec = Arc::new(spec);
        self.model = Arc::new(model);
        self.state.sync("test", &self.spec, &self.model);
    }
    pub fn resync(&self) {
        self.state.sync("test", &self.spec, &self.model);
    }
    pub fn key(&self, key: NamedKey, alt: bool) -> bool {
        let entries = self.state.0.borrow();
        let entry = &entries["test"];
        let view = entry
            .zoom
            .unwrap_or_else(|| self.model.x_domain(self.spec.mark));
        let first = self.model.xs.partition_point(|x| *x < view.lo);
        let end = self.model.xs.partition_point(|x| *x <= view.hi);
        drop(entries);
        self.state.key("test", first, end, key, alt)
    }
    pub fn selected(&self) -> Option<usize> {
        self.state.0.borrow()["test"].hover
    }
    pub fn zoom(&self) -> Option<(f64, f64)> {
        self.state.0.borrow()["test"].zoom.map(|d| (d.lo, d.hi))
    }
    pub fn drag(&self, from: f64, to: f64, width: f32) {
        let mut entries = self.state.0.borrow_mut();
        let entry = entries.get_mut("test").unwrap();
        let view = entry
            .zoom
            .unwrap_or_else(|| self.model.x_domain(self.spec.mark));
        entry.probe.set(Some(aqua_core::Rect {
            x: 0.0,
            y: 0.0,
            w: width,
            h: 200.0,
        }));
        entry.drag = Some((from, to));
        ChartState::finish(entry, view);
    }
    pub fn toggle(&self, index: usize) {
        let mut entries = self.state.0.borrow_mut();
        if let Some(value) = entries
            .get_mut("test")
            .and_then(|entry| entry.hidden.get_mut(index))
        {
            *value = !*value;
        }
    }
    pub fn hidden(&self) -> Vec<bool> {
        self.state.0.borrow()["test"].hidden.clone()
    }
}
