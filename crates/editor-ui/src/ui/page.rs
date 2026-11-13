use super::{
    kit::{Space, Spaced, Surface, TextRole, Tone, surface::tint},
    theme::{ArtisanTheme, Oklch},
};
use aqua_core::Insets;
use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Element},
    style::Style,
};
pub const CONTENT_MAX_WIDTH: f32 = 896.0;
fn label(theme: &ArtisanTheme, text: impl Into<String>, role: TextRole, color: Oklch) -> Element {
    role.text(theme, text.into())
        .gpui_source_color(color.to_reference_text_color())
        .wrap(true)
        .build()
}
fn lead(space: Space) -> Style {
    Style::new().margin_insets(Insets {
        top: space.px(),
        ..Insets::default()
    })
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
    theme: ArtisanTheme,
    children: Vec<Element>,
    selector: Option<String>,
}
impl Notice {
    const SURFACE: Surface = Surface::Callout(Tone::Danger);
    pub fn action(mut self, action: Element) -> Self {
        self.children.push(action);
        self
    }
    pub fn debug_selector(mut self, selector: impl Into<String>) -> Self {
        self.selector = Some(selector.into());
        self
    }
    pub fn build(self) -> Element {
        let body = UiBox::new()
            .layout("column")
            .spaced(Space::S8)
            .children(self.children)
            .build();
        let content = Self::SURFACE
            .build(&self.theme, vec![body])
            .style(lead(Space::S24));
        match self.selector {
            Some(selector) => content.debug_selector(selector),
            None => content,
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
            .spaced(Space::S6)
            .child(label(theme, title, TextRole::Heading, theme.colors.text))
            .child(label(
                theme,
                description,
                TextRole::Label,
                theme.colors.text_muted,
            ))
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
        .spaced(Space::S16)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .justify(JustifyContent::SPACE_BETWEEN),
        )
        .child(label(theme, title, TextRole::LabelStrong, theme.colors.text));
    if let Some(action) = action {
        head = head.child(action);
    }
    let mut heading = UiBox::new()
        .layout("column")
        .spaced(Space::S4)
        .child(head.build());
    if let Some(intro) = intro {
        heading = heading.child(label(theme, intro, TextRole::Label, theme.colors.text_muted));
    }
    PageBlock::new(
        UiBox::new()
            .layout("column")
            .spaced(Space::S12)
            .style(lead(Space::S32))
            .child(heading.build())
            .child(body)
            .build(),
    )
}
fn message(theme: &ArtisanTheme, text: impl Into<String>, color: Oklch) -> PageBlock {
    PageBlock::new(label(theme, text, TextRole::Label, color))
}
pub fn note(theme: &ArtisanTheme, text: impl Into<String>) -> PageBlock {
    message(theme, text, theme.colors.text_muted)
}
pub fn error_text(theme: &ArtisanTheme, text: impl Into<String>) -> PageBlock {
    message(theme, text, theme.colors.destructive_text)
}
pub fn notice(theme: &ArtisanTheme, text: impl Into<String>) -> Notice {
    Notice {
        theme: *theme,
        children: vec![label(
            theme,
            text,
            TextRole::Label,
            tint(theme, Tone::Danger),
        )],
        selector: None,
    }
}
