//! Drawing a [`Tree`]: every cell or bar is a div at plot fractions, so it
//! hovers and clicks on its own; a glass readout names the hovered part.

use std::rc::Rc;

use gpui::{
    AnyElement, App, Div, Entity, FontWeight, IntoElement, ParentElement as _, RenderOnce,
    SharedString, Styled as _, Window, div, prelude::*, px, relative,
};

use super::super::super::kit;
use super::super::frame::{self, ReadoutRow};
use super::super::scale::percent;
use super::super::scene::{Probe, plot_canvas, probe, probed_width};
use super::{Bar, Cell, GROUP_HEADER, Layout, Tree, partition, treemap};
use crate::hover_card::{hover_fill_style, hover_highlight_shadow};
use crate::theme::ArtisanTheme;

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
    pub(super) tree: Rc<Tree>,
    pub(super) layout: Layout,
    pub(super) unit: Option<SharedString>,
    pub(super) levels: usize,
}

struct HierarchyState {
    focus: usize,
    hover: Option<usize>,
    probe: Probe,
}

/// Where a hovered part sits, for its readout.
struct Spot {
    node: usize,
    /// Center across, in plot fractions.
    x: f32,
    /// Whether it sits in the plot's top half.
    high: bool,
}

impl RenderOnce for HierarchyView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = kit::keyed(window, cx, kit::part(&self.id, "state"), || {
            HierarchyState {
                focus: 0,
                hover: None,
                probe: probe(),
            }
        });
        let (focus, hover, plot_probe) = {
            let current = state.read(cx);
            (current.focus, current.hover, current.probe.clone())
        };
        let width = probed_width(&plot_probe, FALLBACK_WIDTH);
        let (height, parts, spots) = match self.layout {
            Layout::Treemap => {
                let cells = treemap(&self.tree, focus, self.levels, width, TREEMAP_HEIGHT);
                let spots = cells
                    .iter()
                    .map(|cell| Spot {
                        node: cell.node,
                        x: (cell.rect.x + cell.rect.w / 2.0) / width,
                        high: cell.rect.y + cell.rect.h / 2.0 < TREEMAP_HEIGHT / 2.0,
                    })
                    .collect::<Vec<_>>();
                let parts = cells
                    .iter()
                    .map(|cell| self.cell(&state, cell, focus, hover, width))
                    .collect::<Vec<_>>();
                (TREEMAP_HEIGHT, parts, spots)
            }
            Layout::Icicle | Layout::Flame => {
                let bars = partition(&self.tree, focus, self.levels);
                let depth = bars.iter().map(|bar| bar.level).max().unwrap_or(0) + 1;
                #[expect(clippy::cast_precision_loss, reason = "levels are few")]
                let height = depth as f32 * LEVEL_PITCH;
                let spots = bars
                    .iter()
                    .map(|bar| Spot {
                        node: bar.node,
                        x: f32::midpoint(bar.x0, bar.x1),
                        high: (self.layout == Layout::Icicle) == (bar.level * 2 < depth),
                    })
                    .collect::<Vec<_>>();
                let parts = bars
                    .iter()
                    .map(|bar| self.bar(&state, bar, depth, focus, hover, width))
                    .collect::<Vec<_>>();
                (height, parts, spots)
            }
        };
        let readout = hover
            .and_then(|node| spots.iter().find(|spot| spot.node == node))
            .map(|spot| self.readout(spot, focus));

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(self.trail(&state, focus))
            .child(
                div()
                    .id(kit::part(&self.id, "plot"))
                    .relative()
                    .w_full()
                    .h(px(height))
                    .child(plot_canvas(Vec::new(), plot_probe))
                    .children(parts)
                    .children(readout),
            )
    }
}

impl HierarchyView {
    fn reading(&self, size: f64) -> String {
        kit::with_unit(&kit::format_number(size), self.unit.as_deref())
    }

    /// The fill of `node` under `focus`, stronger when hovered.
    fn fill(&self, focus: usize, node: usize, alpha: f32) -> gpui::Hsla {
        match self.tree.branch(focus, node) {
            Some(branch) => kit::series_color(&self.theme, branch)
                .with_alpha(alpha)
                .to_paint(),
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
            .text_size(px(12.0))
            .line_height(px(16.0))
            .children(path.into_iter().enumerate().flat_map(|(step, node)| {
                let current = step == last;
                let name = self.tree.node(node).name.clone();
                let state = state.clone();
                let entry = div()
                    .id(kit::part(&self.id, format_args!("trail-{node}")))
                    .px(px(6.0))
                    .h(px(22.0))
                    .flex()
                    .items_center()
                    .rounded(px(6.0))
                    .text_color(if current {
                        theme.colors.foreground.to_paint()
                    } else {
                        theme.colors.muted_foreground.to_paint()
                    })
                    .when(current, |entry| entry.font_weight(FontWeight::SEMIBOLD))
                    .when(!current, |entry| {
                        entry
                            .cursor_pointer()
                            .hover(move |style| hover_fill_style(style, &theme, px(6.0)))
                            .on_click(move |_, _, cx| zoom(&state, node, cx))
                    })
                    .child(name);
                let separator = (step > 0).then(|| {
                    div()
                        .text_color(theme.colors.muted_foreground.with_alpha(0.6).to_paint())
                        .child("/")
                        .into_any_element()
                });
                separator.into_iter().chain([entry.into_any_element()])
            }))
    }

    /// One treemap cell: a leaf, or a group's frame and name strip.
    fn cell(
        &self,
        state: &Entity<HierarchyState>,
        cell: &Cell,
        focus: usize,
        hover: Option<usize>,
        width: f32,
    ) -> AnyElement {
        let theme = self.theme;
        let node = self.tree.node(cell.node);
        let hovered = hover == Some(cell.node);
        let zoomable = !node.children.is_empty();
        let rect = cell.rect;
        let placed = div()
            .absolute()
            .left(relative(rect.x / width))
            .top(relative(rect.y / TREEMAP_HEIGHT))
            .w(relative(rect.w / width))
            .h(relative(rect.h / TREEMAP_HEIGHT))
            .p(px(1.0));
        let text_fits = rect.w >= MIN_TEXT_WIDTH && rect.h >= MIN_TEXT_HEIGHT;
        let name = node.name.clone();
        let value = self.reading(node.size);
        if cell.group {
            let strip = interactive(
                div()
                    .id(kit::part(&self.id, format_args!("node-{}", cell.node)))
                    .h(px(GROUP_HEADER - 1.0))
                    .px(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(11.0))
                    .line_height(px(14.0))
                    .rounded_t(px(5.0))
                    .when(hovered, |strip| strip.bg(self.fill(focus, cell.node, 0.22)))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.colors.foreground.to_paint())
                            .child(name),
                    )
                    .child(
                        div()
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child(value),
                    ),
                state,
                cell.node,
                true,
            );
            return placed
                .child(
                    div()
                        .size_full()
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(self.fill(focus, cell.node, 0.35))
                        .bg(self.fill(focus, cell.node, 0.08))
                        .child(strip),
                )
                .into_any_element();
        }
        let tile = div()
            .id(kit::part(&self.id, format_args!("node-{}", cell.node)))
            .size_full()
            .flex()
            .flex_col()
            .px(px(6.0))
            .py(px(3.0))
            .overflow_hidden()
            .rounded(px(5.0))
            .bg(self.fill(focus, cell.node, if hovered { 0.72 } else { 0.5 }))
            .when(hovered, |tile| tile.shadow(hover_highlight_shadow(&theme)))
            .when(text_fits, |tile| {
                tile.child(
                    div()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_size(px(11.0))
                        .line_height(px(14.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.colors.foreground.to_paint())
                        .child(name),
                )
                .when(rect.h >= 36.0, |tile| {
                    tile.child(
                        div()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_size(px(11.0))
                            .line_height(px(14.0))
                            .text_color(theme.colors.foreground.with_alpha(0.7).to_paint())
                            .child(value),
                    )
                })
            });
        placed
            .child(interactive(tile, state, cell.node, zoomable))
            .into_any_element()
    }

    /// One icicle or flame bar.
    fn bar(
        &self,
        state: &Entity<HierarchyState>,
        bar: &Bar,
        depth: usize,
        focus: usize,
        hover: Option<usize>,
        width: f32,
    ) -> AnyElement {
        let theme = self.theme;
        let node = self.tree.node(bar.node);
        let hovered = hover == Some(bar.node);
        let row = match self.layout {
            Layout::Flame => depth - 1 - bar.level,
            Layout::Treemap | Layout::Icicle => bar.level,
        };
        #[expect(clippy::cast_precision_loss, reason = "levels are few")]
        let top = row as f32 * LEVEL_PITCH;
        let wide = (bar.x1 - bar.x0) * width >= MIN_TEXT_WIDTH;
        // The focus bar zooms out; the rest zoom in when they have children.
        let target = if bar.node == focus {
            node.parent
        } else {
            (!node.children.is_empty()).then_some(bar.node)
        };
        let alpha = if hovered { 0.72 } else { 0.5 };
        let tile = div()
            .id(kit::part(&self.id, format_args!("node-{}", bar.node)))
            .size_full()
            .flex()
            .items_center()
            .px(px(6.0))
            .overflow_hidden()
            .whitespace_nowrap()
            .rounded(px(4.0))
            .bg(self.fill(focus, bar.node, alpha))
            .when(hovered, |tile| tile.shadow(hover_highlight_shadow(&theme)))
            .text_size(px(11.0))
            .line_height(px(14.0))
            .text_color(theme.colors.foreground.to_paint())
            .when(wide, |tile| tile.child(node.name.clone()));
        let tile = match target {
            Some(target) => {
                let state_for_click = state.clone();
                tile.cursor_pointer()
                    .on_click(move |_, _, cx| zoom(&state_for_click, target, cx))
            }
            None => tile,
        };
        div()
            .absolute()
            .left(relative(bar.x0))
            .w(relative(bar.x1 - bar.x0))
            .top(px(top))
            .h(px(LEVEL_HEIGHT))
            .px(px(0.5))
            .child(interactive(tile, state, bar.node, false))
            .into_any_element()
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
fn interactive(
    part: gpui::Stateful<Div>,
    state: &Entity<HierarchyState>,
    node: usize,
    zoomable: bool,
) -> gpui::Stateful<Div> {
    let hovering = state.clone();
    let part = part.on_hover(move |hovered, _, cx| {
        hovering.update(cx, |state, cx| {
            let next = if *hovered {
                Some(node)
            } else if state.hover == Some(node) {
                None
            } else {
                return;
            };
            if state.hover != next {
                state.hover = next;
                cx.notify();
            }
        });
    });
    if zoomable {
        let state = state.clone();
        part.cursor_pointer()
            .on_click(move |_, _, cx| zoom(&state, node, cx))
    } else {
        part
    }
}

fn zoom(state: &Entity<HierarchyState>, node: usize, cx: &mut App) {
    state.update(cx, |state, cx| {
        if state.focus != node {
            state.focus = node;
            state.hover = None;
            cx.notify();
        }
    });
}
