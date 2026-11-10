use aqua_text::font::SharedFonts;

pub fn register_bundled_fonts(fonts: &SharedFonts) {
    let mut manager = fonts.lock();
    for bytes in editor_assets::fonts::bundled_fonts() {
        match bytes {
            std::borrow::Cow::Borrowed(bytes) => manager.load_font_static(bytes),
            std::borrow::Cow::Owned(bytes) => manager.load_font_bytes(bytes),
        }
    }
}
