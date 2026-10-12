//! The sankey widget: bands and nodes on a canvas, labels beside the nodes,
//! and hover that highlights the flows of a node or link.

use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, ContentMask, ElementId, Hsla, IntoElement, MouseMoveEvent,
    ParentElement as _, PathBuilder, Pixels, RenderOnce, SharedString, Styled as _, Window, canvas,
    div, fill, point, prelude::*, px, relative, size,
};

use super::super::super::kit;
use super::super::frame::{self, ReadoutRow};
use super::super::scale::percent;
use super::super::scene::{Probe, probe};
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
    pub(super) graph: Rc<Graph>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Hover {
    Node(usize),
    Link(usize),
}

struct SankeyState {
    hover: Option<Hover>,
    probe: Probe,
}

/// What a frame paints, in pixels within the plot.
struct Paint {
    layout: Layout,
    links: Vec<(usize, usize)>,
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
                hover: None,
                probe: probe(),
            }
        });
        let (hover, plot_probe) = {
            let current = state.read(cx);
            (current.hover, current.probe.clone())
        };
        let tallest = (0..graph.rank_count())
            .map(|rank| graph.ranks.iter().filter(|node| **node == rank).count())
            .max()
            .unwrap_or(1);
        let height = (super::super::scale::f32_of(super::super::scale::f64_of(tallest)) * 44.0)
            .clamp(200.0, 380.0);
        let laid = layout(graph, height, NODE_GAP);
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
                    || graph.links.iter().any(|link| {
                        (link.from == hovered && link.to == node)
                            || (link.to == hovered && link.from == node)
                    })
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
        let labels: Vec<_> = (0..graph.labels.len())
            .filter(|node| graph.throughput(*node) > 0.0)
            .map(|node| self.node_label(&laid, node, node_lit(node)))
            .collect();
        let readout = hover.map(|hover| self.readout(&laid, hover));
        let paint = Paint {
            links: graph
                .links
                .iter()
                .map(|link| (link.from, link.to))
                .collect(),
            layout: laid,
            node_colors,
            link_colors,
        };
        let plot = self.plot(paint, &state, plot_probe);
        div()
            .relative()
            .w_full()
            .h(px(height))
            .child(plot)
            .children(labels)
            .children(readout)
            .into_any_element()
    }
}

impl SankeyView {
    fn node_label(&self, laid: &Layout, node: usize, lit: bool) -> gpui::Div {
        let theme = &self.theme;
        let place = laid.nodes[node];
        let at = laid.column(place.rank);
        let middle = f32::midpoint(place.y0, place.y1);
        let label = div()
            .absolute()
            .top(px(middle - 8.0))
            .h(px(16.0))
            .flex()
            .gap(px(4.0))
            .whitespace_nowrap()
            .text_size(px(12.0))
            .line_height(px(16.0))
            .opacity(if lit { 1.0 } else { 0.4 })
            .child(
                div()
                    .text_color(theme.colors.foreground.to_paint())
                    .child(self.graph.labels[node].clone()),
            )
            .child(
                div()
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

    fn readout(&self, laid: &Layout, hover: Hover) -> gpui::Div {
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
                frame::readout(
                    &self.theme,
                    laid.column(laid.nodes[node].rank),
                    Some(graph.labels[node].clone().into()),
                    rows,
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
                frame::readout(
                    &self.theme,
                    middle,
                    Some(format!("{} → {}", graph.labels[link.from], graph.labels[link.to]).into()),
                    rows,
                )
            }
        }
    }

    fn plot(&self, paint: Paint, state: &gpui::Entity<SankeyState>, probe: Probe) -> AnyElement {
        let hit_layout = paint.layout.clone();
        let hit_links = paint.links.clone();
        let move_probe = probe.clone();
        let moving = state.clone();
        let leaving = state.clone();
        div()
            .id(ElementId::Name(format!("{}-plot", self.id).into()))
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
                    hit(&hit_layout, &hit_links, f32::from(bounds.size.width), x, y)
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
fn hit(laid: &Layout, links: &[(usize, usize)], width: f32, x: f32, y: f32) -> Option<Hover> {
    let node = laid.nodes.iter().position(|place| {
        let left = node_x(laid, place.rank, width);
        (left - 2.0..=left + NODE_WIDTH + 2.0).contains(&x) && (place.y0..=place.y1).contains(&y)
    });
    if let Some(node) = node {
        return Some(Hover::Node(node));
    }
    links.iter().enumerate().rev().find_map(|(index, link)| {
        let (x0, x1) = band_span(laid, *link, width);
        if x1 <= x0 || !(x0..=x1).contains(&x) {
            return None;
        }
        let t = ease(solve(x0, x1, x));
        let band = laid.bands[index];
        let top = band.source.0 + (band.target.0 - band.source.0) * t;
        let bottom = band.source.1 + (band.target.1 - band.source.1) * t;
        (top - 1.0..=bottom + 1.0)
            .contains(&y)
            .then_some(Hover::Link(index))
    })
}

fn paint_sankey(bounds: Bounds<Pixels>, paint: &Paint, window: &mut Window) {
    let width = f32::from(bounds.size.width);
    let at = |x: f32, y: f32| point(bounds.origin.x + px(x), bounds.origin.y + px(y));
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        for (index, link) in paint.links.iter().enumerate() {
            let (x0, x1) = band_span(&paint.layout, *link, width);
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
