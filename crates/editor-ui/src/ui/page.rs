use super::{
    kit::{Space, Spaced, TextRole},
    theme::{ArtisanTheme, Oklch},
};
use aqua_core::{Insets, Role};
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
#[derive(Debug)]
pub struct PageBlock {
    content: Element,
}
impl PageBlock {
    const fn new(content: Element) -> Self {
        Self { content }
    }
    pub fn build(self) -> Element {
        self.content
    }
}
pub fn preview_lines(text: &str) -> Vec<String> {
    let prefix = crate::ui::text::editor::grapheme_prefix(text, 4096);
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
            .child(label(theme, title, TextRole::Heading, theme.colors.text).role(Role::Heading(1)))
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
        .child(
            label(theme, title, TextRole::LabelStrong, theme.colors.text).role(Role::Heading(2)),
        );
    if let Some(action) = action {
        head = head.child(action);
    }
    let mut heading = UiBox::new()
        .layout("column")
        .spaced(Space::S4)
        .child(head.build());
    if let Some(intro) = intro {
        heading = heading.child(label(
            theme,
            intro,
            TextRole::Label,
            theme.colors.text_muted,
        ));
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
#[cfg(test)]
#[path = "../../tests/ui/page.rs"]
mod tests;
