use std::rc::Rc;

use aqua_text::TextStyle;
use aqua_ui::{
    AlignItems, JustifyContent,
    collection::List,
    element::{Box as UiBox, Element, Scroll, Text},
    style::{Length, Style},
};
use editor_assets::AssetId;

use super::CodeRefBlock;
use crate::ui::{
    kit::{Button, ButtonSize, ButtonTone, Ellipsis, Label, Space, Spaced, TextRole},
    rich::actions::{OpenReference, ResourceScope},
    theme::ArtisanTheme,
};

const LINE_PX: f32 = TextRole::Code.spec().line;

impl CodeRefBlock {
    pub(crate) fn copy(&self) -> String {
        self.excerpt()
            .map_or_else(|| self.reference.display(), str::to_owned)
    }

    pub(crate) fn render(
        &self,
        theme: &ArtisanTheme,
        id: &str,
        scope: Option<ResourceScope>,
        instance: Option<editor_core::HostInstance>,
        open: Rc<dyn Fn(OpenReference, Option<editor_core::HostInstance>)>,
    ) -> Element {
        let reference = &self.reference;
        let action = OpenReference {
            scope,
            path: reference.path.clone(),
            line: reference.start,
        };
        let path = Button::new(
            format!("{id}-path"),
            *theme,
            ButtonTone::Quiet,
            ButtonSize::Md,
            Label::IconText(AssetId::TABLER_FILE_TEXT, reference.display()),
        )
        .ellipsis(Ellipsis::Start)
        .on_activate(move |_| open(action.clone(), instance))
        .build();
        let head = UiBox::new()
            .spaced(Space::S8)
            .style(
                Style::new()
                    .align(AlignItems::CENTER)
                    .min_width(Length::Px(0.0)),
            )
            .child(path)
            .build();
        let mut body = UiBox::new()
            .layout("column")
            .spaced(Space::S8)
            .width(Length::Pct(100.0))
            .style(Style::new().min_width(Length::Px(0.0)))
            .child(head);
        if let Some(note) = self.note() {
            body = body.child(
                Text::new(note.to_owned())
                    .text_style(text_style(theme, false))
                    .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                    .wrap(true)
                    .build(),
            );
        }
        if self.excerpt().is_some() {
            body = body.child(numbered(theme, id, self));
        }
        body.build()
    }
}

fn text_style(theme: &ArtisanTheme, mono: bool) -> TextStyle {
    match mono {
        true => TextRole::Code.style(theme),
        false => TextRole::Label.style(theme),
    }
}

fn numbered(theme: &ArtisanTheme, id: &str, block: &CodeRefBlock) -> Element {
    let count = block.lines.len();
    let start = block.reference.start.unwrap_or(1);
    let largest = start.saturating_add(u32::try_from(count).unwrap_or(u32::MAX));
    let gutter = f32::from(u16::try_from(largest.max(1).ilog10() + 1).unwrap_or(u16::MAX))
        .mul_add(Space::S8.px(), Space::S8.px());
    let width = block.width + gutter + Space::S12.px();
    let height = super::super::super::charts::scale::f32_of_count(count.min(18)) * LINE_PX;
    let lines = block.lines.clone();
    let theme = *theme;
    let rows = List::new()
        .items(0..count)
        .key(|index| *index)
        .row(move |index| {
            let number = start.saturating_add(u32::try_from(*index).unwrap_or(u32::MAX));
            let number = Text::new(number.to_string())
                .text_style(text_style(&theme, true))
                .gpui_source_color(theme.colors.text_faint.to_reference_text_color())
                .wrap(false)
                .build();
            let source = Text::new(lines[*index].clone())
                .text_style(text_style(&theme, true))
                .gpui_source_color(theme.colors.text.to_reference_text_color())
                .wrap(false)
                .build();
            UiBox::new()
                .width(Length::Pct(100.0))
                .height(LINE_PX)
                .spaced(Space::S12)
                .style(Style::new().min_width(Length::Px(0.0)))
                .clip(true)
                .child(
                    UiBox::new()
                        .width(gutter)
                        .style(Style::new().flex_shrink(0.0).justify(JustifyContent::END))
                        .child(number)
                        .build(),
                )
                .child(
                    UiBox::new()
                        .style(
                            Style::new()
                                .flex_grow(1.0)
                                .min_width(Length::Px(0.0))
                                .min_height(Length::Px(LINE_PX)),
                        )
                        .child(source)
                        .build(),
                )
                .build()
        })
        .style(
            Style::new()
                .width(Length::Pct(100.0))
                .min_width(Length::Px(width))
                .height(Length::Px(height)),
        )
        .estimated_row_height(Length::Px(LINE_PX))
        .virtualize()
        .build()
        .with_key(format!("{id}-excerpt-list"));
    Scroll::new()
        .width(Length::Pct(100.0))
        .height(height)
        .style(Style::new().min_width(Length::Px(0.0)))
        .child(rows)
        .build()
        .with_key(format!("{id}-excerpt"))
}

#[cfg(test)]
#[path = "../../../../../tests/ui/rich/code/code_ref.rs"]
mod tests;
