//! Buttons own their geometry, keyboard focus and accessible name.

use editor_assets::AssetId;
use gpui::prelude::FluentBuilder;
use gpui::{
    App, BoxShadow, ClickEvent, ElementId, FontWeight, InteractiveElement, IntoElement,
    ParentElement, RenderOnce, Role, SharedString, StatefulInteractiveElement, Styled, Window, div,
    point, px, transparent_black,
};

use crate::asset_seam::asset_glyph;
use crate::motion::MotionPolicy;
use crate::theme::{ArtisanTheme, SurfaceStep};

/// The visual purpose of an action.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ButtonVariant {
    #[default]
    Default,
    Outline,
    Secondary,
    Ghost,
}

/// Supported corner recipes. Geometry is never supplied by individual callers.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ButtonShape {
    #[default]
    Pill,
    Rounded,
    Square,
}

impl ButtonShape {
    fn radius(self) -> gpui::Pixels {
        px(match self {
            Self::Pill => 16.0,
            Self::Rounded => 10.0,
            Self::Square => 4.0,
        })
    }
}

/// Where a bounded text label keeps its meaningful end.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LabelOverflow {
    Start,
    End,
}

fn label_element(label: SharedString, overflow: Option<LabelOverflow>) -> gpui::AnyElement {
    match overflow {
        None => label.into_any_element(),
        Some(overflow) => {
            let text = div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap();
            let text = match overflow {
                LabelOverflow::Start => text.text_ellipsis_start(),
                LabelOverflow::End => text.text_ellipsis(),
            };
            text.child(label).into_any_element()
        }
    }
}

enum Content {
    Text(SharedString),
    Icon { icon: AssetId, label: SharedString },
    IconText { icon: AssetId, label: SharedString },
}

impl Content {
    fn label(&self) -> SharedString {
        match self {
            Self::Text(label) | Self::Icon { label, .. } | Self::IconText { label, .. } => {
                label.clone()
            }
        }
    }
}

type Activation = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// An infallible text, icon, or icon-and-text action. The constructor fixes its shape.
#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    theme: ArtisanTheme,
    content: Content,
    variant: ButtonVariant,
    shape: ButtonShape,
    disabled: bool,
    hover_fill: bool,
    label_overflow: Option<LabelOverflow>,
    icon_slot: Option<gpui::AnyElement>,
    on_activate: Option<Activation>,
    debug_selector: Option<SharedString>,
}

impl Button {
    fn with_content(id: impl Into<ElementId>, theme: ArtisanTheme, content: Content) -> Self {
        Self {
            id: id.into(),
            theme,
            content,
            variant: ButtonVariant::Default,
            shape: ButtonShape::Pill,
            disabled: false,
            hover_fill: false,
            label_overflow: None,
            icon_slot: None,
            on_activate: None,
            debug_selector: None,
        }
    }

    pub fn text(
        id: impl Into<ElementId>,
        theme: ArtisanTheme,
        label: impl Into<SharedString>,
    ) -> Self {
        Self::with_content(id, theme, Content::Text(label.into()))
    }

    /// The icon's accessible label is required even though it is not painted.
    pub fn icon(
        id: impl Into<ElementId>,
        theme: ArtisanTheme,
        icon: AssetId,
        label: impl Into<SharedString>,
    ) -> Self {
        Self::with_content(
            id,
            theme,
            Content::Icon {
                icon,
                label: label.into(),
            },
        )
    }

    pub fn icon_text(
        id: impl Into<ElementId>,
        theme: ArtisanTheme,
        icon: AssetId,
        label: impl Into<SharedString>,
    ) -> Self {
        Self::with_content(
            id,
            theme,
            Content::IconText {
                icon,
                label: label.into(),
            },
        )
    }

    #[must_use]
    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }
    #[must_use]
    pub fn shape(mut self, shape: ButtonShape) -> Self {
        self.shape = shape;
        self
    }
    #[must_use]
    pub fn label_overflow(mut self, overflow: LabelOverflow) -> Self {
        self.label_overflow = Some(overflow);
        self
    }
    #[must_use]
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    /// Uses the established shared row highlight on a surface action.
    pub fn hover_fill(mut self) -> Self {
        self.hover_fill = true;
        self
    }
    #[must_use]
    pub fn on_activate(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_activate = Some(Box::new(handler));
        self
    }
    /// Animated feedback stays inside the component's fixed icon slot.
    #[must_use]
    pub fn icon_slot(mut self, icon: impl IntoElement) -> Self {
        self.icon_slot = Some(icon.into_any_element());
        self
    }
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector = Some(selector.into());
        self
    }
}

impl RenderOnce for Button {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let transparent = transparent_black();
        let (background, foreground, border, hover) = match self.variant {
            ButtonVariant::Default => (
                theme.colors.primary.to_paint(),
                theme.colors.primary_foreground.to_paint(),
                transparent,
                theme.colors.primary.with_alpha(0.8).to_paint(),
            ),
            ButtonVariant::Outline => (
                SurfaceStep::S900.oklch().to_paint(),
                theme.colors.foreground.to_paint(),
                theme.colors.border.to_paint(),
                theme.colors.input.with_alpha(0.5).to_paint(),
            ),
            ButtonVariant::Secondary => (
                theme.colors.secondary.to_paint(),
                theme.colors.secondary_foreground.to_paint(),
                transparent,
                theme.colors.secondary.with_alpha(0.8).to_paint(),
            ),
            ButtonVariant::Ghost => (
                transparent,
                theme.colors.foreground.to_paint(),
                transparent,
                theme.colors.muted.with_alpha(0.5).to_paint(),
            ),
        };
        let icon_only = matches!(self.content, Content::Icon { .. });
        let mut root = div()
            .id(self.id)
            .role(Role::Button)
            .aria_label(self.content.label())
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .gap(px(4.0))
            .h(theme.density.control_sm)
            .rounded(self.shape.radius())
            .border_1()
            .border_color(border)
            .bg(background)
            .text_color(foreground)
            .text_size(theme.typography.control_text)
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .when(icon_only, |el| el.w(theme.density.control_sm))
            .when(!icon_only, |el| el.px(px(12.0)))
            .when_some(self.debug_selector, |el, selector| {
                el.debug_selector(move || selector.to_string())
            });
        root = root.when(self.label_overflow.is_some() && !icon_only, |root| {
            root.flex_shrink_1().min_w_0().max_w(gpui::relative(1.0))
        });
        let overflow = self.label_overflow;
        let icon_slot = self.icon_slot;
        root = match self.content {
            Content::Text(label) => root.child(label_element(label, overflow)),
            Content::Icon { icon, .. } => root.child(
                div()
                    .size(px(16.0))
                    .flex_shrink_0()
                    .overflow_hidden()
                    .child(
                        icon_slot
                            .unwrap_or_else(|| asset_glyph(icon).size(px(16.0)).into_any_element()),
                    ),
            ),
            Content::IconText { icon, label } => root
                .child(
                    div()
                        .size(px(16.0))
                        .flex_shrink_0()
                        .overflow_hidden()
                        .child(asset_glyph(icon).size(px(16.0))),
                )
                .child(label_element(label, overflow)),
        };
        if self.disabled {
            return root.aria_description("Unavailable").opacity(0.5);
        }
        root = root
            .focusable()
            .tab_index(0)
            .cursor_pointer()
            .focus_visible(move |style| {
                style
                    .border_color(theme.colors.ring.to_paint())
                    .shadow(vec![BoxShadow {
                        color: theme.interaction.focus_ring_color.to_paint(),
                        offset: point(px(0.0), px(0.0)),
                        blur_radius: px(0.0),
                        spread_radius: theme.interaction.focus_ring_width,
                        inset: false,
                    }])
            })
            .hover(move |style| {
                if self.hover_fill {
                    crate::hover_card::hover_fill(
                        style,
                        &theme,
                        crate::hover_card::HoverFill::Plain,
                    )
                } else {
                    style.bg(hover)
                }
            })
            .when(MotionPolicy::current(cx) == MotionPolicy::Full, |el| {
                el.active(|style| style.relative().top(px(1.0)))
            });
        if let Some(handler) = self.on_activate {
            root = root.on_click(handler);
        }
        root
    }
}
