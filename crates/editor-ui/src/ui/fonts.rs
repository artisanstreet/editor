use aqua_text::font::SharedFonts;

pub fn register_bundled_fonts(fonts: &SharedFonts) {
    let mut manager = fonts.lock();
    for face in editor_assets::fonts::FACES {
        manager.load_font_static(face);
    }
}
