use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use aqua_core::vector::{FillRule, GpuiPath, GpuiPathStyle, PathCommand};
use aqua_core::{
    CornerRadii, DisplayItem, GpuiBackground, GpuiGradientColorSpace, GpuiGradientStop, Point,
    Rect, Transform2D,
};
use aqua_ui::element::{Box as UiBox, Canvas, Element};
use aqua_ui::style::{GpuiShadowLayer, Style};

use super::motion::MotionCurve;
use super::theme::{ArtisanTheme, Oklch};

pub const DURATION: Duration = Duration::from_millis(650);
const APPROACH: f64 = 0.4;
const FADE: f64 = 0.18;
const GLUE_START: f64 = 0.2;
const GLUE: f64 = 0.55;
const CURL_START: f64 = 0.5;
const CURL: f64 = 0.45;
const PRESS_START: f64 = 0.72;
const OFFSET_X: f32 = 20.0;
const OFFSET_Y: f32 = 16.0;
const LIFT_SCALE: f32 = 0.04;
const PRESS_SCALE: f32 = 0.006;
const CURL_EDGE: f32 = 14.0;
const SHEEN_EDGE: f32 = 0.14;
const SHEEN_ALPHA: f32 = 0.14;
const LIFT_ALPHA: f32 = 0.22;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StickerFrame {
    pub opacity: f32,
    pub shift: Point,
    pub scale: f32,
    pub glue: f32,
    pub curl: f32,
}

impl StickerFrame {
    #[must_use]
    pub fn at(elapsed: Duration) -> Self {
        let t = elapsed.as_secs_f64() / DURATION.as_secs_f64();
        let approach = MotionCurve::SmoothOut.sample_f32(t / APPROACH);
        let glue = MotionCurve::EaseInOut.sample_f32((t - GLUE_START) / GLUE);
        let curl = 1.0 - MotionCurve::SmoothOut.sample_f32((t - CURL_START) / CURL);
        let press = ((t - PRESS_START) / (1.0 - PRESS_START)).clamp(0.0, 1.0);
        let press = (press * std::f64::consts::PI).sin() as f32 * PRESS_SCALE;
        Self {
            opacity: MotionCurve::EaseOut.sample_f32(t / FADE),
            shift: Point::new(OFFSET_X * (1.0 - approach), -OFFSET_Y * (1.0 - approach)),
            scale: LIFT_SCALE.mul_add(1.0 - approach, 1.0) - press,
            glue,
            curl,
        }
    }

    fn transform(self, width: f32) -> Transform2D {
        Transform2D::translate(-width, 0.0)
            .then(Transform2D::scale(self.scale, self.scale))
            .then(Transform2D::translate(width + self.shift.x, self.shift.y))
    }
}

pub struct StickerLive {
    key: String,
    width: Cell<f32>,
    rest: RefCell<Vec<GpuiShadowLayer>>,
    air: Cell<Option<Oklch>>,
}

impl StickerLive {
    #[must_use]
    pub fn new(key: String) -> Rc<Self> {
        Rc::new(Self {
            key,
            width: Cell::new(0.0),
            rest: RefCell::new(Vec::new()),
            air: Cell::new(None),
        })
    }

    fn part(&self, part: &str) -> String {
        format!("{}/sticker-{part}", self.key)
    }

    pub fn paint(&self, ui: &mut aqua::UiApp, frame: StickerFrame) -> bool {
        let mut changed = self.repaint(ui, "bubble", |paint| {
            paint.opacity = Some(frame.opacity);
            paint.transform = Some(frame.transform(self.width.get()));
            paint.gpui_shadows = self
                .rest
                .borrow()
                .iter()
                .cloned()
                .chain((frame.glue < 1.0).then(|| lift_shadow(frame.glue)))
                .collect();
        });
        if let Some(air) = self.air.get() {
            changed |= self.repaint(ui, "sheen", |paint| {
                paint.gpui_background = Some(sheen_background(air, frame.glue));
            });
        }
        changed |= self.repaint(ui, "ear", |paint| {
            paint.transform = Some(ear_transform(frame.curl));
        });
        changed
    }

    fn repaint(
        &self,
        ui: &mut aqua::UiApp,
        part: &str,
        update: impl FnOnce(&mut aqua_ui::style::Paint),
    ) -> bool {
        let key = aqua_ui::tree::hash_key(aqua_ui::element::Key::from(self.part(part)));
        let Some(node) = ui.tree().sole_node_with_key(key) else {
            return false;
        };
        let Some(mut paint) = ui.tree().get(node).map(|record| record.paint().clone()) else {
            return false;
        };
        update(&mut paint);
        ui.tree_mut().set_paint(node, paint)
    }
}

#[derive(Clone)]
pub struct Sticker {
    pub frame: StickerFrame,
    pub live: Rc<StickerLive>,
}

fn lift_shadow(glue: f32) -> GpuiShadowLayer {
    let lift = 1.0 - glue;
    GpuiShadowLayer {
        dx: -6.0 * lift,
        dy: 12.0 * lift,
        blur: 28.0 * lift,
        spread: 0.0,
        inset: false,
        color: Oklch::new(0.0, 0.0, 0.0)
            .with_alpha(LIFT_ALPHA * lift)
            .to_reference_paint(),
    }
}

fn ear_transform(curl: f32) -> Transform2D {
    let scale = curl.max(0.0);
    Transform2D::translate(0.0, -CURL_EDGE)
        .then(Transform2D::scale(scale, scale))
        .then(Transform2D::translate(0.0, CURL_EDGE))
}

impl Sticker {
    #[must_use]
    pub fn key(&self) -> String {
        self.live.part("bubble")
    }

    #[must_use]
    pub fn style(&self) -> Style {
        Style::new()
            .opacity(self.frame.opacity)
            .transform(self.frame.transform(self.live.width.get()))
    }

    pub fn rest_on(&self, shadows: &[GpuiShadowLayer]) {
        self.live.rest.replace(shadows.to_vec());
    }

    #[must_use]
    pub fn lift_shadow(&self) -> GpuiShadowLayer {
        lift_shadow(self.frame.glue)
    }

    #[must_use]
    pub fn layers(&self, theme: &ArtisanTheme, corners: CornerRadii, curl: bool) -> Vec<Element> {
        let air = match theme.colors.mode.is_dark() {
            true => Oklch::new(1.0, 0.0, 0.0),
            false => Oklch::new(0.0, 0.0, 0.0),
        };
        self.live.air.set(Some(air));
        let mut layers = vec![
            self.probe(),
            sheen(corners, air, self.frame.glue).with_key(self.live.part("sheen")),
        ];
        if curl {
            layers.push(dog_ear(theme, self.frame.curl).with_key(self.live.part("ear")));
        }
        layers
    }

    fn probe(&self) -> Element {
        let live = Rc::clone(&self.live);
        Canvas::new()
            .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
            .on_prepaint(move |tree, node| {
                if let Some(bounds) = tree
                    .get(node)
                    .and_then(|record| record.parent)
                    .and_then(|parent| tree.pre_transform_rect(parent))
                {
                    live.width.set(bounds.w);
                }
            })
            .build()
            .exclude_from_hit_testing()
    }
}

fn sheen_background(air: Oklch, glue: f32) -> GpuiBackground {
    let edge = 1.0 - glue;
    let alpha = if glue < 1.0 { SHEEN_ALPHA } else { 0.0 };
    GpuiBackground::LinearGradient {
        angle: 45.0,
        color_space: GpuiGradientColorSpace::Oklab,
        stops: [
            GpuiGradientStop {
                percentage: (edge - SHEEN_EDGE).max(0.0),
                color: air.with_alpha(alpha).to_reference_paint(),
            },
            GpuiGradientStop {
                percentage: edge,
                color: air.with_alpha(0.0).to_reference_paint(),
            },
        ],
    }
}

fn sheen(corners: CornerRadii, air: Oklch, glue: f32) -> Element {
    UiBox::new()
        .style(
            Style::new()
                .absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
                .corners(corners)
                .gpui_background(sheen_background(air, glue)),
        )
        .build()
        .exclude_from_hit_testing()
}

fn triangle(bounds: Rect, points: [Point; 3], background: GpuiBackground) -> DisplayItem {
    let [first, second, third] = points;
    DisplayItem::GpuiPath {
        transformed_window_logical_bounds: bounds,
        path: Arc::new(GpuiPath {
            commands_local: vec![
                PathCommand::Move(first),
                PathCommand::Line(second),
                PathCommand::Line(third),
                PathCommand::Close,
            ],
            origin_window_logical: bounds.origin(),
            element_transform: Transform2D::IDENTITY,
            style: GpuiPathStyle::Fill {
                rule: FillRule::NonZero,
            },
            background,
        }),
    }
}

fn dog_ear(theme: &ArtisanTheme, curl: f32) -> Element {
    let edge = CURL_EDGE;
    let page = theme.colors.background;
    let face = theme.colors.gray.step(match theme.colors.mode.is_dark() {
        true => 6,
        false => 2,
    });
    let fold = theme.colors.gray.step(match theme.colors.mode.is_dark() {
        true => 3,
        false => 5,
    });
    Canvas::new()
        .style(
            Style::new()
                .absolute_at(Some(0.0), None, None, Some(0.0))
                .width(aqua_ui::style::Length::Px(CURL_EDGE))
                .height(aqua_ui::style::Length::Px(CURL_EDGE))
                .transform(ear_transform(curl)),
        )
        .on_paint(move |bounds| {
            let h = bounds.h;
            let corner = [
                Point::new(0.0, h - edge),
                Point::new(edge, h),
                Point::new(0.0, h),
            ];
            let flap = [
                Point::new(0.0, h - edge),
                Point::new(edge, h),
                Point::new(edge, h - edge),
            ];
            vec![
                triangle(
                    bounds,
                    corner,
                    GpuiBackground::Solid(page.to_reference_paint()),
                ),
                triangle(
                    bounds,
                    flap,
                    GpuiBackground::LinearGradient {
                        angle: 45.0,
                        color_space: GpuiGradientColorSpace::Oklab,
                        stops: [
                            GpuiGradientStop {
                                percentage: 0.0,
                                color: fold.to_reference_paint(),
                            },
                            GpuiGradientStop {
                                percentage: 1.0,
                                color: face.to_reference_paint(),
                            },
                        ],
                    },
                ),
            ]
        })
        .build()
        .exclude_from_hit_testing()
}

#[cfg(test)]
#[path = "../../tests/ui/sticker.rs"]
mod tests;
