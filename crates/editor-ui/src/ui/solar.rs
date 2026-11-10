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
        name: "artisan.solar-horizon".into(),
        hlsl: include_str!("solar/horizon.hlsl").into(),
        entry: "solar_horizon".into(),
        reference: Some(ReferenceShader(sample::shade)),
    })
});

pub fn artwork(
    theme: &super::theme::ArtisanTheme,
    composer_height: Rc<Cell<Option<f32>>>,
    reduced_motion: bool,
) -> Element {
    let background = theme.colors.background.to_paint().to_gpu();
    let atmosphere = Oklch::new(0.55, 0.085, 242.0).to_gpu();
    let ember = Oklch::new(0.73, 0.17, 52.0).to_gpu();
    let red = Oklch::new(0.62, 0.20, 28.0).to_gpu();
    let light = Oklch::new(0.94, 0.065, 80.0).to_gpu();
    let canvas = Canvas::new()
        .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
        .on_paint(move |bounds| {
            let horizon = (bounds.h * 0.5 - composer_height.get().unwrap_or(88.0) * 0.5 - 42.0)
                .max(bounds.h * 0.22)
                / bounds.h.max(1.0);
            let mut parameters = Vec::with_capacity(24);
            parameters.extend(background);
            parameters.extend(atmosphere);
            parameters.extend(ember);
            parameters.extend(light);
            parameters.extend([horizon, crate::ui::chrome::card::RADIUS - 2.0, 0.0, 0.0]);
            parameters.extend(red);
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
        .with_key("new-thread-solar-horizon")
        .debug_selector("new-thread-solar-horizon")
        .exclude_from_hit_testing()
}
