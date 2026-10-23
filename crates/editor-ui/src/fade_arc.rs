//! A loading arc: one clock, two reusable tessellated paths, application motion policy.

use crate::{
    motion::{FADE_ARC_ROTATION, MotionPolicy, started_after},
    theme::{ArtisanTheme, Oklch, Opacity},
};
use gpui::{
    App, Bounds, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, Path,
    PathBuilder, Pixels, Point, RenderOnce, StatefulInteractiveElement as _, Styled as _, Window,
    canvas, div, linear_color_stop, linear_gradient, point, px, size,
};
use std::{sync::OnceLock, time::Duration};

#[derive(IntoElement)]
pub struct FadeArc {
    id: ElementId,
    size: Pixels,
    color: Oklch,
}
impl FadeArc {
    pub fn new(id: impl Into<ElementId>, theme: ArtisanTheme) -> Self {
        Self {
            id: id.into(),
            size: px(24.0),
            color: theme.colors.text_muted,
        }
    }
    #[must_use]
    pub fn size(mut self, size: Pixels) -> Self {
        let value = f32::from(size);
        self.size = px(if value.is_finite() {
            value.max(0.0)
        } else {
            0.0
        });
        self
    }
}
impl RenderOnce for FadeArc {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = self.color;
        // The rotation is paint-only: it repaints without rendering again.
        let started = (MotionPolicy::current(cx) == MotionPolicy::Full)
            .then(|| started_after(self.id.clone(), Duration::ZERO, window, cx))
            .flatten();
        let canvas = canvas(
            |bounds, _, _| bounds,
            move |_, bounds, window, cx| {
                let Some(started) = started else {
                    paint_arc(bounds, color, 0.0, window);
                    return;
                };
                window.paint_animated(
                    move |window, cx| {
                        let phase = (started.elapsed().as_secs_f32()
                            / FADE_ARC_ROTATION.as_secs_f32())
                        .fract();
                        paint_arc(bounds, color, phase, window);
                        !cx.reduce_motion()
                    },
                    cx,
                );
            },
        )
        .size_full();
        div()
            .id(self.id)
            .role(gpui::Role::ProgressIndicator)
            .aria_label("Loading")
            .size(self.size)
            .child(canvas)
    }
}

/// Paints the arc turned `phase` of a full rotation.
fn paint_arc(bounds: Bounds<Pixels>, color: Oklch, phase: f32, window: &mut Window) {
    let rotation = phase * std::f32::consts::TAU;
    let (sin, cos) = rotation.sin_cos();
    let edge = f32::from(bounds.size.width.min(bounds.size.height));
    let center = bounds.center();
    for (index, template) in unit_paths().iter().enumerate() {
        let Some(template) = template else {
            continue;
        };
        let mut path = template.clone();
        let mut min = point(px(f32::INFINITY), px(f32::INFINITY));
        let mut max = point(px(f32::NEG_INFINITY), px(f32::NEG_INFINITY));
        for vertex in &mut path.vertices {
            let x = f32::from(vertex.xy_position.x) - 12.0;
            let y = f32::from(vertex.xy_position.y) - 12.0;
            vertex.xy_position = center
                + point(
                    px((x * cos - y * sin) * edge / 24.0),
                    px((x * sin + y * cos) * edge / 24.0),
                );
            min.x = min.x.min(vertex.xy_position.x);
            min.y = min.y.min(vertex.xy_position.y);
            max.x = max.x.max(vertex.xy_position.x);
            max.y = max.y.max(vertex.xy_position.y);
        }
        path.bounds = Bounds::new(min, size(max.x - min.x, max.y - min.y));
        window.paint_path(
            path,
            linear_gradient(
                0.0,
                // The spinning trail fades its one token toward clear.
                linear_color_stop(
                    if index == 0 {
                        color
                    } else {
                        color.at(Opacity::Clear)
                    }
                    .to_paint(),
                    0.0,
                ),
                linear_color_stop(color.fade(0.55).to_paint(), 1.0),
            ),
        );
    }
}

fn unit_paths() -> &'static [Option<Path<Pixels>>; 2] {
    static PATHS: OnceLock<[Option<Path<Pixels>>; 2]> = OnceLock::new();
    PATHS.get_or_init(|| {
        [(-90.0f32, 120.0f32, true), (120.0, 270.0, false)].map(|(start, end, large)| {
            let on_circle = |degrees: f32| -> Point<Pixels> {
                let (sin, cos) = degrees.to_radians().sin_cos();
                point(
                    px(10.5f32.mul_add(cos, 12.0)),
                    px(10.5f32.mul_add(sin, 12.0)),
                )
            };
            let mut path = PathBuilder::stroke(px(3.0));
            path.move_to(on_circle(start));
            path.arc_to(
                point(px(10.5), px(10.5)),
                px(0.0),
                large,
                true,
                on_circle(end),
            );
            path.build().ok()
        })
    })
}
