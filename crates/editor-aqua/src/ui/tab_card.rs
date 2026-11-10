use std::rc::Rc;
use std::sync::Arc;

use aqua_core::{
    CornerRadii, GpuiBackground, GpuiGradientColorSpace, GpuiGradientStop, Point, Rect, Size,
    Transform2D,
    vector::{FillRule, GpuiPath, GpuiPathStyle, LineCap, LineJoin, PathCommand},
};

use super::glass::{COMPOSER_RADIUS, KEY_RADIUS};
use super::theme::{ArtisanTheme, Elevation, Oklch, ShadowLayer};

pub struct TabCardStrip {
    pub reveal: f32,
    pub content: aqua_ui::element::Element,
}

pub struct TabCard {
    pub id: String,
    pub tab: aqua_ui::element::Element,
    pub body: aqua_ui::element::Element,
    pub strip: Option<TabCardStrip>,
}

impl TabCard {
    pub fn build(
        self,
        theme: ArtisanTheme,
        cache: Rc<std::cell::RefCell<GeometryCache>>,
    ) -> aqua_ui::element::Element {
        use aqua_ui::element::{Box as UiBox, Canvas, Key};
        use aqua_ui::style::{Axis, Length, Style};
        let reveal = self
            .strip
            .as_ref()
            .map_or(0.0, |strip| strip.reveal.clamp(0.0, 1.0));
        let tab_key = aqua_ui::tree::hash_key(Key::from(format!("{}-tab", self.id)));
        let width = Rc::new(std::cell::Cell::new(None));
        let prepaint_width = width.clone();
        let probe = Canvas::new()
            .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
            .on_prepaint(move |tree, canvas| {
                let Some(shell) = tree.get(canvas).and_then(|node| node.parent) else {
                    return;
                };
                let mut nodes = vec![shell];
                prepaint_width.set(None);
                while let Some(node) = nodes.pop() {
                    let Some(record) = tree.get(node) else {
                        continue;
                    };
                    if record.key == Some(tab_key) {
                        prepaint_width.set(tree.rect(node).map(|rect| rect.w));
                        break;
                    }
                    nodes.extend(record.children.iter().copied());
                }
            })
            .on_paint(move |bounds| {
                let Some(tab_width) = width.get() else {
                    return Vec::new();
                };
                PreparedTabCard::new(&theme, bounds, tab_width, reveal, &mut cache.borrow_mut())
                    .display_items()
            })
            .build()
            .exclude_from_hit_testing();
        let tab = UiBox::new()
            .height(TAB_CARD_TAB_HEIGHT_PX)
            .style(
                Style::new()
                    .axis(Axis::Row)
                    .align(aqua_ui::AlignItems::CENTER)
                    .padding_insets(aqua_core::Insets::hv(4.0, 0.0)),
            )
            .child(self.tab)
            .build()
            .with_key(format!("{}-tab", self.id));
        let mut row = UiBox::new()
            .width(Length::Pct(100.0))
            .layout("row")
            .child(tab);
        if let Some(strip) = self.strip {
            let content = UiBox::new()
                .style(
                    Style::new()
                        .absolute_at(Some(0.0), Some(0.0), None, None)
                        .width(Length::Pct(strip.reveal.clamp(0.0, 1.0) * 100.0))
                        .height(Length::Pct(100.0)),
                )
                .clip(true)
                .child(strip.content)
                .build();
            row = row.child(
                UiBox::new()
                    .height(TAB_CARD_TAB_HEIGHT_PX)
                    .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                    .child(content)
                    .build(),
            );
        }
        UiBox::new()
            .width(Length::Pct(100.0))
            .style(
                Style::new()
                    .axis(Axis::Column)
                    .align(aqua_ui::AlignItems::START),
            )
            .child(probe)
            .child(row.build())
            .child(
                UiBox::new()
                    .width(Length::Pct(100.0))
                    .padding(TAB_CARD_PADDING_PX)
                    .child(self.body)
                    .build(),
            )
            .build()
            .with_key(self.id)
    }
}

pub const TAB_CARD_PADDING_PX: f32 = 4.0;
pub const TAB_CARD_TAB_HEIGHT_PX: f32 = 36.0;
const TAB_RADIUS: f32 = KEY_RADIUS + TAB_CARD_PADDING_PX;
const FILLET_RADIUS: f32 = 8.0;
const SHELL_RADIUS: f32 = COMPOSER_RADIUS + TAB_CARD_PADDING_PX;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeometryKey {
    pub size: Size,
    pub tab_width: f32,
    pub radius: f32,
    pub reveal: f32,
}

#[derive(Clone, Debug, Default)]
pub struct GeometryCache {
    key: Option<GeometryKey>,
    paths: Rc<Geometry>,
}

impl GeometryCache {
    pub fn get(&mut self, key: GeometryKey) -> Rc<Geometry> {
        if self.key != Some(key) {
            self.paths = Rc::new(Geometry::new(key));
            self.key = Some(key);
        }
        self.paths.clone()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Geometry {
    pub strip: Option<Vec<PathCommand>>,
    pub well: Vec<PathCommand>,
    pub shell: Vec<PathCommand>,
    pub edge: Vec<PathCommand>,
    pub highlight: Vec<PathCommand>,
}

impl Geometry {
    pub fn new(key: GeometryKey) -> Self {
        let bounds = Rect::from_origin_size(Point::ZERO, key.size);
        let strip = strip_right(bounds, key.tab_width, key.reveal)
            .map(|right| trace_strip(bounds, key.tab_width, key.radius, right));
        let well = trace_card_hole(bounds, key.radius);
        let mut shell = trace_silhouette(bounds, key.tab_width, key.radius);
        shell.extend(well.clone());
        let edge_bounds = Rect::from_edges(0.5, 0.5, key.size.w - 0.5, key.size.h - 0.5);
        let edge = trace_silhouette(edge_bounds, key.tab_width - 1.0, key.radius - 0.5);
        let highlight_bounds = Rect::from_edges(1.5, 1.5, key.size.w - 1.5, key.size.h - 1.5);
        let highlight =
            trace_silhouette(highlight_bounds, key.tab_width - 3.0, key.radius - 1.5);
        Self {
            strip,
            well,
            shell,
            edge,
            highlight,
        }
    }
}

fn arc(radius: f32, sweep: bool, x: f32, y: f32) -> PathCommand {
    PathCommand::Arc {
        radii: Point::new(radius, radius),
        rotation_radians: 0.0,
        large: false,
        sweep,
        to: Point::new(x, y),
    }
}
fn mov(x: f32, y: f32) -> PathCommand {
    PathCommand::Move(Point::new(x, y))
}
fn line(x: f32, y: f32) -> PathCommand {
    PathCommand::Line(Point::new(x, y))
}

fn trace_silhouette(bounds: Rect, tab_width: f32, radius: f32) -> Vec<PathCommand> {
    let (x0, y0, x1, y1) = (bounds.x, bounds.y, bounds.right(), bounds.bottom());
    let tab_x1 = x0 + tab_width;
    let body_y0 = y0 + TAB_CARD_TAB_HEIGHT_PX;
    vec![
        mov(x0, y0 + TAB_RADIUS),
        arc(TAB_RADIUS, true, x0 + TAB_RADIUS, y0),
        line(tab_x1 - TAB_RADIUS, y0),
        arc(TAB_RADIUS, true, tab_x1, y0 + TAB_RADIUS),
        line(tab_x1, body_y0 - FILLET_RADIUS),
        arc(FILLET_RADIUS, false, tab_x1 + FILLET_RADIUS, body_y0),
        line(x1 - radius, body_y0),
        arc(radius, true, x1, body_y0 + radius),
        line(x1, y1 - radius),
        arc(radius, true, x1 - radius, y1),
        line(x0 + radius, y1),
        arc(radius, true, x0, y1 - radius),
        PathCommand::Close,
    ]
}

fn strip_right(bounds: Rect, tab_width: f32, reveal: f32) -> Option<f32> {
    let start = bounds.x + tab_width + TAB_RADIUS;
    let end = bounds.right();
    (reveal > 0.0 && end > start).then(|| start + (end - start) * reveal.min(1.0))
}

#[derive(Clone, Copy)]
enum StripBottom {
    Rounded(f32),
    Down(f32),
}
#[derive(Clone, Copy)]
struct StripEnd {
    top: f32,
    bottom: StripBottom,
}

fn strip_end(bounds: Rect, tab_width: f32, radius: f32, right: f32) -> StripEnd {
    let tab_x1 = bounds.x + tab_width;
    let corner_x = bounds.right() - radius;
    let top = TAB_RADIUS.min((right - tab_x1) / 2.0);
    let bottom = if right > corner_x {
        let dx = (right - corner_x).min(radius);
        StripBottom::Down(radius - dx.mul_add(-dx, radius * radius).max(0.0).sqrt())
    } else {
        StripBottom::Rounded(top.min(corner_x - right))
    };
    StripEnd { top, bottom }
}

fn trace_strip(bounds: Rect, tab_width: f32, radius: f32, right: f32) -> Vec<PathCommand> {
    let y0 = bounds.y;
    let body_y0 = y0 + TAB_CARD_TAB_HEIGHT_PX;
    let tab_x1 = bounds.x + tab_width;
    let corner_x = bounds.right() - radius;
    let end = strip_end(bounds, tab_width, radius, right);
    let mut path = vec![
        mov(tab_x1 - TAB_RADIUS, y0),
        line(right - end.top, y0),
        arc(end.top, true, right, y0 + end.top),
    ];
    match end.bottom {
        StripBottom::Rounded(bottom) => {
            path.push(line(right, body_y0 - bottom));
            if bottom > 0.0 {
                path.push(arc(bottom, true, right - bottom, body_y0));
            }
        }
        StripBottom::Down(drop) => {
            path.push(line(right, body_y0 + drop));
            path.push(arc(radius, false, corner_x, body_y0));
        }
    }
    path.extend([
        line(tab_x1 + FILLET_RADIUS, body_y0),
        arc(FILLET_RADIUS, true, tab_x1, body_y0 - FILLET_RADIUS),
        line(tab_x1, y0 + TAB_RADIUS),
        arc(TAB_RADIUS, false, tab_x1 - TAB_RADIUS, y0),
        PathCommand::Close,
    ]);
    path
}

fn trace_card_hole(bounds: Rect, radius: f32) -> Vec<PathCommand> {
    let x0 = bounds.x + TAB_CARD_PADDING_PX;
    let y0 = bounds.y + TAB_CARD_TAB_HEIGHT_PX + TAB_CARD_PADDING_PX;
    let x1 = bounds.right() - TAB_CARD_PADDING_PX;
    let y1 = bounds.bottom() - TAB_CARD_PADDING_PX;
    let r = radius - TAB_CARD_PADDING_PX;
    vec![
        mov(x0 + r, y0),
        arc(r, false, x0, y0 + r),
        line(x0, y1 - r),
        arc(r, false, x0 + r, y1),
        line(x1 - r, y1),
        arc(r, false, x1, y1 - r),
        line(x1, y0 + r),
        arc(r, false, x1 - r, y0),
        PathCommand::Close,
    ]
}

#[derive(Clone, Debug)]
pub struct ShadowPass {
    pub bounds: Rect,
    pub corners: CornerRadii,
    pub layer: ShadowLayer,
    pub clip: Rect,
}
#[derive(Clone, Copy, Debug)]
pub struct BackdropPass {
    pub bounds: Rect,
    pub corners: CornerRadii,
    pub blur: f32,
}
#[derive(Clone, Debug)]
pub struct PathPass {
    pub transformed_window_logical_bounds: Rect,
    pub path: Arc<GpuiPath>,
}
#[derive(Clone, Debug)]
pub struct PreparedTabCard {
    pub shadows: Vec<ShadowPass>,
    pub backdrops: Vec<BackdropPass>,
    pub paths: Vec<PathPass>,
}

fn outside(rect: Rect, reach: Rect) -> [Rect; 4] {
    [
        Rect::from_edges(reach.x, reach.y, reach.right(), rect.y),
        Rect::from_edges(reach.x, rect.bottom(), reach.right(), reach.bottom()),
        Rect::from_edges(reach.x, rect.y, rect.x, rect.bottom()),
        Rect::from_edges(rect.right(), rect.y, reach.right(), rect.bottom()),
    ]
}

fn background(from: Oklch, to: Oklch, angle: f32, end: f32) -> GpuiBackground {
    GpuiBackground::LinearGradient {
        angle,
        color_space: GpuiGradientColorSpace::Oklab,
        stops: [
            GpuiGradientStop {
                percentage: 0.0,
                color: from.to_reference_paint(),
            },
            GpuiGradientStop {
                percentage: end,
                color: to.to_reference_paint(),
            },
        ],
    }
}

impl PreparedTabCard {
    pub fn display_items(&self) -> Vec<aqua_core::DisplayItem> {
        use aqua_core::DisplayItem;
        let mut items = Vec::new();
        for shadow in &self.shadows {
            items.push(DisplayItem::PushGpuiContentMask {
                bounds: shadow.clip,
            });
            items.push(DisplayItem::GpuiShadow {
                bounds: shadow.bounds,
                corners: shadow.corners,
                layer: shadow.layer.into(),
            });
            items.push(DisplayItem::PopClip);
        }
        for backdrop in &self.backdrops {
            items.push(DisplayItem::BackdropBlurRounded {
                bounds: backdrop.bounds,
                radius: backdrop.blur,
                corners: backdrop.corners,
            });
        }
        for path in &self.paths {
            items.push(DisplayItem::GpuiPath {
                transformed_window_logical_bounds: path.transformed_window_logical_bounds,
                path: path.path.clone(),
            });
        }
        items
    }

    pub fn new(
        theme: &ArtisanTheme,
        bounds: Rect,
        tab_width: f32,
        reveal: f32,
        cache: &mut GeometryCache,
    ) -> Self {
        let radius = SHELL_RADIUS;
        let reveal = reveal.clamp(0.0, 1.0);
        let geometry = cache.get(GeometryKey {
            size: bounds.size(),
            tab_width,
            radius,
            reveal,
        });
        let local = Rect::from_origin_size(Point::ZERO, bounds.size());
        let strip = strip_right(local, tab_width, reveal);
        let mut shadows = Vec::new();
        let mut backdrops = Vec::new();
        let mut paths = Vec::new();
        if tab_width <= 0.0 {
            backdrops.push(BackdropPass {
                bounds: Rect::from_edges(
                    bounds.x,
                    bounds.y + TAB_CARD_TAB_HEIGHT_PX,
                    bounds.right(),
                    bounds.bottom(),
                ),
                corners: CornerRadii {
                    top_left: 0.0,
                    top_right: if reveal < 1.0 { radius } else { 0.0 },
                    bottom_right: radius,
                    bottom_left: radius,
                },
                blur: 20.0,
            });
            return Self {
                shadows,
                backdrops,
                paths,
            };
        }
        let body_y = bounds.y + TAB_CARD_TAB_HEIGHT_PX;
        let (right, top_right, bottom_right, bottom) =
            strip.map_or((bounds.x + tab_width, TAB_RADIUS, 0.0, body_y), |right| {
                let end = strip_end(local, tab_width, radius, right);
                match end.bottom {
                    StripBottom::Rounded(corner) => (bounds.x + right, end.top, corner, body_y),
                    StripBottom::Down(drop) => (bounds.x + right, end.top, 0.0, body_y + drop),
                }
            });
        let body = Rect::from_edges(bounds.x, body_y, bounds.right(), bounds.bottom());
        let body_corners = CornerRadii {
            top_left: 0.0,
            top_right: radius,
            bottom_right: radius,
            bottom_left: radius,
        };
        let tab_corners = CornerRadii {
            top_left: TAB_RADIUS,
            top_right,
            bottom_right,
            bottom_left: 0.0,
        };
        let reach = bounds.inflate(128.0);
        for layer in theme.elevation.drops(Elevation::Floating) {
            let tab = Rect::from_edges(
                bounds.x,
                bounds.y,
                right,
                bottom + (-layer.spread_radius).max(0.0) * 2.0,
            );
            for clip in outside(body, reach) {
                shadows.push(ShadowPass {
                    bounds: tab,
                    corners: tab_corners,
                    layer: *layer,
                    clip,
                });
            }
            for clip in outside(tab, reach) {
                shadows.push(ShadowPass {
                    bounds: body,
                    corners: body_corners,
                    layer: *layer,
                    clip,
                });
            }
        }
        backdrops.push(BackdropPass {
            bounds: Rect::from_edges(
                bounds.x,
                bounds.y,
                bounds.x + tab_width,
                bounds.y + TAB_CARD_TAB_HEIGHT_PX,
            ),
            corners: CornerRadii {
                top_left: TAB_RADIUS,
                top_right: TAB_RADIUS,
                bottom_right: 0.0,
                bottom_left: 0.0,
            },
            blur: 20.0,
        });
        backdrops.push(BackdropPass {
            bounds: body,
            corners: CornerRadii {
                top_left: 0.0,
                top_right: if reveal < 1.0 { radius } else { 0.0 },
                bottom_right: radius,
                bottom_left: radius,
            },
            blur: 20.0,
        });
        backdrops.swap(0, 1);
        if let Some(right) = strip {
            let end = strip_end(local, tab_width, radius, right);
            let bottom_right = match end.bottom {
                StripBottom::Rounded(bottom) => bottom,
                StripBottom::Down(_) => 0.0,
            };
            backdrops.push(BackdropPass {
                bounds: Rect::from_edges(bounds.x + tab_width, bounds.y, bounds.x + right, body_y),
                corners: CornerRadii {
                    top_left: 0.0,
                    top_right: end.top,
                    bottom_right,
                    bottom_left: 0.0,
                },
                blur: 20.0,
            });
        }
        let colors = &theme.colors;
        let surfaces = crate::frame::card::Surfaces::of(theme);
        let bevel = crate::frame::card::Edge::of(surfaces.bg, surfaces.ink);
        let surface = GpuiBackground::Solid(surfaces.bg.to_reference_paint());
        let mut push = |commands: &[PathCommand],
                        background: GpuiBackground,
                        stroke: bool,
                        local_bounds: Rect| {
            let style = if stroke {
                GpuiPathStyle::Stroke {
                    width: 1.0,
                    cap: LineCap::Butt,
                    join: LineJoin::Miter,
                    dash: Vec::new(),
                    dash_offset: 0.0,
                }
            } else {
                GpuiPathStyle::Fill {
                    rule: FillRule::NonZero,
                }
            };
            paths.push(PathPass {
                transformed_window_logical_bounds: local_bounds.translate(bounds.x, bounds.y),
                path: Arc::new(GpuiPath {
                    commands_local: commands.to_vec(),
                    origin_window_logical: bounds.origin(),
                    element_transform: Transform2D::IDENTITY,
                    style,
                    background,
                }),
            });
        };
        if let (Some(commands), Some(right)) = (&geometry.strip, strip) {
            let strip_bottom = match strip_end(local, tab_width, radius, right).bottom {
                StripBottom::Rounded(_) => TAB_CARD_TAB_HEIGHT_PX,
                StripBottom::Down(drop) => TAB_CARD_TAB_HEIGHT_PX + drop,
            };
            let strip_bounds = Rect::from_edges(tab_width - TAB_RADIUS, 0.0, right, strip_bottom);
            push(commands, surface.clone(), false, strip_bounds);
        }
        let hole = Rect::from_edges(4.0, 40.0, bounds.w - 4.0, bounds.h - 4.0);
        push(
            &geometry.well,
            GpuiBackground::Solid(colors.background.to_reference_paint()),
            false,
            hole,
        );
        push(&geometry.shell, surface, false, local);
        push(
            &geometry.edge,
            background(bevel.rim_top, bevel.rim_bottom, 180.0, 1.0),
            true,
            local,
        );
        push(
            &geometry.highlight,
            background(bevel.highlight_top, bevel.highlight_bottom, 180.0, 1.0),
            true,
            local,
        );
        Self {
            shadows,
            backdrops,
            paths,
        }
    }
}
