//! `sankey`: flows between categories, laid out in ranks.
//!
//! Nodes rank by the longest path from the roots (links closing a cycle are
//! left out of the drawing); a node is as tall as its larger side of flow;
//! links are filled cubic bands from a source's right edge to a target's
//! left edge, stacked in the order of the nodes they reach.

use crate::markdown_renderer::rich::budget;
use std::collections::HashMap;
use std::sync::Arc;

use gpui::{AnyElement, IntoElement as _};
use serde::Deserialize;
use serde_json::Value;

use super::super::{BlockContext, RichBlock, spec};
use super::scale::{f32_of, f64_of};

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
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Graph {
    pub(super) labels: Vec<String>,
    /// Positive, finite flows between distinct nodes that close no cycle.
    pub(super) links: Vec<Link>,
    /// Each node's rank: the longest path to it from a root.
    pub(super) ranks: Vec<usize>,
    pub(super) incoming: Vec<Vec<usize>>,
    pub(super) outgoing: Vec<Vec<usize>>,
    inflows: Vec<f64>,
    outflows: Vec<f64>,
    rank_count: usize,
    pub(super) tallest: usize,
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
        let mut incoming = vec![Vec::new(); labels.len()];
        let mut outgoing = vec![Vec::new(); labels.len()];
        let mut inflows = vec![0.0; labels.len()];
        let mut outflows = vec![0.0; labels.len()];
        for (index, link) in links.iter().enumerate() {
            incoming[link.to].push(index);
            outgoing[link.from].push(index);
            inflows[link.to] += link.value;
            outflows[link.from] += link.value;
        }
        let ranks = ranks(labels.len(), &links);
        let rank_count = ranks.iter().max().map_or(0, |rank| rank + 1);
        let mut population = vec![0usize; rank_count];
        for (node, rank) in ranks.iter().copied().enumerate() {
            if inflows[node].max(outflows[node]) > 0.0 {
                population[rank] += 1;
            }
        }
        let tallest = population.into_iter().max().unwrap_or(0);
        Self {
            labels,
            links,
            ranks,
            incoming,
            outgoing,
            inflows,
            outflows,
            rank_count,
            tallest,
        }
    }

    pub(super) fn inflow(&self, node: usize) -> f64 {
        self.inflows[node]
    }

    pub(super) fn outflow(&self, node: usize) -> f64 {
        self.outflows[node]
    }

    pub(super) fn throughput(&self, node: usize) -> f64 {
        self.inflow(node).max(self.outflow(node))
    }

    pub(super) fn rank_count(&self) -> usize {
        self.rank_count
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
    let mut outgoing = vec![Vec::new(); count];
    for (index, link) in links.iter().enumerate() {
        incoming[link.to] += 1;
        outgoing[link.from].push(index);
    }
    let mut ranks = vec![0; count];
    let mut ready: Vec<usize> = (0..count).filter(|node| incoming[*node] == 0).collect();
    while let Some(node) = ready.pop() {
        for index in &outgoing[node] {
            let link = &links[*index];
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
    pub(super) columns: Vec<Vec<usize>>,
    /// Sorted by starting rank, then ending rank; each link belongs to one group.
    pub(super) hit_groups: Vec<LinkGroup>,
}

#[derive(Clone, Debug)]
pub(super) struct LinkGroup {
    pub(super) from: usize,
    pub(super) to: usize,
    pub(super) links: Vec<usize>,
    pub(super) y0: f32,
    pub(super) y1: f32,
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
        if graph.throughput(node) > 0.0 {
            columns[*rank].push(node);
        }
    }
    let values: Vec<f64> = (0..graph.labels.len())
        .map(|node| graph.throughput(node))
        .collect();
    let max_nodes = columns.iter().map(Vec::len).max().unwrap_or(0);
    let gap = if max_nodes > 1 {
        gap.min(height / f32_of(f64_of(max_nodes)) * 0.5)
    } else {
        0.0
    };
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
            .map(|node| f32_of(values[*node] * scale))
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
    let mut groups = std::collections::BTreeMap::<(usize, usize), Vec<usize>>::new();
    for (index, link) in graph.links.iter().enumerate() {
        groups
            .entry((nodes[link.from].rank, nodes[link.to].rank))
            .or_default()
            .push(index);
    }
    let hit_groups = groups
        .into_iter()
        .map(|((from, to), links)| {
            let y0 = links
                .iter()
                .map(|index| bands[*index].source.0.min(bands[*index].target.0))
                .fold(f32::INFINITY, f32::min);
            let y1 = links
                .iter()
                .map(|index| bands[*index].source.1.max(bands[*index].target.1))
                .fold(f32::NEG_INFINITY, f32::max);
            LinkGroup {
                from,
                to,
                links,
                y0,
                y1,
            }
        })
        .collect();
    Layout {
        ranks,
        nodes,
        bands,
        columns,
        hit_groups,
    }
}

/// Sorts a column by the flow-weighted middle of each node's sources.
fn order_by_sources(column: &mut [usize], graph: &Graph, placed: &[NodeBox]) {
    let centers: HashMap<usize, f64> = column
        .iter()
        .map(|node| {
            let total = graph.inflow(*node);
            let center = if total > 0.0 {
                graph.incoming[*node]
                    .iter()
                    .map(|index| {
                        let link = &graph.links[*index];
                        f64::from(f32::midpoint(placed[link.from].y0, placed[link.from].y1))
                            * (link.value / total)
                    })
                    .sum()
            } else {
                f64::INFINITY
            };
            (*node, center)
        })
        .collect();
    column.sort_by(|a, b| centers[a].total_cmp(&centers[b]));
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
        let mut outgoing = graph.outgoing[node].clone();
        outgoing.sort_by(|a, b| middle(graph.links[*a].to).total_cmp(&middle(graph.links[*b].to)));
        let mut y = place.y0;
        for link in outgoing {
            let width = f32_of(graph.links[link].value * scale);
            bands[link].source = (y, y + width);
            y += width;
        }
        let mut incoming = graph.incoming[node].clone();
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
    graph: Arc<Graph>,
}

pub(super) fn parse(value: &Value) -> Result<Arc<dyn RichBlock>, String> {
    let spec = spec::<SankeySpec>(value)?;
    if spec.nodes.len() > 512 || spec.links.len() > 2048 {
        return Err("flow preview supports at most 512 declared nodes and 2048 flows".into());
    }
    let mut declared = HashMap::new();
    for node in &spec.nodes {
        if let Some(previous) = declared.insert(&node.id, &node.label) {
            if previous != &node.label {
                return Err(format!("conflicting labels for node `{}`", node.id));
            }
        }
    }
    for (index, link) in spec.links.iter().enumerate() {
        if link.from == link.to || !link.value.is_finite() || link.value <= 0.0 {
            return Err(format!(
                "links[{index}] needs a finite positive flow between distinct nodes"
            ));
        }
    }
    let graph = Arc::new(Graph::from_spec(&spec));
    if graph.labels.len() > 512 {
        return Err("flow preview supports at most 512 nodes".into());
    }
    if graph.links.len() != spec.links.len() {
        return Err("cyclic flows are unsupported; supply an acyclic graph".to_owned());
    }
    if !graph
        .links
        .iter()
        .map(|link| link.value)
        .sum::<f64>()
        .is_finite()
    {
        return Err("flow total exceeds the finite numeric range".to_owned());
    }
    if graph
        .inflows
        .iter()
        .chain(&graph.outflows)
        .any(|value| !value.is_finite())
    {
        return Err("flow totals exceed the finite numeric range".to_owned());
    }
    Ok(Arc::new(Sankey { graph }))
}

impl RichBlock for Sankey {
    fn resident_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + std::mem::size_of::<Graph>()
            + budget::strings(&self.graph.labels)
            + budget::vector(&self.graph.links)
            + budget::vector(&self.graph.ranks)
            + budget::vector(&self.graph.incoming)
            + self
                .graph
                .incoming
                .iter()
                .map(budget::vector)
                .sum::<usize>()
            + budget::vector(&self.graph.outgoing)
            + self
                .graph
                .outgoing
                .iter()
                .map(budget::vector)
                .sum::<usize>()
            + budget::vector(&self.graph.inflows)
            + budget::vector(&self.graph.outflows)
    }

    fn render(&self, cx: &BlockContext<'_>) -> AnyElement {
        view::SankeyView {
            id: cx.key("sankey"),
            theme: *cx.theme,
            graph: Arc::clone(&self.graph),
        }
        .into_any_element()
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    #[test]
    fn dense_rank_fits_its_plot() {
        let links: Vec<LinkSpec> = (0..40)
            .map(|index| LinkSpec {
                from: "root".into(),
                to: index.to_string(),
                value: 1.0,
            })
            .collect();
        let graph = Graph::from_spec(&SankeySpec {
            nodes: Vec::new(),
            links,
        });
        let laid = layout(&graph, 380.0, 10.0);
        for node in &laid.nodes {
            assert!(node.y0 >= 0.0 && node.y1 <= 380.01);
        }
        for band in &laid.bands {
            assert!(
                band.source.0 >= 0.0
                    && band.source.1 <= 380.01
                    && band.target.0 >= 0.0
                    && band.target.1 <= 380.01
            );
        }
    }
}

#[cfg(test)]
mod population_tests {
    use super::*;
    #[test]
    fn cached_population_counts_only_drawable_nodes() {
        let spec = SankeySpec {
            nodes: (0..100)
                .map(|id| NodeSpec {
                    id: format!("unused-{id}"),
                    label: None,
                })
                .collect(),
            links: vec![LinkSpec {
                from: "a".into(),
                to: "b".into(),
                value: 1.0,
            }],
        };
        let graph = Graph::from_spec(&spec);
        assert_eq!(graph.rank_count(), 2);
        assert_eq!(graph.tallest, 1);
    }
}
