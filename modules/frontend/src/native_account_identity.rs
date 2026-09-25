//! Editor account presentation, separate from Forge host identity and engine credentials.
//!
//! The connected Forge supplies the account it runs as (its user's
//! preferences carry the account profile), and the Editor installs it here
//! whenever those preferences arrive; a future Artisan Street sign-in owner
//! can install it the same way. It grants no authority and contains no
//! credentials. The Editor never derives it from its own machine's
//! environment, which may not be the connected host's.

/// Display identity supplied by the connected Forge's account.
pub struct ArtisanAccountIdentity {
    /// Account display name, preserving the account's spelling and capitalization.
    pub display_name: String,
}

impl gpui::Global for ArtisanAccountIdentity {}
