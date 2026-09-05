//! Register the two bundled Spline variable fonts before any window paints.

use gpui::{App, SharedString};

/// Registers every face in [`artisan_assets::fonts::ALL`] with the
/// application's text system (DirectWrite in-memory font-file references on
/// Windows, `gpui-0.2.2/src/platform/windows/direct_write.rs:284–322`).
///
/// Idempotency is the platform's: repeated calls re-add the same in-memory
/// references and rebuild the custom collection, so callers invoke this
/// exactly once at startup.
///
/// # Errors
///
/// Returns the platform loader failure as display text; there is nothing
/// sensible to fall back to, so startup should surface it and continue with
/// system faces explicitly rather than silently.
pub fn register_bundled_fonts(app: &App) -> Result<(), SharedString> {
    app.text_system()
        .add_fonts(artisan_assets::fonts::bundled_fonts())
        .map_err(|error| {
            SharedString::from(format!("bundled typefaces failed to register: {error:#}"))
        })
}
