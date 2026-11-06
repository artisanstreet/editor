use aqua_core::GpuiTextColor;
use aqua_text::overflow::OverflowMode;
use aqua_ui::{
    element::{Element, Text},
    style::{Length, Style},
};

pub struct TruncatedText {
    text: String,
    size: f32,
    color: GpuiTextColor,
    selector: Option<String>,
}

impl TruncatedText {
    pub fn new(text: impl Into<String>, size: f32, color: GpuiTextColor) -> Self {
        Self {
            text: text.into(),
            size,
            color,
            selector: None,
        }
    }

    pub fn debug_selector(mut self, selector: impl Into<String>) -> Self {
        self.selector = Some(selector.into());
        self
    }

    pub fn build(self) -> Element {
        let element = Text::new(self.text)
            .font_size(self.size)
            .gpui_source_color(self.color)
            .overflow(OverflowMode::EndEllipsis)
            .style(Style::new().min_width(Length::Px(0.0)).flex_shrink(1.0))
            .build();
        if let Some(selector) = self.selector {
            element.debug_selector(selector)
        } else {
            element
        }
    }
}
