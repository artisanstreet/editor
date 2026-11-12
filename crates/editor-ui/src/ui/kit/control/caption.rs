use super::focus_ring;
use crate::ui::kit::hover::Hover;
use crate::ui::kit::tokens::IconSize;
use crate::ui::theme::{ArtisanTheme, Oklch};
use aqua_core::{GpuiBackground, Insets, Role};
use aqua_ui::element::{Box as UiBox, Button, Element, Key};
use aqua_ui::style::{Length, Style};
use aqua_ui::tree::ActionEvent;
use aqua_ui::{AlignItems, JustifyContent};
use editor_assets::AssetId;

pub const WIDTH: f32 = 46.0;
const FRAME: f32 = 10.0;
const PANE: f32 = 7.0;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Caption {
    Minimize,
    Maximize,
    Restore,
    Close,
}

pub struct CaptionButton {
    key: Key,
    theme: ArtisanTheme,
    caption: Caption,
    label: String,
    stroke: f32,
    lit: bool,
    on_activate: Option<Box<dyn Fn(ActionEvent)>>,
    on_hover: Option<Box<dyn Fn(bool)>>,
}

impl CaptionButton {
    pub fn new(
        key: impl Into<Key>,
        theme: ArtisanTheme,
        caption: Caption,
        label: impl Into<String>,
    ) -> Self {
        Self {
            key: key.into(),
            theme,
            caption,
            label: label.into(),
            stroke: 1.0,
            lit: false,
            on_activate: None,
            on_hover: None,
        }
    }

    #[must_use]
    pub const fn stroke(mut self, stroke: f32) -> Self {
        self.stroke = stroke;
        self
    }

    #[must_use]
    pub const fn lit(mut self, lit: bool) -> Self {
        self.lit = lit;
        self
    }

    #[must_use]
    pub fn on_activate(mut self, handler: impl Fn(ActionEvent) + 'static) -> Self {
        self.on_activate = Some(Box::new(handler));
        self
    }

    #[must_use]
    pub fn on_hover(mut self, handler: impl Fn(bool) + 'static) -> Self {
        self.on_hover = Some(Box::new(handler));
        self
    }

    fn ink(&self) -> Oklch {
        match (self.caption, self.lit) {
            (Caption::Close, true) => self.theme.colors.red.contrast,
            (_, true) => self.theme.colors.text,
            (_, false) => self.theme.colors.text_muted,
        }
    }

    fn outline(&self, insets: Insets) -> Style {
        Style::new()
            .width(Length::Px(PANE))
            .height(Length::Px(PANE))
            .gpui_border(insets, self.theme.colors.text.to_reference_paint(), false)
    }

    fn glyph(&self) -> Element {
        let stroke = self.stroke;
        match self.caption {
            Caption::Minimize => {
                IconSize::Md.glyph(AssetId::TABLER_MINUS, self.ink().to_reference_paint())
            }
            Caption::Close => {
                IconSize::Md.glyph(AssetId::TABLER_X, self.ink().to_reference_paint())
            }
            Caption::Maximize => UiBox::new()
                .width(FRAME)
                .height(FRAME)
                .style(Style::new().gpui_border(
                    Insets::uniform(stroke),
                    self.theme.colors.text.to_reference_paint(),
                    false,
                ))
                .build(),
            Caption::Restore => UiBox::new()
                .width(FRAME)
                .height(FRAME)
                .child(
                    UiBox::new()
                        .style(
                            self.outline(Insets {
                                top: stroke,
                                right: stroke,
                                ..Insets::default()
                            })
                            .absolute_at(
                                None,
                                Some(0.0),
                                Some(0.0),
                                None,
                            ),
                        )
                        .build(),
                )
                .child(
                    UiBox::new()
                        .style(self.outline(Insets::uniform(stroke)).absolute_at(
                            Some(0.0),
                            None,
                            None,
                            Some(0.0),
                        ))
                        .build(),
                )
                .build(),
        }
    }

    fn hover(&self) -> Style {
        match self.caption {
            Caption::Close => Style::new().gpui_background(GpuiBackground::Solid(
                self.theme.colors.red.solid.to_reference_paint(),
            )),
            Caption::Minimize | Caption::Maximize | Caption::Restore => {
                Hover::Neutral.style(&self.theme)
            }
        }
    }

    #[must_use]
    pub fn build(self) -> Element {
        let hover = self.hover();
        let mut button = Button::new()
            .width(WIDTH)
            .height(Length::Pct(100.0))
            .style(
                Style::new()
                    .flex_shrink(0.0)
                    .align(AlignItems::CENTER)
                    .justify(JustifyContent::CENTER),
            )
            .child(self.glyph());
        if let Some(handler) = self.on_activate {
            button = button.on_activate_event(handler);
        }
        let element = button
            .build()
            .with_key(self.key)
            .role(Role::Button)
            .aria_label(self.label)
            .tab_index(0)
            .gpui_keyboard_activation()
            .hover_style(hover)
            .focus_visible_style(focus_ring(&self.theme));
        match self.on_hover {
            Some(handler) => element.on_hover_change(handler),
            None => element,
        }
    }
}
