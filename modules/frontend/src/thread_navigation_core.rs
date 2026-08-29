//! Dependency-free thread route, status, and recent-time presentation core.
//!
//! This is the narrow native counterpart of the corresponding pure helpers in
//! `modules/frontend/src/lib/root/thread-navigation.ts`. It accepts the small
//! values those helpers actually inspect rather than importing protocol
//! records, URL machinery, or transport state.
//!
//! Route identities are borrowed and never percent-encoded here. Statuses
//! remain strings so that an unknown non-resting status keeps the legacy
//! active-work behavior. Recent-time formatting accepts an already-parsed
//! signed Unix millisecond value; callers own timestamp parsing.

/// The historical prefix removed from a thread's public route segment.
pub const LEGACY_THREAD_PREFIX: &str = "thread_";

/// The reserved route segment representing a thread with no workspace.
pub const DETACHED_WORKSPACE_ROUTE_ID: &str = "_";

/// The status emitted after a run finishes successfully.
pub const COMPLETE_STATUS: &str = "Complete";

/// The current status emitted after a run fails.
pub const FAILED_STATUS: &str = "Failed to complete";

/// The legacy failure status retained for sticky historical projections.
pub const LEGACY_FAILED_STATUS: &str = "Needs attention";

/// The status for a thread with no active or pending run outcome.
pub const IDLE_STATUS: &str = "Idle";

/// Statuses that no longer represent active work.
pub const RESTING_THREAD_STATUSES: [&str; 2] = [COMPLETE_STATUS, IDLE_STATUS];

/// Removes the exact historical `thread_` prefix from a public route segment.
///
/// An exact prefix with no remaining identity falls back to the original
/// input, matching the legacy helper's protection against turning `thread_`
/// into an empty route segment. Other strings, including the empty string,
/// are returned unchanged.
#[must_use]
pub fn thread_route_id(thread_id: &str) -> &str {
    let route_id = thread_id
        .strip_prefix(LEGACY_THREAD_PREFIX)
        .unwrap_or(thread_id);
    if route_id.is_empty() {
        thread_id
    } else {
        route_id
    }
}

/// Maps an absent workspace to the reserved detached-thread route segment.
///
/// A present workspace is preserved exactly, including an empty string or the
/// reserved underscore. Validation of workspace identities belongs elsewhere.
#[must_use]
pub fn thread_workspace_route_id(workspace_id: Option<&str>) -> &str {
    workspace_id.unwrap_or(DETACHED_WORKSPACE_ROUTE_ID)
}

/// Restores the optional workspace identity represented by a route segment.
///
/// The underscore is reserved for a detached workspace, so it maps to
/// `None`; every other segment is borrowed unchanged.
#[must_use]
pub fn thread_workspace_id(route_workspace_id: &str) -> Option<&str> {
    if route_workspace_id == DETACHED_WORKSPACE_ROUTE_ID {
        None
    } else {
        Some(route_workspace_id)
    }
}

/// The two route fields retained by a route instance that may own navigation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ThreadRouteOwner<'a> {
    /// The route surface that rendered the owner, if it has one.
    pub route_id: Option<&'a str>,
    /// The thread route parameter the owner represents.
    pub thread_route_id: &'a str,
}

impl<'a> ThreadRouteOwner<'a> {
    /// Builds an owner from its optional surface and required thread segment.
    #[must_use]
    pub const fn new(route_id: Option<&'a str>, thread_route_id: &'a str) -> Self {
        Self {
            route_id,
            thread_route_id,
        }
    }
}

/// The route fields currently targeted by the rendered page or a navigation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ThreadRouteTarget<'a> {
    /// The target route surface, if present.
    pub route_id: Option<&'a str>,
    /// The target thread parameter, if the target has one.
    pub thread_param: Option<&'a str>,
}

impl<'a> ThreadRouteTarget<'a> {
    /// Builds a target from its optional surface and optional thread segment.
    #[must_use]
    pub const fn new(route_id: Option<&'a str>, thread_param: Option<&'a str>) -> Self {
        Self {
            route_id,
            thread_param,
        }
    }
}

/// Returns whether a route instance still owns the navigation target.
///
/// Both fields are required to match exactly. In particular, a missing target
/// thread never matches the owner's required thread, and a different route
/// surface never matches merely because its thread parameter is equal.
#[must_use]
pub fn thread_route_owns_target(
    owner: ThreadRouteOwner<'_>,
    target: ThreadRouteTarget<'_>,
) -> bool {
    target.route_id == owner.route_id && target.thread_param == Some(owner.thread_route_id)
}

/// Returns whether a status is one of the two resting statuses.
#[must_use]
pub fn is_resting_status(status: &str) -> bool {
    status == COMPLETE_STATUS || status == IDLE_STATUS
}

/// Returns whether the status is non-resting and therefore remains in the
/// working/pinned presentation set.
#[must_use]
pub fn thread_is_working(status: &str) -> bool {
    !is_resting_status(status)
}

/// Returns whether the status is the current or legacy exact failure value.
#[must_use]
pub fn is_failed_status(status: &str) -> bool {
    status == FAILED_STATUS || status == LEGACY_FAILED_STATUS
}

/// Returns whether a thread's minimal status value reports a failure.
#[must_use]
pub fn thread_failed(status: &str) -> bool {
    is_failed_status(status)
}

/// Returns whether the status is the exact successful-completion value.
#[must_use]
pub fn thread_completed(status: &str) -> bool {
    status == COMPLETE_STATUS
}

/// Returns whether the status represents active Forge-owned work.
///
/// Every non-resting status is working, except the two exact failure values;
/// this preserves the legacy behavior for unknown and waiting statuses.
#[must_use]
pub fn thread_has_active_work(status: &str) -> bool {
    thread_is_working(status) && !thread_failed(status)
}

const MILLISECONDS_PER_MINUTE: u64 = 60_000;
const MILLISECONDS_PER_HOUR: u64 = 3_600_000;
const MILLISECONDS_PER_DAY: u64 = 86_400_000;
const MILLISECONDS_PER_TWO_DAYS: u64 = 172_800_000;

/// Computes a nonnegative elapsed duration without allowing signed overflow.
fn elapsed_milliseconds(now_ms: i64, activity_ms: i64) -> u64 {
    match now_ms.checked_sub(activity_ms) {
        Some(elapsed_ms) => u64::try_from(elapsed_ms).unwrap_or(0),
        // When subtraction overflows, the ordering of the operands still
        // distinguishes a logically negative duration from a very large
        // positive one. Saturate the latter so formatting remains total.
        None if now_ms < activity_ms => 0,
        None => u64::MAX,
    }
}

/// Formats an already-parsed activity time for the compact recent-thread row.
///
/// `activity_ms` and `now_ms` are signed Unix epoch milliseconds. Negative
/// elapsed time (including a clock that has moved backwards) is clamped to
/// zero. Boundaries match the legacy formatter: under one minute is `Just
/// now`, then integer minutes/hours, exactly one day through under two days is
/// `Yesterday`, and two days or more is integer days. No timestamp parsing or
/// timezone conversion occurs here.
#[must_use]
pub fn format_recent_thread_time(activity_ms: i64, now_ms: i64) -> String {
    let elapsed_ms = elapsed_milliseconds(now_ms, activity_ms);

    if elapsed_ms < MILLISECONDS_PER_MINUTE {
        return String::from("Just now");
    }
    if elapsed_ms < MILLISECONDS_PER_HOUR {
        return format!("{} min ago", elapsed_ms / MILLISECONDS_PER_MINUTE);
    }
    if elapsed_ms < MILLISECONDS_PER_DAY {
        return format!("{} hr ago", elapsed_ms / MILLISECONDS_PER_HOUR);
    }
    if elapsed_ms < MILLISECONDS_PER_TWO_DAYS {
        return String::from("Yesterday");
    }
    format!("{} days ago", elapsed_ms / MILLISECONDS_PER_DAY)
}
