use std::sync::Arc;

use super::theme::{ArtisanTheme, Oklch, RadiusStep, RadiusTokens};
use aqua_core::{
    DisplayItem, GpuiBackground, GpuiGradientColorSpace, GpuiGradientStop, Insets, Point, Rect,
    Transform2D,
    vector::{FillRule, GpuiPath, GpuiPathStyle, PathCommand},
};
use aqua_text::{FontFamily, FontWeight, TextStyle};
use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Canvas, Element, Text},
    style::{GpuiShadowLayer, Length, Style},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BadgeVariant {
    #[default]
    Pill,
    Keycap,
}

pub struct Badge {
    theme: ArtisanTheme,
    label: String,
    variant: BadgeVariant,
}

impl Badge {
    pub fn new(theme: ArtisanTheme, label: impl Into<String>) -> Self {
        Self {
            theme,
            label: label.into(),
            variant: BadgeVariant::Pill,
        }
    }

    #[must_use]
    pub const fn variant(mut self, variant: BadgeVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn build(self) -> Element {
        match self.variant {
            BadgeVariant::Pill => pill(&self.theme, self.label),
            BadgeVariant::Keycap => keycap(&self.theme, self.label),
        }
    }
}

fn pill(t: &ArtisanTheme, label: String) -> Element {
    let text = Text::new(label)
        .text_style(TextStyle {
            families: vec![FontFamily::Named(t.typography.sans.family.to_owned())],
            size_px: t.typography.label_text,
            line_height_px: t.spacing.steps(4.0),
            weight: FontWeight::MEDIUM,
            ..TextStyle::default()
        })
        .gpui_source_color(t.colors.text.to_reference_text_color())
        .build();
    UiBox::new()
        .height(t.density.badge_height)
        .radius(RadiusTokens::value(RadiusStep::X4l))
        .border((1.0, t.colors.line.to_paint()))
        .fill(t.colors.surface.to_paint())
        .clip(true)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .justify(JustifyContent::CENTER)
                .flex_shrink(0.0)
                .padding_insets(Insets {
                    left: t.spacing.steps(2.0),
                    right: t.spacing.steps(2.0),
                    top: t.spacing.steps(0.5),
                    bottom: t.spacing.steps(0.5),
                }),
        )
        .child(text)
        .build()
}

const KEY: f32 = 22.0;
const BACK: f32 = 0.07 * KEY;
const LEFT: f32 = 0.16 * KEY;
const RIGHT: f32 = 0.05 * KEY;
const FRONT: f32 = 0.24 * KEY;
const BODY_RADIUS: f32 = 0.15 * KEY;
const FACE_RADIUS: f32 = 0.09 * KEY;
const FACE_HEIGHT: f32 = KEY - BACK - FRONT;
const LEGEND: f32 = 0.45 * KEY;
const MIN_WIDTH: f32 = 0.92 * KEY;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Finish {
    Porcelain,
    Graphite,
}

impl Finish {
    fn of(theme: &ArtisanTheme) -> Self {
        match theme.colors.mode.is_dark() {
            true => Self::Porcelain,
            false => Self::Graphite,
        }
    }

    const fn base(self) -> f32 {
        match self {
            Self::Porcelain => 96.0,
            Self::Graphite => 27.0,
        }
    }

    const fn contrast(self) -> f32 {
        match self {
            Self::Porcelain => 1.0,
            Self::Graphite => 0.55,
        }
    }

    fn shade(self, offset: f32) -> Oklch {
        gray(self.base() + offset * self.contrast())
    }

    const fn ink(self) -> f32 {
        match self {
            Self::Porcelain => 8.6,
            Self::Graphite => 95.0,
        }
    }

    const fn cast(self) -> f32 {
        match self {
            Self::Porcelain => 0.5,
            Self::Graphite => 0.3,
        }
    }

    const fn inner_dark(self) -> f32 {
        match self {
            Self::Porcelain => 0.17,
            Self::Graphite => 0.47,
        }
    }

    const fn inner_light(self) -> f32 {
        match self {
            Self::Porcelain => 0.6,
            Self::Graphite => 0.05,
        }
    }

    const fn rim(self) -> f32 {
        match self {
            Self::Porcelain => 0.9,
            Self::Graphite => 0.1,
        }
    }
}

fn gray(percent: f32) -> Oklch {
    let value = (percent / 100.0).clamp(0.0, 1.0);
    let linear = match value <= 0.04045 {
        true => value / 12.92,
        false => ((value + 0.055) / 1.055).powf(2.4),
    };
    Oklch::new(linear.cbrt(), 0.0, 0.0)
}

fn shadow(dx: f32, dy: f32, blur: f32, spread: f32, inset: bool, color: Oklch) -> GpuiShadowLayer {
    GpuiShadowLayer {
        dx,
        dy,
        blur,
        spread,
        inset,
        color: color.to_reference_paint(),
    }
}

fn gradient(angle: f32, from: Oklch, to: Oklch) -> GpuiBackground {
    GpuiBackground::LinearGradient {
        angle,
        color_space: GpuiGradientColorSpace::Oklab,
        stops: [
            GpuiGradientStop {
                percentage: 0.0,
                color: from.to_reference_paint(),
            },
            GpuiGradientStop {
                percentage: 1.0,
                color: to.to_reference_paint(),
            },
        ],
    }
}

fn to(x: f32, y: f32) -> PathCommand {
    PathCommand::Line(Point::new(x, y))
}

fn corner(x: f32, y: f32, sweep: bool) -> PathCommand {
    PathCommand::Arc {
        radii: Point::new(BODY_RADIUS, BODY_RADIUS),
        rotation_radians: 0.0,
        large: false,
        sweep,
        to: Point::new(x, y),
    }
}

fn facet(bounds: Rect, commands: Vec<PathCommand>, background: GpuiBackground) -> DisplayItem {
    DisplayItem::GpuiPath {
        transformed_window_logical_bounds: bounds,
        path: Arc::new(GpuiPath {
            commands_local: commands,
            origin_window_logical: bounds.origin(),
            element_transform: Transform2D::IDENTITY,
            style: GpuiPathStyle::Fill {
                rule: FillRule::NonZero,
            },
            background,
        }),
    }
}

fn skirts(finish: Finish) -> Element {
    Canvas::new()
        .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
        .on_paint(move |bounds| {
            let (w, h) = (bounds.w, bounds.h);
            let (face_left, face_right) = (LEFT, w - RIGHT);
            let (face_top, face_bottom) = (BACK, h - FRONT);
            let r = BODY_RADIUS;
            vec![
                facet(
                    bounds,
                    vec![
                        PathCommand::Move(Point::new(face_right, face_top)),
                        to(w - r, 0.0),
                        corner(w, r, true),
                        to(w, h - r),
                        to(face_right, face_bottom),
                        PathCommand::Close,
                    ],
                    GpuiBackground::Solid(finish.shade(-36.0).to_reference_paint()),
                ),
                facet(
                    bounds,
                    vec![
                        PathCommand::Move(Point::new(face_left, face_bottom)),
                        to(0.0, h - r),
                        corner(r, h, false),
                        to(w - r, h),
                        corner(w, h - r, false),
                        to(face_right, face_bottom),
                        PathCommand::Close,
                    ],
                    gradient(90.0, finish.shade(-18.0), finish.shade(-37.0)),
                ),
            ]
        })
        .build()
        .exclude_from_hit_testing()
}

fn keycap(theme: &ArtisanTheme, label: String) -> Element {
    let finish = Finish::of(theme);
    let black = Oklch::new(0.0, 0.0, 0.0);
    let white = Oklch::new(1.0, 0.0, 0.0);
    let legend = Text::new(label)
        .text_style(TextStyle {
            families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
            size_px: LEGEND.round(),
            line_height_px: FACE_HEIGHT,
            weight: FontWeight::MEDIUM,
            ..TextStyle::default()
        })
        .gpui_source_color(gray(finish.ink()).to_reference_text_color())
        .wrap(false)
        .build();
    let face = UiBox::new()
        .height(FACE_HEIGHT)
        .style(
            Style::new()
                .min_width(Length::Px(MIN_WIDTH - LEFT - RIGHT))
                .radius(Length::Px(FACE_RADIUS))
                .align(AlignItems::CENTER)
                .justify(JustifyContent::CENTER)
                .padding_insets(Insets::hv(FACE_HEIGHT * 0.25, 0.0))
                .gpui_background(gradient(135.0, finish.shade(-3.0), finish.shade(1.0)))
                .gpui_shadows(vec![
                    shadow(
                        0.0,
                        0.005 * KEY,
                        0.028 * KEY,
                        0.012 * KEY,
                        false,
                        white.with_alpha(finish.rim()),
                    ),
                    shadow(
                        0.03 * KEY,
                        0.04 * KEY,
                        0.13 * KEY,
                        0.0,
                        true,
                        black.with_alpha(finish.inner_dark()),
                    ),
                    shadow(
                        -0.021 * KEY,
                        -0.028 * KEY,
                        0.13 * KEY,
                        0.0,
                        true,
                        white.with_alpha(finish.inner_light()),
                    ),
                ]),
        )
        .child(legend)
        .build();
    let body = UiBox::new()
        .height(KEY)
        .radius(BODY_RADIUS)
        .fill(finish.shade(0.0).to_paint())
        .clip(true)
        .style(Style::new().padding_insets(Insets {
            left: LEFT,
            right: RIGHT,
            top: BACK,
            bottom: FRONT,
        }))
        .child(skirts(finish))
        .child(face)
        .build();
    UiBox::new()
        .height(KEY)
        .style(
            Style::new()
                .flex_shrink(0.0)
                .radius(Length::Px(BODY_RADIUS))
                .gpui_shadows(vec![
                    shadow(
                        0.0,
                        0.08 * KEY,
                        0.18 * KEY,
                        -0.04 * KEY,
                        false,
                        black.with_alpha(finish.cast() * 0.35 * 0.51),
                    ),
                    shadow(
                        0.06 * KEY,
                        0.28 * KEY,
                        0.52 * KEY,
                        -0.01 * KEY,
                        false,
                        black.with_alpha(finish.cast() * 0.6 * 0.35),
                    ),
                ]),
        )
        .child(body)
        .build()
}
