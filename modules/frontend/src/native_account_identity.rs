//! Editor account presentation, separate from Forge host identity and engine credentials.
//!
//! A future Artisan Street sign-in owner can install this global after authentication
//! and remove it on sign-out, then refresh the application. It grants no authority
//! and contains no credentials. Without it, the editor uses its local identity.

/// Display identity supplied by the Artisan Street account session.
pub struct ArtisanAccountIdentity {
    /// Account display name, preserving the account's spelling and capitalization.
    pub display_name: String,
}

impl gpui::Global for ArtisanAccountIdentity {}
