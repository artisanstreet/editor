//! `hierarchy`: a tree of sized parts, drawn as a treemap, an icicle, or a
//! flame graph.
//!
//! The spec is the root node: `{ name?, value?, children? }`, nested. A
//! node's size is its own `value` or the sum of its children, whichever is
//! larger; the difference is the node's own share (self time in a profile)
//! and stays empty. Clicking a group zooms into it; the path above the plot
//! zooms back out.
//!
//! - **Treemap** nests rectangles, two levels below the focus by default.
//! - **Icicle** stacks levels top down, each part as wide as its share.
//! - **Flame** is the icicle upside down, the usual reading of a profile.

use crate::markdown_renderer::rich::budget;
use std::sync::Arc;

use gpui::{AnyElement, IntoElement as _, SharedString};
use serde::Deserialize;
use serde_json::Value;

use super::super::{BlockContext, RichBlock, spec};
use super::scale::f32_of;

mod view;

/// Deepest level the Markdown outline lists.
const MAX_LEVELS: usize = 12;

/// Space around a treemap group's children and the height of its name strip.
pub(super) const GROUP_PAD: f32 = 2.0;
pub(super) const GROUP_HEADER: f32 = 18.0;

/// A group smaller than this on either side draws as a single cell.
const MIN_GROUP_SIDE: f32 = 40.0;

#[derive(Clone, Debug, Default, Deserialize)]
struct NodeSpec {
    #[serde(default)]
    name: String,
    #[serde(default)]
    value: Option<f64>,
    #[serde(default)]
    children: Vec<NodeSpec>,
}

/// How the tree is drawn.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(super) enum Layout {
    #[default]
    Treemap,
    Icicle,
    Flame,
}

/// The `hierarchy` spec.
#[derive(Clone, Debug, Deserialize)]
struct HierarchySpec {
    #[serde(flatten)]
    root: NodeSpec,
    #[serde(default)]
    layout: Layout,
    #[serde(default)]
    unit: Option<String>,
    /// Levels drawn below the focus; defaults per layout.
    #[serde(default)]
    depth: Option<usize>,
}

/// One node of the flattened tree.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct TreeNode {
    pub(super) name: SharedString,
    /// The larger of the node's own value and its children's sum.
    pub(super) size: f64,
    pub(super) parent: Option<usize>,
    sibling: usize,
    /// Largest first.
    pub(super) children: Vec<usize>,
}

/// The tree, flattened; node 0 is the root.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Tree {
    pub(super) nodes: Vec<TreeNode>,
}

impl Tree {
    fn from_spec(root: &NodeSpec) -> Self {
        let mut tree = Self { nodes: Vec::new() };
        tree.push(root, None);
        tree
    }

    /// Adds `spec` and its subtree; returns its index.
    fn push(&mut self, spec: &NodeSpec, parent: Option<usize>) -> usize {
        let index = self.nodes.len();
        let name = spec.name.trim();
        self.nodes.push(TreeNode {
            name: SharedString::from(if name.is_empty() && parent.is_none() {
                "All".to_owned()
            } else {
                name.to_owned()
            }),
            size: 0.0,
            parent,
            sibling: 0,
            children: Vec::new(),
        });
        let mut children = Vec::with_capacity(spec.children.len());
        for child in &spec.children {
            let child = self.push(child, Some(index));
            if self.nodes[child].size > 0.0 {
                children.push(child);
            } else {
                self.nodes.truncate(child);
            }
        }
        children.sort_by(|a, b| self.nodes[*b].size.total_cmp(&self.nodes[*a].size));
        for (sibling, child) in children.iter().enumerate() {
            self.nodes[*child].sibling = sibling;
        }
        let sum: f64 = children.iter().map(|child| self.nodes[*child].size).sum();
        let own = spec
            .value
            .filter(|value| value.is_finite() && *value > 0.0)
            .unwrap_or(0.0);
        let node = &mut self.nodes[index];
        node.size = own.max(sum);
        node.children = children;
        index
    }

    pub(super) fn node(&self, index: usize) -> &TreeNode {
        &self.nodes[index]
    }

    /// The path from the root down to `index`.
    pub(super) fn path(&self, index: usize) -> Vec<usize> {
        let mut path = vec![index];
        let mut at = index;
        while let Some(parent) = self.nodes[at].parent {
            path.push(parent);
            at = parent;
        }
        path.reverse();
        path
    }

    /// Which child of `focus` holds `index`; `None` for `focus` itself or a
    /// node outside it. Colors follow it, so a branch keeps one hue.
    pub(super) fn branch(&self, focus: usize, index: usize) -> Option<usize> {
        let mut at = index;
        while let Some(parent) = self.nodes[at].parent {
            if parent == focus {
                return Some(self.nodes[at].sibling);
            }
            at = parent;
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Treemap

/// A rectangle in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Rect {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) w: f32,
    pub(super) h: f32,
}

impl Rect {
    fn inset(self, left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self {
            x: self.x + left,
            y: self.y + top,
            w: (self.w - left - right).max(0.0),
            h: (self.h - top - bottom).max(0.0),
        }
    }
}

/// One drawn treemap cell.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Cell {
    pub(super) node: usize,
    pub(super) rect: Rect,
    /// A group draws its children inside it under a name strip.
    pub(super) group: bool,
}

/// The treemap of `focus`'s subtree in a `width` by `height` pixel plot,
/// `levels` deep. Groups come before their children, so painting in order
/// puts children on top.
pub(super) fn treemap(
    tree: &Tree,
    focus: usize,
    levels: usize,
    width: f32,
    height: f32,
) -> Vec<Cell> {
    let mut cells = Vec::new();
    let plot = Rect {
        x: 0.0,
        y: 0.0,
        w: width,
        h: height,
    };
    if tree.node(focus).children.is_empty() {
        cells.push(Cell {
            node: focus,
            rect: plot,
            group: false,
        });
    } else {
        place(tree, focus, plot, levels.clamp(1, MAX_LEVELS), &mut cells);
    }
    cells
}

fn place(tree: &Tree, parent: usize, rect: Rect, levels: usize, cells: &mut Vec<Cell>) {
    let node = tree.node(parent);
    if node.size <= 0.0 || rect.w <= 0.0 || rect.h <= 0.0 {
        return;
    }
    let area = f64::from(rect.w * rect.h);
    let mut items: Vec<(Option<usize>, f64)> = node
        .children
        .iter()
        .map(|child| (Some(*child), tree.node(*child).size / node.size * area))
        .collect();
    let own_area = area - items.iter().map(|(_, area)| area).sum::<f64>();
    if own_area > area * 1e-9 {
        // The parent's own share: laid out, then left empty.
        items.push((Some(parent), own_area));
        items.sort_by(|a, b| b.1.total_cmp(&a.1));
    }
    for (child, rect) in squarify(&items, rect) {
        let Some(child) = child else { continue };
        let nested = child != parent
            && levels > 1
            && !tree.node(child).children.is_empty()
            && rect.w >= MIN_GROUP_SIDE
            && rect.h >= MIN_GROUP_SIDE;
        cells.push(Cell {
            node: child,
            rect,
            group: nested,
        });
        if nested {
            let inner = rect.inset(GROUP_PAD, GROUP_HEADER, GROUP_PAD, GROUP_PAD);
            place(tree, child, inner, levels - 1, cells);
        }
    }
}

/// Squarified treemap layout (Bruls, Huizing, van Wijk): `items` are areas
/// in square pixels, largest first, summing to `rect`'s area.
pub(super) fn squarify<T: Copy>(items: &[(T, f64)], rect: Rect) -> Vec<(T, Rect)> {
    let mut out = Vec::with_capacity(items.len());
    let mut rect = rect;
    let mut start = 0;
    while start < items.len() {
        let short = f64::from(rect.w.min(rect.h));
        if short <= 0.0 {
            break;
        }
        let mut end = start + 1;
        let mut sum = items[start].1;
        let mut min = sum;
        let mut max = sum;
        let ratio = |sum: f64, min: f64, max: f64| {
            let side = short * short;
            (side * max / (sum * sum)).max(sum * sum / (side * min))
        };
        let mut worst = ratio(sum, min, max);
        while end < items.len() {
            let area = items[end].1;
            let next = ratio(sum + area, min.min(area), max.max(area));
            if next > worst {
                break;
            }
            worst = next;
            sum += area;
            min = min.min(area);
            max = max.max(area);
            end += 1;
        }
        let row = &items[start..end];

        if rect.w >= rect.h {
            // A column along the left edge.
            let width = f32_of(sum / f64::from(rect.h)).min(rect.w);
            let mut y = rect.y;
            for (item, area) in row {
                let height = f32_of(area / sum) * rect.h;
                out.push((
                    *item,
                    Rect {
                        x: rect.x,
                        y,
                        w: width,
                        h: height,
                    },
                ));
                y += height;
            }
            rect = rect.inset(width, 0.0, 0.0, 0.0);
        } else {
            // A row along the top edge.
            let height = f32_of(sum / f64::from(rect.w)).min(rect.h);
            let mut x = rect.x;
            for (item, area) in row {
                let width = f32_of(area / sum) * rect.w;
                out.push((
                    *item,
                    Rect {
                        x,
                        y: rect.y,
                        w: width,
                        h: height,
                    },
                ));
                x += width;
            }
            rect = rect.inset(0.0, height, 0.0, 0.0);
        }
        start = end;
    }
    out
}

// ---------------------------------------------------------------------------
// Icicle and flame

/// One bar of an icicle or flame graph: plot fractions across, a level down.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Bar {
    pub(super) node: usize,
    pub(super) x0: f32,
    pub(super) x1: f32,
    /// 0 for the focus, 1 for its children, and so on.
    pub(super) level: usize,
}

/// The partition of `focus`'s subtree, `levels` deep below it; the focus
/// spans the full width on level 0.
pub(super) fn partition(tree: &Tree, focus: usize, levels: usize) -> Vec<Bar> {
    let mut bars = vec![Bar {
        node: focus,
        x0: 0.0,
        x1: 1.0,
        level: 0,
    }];
    let mut next = 0;
    while next < bars.len() {
        let Bar {
            node,
            x0,
            x1,
            level,
        } = bars[next].clone();
        next += 1;
        if level >= levels {
            continue;
        }
        let parent = tree.node(node);
        let mut x = x0;
        for child in &parent.children {
            let width = f32_of(tree.node(*child).size / parent.size) * (x1 - x0);
            bars.push(Bar {
                node: *child,
                x0: x,
                x1: x + width,
                level: level + 1,
            });
            x += width;
        }
    }
    bars
}

// ---------------------------------------------------------------------------
// Block

#[derive(Debug)]
pub(super) struct Hierarchy {
    layout: Layout,
    unit: Option<String>,
    depth: Option<usize>,
    tree: Arc<Tree>,
}

pub(super) fn parse(value: &Value) -> Result<Arc<dyn RichBlock>, String> {
    let spec = spec::<HierarchySpec>(value)?;
    let tree = Tree::from_spec(&spec.root);
    if tree.nodes.len() > 4096 {
        return Err("hierarchy preview supports at most 4096 nodes".into());
    }
    if tree.node(0).size <= 0.0 || !tree.node(0).size.is_finite() {
        return Err("needs a finite positive total `value` on the root or its children".to_owned());
    }
    Ok(Arc::new(Hierarchy {
        layout: spec.layout,
        unit: spec.unit,
        depth: spec.depth,
        tree: Arc::new(tree),
    }))
}

impl Hierarchy {
    fn levels(&self) -> usize {
        self.depth
            .filter(|depth| *depth > 0)
            .unwrap_or(match self.layout {
                Layout::Treemap => 2,
                Layout::Icicle | Layout::Flame => 6,
            })
            .min(MAX_LEVELS)
    }
}

impl RichBlock for Hierarchy {
    fn resident_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + std::mem::size_of::<Tree>()
            + budget::optional(&self.unit)
            + budget::vector(&self.tree.nodes)
            + self
                .tree
                .nodes
                .iter()
                .map(|node| node.name.len() + budget::vector(&node.children))
                .sum::<usize>()
    }

    fn render(&self, cx: &BlockContext<'_>) -> AnyElement {
        view::HierarchyView {
            id: cx.key("hierarchy"),
            theme: *cx.theme,
            tree: Arc::clone(&self.tree),
            layout: self.layout,
            unit: self.unit.clone().map(SharedString::from),
            levels: self.levels(),
        }
        .into_any_element()
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    #[test]
    fn positive_focused_leaf_fills_the_treemap() {
        let tree = Tree::from_spec(&NodeSpec {
            name: "root".into(),
            value: Some(10.0),
            children: Vec::new(),
        });
        let cells = treemap(&tree, 0, 2, 320.0, 240.0);
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].node, 0);
        assert_eq!(
            cells[0].rect,
            Rect {
                x: 0.0,
                y: 0.0,
                w: 320.0,
                h: 240.0
            }
        );
    }
}
