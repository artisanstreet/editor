use super::{GROUP_HEADER, Layout, Tree, partition, treemap};
use crate::ui::kit::Space;
use crate::ui::rich::charts::scale::{f32_of_count, percent};
use crate::ui::rich::charts::scene::{self, Probe};
use crate::ui::rich::kit::{format_number, with_unit};
use crate::ui::theme::{ArtisanTheme, Opacity};
use aqua_core::{
    CornerRadii, DisplayItem, GpuiBackground, GpuiHsla, InputEvent, Insets, Key, Modifiers,
    NamedKey, Point, Rect, Role,
};
use aqua_ui::element::{Element, ui};
use aqua_ui::input::{EventPhase, InputKind, InputResponse, RoutedInput};
use aqua_ui::style::{Length, Style};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct PartLabel {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub plot_width: f32,
    pub plot_height: f32,
    pub stacked: bool,
    pub name: String,
    pub value: Option<String>,
}
pub(crate) type LabelFactory = Rc<dyn Fn(PartLabel) -> Element>;
pub(crate) type ReadoutFactory =
    Rc<dyn Fn(bool, f32, &str, Vec<(Option<GpuiHsla>, String, String)>) -> Element>;
pub(crate) type TrailFactory = Rc<dyn Fn(Vec<(usize, String, bool)>, Rc<dyn Fn(usize)>) -> Element>;
pub(crate) type ClickFactory = Rc<dyn Fn(Element, Rc<dyn Fn(Point)>) -> Element>;
pub(crate) type NavigationFrame = Rc<dyn Fn(Element, GpuiHsla) -> Element>;
pub(crate) struct Services {
    pub label: LabelFactory,
    pub readout: ReadoutFactory,
    pub trail: TrailFactory,
    pub click: ClickFactory,
    pub navigation: NavigationFrame,
}

pub(crate) fn levels(layout: Layout, depth: Option<usize>) -> usize {
    depth
        .filter(|depth| *depth > 0)
        .unwrap_or(match layout {
            Layout::Treemap => 2,
            Layout::Icicle | Layout::Flame => 6,
        })
        .min(12)
}

#[derive(Clone, Debug)]
struct Part {
    node: usize,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    group: bool,
    target: Option<usize>,
}
impl Part {
    fn contains(&self, x: f32, y: f32, height: f32) -> bool {
        let h = if self.group {
            self.h.min(GROUP_HEADER / height)
        } else {
            self.h
        };
        (self.x..=self.x + self.w).contains(&x) && (self.y..=self.y + h).contains(&y)
    }
}
struct Geometry {
    layout: Layout,
    focus: usize,
    levels: usize,
    width: f32,
    height: f32,
    parts: Arc<[Part]>,
    nodes: Arc<[usize]>,
}
struct Entry {
    tree: Arc<Tree>,
    focus: usize,
    hover: Option<usize>,
    probe: Probe,
    geometry: Option<Arc<Geometry>>,
}
#[derive(Clone, Default)]
pub(crate) struct HierarchyState(Rc<RefCell<HashMap<String, Entry>>>);
impl HierarchyState {
    fn sync(&self, id: &str, tree: &Arc<Tree>) -> (usize, Option<usize>, Probe) {
        let mut states = self.0.borrow_mut();
        let entry = states.entry(id.to_owned()).or_insert_with(|| Entry {
            tree: tree.clone(),
            focus: 0,
            hover: None,
            probe: Rc::new(Cell::new(None)),
            geometry: None,
        });
        if !Arc::ptr_eq(&entry.tree, tree) {
            if *entry.tree != **tree {
                entry.geometry = None;
                entry.focus = 0;
                entry.hover = None;
            }
            entry.tree = tree.clone();
        }
        (entry.focus, entry.hover, entry.probe.clone())
    }
    fn geometry(
        &self,
        id: &str,
        tree: &Tree,
        layout: Layout,
        levels: usize,
        width: f32,
    ) -> Arc<Geometry> {
        let mut states = self.0.borrow_mut();
        let entry = states.get_mut(id).unwrap();
        if let Some(geometry) = &entry.geometry
            && geometry.layout == layout
            && geometry.focus == entry.focus
            && geometry.levels == levels
            && geometry.width == width
        {
            return geometry.clone();
        }
        let focus = entry.focus;
        let (height, parts) = match layout {
            Layout::Treemap => (
                340.0,
                treemap(tree, focus, levels, width, 340.0)
                    .into_iter()
                    .map(|cell| Part {
                        node: cell.node,
                        x: cell.rect.x / width,
                        y: cell.rect.y / 340.0,
                        w: cell.rect.w / width,
                        h: cell.rect.h / 340.0,
                        group: cell.group,
                        target: (!tree.node(cell.node).children.is_empty()).then_some(cell.node),
                    })
                    .collect::<Vec<_>>(),
            ),
            Layout::Icicle | Layout::Flame => {
                let bars = partition(tree, focus, levels);
                let depth = bars.iter().map(|bar| bar.level).max().unwrap_or(0) + 1;
                let height = f32_of_count(depth) * 24.0;
                (
                    height,
                    bars.into_iter()
                        .map(|bar| {
                            let row = if layout == Layout::Flame {
                                depth - 1 - bar.level
                            } else {
                                bar.level
                            };
                            let node = tree.node(bar.node);
                            Part {
                                node: bar.node,
                                x: bar.x0,
                                y: f32_of_count(row) * 24.0 / height,
                                w: bar.x1 - bar.x0,
                                h: 22.0 / height,
                                group: false,
                                target: if bar.node == focus {
                                    node.parent
                                } else {
                                    (!node.children.is_empty()).then_some(bar.node)
                                },
                            }
                        })
                        .collect(),
                )
            }
        };
        let parts: Arc<[Part]> = parts.into();
        let mut seen = HashSet::new();
        let nodes = parts
            .iter()
            .map(|part| part.node)
            .filter(|node| seen.insert(*node))
            .collect();
        let geometry = Arc::new(Geometry {
            layout,
            focus,
            levels,
            width,
            height,
            parts,
            nodes,
        });
        entry.geometry = Some(geometry.clone());
        geometry
    }
    fn hover(&self, id: &str, value: Option<usize>) -> bool {
        let mut states = self.0.borrow_mut();
        let Some(entry) = states.get_mut(id) else {
            return false;
        };
        let changed = entry.hover != value;
        entry.hover = value;
        changed
    }
    fn zoom(&self, id: &str, node: usize) -> bool {
        let mut states = self.0.borrow_mut();
        let Some(entry) = states.get_mut(id) else {
            return false;
        };
        if entry.focus == node {
            return false;
        }
        entry.focus = node;
        entry.hover = None;
        true
    }
    fn key(&self, id: &str, nodes: &[usize], code: NamedKey, alt: bool) -> bool {
        let mut states = self.0.borrow_mut();
        let Some(entry) = states.get_mut(id) else {
            return false;
        };
        let current = entry
            .hover
            .and_then(|node| nodes.iter().position(|candidate| *candidate == node));
        let target = match code {
            NamedKey::Escape => Some(0),
            NamedKey::Backspace => Some(entry.tree.node(entry.focus).parent.unwrap_or(0)),
            NamedKey::Left if alt => Some(entry.tree.node(entry.focus).parent.unwrap_or(0)),
            NamedKey::Enter if !nodes.is_empty() => {
                let node = nodes[current.unwrap_or(0)];
                Some(if node == entry.focus {
                    entry.tree.node(entry.focus).parent.unwrap_or(0)
                } else {
                    node
                })
            }
            NamedKey::Home if !nodes.is_empty() => {
                entry.hover = Some(nodes[0]);
                return true;
            }
            NamedKey::End if !nodes.is_empty() => {
                entry.hover = nodes.last().copied();
                return true;
            }
            NamedKey::Left | NamedKey::Up if !nodes.is_empty() => {
                entry.hover = Some(nodes[current.map_or(0, |n| n.saturating_sub(1))]);
                return true;
            }
            NamedKey::Right | NamedKey::Down if !nodes.is_empty() => {
                entry.hover = Some(nodes[current.map_or(0, |n| (n + 1).min(nodes.len() - 1))]);
                return true;
            }
            _ => return false,
        };
        if let Some(node) = target
            && entry.focus != node
        {
            entry.focus = node;
            entry.hover = None;
        }
        true
    }
    pub(crate) fn remove(&self, id: &str) {
        self.0.borrow_mut().remove(id);
    }
}

#[derive(Clone, Copy)]
enum Mark {
    Leaf,
    LeafHover,
    Group,
    Edge,
    Header,
}
fn color(theme: &ArtisanTheme, tree: &Tree, focus: usize, node: usize, mark: Mark) -> GpuiHsla {
    let (branch, focus_opacity) = match mark {
        Mark::Leaf => (Opacity::TreemapLeaf, Opacity::TreemapFocusLeaf),
        Mark::LeafHover => (Opacity::TreemapLeafHover, Opacity::TreemapFocusLeafHover),
        Mark::Group => (Opacity::TreemapGroup, Opacity::TreemapFocusGroup),
        Mark::Edge => (Opacity::TreemapEdge, Opacity::TreemapFocusEdge),
        Mark::Header => (Opacity::TreemapHeader, Opacity::TreemapFocusHeader),
    };
    tree.branch(focus, node)
        .map_or_else(
            || theme.colors.text.at(focus_opacity),
            |index| theme.colors.palette[index % 8].at(branch),
        )
        .to_reference_paint()
}
fn paint(
    bounds: Rect,
    theme: &ArtisanTheme,
    tree: &Tree,
    geometry: &Geometry,
    hover: Option<usize>,
) -> Vec<DisplayItem> {
    let mut items = vec![DisplayItem::PushClip {
        bounds,
        radius: 0.0,
    }];
    for part in geometry.parts.iter() {
        let pad = if geometry.layout == Layout::Treemap {
            1.0
        } else {
            0.5
        };
        let vertical = if geometry.layout == Layout::Treemap {
            pad
        } else {
            0.0
        };
        let rect = Rect {
            x: bounds.x + bounds.w * part.x + pad,
            y: bounds.y + bounds.h * part.y + vertical,
            w: (bounds.w * part.w - pad * 2.0).max(0.0),
            h: (bounds.h * part.h - vertical * 2.0).max(0.0),
        };
        if rect.w <= 0.0 || rect.h <= 0.0 {
            continue;
        }
        let selected = hover == Some(part.node);
        let mark = if part.group {
            Mark::Group
        } else if selected {
            Mark::LeafHover
        } else {
            Mark::Leaf
        };
        let fill = color(theme, tree, geometry.focus, part.node, mark);
        let radius = if part.group {
            6.0
        } else if geometry.layout == Layout::Treemap {
            5.0
        } else {
            4.0
        };
        let ring = if part.group {
            color(theme, tree, geometry.focus, part.node, Mark::Edge)
        } else {
            theme
                .colors
                .text
                .at(if selected {
                    Opacity::HoverRing
                } else {
                    Opacity::Clear
                })
                .to_reference_paint()
        };
        let border = if part.group {
            1.0
        } else if selected {
            0.5
        } else {
            0.0
        };
        items.push(DisplayItem::GpuiBackground {
            bounds: rect,
            corners: CornerRadii::uniform(radius),
            background: GpuiBackground::Solid(fill),
        });
        if border > 0.0 {
            items.push(DisplayItem::GpuiBorder {
                bounds: rect,
                corners: CornerRadii::uniform(radius),
                widths: Insets::uniform(border),
                color: ring,
                dashed: false,
            });
        }
        if part.group && selected {
            let strip = Rect {
                x: rect.x + 1.0,
                y: rect.y + 1.0,
                w: (rect.w - 2.0).max(0.0),
                h: (GROUP_HEADER - 1.0).min((rect.h - 2.0).max(0.0)),
            };
            let fill = color(theme, tree, geometry.focus, part.node, Mark::Header);
            items.push(DisplayItem::GpuiBackground {
                bounds: strip,
                corners: CornerRadii {
                    top_left: 5.0,
                    top_right: 5.0,
                    bottom_left: 0.0,
                    bottom_right: 0.0,
                },
                background: GpuiBackground::Solid(fill),
            });
        }
    }
    items.push(DisplayItem::PopClip);
    items
}
fn reading(size: f64, unit: Option<&str>) -> String {
    with_unit(&format_number(size), unit)
}

pub(crate) fn view(
    id: String,
    theme: &ArtisanTheme,
    tree: Arc<Tree>,
    layout: Layout,
    unit: Option<String>,
    levels: usize,
    state: HierarchyState,
    services: &Services,
    invalidate: Rc<dyn Fn()>,
) -> Element {
    let theme = *theme;
    let (focus, hover, probe) = state.sync(&id, &tree);
    let width = probe.get().map_or(640.0, |bounds| bounds.w);
    let geometry = state.geometry(&id, &tree, layout, levels, width);
    let height = geometry.height;
    let draw_geometry = geometry.clone();
    let draw_tree = tree.clone();
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
            paint(bounds, &theme, &draw_tree, &draw_geometry, hover)
        })
        .build();
    let mut plot = ui::Box::new()
        .style(
            Style::new()
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0)),
        )
        .child(canvas);
    for part in geometry.parts.iter() {
        let w = part.w * width;
        let h = part.h * height;
        if w < 36.0 || h < 18.0 {
            continue;
        }
        let node = tree.node(part.node);
        let value = (layout == Layout::Treemap && (part.group || h >= 36.0))
            .then(|| reading(node.size, unit.as_deref()));
        plot = plot.child((services.label)(PartLabel {
            x: part.x,
            y: part.y,
            width: part.w,
            height: if part.group { GROUP_HEADER } else { h },
            plot_width: width,
            plot_height: height,
            stacked: !part.group && layout == Layout::Treemap,
            name: node.name.clone(),
            value,
        }));
    }
    let selection = hover.map(|node| {
        format!(
            "{}: {}",
            tree.node(node).name,
            reading(tree.node(node).size, unit.as_deref())
        )
    });
    if let Some((node, part)) = hover.and_then(|node| {
        geometry
            .parts
            .iter()
            .find(|part| part.node == node)
            .map(|part| (node, part))
    }) {
        let current = tree.node(node);
        let focused = tree.node(focus);
        let swatch = tree
            .branch(focus, node)
            .map_or_else(
                || theme.colors.text.at(Opacity::TreemapFocusSwatch),
                |branch| theme.colors.palette[branch % 8],
            )
            .to_reference_paint();
        let mut rows = vec![(
            Some(swatch),
            "Size".to_owned(),
            reading(current.size, unit.as_deref()),
        )];
        if node != focus && focused.size > 0.0 {
            rows.push((
                None,
                format!("Share of {}", focused.name),
                percent(current.size / focused.size),
            ));
        }
        if let Some(parent) = current
            .parent
            .filter(|parent| *parent != focus && node != focus)
        {
            let parent = tree.node(parent);
            if parent.size > 0.0 {
                rows.push((
                    None,
                    format!("Share of {}", parent.name),
                    percent(current.size / parent.size),
                ));
            }
        }
        let own = current.size
            - current
                .children
                .iter()
                .map(|child| tree.node(*child).size)
                .sum::<f64>();
        if !current.children.is_empty() && own > current.size * 1e-9 {
            rows.push((None, "Own".to_owned(), reading(own, unit.as_deref())));
        }
        plot = plot.child((services.readout)(
            part.y + part.h / 2.0 < 0.5,
            part.x + part.w / 2.0,
            &current.name,
            rows,
        ));
    }
    let moving = state.clone();
    let moving_id = id.clone();
    let moving_geometry = geometry.clone();
    let moving_probe = probe.clone();
    let moving_invalidate = invalidate.clone();
    let leaving = state.clone();
    let leaving_id = id.clone();
    let leaving_invalidate = invalidate.clone();
    let plot = plot
        .build()
        .with_key(format!("{id}-plot"))
        .on_input(InputKind::PointerMove, EventPhase::Bubble, move |event| {
            let RoutedInput::Native(InputEvent::PointerMove(pointer)) = &event.input else {
                return InputResponse::default();
            };
            let selected = scene::fractions(&moving_probe, pointer.position).and_then(|(x, y)| {
                moving_geometry
                    .parts
                    .iter()
                    .rev()
                    .find(|part| part.contains(x, y, height))
                    .map(|part| part.node)
            });
            if moving.hover(&moving_id, selected) {
                moving_invalidate();
            }
            InputResponse::default()
        })
        .on_hover_change(move |over| {
            if !over && leaving.hover(&leaving_id, None) {
                leaving_invalidate();
            }
        });
    let clicked = state.clone();
    let clicked_id = id.clone();
    let clicked_geometry = geometry.clone();
    let clicked_probe = probe.clone();
    let clicked_invalidate = invalidate.clone();
    let plot = (services.click)(
        plot,
        Rc::new(move |position| {
            let target = scene::fractions(&clicked_probe, position)
                .and_then(|(x, y)| {
                    clicked_geometry
                        .parts
                        .iter()
                        .rev()
                        .find(|part| part.contains(x, y, height))
                })
                .and_then(|part| part.target);
            if let Some(target) = target
                && clicked.zoom(&clicked_id, target)
            {
                clicked_invalidate();
            }
        }),
    );
    let mut aria="Hierarchy. Arrows explore nodes; Enter zooms; Backspace returns to parent; Escape returns to root.".to_owned();
    if let Some(value) = &selection {
        aria.push(' ');
        aria.push_str(value);
    }
    let keyboard = state.clone();
    let keyboard_id = id.clone();
    let keyboard_nodes = geometry.nodes.clone();
    let keyboard_invalidate = invalidate.clone();
    let mut nav = ui::Box::new()
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .child(plot)
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
                &keyboard_nodes,
                code,
                key.modifiers.contains(Modifiers::ALT),
            ) {
                return InputResponse::default();
            }
            keyboard_invalidate();
            InputResponse {
                stop_propagation: true,
                ..InputResponse::default()
            }
        });
    if let Some(value) = selection {
        nav = nav.aria_value(value);
    }
    let nav = (services.navigation)(nav, theme.colors.ring.to_reference_paint());
    let path = tree.path(focus);
    let last = path.len() - 1;
    let entries = path
        .into_iter()
        .enumerate()
        .map(|(i, node)| (node, tree.node(node).name.clone(), i == last))
        .collect();
    let trail_state = state.clone();
    let trail_id = id.clone();
    let trail_invalidate = invalidate.clone();
    let trail = (services.trail)(
        entries,
        Rc::new(move |node| {
            if trail_state.zoom(&trail_id, node) {
                trail_invalidate();
            }
        }),
    );
    ui::Box::new()
        .layout("column")
        .style(Style::new().width(Length::Pct(100.0)).gap(Space::S8.px()))
        .child(trail)
        .child(
            ui::Box::new()
                .style(
                    Style::new()
                        .width(Length::Pct(100.0))
                        .height(Length::Px(height)),
                )
                .child(nav)
                .build(),
        )
        .build()
}

#[cfg(feature = "test-support")]
pub struct HierarchyFixture {
    tree: Arc<Tree>,
    layout: Layout,
    levels: usize,
    state: HierarchyState,
}
#[cfg(feature = "test-support")]
impl HierarchyFixture {
    pub fn new(json: &str) -> Self {
        let value = serde_json::from_str(json).expect("JSON");
        let prepared = super::prepare(&value).expect("hierarchy");
        let levels = levels(prepared.layout, prepared.depth);
        let tree = prepared.tree;
        let state = HierarchyState::default();
        state.sync("test", &tree);
        Self {
            tree,
            layout: prepared.layout,
            levels,
            state,
        }
    }
    pub fn replace(&mut self, json: &str) {
        let value = serde_json::from_str(json).expect("JSON");
        let prepared = super::prepare(&value).expect("hierarchy");
        self.levels = levels(prepared.layout, prepared.depth);
        self.layout = prepared.layout;
        self.tree = prepared.tree;
        self.state.sync("test", &self.tree);
    }
    pub fn focus(&self) -> usize {
        self.state.sync("test", &self.tree).0
    }
    pub fn selected(&self) -> Option<usize> {
        self.state.sync("test", &self.tree).1
    }
    pub fn key(&self, key: NamedKey, alt: bool) -> bool {
        let geometry = self
            .state
            .geometry("test", &self.tree, self.layout, self.levels, 640.0);
        self.state.key("test", &geometry.nodes, key, alt)
    }
    pub fn parts(&self) -> Vec<(usize, f32, f32, f32, f32, bool)> {
        let geometry = self
            .state
            .geometry("test", &self.tree, self.layout, self.levels, 640.0);
        geometry
            .parts
            .iter()
            .map(|part| (part.node, part.x, part.y, part.w, part.h, part.group))
            .collect()
    }
    pub fn click(&self, x: f32, y: f32) -> bool {
        let geometry = self
            .state
            .geometry("test", &self.tree, self.layout, self.levels, 640.0);
        let target = geometry
            .parts
            .iter()
            .rev()
            .find(|part| part.contains(x, y, geometry.height))
            .and_then(|part| part.target);
        target.is_some_and(|target| self.state.zoom("test", target))
    }
    pub fn resolved_levels(&self) -> usize {
        self.levels
    }
}
