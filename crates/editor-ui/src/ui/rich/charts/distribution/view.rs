use super::{Distribution, Mode, Summary, share_at_or_below};
use crate::ui::kit::Space;
use crate::ui::rich::charts::scale::{
    Domain, f32_of_count, f64_of, index_of, nice_ticks, percent, reading, ticks_within,
};
use crate::ui::rich::charts::scene::{self, Probe, Shape};
use crate::ui::slider::{Slider, SliderRange, SliderState};
use crate::ui::theme::{ArtisanTheme, Opacity};
use aqua_core::{GpuiBackground, GpuiHsla, InputEvent, Insets, Key, Modifiers, NamedKey, Role};
use aqua_text::FontWeight;
use aqua_ui::collection::List;
use aqua_ui::element::{Element, ui};
use aqua_ui::input::{EventPhase, InputKind, InputResponse, RoutedInput};
use aqua_ui::style::{Length, Style};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct Tick {
    pub at: f32,
    pub label: String,
}
#[derive(Clone)]
pub(crate) struct ReadoutRow {
    pub color: Option<GpuiHsla>,
    pub label: String,
    pub value: String,
}
pub(crate) struct PlotFrame {
    pub x_ticks: Vec<Tick>,
    pub y_ticks: Vec<Tick>,
    pub y_title: Option<String>,
    pub x_title: Option<String>,
    pub y_axis_width: f32,
    pub height: f32,
}
pub(crate) type FrameFactory = Rc<dyn Fn(Element, PlotFrame) -> Element>;
pub(crate) type ReadoutFactory = Rc<dyn Fn(f32, &str, Vec<ReadoutRow>) -> Element>;
pub(crate) type PlotLabelFactory = Rc<dyn Fn(f32, f32, &str, GpuiHsla) -> Element>;
pub(crate) type LegendFactory = Rc<dyn Fn(Vec<(String, GpuiHsla)>) -> Element>;
pub(crate) type TextFactory =
    Rc<dyn Fn(&str, f32, f32, FontWeight, crate::ui::theme::Oklch, bool) -> Element>;
pub(crate) type EmptyFactory = Rc<dyn Fn(&str) -> Element>;
pub(crate) type NavigationFrame = Rc<dyn Fn(Element, GpuiHsla) -> Element>;
pub(crate) type HoverFill = Rc<dyn Fn() -> GpuiBackground>;
pub(crate) type TickStripFactory = Rc<dyn Fn(Vec<Tick>) -> Element>;

struct Entry {
    model: Arc<Distribution>,
    hover: Option<(f32, f32)>,
    probe: Probe,
    slider: Rc<RefCell<SliderState>>,
    reveal: Option<usize>,
    reset: u64,
}
#[derive(Clone, Default)]
pub(crate) struct DistributionState(Rc<RefCell<HashMap<String, Entry>>>);
impl DistributionState {
    fn sync(
        &self,
        id: &str,
        model: &Arc<Distribution>,
    ) -> (Option<(f32, f32)>, Probe, Rc<RefCell<SliderState>>, u64) {
        let mut entries = self.0.borrow_mut();
        let (domain, step, start) = model.slider();
        let entry = entries.entry(id.to_owned()).or_insert_with(|| Entry {
            model: model.clone(),
            hover: None,
            probe: Rc::new(Cell::new(None)),
            slider: Rc::new(RefCell::new(SliderState::new(
                SliderRange::new(domain.lo, domain.hi, Some(step)),
                start,
            ))),
            reveal: None,
            reset: 0,
        });
        if !Arc::ptr_eq(&entry.model, model) {
            if *entry.model != **model {
                entry.hover = None;
                entry.reveal = None;
                entry.reset = entry.reset.wrapping_add(1);
                entry
                    .slider
                    .borrow_mut()
                    .reset(SliderRange::new(domain.lo, domain.hi, Some(step)), start);
            }
            entry.model = model.clone();
        }
        (
            entry.hover,
            entry.probe.clone(),
            entry.slider.clone(),
            entry.reset,
        )
    }
    fn set(&self, id: &str, hover: Option<(f32, f32)>) -> bool {
        let mut entries = self.0.borrow_mut();
        let Some(entry) = entries.get_mut(id) else {
            return false;
        };
        let changed = entry.hover != hover;
        entry.hover = hover;
        changed
    }
    fn key(&self, id: &str, mode: Mode, count: usize, code: NamedKey, alt: bool) -> bool {
        let mut entries = self.0.borrow_mut();
        let Some(entry) = entries.get_mut(id) else {
            return false;
        };
        if count == 0 {
            return false;
        }
        let selected = entry.hover.map(|(x, y)| match mode {
            Mode::Box => index_of(f64::from(y * f32_of_count(count))).min(count - 1),
            Mode::Histogram => index_of(f64::from(x * f32_of_count(count))).min(count - 1),
            Mode::Cdf => index_of(f64::from((x * 100.0).round())).min(count - 1),
        });
        let next = match code {
            NamedKey::Escape | NamedKey::Backspace => None,
            NamedKey::Left if alt => None,
            NamedKey::Home => Some(0),
            NamedKey::End => Some(count - 1),
            NamedKey::Left | NamedKey::Up => Some(selected.map_or(0, |n| n.saturating_sub(1))),
            NamedKey::Right | NamedKey::Down => {
                Some(selected.map_or(0, |n| n.saturating_add(1).min(count - 1)))
            }
            NamedKey::Enter => Some(selected.unwrap_or(0)),
            _ => return false,
        };
        entry.hover = next.map(|index| match mode {
            Mode::Box => (0.5, (f32_of_count(index) + 0.5) / f32_of_count(count)),
            Mode::Histogram => ((f32_of_count(index) + 0.5) / f32_of_count(count), 0.5),
            Mode::Cdf => (f32_of_count(index) / 100.0, 0.5),
        });
        entry.reveal = if mode == Mode::Box { next } else { None };
        true
    }
    pub(crate) fn pending_reveal(&self, id: &str) -> Option<(String, usize)> {
        let entries = self.0.borrow();
        let e = entries.get(id)?;
        Some((format!("{id}-rows-{}", e.reset), e.reveal?))
    }
    pub(crate) fn revealed(&self, id: &str, key: &str, index: usize) {
        if let Some(e) = self.0.borrow_mut().get_mut(id)
            && key == format!("{id}-rows-{}", e.reset)
            && e.reveal == Some(index)
        {
            e.reveal = None;
        }
    }
    pub(crate) fn remove(&self, id: &str) {
        self.0.borrow_mut().remove(id);
    }
}
fn line(points: Vec<(f32, f32)>, width: f32, color: GpuiHsla, dash: bool) -> Shape {
    Shape::Line {
        points,
        width,
        color,
        dash,
    }
}
fn ticks(domain: Domain, width: f32) -> Vec<Tick> {
    let budget = index_of(f64::from((width / 96.0).floor().clamp(2.0, 24.0)));
    let ticks = ticks_within(domain, budget);
    ticks
        .values
        .into_iter()
        .map(|v| Tick {
            at: domain.fraction(v),
            label: reading(v, None),
        })
        .collect()
}
fn box_shapes(summary: &Summary, domain: Domain, color: GpuiHsla, fill: GpuiHsla) -> Vec<Shape> {
    let (lo, hi) = (
        domain.fraction(summary.whiskers.0),
        domain.fraction(summary.whiskers.1),
    );
    let (q1, median, q3) = (
        domain.fraction(summary.q1),
        domain.fraction(summary.median),
        domain.fraction(summary.q3),
    );
    let mut shapes = vec![
        line(vec![(lo, 0.5), (q1, 0.5)], 1.5, color, false),
        line(vec![(q3, 0.5), (hi, 0.5)], 1.5, color, false),
        line(vec![(lo, 0.36), (lo, 0.64)], 1.5, color, false),
        line(vec![(hi, 0.36), (hi, 0.64)], 1.5, color, false),
        Shape::Rect {
            x0: q1,
            y0: 0.24,
            x1: q3,
            y1: 0.76,
            color: fill,
        },
        line(
            vec![(q1, 0.24), (q3, 0.24), (q3, 0.76), (q1, 0.76), (q1, 0.24)],
            1.5,
            color,
            false,
        ),
        line(vec![(median, 0.24), (median, 0.76)], 2.5, color, false),
    ];
    let mut i = 0;
    while let Some(value) = summary.outliers.get(i) {
        i += summary.outliers[i..].partition_point(|other| other == value);
        let mut c = color;
        c.a *= Opacity::Outlier.alpha();
        shapes.push(Shape::Dot {
            x: domain.fraction(*value),
            y: 0.5,
            radius: 3.0,
            color: c,
        });
    }
    shapes
}
fn canvas(shapes: Vec<Shape>, probe: Probe) -> Element {
    let shapes: Rc<[Shape]> = shapes.into();
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
fn label(rows: &[ReadoutRow], heading: &str) -> String {
    let mut parts = vec![heading.to_owned()];
    parts.extend(
        rows.iter()
            .map(|row| format!("{}: {}", row.label, row.value)),
    );
    parts.join("; ")
}

pub(crate) struct Services {
    pub frame: FrameFactory,
    pub readout: ReadoutFactory,
    pub plot_label: PlotLabelFactory,
    pub legend: LegendFactory,
    pub text: TextFactory,
    pub empty: EmptyFactory,
    pub navigation: NavigationFrame,
    pub hover_fill: HoverFill,
    pub tick_strip: TickStripFactory,
}

pub(crate) fn view(
    id: String,
    theme: &ArtisanTheme,
    data: Arc<Distribution>,
    states: DistributionState,
    services: &Services,
    invalidate: Rc<dyn Fn()>,
) -> Element {
    if data.is_empty() {
        return (services.empty)("No values to plot");
    }
    let theme = *theme;
    let (hover, probe, slider, reset) = states.sync(&id, &data);
    let width = probe.get().map_or(480.0, |bounds| bounds.w);
    if data.mode == Mode::Box {
        return boxes(
            id, theme, data, states, services, invalidate, hover, probe, reset, width,
        );
    }
    let domain = data.domain();
    let threshold = slider.borrow().value();
    let selected = hover.and_then(|(x, _)| (0.0..=1.0).contains(&x).then_some(x));
    let mut shapes = Vec::new();
    let mut readout_rows = Vec::new();
    let mut heading = String::new();
    let mut readout_x = 0.0;
    let y_ticks;
    if data.mode == Mode::Histogram {
        let tallest = data
            .samples
            .iter()
            .flat_map(|sample| sample.counts.iter())
            .copied()
            .max()
            .unwrap_or(0)
            .max(1);
        let y = nice_ticks(Domain::new(0.0, f64_of(tallest)), 4, true, false);
        y_ticks = y
            .values
            .iter()
            .map(|v| Tick {
                at: y.domain.fraction_down(*v),
                label: reading(*v, None),
            })
            .collect::<Vec<_>>();
        for tick in &y_ticks {
            shapes.push(line(
                vec![(0.0, tick.at), (1.0, tick.at)],
                1.0,
                theme.colors.text.at(Opacity::Rule).to_reference_paint(),
                false,
            ));
        }
        if let Some(x) = selected {
            let bin = data.bins.index(domain.value_at(x));
            let (lo, hi) = data.bins.edges(bin);
            shapes.push(Shape::HoverBand {
                x0: domain.fraction(lo),
                x1: domain.fraction(hi),
                fill: (services.hover_fill)(),
                ring: theme
                    .colors
                    .text
                    .at(Opacity::HoverRing)
                    .to_reference_paint(),
            });
            heading = format!(
                "{} – {}",
                reading(lo, data.unit.as_deref()),
                reading(hi, data.unit.as_deref())
            );
            readout_x = domain.fraction(f64::midpoint(lo, hi));
            for (index, sample) in data.samples.iter().enumerate() {
                let count = sample.counts[bin];
                readout_rows.push(ReadoutRow {
                    color: Some(theme.colors.palette[index % 8].to_reference_paint()),
                    label: sample.name.clone(),
                    value: format!(
                        "{count} · {}",
                        percent(f64_of(count) / f64_of(sample.sorted.len().max(1)))
                    ),
                });
            }
        }
        let opacity = if data.samples.len() > 1 {
            Opacity::Overlap
        } else {
            Opacity::BarSolo
        };
        for (index, sample) in data.samples.iter().enumerate() {
            for (bin, count) in sample
                .counts
                .iter()
                .enumerate()
                .filter(|(_, count)| **count > 0)
            {
                let (lo, hi) = data.bins.edges(bin);
                let gap = data.bins.width() * 0.04;
                shapes.push(Shape::Rect {
                    x0: domain.fraction(lo + gap),
                    x1: domain.fraction(hi - gap),
                    y0: y.domain.fraction_down(f64_of(*count)),
                    y1: 1.0,
                    color: theme.colors.palette[index % 8]
                        .at(opacity)
                        .to_reference_paint(),
                });
            }
        }
    } else {
        let y = Domain::new(0.0, 1.0);
        y_ticks = [0.0, 0.25, 0.5, 0.75, 1.0]
            .into_iter()
            .map(|v| Tick {
                at: y.fraction_down(v),
                label: percent(v),
            })
            .collect();
        for tick in &y_ticks {
            shapes.push(line(
                vec![(0.0, tick.at), (1.0, tick.at)],
                1.0,
                theme.colors.text.at(Opacity::Rule).to_reference_paint(),
                false,
            ));
        }
        if let Some(x) = selected {
            let at = domain.value_at(x);
            shapes.push(line(
                vec![(x, 0.0), (x, 1.0)],
                1.0,
                theme
                    .colors
                    .text
                    .at(Opacity::ChartRule)
                    .to_reference_paint(),
                false,
            ));
            heading = reading(at, data.unit.as_deref());
            readout_x = x;
            for (index, sample) in data.samples.iter().enumerate() {
                readout_rows.push(ReadoutRow {
                    color: Some(theme.colors.palette[index % 8].to_reference_paint()),
                    label: sample.name.clone(),
                    value: if sample.sorted.is_empty() {
                        "No data".to_owned()
                    } else {
                        format!(
                            "{} at or below",
                            percent(share_at_or_below(&sample.sorted, at))
                        )
                    },
                });
            }
        }
        for (index, sample) in data.samples.iter().enumerate() {
            if sample.sorted.is_empty() {
                continue;
            }
            let count = f64_of(sample.sorted.len());
            let mut points = vec![(0.0, 1.0)];
            let mut rank = 0;
            while rank < sample.sorted.len() {
                let value = sample.sorted[rank];
                let end = sample.sorted.partition_point(|other| *other <= value);
                let x = domain.fraction(value);
                points.push((x, y.fraction_down(f64_of(rank) / count)));
                points.push((x, y.fraction_down(f64_of(end) / count)));
                rank = end;
            }
            points.push((1.0, 0.0));
            shapes.push(line(
                points,
                2.0,
                theme.colors.palette[index % 8].to_reference_paint(),
                false,
            ));
        }
    }
    let at = domain.fraction(threshold);
    let warning = theme.colors.banner_warning.text.to_reference_paint();
    shapes.push(line(vec![(at, 0.0), (at, 1.0)], 1.5, warning, true));
    let mut plot = ui::Box::new()
        .style(
            Style::new()
                .absolute_at(Some(0.0), Some(0.0), None, None)
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0)),
        )
        .child(canvas(shapes, probe.clone()))
        .child((services.plot_label)(
            at,
            0.0,
            &format!("≤ {}", reading(threshold, data.unit.as_deref())),
            warning,
        ));
    let selection = selected.map(|_| label(&readout_rows, &heading));
    if selected.is_some() {
        plot = plot.child((services.readout)(readout_x, &heading, readout_rows));
    }
    let moving = states.clone();
    let moving_id = id.clone();
    let moving_probe = probe.clone();
    let moving_invalidate = invalidate.clone();
    let leaving = states.clone();
    let leaving_id = id.clone();
    let leaving_invalidate = invalidate.clone();
    let plot = plot
        .build()
        .with_key(format!("{id}-plot"))
        .on_input(InputKind::PointerMove, EventPhase::Bubble, move |event| {
            let RoutedInput::Native(InputEvent::PointerMove(pointer)) = &event.input else {
                return InputResponse::default();
            };
            if moving.set(
                &moving_id,
                scene::fractions(&moving_probe, pointer.position),
            ) {
                moving_invalidate();
            }
            InputResponse::default()
        })
        .on_hover_change(move |over| {
            if !over && leaving.set(&leaving_id, None) {
                leaving_invalidate();
            }
        });
    let count = if data.mode == Mode::Histogram {
        data.bins.count
    } else {
        101
    };
    let nav = navigation(
        &id,
        plot,
        data.mode,
        count,
        selection,
        states.clone(),
        theme.colors.ring.to_reference_paint(),
        services.navigation.clone(),
        invalidate.clone(),
    );
    let framed = (services.frame)(
        nav,
        PlotFrame {
            x_ticks: ticks(domain, width),
            y_ticks,
            y_title: (data.mode == Mode::Histogram).then(|| "Count".into()),
            x_title: data.unit.clone(),
            y_axis_width: 48.0,
            height: 240.0,
        },
    );
    let mut root = ui::Box::new()
        .layout("column")
        .style(Style::new().width(Length::Pct(100.0)).gap(Space::S8.px()));
    if data.samples.len() > 1 {
        root = root.child((services.legend)(
            data.samples
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    (
                        if s.sorted.is_empty() {
                            format!("{} (no data)", s.name)
                        } else {
                            s.name.clone()
                        },
                        theme.colors.palette[i % 8].to_reference_paint(),
                    )
                })
                .collect(),
        ));
    }
    root = root.child(framed);
    let shares = data
        .samples
        .iter()
        .map(|sample| {
            let value = if sample.sorted.is_empty() {
                "no data".to_owned()
            } else {
                percent(share_at_or_below(&sample.sorted, threshold))
            };
            if data.samples.len() > 1 {
                format!("{} {value}", sample.name)
            } else {
                value
            }
        })
        .collect::<Vec<_>>()
        .join(" · ");
    let slider = Slider::new(
        format!("{id}-slider"),
        theme,
        "Threshold",
        format!("≤ {} · {shares}", reading(threshold, data.unit.as_deref())),
        slider,
    )
    .on_change(move |_| invalidate())
    .build();
    root.child(slider).build()
}

fn navigation(
    id: &str,
    content: Element,
    mode: Mode,
    count: usize,
    selection: Option<String>,
    states: DistributionState,
    ring: GpuiHsla,
    frame: NavigationFrame,
    invalidate: Rc<dyn Fn()>,
) -> Element {
    let prefix = if mode == Mode::Box {
        "Box distributions. Arrows select rows; Escape clears the reading."
    } else {
        "Distribution chart. Arrows explore readings; Escape clears the reading."
    };
    let label = selection
        .as_ref()
        .map_or_else(|| prefix.to_owned(), |s| format!("{prefix} {s}"));
    let key = id.to_owned();
    let mut wrapper = ui::Box::new().width(Length::Pct(100.0));
    if mode != Mode::Box {
        wrapper = wrapper.height(Length::Pct(100.0));
    }
    let mut element = wrapper
        .child(content)
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
            if !states.key(
                &key,
                mode,
                count,
                code,
                key_event.modifiers.contains(Modifiers::ALT),
            ) {
                return InputResponse::default();
            }
            invalidate();
            InputResponse {
                stop_propagation: true,
                ..InputResponse::default()
            }
        });
    if let Some(label) = selection {
        element = element.aria_value(label);
    }
    frame(element, ring)
}

fn boxes(
    id: String,
    theme: ArtisanTheme,
    data: Arc<Distribution>,
    states: DistributionState,
    services: &Services,
    invalidate: Rc<dyn Fn()>,
    hover: Option<(f32, f32)>,
    probe: Probe,
    reset: u64,
    width: f32,
) -> Element {
    let count = data.samples.len();
    let selected = hover.map(|(_, y)| index_of(f64::from(y) * f64_of(count)).min(count - 1));
    let budget = index_of(f64::from((width * 0.75 / 96.0).floor().clamp(2.0, 24.0)));
    let axis = nice_ticks(data.domain(), budget, false, false);
    let domain = axis.domain;
    let axis_ticks: Vec<Tick> = axis
        .values
        .iter()
        .map(|value| Tick {
            at: domain.fraction(*value),
            label: reading(*value, None),
        })
        .collect();
    let grid: Vec<Shape> = axis_ticks
        .iter()
        .map(|tick| {
            line(
                vec![(tick.at, 0.0), (tick.at, 1.0)],
                1.0,
                theme.colors.text.at(Opacity::Rule).to_reference_paint(),
                false,
            )
        })
        .collect();
    let model = data.clone();
    let row_states = states.clone();
    let row_id = id.clone();
    let row_invalidate = invalidate.clone();
    let row_text = services.text.clone();
    let pitch = theme.density.control_sm;
    let rows = List::new()
        .items(0..count)
        .key(|row| *row)
        .virtualize()
        .estimated_row_height(Length::Px(pitch))
        .style(
            Style::new()
                .width(Length::Pct(100.0))
                .height(Length::Px(pitch * f32_of_count(count.min(10)))),
        )
        .row(move |row| {
            let row = *row;
            let sample = &model.samples[row];
            let mut shapes = grid.clone();
            if let Some(summary) = &sample.summary {
                shapes.extend(box_shapes(
                    summary,
                    domain,
                    theme.colors.palette[row % 8].to_reference_paint(),
                    theme.colors.palette[row % 8]
                        .at(Opacity::BoxFill)
                        .to_reference_paint(),
                ));
            }
            let label = ui::Box::new()
                .style(
                    Style::new()
                        .width(Length::Pct(25.0))
                        .min_width(Length::Px(0.0))
                        .padding_insets(Insets {
                            right: 8.0,
                            ..Insets::default()
                        }),
                )
                .clip(true)
                .child(row_text(
                    &sample.name,
                    theme.typography.control_text,
                    pitch,
                    FontWeight::NORMAL,
                    theme.colors.text_muted,
                    false,
                ))
                .build();
            let plot = ui::Box::new()
                .style(
                    Style::new()
                        .flex_grow(1.0)
                        .min_width(Length::Px(0.0))
                        .height(Length::Pct(100.0)),
                )
                .child(canvas(shapes, Rc::new(Cell::new(None))))
                .build();
            let state = row_states.clone();
            let key = row_id.clone();
            let invalidate = row_invalidate.clone();
            ui::Box::new()
                .layout("row")
                .style(
                    Style::new()
                        .width(Length::Pct(100.0))
                        .height(Length::Px(pitch))
                        .align(aqua_ui::AlignItems::CENTER),
                )
                .child(label)
                .child(plot)
                .build()
                .with_key(format!("{row_id}-box-row-{row}"))
                .on_hover_change(move |over| {
                    let previous = state
                        .0
                        .borrow()
                        .get(&key)
                        .and_then(|e| e.hover)
                        .map(|(_, y)| index_of(f64::from(y) * f64_of(count)).min(count - 1));
                    let next = if over {
                        Some((0.5, (f32_of_count(row) + 0.5) / f32_of_count(count)))
                    } else if previous == Some(row) {
                        None
                    } else {
                        return;
                    };
                    if state.set(&key, next) {
                        invalidate();
                    }
                })
        })
        .build()
        .with_key(format!("{id}-rows-{reset}"));
    let mut content = ui::Box::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .child(rows)
        .child(
            ui::Box::new()
                .layout("row")
                .width(Length::Pct(100.0))
                .child(ui::Box::new().width(Length::Pct(25.0)).build())
                .child(
                    ui::Box::new()
                        .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                        .child((services.tick_strip)(axis_ticks))
                        .build(),
                )
                .build(),
        );
    let plot_probe = probe.clone();
    content = content.child(
        ui::Canvas::new()
            .style(
                Style::new()
                    .absolute_at(Some(0.0), Some(0.0), None, None)
                    .width(Length::Pct(100.0))
                    .height(Length::Pct(100.0)),
            )
            .on_paint(move |bounds| {
                plot_probe.set(Some(bounds));
                Vec::new()
            })
            .build(),
    );
    let mut selection = None;
    if let Some((sample, summary)) = selected
        .and_then(|row| data.samples.get(row))
        .and_then(|sample| sample.summary.as_ref().map(|summary| (sample, summary)))
    {
        let unit = data.unit.as_deref();
        let rows = vec![
            ("n", summary.count.to_string()),
            ("min", reading(summary.min, unit)),
            ("q1", reading(summary.q1, unit)),
            ("median", reading(summary.median, unit)),
            ("q3", reading(summary.q3, unit)),
            ("max", reading(summary.max, unit)),
            ("mean", reading(summary.mean, unit)),
            ("outliers", summary.outliers.len().to_string()),
        ]
        .into_iter()
        .map(|(label, value)| ReadoutRow {
            color: None,
            label: label.to_owned(),
            value,
        })
        .collect::<Vec<_>>();
        selection = Some(label(&rows, &sample.name));
        content = content.child((services.readout)(0.5, &sample.name, rows));
    }
    navigation(
        &id,
        content.build(),
        Mode::Box,
        count,
        selection,
        states,
        theme.colors.ring.to_reference_paint(),
        services.navigation.clone(),
        invalidate,
    )
}

#[cfg(feature = "test-support")]
pub struct DistributionFixture {
    model: Arc<Distribution>,
    state: DistributionState,
}
#[cfg(feature = "test-support")]
impl DistributionFixture {
    pub fn new(json: &str) -> Self {
        let value = serde_json::from_str(json).expect("JSON");
        let model = Arc::new(super::prepare(&value).expect("distribution"));
        let state = DistributionState::default();
        state.sync("test", &model);
        Self { model, state }
    }
    pub fn replace(&mut self, json: &str) {
        let value = serde_json::from_str(json).expect("JSON");
        self.model = Arc::new(super::prepare(&value).expect("distribution"));
        self.state.sync("test", &self.model);
    }
    pub fn hover(&self, x: f32, y: f32) {
        self.state.set("test", Some((x, y)));
    }
    pub fn selected(&self) -> Option<(f32, f32)> {
        self.state.sync("test", &self.model).0
    }
    pub fn generation(&self) -> u64 {
        self.state.sync("test", &self.model).3
    }
    pub fn key(&self, key: NamedKey, alt: bool) -> bool {
        let count = match self.model.mode {
            Mode::Box => self.model.samples.len(),
            Mode::Histogram => self.model.bins.count,
            Mode::Cdf => 101,
        };
        self.state.key("test", self.model.mode, count, key, alt)
    }
    pub fn threshold(&self) -> f64 {
        self.state.sync("test", &self.model).2.borrow().value()
    }
    pub fn set_threshold(&self, value: f64) {
        let (domain, step, _) = self.model.slider();
        self.state
            .sync("test", &self.model)
            .2
            .borrow_mut()
            .reset(SliderRange::new(domain.lo, domain.hi, Some(step)), value);
    }
    pub fn pending_reveal(&self) -> Option<(String, usize)> {
        self.state.pending_reveal("test")
    }
    pub fn revealed(&self, key: &str, index: usize) {
        self.state.revealed("test", key, index);
    }
}
