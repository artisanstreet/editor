//! Register the bundled font faces before any window paints.

use gpui::{App, SharedString};

/// Registers every face in [`editor_assets::fonts::ALL`] with the
/// application's text system.
///
/// The production Windows and Linux backends both
/// use Cosmic, with static weight faces to preserve the same glyph shapes.
///
/// Call once at startup; repeated registration can duplicate platform font
/// entries.
///
/// # Errors
///
/// Returns the platform loader failure as display text; there is nothing
/// sensible to fall back to, so startup should surface it and continue with
/// system faces explicitly rather than silently.
pub fn register_bundled_fonts(app: &App) -> Result<(), SharedString> {
    app.text_system()
        .add_fonts(editor_assets::fonts::bundled_fonts())
        .map_err(|error| {
            SharedString::from(format!("bundled typefaces failed to register: {error:#}"))
        })
}
