use std::sync::Arc;

use aqua_core::{DisplayItem, Rect};
use aqua_ui::element::{Canvas, Element};
use aqua_ui::style::Style;

use super::Carve;
use crate::ui::chrome::card::hex;
use crate::ui::theme::Oklch;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stop {
    pub at: f32,
    pub color: Oklch,
}

#[must_use]
pub const fn stop(at: f32, color: Oklch) -> Stop {
    Stop { at, color }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Fill {
    Solid(Oklch),
    Radial {
        rx: f32,
        ry: f32,
        cx: f32,
        cy: f32,
        stops: Vec<Stop>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Line {
    pub dx: f32,
    pub dy: f32,
    pub color: Oklch,
}

#[must_use]
pub const fn line(dx: f32, dy: f32, color: Oklch) -> Line {
    Line { dx, dy, color }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub fill: Fill,
    pub border: Option<Oklch>,
    pub radius: f32,
    pub lines: Vec<Line>,
}

#[must_use]
pub fn rgb(red: u8, green: u8, blue: u8) -> Oklch {
    let [lightness, chroma, hue, _] = color::AlphaColor::<color::Srgb>::new([
        f32::from(red) / 255.0,
        f32::from(green) / 255.0,
        f32::from(blue) / 255.0,
        1.0,
    ])
    .convert::<color::Oklch>()
    .components;
    Oklch::new(lightness, chroma, if hue.is_nan() { 0.0 } else { hue })
}

#[must_use]
pub fn horizon() -> Vec<Stop> {
    vec![
        stop(0.0, rgb(0xc6, 0xdb, 0xf0)),
        stop(0.25, rgb(0x6f, 0x9b, 0xcb)),
        stop(0.58, rgb(0x33, 0x59, 0x9a)),
        stop(0.92, rgb(0x1d, 0x32, 0x70)),
    ]
}

#[must_use]
pub fn pill() -> Vec<Stop> {
    vec![
        stop(0.0, rgb(0xc6, 0xdb, 0xf0)),
        stop(0.26, rgb(0x6f, 0x9b, 0xcb)),
        stop(0.60, rgb(0x39, 0x60, 0xa4)),
        stop(1.0, rgb(0x26, 0x42, 0x82)),
    ]
}

#[must_use]
pub fn quota() -> Vec<Stop> {
    vec![
        stop(0.0, rgb(0xc6, 0xdb, 0xf0)),
        stop(0.28, rgb(0x74, 0xa0, 0xd1)),
        stop(0.64, rgb(0x3c, 0x65, 0xaa)),
        stop(1.0, rgb(0x2b, 0x4a, 0x8e)),
    ]
}

#[must_use]
pub fn ember() -> Vec<Stop> {
    vec![
        stop(0.0, Oklch::new(0.86, 0.07, 20.0)),
        stop(0.25, Oklch::new(0.68, 0.16, 22.0)),
        stop(0.58, Oklch::new(0.52, 0.18, 25.0)),
        stop(0.92, Oklch::new(0.38, 0.14, 26.0)),
    ]
}

#[must_use]
pub fn navy_edge() -> Oklch {
    rgb(0x0b, 0x16, 0x40)
}

#[must_use]
pub fn sky_glint(alpha: f32) -> Oklch {
    rgb(220, 236, 255).with_alpha(alpha)
}

const EMBER_EDGE: Oklch = Oklch::new(0.25, 0.09, 25.0);

impl Face {
    #[must_use]
    pub fn horizon(radius: f32) -> Self {
        Self {
            fill: Fill::Radial {
                rx: 1.3,
                ry: 1.4,
                cx: 0.5,
                cy: 1.25,
                stops: horizon(),
            },
            border: Some(navy_edge()),
            radius,
            lines: vec![
                line(0.0, 1.0, sky_glint(0.28)),
                line(0.0, -1.0, super::black(0.22)),
            ],
        }
    }

    #[must_use]
    pub fn pill(radius: f32, glint: f32, shade: f32) -> Self {
        Self {
            fill: Fill::Radial {
                rx: 1.4,
                ry: 1.35,
                cx: 0.5,
                cy: 1.2,
                stops: pill(),
            },
            border: Some(navy_edge()),
            radius,
            lines: vec![
                line(0.0, 1.0, sky_glint(glint)),
                line(0.0, -1.0, super::black(shade)),
            ],
        }
    }

    #[must_use]
    pub fn ember(radius: f32) -> Self {
        Self {
            fill: Fill::Radial {
                rx: 1.3,
                ry: 1.4,
                cx: 0.5,
                cy: 1.25,
                stops: ember(),
            },
            border: Some(EMBER_EDGE),
            radius,
            lines: vec![
                line(0.0, 1.0, Oklch::new(0.95, 0.03, 20.0).with_alpha(0.28)),
                line(0.0, -1.0, super::black(0.22)),
            ],
        }
    }

    #[must_use]
    pub fn secondary(carve: &Carve, radius: f32, hovered: bool) -> Self {
        let (center, outer, reach) = if hovered {
            (carve.toward_ink(carve.bg, 0.11), carve.bg_raised, 0.75)
        } else {
            (carve.toward_ink(carve.bg, 0.07), carve.bg, 0.70)
        };
        Self {
            fill: Fill::Radial {
                rx: 0.7,
                ry: 1.1,
                cx: 0.5,
                cy: 1.15,
                stops: vec![stop(0.0, center), stop(reach, outer)],
            },
            border: Some(carve.toward_ink(carve.bg, 0.08)),
            radius,
            lines: vec![
                line(0.0, 1.0, carve.toward_ink(carve.bg, 0.12)),
                line(0.0, -1.0, super::mix(carve.bg, super::black(1.0), 0.25)),
            ],
        }
    }

    #[must_use]
    pub fn thumb(carve: &Carve, radius: f32) -> Self {
        if carve.dark {
            Self {
                fill: Fill::Radial {
                    rx: 0.7,
                    ry: 1.1,
                    cx: 0.5,
                    cy: 1.15,
                    stops: vec![
                        stop(0.0, carve.toward_ink(carve.bg, 0.14)),
                        stop(0.7, carve.toward_ink(carve.bg, 0.04)),
                    ],
                },
                border: Some(carve.toward_ink(carve.bg, 0.10)),
                radius,
                lines: vec![
                    line(0.0, 1.0, carve.toward_ink(carve.bg, 0.18)),
                    line(0.0, -1.0, super::black(0.35)),
                ],
            }
        } else {
            Self {
                fill: Fill::Radial {
                    rx: 100.0,
                    ry: 1.0,
                    cx: 0.5,
                    cy: 0.0,
                    stops: vec![
                        stop(0.0, super::white(1.0)),
                        stop(1.0, Oklch::new(0.95, 0.0, 0.0)),
                    ],
                },
                border: Some(super::black(0.12)),
                radius,
                lines: vec![
                    line(0.0, 1.0, super::white(1.0)),
                    line(0.0, -1.0, super::black(0.08)),
                ],
            }
        }
    }

    #[must_use]
    pub fn avatar(radius: f32) -> Self {
        Self {
            fill: Fill::Radial {
                rx: 1.3,
                ry: 1.4,
                cx: 0.5,
                cy: 1.25,
                stops: horizon(),
            },
            border: None,
            radius,
            lines: vec![
                line(0.0, 1.0, super::white(0.32)),
                line(1.0, 0.0, super::white(0.14)),
                line(-1.0, 0.0, super::white(0.14)),
                line(0.0, -1.0, super::black(0.45)),
                line(0.0, 2.0, super::white(0.1)),
                line(2.0, 0.0, super::white(0.06)),
                line(-2.0, 0.0, super::white(0.06)),
                line(0.0, -2.0, super::black(0.2)),
            ],
        }
    }

    #[must_use]
    pub fn svg(&self, w: f32, h: f32) -> String {
        let w = w.max(1.0);
        let h = h.max(1.0);
        let r = self.radius.min(w / 2.0).min(h / 2.0);
        let edge = if self.border.is_some() { 1.0 } else { 0.0 };
        let (iw, ih) = (
            2.0f32.mul_add(-edge, w).max(0.0),
            2.0f32.mul_add(-edge, h).max(0.0),
        );
        let ir = (r - edge).max(0.0);
        let (gradient, paint) = match &self.fill {
            Fill::Solid(color) => (String::new(), paint_of(*color)),
            Fill::Radial {
                rx,
                ry,
                cx,
                cy,
                stops,
            } => {
                let (cx, cy) = (iw.mul_add(*cx, edge), ih.mul_add(*cy, edge));
                let (rx, ry) = (iw * rx, ih * ry);
                let stops = stops
                    .iter()
                    .map(|stop| {
                        format!(
                            r#"<stop offset="{}" stop-color="{}" stop-opacity="{}"/>"#,
                            stop.at,
                            hex(stop.color),
                            stop.color.to_paint().alpha
                        )
                    })
                    .collect::<Vec<_>>()
                    .concat();
                (
                    format!(
                        r#"<radialGradient id="f" gradientUnits="userSpaceOnUse" cx="{cx}" cy="{cy}" r="{rx}" gradientTransform="translate({cx} {cy}) scale(1 {s}) translate(-{cx} -{cy})">{stops}</radialGradient>"#,
                        s = ry / rx.max(0.001),
                    ),
                    r#"fill="url(#f)""#.to_owned(),
                )
            }
        };
        let inner = rounded(edge, edge, iw, ih, ir);
        let outer = rounded(0.0, 0.0, w, h, r);
        let border = self.border.map_or_else(String::new, |border| {
            format!(
                r#"<path fill-rule="evenodd" d="{outer} {inner}" {}/>"#,
                paint_of(border)
            )
        });
        let lines = self
            .lines
            .iter()
            .map(|line| {
                format!(
                    r#"<path clip-path="url(#c)" fill-rule="evenodd" d="{inner} {}" {}/>"#,
                    rounded(edge + line.dx, edge + line.dy, iw, ih, ir),
                    paint_of(line.color)
                )
            })
            .collect::<Vec<_>>()
            .concat();
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}"><defs>{gradient}<clipPath id="c"><path d="{inner}"/></clipPath></defs><path d="{outer}" {paint}/>{border}{lines}</svg>"#
        )
    }

    #[must_use]
    pub fn item(&self, bounds: Rect) -> DisplayItem {
        DisplayItem::GpuiSvg {
            bounds,
            source: Arc::<str>::from(self.svg(bounds.w, bounds.h)),
            tint: None,
            opacity: 1.0,
        }
    }

    #[must_use]
    pub fn layer(self) -> Element {
        Canvas::new()
            .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
            .scroll_stable()
            .on_paint(move |bounds| vec![self.item(bounds)])
            .build()
            .exclude_from_hit_testing()
    }
}

fn paint_of(color: Oklch) -> String {
    format!(
        r#"fill="{}" fill-opacity="{}""#,
        hex(color),
        color.to_paint().alpha
    )
}

#[must_use]
pub fn rounded(left: f32, top: f32, width: f32, height: f32, radius: f32) -> String {
    let r = radius.min(width / 2.0).min(height / 2.0).max(0.0);
    let (right, bottom) = (left + width, top + height);
    format!(
        "M{l} {top}H{rr}A{r} {r} 0 0 1 {right} {tr}V{br}A{r} {r} 0 0 1 {rr} {bottom}H{l}A{r} {r} 0 0 1 {left} {br}V{tr}A{r} {r} 0 0 1 {l} {top}Z",
        l = left + r,
        rr = right - r,
        tr = top + r,
        br = bottom - r,
    )
}

#[must_use]
pub fn outline(carve: &Carve, w: f32, h: f32, radius: f32) -> String {
    let (rim_top, rim_bottom, glint_top, glint_bottom) = if carve.dark {
        (
            super::black(0.52),
            super::black(0.72),
            super::white(0.08),
            super::white(0.015),
        )
    } else {
        let shade = Oklch::new(0.21, 0.034, 264.665);
        (
            shade.with_alpha(0.10),
            shade.with_alpha(0.16),
            super::white(0.92),
            super::white(0.35),
        )
    };
    let gradient = |id: &str, top: Oklch, bottom: Oklch| {
        format!(
            r#"<linearGradient id="{id}" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{}" stop-opacity="{}"/><stop offset="1" stop-color="{}" stop-opacity="{}"/></linearGradient>"#,
            hex(top),
            top.to_paint().alpha,
            hex(bottom),
            bottom.to_paint().alpha
        )
    };
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}"><defs>{}{}</defs><rect x="0.5" y="0.5" width="{}" height="{}" rx="{}" fill="none" stroke="url(#r)" stroke-width="1"/><rect x="1.5" y="1.5" width="{}" height="{}" rx="{}" fill="none" stroke="url(#g)" stroke-width="1"/></svg>"#,
        gradient("r", rim_top, rim_bottom),
        gradient("g", glint_top, glint_bottom),
        (w - 1.0).max(0.0),
        (h - 1.0).max(0.0),
        (radius - 0.5).max(0.0),
        (w - 3.0).max(0.0),
        (h - 3.0).max(0.0),
        (radius - 1.5).max(0.0),
    )
}

#[must_use]
pub fn outline_layer(carve: &Carve, radius: f32) -> Element {
    let carve = *carve;
    Canvas::new()
        .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
        .scroll_stable()
        .on_paint(move |bounds| {
            vec![DisplayItem::GpuiSvg {
                bounds,
                source: Arc::<str>::from(outline(&carve, bounds.w, bounds.h, radius)),
                tint: None,
                opacity: 1.0,
            }]
        })
        .build()
        .exclude_from_hit_testing()
}

#[must_use]
pub fn bevel(carve: &Carve) -> Element {
    let carve = *carve;
    Canvas::new()
        .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
        .scroll_stable()
        .on_paint(move |bounds| {
            let band = |y: f32, color: Oklch| DisplayItem::GpuiBackground {
                bounds: Rect {
                    x: bounds.x + 1.0,
                    y,
                    w: (bounds.w - 2.0).max(0.0),
                    h: 1.0,
                },
                corners: aqua_core::CornerRadii::uniform(0.0),
                background: aqua_core::GpuiBackground::Solid(color.to_reference_paint()),
            };
            vec![
                band(bounds.y - 1.0, carve.rule_shade),
                band(bounds.y, carve.rule_light),
            ]
        })
        .build()
        .exclude_from_hit_testing()
}
