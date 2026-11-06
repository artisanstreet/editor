use super::{Graph, Layout, layout};
use crate::ui::rich::charts::scale::{f32_of_count, index_of, percent};
use crate::ui::rich::charts::scene::Probe;
use crate::ui::rich::kit::format_number;
use crate::ui::theme::{ArtisanTheme, Opacity};
use aqua_core::geometry::CornerRadii;
use aqua_core::vector::{FillRule, GpuiPath, GpuiPathStyle, PathCommand};
use aqua_core::{
    DisplayItem, GpuiBackground, GpuiHsla, InputEvent, Key, Modifiers, NamedKey, Point, Rect, Role,
    Transform2D,
};
use aqua_ui::element::{Element, ui};
use aqua_ui::input::{EventPhase, InputKind, InputResponse, RoutedInput};
use aqua_ui::style::{Length, Style};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

const NODE_WIDTH: f32 = 10.0;
const NODE_GAP: f32 = 10.0;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hover {
    Node(usize),
    Link(usize),
}
#[derive(Clone)]
pub(crate) struct NodeLabel {
    pub x: f32,
    pub y: f32,
    pub max_width: f32,
    pub last: bool,
    pub alpha: f32,
    pub name: String,
    pub value: String,
    pub line_height: f32,
}
pub(crate) type LabelFactory = Rc<dyn Fn(NodeLabel) -> Element>;
pub(crate) type ReadoutFactory =
    Rc<dyn Fn(f32, &str, Vec<(Option<GpuiHsla>, String, String)>) -> Element>;
pub(crate) type EmptyFactory = Rc<dyn Fn(&str) -> Element>;
pub(crate) type NavigationFrame = Rc<dyn Fn(Element, GpuiHsla) -> Element>;
struct Entry {
    graph: Arc<Graph>,
    layout: Option<Rc<Layout>>,
    hover: Option<Hover>,
    probe: Probe,
}
#[derive(Clone, Default)]
pub(crate) struct SankeyState(Rc<RefCell<HashMap<String, Entry>>>);
impl SankeyState {
    fn sync(&self, id: &str, graph: &Arc<Graph>) -> (Option<Hover>, Probe, Rc<Layout>) {
        let mut entries = self.0.borrow_mut();
        let entry = entries.entry(id.to_owned()).or_insert_with(|| Entry {
            graph: graph.clone(),
            layout: None,
            hover: None,
            probe: Rc::new(Cell::new(None)),
        });
        if !Arc::ptr_eq(&entry.graph, graph) {
            if *entry.graph != **graph {
                entry.layout = None;
                entry.hover = None;
            }
            entry.graph = graph.clone();
        }
        let height = (f32_of_count(graph.tallest) * 44.0).clamp(200.0, 380.0);
        let laid = entry
            .layout
            .get_or_insert_with(|| Rc::new(layout(graph, height, NODE_GAP)))
            .clone();
        (entry.hover, entry.probe.clone(), laid)
    }
    fn set(&self, id: &str, hover: Option<Hover>) -> bool {
        let mut entries = self.0.borrow_mut();
        let Some(entry) = entries.get_mut(id) else {
            return false;
        };
        let changed = entry.hover != hover;
        entry.hover = hover;
        changed
    }
    fn key(&self, id: &str, graph: &Graph, code: NamedKey, alt: bool) -> bool {
        let entries_list = entries(graph);
        let mut entries = self.0.borrow_mut();
        let Some(state) = entries.get_mut(id) else {
            return false;
        };
        let current = state
            .hover
            .and_then(|value| entries_list.iter().position(|item| *item == value));
        let next = match code {
            NamedKey::Escape | NamedKey::Backspace => None,
            NamedKey::Left if alt => None,
            NamedKey::Home if !entries_list.is_empty() => Some(0),
            NamedKey::End if !entries_list.is_empty() => Some(entries_list.len() - 1),
            NamedKey::Left | NamedKey::Up if !entries_list.is_empty() => {
                Some(current.map_or(0, |n| n.saturating_sub(1)))
            }
            NamedKey::Right | NamedKey::Down if !entries_list.is_empty() => {
                Some(current.map_or(0, |n| (n + 1).min(entries_list.len() - 1)))
            }
            NamedKey::Enter if !entries_list.is_empty() => Some(current.unwrap_or(0)),
            _ => return false,
        };
        state.hover = next.and_then(|index| entries_list.get(index).copied());
        true
    }
    pub(crate) fn remove(&self, id: &str) {
        self.0.borrow_mut().remove(id);
    }
}
fn entries(graph: &Graph) -> Vec<Hover> {
    (0..graph.labels.len())
        .filter(|node| graph.throughput(*node) > 0.0)
        .map(Hover::Node)
        .chain((0..graph.links.len()).map(Hover::Link))
        .collect()
}

fn node_x(laid: &Layout, rank: usize, width: f32) -> f32 {
    laid.column(rank) * (width - NODE_WIDTH).max(0.0)
}

fn band_span(laid: &Layout, link: (usize, usize), width: f32) -> (f32, f32) {
    let source = node_x(laid, laid.nodes[link.0].rank, width) + NODE_WIDTH;
    let target = node_x(laid, laid.nodes[link.1].rank, width);
    (source, target)
}

fn ease(t: f32) -> f32 {
    t * t * 2.0f32.mul_add(-t, 3.0)
}

fn solve(x0: f32, x1: f32, x: f32) -> f32 {
    let middle = f32::midpoint(x0, x1);
    let at = |t: f32| {
        let u = 1.0 - t;
        (t * t * t).mul_add(
            x1,
            (3.0 * u * t * t).mul_add(middle, (3.0 * u * u * t).mul_add(middle, u * u * u * x0)),
        )
    };
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..24 {
        let mid = f32::midpoint(lo, hi);
        if at(mid) < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    f32::midpoint(lo, hi)
}

fn hit(laid: &Layout, width: f32, x: f32, y: f32) -> Option<Hover> {
    let rank_width = (width - NODE_WIDTH).max(0.0);
    let column_x = |rank| laid.column(rank) * rank_width;
    let first = if laid.ranks > 1 && rank_width > 0.0 {
        index_of(f64::from(
            ((x - NODE_WIDTH - 2.0).max(0.0) / rank_width) * f32_of_count(laid.ranks - 1),
        ))
    } else {
        0
    };
    let mut node_hit = None;
    for (rank, nodes) in laid.columns.iter().enumerate().skip(first) {
        let left = column_x(rank);
        if left - 2.0 > x {
            break;
        }
        if x > left + NODE_WIDTH + 2.0 {
            continue;
        }
        let at = nodes.partition_point(|node| laid.nodes[*node].y1 < y);
        if let Some(node) = nodes.get(at).filter(|node| laid.nodes[**node].y0 <= y) {
            node_hit = Some(node_hit.map_or(*node, |previous: usize| previous.min(*node)));
        }
    }
    if let Some(node) = node_hit {
        return Some(Hover::Node(node));
    }
    let limit = laid
        .hit_groups
        .partition_point(|group| column_x(group.from) + NODE_WIDTH <= x);
    let mut found = None;
    for group in &laid.hit_groups[..limit] {
        let (x0, x1) = (column_x(group.from) + NODE_WIDTH, column_x(group.to));
        if x1 <= x0 || x > x1 || y < group.y0 - 1.0 || y > group.y1 + 1.0 {
            continue;
        }
        let t = ease(solve(x0, x1, x));
        for &index in group.links.iter().rev() {
            if found.is_some_and(|previous| index <= previous) {
                break;
            }
            let band = laid.bands[index];
            let top = (band.target.0 - band.source.0).mul_add(t, band.source.0);
            let bottom = (band.target.1 - band.source.1).mul_add(t, band.source.1);
            if (top - 1.0..=bottom + 1.0).contains(&y) {
                found = Some(index);
                break;
            }
        }
    }
    found.map(Hover::Link)
}

fn paint(
    bounds: Rect,
    graph: &Graph,
    laid: &Layout,
    nodes: &[GpuiHsla],
    links: &[GpuiHsla],
) -> Vec<DisplayItem> {
    let mut items = vec![DisplayItem::PushClip {
        bounds,
        radius: 0.0,
    }];
    for (index, link) in graph.links.iter().enumerate() {
        let (x0, x1) = band_span(laid, (link.from, link.to), bounds.w);
        let middle = f32::midpoint(x0, x1);
        let band = laid.bands[index];
        let commands = vec![
            PathCommand::Move(Point::new(x0, band.source.0)),
            PathCommand::Cubic {
                first: Point::new(middle, band.source.0),
                second: Point::new(middle, band.target.0),
                to: Point::new(x1, band.target.0),
            },
            PathCommand::Line(Point::new(x1, band.target.1)),
            PathCommand::Cubic {
                first: Point::new(middle, band.target.1),
                second: Point::new(middle, band.source.1),
                to: Point::new(x0, band.source.1),
            },
            PathCommand::Close,
        ];
        items.push(DisplayItem::GpuiPath {
            transformed_window_logical_bounds: bounds,
            path: Arc::new(GpuiPath {
                commands_local: commands,
                origin_window_logical: Point::new(bounds.x, bounds.y),
                element_transform: Transform2D::IDENTITY,
                style: GpuiPathStyle::Fill {
                    rule: FillRule::EvenOdd,
                },
                background: GpuiBackground::Solid(links[index]),
            }),
        });
    }
    for (index, node) in laid.nodes.iter().enumerate() {
        if node.y1 <= node.y0 {
            continue;
        }
        items.push(DisplayItem::GpuiBackground {
            bounds: Rect {
                x: bounds.x + node_x(laid, node.rank, bounds.w),
                y: bounds.y + node.y0,
                w: NODE_WIDTH,
                h: node.y1 - node.y0,
            },
            corners: CornerRadii::uniform(2.0),
            background: GpuiBackground::Solid(nodes[index]),
        });
    }
    items.push(DisplayItem::PopClip);
    items
}

pub(crate) fn view(
    id: String,
    theme: &ArtisanTheme,
    graph: Arc<Graph>,
    state: SankeyState,
    label: LabelFactory,
    readout: ReadoutFactory,
    empty: EmptyFactory,
    navigation_frame: NavigationFrame,
    invalidate: Rc<dyn Fn()>,
) -> Element {
    if graph.links.is_empty() {
        return empty("No flows to draw");
    }
    let theme = *theme;
    let (hover, probe, laid) = state.sync(&id, &graph);
    let height = (f32_of_count(graph.tallest) * 44.0).clamp(200.0, 380.0);
    let width = probe.get().map_or(640.0, |rect| rect.w);
    let node_lit = |node: usize| match hover {
        None => true,
        Some(Hover::Node(selected)) => {
            selected == node
                || graph.outgoing[selected]
                    .iter()
                    .any(|i| graph.links[*i].to == node)
                || graph.incoming[selected]
                    .iter()
                    .any(|i| graph.links[*i].from == node)
        }
        Some(Hover::Link(link)) => graph.links[link].from == node || graph.links[link].to == node,
    };
    let nodes = (0..graph.labels.len())
        .map(|node| {
            let color = theme.colors.palette[node % 8];
            if graph.throughput(node) <= 0.0 {
                color.at(Opacity::Clear)
            } else if node_lit(node) {
                color
            } else {
                color.at(Opacity::NodeDimmed)
            }
            .to_reference_paint()
        })
        .collect::<Vec<_>>();
    let links = graph
        .links
        .iter()
        .enumerate()
        .map(|(i, link)| {
            let lit = match hover {
                None => None,
                Some(Hover::Link(selected)) => Some(selected == i),
                Some(Hover::Node(node)) => Some(link.from == node || link.to == node),
            };
            theme.colors.palette[link.from % 8]
                .at(match lit {
                    None => Opacity::Flow,
                    Some(true) => Opacity::FlowLit,
                    Some(false) => Opacity::FlowDimmed,
                })
                .to_reference_paint()
        })
        .collect::<Vec<_>>();
    let draw_graph = graph.clone();
    let draw_layout = laid.clone();
    let draw_probe = probe.clone();
    let canvas = ui::Canvas::new()
        .style(
            Style::new()
                .absolute_at(Some(0.0), Some(0.0), None, None)
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0)),
        )
        .on_paint(move |bounds| {
            draw_probe.set(Some(bounds));
            paint(bounds, &draw_graph, &draw_layout, &nodes, &links)
        })
        .build();
    let mut content = ui::Box::new()
        .style(
            Style::new()
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0)),
        )
        .child(canvas);
    let line_height = theme.typography.control_text * (4.0 / 3.0);
    for node in 0..graph.labels.len() {
        let place = laid.nodes[node];
        if graph.throughput(node) <= 0.0 || place.y1 - place.y0 < line_height {
            continue;
        }
        let at = laid.column(place.rank);
        let room = (width - NODE_WIDTH) / f32_of_count(laid.ranks.saturating_sub(1).max(1));
        let last = laid.ranks > 1 && place.rank + 1 == laid.ranks;
        content = content.child(label(NodeLabel {
            x: if last {
                NODE_WIDTH + 6.0
            } else {
                at * width + NODE_WIDTH.mul_add(1.0 - at, 6.0)
            },
            y: f32::midpoint(place.y0, place.y1) - line_height / 2.0,
            max_width: (room - NODE_WIDTH - 12.0).max(0.0),
            last,
            alpha: if node_lit(node) {
                1.0
            } else {
                Opacity::LabelDimmed.alpha()
            },
            name: graph.labels[node].clone(),
            value: format_number(graph.throughput(node)),
            line_height,
        }));
    }
    let mut selection = None;
    if let Some(hover) = hover {
        let (at, heading, rows) = match hover {
            Hover::Node(node) => {
                let mut rows = Vec::new();
                if graph.inflow(node) > 0.0 {
                    rows.push((None, "In".to_owned(), format_number(graph.inflow(node))));
                }
                if graph.outflow(node) > 0.0 {
                    rows.push((None, "Out".to_owned(), format_number(graph.outflow(node))));
                }
                (
                    laid.column(laid.nodes[node].rank),
                    graph.labels[node].clone(),
                    rows,
                )
            }
            Hover::Link(index) => {
                let link = graph.links[index];
                let mut rows = vec![(None, "Flow".to_owned(), format_number(link.value))];
                let outflow = graph.outflow(link.from);
                if outflow > 0.0 {
                    rows.push((
                        None,
                        format!("Of {}", graph.labels[link.from]),
                        percent(link.value / outflow),
                    ));
                }
                (
                    f32::midpoint(
                        laid.column(laid.nodes[link.from].rank),
                        laid.column(laid.nodes[link.to].rank),
                    ),
                    format!("{} → {}", graph.labels[link.from], graph.labels[link.to]),
                    rows,
                )
            }
        };
        let mut pieces = vec![heading.clone()];
        pieces.extend(
            rows.iter()
                .map(|(_, label, value)| format!("{label}: {value}")),
        );
        selection = Some(pieces.join("; "));
        content = content.child(readout(at, &heading, rows));
    }
    let moving = state.clone();
    let moving_id = id.clone();
    let moving_probe = probe.clone();
    let moving_layout = laid.clone();
    let moving_invalidate = invalidate.clone();
    let leaving = state.clone();
    let leaving_id = id.clone();
    let leaving_invalidate = invalidate.clone();
    let content = content
        .build()
        .with_key(format!("{id}-plot"))
        .on_input(InputKind::PointerMove, EventPhase::Bubble, move |event| {
            let RoutedInput::Native(InputEvent::PointerMove(pointer)) = &event.input else {
                return InputResponse::default();
            };
            let selected = moving_probe.get().and_then(|bounds| {
                hit(
                    &moving_layout,
                    bounds.w,
                    pointer.position.x - bounds.x,
                    pointer.position.y - bounds.y,
                )
            });
            if moving.set(&moving_id, selected) {
                moving_invalidate();
            }
            InputResponse::default()
        })
        .on_hover_change(move |over| {
            if !over && leaving.set(&leaving_id, None) {
                leaving_invalidate();
            }
        });
    let mut aria="Flow chart. Arrows explore nodes and flows; Home and End select endpoints; Escape clears the reading.".to_owned();
    if let Some(selection) = &selection {
        aria.push(' ');
        aria.push_str(selection);
    }
    let keyboard = state.clone();
    let keyboard_id = id.clone();
    let keyboard_graph = graph.clone();
    let mut nav = ui::Box::new()
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .child(content)
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
            if !keyboard.key(
                &keyboard_id,
                &keyboard_graph,
                code,
                key.modifiers.contains(Modifiers::ALT),
            ) {
                return InputResponse::default();
            }
            invalidate();
            InputResponse {
                stop_propagation: true,
                ..InputResponse::default()
            }
        });
    if let Some(selection) = selection {
        nav = nav.aria_value(selection);
    }
    let nav = navigation_frame(nav, theme.colors.ring.to_reference_paint());
    ui::Box::new()
        .style(
            Style::new()
                .width(Length::Pct(100.0))
                .height(Length::Px(height)),
        )
        .child(nav)
        .build()
}

#[cfg(feature = "test-support")]
pub struct SankeyFixture {
    graph: Arc<Graph>,
    state: SankeyState,
}
#[cfg(feature = "test-support")]
impl SankeyFixture {
    pub fn new(json: &str) -> Self {
        let value = serde_json::from_str(json).expect("JSON");
        let graph = Arc::new(super::prepare(&value).expect("sankey"));
        let state = SankeyState::default();
        state.sync("test", &graph);
        Self { graph, state }
    }
    pub fn replace(&mut self, json: &str) {
        let value = serde_json::from_str(json).expect("JSON");
        self.graph = Arc::new(super::prepare(&value).expect("sankey"));
        self.state.sync("test", &self.graph);
    }
    pub fn selected(&self) -> Option<(bool, usize)> {
        self.state
            .sync("test", &self.graph)
            .0
            .map(|value| match value {
                Hover::Node(i) => (true, i),
                Hover::Link(i) => (false, i),
            })
    }
    pub fn key(&self, key: NamedKey, alt: bool) -> bool {
        self.state.key("test", &self.graph, key, alt)
    }
    pub fn hit(&self, width: f32, x: f32, y: f32) -> Option<(bool, usize)> {
        let laid = self.state.sync("test", &self.graph).2;
        hit(&laid, width, x, y).map(|value| match value {
            Hover::Node(i) => (true, i),
            Hover::Link(i) => (false, i),
        })
    }
    pub fn node_center(&self, index: usize, width: f32) -> (f32, f32) {
        let laid = self.state.sync("test", &self.graph).2;
        let place = laid.nodes[index];
        (
            node_x(&laid, place.rank, width) + 5.0,
            f32::midpoint(place.y0, place.y1),
        )
    }
    pub fn link_center(&self, index: usize, width: f32) -> (f32, f32) {
        let laid = self.state.sync("test", &self.graph).2;
        let link = self.graph.links[index];
        let (x0, x1) = band_span(&laid, (link.from, link.to), width);
        let band = laid.bands[index];
        (
            f32::midpoint(x0, x1),
            f32::midpoint(
                f32::midpoint(band.source.0, band.source.1),
                f32::midpoint(band.target.0, band.target.1),
            ),
        )
    }
}
