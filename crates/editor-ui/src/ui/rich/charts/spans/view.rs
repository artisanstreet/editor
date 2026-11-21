use super::Layout;
use crate::ui::rich::charts::keyboard::navigation_group;
use crate::ui::rich::charts::scale::{
    Domain, f32_of_count, f64_of, index_of, nice_step, nice_ticks,
};
use crate::ui::rich::charts::scene::{self, Probe, Shape};
use crate::ui::theme::{ArtisanTheme, Oklch, Opacity};
use aqua_core::geometry::CornerRadii;
use aqua_core::scene::GpuiShadowLayer;
use aqua_core::{DisplayItem, GpuiBackground, GpuiHsla, InputEvent, NamedKey, Rect};
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
            model: Arc::clone(model),
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
            entry.model = Arc::clone(model);
        }
        (entry.hover, Rc::clone(&entry.probe), entry.reset)
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
            NamedKey::Tab
            | NamedKey::Delete
            | NamedKey::Insert
            | NamedKey::ContextMenu
            | NamedKey::PageUp
            | NamedKey::PageDown
            | NamedKey::Function(_)
            | NamedKey::Unidentified => return false,
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
fn row_items(
    size: (f32, f32),
    theme: &ArtisanTheme,
    layout: &Layout,
    domain: Domain,
    ticks: &[Tick],
    row: usize,
    hover: Option<usize>,
) -> Vec<DisplayItem> {
    let bounds = Rect {
        x: 0.0,
        y: 0.0,
        w: size.0,
        h: size.1,
    };
    let mut items = vec![DisplayItem::PushClip {
        bounds,
        radius: 0.0,
    }];
    items.extend(scene::shape_items(
        size,
        &row_shapes(theme, layout, domain, ticks, row),
    ));
    items.push(DisplayItem::PopClip);
    for index in &layout.row_spans[row] {
        let span = &layout.spans[*index];
        let x0 = domain.fraction(span.start);
        let x1 = domain.fraction(span.end);
        let rect = Rect {
            x: bounds.w.mul_add(x0, bounds.x),
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

#[expect(
    clippy::too_many_lines,
    reason = "one spans view laying out lanes, marks, readout and navigation"
)]
pub(crate) fn view(
    id: &str,
    theme: &ArtisanTheme,
    layout: &Arc<Layout>,
    state: SpansState,
    services: &Services,
    invalidate: Rc<dyn Fn()>,
) -> Element {
    if layout.spans.is_empty() {
        return (services.empty)("No spans");
    }
    let theme = *theme;
    let (hover, probe, reset) = state.sync(id, layout);
    let width = probe.get().map_or(560.0, |bounds| bounds.w);
    let (domain, ticks) = tick_values(layout, width);
    let pitch = theme.density.control_sm;
    let model = Arc::clone(layout);
    let row_id = id.to_owned();
    let row_ticks = ticks.clone();
    let row_state = state.clone();
    let row_invalidate = Rc::clone(&invalidate);
    let text = Rc::clone(&services.text);
    let bar_label = Rc::clone(&services.bar_label);
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
            let draw_probe = Rc::clone(&row_probe);
            let draw_model = Arc::clone(&model);
            let draw_ticks = row_ticks.clone();
            let row_scene = scene::Scene::drawn(
                move |size| row_items(size, &theme, &draw_model, domain, &draw_ticks, row, hover),
                scene::Clip::Overflow,
            );
            let canvas = ui::Canvas::new()
                .style(
                    Style::new()
                        .absolute_at(Some(0.0), Some(0.0), None, None)
                        .width(Length::Pct(100.0))
                        .height(Length::Pct(100.0)),
                )
                .on_paint(move |bounds| {
                    draw_probe.set(Some(bounds));
                    row_scene.paint(bounds)
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
            let moving_probe = row_probe;
            let moving_model = Arc::clone(&model);
            let moving_invalidate = Rc::clone(&row_invalidate);
            let leaving = row_state.clone();
            let leaving_id = row_id.clone();
            let leaving_model = Arc::clone(&model);
            let leaving_invalidate = Rc::clone(&row_invalidate);
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
    let plot_probe = probe;
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
    let keyboard = state;
    let keyboard_id = id.to_owned();
    let navigation = navigation_group(plot.build(), id, aria, selection, move |code, alt| {
        let handled = keyboard.key(&keyboard_id, code, alt);
        if handled {
            invalidate();
        }
        handled
    });
    (services.navigation)(navigation, theme.colors.ring.to_reference_paint())
}

#[cfg(test)]
#[path = "../../../../../tests/ui/rich/charts/spans/view.rs"]
mod tests;
