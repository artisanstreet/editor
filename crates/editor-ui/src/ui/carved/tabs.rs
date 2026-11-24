use aqua_core::Role;
use aqua_ui::element::{Button, Element};
use aqua_ui::style::{Length, Style};
use aqua_ui::{AlignItems, JustifyContent};

use super::face::Face;
use super::glyph::{Icon, glyph};
use super::{Carve, RADIUS_MD, Type};
use crate::ui::theme::ArtisanTheme;

const HEIGHT: f32 = 30.0;
const MIN_WIDTH: f32 = 72.0;
const ICON_WIDTH: f32 = 36.0;
const PAD_X: f32 = 12.0;
const LABEL: Type = Type::sans(13.0, 20.0, 500);

#[must_use]
pub fn well(carve: &Carve) -> Style {
    Style::new()
        .padding(Length::Px(3.0))
        .radius(Length::Px(RADIUS_MD))
        .background(carve.well.to_paint())
        .shadows(carve.inset())
}

#[derive(Debug)]
pub struct Tab {
    pub index: usize,
    pub label: String,
    pub glyph: Option<Icon>,
    pub chosen: bool,
    pub disabled: bool,
}

#[must_use]
pub fn tab(
    theme: &ArtisanTheme,
    carve: &Carve,
    tab: Tab,
    on_activate: Option<Box<dyn Fn()>>,
) -> Element {
    let ink = if tab.chosen {
        super::white(1.0)
    } else {
        carve.ink_dim
    };
    let content = tab.glyph.map_or_else(
        || {
            LABEL
                .text(theme, ink, tab.label.clone())
                .wrap(false)
                .build()
        },
        |icon| glyph(icon, 16.0, ink),
    );
    let width = if tab.glyph.is_some() {
        Style::new().width(Length::Px(ICON_WIDTH))
    } else {
        Style::new()
            .min_width(Length::Px(MIN_WIDTH))
            .padding_insets(aqua_core::Insets {
                left: PAD_X,
                right: PAD_X,
                top: 0.0,
                bottom: 0.0,
            })
    };
    let radius = RADIUS_MD - 3.0;
    let mut style = Style::new()
        .align(AlignItems::CENTER)
        .justify(JustifyContent::CENTER)
        .radius(Length::Px(radius));
    style.merge(&width);
    let mut button = Button::new().height(HEIGHT).style(style);
    if tab.chosen {
        let mut shadows = carve.rest();
        shadows.extend(carve.press());
        button = button
            .style(Style::new().shadows(shadows))
            .child(Face::pill(radius, 0.34, 0.22).layer());
    }
    button = button.child(content);
    if tab.disabled {
        button = button.opacity(0.5);
    }
    if let Some(handler) = on_activate {
        button = button.on_activate(handler);
    }
    let mut item = button
        .build()
        .with_key(tab.index)
        .role(Role::RadioButton)
        .aria_label(tab.label)
        .aria_checked(tab.chosen)
        .focusable(false)
        .disabled(tab.disabled);
    if tab.chosen {
        item = item.aria_active_descendant();
    }
    if !tab.disabled {
        item = item.cursor(aqua_core::CursorIcon::Pointer);
    }
    item
}
