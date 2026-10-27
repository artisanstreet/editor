//! Relay route constants. Auth travels in-band (`hello`), never in the
//! URL or in required `Authorization` WS headers (browsers cannot set them).

use crate::grant::{is_valid_id, AccountId, MachineId};

/// Audience for Relay admission.
pub const RELAY_AUD: &str = "ast-relay";
/// First frame deadline (ms).
pub const HELLO_TIMEOUT_MS: i64 = 5_000;
/// Challenge lifetime (ms).
pub const CHALLENGE_TTL_MS: i64 = 60_000;
/// Heartbeat cadence (ms).
pub const PING_INTERVAL_MS: i64 = 20_000;
/// Heartbeat loss threshold (ms).
pub const PONG_TIMEOUT_MS: i64 = 60_000;
/// Ciphertext ceiling per `data` frame (base64url chars).
pub const MAX_BODY_B64: usize = 349_525;
/// Queued frames per channel.
pub const MAX_QUEUE_PER_CHANNEL: usize = 256;
/// Concurrent clients per room.
pub const MAX_CLIENTS: usize = 64;

/// A validated `{account}/{machine}` route.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutePath {
    pub account: AccountId,
    pub machine: MachineId,
    /// `/v1/route/{account}/{machine}` (no credentials in URL).
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid relay route")]
pub struct RouteError;

/// Build the route path, rejecting ids outside the shared charset.
pub fn route_path(account: &str, machine: &str) -> Result<RoutePath, RouteError> {
    if !is_valid_id(account) || !is_valid_id(machine) {
        return Err(RouteError);
    }
    Ok(RoutePath {
        account: AccountId(account.to_owned()),
        machine: MachineId(machine.to_owned()),
        path: format!("/v1/route/{account}/{machine}"),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn route_paths_carry_no_credentials() {
        let route = route_path("alice", "forge-01").unwrap();
        assert_eq!(route.path, "/v1/route/alice/forge-01");
        assert!(!route.path.contains("grant"));
        assert!(route_path("bad id!", "forge-01").is_err());
    }
}
