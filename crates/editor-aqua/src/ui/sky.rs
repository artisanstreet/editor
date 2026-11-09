mod sample;

use aqua_core::shader::{FragmentShader, ReferenceShader};
use aqua_core::{DisplayItem, Oklch};
use aqua_ui::element::{Box as UiBox, Canvas, Element};
use aqua_ui::style::Style;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, LazyLock};

static PROGRAM: LazyLock<Arc<FragmentShader>> = LazyLock::new(|| {
    Arc::new(FragmentShader {
        name: "artisan.winter-sky".into(),
        hlsl: include_str!("sky/sky.hlsl").into(),
        entry: "winter_sky".into(),
        reference: Some(ReferenceShader(sample::shade)),
    })
});

pub fn artwork(
    theme: &super::theme::ArtisanTheme,
    composer_height: Rc<Cell<Option<f32>>>,
    reduced_motion: bool,
) -> Element {
    let background = theme.colors.background.to_paint().to_gpu();
    let palette = [
        Oklch::from_srgb8(7, 11, 36, 1.0),
        Oklch::from_srgb8(16, 26, 74, 1.0),
        Oklch::from_srgb8(31, 58, 122, 1.0),
        Oklch::from_srgb8(78, 122, 176, 1.0),
        Oklch::from_srgb8(168, 195, 220, 1.0),
    ]
    .map(|color| {
        let angle = color.h.to_radians();
        [color.l, color.c * angle.cos(), color.c * angle.sin(), 1.0]
    });
    let mut sunlight = Oklch::new(0.97, 0.012, 220.0).to_gpu();
    sunlight[3] = crate::frame::card::RADIUS - 2.0;
    let canvas = Canvas::new()
        .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
        .on_paint(move |bounds| {
            let mut parameters = Vec::with_capacity(32);
            parameters.extend(background);
            for color in palette {
                parameters.extend(color);
            }
            parameters.extend(sunlight);
            let horizon = (bounds.h * 0.5 - composer_height.get().unwrap_or(88.0) * 0.5 - 42.0)
                .max(bounds.h * 0.22)
                / bounds.h.max(1.0);
            parameters.extend([horizon, 0.0, 0.0, 0.0]);
            vec![DisplayItem::Shader {
                bounds,
                program: Arc::clone(&PROGRAM),
                parameters: parameters.into(),
                animated: !reduced_motion,
            }]
        })
        .build();
    UiBox::new()
        .style(Style::new().absolute_at(Some(2.0), Some(2.0), Some(2.0), Some(2.0)))
        .child(canvas)
        .build()
        .with_key("new-thread-winter-sky")
        .exclude_from_hit_testing()
}
