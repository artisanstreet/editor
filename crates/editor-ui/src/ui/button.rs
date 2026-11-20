use super::{
    asset::source_asset_glyph,
    kit::{Hover, Radius, live},
    theme::{ArtisanTheme, Opacity},
};
use aqua_core::{CursorIcon, Insets, Oklch, Role};
use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Button as UiButton, Element, Key},
    style::Style,
    tree::ActionEvent,
};
use editor_assets::AssetId;
pub(crate) struct Button {
    id: Key,
    theme: ArtisanTheme,
    icon: AssetId,
    label: String,
    icon_slot: Option<Element>,
    on_activate: Option<Box<dyn Fn(ActionEvent)>>,
}
impl Button {
    #[expect(
        clippy::large_types_passed_by_value,
        reason = "the icon button stores this theme copy and paints from it when built"
    )]
    pub(crate) fn icon(
        id: impl Into<Key>,
        theme: ArtisanTheme,
        icon: AssetId,
        label: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            theme,
            icon,
            label: label.into(),
            icon_slot: None,
            on_activate: None,
        }
    }
    pub(crate) fn on_activate(mut self, handler: impl Fn(ActionEvent) + 'static) -> Self {
        self.on_activate = Some(Box::new(handler));
        self
    }
    pub(crate) fn icon_slot(mut self, icon: Element) -> Self {
        self.icon_slot = Some(icon);
        self
    }
    pub(crate) fn build(self) -> Element {
        let t = self.theme;
        let icon_foreground = t.colors.text;
        let height = t.density.control_sm;
        let radius = Radius::Field.px();
        let style = Style::new()
            .align(AlignItems::CENTER)
            .justify(JustifyContent::CENTER)
            .gpui_source_text_color(icon_foreground.to_reference_text_color())
            .flex_shrink(0.0)
            .padding_insets(Insets {
                left: 0.0,
                right: 0.0,
                top: 0.0,
                bottom: 0.0,
            });
        let icon = self.icon;
        let mut root = UiButton::new()
            .height(height)
            .width(height)
            .gap(4.0)
            .radius(radius)
            .border((1.0, Oklch::TRANSPARENT))
            .fill(Oklch::TRANSPARENT)
            .style(style)
            .child(live::layer(Hover::Neutral, &t, radius, false))
            .child(
                UiBox::new()
                    .width(16.0)
                    .height(16.0)
                    .style(Style::new().flex_shrink(0.0))
                    .clip(true)
                    .child(self.icon_slot.unwrap_or_else(|| {
                        source_asset_glyph(icon, 16.0, icon_foreground.to_reference_paint())
                    }))
                    .build(),
            );
        if let Some(handler) = self.on_activate {
            root = root.on_activate_event(handler);
        }
        let element = root
            .build()
            .with_key(self.id)
            .role(Role::Button)
            .aria_label(self.label)
            .gpui_keyboard_activation()
            .disabled(false)
            .focusable(true);
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
        let element = live::track(element)
            .cursor(CursorIcon::Pointer)
            .focus_visible_style(focus)
            .hover_style(Style::new());
        super::kit::control::pressable(element, &t, 1.0)
    }
}
