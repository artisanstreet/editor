//! Machine discovery, routing, config revisions, and the provider vault.
//!
//! The account is the authority for discovery; the Forge stays authoritative
//! for threads/runs/files. Direct routes are preferred; Relay is a fallback.
//! The vault holds assigned provider material in memory only and never logs
//! secrets.

/// A machine known to the account. Core distributes the Ed25519 identity
/// key plus the Noise X25519 static (`contracts/e2e.md` §5); the store
/// carries both so Forge/Editor can check peers with no TOFU.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineRecord {
    pub machine: String,
    pub ed25519_pk_b64u: String,
    pub noise_static_b64u: String,
    pub fingerprint: String,
}

/// Where to reach a machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoutingDecision {
    /// A paired direct host exists and is preferred.
    Direct,
    /// No usable direct route; use the Relay route.
    Relay,
    /// Neither is available.
    Unreachable,
}

/// Inputs to [`decide_route`]: one struct instead of four bools.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteConditions {
    pub has_paired_direct: bool,
    pub direct_reachable: bool,
    pub relay_configured: bool,
    pub forge_online: bool,
}

/// Prefer direct local connections; never invalidate a working direct route
/// because Relay is reachable.
#[must_use]
pub const fn decide_route(conditions: RouteConditions) -> RoutingDecision {
    if conditions.has_paired_direct && conditions.direct_reachable {
        RoutingDecision::Direct
    } else if conditions.relay_configured && conditions.forge_online {
        RoutingDecision::Relay
    } else if conditions.has_paired_direct {
        // Keep the direct entry even while offline; the UI shows it as such.
        RoutingDecision::Direct
    } else {
        RoutingDecision::Unreachable
    }
}

/// Versioned configuration the account distributes (last-good cache wins
/// while offline; a newer revision replaces the cache).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigRevision {
    pub global_prompt_version: u64,
    pub skills_revision: u64,
    pub updated_at_sec: i64,
}

impl ConfigRevision {
    /// Whether `other` supersedes `self`.
    #[must_use]
    pub const fn is_superseded_by(&self, other: &Self) -> bool {
        other.global_prompt_version > self.global_prompt_version
            || (other.global_prompt_version == self.global_prompt_version
                && other.skills_revision > self.skills_revision)
    }
}

/// Assigned provider material, sealed opaque. Forge keeps it in memory only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VaultItem {
    pub provider: String,
    pub revision: u64,
    pub sealed: Vec<u8>,
    pub updated_at_sec: i64,
}

impl VaultItem {
    /// Secrets are redacted in logs: only names and revisions print.
    pub fn describe(&self) -> String {
        format!("provider={} revision={}", self.provider, self.revision)
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test fixtures; failures fail the test"
)]
mod tests {
    use super::*;

    const fn route(
        has_paired_direct: bool,
        direct_reachable: bool,
        relay_configured: bool,
        forge_online: bool,
    ) -> RouteConditions {
        RouteConditions {
            has_paired_direct,
            direct_reachable,
            relay_configured,
            forge_online,
        }
    }

    #[test]
    fn direct_routes_win_over_relay() {
        assert_eq!(
            decide_route(route(true, true, true, true)),
            RoutingDecision::Direct
        );
        assert_eq!(
            decide_route(route(false, false, true, true)),
            RoutingDecision::Relay
        );
        assert_eq!(
            decide_route(route(false, false, false, false)),
            RoutingDecision::Unreachable
        );
    }

    #[test]
    fn revisions_only_move_forward() {
        let old = ConfigRevision {
            global_prompt_version: 3,
            skills_revision: 9,
            updated_at_sec: 1,
        };
        let same = old.clone();
        assert!(!old.is_superseded_by(&same));
        let newer = ConfigRevision {
            global_prompt_version: 4,
            skills_revision: 0,
            updated_at_sec: 2,
        };
        assert!(old.is_superseded_by(&newer));
    }
}
