use aqua_core::{CornerRadii, DisplayItem, GpuiBackground, Insets, Rect, Role};
use aqua_ui::{
    element::{Button, Canvas, Element, Key},
    style::{Length, Style},
};

use super::kit::{Corner, Space};
use super::theme::{ArtisanTheme, Opacity};

const THUMB: f32 = 16.0;
const REACH: Space = Space::S8;

pub struct Switch {
    id: Key,
    theme: ArtisanTheme,
    checked: bool,
    label: String,
    disabled: bool,
    focus_visible: bool,
    on_change: Option<Box<dyn Fn(bool)>>,
}

impl Switch {
    pub fn new(
        id: impl Into<Key>,
        theme: ArtisanTheme,
        checked: bool,
        label: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            theme,
            checked,
            label: label.into(),
            disabled: false,
            focus_visible: false,
            on_change: None,
        }
    }

    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub const fn focus_visible(mut self, focus_visible: bool) -> Self {
        self.focus_visible = focus_visible;
        self
    }

    pub fn on_change(mut self, handler: impl Fn(bool) + 'static) -> Self {
        self.on_change = Some(Box::new(handler));
        self
    }

    pub fn build(self) -> Element {
        let (width, height) = self.theme.density.switch_default;
        let thumb = THUMB;
        let track_color = if self.checked {
            self.theme.colors.primary
        } else {
            self.theme.colors.line_strong
        }
        .to_reference_paint();
        let thumb_color = if self.checked {
            self.theme.colors.primary_text
        } else {
            self.theme.colors.text
        }
        .to_reference_paint();
        let checked = self.checked;
        let track = Canvas::new()
            .width(width)
            .height(height)
            .on_paint(move |bounds| {
                let track = DisplayItem::GpuiBackground {
                    bounds,
                    corners: CornerRadii::uniform(Corner::Full.resolve(height)),
                    background: GpuiBackground::Solid(track_color),
                };
                let thumb_bounds = Rect {
                    x: bounds.x + 1.0 + if checked { width - thumb - 2.0 } else { 0.0 },
                    y: bounds.y + (height - thumb) / 2.0,
                    w: thumb,
                    h: thumb,
                };
                let knob = DisplayItem::GpuiBackground {
                    bounds: thumb_bounds,
                    corners: CornerRadii::uniform(Corner::Full.resolve(thumb)),
                    background: GpuiBackground::Solid(thumb_color),
                };
                vec![track, knob]
            })
            .build();
        let reach = REACH.px();
        let mut style = Style::new()
            .flex_shrink(0.0)
            .padding(Length::Px(reach))
            .margin_insets(Insets::uniform(-reach));
        let focus_style = Style::new().gpui_shadows(vec![aqua_ui::style::GpuiShadowLayer {
            dx: 0.0,
            dy: 0.0,
            blur: 0.0,
            spread: self.theme.interaction.focus_ring_width,
            inset: false,
            color: self
                .theme
                .colors
                .ring
                .at(Opacity::FocusRing)
                .to_reference_paint(),
        }]);
        if self.focus_visible && !self.disabled {
            style.merge(&focus_style);
        }
        let outer = height + 2.0 * reach;
        let mut root = Button::new()
            .width(width + 2.0 * reach)
            .height(outer)
            .radius(Corner::Full.resolve(outer))
            .style(style)
            .child(track);
        if self.disabled {
            root = root.opacity(Opacity::Disabled.alpha());
        }
        if let Some(handler) = self.on_change {
            root = root.on_activate(move || handler(!checked));
        }
        let element = root
            .build()
            .with_key(self.id)
            .role(Role::Switch)
            .aria_label(self.label)
            .aria_checked(checked)
            .focus_visible_style(focus_style)
            .disabled(self.disabled);
        if self.disabled {
            element
        } else {
            element.cursor(aqua_core::CursorIcon::Pointer)
        }
    }
}
