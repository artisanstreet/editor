//! One canvas paints hierarchy marks; only readable labels build text elements.
//! The plot owns pointer hit testing and the shared chart keyboard interaction.

use crate::theme::{ArtisanTheme, TextStep, TypographyTokens};
use std::sync::Arc;

use gpui::{
    AnyElement, App, Div, Entity, FontWeight, IntoElement, ParentElement as _, RenderOnce,
    SharedString, Styled as _, Window, div, prelude::*, px, relative,
};

use super::super::super::kit;
use super::super::frame::{self, ReadoutRow};
use super::super::scale::percent;
use super::super::scene::{Probe, fractions, probe, probed_width};
use super::{GROUP_HEADER, Layout, Tree, partition, treemap};

/// Height of a treemap plot.
const TREEMAP_HEIGHT: f32 = 340.0;

/// Pitch and height of an icicle or flame level.
const LEVEL_PITCH: f32 = 24.0;
const LEVEL_HEIGHT: f32 = 22.0;

/// Plot width assumed before the first paint.
const FALLBACK_WIDTH: f32 = 640.0;

/// A cell narrower or shorter than this hides its text.
const MIN_TEXT_WIDTH: f32 = 36.0;
const MIN_TEXT_HEIGHT: f32 = 18.0;

#[derive(IntoElement)]
pub(super) struct HierarchyView {
    pub(super) id: SharedString,
    pub(super) theme: ArtisanTheme,
    pub(super) tree: Arc<Tree>,
    pub(super) layout: Layout,
    pub(super) unit: Option<SharedString>,
    pub(super) levels: usize,
}

struct HierarchyState {
    model: Arc<Tree>,
    focus: usize,
    hover: Option<usize>,
    probe: Probe,
    geometry: Option<Arc<Geometry>>,
}

/// A hierarchy mark and its presentation-independent hit rectangle.
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
    parts: Arc<Vec<Part>>,
    nodes: Arc<Vec<usize>>,
}
struct Spot {
    node: usize,
    x: f32,
    high: bool,
}

impl RenderOnce for HierarchyView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = kit::keyed(window, cx, kit::part(&self.id, "state"), || {
            HierarchyState {
                model: self.tree.clone(),
                focus: 0,
                hover: None,
                probe: probe(),
                geometry: None,
            }
        });
        state.update(cx, |state, _| {
            if !Arc::ptr_eq(&state.model, &self.tree) {
                let changed = *state.model != *self.tree;
                state.model = self.tree.clone();
                if !changed {
                    return;
                }
                state.geometry = None;
                state.focus = 0;
                state.hover = None;
            }
        });
        let (focus, hover, plot_probe) = {
            let current = state.read(cx);
            (current.focus, current.hover, current.probe.clone())
        };
        let width = probed_width(&plot_probe, FALLBACK_WIDTH);
        let geometry = state.update(cx, |state, _| {
            if let Some(geometry) = &state.geometry
                && geometry.layout == self.layout
                && geometry.focus == focus
                && geometry.levels == self.levels
                && geometry.width == width
            {
                return geometry.clone();
            }
            let (height, parts) = match self.layout {
                Layout::Treemap => {
                    let cells = treemap(&self.tree, focus, self.levels, width, TREEMAP_HEIGHT);
                    let parts = cells
                        .into_iter()
                        .map(|cell| Part {
                            node: cell.node,
                            x: cell.rect.x / width,
                            y: cell.rect.y / TREEMAP_HEIGHT,
                            w: cell.rect.w / width,
                            h: cell.rect.h / TREEMAP_HEIGHT,
                            group: cell.group,
                            target: (!self.tree.node(cell.node).children.is_empty())
                                .then_some(cell.node),
                        })
                        .collect::<Vec<_>>();
                    (TREEMAP_HEIGHT, parts)
                }
                Layout::Icicle | Layout::Flame => {
                    let bars = partition(&self.tree, focus, self.levels);
                    let depth = bars.iter().map(|bar| bar.level).max().unwrap_or(0) + 1;
                    let height = depth as f32 * LEVEL_PITCH;
                    let parts = bars
                        .into_iter()
                        .map(|bar| {
                            let row = if self.layout == Layout::Flame {
                                depth - 1 - bar.level
                            } else {
                                bar.level
                            };
                            let node = self.tree.node(bar.node);
                            Part {
                                node: bar.node,
                                x: bar.x0,
                                y: row as f32 * LEVEL_PITCH / height,
                                w: bar.x1 - bar.x0,
                                h: LEVEL_HEIGHT / height,
                                group: false,
                                target: if bar.node == focus {
                                    node.parent
                                } else {
                                    (!node.children.is_empty()).then_some(bar.node)
                                },
                            }
                        })
                        .collect::<Vec<_>>();
                    (height, parts)
                }
            };
            let parts = Arc::new(parts);
            let mut seen = std::collections::HashSet::new();
            let nodes: Arc<Vec<usize>> = Arc::new(
                parts
                    .iter()
                    .map(|part| part.node)
                    .filter(|node| seen.insert(*node))
                    .collect(),
            );
            let geometry = Arc::new(Geometry {
                layout: self.layout,
                focus,
                levels: self.levels,
                width,
                height,
                parts,
                nodes,
            });
            state.geometry = Some(geometry.clone());
            geometry
        });
        let height = geometry.height;
        let parts = geometry.parts.clone();
        let readout = hover
            .and_then(|node| parts.iter().find(|part| part.node == node))
            .map(|part| {
                self.readout(
                    &Spot {
                        node: part.node,
                        x: part.x + part.w / 2.0,
                        high: part.y + part.h / 2.0 < 0.5,
                    },
                    focus,
                )
            });
        let readout = readout.map(|card| {
            card.id(kit::part(&self.id, "readout"))
                .on_mouse_move(|_, _, cx| cx.stop_propagation())
                .on_click(|_, _, cx| cx.stop_propagation())
        });
        let nodes = geometry.nodes.clone();
        let selected = hover.and_then(|node| nodes.iter().position(|entry| *entry == node));
        let selection_label = hover.map(|node| {
            format!(
                "{}: {}",
                self.tree.node(node).name,
                self.reading(self.tree.node(node).size)
            )
        });
        let count = nodes.len();
        let keyboard = state.clone();
        let tree = self.tree.clone();
        let labels = parts
            .iter()
            .filter_map(|part| self.label(part, width, height));
        let painted = parts.clone();
        let colors: Vec<_> = parts
            .iter()
            .map(|part| {
                (
                    self.fill(
                        focus,
                        part.node,
                        if part.group {
                            0.08
                        } else if hover == Some(part.node) {
                            0.72
                        } else {
                            0.5
                        },
                    ),
                    self.fill(focus, part.node, 0.35),
                    self.fill(focus, part.node, 0.22),
                )
            })
            .collect();
        let moving_parts = parts.clone();
        let clicked_parts = parts.clone();
        let moving_probe = plot_probe.clone();
        let clicked_probe = plot_probe.clone();
        let moving = state.clone();
        let leaving = state.clone();
        let clicked = state.clone();
        let mark_layout = self.layout;
        let mark_theme = self.theme;
        let plot = div()
            .id(kit::part(&self.id, "plot"))
            .relative()
            .size_full()
            .child(
                gpui::canvas(
                    move |bounds, _, _| plot_probe.set(Some(bounds)),
                    move |bounds, (), window, _| {
                        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                            for (part, (color, border, strip)) in painted.iter().zip(&colors) {
                                let pad = if mark_layout == Layout::Treemap {
                                    1.0
                                } else {
                                    0.5
                                };
                                let rect = gpui::Bounds::new(
                                    gpui::point(
                                        bounds.origin.x + bounds.size.width * part.x + px(pad),
                                        bounds.origin.y
                                            + bounds.size.height * part.y
                                            + px(if mark_layout == Layout::Treemap {
                                                pad
                                            } else {
                                                0.0
                                            }),
                                    ),
                                    gpui::size(
                                        (bounds.size.width * part.w - px(pad * 2.0)).max(px(0.0)),
                                        (bounds.size.height * part.h
                                            - px(if mark_layout == Layout::Treemap {
                                                pad * 2.0
                                            } else {
                                                0.0
                                            }))
                                        .max(px(0.0)),
                                    ),
                                );
                                if rect.size.width <= px(0.0) || rect.size.height <= px(0.0) {
                                    continue;
                                }
                                let selected = hover == Some(part.node);
                                let ring = if part.group {
                                    *border
                                } else {
                                    mark_theme
                                        .colors
                                        .foreground
                                        .with_alpha(if selected { 0.13 } else { 0.0 })
                                        .to_paint()
                                };
                                window.paint_quad(gpui::quad(
                                    rect,
                                    px(if part.group {
                                        6.0
                                    } else if mark_layout == Layout::Treemap {
                                        5.0
                                    } else {
                                        4.0
                                    }),
                                    *color,
                                    px(if part.group {
                                        1.0
                                    } else if selected {
                                        0.5
                                    } else {
                                        0.0
                                    }),
                                    ring,
                                    gpui::BorderStyle::Solid,
                                ));
                                if part.group && selected {
                                    let strip_bounds = gpui::Bounds::new(
                                        gpui::point(
                                            rect.origin.x + px(1.0),
                                            rect.origin.y + px(1.0),
                                        ),
                                        gpui::size(
                                            (rect.size.width - px(2.0)).max(px(0.0)),
                                            px(GROUP_HEADER - 1.0)
                                                .min((rect.size.height - px(2.0)).max(px(0.0))),
                                        ),
                                    );
                                    window.paint_quad(gpui::quad(
                                        strip_bounds,
                                        gpui::Corners {
                                            top_left: px(5.0),
                                            top_right: px(5.0),
                                            bottom_left: px(0.0),
                                            bottom_right: px(0.0),
                                        },
                                        *strip,
                                        px(0.0),
                                        *strip,
                                        gpui::BorderStyle::Solid,
                                    ));
                                }
                            }
                        });
                    },
                )
                .absolute()
                .size_full(),
            )
            .children(labels)
            .children(readout)
            .on_mouse_move(move |event: &gpui::MouseMoveEvent, _, cx| {
                let next = fractions(&moving_probe, event.position).and_then(|(x, y)| {
                    moving_parts
                        .iter()
                        .rev()
                        .find(|part| part.contains(x, y, height))
                        .map(|part| part.node)
                });
                moving.update(cx, |state, cx| {
                    if state.hover != next {
                        state.hover = next;
                        cx.notify();
                    }
                });
            })
            .on_hover(move |entered, _, cx| {
                if !*entered {
                    leaving.update(cx, |state, cx| {
                        if state.hover.take().is_some() {
                            cx.notify();
                        }
                    });
                }
            })
            .on_click(move |event, _, cx| {
                if let Some(target) = fractions(&clicked_probe, event.position())
                    .and_then(|(x, y)| {
                        clicked_parts
                            .iter()
                            .rev()
                            .find(|part| part.contains(x, y, height))
                    })
                    .and_then(|part| part.target)
                {
                    zoom(&clicked, target, cx);
                }
            });
        let mut navigation = crate::chart_navigation::ChartNavigation::new(
            kit::part(&self.id, "navigation"), self.theme,
            "Hierarchy. Arrows explore nodes; Enter zooms; Backspace returns to parent; Escape returns to root.",
            0..count, plot,
            move |command, _, cx| {
                use crate::chart_navigation::ChartCommand;
                match command {
                    ChartCommand::Select(index) => if let Some(node) = nodes.get(index) { keyboard.update(cx, |state, cx| { state.hover = Some(*node); cx.notify(); }); },
                    ChartCommand::Activate(index) => if let Some(node) = nodes.get(index) {
                        let target = if *node == focus { tree.node(focus).parent.unwrap_or(0) } else { *node };
                        zoom(&keyboard, target, cx);
                    },
                    ChartCommand::Parent => zoom(&keyboard, tree.node(focus).parent.unwrap_or(0), cx),
                    ChartCommand::Reset => zoom(&keyboard, 0, cx),
                }
            },
        ).selected(selected);
        if let Some(label) = selection_label {
            navigation = navigation.selection_label(label);
        }
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(self.trail(&state, focus))
            .child(div().w_full().h(px(height)).child(navigation))
    }
}

impl HierarchyView {
    fn reading(&self, size: f64) -> String {
        kit::with_unit(&kit::format_number(size), self.unit.as_deref())
    }

    /// The fill of `node` under `focus`, stronger when hovered.
    fn fill(&self, focus: usize, node: usize, alpha: f32) -> gpui::Hsla {
        match self.tree.branch(focus, node) {
            Some(branch) => kit::series_color(branch).with_alpha(alpha).to_paint(),
            None => self
                .theme
                .colors
                .foreground
                .with_alpha(alpha * 0.2)
                .to_paint(),
        }
    }

    /// The path from the root to the focus; every step but the last zooms
    /// back out to it.
    fn trail(&self, state: &Entity<HierarchyState>, focus: usize) -> Div {
        let theme = self.theme;
        let path = self.tree.path(focus);
        let last = path.len() - 1;
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(2.0))
            .text_size(TypographyTokens::size(TextStep::Xs))
            .line_height(px(16.0))
            .children(path.into_iter().enumerate().flat_map(|(step, node)| {
                let current = step == last;
                let name = self.tree.node(node).name.clone();
                let state = state.clone();
                let entry = crate::button::Button::text(
                    kit::part(&self.id, format_args!("trail-{node}")),
                    theme,
                    name,
                )
                .variant(crate::button::ButtonVariant::Ghost)
                .disabled(current)
                .on_activate(move |_, _, cx| zoom(&state, node, cx))
                .into_any_element();
                let separator = (step > 0).then(|| {
                    div()
                        .text_color(theme.colors.muted_foreground.with_alpha(0.6).to_paint())
                        .child("/")
                        .into_any_element()
                });
                separator.into_iter().chain([entry.into_any_element()])
            }))
    }

    /// Readable labels are the only per-node elements. Mark chrome lives in canvas.
    fn label(&self, part: &Part, width: f32, height: f32) -> Option<AnyElement> {
        let w = part.w * width;
        let h = part.h * height;
        if w < MIN_TEXT_WIDTH || h < MIN_TEXT_HEIGHT {
            return None;
        }
        let node = self.tree.node(part.node);
        let value = self.reading(node.size);
        let label = div()
            .id(kit::part(&self.id, format_args!("node-{}", part.node)))
            .absolute()
            .left(relative(part.x))
            .top(relative(part.y))
            .w(relative(part.w))
            .h(if part.group { px(GROUP_HEADER) } else { px(h) })
            .px(px(6.0))
            .py(px(if part.group || self.layout != Layout::Treemap {
                0.0
            } else {
                3.0
            }))
            .overflow_hidden()
            .text_size(TypographyTokens::size(TextStep::X2s))
            .line_height(px(14.0))
            .text_color(self.theme.colors.foreground.to_paint())
            .flex()
            .when(!part.group && self.layout == Layout::Treemap, |label| {
                label.flex_col()
            })
            .when(part.group || self.layout != Layout::Treemap, |label| {
                label.items_center().gap(px(6.0))
            })
            .child(
                div()
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .font_weight(FontWeight::MEDIUM)
                    .child(node.name.clone()),
            )
            .when(
                self.layout == Layout::Treemap && (part.group || h >= 36.0),
                |label| {
                    label.child(
                        div()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_color(self.theme.colors.muted_foreground.to_paint())
                            .child(value),
                    )
                },
            );
        Some(label.into_any_element())
    }

    fn readout(&self, spot: &Spot, focus: usize) -> Div {
        let node = self.tree.node(spot.node);
        let focused = self.tree.node(focus);
        let mut rows = vec![ReadoutRow::new(
            Some(self.fill(focus, spot.node, 1.0)),
            "Size",
            self.reading(node.size),
        )];
        if spot.node != focus && focused.size > 0.0 {
            rows.push(ReadoutRow::new(
                None,
                format!("Share of {}", focused.name),
                percent(node.size / focused.size),
            ));
        }
        if let Some(parent) = node
            .parent
            .filter(|parent| *parent != focus && spot.node != focus)
        {
            let parent = self.tree.node(parent);
            if parent.size > 0.0 {
                rows.push(ReadoutRow::new(
                    None,
                    format!("Share of {}", parent.name),
                    percent(node.size / parent.size),
                ));
            }
        }
        let own = node.size
            - node
                .children
                .iter()
                .map(|child| self.tree.node(*child).size)
                .sum::<f64>();
        if !node.children.is_empty() && own > node.size * 1e-9 {
            rows.push(ReadoutRow::new(None, "Own", self.reading(own)));
        }
        let heading = Some(node.name.clone());
        if spot.high {
            frame::readout_low(&self.theme, spot.x, heading, rows)
        } else {
            frame::readout(&self.theme, spot.x, heading, rows)
        }
    }
}

/// Wires hover tracking, and zooming when `zoomable`, onto a part.
fn zoom(state: &Entity<HierarchyState>, node: usize, cx: &mut App) {
    state.update(cx, |state, cx| {
        if state.focus != node {
            state.focus = node;
            state.hover = None;
            cx.notify();
        }
    });
}

#[cfg(test)]
mod native_tests;

#[cfg(test)]
mod batching_tests {
    use super::*;
    #[test]
    fn dense_rectangles_have_no_text_elements_but_remain_exact_hit_targets() {
        let tree = Tree::from_spec(&super::super::NodeSpec {
            name: "root".into(),
            value: None,
            children: (0..4095)
                .map(|index| super::super::NodeSpec {
                    name: format!("leaf {index}"),
                    value: Some(1.0),
                    children: Vec::new(),
                })
                .collect(),
        });
        let view = HierarchyView {
            id: "dense".into(),
            theme: ArtisanTheme::resolved(),
            tree: Arc::new(tree),
            layout: Layout::Treemap,
            unit: None,
            levels: 3,
        };
        let cells = treemap(&view.tree, 0, 3, 640.0, TREEMAP_HEIGHT);
        let labels = cells
            .iter()
            .filter(|cell| {
                let part = Part {
                    node: cell.node,
                    x: cell.rect.x / 640.0,
                    y: cell.rect.y / TREEMAP_HEIGHT,
                    w: cell.rect.w / 640.0,
                    h: cell.rect.h / TREEMAP_HEIGHT,
                    group: cell.group,
                    target: None,
                };
                assert!(part.contains(
                    part.x + part.w / 2.0,
                    part.y
                        + (if part.group {
                            GROUP_HEADER / TREEMAP_HEIGHT
                        } else {
                            part.h
                        }) / 2.0,
                    TREEMAP_HEIGHT
                ));
                view.label(&part, 640.0, TREEMAP_HEIGHT).is_some()
            })
            .count();
        assert!(cells.len() > 4000);
        assert!(
            labels < 32,
            "dense leaves must not construct thousands of text elements: {labels}"
        );
    }
}

#[cfg(test)]
mod geometry_cache_tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext};
    struct Fixture {
        tree: Arc<Tree>,
        geometry: Option<Arc<Geometry>>,
    }
    impl Render for Fixture {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let plot = HierarchyView {
                id: "cached-hierarchy".into(),
                theme: ArtisanTheme::resolved(),
                tree: self.tree.clone(),
                layout: Layout::Treemap,
                unit: None,
                levels: 3,
            }
            .render(window, cx)
            .into_any_element();
            // Inspect the actual keyed renderer state during its valid render phase.
            let state = kit::keyed::<HierarchyState>(
                window,
                cx,
                kit::part(&"cached-hierarchy".into(), "state"),
                || panic!("rendered state must exist"),
            );
            self.geometry = state.read(cx).geometry.clone();
            div().w(px(640.0)).child(plot)
        }
    }
    #[gpui::test]
    fn changing_hover_reuses_prepared_geometry(cx: &mut TestAppContext) {
        let (view,cx)=cx.add_window_view(|_,_|Fixture {
            tree:Arc::new(Tree::from_spec(&serde_json::from_str::<super::super::NodeSpec>(r#"{"name":"root","children":[{"name":"one","value":4},{"name":"two","value":3}]}"#).unwrap())),geometry:None,
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        let before = cx.update(|_, cx| view.read(cx).geometry.clone().unwrap());
        cx.simulate_keystrokes("tab right");
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        let after = cx.update(|_, cx| view.read(cx).geometry.clone().unwrap());
        assert!(
            Arc::ptr_eq(&before, &after),
            "selection alone must not rebuild hierarchy geometry"
        );
    }
}
