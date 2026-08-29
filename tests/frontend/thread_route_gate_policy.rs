//! Dependency-free coverage for the thread-route gate policy.
//!
//! The implementation is included directly so these tests exercise the leaf
//! without Cargo, Bazel, transport, protocol, Svelte, or a DOM runtime.

#![forbid(unsafe_code)]

#[path = "../../modules/frontend/src/thread_route_gate_policy.rs"]
mod thread_route_gate_policy;

use thread_route_gate_policy::{
    LOADING_THREAD_ARIA_LABEL, LOADING_THREAD_ROLE, ThreadRouteGate, ThreadRouteGateAction,
    ThreadRouteGateLoadAdmission, ThreadRouteGatePresentation, ThreadRouteGateRender,
    ThreadRouteGateTransition,
};

#[derive(Clone, Debug, Eq, PartialEq)]
struct Snapshot {
    id: String,
}

impl Snapshot {
    fn new(id: &str) -> Self {
        Self { id: id.to_owned() }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Measurement {
    duration_ms: u64,
    reason: &'static str,
}

impl Measurement {
    const fn new(duration_ms: u64, reason: &'static str) -> Self {
        Self {
            duration_ms,
            reason,
        }
    }
}

fn snapshot(id: &str) -> Snapshot {
    Snapshot::new(id)
}

fn measurement(duration_ms: u64, reason: &'static str) -> Measurement {
    Measurement::new(duration_ms, reason)
}

#[test]
fn initialization_distinguishes_cached_and_uncached_draft_states() {
    let cases = [
        (
            None,
            false,
            true,
            false,
            ThreadRouteGateRender::LoadingIndicator,
        ),
        (
            None,
            true,
            true,
            true,
            ThreadRouteGateRender::LoadingIndicator,
        ),
        (
            Some(snapshot("cached")),
            false,
            false,
            false,
            ThreadRouteGateRender::OpenedRoute,
        ),
        (
            Some(snapshot("draft-cached")),
            true,
            false,
            true,
            ThreadRouteGateRender::OpenedRoute,
        ),
    ];

    for (cached, draft_handoff, loading, settled, render) in cases {
        let gate = ThreadRouteGate::<Snapshot, Measurement>::new(cached, draft_handoff);

        assert_eq!(gate.is_loading(), loading);
        assert_eq!(gate.is_visually_settled(), settled);
        assert_eq!(gate.render(), render);
        assert_eq!(gate.failure(), None);
        assert_eq!(gate.visual_settlement(), None);
    }
}

#[test]
fn load_admission_identifies_initial_open_retry_and_not_admitted_states() {
    let cold = ThreadRouteGate::<Snapshot, Measurement>::new(None, false);
    assert_eq!(
        cold.load_admission(),
        ThreadRouteGateLoadAdmission::InitialOpen
    );
    assert!(cold.load_admission().is_admitted());
    assert!(cold.load_admission().is_initial_open());

    let mut failed = ThreadRouteGate::<Snapshot, Measurement>::new(None, false);
    let _ = failed.open_failure("not found");
    assert_eq!(failed.load_admission(), ThreadRouteGateLoadAdmission::Retry);
    assert!(failed.load_admission().is_retry());

    let cached = ThreadRouteGate::<Snapshot, Measurement>::new(Some(snapshot("cached")), false);
    assert_eq!(
        cached.admit_load(),
        ThreadRouteGateLoadAdmission::NotAdmitted
    );
    assert!(!cached.admit_load().is_admitted());
    assert_eq!(cached.snapshot(), cached.thread_open());
    assert!(!cached.draft_handoff());
    assert!(!cached.has_failure());
}

#[test]
fn every_load_begin_clears_failure_and_settlement_without_inventing_a_snapshot() {
    let first = snapshot("cached");
    let mut gate = ThreadRouteGate::new(Some(first.clone()), false);
    let _ = gate.open_failure("old failure");
    let _ = gate.begin_load();
    let _ = gate.reveal(measurement(12, "stable"));
    let _ = gate.open_failure("new failure");

    let transition = gate.begin_load();

    assert_eq!(
        transition,
        ThreadRouteGateTransition::LoadBegan {
            admission: ThreadRouteGateLoadAdmission::NotAdmitted,
        }
    );
    assert!(gate.is_loading());
    assert_eq!(gate.failure(), None);
    assert_eq!(gate.visual_settlement(), None);
    assert!(!gate.is_visually_settled());
    assert_eq!(gate.thread_open(), Some(&first));
    assert_eq!(gate.render(), ThreadRouteGateRender::OpenedRoute);
}

#[test]
fn draft_handoff_is_restored_by_repeated_load_begin() {
    let mut gate = ThreadRouteGate::<Snapshot, Measurement>::new(None, true);

    assert!(gate.is_loading());
    assert!(gate.is_visually_settled());
    let _ = gate.open_failure("first failure");
    assert!(!gate.is_loading());

    let transition = gate.retry();

    assert_eq!(
        transition,
        ThreadRouteGateTransition::RetryBegan {
            admission: ThreadRouteGateLoadAdmission::Retry,
        }
    );
    assert!(gate.is_loading());
    assert_eq!(gate.failure(), None);
    assert_eq!(gate.visual_settlement(), None);
    assert!(gate.is_visually_settled());
    assert_eq!(gate.thread_open(), None);
}

#[test]
fn success_stores_the_snapshot_and_clears_loading() {
    let loaded = snapshot("loaded");
    let mut gate = ThreadRouteGate::<Snapshot, Measurement>::new(None, false);
    let begin: thread_route_gate_policy::ThreadRouteGateEvent<Snapshot, Measurement> =
        ThreadRouteGateAction::begin_load();
    assert_eq!(
        gate.apply(begin),
        ThreadRouteGateTransition::LoadBegan {
            admission: ThreadRouteGateLoadAdmission::InitialOpen,
        }
    );

    assert_eq!(
        gate.apply(ThreadRouteGateAction::open_success(loaded.clone())),
        ThreadRouteGateTransition::OpenSucceeded
    );
    assert_eq!(gate.thread_open(), Some(&loaded));
    assert!(!gate.is_loading());
    assert_eq!(gate.render(), ThreadRouteGateRender::OpenedRoute);
}

#[test]
fn failure_preserves_exact_empty_and_unicode_messages_and_clears_loading() {
    for message in ["", "open failed — 失敗 🚀\nexact spacing"] {
        let mut gate = ThreadRouteGate::<Snapshot, Measurement>::new(None, false);
        let _ = gate.begin_load();

        assert_eq!(
            gate.apply(ThreadRouteGateAction::open_failure(message)),
            ThreadRouteGateTransition::OpenFailed
        );
        assert_eq!(gate.failure(), Some(message));
        assert!(!gate.is_loading());
        assert_eq!(gate.render(), ThreadRouteGateRender::FailureRetry);
    }
}

#[test]
fn retry_clears_the_failure_then_a_second_failure_is_rendered_again() {
    let mut gate = ThreadRouteGate::<Snapshot, Measurement>::new(None, false);
    let _ = gate.open_failure("first");
    assert_eq!(gate.render(), ThreadRouteGateRender::FailureRetry);

    assert_eq!(
        gate.apply(ThreadRouteGateAction::retry()),
        ThreadRouteGateTransition::RetryBegan {
            admission: ThreadRouteGateLoadAdmission::Retry,
        }
    );
    assert!(gate.is_loading());
    assert_eq!(gate.failure(), None);
    assert_eq!(gate.render(), ThreadRouteGateRender::LoadingIndicator);

    let _ = gate.open_failure("second");
    assert_eq!(gate.failure(), Some("second"));
    assert_eq!(gate.render(), ThreadRouteGateRender::FailureRetry);
}

#[test]
fn first_reveal_preserves_measurement_and_repeated_reveal_is_a_no_op() {
    let first = measurement(37, "deadline");
    let second = measurement(99, "stable");
    let mut gate = ThreadRouteGate::<Snapshot, Measurement>::new(Some(snapshot("open")), false);

    let revealed = gate.apply(ThreadRouteGateAction::reveal(first.clone()));
    assert_eq!(revealed, ThreadRouteGateTransition::Revealed);
    assert!(revealed.is_revealed());
    assert!(!revealed.is_no_op());
    assert!(gate.is_visually_settled());
    assert_eq!(gate.visual_settlement(), Some(&first));

    let ignored = gate.reveal(second);
    assert_eq!(ignored, ThreadRouteGateTransition::RevealIgnored);
    assert!(ignored.is_no_op());
    assert!(!ignored.is_revealed());
    assert_eq!(gate.visual_settlement(), Some(&first));
    assert_eq!(
        gate.visual_settlement().map(|value| value.duration_ms),
        Some(37)
    );
    assert_eq!(
        gate.visual_settlement().map(|value| value.reason),
        Some("deadline")
    );
}

#[test]
fn load_begin_clears_the_first_measurement_so_a_new_reveal_can_settle() {
    let first = measurement(20, "stable");
    let replacement = measurement(44, "measurement_unavailable");
    let mut gate = ThreadRouteGate::<Snapshot, Measurement>::new(Some(snapshot("open")), false);
    let _ = gate.reveal(first);
    assert!(gate.is_visually_settled());

    let _ = gate.begin_load();
    assert!(!gate.is_visually_settled());
    assert_eq!(gate.visual_settlement(), None);
    assert_eq!(
        gate.reveal(replacement.clone()),
        ThreadRouteGateTransition::Revealed
    );
    assert_eq!(gate.visual_settlement(), Some(&replacement));
}

#[test]
fn render_precedence_keeps_open_route_above_loading_and_failure() {
    let mut gate = ThreadRouteGate::<Snapshot, Measurement>::new(Some(snapshot("open")), false);
    let _ = gate.begin_load();
    let _ = gate.open_failure("hidden while route exists");

    assert_eq!(gate.render(), ThreadRouteGateRender::OpenedRoute);
    assert_eq!(gate.failure(), Some("hidden while route exists"));
    assert!(!gate.is_loading());

    let mut cold = ThreadRouteGate::<Snapshot, Measurement>::new(None, false);
    let _ = cold.open_failure("loading loses to failure");
    assert_eq!(cold.render(), ThreadRouteGateRender::FailureRetry);
    let _ = cold.begin_load();
    assert_eq!(cold.render(), ThreadRouteGateRender::LoadingIndicator);

    let empty = ThreadRouteGate::<Snapshot, Measurement>::new(None, false);
    assert_eq!(empty.render(), ThreadRouteGateRender::LoadingIndicator);
}

#[test]
fn render_projection_exposes_every_precedence_branch_in_order() {
    use thread_route_gate_policy::thread_route_gate_render;

    assert_eq!(
        thread_route_gate_render(false, false, false),
        ThreadRouteGateRender::EmptyFallback
    );
    assert_eq!(
        thread_route_gate_render(false, false, true),
        ThreadRouteGateRender::FailureRetry
    );
    assert_eq!(
        thread_route_gate_render(false, true, true),
        ThreadRouteGateRender::LoadingIndicator
    );
    assert_eq!(
        thread_route_gate_render(true, true, true),
        ThreadRouteGateRender::OpenedRoute
    );

    assert!(ThreadRouteGateRender::FailureRetry.is_failure_retry());
    assert!(ThreadRouteGateRender::EmptyFallback.is_empty_fallback());
    let render_state: thread_route_gate_policy::ThreadRouteGateRenderState =
        thread_route_gate_render(false, false, false);
    assert!(render_state.is_empty_fallback());
}

#[test]
fn state_aliases_and_default_keep_the_cold_non_draft_initial_state() {
    let state: thread_route_gate_policy::ThreadRouteGateState<Snapshot, Measurement> =
        ThreadRouteGate::default();
    let policy: thread_route_gate_policy::ThreadRouteGatePolicy<Snapshot, Measurement> =
        ThreadRouteGate::default();

    assert!(state.is_loading());
    assert!(!state.draft_handoff());
    assert_eq!(policy.render(), ThreadRouteGateRender::LoadingIndicator);
}

#[test]
fn presentation_matches_cover_inert_busy_and_callback_flags() {
    let expected_cold = ThreadRouteGatePresentation {
        render: ThreadRouteGateRender::LoadingIndicator,
        route_mounted: false,
        loading_indicator_visible: true,
        cover_visible: false,
        route_opacity_hidden: false,
        route_pointer_events_none: false,
        route_aria_hidden: false,
        route_inert: false,
        aria_busy: None,
        visual_settlement_callback_attached: false,
        loading_indicator_role: Some(LOADING_THREAD_ROLE),
        loading_indicator_aria_label: Some(LOADING_THREAD_ARIA_LABEL),
        visual_settlement_present: false,
    };
    let cold = ThreadRouteGate::<Snapshot, Measurement>::new(None, false);
    assert_eq!(cold.presentation(), expected_cold);

    let open = ThreadRouteGate::<Snapshot, Measurement>::new(Some(snapshot("open")), false);
    let covered = open.presentation();
    assert_eq!(covered.render, ThreadRouteGateRender::OpenedRoute);
    assert!(covered.route_mounted);
    assert!(covered.loading_indicator_visible);
    assert!(covered.cover_visible);
    assert!(covered.route_opacity_hidden);
    assert!(covered.route_pointer_events_none);
    assert!(covered.route_aria_hidden);
    assert!(covered.route_inert);
    assert_eq!(covered.aria_busy, Some(true));
    assert!(covered.visual_settlement_callback_attached);

    let draft = ThreadRouteGate::<Snapshot, Measurement>::new(Some(snapshot("draft")), true);
    let revealed = draft.presentation();
    assert_eq!(revealed.aria_busy, Some(false));
    assert!(!revealed.cover_visible);
    assert!(!revealed.route_inert);
    assert!(!revealed.route_aria_hidden);
    assert!(!revealed.visual_settlement_callback_attached);
    assert!(!revealed.loading_indicator_visible);

    let mut settled =
        ThreadRouteGate::<Snapshot, Measurement>::new(Some(snapshot("settled")), false);
    let _ = settled.reveal(measurement(18, "stable"));
    let settled_view = settled.presentation();
    assert_eq!(settled_view.aria_busy, Some(false));
    assert!(!settled_view.cover_visible);
    assert!(!settled_view.route_inert);
    assert!(!settled_view.route_aria_hidden);
    assert!(settled_view.visual_settlement_callback_attached);
    assert!(!settled_view.loading_indicator_visible);
    assert!(settled_view.visual_settlement_present);
}

#[test]
fn failure_and_empty_branches_have_no_route_cover_or_busy_attribute() {
    let mut failed = ThreadRouteGate::<Snapshot, Measurement>::new(None, false);
    let _ = failed.open_failure("");
    let failure = failed.presentation();
    assert_eq!(failure.render, ThreadRouteGateRender::FailureRetry);
    assert!(!failure.route_mounted);
    assert!(!failure.cover_visible);
    assert!(!failure.loading_indicator_visible);
    assert_eq!(failure.aria_busy, None);
    assert_eq!(failure.loading_indicator_role, None);
    assert_eq!(failure.loading_indicator_aria_label, None);

    let mut empty = ThreadRouteGate::<Snapshot, Measurement>::new(None, false);
    let _ = empty.open_failure("temporary");
    let _ = empty.begin_load();
    let _ = empty.open_failure("temporary");
    let _ = empty.retry();
    let _ = empty.open_failure("temporary");
    assert_eq!(
        empty.presentation().render,
        ThreadRouteGateRender::FailureRetry
    );
}
