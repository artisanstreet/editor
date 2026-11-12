use super::{
    card::Card,
    kit::{Surface, Tone, surface::tint},
    theme::{ArtisanTheme, Oklch, Opacity},
};
use aqua_core::Insets;
use aqua_text::{FontFamily, FontWeight, TextStyle};
use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Element, Text},
    style::{Length, Style},
};
pub const CONTENT_MAX_WIDTH: f32 = 896.0;
fn label(
    theme: &ArtisanTheme,
    text: impl Into<String>,
    size: f32,
    color: Oklch,
    medium: bool,
) -> Element {
    Text::new(text.into())
        .text_style(TextStyle {
            families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
            size_px: size,
            line_height_px: size * 1.25,
            weight: if medium {
                FontWeight::MEDIUM
            } else {
                FontWeight::NORMAL
            },
            ..TextStyle::default()
        })
        .gpui_source_color(color.to_reference_text_color())
        .wrap(true)
        .build()
}
fn inset(x: f32, y: f32) -> Insets {
    Insets {
        left: x,
        right: x,
        top: y,
        bottom: y,
    }
}
pub struct PageBlock {
    content: Element,
}
impl PageBlock {
    pub const fn new(content: Element) -> Self {
        Self { content }
    }
    pub fn debug_selector(mut self, selector: impl Into<String>) -> Self {
        self.content = self.content.debug_selector(selector);
        self
    }
    pub fn build(self) -> Element {
        self.content
    }
}
pub struct Notice {
    content: UiBox,
    selector: Option<String>,
}
impl Notice {
    pub fn action(mut self, action: Element) -> Self {
        self.content = self.content.child(
            UiBox::new()
                .style(Style::new().margin_insets(Insets {
                    top: 8.0,
                    ..Insets::default()
                }))
                .child(action)
                .build(),
        );
        self
    }
    pub fn debug_selector(mut self, selector: impl Into<String>) -> Self {
        self.selector = Some(selector.into());
        self
    }
    pub fn build(self) -> Element {
        let content = self.content.build();
        if let Some(selector) = self.selector {
            content.debug_selector(selector)
        } else {
            content
        }
    }
}
pub fn preview_lines(text: &str) -> Vec<String> {
    let mut end = text.len().min(4096);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let prefix = &text[..end];
    let mut lines = prefix.lines();
    let mut shown: Vec<_> = lines.by_ref().take(8).map(str::to_owned).collect();
    let truncated = prefix.len() < text.len() || lines.next().is_some();
    if shown.iter().all(|line| line.trim().is_empty()) {
        shown = vec!["(empty)".to_owned()];
    }
    if truncated {
        shown.push("…".to_owned());
    }
    shown
}
pub fn header(
    theme: &ArtisanTheme,
    title: impl Into<String>,
    description: impl Into<String>,
) -> PageBlock {
    PageBlock::new(
        UiBox::new()
            .layout("column")
            .child(label(theme, title, 20.0, theme.colors.text, true))
            .child(
                UiBox::new()
                    .style(Style::new().margin_insets(Insets {
                        top: 6.0,
                        ..Insets::default()
                    }))
                    .child(label(
                        theme,
                        description,
                        14.0,
                        theme.colors.text_muted,
                        false,
                    ))
                    .build(),
            )
            .build(),
    )
}
pub fn section_shell(
    theme: &ArtisanTheme,
    title: &str,
    intro: Option<&str>,
    action: Option<Element>,
    body: Element,
) -> PageBlock {
    let mut head = UiBox::new()
        .gap(16.0)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .justify(JustifyContent::SPACE_BETWEEN),
        )
        .child(label(theme, title, 14.0, theme.colors.text, true));
    if let Some(action) = action {
        head = head.child(action);
    }
    let mut section = UiBox::new()
        .layout("column")
        .style(Style::new().margin_insets(Insets {
            top: 40.0,
            ..Insets::default()
        }))
        .child(head.build());
    if let Some(intro) = intro {
        section = section.child(
            UiBox::new()
                .style(Style::new().margin_insets(Insets {
                    top: 4.0,
                    ..Insets::default()
                }))
                .child(label(theme, intro, 14.0, theme.colors.text_muted, false))
                .build(),
        );
    }
    PageBlock::new(
        section
            .child(
                UiBox::new()
                    .style(Style::new().margin_insets(Insets {
                        top: 12.0,
                        ..Insets::default()
                    }))
                    .child(body)
                    .build(),
            )
            .build(),
    )
}
pub const fn card(theme: &ArtisanTheme, blocks: Vec<Element>) -> Card {
    Card::bands(*theme, blocks)
}
pub fn row(
    theme: &ArtisanTheme,
    title: impl Into<String>,
    description: impl Into<String>,
    control: Option<Element>,
) -> PageBlock {
    let description = description.into();
    let mut text = UiBox::new()
        .layout("column")
        .gap(2.0)
        .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
        .child(label(theme, title, 14.0, theme.colors.text, false));
    if !description.is_empty() {
        text = text.child(label(
            theme,
            description,
            12.0,
            theme.colors.text_muted,
            false,
        ));
    }
    let mut row = UiBox::new()
        .width(Length::Pct(100.0))
        .gap(24.0)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .justify(JustifyContent::SPACE_BETWEEN)
                .padding_insets(inset(0.0, 14.0)),
        )
        .child(text.build());
    if let Some(control) = control {
        row = row.child(
            UiBox::new()
                .style(Style::new().flex_shrink(0.0))
                .child(control)
                .build(),
        );
    }
    PageBlock::new(row.build())
}
fn message(theme: &ArtisanTheme, text: impl Into<String>, color: Oklch) -> PageBlock {
    PageBlock::new(
        UiBox::new()
            .width(Length::Pct(100.0))
            .style(Style::new().padding_insets(inset(0.0, 12.0)))
            .child(label(theme, text, 14.0, color, false))
            .build(),
    )
}
pub fn note(theme: &ArtisanTheme, text: impl Into<String>) -> PageBlock {
    message(theme, text, theme.colors.text_muted)
}
pub fn error_text(theme: &ArtisanTheme, text: impl Into<String>) -> PageBlock {
    message(theme, text, theme.colors.destructive_text)
}
pub fn notice(theme: &ArtisanTheme, text: impl Into<String>) -> Notice {
    let surface = Surface::Callout(Tone::Danger);
    Notice {
        selector: None,
        content: UiBox::new()
            .layout("column")
            .style(
                Style::new()
                    .margin_insets(Insets {
                        top: 24.0,
                        ..Insets::default()
                    })
                    .padding(Length::Px(surface.padding_px())),
            )
            .radius(surface.radius_px())
            .fill(tint(theme, Tone::Danger).at(Opacity::ToneTint).to_paint())
            .child(label(theme, text, 14.0, tint(theme, Tone::Danger), false)),
    }
}
