pub mod caption;
pub mod hit_area;
pub mod icon_button;
pub mod key;
pub mod link;
pub mod trigger;

use crate::ui::theme::{ArtisanTheme, Opacity};
use aqua::ShadowLayer;
use aqua_core::{Role, Transform2D};
use aqua_ui::element::{Button, Element, Key};
use aqua_ui::style::Style;
use aqua_ui::tree::ActionEvent;

#[must_use]
pub(crate) fn focus_ring(theme: &ArtisanTheme) -> Style {
    Style::new()
        .border_of(1.0, theme.colors.ring.to_paint())
        .shadows(vec![ShadowLayer {
            dx: 0.0,
            dy: 0.0,
            blur: 0.0,
            spread: theme.interaction.focus_ring_width,
            inset: false,
            color: theme.colors.ring.at(Opacity::FocusRing).to_paint(),
        }])
}

pub(crate) struct Activation {
    pub key: Key,
    pub label: String,
    pub disabled: bool,
    pub on_activate: Option<Box<dyn Fn(ActionEvent)>>,
    pub debug_selector: Option<String>,
}

pub(crate) fn activatable(
    mut button: Button,
    activation: Activation,
    enabled: impl FnOnce(Element) -> Element,
) -> Element {
    if activation.disabled {
        button = button.opacity(Opacity::Disabled.alpha());
    } else if let Some(handler) = activation.on_activate {
        button = button.on_activate_event(handler);
    }
    let element = button
        .build()
        .with_key(activation.key)
        .role(Role::Button)
        .aria_label(activation.label)
        .gpui_keyboard_activation()
        .disabled(activation.disabled)
        .focusable(!activation.disabled);
    let element = match activation.debug_selector {
        Some(selector) => element.debug_selector(selector),
        None => element,
    };
    if activation.disabled {
        return element.aria_description("Unavailable");
    }
    enabled(element)
}

pub(crate) fn pressable(element: Element, theme: &ArtisanTheme, travel: f32) -> Element {
    if theme.reduced_motion() {
        element
    } else {
        element.pressed_style(Style::new().transform(Transform2D::translate(0.0, travel)))
    }
}
