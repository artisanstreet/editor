//! OpenCode2 profile registry, registration, and verified launch resolution.
//!
//! The bounded registry codec lives in [`registry`], registry-facing authority
//! operations in [`authority`], and verified launch capabilities in [`launch`].

#![forbid(unsafe_code)]

#[path = "profile/authority.rs"]
mod authority;
#[path = "profile/launch.rs"]
mod launch;
#[path = "profile/registry.rs"]
mod registry;
#[cfg(test)]
#[path = "profile/tests.rs"]
mod tests;
#[path = "profile/types.rs"]
mod types;

pub use self::launch::VerifiedOpenCode2ProfileLaunch;
pub use self::types::{
    NativeOpenCode2ProfileError, NativeOpenCode2ProfileLaunchError, OpenCode2Profile,
    ProfileHomeKind, ProfileRegistrationOutcome,
};

const MAX_PROFILE_REGISTRY_BYTES: usize = 16 * 1024;
const MAX_PROFILES: usize = 64;
const PROFILE_REGISTRY_FORMAT_VERSION: u64 = 1;

const PROFILE_REGISTRY_FIELDS: &[&str] = &["engine_id", "format_version", "profiles"];
const PROFILE_FIELDS: &[&str] = &["profile_id", "home"];
