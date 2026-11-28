use aqua_ui::element::{Box as UiBox, Element};
use aqua_ui::style::{Length, Style};
use aqua_ui::{AlignItems, JustifyContent};

use super::face::Face;
use super::{Carve, Type};
use crate::ui::chrome::account::{Account, portrait};
use crate::ui::theme::ArtisanTheme;

#[derive(Clone, Copy, Debug)]
pub struct Avatar {
    pub edge: f32,
    pub radius: f32,
    pub initial: Type,
    pub ring: bool,
}

#[must_use]
pub fn avatar(
    theme: &ArtisanTheme,
    account: Option<&Account>,
    name: &str,
    shape: Avatar,
) -> Element {
    if let Some(account) = account.filter(|account| account.avatar.is_some()) {
        return portrait(account, shape.edge, shape.radius, shape.edge * 0.6);
    }
    let initial: String = name.chars().take(1).flat_map(char::to_uppercase).collect();
    let mut style = Style::new()
        .align(AlignItems::CENTER)
        .justify(JustifyContent::CENTER)
        .radius(Length::Px(shape.radius))
        .flex_shrink(0.0);
    if shape.ring {
        style = style.shadows(vec![super::drop_shadow(
            0.0,
            0.0,
            1.0,
            Carve::of(theme).ink.with_alpha(0.12),
        )]);
    }
    UiBox::new()
        .width(shape.edge)
        .height(shape.edge)
        .clip(true)
        .style(style)
        .child(Face::avatar(shape.radius).layer())
        .child(
            shape
                .initial
                .text(theme, super::white(1.0), initial)
                .wrap(false)
                .build(),
        )
        .build()
}
