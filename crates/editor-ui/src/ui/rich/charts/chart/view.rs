use super::{
    ChartStyle,
    model::Model,
    plot::{self, EndLabel, Geometry, PlotLabel, Snapshot, Tick},
};
use crate::ui::kit::Space;
use crate::ui::rich::charts::keyboard::navigation_group;
use crate::ui::rich::charts::scale::{Domain, f64_of, percent, reading};
use crate::ui::rich::charts::scene::{self, Probe, Shape};
use crate::ui::slider::{Slider, SliderRange, SliderState};
use crate::ui::theme::{ArtisanTheme, ThemeMode};
use aqua_core::{CursorIcon, GpuiHsla, InputEvent, NamedKey, Point};
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
            spec: Arc::clone(spec),
            model: Arc::clone(model),
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
            entry.spec = Arc::clone(spec);
            entry.model = Arc::clone(model);
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
            NamedKey::Tab
            | NamedKey::Backspace
            | NamedKey::Delete
            | NamedKey::Insert
            | NamedKey::ContextMenu
            | NamedKey::Escape
            | NamedKey::PageUp
            | NamedKey::PageDown
            | NamedKey::Function(_)
            | NamedKey::Unidentified => return false,
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
    let cache = scene::SceneCache::default();
    ui::Canvas::new()
        .style(
            Style::new()
                .absolute_at(Some(0.0), Some(0.0), None, None)
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0)),
        )
        .on_paint(move |bounds| {
            probe.set(Some(bounds));
            scene::cached_scene(bounds, &shapes, &cache)
        })
        .build()
}

struct EntrySnapshot {
    hidden: Vec<bool>,
    hover: Option<usize>,
    zoom: Option<Domain>,
    drag: Option<(f64, f64)>,
    probe: Probe,
    sliders: Vec<(String, Rc<RefCell<SliderState>>)>,
}

#[derive(Clone, Copy)]
struct ChartScope<'a> {
    id: &'a str,
    states: &'a ChartState,
    invalidate: &'a Rc<dyn Fn()>,
}

impl ChartScope<'_> {
    fn update(&self, change: impl Fn(&mut Entry) -> bool + 'static) -> impl Fn() + 'static {
        let states = self.states.clone();
        let id = self.id.to_owned();
        let invalidate = Rc::clone(self.invalidate);
        move || {
            let changed = states.0.borrow_mut().get_mut(&id).is_some_and(&change);
            if changed {
                invalidate();
            }
        }
    }
}

pub(crate) fn view(
    id: &str,
    theme: &ArtisanTheme,
    spec: &Arc<ChartStyle>,
    model: &Arc<Model>,
    states: &ChartState,
    services: &Services,
    invalidate: &Rc<dyn Fn()>,
) -> Element {
    if !model.has_values() {
        return (services.empty)("No data to chart");
    }
    states.sync(id, spec, model);
    let scope = ChartScope {
        id,
        states,
        invalidate,
    };
    let theme = *theme;
    let entry = entry_snapshot(states, id, spec);
    let thresholds = entry
        .sliders
        .iter()
        .map(|(label, slider)| (label.clone(), slider.borrow().value()))
        .collect::<Vec<_>>();
    let width = entry.probe.get().map_or(480.0, |rect| rect.w);
    let snapshot = Snapshot {
        hidden: &entry.hidden,
        hover: entry.hover,
        zoom: entry.zoom,
        drag: entry.drag,
        thresholds,
        width,
    };
    let geometry = cached_geometry(states, id, &theme, spec, model, &snapshot);
    let interaction: Rc<[Shape]> =
        plot::interaction(&theme, spec, model, &geometry, &snapshot).into();
    let visible = snapshot.visible(model.series.len());
    let mut plot = ui::Box::new()
        .style(
            Style::new()
                .absolute_at(Some(0.0), Some(0.0), None, None)
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0)),
        )
        .child(canvas(Rc::clone(&geometry.shapes), Rc::clone(&entry.probe)))
        .child(canvas(interaction, Rc::clone(&entry.probe)));
    for label in &geometry.labels {
        plot = plot.child((services.label)(label.clone()));
    }
    let selection = match entry.hover {
        Some(hover) => {
            let (selection, readout) =
                hover_readout(&theme, spec, model, &geometry, &visible, hover, services);
            plot = plot.child(readout);
            Some(selection)
        }
        None => None,
    };
    let plot = pointer_input(plot.build(), scope, geometry.view, &entry.probe);
    let plot = pressable(plot, scope, geometry.view, &entry.probe, services);
    let nav = keyboard_navigation(plot, scope, model, &geometry, selection);
    let nav = (services.navigation)(nav, theme.colors.ring.to_reference_paint());
    let framed = (services.frame)(
        nav,
        geometry.y_ticks.clone(),
        geometry.x_ticks.clone(),
        geometry.end_labels.clone(),
        axis_title(spec.y.label.as_deref(), spec.y.unit.as_deref()),
        axis_title(spec.x.label.as_deref(), spec.x.unit.as_deref()),
    );
    let mut root = ui::Box::new()
        .layout("column")
        .style(Style::new().width(Length::Pct(100.0)).gap(Space::S8.px()))
        .child(chart_header(&theme, model, &entry, scope, services))
        .child(framed);
    for (index, threshold) in entry.sliders.into_iter().enumerate() {
        root = root.child(threshold_slider(
            &theme, spec, model, &visible, scope, index, threshold,
        ));
    }
    root.build()
}

fn entry_snapshot(states: &ChartState, id: &str, spec: &ChartStyle) -> EntrySnapshot {
    let entries = states.0.borrow();
    let entry = &entries[id];
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
            (label, Rc::clone(&entry.sliders[&key]))
        })
        .collect::<Vec<_>>();
    EntrySnapshot {
        hidden: entry.hidden.clone(),
        hover: entry.hover,
        zoom: entry.zoom,
        drag: entry.drag,
        probe: Rc::clone(&entry.probe),
        sliders,
    }
}

fn cached_geometry(
    states: &ChartState,
    id: &str,
    theme: &ArtisanTheme,
    spec: &Arc<ChartStyle>,
    model: &Arc<Model>,
    snapshot: &Snapshot<'_>,
) -> Rc<Geometry> {
    let key = GeometryKey {
        mode: theme.colors.mode,
        hidden: snapshot.hidden.to_vec(),
        zoom: snapshot.zoom,
        thresholds: snapshot.thresholds.clone(),
        width: snapshot.width,
    };
    let mut entries = states.0.borrow_mut();
    let entry = entries
        .get_mut(id)
        .unwrap_or_else(|| panic!("invariant: chart {id} is synced before its geometry"));
    if let Some((known, geometry)) = &entry.geometry
        && *known == key
    {
        return Rc::clone(geometry);
    }
    let stable = Snapshot {
        hidden: snapshot.hidden,
        hover: None,
        zoom: snapshot.zoom,
        drag: None,
        thresholds: snapshot.thresholds.clone(),
        width: snapshot.width,
    };
    let geometry = Rc::new(plot::layout(theme, spec, model, &stable));
    entry.geometry = Some((key, Rc::clone(&geometry)));
    geometry
}

fn hover_readout(
    theme: &ArtisanTheme,
    spec: &ChartStyle,
    model: &Model,
    geometry: &Geometry,
    visible: &[bool],
    hover: usize,
    services: &Services,
) -> (String, Element) {
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
    let readout = (services.readout)(geometry.view.fraction(x), &heading, rows);
    (pieces.join("; "), readout)
}

fn pointer_input(plot: Element, scope: ChartScope<'_>, view: Domain, probe: &Probe) -> Element {
    let moving = scope.states.clone();
    let moving_id = scope.id.to_owned();
    let moving_probe = Rc::clone(probe);
    let moving_invalidate = Rc::clone(scope.invalidate);
    let release = scope.update(move |entry| ChartState::finish(entry, view));
    let leave = scope.update(|entry| entry.hover.take().is_some());
    plot.with_key(format!("{}-plot", scope.id))
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
            release();
            InputResponse {
                pointer_capture: CaptureRequest::Release,
                ..InputResponse::default()
            }
        })
        .on_hover_change(move |over| {
            if !over {
                leave();
            }
        })
}

fn pressable(
    plot: Element,
    scope: ChartScope<'_>,
    view: Domain,
    probe: &Probe,
    services: &Services,
) -> Element {
    let pressing = scope.states.clone();
    let pressing_id = scope.id.to_owned();
    let pressing_probe = Rc::clone(probe);
    let pressing_invalidate = Rc::clone(scope.invalidate);
    (services.press)(
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
    )
}

fn keyboard_navigation(
    plot: Element,
    scope: ChartScope<'_>,
    model: &Model,
    geometry: &Geometry,
    selection: Option<String>,
) -> Element {
    let first = model.xs.partition_point(|x| *x < geometry.view.lo);
    let end = model.xs.partition_point(|x| *x <= geometry.view.hi);
    let mut label =
        "Chart. Arrows explore values; Home and End select endpoints; Escape resets zoom."
            .to_owned();
    if let Some(value) = &selection {
        label.push(' ');
        label.push_str(value);
    }
    let keyboard = scope.states.clone();
    let keyboard_id = scope.id.to_owned();
    let keyboard_invalidate = Rc::clone(scope.invalidate);
    navigation_group(
        ui::Box::new()
            .style(
                Style::new()
                    .width(Length::Pct(100.0))
                    .height(Length::Pct(100.0)),
            )
            .child(plot)
            .build(),
        scope.id,
        label,
        selection,
        move |code, alt| {
            let handled = keyboard.key(&keyboard_id, first, end, code, alt);
            if handled {
                keyboard_invalidate();
            }
            handled
        },
    )
}

fn chart_header(
    theme: &ArtisanTheme,
    model: &Model,
    entry: &EntrySnapshot,
    scope: ChartScope<'_>,
    services: &Services,
) -> Element {
    let toggling = scope.states.clone();
    let toggle_id = scope.id.to_owned();
    let toggle_invalidate = Rc::clone(scope.invalidate);
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
    let legend = model
        .series
        .iter()
        .enumerate()
        .map(|(i, series)| {
            (
                series.name.clone(),
                theme.colors.palette[i % 8].to_reference_paint(),
                entry.hidden.get(i).copied().unwrap_or(false),
            )
        })
        .collect();
    let mut header = ui::Box::new()
        .layout("row")
        .style(
            Style::new()
                .width(Length::Pct(100.0))
                .align(aqua_ui::AlignItems::CENTER)
                .gap(Space::S12.px()),
        )
        .child((services.legend)(legend, toggle))
        .child(ui::Box::new().style(Style::new().flex_grow(1.0)).build());
    if entry.zoom.is_some() {
        let reset = scope.update(|entry| {
            entry.zoom = None;
            true
        });
        header = header.child((services.reset)(Rc::new(reset)));
    }
    header.build()
}

fn threshold_slider(
    theme: &ArtisanTheme,
    spec: &ChartStyle,
    model: &Model,
    visible: &[bool],
    scope: ChartScope<'_>,
    index: usize,
    (label, slider): (String, Rc<RefCell<SliderState>>),
) -> Element {
    let value = slider.borrow().value();
    let (above, all) = model.at_or_above(visible, value);
    let share = if all == 0 {
        "–".to_owned()
    } else {
        percent(f64_of(above) / f64_of(all))
    };
    let unit = plot::effective_axis(spec).unit;
    let readout = format!(
        "{} · {share} of points at or above ({above} of {all})",
        reading(value, unit.as_deref())
    );
    let refresh = Rc::clone(scope.invalidate);
    Slider::new(
        format!("{}-slider-{index}", scope.id),
        *theme,
        label,
        readout,
        slider,
    )
    .on_change(move |_| refresh())
    .build()
}
