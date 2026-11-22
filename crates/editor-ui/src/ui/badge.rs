use std::sync::Arc;

use super::theme::{ArtisanTheme, Oklch, RadiusStep, RadiusTokens, SpacingTokens};
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

pub(crate) struct Badge {
    theme: ArtisanTheme,
    label: String,
    variant: BadgeVariant,
}

impl Badge {
    #[expect(
        clippy::large_types_passed_by_value,
        reason = "the badge stores this theme copy and paints from it when built"
    )]
    pub(crate) fn new(theme: ArtisanTheme, label: impl Into<String>) -> Self {
        Self {
            theme,
            label: label.into(),
            variant: BadgeVariant::Pill,
        }
    }

    #[must_use]
    pub(crate) const fn variant(mut self, variant: BadgeVariant) -> Self {
        self.variant = variant;
        self
    }

    pub(crate) fn build(self) -> Element {
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
            line_height_px: SpacingTokens::steps(4.0),
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
                    left: SpacingTokens::steps(2.0),
                    right: SpacingTokens::steps(2.0),
                    top: SpacingTokens::steps(0.5),
                    bottom: SpacingTokens::steps(0.5),
                }),
        )
        .child(text)
        .build()
}

#[derive(Clone, Copy)]
struct Key(f32);

impl Key {
    fn of(theme: &ArtisanTheme) -> Self {
        Self(theme.typography.size(22.0))
    }
    const fn back(self) -> f32 {
        0.07 * self.0
    }
    const fn left(self) -> f32 {
        0.16 * self.0
    }
    const fn right(self) -> f32 {
        0.05 * self.0
    }
    const fn front(self) -> f32 {
        0.24 * self.0
    }
    const fn body_radius(self) -> f32 {
        0.15 * self.0
    }
    const fn face_radius(self) -> f32 {
        0.09 * self.0
    }
    const fn face_height(self) -> f32 {
        self.0 - self.back() - self.front()
    }
    const fn legend(self) -> f32 {
        0.45 * self.0
    }
    const fn min_width(self) -> f32 {
        0.92 * self.0
    }
}

const BASE: f32 = 27.0;
const CONTRAST: f32 = 0.55;
const INK: f32 = 95.0;
const CAST: f32 = 0.3;
const INNER_DARK: f32 = 0.47;
const INNER_LIGHT: f32 = 0.05;
const RIM: f32 = 0.1;

fn shade(offset: f32) -> Oklch {
    gray(offset.mul_add(CONTRAST, BASE))
}

fn gray(percent: f32) -> Oklch {
    let value = (percent / 100.0).clamp(0.0, 1.0);
    let linear = if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
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

fn corner(key: Key, x: f32, y: f32, sweep: bool) -> PathCommand {
    PathCommand::Arc {
        radii: Point::new(key.body_radius(), key.body_radius()),
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

fn skirts(key: Key) -> Element {
    Canvas::new()
        .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
        .on_paint(move |bounds| {
            let (w, h) = (bounds.w, bounds.h);
            let (face_left, face_right) = (key.left(), w - key.right());
            let (face_top, face_bottom) = (key.back(), h - key.front());
            let r = key.body_radius();
            vec![
                facet(
                    bounds,
                    vec![
                        PathCommand::Move(Point::new(face_right, face_top)),
                        to(w - r, 0.0),
                        corner(key, w, r, true),
                        to(w, h - r),
                        to(face_right, face_bottom),
                        PathCommand::Close,
                    ],
                    GpuiBackground::Solid(shade(-36.0).to_reference_paint()),
                ),
                facet(
                    bounds,
                    vec![
                        PathCommand::Move(Point::new(face_left, face_bottom)),
                        to(0.0, h - r),
                        corner(key, r, h, false),
                        to(w - r, h),
                        corner(key, w, h - r, false),
                        to(face_right, face_bottom),
                        PathCommand::Close,
                    ],
                    gradient(90.0, shade(-18.0), shade(-37.0)),
                ),
            ]
        })
        .build()
        .exclude_from_hit_testing()
}

fn keycap(theme: &ArtisanTheme, label: String) -> Element {
    let black = Oklch::new(0.0, 0.0, 0.0);
    let white = Oklch::new(1.0, 0.0, 0.0);
    let key = Key::of(theme);
    let legend = Text::new(label)
        .text_style(TextStyle {
            families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
            size_px: key.legend().round(),
            line_height_px: key.face_height(),
            weight: FontWeight::MEDIUM,
            ..TextStyle::default()
        })
        .gpui_source_color(gray(INK).to_reference_text_color())
        .wrap(false)
        .build();
    let face = UiBox::new()
        .height(key.face_height())
        .style(
            Style::new()
                .min_width(Length::Px(key.min_width() - key.left() - key.right()))
                .radius(Length::Px(key.face_radius()))
                .align(AlignItems::CENTER)
                .justify(JustifyContent::CENTER)
                .padding_insets(Insets::hv(key.face_height() * 0.25, 0.0))
                .gpui_background(gradient(135.0, shade(-3.0), shade(1.0)))
                .gpui_shadows(vec![
                    shadow(
                        0.0,
                        0.005 * key.0,
                        0.028 * key.0,
                        0.012 * key.0,
                        false,
                        white.with_alpha(RIM),
                    ),
                    shadow(
                        0.03 * key.0,
                        0.04 * key.0,
                        0.13 * key.0,
                        0.0,
                        true,
                        black.with_alpha(INNER_DARK),
                    ),
                    shadow(
                        -0.021 * key.0,
                        -0.028 * key.0,
                        0.13 * key.0,
                        0.0,
                        true,
                        white.with_alpha(INNER_LIGHT),
                    ),
                ]),
        )
        .child(legend)
        .build();
    let body = UiBox::new()
        .height(key.0)
        .radius(key.body_radius())
        .fill(shade(0.0).to_paint())
        .clip(true)
        .style(Style::new().padding_insets(Insets {
            left: key.left(),
            right: key.right(),
            top: key.back(),
            bottom: key.front(),
        }))
        .child(skirts(key))
        .child(face)
        .build();
    UiBox::new()
        .height(key.0)
        .style(
            Style::new()
                .flex_shrink(0.0)
                .radius(Length::Px(key.body_radius()))
                .gpui_shadows(vec![
                    shadow(
                        0.0,
                        0.08 * key.0,
                        0.18 * key.0,
                        -0.04 * key.0,
                        false,
                        black.with_alpha(CAST * 0.35 * 0.51),
                    ),
                    shadow(
                        0.06 * key.0,
                        0.28 * key.0,
                        0.52 * key.0,
                        -0.01 * key.0,
                        false,
                        black.with_alpha(CAST * 0.6 * 0.35),
                    ),
                ]),
        )
        .child(body)
        .build()
}
