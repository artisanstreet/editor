use crate::frame::sidebar::{Command, Event};
use crate::frame::sign_in::card::{field_key, keyed_field};
use crate::ui::chrome::card::POLAR_INK;
use crate::ui::kit::{IconSize, Space, Spaced};
use crate::ui::theme::ArtisanTheme;
use aqua_core::CursorIcon;
use aqua_ui::AlignItems;
use aqua_ui::element::{Box as UiBox, Element};
use aqua_ui::style::{Length, Style};
use editor_assets::AssetId;
use std::rc::Rc;

#[must_use]
pub fn bar(theme: &ArtisanTheme, editor: Element, emit: Rc<dyn Fn(Event)>) -> Element {
    let content = UiBox::new()
        .layout("row")
        .spaced(Space::S8)
        .style(Style::new().align(AlignItems::CENTER))
        .child(
            UiBox::new()
                .style(Style::new().flex_shrink(0.0))
                .child(IconSize::Md.glyph(
                    AssetId::TABLER_SEARCH,
                    theme.colors.text_muted.to_reference_paint(),
                ))
                .build()
                .exclude_from_hit_testing(),
        )
        .child(
            UiBox::new()
                .layout("column")
                .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                .child(editor)
                .build(),
        )
        .build();
    keyed_field(theme, content, new_thread_key(emit))
}

fn new_thread_key(emit: Rc<dyn Fn(Event)>) -> Element {
    field_key(
        "artisan-sidebar-new-thread",
        "New thread",
        CursorIcon::Pointer,
        IconSize::Md.glyph(AssetId::TABLER_PENCIL_PLUS, POLAR_INK.to_reference_paint()),
        Rc::new(move || emit(Event::Navigate(Command::New))),
    )
}
