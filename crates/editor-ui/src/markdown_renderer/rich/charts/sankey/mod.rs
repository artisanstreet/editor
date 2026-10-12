//! `sankey`: flows between categories, laid out in ranks.
//!
//! Nodes rank by the longest path from the roots (links closing a cycle are
//! left out of the drawing); a node is as tall as its larger side of flow;
//! links are filled cubic bands from a source's right edge to a target's
//! left edge, stacked in the order of the nodes they reach.

use std::collections::HashMap;
use std::rc::Rc;

use gpui::{AnyElement, IntoElement as _};
use serde::Deserialize;
use serde_json::Value;

use super::super::{BlockContext, RichBlock, kit, spec};
use super::scale::{exact, f32_of, f64_of};

mod view;

#[derive(Clone, Debug, Deserialize)]
struct NodeSpec {
    id: String,
    #[serde(default)]
    label: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct LinkSpec {
    from: String,
    to: String,
    value: f64,
}

/// The `sankey` spec.
#[derive(Clone, Debug, Deserialize)]
pub(super) struct SankeySpec {
    #[serde(default)]
    nodes: Vec<NodeSpec>,
    links: Vec<LinkSpec>,
}

/// A drawable flow between two nodes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Link {
    pub(super) from: usize,
    pub(super) to: usize,
    pub(super) value: f64,
}

/// The nodes and the flows the drawing uses.
#[derive(Clone, Debug)]
pub(super) struct Graph {
    pub(super) labels: Vec<String>,
    /// Positive, finite flows between distinct nodes that close no cycle.
    pub(super) links: Vec<Link>,
    /// Each node's rank: the longest path to it from a root.
    pub(super) ranks: Vec<usize>,
}

impl Graph {
    pub(super) fn from_spec(spec: &SankeySpec) -> Self {
        let mut labels = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut node = |id: &str, label: Option<&str>, labels: &mut Vec<String>| -> usize {
            *index.entry(id.to_owned()).or_insert_with(|| {
                let label = label
                    .map(str::trim)
                    .filter(|label| !label.is_empty())
                    .unwrap_or(id);
                labels.push(label.to_owned());
                labels.len() - 1
            })
        };
        for spec in &spec.nodes {
            node(&spec.id, spec.label.as_deref(), &mut labels);
        }
        let mut links = Vec::new();
        for spec in &spec.links {
            let from = node(&spec.from, None, &mut labels);
            let to = node(&spec.to, None, &mut labels);
            if from != to && spec.value.is_finite() && spec.value > 0.0 {
                links.push(Link {
                    from,
                    to,
                    value: spec.value,
                });
            }
        }
        let links = acyclic(labels.len(), links);
        let ranks = ranks(labels.len(), &links);
        Self {
            labels,
            links,
            ranks,
        }
    }

    pub(super) fn inflow(&self, node: usize) -> f64 {
        self.links
            .iter()
            .filter(|link| link.to == node)
            .map(|link| link.value)
            .sum()
    }

    pub(super) fn outflow(&self, node: usize) -> f64 {
        self.links
            .iter()
            .filter(|link| link.from == node)
            .map(|link| link.value)
            .sum()
    }

    pub(super) fn throughput(&self, node: usize) -> f64 {
        self.inflow(node).max(self.outflow(node))
    }

    pub(super) fn rank_count(&self) -> usize {
        self.ranks.iter().max().map_or(0, |rank| rank + 1)
    }
}

/// `links` without those that close a cycle, found by depth-first search
/// from each node in order.
fn acyclic(count: usize, links: Vec<Link>) -> Vec<Link> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        New,
        Open,
        Done,
    }
    let mut outgoing: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (index, link) in links.iter().enumerate() {
        outgoing[link.from].push(index);
    }
    let mut marks = vec![Mark::New; count];
    let mut back = vec![false; links.len()];
    for root in 0..count {
        if marks[root] != Mark::New {
            continue;
        }
        // (node, next outgoing link to visit)
        let mut stack = vec![(root, 0)];
        marks[root] = Mark::Open;
        while let Some((node, next)) = stack.pop() {
            let Some(&link) = outgoing[node].get(next) else {
                marks[node] = Mark::Done;
                continue;
            };
            stack.push((node, next + 1));
            let target = links[link].to;
            match marks[target] {
                Mark::New => {
                    marks[target] = Mark::Open;
                    stack.push((target, 0));
                }
                Mark::Open => back[link] = true,
                Mark::Done => {}
            }
        }
    }
    links
        .into_iter()
        .zip(back)
        .filter_map(|(link, back)| (!back).then_some(link))
        .collect()
}

/// The longest path from a root to each node of an acyclic graph.
fn ranks(count: usize, links: &[Link]) -> Vec<usize> {
    let mut incoming = vec![0usize; count];
    for link in links {
        incoming[link.to] += 1;
    }
    let mut ranks = vec![0; count];
    let mut ready: Vec<usize> = (0..count).filter(|node| incoming[*node] == 0).collect();
    while let Some(node) = ready.pop() {
        for link in links.iter().filter(|link| link.from == node) {
            ranks[link.to] = ranks[link.to].max(ranks[node] + 1);
            incoming[link.to] -= 1;
            if incoming[link.to] == 0 {
                ready.push(link.to);
            }
        }
    }
    ranks
}

/// A node's place: its rank and its vertical extent in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct NodeBox {
    pub(super) rank: usize,
    pub(super) y0: f32,
    pub(super) y1: f32,
}

/// A link's band: its vertical extent at the source and at the target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Band {
    pub(super) source: (f32, f32),
    pub(super) target: (f32, f32),
}

/// The drawing in pixels down and ranks across.
#[derive(Clone, Debug)]
pub(super) struct Layout {
    pub(super) ranks: usize,
    pub(super) nodes: Vec<NodeBox>,
    /// One band per [`Graph::links`] entry.
    pub(super) bands: Vec<Band>,
}

impl Layout {
    /// Where rank `rank` sits across the plot, 0..=1.
    pub(super) fn column(&self, rank: usize) -> f32 {
        if self.ranks < 2 {
            0.5
        } else {
            f32_of(f64_of(rank) / f64_of(self.ranks - 1))
        }
    }
}

/// Lays `graph` out in `height` pixels with `gap` pixels between nodes.
pub(super) fn layout(graph: &Graph, height: f32, gap: f32) -> Layout {
    let ranks = graph.rank_count();
    let mut columns: Vec<Vec<usize>> = vec![Vec::new(); ranks];
    for (node, rank) in graph.ranks.iter().enumerate() {
        columns[*rank].push(node);
    }
    let values: Vec<f64> = (0..graph.labels.len())
        .map(|node| graph.throughput(node))
        .collect();
    let scale = columns
        .iter()
        .filter_map(|column| {
            let total: f64 = column.iter().map(|node| values[*node]).sum();
            let room = f64::from(height - gap * f32_of(f64_of(column.len().saturating_sub(1))));
            (total > 0.0).then(|| room.max(1.0) / total)
        })
        .fold(f64::INFINITY, f64::min);
    let scale = if scale.is_finite() { scale } else { 1.0 };
    let mut nodes = vec![
        NodeBox {
            rank: 0,
            y0: 0.0,
            y1: 0.0
        };
        graph.labels.len()
    ];
    for (rank, column) in columns.iter_mut().enumerate() {
        if rank > 0 {
            order_by_sources(column, graph, &nodes);
        }
        let heights: Vec<f32> = column
            .iter()
            .map(|node| f32_of(values[*node] * scale).max(2.0))
            .collect();
        let total =
            heights.iter().sum::<f32>() + gap * f32_of(f64_of(column.len().saturating_sub(1)));
        let mut y = ((height - total) / 2.0).max(0.0);
        for (node, node_height) in column.iter().zip(heights) {
            nodes[*node] = NodeBox {
                rank,
                y0: y,
                y1: y + node_height,
            };
            y += node_height + gap;
        }
    }
    let bands = bands(graph, &nodes, scale);
    Layout {
        ranks,
        nodes,
        bands,
    }
}

/// Sorts a column by the flow-weighted middle of each node's sources.
fn order_by_sources(column: &mut [usize], graph: &Graph, placed: &[NodeBox]) {
    let center = |node: usize| {
        let (weight, sum) = graph.links.iter().filter(|link| link.to == node).fold(
            (0.0, 0.0),
            |(weight, sum), link| {
                let source = placed[link.from];
                let middle = f64::from(source.y0 + source.y1) / 2.0;
                (weight + link.value, sum + middle * link.value)
            },
        );
        if weight > 0.0 {
            sum / weight
        } else {
            f64::INFINITY
        }
    };
    column.sort_by(|a, b| center(*a).total_cmp(&center(*b)));
}

fn bands(graph: &Graph, nodes: &[NodeBox], scale: f64) -> Vec<Band> {
    let middle = |node: usize| f32::midpoint(nodes[node].y0, nodes[node].y1);
    let mut bands = vec![
        Band {
            source: (0.0, 0.0),
            target: (0.0, 0.0),
        };
        graph.links.len()
    ];
    for (node, place) in nodes.iter().enumerate() {
        let mut outgoing: Vec<usize> = (0..graph.links.len())
            .filter(|link| graph.links[*link].from == node)
            .collect();
        outgoing.sort_by(|a, b| middle(graph.links[*a].to).total_cmp(&middle(graph.links[*b].to)));
        let mut y = place.y0;
        for link in outgoing {
            let width = f32_of(graph.links[link].value * scale);
            bands[link].source = (y, y + width);
            y += width;
        }
        let mut incoming: Vec<usize> = (0..graph.links.len())
            .filter(|link| graph.links[*link].to == node)
            .collect();
        incoming
            .sort_by(|a, b| middle(graph.links[*a].from).total_cmp(&middle(graph.links[*b].from)));
        let mut y = place.y0;
        for link in incoming {
            let width = f32_of(graph.links[link].value * scale);
            bands[link].target = (y, y + width);
            y += width;
        }
    }
    bands
}

/// A parsed sankey: the spec for Markdown, the graph for drawing.
#[derive(Debug)]
pub(super) struct Sankey {
    spec: SankeySpec,
    graph: Rc<Graph>,
}

pub(super) fn parse(value: &Value) -> Result<Rc<dyn RichBlock>, String> {
    let spec = spec::<SankeySpec>(value)?;
    let graph = Rc::new(Graph::from_spec(&spec));
    Ok(Rc::new(Sankey { spec, graph }))
}

impl RichBlock for Sankey {
    fn render(&self, cx: &BlockContext<'_>) -> AnyElement {
        view::SankeyView {
            id: cx.key("sankey"),
            theme: *cx.theme,
            graph: Rc::clone(&self.graph),
        }
        .into_any_element()
    }

    fn markdown(&self) -> String {
        if self.spec.links.is_empty() {
            return "_No flows._\n".to_owned();
        }
        let label = |id: &str| {
            self.spec
                .nodes
                .iter()
                .find(|node| node.id == id)
                .and_then(|node| node.label.clone())
                .filter(|label| !label.trim().is_empty())
                .unwrap_or_else(|| id.to_owned())
        };
        let header: Vec<String> = ["From", "To", "Value"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let rows: Vec<Vec<String>> = self
            .spec
            .links
            .iter()
            .map(|link| vec![label(&link.from), label(&link.to), exact(link.value)])
            .collect();
        kit::markdown_table(&header, &rows)
    }
}
