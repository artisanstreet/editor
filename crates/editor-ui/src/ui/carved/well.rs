use aqua_ui::element::{Box as UiBox, Element};
use aqua_ui::style::{Length, Style};

use super::face::Face;
use super::{Carve, RADIUS_MD};

pub const SWITCH_WIDTH: f32 = 62.0;
pub const SWITCH_HEIGHT: f32 = 36.0;
const THUMB: f32 = 30.0;
const THUMB_INSET: f32 = 3.0;
const TRAVEL: f32 = 26.0;

#[must_use]
pub fn sunken(carve: &Carve, radius: f32) -> Style {
    Style::new()
        .radius(Length::Px(radius))
        .background(carve.well.to_paint())
        .shadows(carve.inset())
}

#[must_use]
pub fn switch_track(carve: &Carve, checked: bool) -> Element {
    let radius = RADIUS_MD - THUMB_INSET;
    let (face, shadows) = if checked {
        let mut shadows = carve.rest();
        shadows.extend(carve.press());
        (Face::pill(radius, 0.36, 0.26), shadows)
    } else {
        (Face::thumb(carve, radius), carve.rest())
    };
    let left = THUMB_INSET + if checked { TRAVEL } else { 0.0 };
    let thumb = UiBox::new()
        .width(THUMB)
        .height(THUMB)
        .style(
            Style::new()
                .absolute_at(Some(left), Some(THUMB_INSET), None, None)
                .radius(Length::Px(radius))
                .shadows(shadows),
        )
        .child(face.layer())
        .build()
        .exclude_from_hit_testing();
    UiBox::new()
        .width(SWITCH_WIDTH)
        .height(SWITCH_HEIGHT)
        .style(sunken(carve, RADIUS_MD).flex_shrink(0.0))
        .child(thumb)
        .build()
}
