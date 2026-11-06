use super::Layout;
use crate::ui::rich::charts::scale::{
    Domain, f32_of_count, f64_of, index_of, nice_step, nice_ticks,
};
use crate::ui::rich::charts::scene::{self, Probe, Shape};
use crate::ui::theme::{ArtisanTheme, Oklch, Opacity};
use aqua_core::geometry::CornerRadii;
use aqua_core::scene::GpuiShadowLayer;
use aqua_core::{
    DisplayItem, GpuiBackground, GpuiHsla, InputEvent, Key, Modifiers, NamedKey, Rect, Role,
};
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
pub(crate) struct BarLabel {
    pub x0: f32,
    pub x1: f32,
    pub row_pitch: f32,
    pub text: String,
}
pub(crate) type TextFactory = Rc<dyn Fn(&str, f32, Oklch) -> Element>;
pub(crate) type BarLabelFactory = Rc<dyn Fn(BarLabel) -> Element>;
pub(crate) type MarkerFactory = Rc<dyn Fn(f32, &str, GpuiHsla) -> Element>;
pub(crate) type TickStripFactory = Rc<dyn Fn(Vec<Tick>) -> Element>;
pub(crate) type ReadoutFactory =
    Rc<dyn Fn(f32, Option<&str>, Vec<(Option<GpuiHsla>, String, String)>) -> Element>;
pub(crate) type EmptyFactory = Rc<dyn Fn(&str) -> Element>;
pub(crate) type NavigationFrame = Rc<dyn Fn(Element, GpuiHsla) -> Element>;
pub(crate) struct Services {
    pub text: TextFactory,
    pub bar_label: BarLabelFactory,
    pub marker: MarkerFactory,
    pub ticks: TickStripFactory,
    pub readout: ReadoutFactory,
    pub empty: EmptyFactory,
    pub navigation: NavigationFrame,
}
struct Entry {
    model: Arc<Layout>,
    hover: Option<usize>,
    probe: Probe,
    reveal: Option<usize>,
    reset: u64,
}
#[derive(Clone, Default)]
pub(crate) struct SpansState(Rc<RefCell<HashMap<String, Entry>>>);
impl SpansState {
    fn sync(&self, id: &str, model: &Arc<Layout>) -> (Option<usize>, Probe, u64) {
        let mut entries = self.0.borrow_mut();
        let entry = entries.entry(id.to_owned()).or_insert_with(|| Entry {
            model: model.clone(),
            hover: None,
            probe: Rc::new(Cell::new(None)),
            reveal: None,
            reset: 0,
        });
        if !Arc::ptr_eq(&entry.model, model) {
            if *entry.model != **model {
                entry.hover = None;
                entry.reveal = None;
                entry.reset = entry.reset.wrapping_add(1);
            }
            entry.model = model.clone();
        }
        (entry.hover, entry.probe.clone(), entry.reset)
    }
    fn hover(&self, id: &str, value: Option<usize>) -> bool {
        let mut entries = self.0.borrow_mut();
        let Some(entry) = entries.get_mut(id) else {
            return false;
        };
        let changed = entry.hover != value;
        entry.hover = value;
        changed
    }
    fn key(&self, id: &str, code: NamedKey, alt: bool) -> bool {
        let mut entries = self.0.borrow_mut();
        let Some(entry) = entries.get_mut(id) else {
            return false;
        };
        let count = entry.model.spans.len();
        if count == 0 {
            return false;
        }
        let next = match code {
            NamedKey::Escape | NamedKey::Backspace => None,
            NamedKey::Left if alt => None,
            NamedKey::Home => Some(0),
            NamedKey::End => Some(count - 1),
            NamedKey::Left | NamedKey::Up => Some(entry.hover.map_or(0, |n| n.saturating_sub(1))),
            NamedKey::Right | NamedKey::Down => {
                Some(entry.hover.map_or(0, |n| (n + 1).min(count - 1)))
            }
            NamedKey::Enter => Some(entry.hover.unwrap_or(0)),
            _ => return false,
        };
        entry.hover = next;
        entry.reveal = next.map(|index| entry.model.spans[index].row);
        true
    }
    pub(crate) fn pending_reveal(&self, id: &str) -> Option<(String, usize)> {
        let entries = self.0.borrow();
        let entry = entries.get(id)?;
        Some((format!("{id}-rows-{}", entry.reset), entry.reveal?))
    }
    pub(crate) fn revealed(&self, id: &str, key: &str, row: usize) {
        if let Some(entry) = self.0.borrow_mut().get_mut(id)
            && key == format!("{id}-rows-{}", entry.reset)
            && entry.reveal == Some(row)
        {
            entry.reveal = None;
        }
    }
    pub(crate) fn remove(&self, id: &str) {
        self.0.borrow_mut().remove(id);
    }
}
fn tick_values(layout: &Layout, width: f32) -> (Domain, Vec<Tick>) {
    let budget = index_of(f64::from((width / 96.0).floor().clamp(2.0, 24.0)));
    let ticks = if layout.dates {
        let step = nice_step(layout.domain.span() / f64_of(budget))
            .ceil()
            .max(1.0);
        let first = (layout.domain.lo / step).ceil() * step;
        crate::ui::rich::charts::scale::Ticks {
            domain: layout.domain,
            values: (0..=200)
                .map(|i| f64::from(i).mul_add(step, first))
                .take_while(|v| *v <= layout.domain.hi)
                .collect(),
        }
    } else {
        nice_ticks(layout.domain, budget, false, false)
    };
    let domain = ticks.domain;
    (
        domain,
        ticks
            .values
            .into_iter()
            .map(|value| Tick {
                at: domain.fraction(value),
                label: layout.time(value),
            })
            .collect(),
    )
}
fn row_shapes(
    theme: &ArtisanTheme,
    layout: &Layout,
    domain: Domain,
    ticks: &[Tick],
    row: usize,
) -> Vec<Shape> {
    let line = |points, color, dash| Shape::Line {
        points,
        width: 1.0,
        color,
        dash,
    };
    let rule = theme.colors.text.at(Opacity::Rule).to_reference_paint();
    let mut shapes = ticks
        .iter()
        .map(|tick| line(vec![(tick.at, 0.0), (tick.at, 1.0)], rule, false))
        .collect::<Vec<_>>();
    if layout
        .lanes
        .iter()
        .skip(1)
        .any(|lane| lane.first_row == row)
    {
        shapes.push(line(vec![(0.0, 0.0), (1.0, 0.0)], rule, false));
    }
    let marker = theme
        .colors
        .text
        .at(Opacity::EventRule)
        .to_reference_paint();
    for item in &layout.markers {
        let x = domain.fraction(item.at);
        shapes.push(line(vec![(x, 0.0), (x, 1.0)], marker, true));
    }
    shapes
}
fn row_paint(
    bounds: Rect,
    theme: &ArtisanTheme,
    layout: &Layout,
    domain: Domain,
    ticks: &[Tick],
    row: usize,
    hover: Option<usize>,
) -> Vec<DisplayItem> {
    let mut items = scene::source_scene(bounds, &row_shapes(theme, layout, domain, ticks, row));
    for index in &layout.row_spans[row] {
        let span = &layout.spans[*index];
        let x0 = domain.fraction(span.start);
        let x1 = domain.fraction(span.end);
        let rect = Rect {
            x: bounds.x + bounds.w * x0,
            y: bounds.y + 2.0,
            w: (bounds.w * (x1 - x0)).max(2.0),
            h: (bounds.h - 4.0).max(0.0),
        };
        let selected = hover == Some(*index);
        let color = theme.colors.palette[span.color % 8]
            .at(if selected {
                Opacity::SpanHover
            } else {
                Opacity::Span
            })
            .to_reference_paint();
        let corners = CornerRadii::uniform(4.0);
        items.push(DisplayItem::GpuiBackground {
            bounds: rect,
            corners,
            background: GpuiBackground::Solid(color),
        });
        if selected {
            items.push(DisplayItem::GpuiShadow {
                bounds: rect,
                corners,
                layer: GpuiShadowLayer {
                    dx: 0.0,
                    dy: 0.0,
                    blur: 0.0,
                    spread: 0.5,
                    inset: true,
                    color: theme
                        .colors
                        .text
                        .at(Opacity::HoverRing)
                        .to_reference_paint(),
                },
            });
        }
    }
    items
}

fn hit_bar(
    layout: &Layout,
    domain: Domain,
    row: usize,
    bounds: Rect,
    x: f32,
    y: f32,
) -> Option<usize> {
    if y < 2.0 || y > bounds.h - 2.0 {
        return None;
    }
    layout.row_spans[row].iter().rev().copied().find(|index| {
        let span = &layout.spans[*index];
        let left = bounds.w * domain.fraction(span.start);
        let width = (bounds.w * (domain.fraction(span.end) - domain.fraction(span.start))).max(2.0);
        (left..=left + width).contains(&x)
    })
}

pub(crate) fn view(
    id: String,
    theme: &ArtisanTheme,
    layout: Arc<Layout>,
    state: SpansState,
    services: &Services,
    invalidate: Rc<dyn Fn()>,
) -> Element {
    if layout.spans.is_empty() {
        return (services.empty)("No spans");
    }
    let theme = *theme;
    let (hover, probe, reset) = state.sync(&id, &layout);
    let width = probe.get().map_or(560.0, |bounds| bounds.w);
    let (domain, ticks) = tick_values(&layout, width);
    let pitch = theme.density.control_sm;
    let model = layout.clone();
    let row_id = id.clone();
    let row_ticks = ticks.clone();
    let row_state = state.clone();
    let row_invalidate = invalidate.clone();
    let text = services.text.clone();
    let bar_label = services.bar_label.clone();
    let rows = List::new()
        .items(0..layout.rows)
        .key(|row| *row)
        .virtualize()
        .estimated_row_height(Length::Px(pitch))
        .style(
            Style::new()
                .width(Length::Pct(100.0))
                .height(Length::Px(pitch * f32_of_count(layout.rows.min(10)))),
        )
        .row(move |row| {
            let row = *row;
            let name = if model.lanes.is_empty() {
                model.row_spans[row]
                    .first()
                    .map(|index| model.spans[*index].label.clone())
                    .unwrap_or_default()
            } else {
                let lane = model
                    .lanes
                    .partition_point(|lane| lane.first_row <= row)
                    .saturating_sub(1);
                model
                    .lanes
                    .get(lane)
                    .map(|lane| lane.name.clone())
                    .unwrap_or_default()
            };
            let name = ui::Box::new()
                .style(
                    Style::new()
                        .width(Length::Px(96.0))
                        .min_width(Length::Px(0.0))
                        .padding_insets(aqua_core::Insets {
                            right: 8.0,
                            ..aqua_core::Insets::default()
                        }),
                )
                .clip(true)
                .child(text(
                    &name,
                    theme.typography.control_text,
                    theme.colors.text_muted,
                ))
                .build();
            let row_probe: Probe = Rc::new(Cell::new(None));
            let draw_probe = row_probe.clone();
            let draw_model = model.clone();
            let draw_ticks = row_ticks.clone();
            let canvas = ui::Canvas::new()
                .style(
                    Style::new()
                        .absolute_at(Some(0.0), Some(0.0), None, None)
                        .width(Length::Pct(100.0))
                        .height(Length::Pct(100.0)),
                )
                .on_paint(move |bounds| {
                    draw_probe.set(Some(bounds));
                    row_paint(bounds, &theme, &draw_model, domain, &draw_ticks, row, hover)
                })
                .build();
            let mut content = ui::Box::new()
                .style(
                    Style::new()
                        .flex_grow(1.0)
                        .min_width(Length::Px(0.0))
                        .height(Length::Pct(100.0)),
                )
                .child(canvas);
            for index in &model.row_spans[row] {
                let span = &model.spans[*index];
                let x0 = domain.fraction(span.start);
                let x1 = domain.fraction(span.end);
                if (x1 - x0) * width >= 44.0 {
                    let label = if model.lanes.is_empty() {
                        model.duration(span)
                    } else {
                        span.label.clone()
                    };
                    content = content.child(bar_label(BarLabel {
                        x0,
                        x1,
                        row_pitch: pitch,
                        text: label,
                    }));
                }
            }
            let moving = row_state.clone();
            let moving_id = row_id.clone();
            let moving_probe = row_probe.clone();
            let moving_model = model.clone();
            let moving_invalidate = row_invalidate.clone();
            let leaving = row_state.clone();
            let leaving_id = row_id.clone();
            let leaving_model = model.clone();
            let leaving_invalidate = row_invalidate.clone();
            let content = content
                .build()
                .on_input(InputKind::PointerMove, EventPhase::Bubble, move |event| {
                    let RoutedInput::Native(InputEvent::PointerMove(pointer)) = &event.input else {
                        return InputResponse::default();
                    };
                    let selected = moving_probe.get().and_then(|bounds| {
                        hit_bar(
                            &moving_model,
                            domain,
                            row,
                            bounds,
                            pointer.position.x - bounds.x,
                            pointer.position.y - bounds.y,
                        )
                    });
                    if moving.hover(&moving_id, selected) {
                        moving_invalidate();
                    }
                    InputResponse::default()
                })
                .on_hover_change(move |over| {
                    if over {
                        return;
                    }
                    let current = leaving
                        .0
                        .borrow()
                        .get(&leaving_id)
                        .and_then(|entry| entry.hover);
                    if current.is_some_and(|index| leaving_model.spans[index].row == row)
                        && leaving.hover(&leaving_id, None)
                    {
                        leaving_invalidate();
                    }
                });
            ui::Box::new()
                .layout("row")
                .style(
                    Style::new()
                        .width(Length::Pct(100.0))
                        .height(Length::Px(pitch)),
                )
                .child(name)
                .child(content)
                .build()
                .with_key(format!("{row_id}-row-{row}"))
        })
        .build()
        .with_key(format!("{id}-rows-{reset}"));
    let mut plot = ui::Box::new().layout("column").width(Length::Pct(100.0));
    if !layout.markers.is_empty() {
        let mut header =
            ui::Box::new().style(Style::new().height(Length::Px(20.0)).padding_insets(
                aqua_core::Insets {
                    left: 96.0,
                    ..aqua_core::Insets::default()
                },
            ));
        for marker in &layout.markers {
            if !marker.label.is_empty() {
                header = header.child((services.marker)(
                    domain.fraction(marker.at),
                    &marker.label,
                    theme.colors.text.to_reference_paint(),
                ));
            }
        }
        plot = plot.child(header.build());
    }
    plot = plot.child(rows).child(
        ui::Box::new()
            .style(Style::new().padding_insets(aqua_core::Insets {
                left: 96.0,
                ..aqua_core::Insets::default()
            }))
            .child((services.ticks)(ticks))
            .build(),
    );
    let plot_probe = probe.clone();
    plot = plot.child(
        ui::Canvas::new()
            .style(
                Style::new()
                    .absolute_at(Some(96.0), Some(0.0), Some(0.0), None)
                    .height(Length::Px(1.0)),
            )
            .on_paint(move |bounds| {
                plot_probe.set(Some(bounds));
                Vec::new()
            })
            .build(),
    );
    let mut selection = None;
    if let Some(span) = hover.and_then(|index| layout.spans.get(index)) {
        let mut rows = vec![
            (
                Some(theme.colors.palette[span.color % 8].to_reference_paint()),
                "Length".to_owned(),
                layout.duration(span),
            ),
            (None, "Start".to_owned(), layout.time(span.start)),
            (None, "End".to_owned(), layout.time(span.end)),
        ];
        if let Some(lane) = span.lane.and_then(|lane| layout.lanes.get(lane)) {
            rows.push((None, "Lane".to_owned(), lane.name.clone()));
        }
        if let Some(group) = &span.group {
            rows.push((None, "Group".to_owned(), group.clone()));
        }
        let heading = (!span.label.is_empty()).then_some(span.label.as_str());
        let mut pieces = heading.into_iter().map(str::to_owned).collect::<Vec<_>>();
        pieces.extend(
            rows.iter()
                .map(|(_, label, value)| format!("{label}: {value}")),
        );
        selection = Some(pieces.join("; "));
        plot = plot.child((services.readout)(
            f32::midpoint(domain.fraction(span.start), domain.fraction(span.end)),
            heading,
            rows,
        ));
    }
    let mut aria =
        "Timeline. Arrows explore spans; Home and End select endpoints; Escape clears the reading."
            .to_owned();
    if let Some(value) = &selection {
        aria.push(' ');
        aria.push_str(value);
    }
    let keyboard = state.clone();
    let keyboard_id = id.clone();
    let mut navigation = plot
        .build()
        .with_key(format!("{id}-navigation"))
        .role(Role::Group)
        .focusable(true)
        .tab_index(0)
        .aria_label(aria)
        .on_input(InputKind::KeyDown, EventPhase::Bubble, move |event| {
            let RoutedInput::Native(InputEvent::KeyDown(key)) = &event.input else {
                return InputResponse::default();
            };
            let Key::Named(code) = key.key else {
                return InputResponse::default();
            };
            if !keyboard.key(&keyboard_id, code, key.modifiers.contains(Modifiers::ALT)) {
                return InputResponse::default();
            }
            invalidate();
            InputResponse {
                stop_propagation: true,
                ..InputResponse::default()
            }
        });
    if let Some(value) = selection {
        navigation = navigation.aria_value(value);
    }
    (services.navigation)(navigation, theme.colors.ring.to_reference_paint())
}

#[cfg(feature = "test-support")]
pub struct SpansFixture {
    model: Arc<Layout>,
    state: SpansState,
}
#[cfg(feature = "test-support")]
impl SpansFixture {
    pub fn new(json: &str) -> Self {
        let value = serde_json::from_str(json).expect("JSON");
        let model = Arc::new(super::prepare(&value).expect("spans"));
        let state = SpansState::default();
        state.sync("test", &model);
        Self { model, state }
    }
    pub fn replace(&mut self, json: &str) {
        let value = serde_json::from_str(json).expect("JSON");
        self.model = Arc::new(super::prepare(&value).expect("spans"));
        self.state.sync("test", &self.model);
    }
    pub fn selected(&self) -> Option<usize> {
        self.state.sync("test", &self.model).0
    }
    pub fn generation(&self) -> u64 {
        self.state.sync("test", &self.model).2
    }
    pub fn key(&self, key: NamedKey, alt: bool) -> bool {
        self.state.key("test", key, alt)
    }
    pub fn pending_reveal(&self) -> Option<(String, usize)> {
        self.state.pending_reveal("test")
    }
    pub fn revealed(&self, key: &str, row: usize) {
        self.state.revealed("test", key, row);
    }
    pub fn row(&self, index: usize) -> usize {
        self.model.spans[index].row
    }
    pub fn hit(&self, row: usize, x: f32, y: f32) -> Option<usize> {
        let (domain, _) = tick_values(&self.model, 560.0);
        hit_bar(
            &self.model,
            domain,
            row,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 560.0,
                h: 32.0,
            },
            x,
            y,
        )
    }
    pub fn bar_center(&self, index: usize) -> f32 {
        let (domain, _) = tick_values(&self.model, 560.0);
        let span = &self.model.spans[index];
        let left = 560.0 * domain.fraction(span.start);
        let width = (560.0 * (domain.fraction(span.end) - domain.fraction(span.start))).max(2.0);
        left + width / 2.0
    }
}
