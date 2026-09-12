//! Leaf support helpers for the configured-run scheduler.
//!
//! Wall-clock conversion, lease renewal cadence, identity/capability minting,
//! and the engine-tagged provider binding document. Every function here is a
//! leaf: it reads its arguments (plus the process clock and RNG) and returns a
//! value without touching dispatcher or owner state.

use std::time::Duration;

use artisan_database::{DispatchLeaseOwner, RunLaunchCredentials, RunStartKey};
use artisan_domain::{ItemId, PatchId, RunId, TurnId, UnixMillis};
use artisan_transport::CancelHandle;

use crate::{CommandOrigin, SystemCommandOrigin};

pub(super) async fn wait_for_next_claim(
    stop: &CancelHandle,
    process_cancel: &CancelHandle,
    interval: Duration,
) -> bool {
    tokio::select! {
        biased;
        () = stop.wait() => false,
        () = process_cancel.wait() => false,
        () = tokio::time::sleep(interval) => true,
    }
}

pub(super) fn wall_clock(origin: &SystemCommandOrigin) -> Option<UnixMillis> {
    origin.acceptance_instant().ok()
}

pub(super) fn add_duration(value: UnixMillis, duration: Duration) -> Option<UnixMillis> {
    let milliseconds = i64::try_from(duration.as_millis()).ok()?;
    value
        .as_millis()
        .checked_add(milliseconds)
        .map(UnixMillis::from_millis)
}

/// Renewal cadence for one claimed turn: a third of the lease, floored at a
/// millisecond so a tiny test lease still heartbeats.
pub(super) fn claim_renew_interval(claim_lease: Duration) -> Duration {
    (claim_lease / 3).max(Duration::from_millis(1))
}

pub(super) fn at_or_after(
    origin: &SystemCommandOrigin,
    not_before: UnixMillis,
) -> Option<UnixMillis> {
    Some(wall_clock(origin)?.max(not_before))
}

pub(super) fn mint_dispatch_owner() -> Option<DispatchLeaseOwner> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).ok()?;
    Some(DispatchLeaseOwner::new(bytes))
}

pub(super) fn mint_run_capabilities() -> Option<(RunStartKey, RunLaunchCredentials)> {
    let mut start = [0_u8; 32];
    let mut owner = [0_u8; 32];
    let mut lease = [0_u8; 32];
    let mut claim = [0_u8; 32];
    getrandom::fill(&mut start).ok()?;
    getrandom::fill(&mut owner).ok()?;
    getrandom::fill(&mut lease).ok()?;
    getrandom::fill(&mut claim).ok()?;
    Some((
        RunStartKey::new(start),
        RunLaunchCredentials::new(owner, lease, claim),
    ))
}

pub(super) fn mint_run_id(origin: &SystemCommandOrigin) -> Option<RunId> {
    RunId::parse(origin.mint_identity().ok()?).ok()
}

pub(super) fn mint_turn_id(origin: &SystemCommandOrigin) -> Option<TurnId> {
    TurnId::parse(origin.mint_identity().ok()?).ok()
}

pub(super) fn mint_item_id(origin: &SystemCommandOrigin) -> Option<ItemId> {
    ItemId::parse(origin.mint_identity().ok()?).ok()
}

pub(super) fn mint_patch_id(origin: &SystemCommandOrigin) -> Option<PatchId> {
    PatchId::parse(origin.mint_identity().ok()?).ok()
}

/// Builds the raw engine-tagged binding document with format 1 and the exact
/// native thread identity from the app-server contract.
///
/// The `engine` tag is `opencode2`, `codex`, `claude`, `grok`, `cursor`, or
/// `hermes`; the session id is the native thread id returned by
/// `thread/start` (Codex), `CreateSession` session (`OpenCode2`), `system/init`
/// session (Claude), `session/new` session (Grok), the ACP `session/new`
/// result (Cursor), or the durable stored session (Hermes). Empty identities
/// reject so a corrupt bind never persists.
///
/// Split from the [`ProviderBindingBytes`] wrap so the tag/format/profile
/// round trip is provable over plain bytes: [`ProviderBindingBytes`]
/// deliberately exposes no raw-byte accessor.
pub(crate) fn binding_bytes_vec(
    engine: &str,
    profile_id: &str,
    session_id: &str,
) -> Option<Vec<u8>> {
    if engine.is_empty() || profile_id.is_empty() || session_id.is_empty() {
        return None;
    }
    if engine.len() > 32 || profile_id.len() > 256 || session_id.len() > 256 {
        return None;
    }
    let value = serde_json::json!({
        "engine": engine,
        "format": 1,
        "profile_id": profile_id,
        "session_id": session_id,
    });
    serde_json::to_vec(&value).ok()
}

/// Round-trips binding bytes and proves the engine tag, format, profile, and
/// native thread identity match the selection that produced them.
///
/// A mismatch requeues through abandonment instead of persisting a corrupt
/// bind.
pub(crate) fn binding_matches_bytes(
    bytes: &[u8],
    engine: &str,
    profile_id: &str,
    session_id: &str,
) -> bool {
    let parsed: serde_json::Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(_) => return false,
    };
    let Some(object) = parsed.as_object() else {
        return false;
    };
    object.get("engine").and_then(|value| value.as_str()) == Some(engine)
        && object.get("format").and_then(serde_json::Value::as_i64) == Some(1)
        && object.get("profile_id").and_then(|value| value.as_str()) == Some(profile_id)
        && object.get("session_id").and_then(|value| value.as_str()) == Some(session_id)
}
