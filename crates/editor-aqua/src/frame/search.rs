use crate::frame::sidebar::{Command, Event};
use crate::frame::sign_in::card::{field_key, keyed_field};
use crate::frame::tree::ROW_PAD;
use crate::ui::chrome::card::{POLAR_INK, Surfaces};
use crate::ui::theme::ArtisanTheme;
use aqua::{Aqua, px};
use aqua_core::{CursorIcon, Insets};
use aqua_ui::AlignItems;
use aqua_ui::element::Element;
use editor_assets::AssetId;
use std::rc::Rc;

const BELOW: f32 = 16.0;
const KEY_ICON: f32 = 16.0;

#[must_use]
pub fn bar(theme: &ArtisanTheme, editor: Element, emit: Rc<dyn Fn(Event)>) -> Element {
    let surfaces = Surfaces::of(theme);
    let content = Aqua::Box()
        .layout("row")
        .styles([Aqua::Style().align(AlignItems::CENTER).gap(8.0)])
        .child(
            Aqua::Box()
                .styles([Aqua::Style().flex_shrink(0.0)])
                .child(crate::ui::asset::source_asset_glyph(
                    AssetId::TABLER_SEARCH,
                    16.0,
                    surfaces.ink_dim.to_reference_paint(),
                ))
                .build()
                .exclude_from_hit_testing(),
        )
        .child(
            Aqua::Box()
                .layout("column")
                .styles([Aqua::Style().flex_grow(1.0).min_width(px(0.0))])
                .child(editor)
                .build(),
        )
        .build();
    Aqua::Box()
        .layout("column")
        .styles([Aqua::Style().flex_shrink(0.0).margin_insets(Insets {
            left: ROW_PAD,
            bottom: BELOW,
            ..Insets::default()
        })])
        .child(keyed_field(&surfaces, content, new_thread_key(emit)))
        .build()
}

fn new_thread_key(emit: Rc<dyn Fn(Event)>) -> Element {
    field_key(
        "artisan-sidebar-new-thread",
        "New thread",
        CursorIcon::Pointer,
        crate::ui::asset::source_asset_glyph(
            AssetId::TABLER_PENCIL_PLUS,
            KEY_ICON,
            POLAR_INK.to_reference_paint(),
        ),
        Rc::new(move || emit(Event::Navigate(Command::New))),
    )
}
