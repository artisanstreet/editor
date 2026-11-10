use super::{
    asset::source_asset_glyph,
    hover::hover_fill_stops,
    motion::MotionPolicy,
    theme::{ArtisanTheme, Opacity, RadiusTokens},
};
use aqua_core::{CursorIcon, Insets, Oklch, Role};
use aqua_text::{FontFamily, FontWeight, TextStyle};
use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Button as UiButton, Canvas, Element, Key, Text},
    style::{Length, Style},
    tree::ActionEvent,
};
use editor_assets::AssetId;
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ButtonVariant {
    #[default]
    Default,
    Outline,
    Secondary,
    Ghost,
    Accent,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ButtonShape {
    #[default]
    Pill,
    Rounded,
    Square,
}
impl ButtonShape {
    pub const fn radius(self) -> f32 {
        match self {
            Self::Pill => 16.0,
            Self::Rounded => 10.0,
            Self::Square => 4.0,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LabelOverflow {
    Start,
    End,
}
enum Content {
    Text(String),
    Icon(AssetId, String),
    IconText(AssetId, String),
}
impl Content {
    fn label(&self) -> &str {
        match self {
            Self::Text(label) | Self::Icon(_, label) | Self::IconText(_, label) => label,
        }
    }
}
pub fn accent_background(theme: &ArtisanTheme, hovered: bool) -> aqua::GpuiBackground {
    let (top, bottom) = if hovered {
        (
            theme.colors.action_hover_top,
            theme.colors.action_hover_bottom,
        )
    } else {
        (theme.colors.action_top, theme.colors.action_bottom)
    };
    aqua::GpuiBackground::LinearGradient {
        angle: 180.0,
        color_space: aqua::GpuiGradientColorSpace::Oklab,
        stops: [
            aqua::GpuiGradientStop {
                percentage: 0.0,
                color: top.to_reference_paint(),
            },
            aqua::GpuiGradientStop {
                percentage: 1.0,
                color: bottom.to_reference_paint(),
            },
        ],
    }
}
pub struct Button {
    id: Key,
    theme: ArtisanTheme,
    content: Content,
    variant: ButtonVariant,
    shape: ButtonShape,
    disabled: bool,
    hover_fill: bool,
    muted: bool,
    radius: Option<f32>,
    glass: bool,
    size: Option<f32>,
    label_overflow: Option<LabelOverflow>,
    icon_slot: Option<Element>,
    on_activate: Option<Box<dyn Fn(ActionEvent)>>,
    debug_selector: Option<String>,
    motion_policy: MotionPolicy,
}
impl Button {
    fn with_content(id: impl Into<Key>, theme: ArtisanTheme, content: Content) -> Self {
        Self {
            id: id.into(),
            theme,
            content,
            variant: ButtonVariant::Default,
            shape: ButtonShape::Pill,
            disabled: false,
            hover_fill: false,
            muted: false,
            radius: None,
            glass: false,
            size: None,
            label_overflow: None,
            icon_slot: None,
            on_activate: None,
            debug_selector: None,
            motion_policy: MotionPolicy::Full,
        }
    }
    pub fn text(id: impl Into<Key>, theme: ArtisanTheme, label: impl Into<String>) -> Self {
        Self::with_content(id, theme, Content::Text(label.into()))
    }
    pub fn icon(
        id: impl Into<Key>,
        theme: ArtisanTheme,
        icon: AssetId,
        label: impl Into<String>,
    ) -> Self {
        Self::with_content(id, theme, Content::Icon(icon, label.into()))
    }
    pub fn icon_text(
        id: impl Into<Key>,
        theme: ArtisanTheme,
        icon: AssetId,
        label: impl Into<String>,
    ) -> Self {
        Self::with_content(id, theme, Content::IconText(icon, label.into()))
    }
    pub const CORNER_INSET: f32 = 2.0;
    pub fn corner_close(
        id: impl Into<Key>,
        theme: ArtisanTheme,
        label: impl Into<String>,
        surface_radius: f32,
    ) -> Self {
        Self::icon(id, theme, AssetId::TABLER_X, label)
            .variant(ButtonVariant::Ghost)
            .hover_fill()
            .muted()
            .glass()
            .compact(24.0)
            .nested_radius(RadiusTokens::nested(surface_radius, Self::CORNER_INSET))
    }
    pub const fn variant(mut self, value: ButtonVariant) -> Self {
        self.variant = value;
        self
    }
    pub const fn shape(mut self, value: ButtonShape) -> Self {
        self.shape = value;
        self
    }
    pub const fn label_overflow(mut self, value: LabelOverflow) -> Self {
        self.label_overflow = Some(value);
        self
    }
    pub const fn disabled(mut self, value: bool) -> Self {
        self.disabled = value;
        self
    }
    pub const fn hover_fill(mut self) -> Self {
        self.hover_fill = true;
        self
    }
    pub const fn muted(mut self) -> Self {
        self.muted = true;
        self
    }
    pub const fn nested_radius(mut self, value: f32) -> Self {
        self.radius = Some(value);
        self
    }
    pub const fn glass(mut self) -> Self {
        self.glass = true;
        self
    }
    pub const fn compact(mut self, value: f32) -> Self {
        self.size = Some(value);
        self
    }
    pub fn on_activate(mut self, handler: impl Fn(ActionEvent) + 'static) -> Self {
        self.on_activate = Some(Box::new(handler));
        self
    }
    pub fn icon_slot(mut self, icon: Element) -> Self {
        self.icon_slot = Some(icon);
        self
    }
    pub fn debug_selector(mut self, selector: impl Into<String>) -> Self {
        self.debug_selector = Some(selector.into());
        self
    }
    pub const fn motion_policy(mut self, policy: MotionPolicy) -> Self {
        self.motion_policy = policy;
        self
    }
    pub fn build(self) -> Element {
        let t = self.theme;
        let (background, _foreground, border) = match self.variant {
            ButtonVariant::Default => (
                t.colors.primary.to_paint(),
                t.colors.primary_text.to_paint(),
                Oklch::TRANSPARENT,
            ),
            ButtonVariant::Outline => (
                t.colors.surface.to_paint(),
                t.colors.text.to_paint(),
                t.colors.line.to_paint(),
            ),
            ButtonVariant::Secondary => (
                t.colors.raised.to_paint(),
                t.colors.text.to_paint(),
                Oklch::TRANSPARENT,
            ),
            ButtonVariant::Ghost => (
                Oklch::TRANSPARENT,
                t.colors.text.to_paint(),
                Oklch::TRANSPARENT,
            ),
            ButtonVariant::Accent => (
                Oklch::TRANSPARENT,
                t.colors.action_text.to_paint(),
                Oklch::TRANSPARENT,
            ),
        };
        let icon_foreground = if self.muted {
            t.colors.text_muted
        } else {
            match self.variant {
                ButtonVariant::Default => t.colors.primary_text,
                ButtonVariant::Accent => t.colors.action_text,
                _ => t.colors.text,
            }
        };
        let background = if self.glass {
            Oklch::TRANSPARENT
        } else {
            background
        };
        let border = if self.glass {
            t.colors.text.at(Opacity::GlassBorder).to_paint()
        } else {
            border
        };
        let height = self.size.unwrap_or(t.density.control_sm);
        let icon_only = matches!(self.content, Content::Icon(..));
        let label = self.content.label().to_owned();
        let mut style = Style::new()
            .align(AlignItems::CENTER)
            .justify(JustifyContent::CENTER)
            .gpui_source_text_color(icon_foreground.to_reference_text_color())
            .flex_shrink(if self.label_overflow.is_some() && !icon_only {
                1.0
            } else {
                0.0
            })
            .padding_insets(Insets {
                left: if icon_only { 0.0 } else { 12.0 },
                right: if icon_only { 0.0 } else { 12.0 },
                top: 0.0,
                bottom: 0.0,
            });
        if self.variant == ButtonVariant::Accent && !self.glass {
            style = style.gpui_background(accent_background(&t, false));
        }
        if self.label_overflow.is_some() && !icon_only {
            style = style
                .min_width(Length::Px(0.0))
                .max_width(Length::Pct(100.0));
        }
        let mut root = UiButton::new()
            .height(height)
            .gap(4.0)
            .radius(self.radius.unwrap_or(self.shape.radius()))
            .border((1.0, border))
            .fill(background)
            .style(style);
        if icon_only {
            root = root.width(height);
        }
        if self.glass {
            let radius = self.radius.unwrap_or(self.shape.radius());
            let base = t.colors.text.at(Opacity::GlassBase).to_paint();
            root = root.child(
                Canvas::new()
                    .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
                    .scroll_stable()
                    .on_paint(move |bounds| {
                        vec![
                            aqua_core::DisplayItem::BackdropBlurRounded {
                                bounds,
                                radius: 12.0,
                                corners: aqua_core::CornerRadii::uniform(radius),
                            },
                            aqua_core::DisplayItem::Rect {
                                bounds,
                                radius,
                                fill: base,
                                border_width: 0.0,
                                border_color: Oklch::TRANSPARENT,
                                shadow: None,
                            },
                        ]
                    })
                    .build(),
            );
        }
        let make_text = |text: String| {
            let text = Text::new(text).text_style(TextStyle {
                families: vec![FontFamily::Named(t.typography.sans.family.to_owned())],
                size_px: t.typography.control_text,
                line_height_px: t.typography.control_text * 1.25,
                weight: FontWeight::MEDIUM,
                ..TextStyle::default()
            });
            let text = match self.label_overflow {
                Some(LabelOverflow::Start) => {
                    text.overflow(aqua_text::overflow::OverflowMode::StartEllipsis)
                }
                Some(LabelOverflow::End) => {
                    text.overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
                }
                None => text,
            };
            if self.label_overflow.is_some() {
                text.style(Style::new().flex_shrink(1.0).min_width(Length::Px(0.0)))
                    .build()
            } else {
                text.build()
            }
        };
        match self.content {
            Content::Text(text) => root = root.child(make_text(text)),
            Content::Icon(icon, _) => {
                root = root.child(
                    UiBox::new()
                        .width(16.0)
                        .height(16.0)
                        .style(Style::new().flex_shrink(0.0))
                        .clip(true)
                        .child(self.icon_slot.unwrap_or_else(|| {
                            source_asset_glyph(icon, 16.0, icon_foreground.to_reference_paint())
                        }))
                        .build(),
                )
            }
            Content::IconText(icon, text) => {
                root = root
                    .child(
                        UiBox::new()
                            .width(16.0)
                            .height(16.0)
                            .style(Style::new().flex_shrink(0.0))
                            .clip(true)
                            .child(source_asset_glyph(
                                icon,
                                16.0,
                                icon_foreground.to_reference_paint(),
                            ))
                            .build(),
                    )
                    .child(make_text(text))
            }
        }
        if self.disabled {
            root = root.opacity(Opacity::Disabled.alpha());
        } else if let Some(handler) = self.on_activate {
            root = root.on_activate_event(handler);
        }
        let hover = match self.variant {
            ButtonVariant::Default => t.colors.primary_hover.to_paint(),
            ButtonVariant::Outline | ButtonVariant::Secondary | ButtonVariant::Ghost => {
                t.colors.hover.to_paint()
            }
            ButtonVariant::Accent => Oklch::TRANSPARENT,
        };
        let element = root
            .build()
            .with_key(self.id)
            .role(Role::Button)
            .aria_label(label)
            .gpui_keyboard_activation()
            .disabled(self.disabled)
            .focusable(!self.disabled);
        let element = if let Some(selector) = self.debug_selector {
            element.debug_selector(selector)
        } else {
            element
        };
        if self.disabled {
            element.aria_description("Unavailable")
        } else {
            let focus = Style::new()
                .border_of(1.0, t.colors.ring.to_paint())
                .shadows(vec![aqua::ShadowLayer {
                    dx: 0.0,
                    dy: 0.0,
                    blur: 0.0,
                    spread: t.interaction.focus_ring_width,
                    inset: false,
                    color: t.colors.ring.at(Opacity::FocusRing).to_paint(),
                }]);
            let hover_style = if self.hover_fill {
                let (top, bottom) = hover_fill_stops(&t, t.colors.text);
                Style::new().gpui_background(aqua::GpuiBackground::LinearGradient {
                    angle: 180.0,
                    color_space: aqua::GpuiGradientColorSpace::Oklab,
                    stops: [
                        aqua::GpuiGradientStop {
                            percentage: 0.0,
                            color: top.to_reference_paint(),
                        },
                        aqua::GpuiGradientStop {
                            percentage: 1.0,
                            color: bottom.to_reference_paint(),
                        },
                    ],
                })
            } else if self.variant == ButtonVariant::Accent {
                Style::new().gpui_background(accent_background(&t, true))
            } else {
                Style::new().background(hover)
            };
            let hover_style = if self.muted {
                hover_style.gpui_source_text_color(match self.variant {
                    ButtonVariant::Default => t.colors.primary_text.to_reference_text_color(),
                    ButtonVariant::Accent => t.colors.action_text.to_reference_text_color(),
                    _ => t.colors.text.to_reference_text_color(),
                })
            } else {
                hover_style
            };
            let element = element
                .cursor(CursorIcon::Pointer)
                .focus_visible_style(focus)
                .hover_style(hover_style);
            if self.motion_policy == MotionPolicy::Full {
                element.pressed_style(
                    Style::new().transform(aqua_core::Transform2D::translate(0.0, 1.0)),
                )
            } else {
                element
            }
        }
    }
}
