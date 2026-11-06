mod view;
#[cfg(feature = "test-support")]
pub use view::HierarchyFixture;
pub(crate) use view::{HierarchyState, PartLabel, Services, levels, view};

use super::scale::f32_of;
use serde::Deserialize;

const MAX_LEVELS: usize = 12;

pub(crate) const GROUP_PAD: f32 = 2.0;
pub(crate) const GROUP_HEADER: f32 = 18.0;

const MIN_GROUP_SIDE: f32 = 40.0;

#[derive(Clone, Debug, Default, Deserialize)]
struct NodeSpec {
    #[serde(default)]
    name: String,
    #[serde(default)]
    value: Option<f64>,
    #[serde(default)]
    children: Vec<Self>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Layout {
    #[default]
    Treemap,
    Icicle,
    Flame,
}

#[derive(Clone, Debug, Deserialize)]
struct HierarchySpec {
    #[serde(flatten)]
    root: NodeSpec,
    #[serde(default)]
    layout: Layout,
    #[serde(default)]
    unit: Option<String>,
    #[serde(default)]
    depth: Option<usize>,
}

#[derive(Debug)]
pub(crate) struct Prepared {
    pub(crate) layout: Layout,
    pub(crate) unit: Option<String>,
    pub(crate) depth: Option<usize>,
    pub(crate) tree: std::sync::Arc<Tree>,
}
impl Prepared {
    pub(crate) fn resident_bytes(&self) -> usize {
        use crate::ui::rich::kit::{retained_optional, retained_vec};
        std::mem::size_of::<Self>()
            + std::mem::size_of::<Tree>()
            + retained_optional(self.unit.as_ref())
            + retained_vec(&self.tree.nodes)
            + self
                .tree
                .nodes
                .iter()
                .map(|node| node.name.capacity() + retained_vec(&node.children))
                .sum::<usize>()
    }
}

pub(crate) fn prepare(value: &serde_json::Value) -> Result<Prepared, String> {
    let spec = HierarchySpec::deserialize(value).map_err(|error| error.to_string())?;
    let tree = Tree::from_spec(&spec.root);
    if tree.nodes.len() > 4096 {
        return Err("hierarchy preview supports at most 4096 nodes".into());
    }
    if tree.node(0).size <= 0.0 || !tree.node(0).size.is_finite() {
        return Err("needs a finite positive total `value` on the root or its children".to_owned());
    }
    Ok(Prepared {
        layout: spec.layout,
        unit: spec.unit,
        depth: spec.depth,
        tree: std::sync::Arc::new(tree),
    })
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TreeNode {
    pub(crate) name: String,
    pub(crate) size: f64,
    pub(crate) parent: Option<usize>,
    sibling: usize,
    pub(crate) children: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Tree {
    pub(crate) nodes: Vec<TreeNode>,
}

impl Tree {
    fn from_spec(root: &NodeSpec) -> Self {
        let mut tree = Self { nodes: Vec::new() };
        tree.push(root, None);
        tree
    }

    fn push(&mut self, spec: &NodeSpec, parent: Option<usize>) -> usize {
        let index = self.nodes.len();
        let name = spec.name.trim();
        self.nodes.push(TreeNode {
            name: String::from(if name.is_empty() && parent.is_none() {
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

    pub(crate) fn node(&self, index: usize) -> &TreeNode {
        &self.nodes[index]
    }

    pub(crate) fn path(&self, index: usize) -> Vec<usize> {
        let mut path = vec![index];
        let mut at = index;
        while let Some(parent) = self.nodes[at].parent {
            path.push(parent);
            at = parent;
        }
        path.reverse();
        path
    }

    pub(crate) fn branch(&self, focus: usize, index: usize) -> Option<usize> {
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Rect {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) w: f32,
    pub(crate) h: f32,
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

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Cell {
    pub(crate) node: usize,
    pub(crate) rect: Rect,
    pub(crate) group: bool,
}

pub(crate) fn treemap(
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

#[expect(
    clippy::suspicious_operation_groupings,
    reason = "the squarified aspect ratio: max(w²·max/s², s²/(w²·min))"
)]
pub(crate) fn squarify<T: Copy>(items: &[(T, f64)], rect: Rect) -> Vec<(T, Rect)> {
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

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Bar {
    pub(crate) node: usize,
    pub(crate) x0: f32,
    pub(crate) x1: f32,
    pub(crate) level: usize,
}

pub(crate) fn partition(tree: &Tree, focus: usize, levels: usize) -> Vec<Bar> {
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
