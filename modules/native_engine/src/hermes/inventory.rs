//! Live-inventory shape and installed-profile layout recorded as data.
//!
//! TypeScript evidence:
//! - `modules/engines/src/hermes/engine.ts` (`CatalogFrom`): the live catalog
//!   comes from the `model.options` gateway method with `explicit_only: true`,
//!   `include_unconfigured: false`, `refresh: false`.
//! - `modules/engines/src/hermes/engine.ts` (descriptor): `auth: unsupported`
//!   because provider setup stays owned by the installed Hermes profile; the
//!   session profile is `default` unless a named profile is selected.
//!
//! These types only describe that shape for the later runtime packet. Nothing
//! here sends a request, opens a session, or touches credentials.

use std::fmt;

/// Hermes engine identifier shared with the TypeScript descriptor.
pub const HERMES_ENGINE_ID: &str = "hermes";

/// Gateway method serving the live provider/model inventory.
pub const MODEL_OPTIONS_METHOD: &str = "model.options";

/// Machine-readable reason for `Unknown` probe authentication.
///
/// Mirrors the TypeScript descriptor reason ("Provider setup remains owned by
/// the installed Hermes profile.") without synthesizing auth state.
pub const AUTH_UNKNOWN_REASON: &str = "owned-by-installed-profile";

/// Owner marker for credentials: the installed Hermes profile, never Artisan.
pub const INSTALLED_PROFILE_AUTH_OWNER: &str = "installed-hermes-profile";

/// Profile identifier used by the TypeScript probe scope.
pub const DEFAULT_PROFILE_ID: &str = "default";

/// The exact `model.options` request shape the later runtime packet must send.
///
/// Recorded here as data only; this packet never executes it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HermesInventoryRequest {
    method: &'static str,
    explicit_only: bool,
    include_unconfigured: bool,
    refresh: bool,
}

impl HermesInventoryRequest {
    /// Returns the gateway method name (`model.options`).
    #[must_use]
    pub const fn method(&self) -> &'static str {
        self.method
    }

    /// Returns whether only explicitly enabled providers are requested.
    #[must_use]
    pub const fn explicit_only(&self) -> bool {
        self.explicit_only
    }

    /// Returns whether unconfigured providers are included.
    #[must_use]
    pub const fn include_unconfigured(&self) -> bool {
        self.include_unconfigured
    }

    /// Returns whether a live refresh is requested.
    #[must_use]
    pub const fn refresh(&self) -> bool {
        self.refresh
    }
}

impl fmt::Display for HermesInventoryRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} explicit_only={} include_unconfigured={} refresh={}",
            self.method, self.explicit_only, self.include_unconfigured, self.refresh
        )
    }
}

/// Returns the live-inventory request shape from TypeScript `CatalogFrom`.
#[must_use]
pub const fn live_inventory_request() -> HermesInventoryRequest {
    HermesInventoryRequest {
        method: MODEL_OPTIONS_METHOD,
        explicit_only: true,
        include_unconfigured: false,
        refresh: false,
    }
}

/// Installed-profile layout: auth lives in the Hermes profile, not Artisan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HermesInstalledProfileLayout {
    engine_id: &'static str,
    profile_id: String,
    default_profile: bool,
    auth_owner: &'static str,
}

impl HermesInstalledProfileLayout {
    /// Returns the engine identifier (`hermes`).
    #[must_use]
    pub const fn engine_id(&self) -> &'static str {
        self.engine_id
    }

    /// Returns the profile identifier selecting the installed Hermes profile.
    #[must_use]
    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }

    /// Returns whether this is the default profile scope.
    #[must_use]
    pub const fn default_profile(&self) -> bool {
        self.default_profile
    }

    /// Returns who owns credentials (always the installed Hermes profile).
    #[must_use]
    pub const fn auth_owner(&self) -> &'static str {
        self.auth_owner
    }
}

/// Records which installed Hermes profile a probe scope selects.
#[must_use]
pub fn installed_profile_layout(profile_id: &str) -> HermesInstalledProfileLayout {
    HermesInstalledProfileLayout {
        engine_id: HERMES_ENGINE_ID,
        profile_id: profile_id.to_string(),
        default_profile: profile_id == DEFAULT_PROFILE_ID,
        auth_owner: INSTALLED_PROFILE_AUTH_OWNER,
    }
}
