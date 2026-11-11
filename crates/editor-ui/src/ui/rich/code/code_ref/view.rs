use std::rc::Rc;

use aqua_core::Insets;
use aqua_text::{FontFamily, FontWeight, TextStyle};
use aqua_ui::{
    AlignItems, JustifyContent,
    collection::List,
    element::{Box as UiBox, Element, Scroll, Text},
    style::{Length, Style},
};
use editor_assets::AssetId;

use super::CodeRefBlock;
use crate::ui::{
    button::{Button, ButtonVariant, LabelOverflow},
    rich::actions::{OpenReference, ResourceScope},
    theme::{ArtisanTheme, TextStep, TypographyTokens},
};

const LINE_PX: f32 = 20.0;

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
        let path = Button::icon_text(
            format!("{id}-path"),
            *theme,
            AssetId::TABLER_FILE_TEXT,
            reference.display(),
        )
        .variant(ButtonVariant::Ghost)
        .label_overflow(LabelOverflow::Start)
        .on_activate(move |_| open(action.clone(), instance))
        .build();
        let head = UiBox::new()
            .gap(8.0)
            .style(
                Style::new()
                    .align(AlignItems::CENTER)
                    .min_width(Length::Px(0.0)),
            )
            .child(path)
            .build();
        let mut body = UiBox::new()
            .layout("column")
            .gap(8.0)
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
    TextStyle {
        families: vec![FontFamily::Named(if mono {
            theme.typography.mono.family.to_owned()
        } else {
            theme.typography.sans.family.to_owned()
        })],
        size_px: TypographyTokens::size(TextStep::XsMid),
        line_height_px: LINE_PX,
        weight: FontWeight::NORMAL,
        letter_spacing_em: if mono { Some(0.0) } else { None },
        ..TextStyle::default()
    }
}

fn numbered(theme: &ArtisanTheme, id: &str, block: &CodeRefBlock) -> Element {
    let count = block.lines.len();
    let start = block.reference.start.unwrap_or(1);
    let largest = start.saturating_add(u32::try_from(count).unwrap_or(u32::MAX));
    let gutter = f32::from(u16::try_from(largest.max(1).ilog10() + 1).unwrap_or(u16::MAX))
        .mul_add(8.0, 20.0);
    let width = block.width + gutter;
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
                .style(Style::new().min_width(Length::Px(0.0)))
                .clip(true)
                .child(
                    UiBox::new()
                        .width(gutter)
                        .style(
                            Style::new()
                                .flex_shrink(0.0)
                                .justify(JustifyContent::END)
                                .padding_insets(Insets {
                                    right: 12.0,
                                    ..Insets::default()
                                }),
                        )
                        .child(number)
                        .build(),
                )
                .child(
                    UiBox::new()
                        .style(
                            Style::new()
                                .flex_grow(1.0)
                                .min_width(Length::Px(0.0))
                                .min_height(Length::Px(LINE_PX))
                                .padding_insets(Insets {
                                    right: 12.0,
                                    ..Insets::default()
                                }),
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
