//! Viewers for stored Forge sources. Text and collections too large to
//! keep inline live in Forge; these show a bounded window of them through
//! [`editor_core::RangeWindow`] and let the reader page, select, copy and
//! export. Also where the application installs image decoding for
//! `editor-core`, which does not depend on the UI.

mod collection_browser;
mod content_text;

pub use collection_browser::{
    CollectionActivation, CollectionBrowser, CollectionPresenter, CollectionPreview,
};
pub use content_text::ContentText;

/// Installs what `editor-core` needs from the UI: image decoding.
pub fn init(cx: &mut gpui::App) {
    cx.set_global(editor_core::ImageDecoder(std::sync::Arc::new(
        editor_ui::markdown_renderer::decode_image_bytes,
    )));
}
