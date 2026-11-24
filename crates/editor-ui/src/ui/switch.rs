use aqua_core::{CornerRadii, DisplayItem, GpuiBackground, Insets, Rect, Role};
use aqua_ui::{
    element::{Button, Canvas, Element, Key},
    style::{Length, Style},
};

use super::kit::{Corner, Space};
use super::theme::{ArtisanTheme, Opacity};

const THUMB: f32 = 16.0;
const REACH: Space = Space::S8;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SwitchFinish {
    Standard,
    Recessed,
}

pub struct Switch {
    id: Key,
    theme: ArtisanTheme,
    checked: bool,
    label: String,
    disabled: bool,
    focus_visible: bool,
    finish: SwitchFinish,
    on_change: Option<Box<dyn Fn(bool)>>,
}

impl std::fmt::Debug for Switch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Switch").finish_non_exhaustive()
    }
}

impl Switch {
    #[expect(
        clippy::large_types_passed_by_value,
        reason = "the switch stores this theme copy and paints from it when built"
    )]
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
            finish: SwitchFinish::Standard,
            on_change: None,
        }
    }

    #[must_use]
    pub const fn recessed(mut self) -> Self {
        self.finish = SwitchFinish::Recessed;
        self
    }

    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    #[must_use]
    pub fn on_change(mut self, handler: impl Fn(bool) + 'static) -> Self {
        self.on_change = Some(Box::new(handler));
        self
    }

    fn track(&self) -> (f32, f32, Element) {
        let (width, height) = self.theme.density.switch_default;
        let (thumb, rim) = (THUMB, 1.0);
        let colors = &self.theme.colors;
        let track_color = if self.checked {
            colors.primary
        } else {
            colors.line_strong
        }
        .to_reference_paint();
        let thumb_background = GpuiBackground::Solid(
            if self.checked {
                colors.primary_text
            } else {
                colors.text
            }
            .to_reference_paint(),
        );
        let outer_radius = Corner::Full.resolve(height);
        let thumb_radius = Corner::Full.resolve(thumb);
        let checked = self.checked;
        let track = Canvas::new()
            .width(width)
            .height(height)
            .on_paint(move |bounds| {
                let track = DisplayItem::GpuiBackground {
                    bounds,
                    corners: CornerRadii::uniform(outer_radius),
                    background: GpuiBackground::Solid(track_color),
                };
                let thumb_bounds = Rect {
                    x: bounds.x
                        + rim
                        + if checked {
                            rim.mul_add(-2.0, width - thumb)
                        } else {
                            0.0
                        },
                    y: bounds.y + (height - thumb) / 2.0,
                    w: thumb,
                    h: thumb,
                };
                let knob = DisplayItem::GpuiBackground {
                    bounds: thumb_bounds,
                    corners: CornerRadii::uniform(thumb_radius),
                    background: thumb_background,
                };
                vec![track, knob]
            })
            .build();
        (width, height, track)
    }

    pub fn build(self) -> Element {
        let (width, height, track) = if self.finish == SwitchFinish::Recessed {
            let carve = super::carved::Carve::of(&self.theme);
            (
                super::carved::well::SWITCH_WIDTH,
                super::carved::well::SWITCH_HEIGHT,
                super::carved::well::switch_track(&carve, self.checked),
            )
        } else {
            self.track()
        };
        let checked = self.checked;
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
        let outer = 2.0f32.mul_add(reach, height);
        let mut root = Button::new()
            .width(2.0f32.mul_add(reach, width))
            .height(outer)
            .radius(if self.finish == SwitchFinish::Recessed {
                super::carved::RADIUS_MD + reach
            } else {
                Corner::Full.resolve(outer)
            })
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
