//! The sankey widget: bands and nodes on a canvas, labels beside the nodes,
//! and hover that highlights the flows of a node or link.

use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, App, Bounds, ContentMask, ElementId, Hsla, IntoElement, MouseMoveEvent,
    ParentElement as _, PathBuilder, Pixels, RenderOnce, SharedString, Styled as _, Window, canvas,
    div, fill, point, prelude::*, px, relative, size,
};

use super::super::super::kit;
use super::super::frame::{self, ReadoutRow};
use super::super::scale::percent;
use super::super::scene::{Probe, probe, probed_width};
use super::{Graph, Layout, layout};
use crate::theme::ArtisanTheme;

/// Width of a node bar.
const NODE_WIDTH: f32 = 10.0;

/// Space between nodes in a column.
const NODE_GAP: f32 = 10.0;

#[derive(IntoElement)]
pub(super) struct SankeyView {
    pub(super) id: SharedString,
    pub(super) theme: ArtisanTheme,
    pub(super) graph: Arc<Graph>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Hover {
    Node(usize),
    Link(usize),
}

struct SankeyState {
    model: Arc<Graph>,
    layout: Option<Rc<Layout>>,
    hover: Option<Hover>,
    probe: Probe,
}

/// What a frame paints, in pixels within the plot.
struct Paint {
    layout: Rc<Layout>,
    graph: Arc<Graph>,
    node_colors: Vec<Hsla>,
    link_colors: Vec<Hsla>,
}

impl RenderOnce for SankeyView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let graph = &self.graph;
        if graph.links.is_empty() {
            return frame::empty(&theme, "No flows to draw");
        }
        let state = kit::keyed(window, cx, ElementId::Name(self.id.clone()), || {
            SankeyState {
                model: self.graph.clone(),
                layout: None,
                hover: None,
                probe: probe(),
            }
        });
        state.update(cx, |state, _| {
            if !Arc::ptr_eq(&state.model, &self.graph) {
                let changed = *state.model != *self.graph;
                state.model = self.graph.clone();
                if !changed {
                    return;
                }
                state.layout = None;
                state.hover = None;
            }
        });
        let (hover, plot_probe) = {
            let current = state.read(cx);
            (current.hover, current.probe.clone())
        };
        let height = (super::super::scale::f32_of(super::super::scale::f64_of(graph.tallest))
            * 44.0)
            .clamp(200.0, 380.0);
        let laid = state.update(cx, |state, _| {
            state
                .layout
                .get_or_insert_with(|| Rc::new(layout(graph, height, NODE_GAP)))
                .clone()
        });
        let lit = |link: usize| match hover {
            None => None,
            Some(Hover::Link(hovered)) => Some(hovered == link),
            Some(Hover::Node(node)) => {
                Some(graph.links[link].from == node || graph.links[link].to == node)
            }
        };
        let node_lit = |node: usize| match hover {
            None => true,
            Some(Hover::Node(hovered)) => {
                hovered == node
                    || graph.outgoing[hovered]
                        .iter()
                        .any(|index| graph.links[*index].to == node)
                    || graph.incoming[hovered]
                        .iter()
                        .any(|index| graph.links[*index].from == node)
            }
            Some(Hover::Link(link)) => {
                graph.links[link].from == node || graph.links[link].to == node
            }
        };
        let node_colors: Vec<Hsla> = (0..graph.labels.len())
            .map(|node| {
                let alpha = if graph.throughput(node) <= 0.0 {
                    0.0
                } else if node_lit(node) {
                    1.0
                } else {
                    0.3
                };
                kit::series_color(&theme, node).with_alpha(alpha).to_paint()
            })
            .collect();
        let link_colors: Vec<Hsla> = graph
            .links
            .iter()
            .enumerate()
            .map(|(index, link)| {
                let alpha = match lit(index) {
                    None => 0.32,
                    Some(true) => 0.6,
                    Some(false) => 0.08,
                };
                kit::series_color(&theme, link.from)
                    .with_alpha(alpha)
                    .to_paint()
            })
            .collect();
        let label_height = f32::from(theme.typography.control_text) * (4.0 / 3.0);
        let width = probed_width(&plot_probe, 640.0);
        let labels: Vec<_> = (0..graph.labels.len())
            .filter(|node| {
                graph.throughput(*node) > 0.0
                    && laid.nodes[*node].y1 - laid.nodes[*node].y0 >= label_height
            })
            .map(|node| self.node_label(&laid, node, node_lit(node), width))
            .collect();
        let (readout, selection_label) = hover.map(|hover| self.readout(&laid, hover)).unzip();
        let paint = Paint {
            graph: self.graph.clone(),
            layout: laid,
            node_colors,
            link_colors,
        };
        let plot = self.plot(paint, &state, plot_probe);
        let entries: Vec<Hover> = (0..graph.labels.len())
            .filter(|node| graph.throughput(*node) > 0.0)
            .map(Hover::Node)
            .chain((0..graph.links.len()).map(Hover::Link))
            .collect();
        let selected = hover.and_then(|hover| entries.iter().position(|entry| *entry == hover));
        let keyboard = state.clone();
        let mut navigation = crate::chart_navigation::ChartNavigation::new(
            kit::part(&self.id, "navigation"), theme,
            "Flow chart. Arrows explore nodes and flows; Home and End select endpoints; Escape clears the reading.",
            0..entries.len(),
            div().relative().size_full().child(plot).children(labels).children(readout),
            move |command, _, cx| keyboard.update(cx, |state, cx| {
                use crate::chart_navigation::ChartCommand;
                state.hover = match command {
                    ChartCommand::Select(index) | ChartCommand::Activate(index) => entries.get(index).copied(),
                    ChartCommand::Parent | ChartCommand::Reset => None,
                };
                cx.notify();
            }),
        ).selected(selected);
        if let Some(label) = selection_label {
            navigation = navigation.selection_label(label);
        }
        div()
            .relative()
            .w_full()
            .h(px(height))
            .child(navigation)
            .into_any_element()
    }
}

impl SankeyView {
    fn node_label(&self, laid: &Layout, node: usize, lit: bool, width: f32) -> gpui::Div {
        let theme = &self.theme;
        let place = laid.nodes[node];
        let at = laid.column(place.rank);
        let middle = f32::midpoint(place.y0, place.y1);
        let line_height = self.theme.typography.control_text * (4.0 / 3.0);
        let room = (width - NODE_WIDTH) / (laid.ranks.saturating_sub(1).max(1) as f32);
        let label = div()
            .absolute()
            .top(px(middle) - line_height / 2.0)
            .h(line_height)
            .max_w(px((room - NODE_WIDTH - 12.0).max(0.0)))
            .min_w_0()
            .overflow_hidden()
            .flex()
            .gap(px(4.0))
            .whitespace_nowrap()
            .text_size(self.theme.typography.control_text)
            .line_height(line_height)
            .opacity(if lit { 1.0 } else { 0.4 })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(theme.colors.foreground.to_paint())
                    .child(self.graph.labels[node].clone()),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(kit::format_number(self.graph.throughput(node))),
            );
        if laid.ranks > 1 && place.rank + 1 == laid.ranks {
            label.right(px(NODE_WIDTH + 6.0))
        } else {
            label
                .left(relative(at))
                .ml(px(NODE_WIDTH * (1.0 - at) + 6.0))
        }
    }

    fn readout(&self, laid: &Layout, hover: Hover) -> (gpui::Div, SharedString) {
        let graph = &self.graph;
        match hover {
            Hover::Node(node) => {
                let (inflow, outflow) = (graph.inflow(node), graph.outflow(node));
                let mut rows = Vec::new();
                if inflow > 0.0 {
                    rows.push(ReadoutRow::new(None, "In", kit::format_number(inflow)));
                }
                if outflow > 0.0 {
                    rows.push(ReadoutRow::new(None, "Out", kit::format_number(outflow)));
                }
                let heading = graph.labels[node].clone();
                let label = frame::reading_label(Some(&heading), &rows);
                (
                    frame::readout(
                        &self.theme,
                        laid.column(laid.nodes[node].rank),
                        Some(heading.into()),
                        rows,
                    ),
                    label,
                )
            }
            Hover::Link(index) => {
                let link = graph.links[index];
                let outflow = graph.outflow(link.from);
                let mut rows = vec![ReadoutRow::new(
                    None,
                    "Flow",
                    kit::format_number(link.value),
                )];
                if outflow > 0.0 {
                    rows.push(ReadoutRow::new(
                        None,
                        format!("Of {}", graph.labels[link.from]),
                        percent(link.value / outflow),
                    ));
                }
                let middle = f32::midpoint(
                    laid.column(laid.nodes[link.from].rank),
                    laid.column(laid.nodes[link.to].rank),
                );
                let heading = format!("{} → {}", graph.labels[link.from], graph.labels[link.to]);
                let label = frame::reading_label(Some(&heading), &rows);
                (
                    frame::readout(&self.theme, middle, Some(heading.into()), rows),
                    label,
                )
            }
        }
    }

    fn plot(&self, paint: Paint, state: &gpui::Entity<SankeyState>, probe: Probe) -> AnyElement {
        let hit_layout = paint.layout.clone();
        let move_probe = probe.clone();
        let moving = state.clone();
        let leaving = state.clone();
        div()
            .id(kit::part(&self.id, "plot"))
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .child(
                canvas(
                    move |bounds, _, _| probe.set(Some(bounds)),
                    move |bounds, (), window, _| paint_sankey(bounds, &paint, window),
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .on_mouse_move(move |event: &MouseMoveEvent, _, cx| {
                let hover = move_probe.get().and_then(|bounds| {
                    let x = f32::from(event.position.x - bounds.origin.x);
                    let y = f32::from(event.position.y - bounds.origin.y);
                    hit(&hit_layout, f32::from(bounds.size.width), x, y)
                });
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

/// The left edge of a rank's nodes, in pixels across `width`.
fn node_x(laid: &Layout, rank: usize, width: f32) -> f32 {
    laid.column(rank) * (width - NODE_WIDTH).max(0.0)
}

/// A band's ends: source right edge and target left edge, in pixels.
fn band_span(laid: &Layout, link: (usize, usize), width: f32) -> (f32, f32) {
    let source = node_x(laid, laid.nodes[link.0].rank, width) + NODE_WIDTH;
    let target = node_x(laid, laid.nodes[link.1].rank, width);
    (source, target)
}

/// Eases from 0 to 1 the way a band's edge does between its ends.
fn ease(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// The parameter along a band whose cubic x (controls at the middle) is `x`.
fn solve(x0: f32, x1: f32, x: f32) -> f32 {
    let middle = f32::midpoint(x0, x1);
    let at = |t: f32| {
        let u = 1.0 - t;
        u * u * u * x0 + 3.0 * u * u * t * middle + 3.0 * u * t * t * middle + t * t * t * x1
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

/// The node or link under (`x`, `y`) pixels in a plot `width` wide.
fn hit(laid: &Layout, width: f32, x: f32, y: f32) -> Option<Hover> {
    let rank_width = (width - NODE_WIDTH).max(0.0);
    let column_x = |rank| laid.column(rank) * rank_width;
    // Only columns whose horizontal node extent can contain the pointer.
    let first = if laid.ranks > 1 && rank_width > 0.0 {
        (((x - NODE_WIDTH - 2.0).max(0.0) / rank_width) * (laid.ranks - 1) as f32).floor() as usize
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
        // The column is sorted vertically during layout.
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
        // One cubic inversion per candidate rank pair, without an event-time map.
        let t = ease(solve(x0, x1, x));
        for &index in group.links.iter().rev() {
            if found.is_some_and(|previous| index <= previous) {
                break;
            }
            let band = laid.bands[index];
            let top = band.source.0 + (band.target.0 - band.source.0) * t;
            let bottom = band.source.1 + (band.target.1 - band.source.1) * t;
            if (top - 1.0..=bottom + 1.0).contains(&y) {
                found = Some(index);
                break;
            }
        }
    }
    found.map(Hover::Link)
}

fn paint_sankey(bounds: Bounds<Pixels>, paint: &Paint, window: &mut Window) {
    let width = f32::from(bounds.size.width);
    let at = |x: f32, y: f32| point(bounds.origin.x + px(x), bounds.origin.y + px(y));
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        for (index, link) in paint.graph.links.iter().enumerate() {
            let (x0, x1) = band_span(&paint.layout, (link.from, link.to), width);
            let middle = f32::midpoint(x0, x1);
            let band = paint.layout.bands[index];
            let mut builder = PathBuilder::fill();
            builder.move_to(at(x0, band.source.0));
            builder.cubic_bezier_to(
                at(x1, band.target.0),
                at(middle, band.source.0),
                at(middle, band.target.0),
            );
            builder.line_to(at(x1, band.target.1));
            builder.cubic_bezier_to(
                at(x0, band.source.1),
                at(middle, band.target.1),
                at(middle, band.source.1),
            );
            builder.close();
            if let Ok(path) = builder.build() {
                window.paint_path(path, paint.link_colors[index]);
            }
        }
        for (node, place) in paint.layout.nodes.iter().enumerate() {
            if place.y1 - place.y0 <= 0.0 {
                continue;
            }
            let origin = at(node_x(&paint.layout, place.rank, width), place.y0);
            let quad = fill(
                Bounds::new(origin, size(px(NODE_WIDTH), px(place.y1 - place.y0))),
                paint.node_colors[node],
            )
            .corner_radii(px(2.0));
            window.paint_quad(quad);
        }
    });
}

#[cfg(test)]
mod native_tests {
    use super::super::{LinkSpec, SankeySpec};
    use super::*;
    use gpui::{Context, Render, TestAppContext};
    struct Fixture {
        graph: Arc<Graph>,
    }
    impl Render for Fixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(640.0))
                .h(px(380.0))
                .debug_selector(|| "native-sankey".into())
                .child(SankeyView {
                    id: "native-sankey".into(),
                    theme: ArtisanTheme::default(),
                    graph: self.graph.clone(),
                })
        }
    }
    #[gpui::test]
    fn dense_sankey_painted_nodes_stay_inside_native_plot(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| Fixture {
            graph: Arc::new(Graph::from_spec(&SankeySpec {
                nodes: Vec::new(),
                links: (0..40)
                    .map(|index| LinkSpec {
                        from: "root".into(),
                        to: index.to_string(),
                        value: 1.0,
                    })
                    .collect(),
            })),
        });
        let bounds = cx.debug_bounds("native-sankey").unwrap();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            let scale = window.scale_factor();
            let marks: Vec<_> = window
                .painted_quads()
                .into_iter()
                .filter(|quad| {
                    (quad.bounds.size.width.as_f32() / scale - NODE_WIDTH).abs() < 0.01
                        && !quad.background.is_transparent()
                })
                .collect();
            assert_eq!(
                marks.len(),
                41,
                "each drawable node must have a real painted mark"
            );
            for mark in marks {
                let x = mark.bounds.origin.x.as_f32() / scale;
                let y = mark.bounds.origin.y.as_f32() / scale;
                let right = x + mark.bounds.size.width.as_f32() / scale;
                let bottom = y + mark.bounds.size.height.as_f32() / scale;
                assert!(
                    x >= f32::from(bounds.origin.x) - 0.01
                        && y >= f32::from(bounds.origin.y) - 0.01
                );
                assert!(
                    right <= f32::from(bounds.right()) + 0.01
                        && bottom <= f32::from(bounds.bottom()) + 0.01
                );
            }
        });
    }
}

#[cfg(test)]
mod hit_index_tests {
    use super::super::{Graph, LinkSpec, SankeySpec};
    use super::*;
    fn reference_hit(
        laid: &Layout,
        links: &[(usize, usize)],
        width: f32,
        x: f32,
        y: f32,
    ) -> Option<Hover> {
        let node = laid.nodes.iter().position(|place| {
            let left = node_x(laid, place.rank, width);
            (left - 2.0..=left + NODE_WIDTH + 2.0).contains(&x)
                && (place.y0..=place.y1).contains(&y)
        });
        if let Some(node) = node {
            return Some(Hover::Node(node));
        }
        let mut parameters = std::collections::HashMap::new();
        links.iter().enumerate().rev().find_map(|(index, link)| {
            let (x0, x1) = band_span(laid, *link, width);
            if x1 <= x0 || !(x0..=x1).contains(&x) {
                return None;
            }
            let band = laid.bands[index];
            if y < band.source.0.min(band.target.0) - 1.0
                || y > band.source.1.max(band.target.1) + 1.0
            {
                return None;
            }
            let pair = (laid.nodes[link.0].rank, laid.nodes[link.1].rank);
            let t = *parameters
                .entry(pair)
                .or_insert_with(|| ease(solve(x0, x1, x)));
            let top = band.source.0 + (band.target.0 - band.source.0) * t;
            let bottom = band.source.1 + (band.target.1 - band.source.1) * t;
            (top - 1.0..=bottom + 1.0)
                .contains(&y)
                .then_some(Hover::Link(index))
        })
    }

    #[test]
    fn grouped_index_preserves_topmost_band_and_node_hits() {
        let graph = Graph::from_spec(&SankeySpec {
            nodes: Vec::new(),
            links: vec![
                LinkSpec {
                    from: "a".into(),
                    to: "b".into(),
                    value: 4.0,
                },
                LinkSpec {
                    from: "a".into(),
                    to: "c".into(),
                    value: 3.0,
                },
                LinkSpec {
                    from: "b".into(),
                    to: "d".into(),
                    value: 2.0,
                },
                LinkSpec {
                    from: "c".into(),
                    to: "d".into(),
                    value: 2.0,
                },
                LinkSpec {
                    from: "a".into(),
                    to: "d".into(),
                    value: 1.0,
                },
            ],
        });
        let laid = super::super::layout(&graph, 300.0, 8.0);
        let links: Vec<_> = graph
            .links
            .iter()
            .map(|link| (link.from, link.to))
            .collect();
        assert_eq!(
            laid.hit_groups
                .iter()
                .map(|group| group.links.len())
                .sum::<usize>(),
            links.len()
        );
        assert!(laid.hit_groups.len() < links.len());
        for x in (0..640).step_by(3) {
            for y in (0..300).step_by(3) {
                assert_eq!(
                    hit(&laid, 640.0, x as f32, y as f32),
                    reference_hit(&laid, &links, 640.0, x as f32, y as f32),
                    "at {x},{y}"
                );
            }
        }
    }
}
