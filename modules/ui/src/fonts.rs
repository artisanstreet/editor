//! Startup registration of the bundled legacy typefaces.
//!
//! [`artisan_assets::fonts`] embeds the four legacy `@font-face` sources as
//! compile-time bytes; this module is the one call that feeds them to the
//! platform text system. Registration must happen before any window paints
//! text in a bundled family, i.e. first inside
//! `Application::new().run(|cx| …)` in `native_application.rs` (a forbidden
//! path for this wave, so the orchestrator wires the call; see the report).
//! Until then every `.font_family("Artisan Neo")` refinement resolves
//! through GPUI's fallback stack instead of the vendored faces.

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
