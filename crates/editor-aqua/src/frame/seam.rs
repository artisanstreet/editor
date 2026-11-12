pub mod motion;
pub mod outline;

use crate::ui::chrome::card::{Edge, rest_shadows};
use crate::ui::kit::Radius;
use crate::ui::theme::ArtisanTheme;
use aqua::{Aqua, Style};
use aqua_core::shape::{ShapeProgram, ShapeTransition};
use aqua_core::vector::{FillRule, GpuiPath, GpuiPathStyle};
use aqua_core::{DisplayItem, GpuiGradientColorSpace, GpuiHsla, Rect, Transform2D};
use aqua_ui::element::Element;
use palette::FromColor;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

pub const ACTIVE: &str = "artisan-active-thread-tab";
pub const SHEET: &str = "artisan-thread-sheet";

pub fn surface(theme: ArtisanTheme, motion: Rc<RefCell<motion::Motion>>, reduced: bool) -> Element {
    let geometry = Rc::new(Cell::new(None));
    let measured = Rc::clone(&geometry);
    let targeting = Rc::clone(&motion);
    Aqua::Canvas()
        .styles([Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0))])
        .on_prepaint(move |tree, canvas| {
            measured.set(None);
            let Some(parent) = tree.get(canvas).and_then(|node| node.parent) else {
                return;
            };
            let mut nodes = vec![parent];
            let mut sheet = None;
            let mut tab = None;
            while let Some(id) = nodes.pop() {
                let Some(node) = tree.get(id) else {
                    continue;
                };
                match node.debug_selector.as_deref() {
                    Some(SHEET) => {
                        sheet = tree.rect(id);
                        continue;
                    }
                    Some(ACTIVE) => tab = node.key.zip(tree.rect(id)),
                    _ => {}
                }
                nodes.extend(node.children.iter().copied());
            }
            let Some(sheet) = sheet else {
                return;
            };
            let Some(bounds) = tree.rect(canvas) else {
                return;
            };
            let local = sheet.translate(-bounds.x, -bounds.y);
            let tab = tab.map(|(key, rect)| {
                (
                    key,
                    Rect::from_edges(
                        rect.x,
                        rect.y + (rect.h - crate::frame::strip::HEIGHT) * 0.5,
                        rect.right(),
                        sheet.y,
                    )
                    .translate(-bounds.x, -bounds.y),
                )
            });
            targeting
                .borrow_mut()
                .target(local, tab, std::time::Instant::now(), true);
            measured.set(Some(sheet));
        })
        .on_paint(move |bounds| {
            let Some(sheet) = geometry.get() else {
                return Vec::new();
            };
            let local = sheet.translate(-bounds.x, -bounds.y);
            let target = motion
                .borrow()
                .destination()
                .unwrap_or(Rect::from_edges(local.x, local.y, local.x, local.y));
            let edge = Edge::of(theme.colors.background, theme.colors.text);
            let colors = [
                edge.rim_top,
                edge.rim_bottom,
                edge.highlight_top,
                edge.highlight_bottom,
                theme.colors.background,
            ];
            let mut parameters = vec![local.x, local.y, local.w, local.h];
            for color in colors {
                let color = color.to_reference_paint();
                parameters.extend([color.h, color.s, color.l, color.a]);
            }
            vec![DisplayItem::Shape {
                bounds,
                damage: Rect::from_edges(
                    sheet.x - 24.0,
                    sheet.y - crate::frame::strip::HEIGHT - 24.0,
                    sheet.right() + 24.0,
                    sheet.y + Radius::Shell.px() + 24.0,
                ),
                program: Arc::new(ShapeProgram {
                    name: "artisan.thread-seam".into(),
                    parameters: parameters.into(),
                    paint: paint_shape,
                }),
                target,
                transition: ShapeTransition {
                    duration: std::time::Duration::from_millis(match (reduced, target.h > 0.001) {
                        (true, _) => 0,
                        (false, true) => 250,
                        (false, false) => 150,
                    }),
                    easing: [0.22, 1.0, 0.36, 1.0],
                    vertical_reveal: true,
                },
            }]
        })
        .build()
        .with_key("artisan-thread-outline")
        .exclude_from_hit_testing()
}

#[expect(
    clippy::disallowed_types,
    reason = "the seam sheet is a path-traced silhouette painted layer by layer; no kit surface can draw it"
)]
fn paint_shape(bounds: Rect, target: Rect, parameters: &[f32]) -> Vec<DisplayItem> {
    let local = Rect {
        x: parameters[0],
        y: parameters[1],
        w: parameters[2],
        h: parameters[3],
    };
    let sheet = local.translate(bounds.x, bounds.y);
    let tab = (target.h > 0.001).then_some(target);
    let colors: [GpuiHsla; 5] = std::array::from_fn(|i| {
        let offset = 4 + i * 4;
        GpuiHsla {
            h: parameters[offset],
            s: parameters[offset + 1],
            l: parameters[offset + 2],
            a: parameters[offset + 3],
        }
    });
    let gradient = |start, end| aqua_core::GpuiBackground::LinearGradient {
        angle: 180.0,
        color_space: GpuiGradientColorSpace::Srgb,
        stops: [
            aqua_core::GpuiGradientStop {
                percentage: 0.0,
                color: start,
            },
            aqua_core::GpuiGradientStop {
                percentage: 1.0,
                color: end,
            },
        ],
    };
    let layers = [
        gradient(colors[0], colors[1]),
        gradient(colors[2], colors[3]),
        aqua_core::GpuiBackground::Solid(colors[4]),
    ];
    let mut items = rest_shadows()
        .into_iter()
        .map(|layer| DisplayItem::Shadow {
            bounds: sheet,
            corners: aqua_core::CornerRadii::uniform(Radius::Shell.px()),
            layer,
        })
        .collect::<Vec<_>>();
    for (inset, background) in layers.into_iter().enumerate() {
        let inset = inset as f32;
        let (commands, body) = outline::split(local, tab, inset);
        let gradient_top = tab
            .filter(|tab| {
                tab.right() > local.x + inset
                    && tab.x < local.right() - inset
                    && tab.y + inset < local.y + inset
            })
            .map_or(local.y + inset, |tab| tab.y + inset);
        let gradient_bottom = local.bottom() - inset;
        if body.w > 0.0 && body.h > 0.0 {
            let radius = (Radius::Shell.px() - inset)
                .max(0.0)
                .min(body.w * 0.5)
                .min((local.h - inset * 2.0).max(0.0) * 0.5);
            items.push(DisplayItem::GpuiBackground {
                bounds: body.translate(bounds.x, bounds.y),
                corners: aqua_core::CornerRadii {
                    top_left: 0.0,
                    top_right: 0.0,
                    bottom_left: radius,
                    bottom_right: radius,
                },
                background: gradient_slice(
                    background,
                    gradient_top,
                    gradient_bottom,
                    body.y,
                    body.bottom(),
                ),
            });
        }
        let path_bottom = if body.h > 0.0 {
            body.y + 1.0
        } else {
            gradient_bottom
        };
        items.push(DisplayItem::GpuiPath {
            transformed_window_logical_bounds: bounds,
            path: Arc::new(GpuiPath {
                commands_local: commands,
                origin_window_logical: bounds.origin(),
                element_transform: Transform2D::IDENTITY,
                style: GpuiPathStyle::Fill {
                    rule: FillRule::NonZero,
                },
                background: gradient_slice(
                    background,
                    gradient_top,
                    gradient_bottom,
                    gradient_top,
                    path_bottom,
                ),
            }),
        });
    }
    items
}

#[expect(
    clippy::disallowed_types,
    reason = "slices the seam sheet's painted gradient layers to each path segment"
)]
fn gradient_slice(
    background: aqua_core::GpuiBackground,
    from: f32,
    to: f32,
    top: f32,
    bottom: f32,
) -> aqua_core::GpuiBackground {
    match background {
        aqua_core::GpuiBackground::LinearGradient {
            angle,
            color_space,
            stops,
        } => {
            let start = stops[0].color.to_srgba();
            let end = stops[1].color.to_srgba();
            let sample = |y: f32| {
                let fraction = ((y - from) / (to - from).max(f32::EPSILON)).clamp(0.0, 1.0);
                let rgba: [f32; 4] =
                    std::array::from_fn(|i| (end[i] - start[i]).mul_add(fraction, start[i]));
                let color = palette::Hsla::from_color(palette::Srgba::new(
                    rgba[0], rgba[1], rgba[2], rgba[3],
                ));
                GpuiHsla {
                    h: color.hue.into_positive_degrees() / 360.0,
                    s: color.saturation,
                    l: color.lightness,
                    a: color.alpha,
                }
            };
            aqua_core::GpuiBackground::LinearGradient {
                angle,
                color_space,
                stops: [
                    aqua_core::GpuiGradientStop {
                        percentage: 0.0,
                        color: sample(top),
                    },
                    aqua_core::GpuiGradientStop {
                        percentage: 1.0,
                        color: sample(bottom),
                    },
                ],
            }
        }
        solid => solid,
    }
}
