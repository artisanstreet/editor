//! Focused dependency-free coverage for engine usage refresh claim ownership.

#[path = "../../modules/frontend/src/usage_refresh_claims.rs"]
mod usage_refresh_claims;

use usage_refresh_claims::{
    EngineUsageRefreshClaim, EngineUsageRefreshError, EngineUsageRefreshState,
};

fn engine_ids(claims: &[EngineUsageRefreshClaim]) -> Vec<&str> {
    claims
        .iter()
        .map(EngineUsageRefreshClaim::engine_id)
        .collect()
}

fn claim_ids(claims: &[EngineUsageRefreshClaim]) -> Vec<u64> {
    claims
        .iter()
        .map(EngineUsageRefreshClaim::claim_id)
        .collect()
}

#[test]
fn a_new_state_is_idle_with_no_active_engines() {
    let state = EngineUsageRefreshState::new();

    assert!(state.is_idle());
    assert!(state.active_engine_ids().is_empty());
}

#[test]
fn claim_deduplicates_ids_and_preserves_first_request_order() {
    let mut state = EngineUsageRefreshState::new();

    let claimed = state
        .claim(["engine-b", "engine-a", "engine-b", "engine-c", "engine-a"])
        .expect("initial claim succeeds");

    assert_eq!(engine_ids(&claimed), ["engine-b", "engine-a", "engine-c"]);
    assert_eq!(claim_ids(&claimed), [0, 1, 2]);
    assert_eq!(claimed[0].generation(), claimed[0].claim_id());
    assert_eq!(
        state.active_engine_ids(),
        ["engine-b", "engine-a", "engine-c"]
    );
    assert!(!state.is_idle());
}

#[test]
fn overlapping_claims_skip_active_engines_and_keep_new_ids_ordered() {
    let mut state = EngineUsageRefreshState::new();
    state
        .claim(["engine-a", "engine-b"])
        .expect("first claim succeeds");

    let overlapping = state
        .claim(["engine-b", "engine-c", "engine-a", "engine-d", "engine-c"])
        .expect("overlapping claim succeeds");

    assert_eq!(engine_ids(&overlapping), ["engine-c", "engine-d"]);
    assert_eq!(claim_ids(&overlapping), [2, 3]);
    assert_eq!(
        state.active_engine_ids(),
        ["engine-a", "engine-b", "engine-c", "engine-d"]
    );
}

#[test]
fn an_empty_or_already_claimed_request_leaves_state_and_generation_unchanged() {
    let mut state = EngineUsageRefreshState::new();
    let initial = state.claim(["engine-a"]).expect("initial claim succeeds");
    let active_before = state.active_engine_ids();

    assert!(state.claim(std::iter::empty::<&str>()).unwrap().is_empty());
    assert!(
        state
            .claim(["engine-a", "engine-a"])
            .expect("already claimed request succeeds")
            .is_empty()
    );
    assert_eq!(state.active_engine_ids(), active_before);

    let next = state.claim(["engine-b"]).expect("next claim succeeds");
    assert_eq!(claim_ids(&initial), [0]);
    assert_eq!(claim_ids(&next), [1]);
}

#[test]
fn stale_release_cannot_remove_a_reacquired_current_claim() {
    let mut state = EngineUsageRefreshState::new();
    let first = state
        .claim(["engine-a"])
        .expect("first claim succeeds")
        .pop()
        .expect("one claim exists");

    assert!(state.release(&first));
    assert!(state.is_idle());

    let second = state
        .claim(["engine-a"])
        .expect("reacquisition succeeds")
        .pop()
        .expect("one reacquired claim exists");
    assert_eq!(second.engine_id(), first.engine_id());
    assert_eq!(second.claim_id(), 1);

    assert!(!state.release(&first), "stale cleanup must be ignored");
    assert_eq!(state.active_engine_ids(), ["engine-a"]);
    assert!(state.release(&second));
    assert!(!state.release(&second), "a current release is idempotent");
    assert!(state.is_idle());
}

#[test]
fn release_all_removes_matching_claims_and_preserves_monotonic_reacquisition() {
    let mut state = EngineUsageRefreshState::new();
    let claims = state
        .claim(["engine-a", "engine-b", "engine-c"])
        .expect("initial claims succeed");

    assert_eq!(state.release_all(&claims), 3);
    assert!(state.is_idle());
    assert_eq!(state.release_all(&claims), 0);

    let reacquired = state.claim(["engine-b"]).expect("reacquisition succeeds");
    assert_eq!(claim_ids(&reacquired), [3]);
    assert_eq!(state.active_engine_ids(), ["engine-b"]);
}

#[test]
fn release_all_counts_current_matches_once_and_ignores_stale_duplicates() {
    let mut state = EngineUsageRefreshState::new();
    let first = state
        .claim(["engine-a", "engine-b"])
        .expect("initial claims succeed");
    assert!(state.release(&first[1]));
    let replacement = state
        .claim(["engine-b"])
        .expect("replacement claim succeeds")
        .pop()
        .expect("replacement exists");

    let cleanup = [
        first[0].clone(),
        first[1].clone(),
        replacement.clone(),
        replacement,
    ];
    assert_eq!(state.release_all(&cleanup), 2);
    assert!(state.is_idle());
}

#[test]
fn exhaustion_assigns_u64_max_once_then_refuses_without_wrapping_or_partial_state() {
    let mut state = EngineUsageRefreshState::from_next_claim_id(u64::MAX - 1);

    let penultimate = state
        .claim(["engine-penultimate"])
        .expect("penultimate generation succeeds");
    assert_eq!(claim_ids(&penultimate), [u64::MAX - 1]);

    let final_claim = state
        .claim(["engine-final"])
        .expect("u64::MAX remains a usable generation");
    assert_eq!(claim_ids(&final_claim), [u64::MAX]);
    assert_eq!(
        state.active_engine_ids(),
        ["engine-penultimate", "engine-final"]
    );

    let active_before = state.active_engine_ids();
    assert_eq!(
        state.claim(["engine-after"]),
        Err(EngineUsageRefreshError::GenerationExhausted)
    );
    assert_eq!(state.active_engine_ids(), active_before);

    assert!(
        state
            .claim(["engine-final", "engine-final"])
            .expect("an already active engine needs no new generation")
            .is_empty()
    );
    assert!(!state.is_idle());
}

#[test]
fn an_overflowing_multi_engine_claim_is_atomic() {
    let mut state = EngineUsageRefreshState::from_next_claim_id(u64::MAX);

    assert_eq!(
        state.claim(["engine-a", "engine-b"]),
        Err(EngineUsageRefreshError::GenerationExhausted)
    );
    assert!(state.is_idle());
    assert!(state.active_engine_ids().is_empty());

    let final_claim = state
        .claim(["engine-a"])
        .expect("the final generation can still be allocated");
    assert_eq!(claim_ids(&final_claim), [u64::MAX]);
}
