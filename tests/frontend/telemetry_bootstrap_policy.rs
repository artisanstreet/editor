//! Focused, dependency-free coverage for renderer telemetry bootstrap policy.

#[allow(dead_code)]
#[path = "../../modules/frontend/src/runtime_surface.rs"]
mod runtime_surface;
#[path = "../../modules/frontend/src/telemetry_bootstrap_policy.rs"]
mod telemetry_bootstrap_policy;

use runtime_surface::RuntimeSurface;
use telemetry_bootstrap_policy::{
    CaptureOutcome, EDITOR_SESSION_STARTED_EVENT, MAX_TIME_TO_READY_MS, RendererSignal,
    TelemetryBootstrapAction, TelemetryBootstrapPolicy, TelemetryBootstrapState,
    editor_session_started_intent,
};

#[test]
fn desktop_ready_admits_the_exact_local_renderer_event() {
    let mut policy = TelemetryBootstrapPolicy::new(1_u64, 1_000, RuntimeSurface::Desktop);

    let transition = policy.observe(RendererSignal::Ready { ready_at_ms: 1_250 });

    assert_eq!(transition.state, TelemetryBootstrapState::CaptureAdmitted);
    assert_eq!(transition.state(), TelemetryBootstrapState::CaptureAdmitted);
    assert_eq!(policy.state(), TelemetryBootstrapState::CaptureAdmitted);
    assert_eq!(policy.renderer_started_at_ms(), 1_000);
    assert_eq!(policy.surface(), RuntimeSurface::Desktop);
    assert!(transition.action().is_capture());
    assert_eq!(
        transition
            .action
            .capture_intent()
            .map(|intent| intent.event),
        Some(EDITOR_SESSION_STARTED_EVENT)
    );
    assert_eq!(
        transition.action,
        TelemetryBootstrapAction::Capture(editor_session_started_intent(
            RuntimeSurface::Desktop,
            1_000,
            1_250,
        ))
    );
    assert_eq!(
        transition
            .action
            .capture_intent()
            .map(|intent| intent.forge_connection),
        Some("local")
    );
    assert_eq!(
        transition
            .action
            .capture_intent()
            .map(|intent| intent.surface),
        Some("desktop_renderer")
    );
}

#[test]
fn browser_ready_admits_the_exact_remote_renderer_event() {
    let mut policy = TelemetryBootstrapPolicy::new(2_u64, 10_000, RuntimeSurface::Browser);

    let transition = policy.observe(RendererSignal::Ready {
        ready_at_ms: 10_500,
    });

    let TelemetryBootstrapAction::Capture(intent) = transition.action else {
        panic!("first ready signal must admit capture");
    };
    assert_eq!(intent.event, "editor_session_started");
    assert_eq!(intent.forge_connection, "remote");
    assert_eq!(intent.surface, "browser_renderer");
    assert_eq!(intent.time_to_ready_ms, 500);
}

#[test]
fn ready_duration_clamps_negative_and_oversized_values_at_exact_boundaries() {
    let cases = [
        (1_000, 999, 0),
        (1_000, 1_000, 0),
        (1_000, 1_001, 1),
        (1_000, 601_000, 600_000),
        (1_000, 601_001, 600_000),
    ];

    for (renderer_started_at_ms, ready_at_ms, expected) in cases {
        assert_eq!(
            editor_session_started_intent(
                RuntimeSurface::Browser,
                renderer_started_at_ms,
                ready_at_ms,
            )
            .time_to_ready_ms,
            expected,
            "started={renderer_started_at_ms}, ready={ready_at_ms}"
        );
    }
}

#[test]
fn signed_timestamp_extremes_still_use_the_inclusive_clamp() {
    assert_eq!(
        editor_session_started_intent(RuntimeSurface::Desktop, i64::MIN, i64::MAX).time_to_ready_ms,
        MAX_TIME_TO_READY_MS
    );
    assert_eq!(
        editor_session_started_intent(RuntimeSurface::Desktop, i64::MAX, i64::MIN).time_to_ready_ms,
        0
    );
}

#[test]
fn duplicate_ready_and_connection_retry_signals_never_reemit_capture() {
    let mut policy = TelemetryBootstrapPolicy::new(3_u64, 1_000, RuntimeSurface::Desktop);

    let first = policy.observe(RendererSignal::Ready { ready_at_ms: 1_100 });
    assert!(first.action.is_capture());

    for signal in [
        RendererSignal::Ready { ready_at_ms: 1_101 },
        RendererSignal::ConnectionRetry,
        RendererSignal::ConnectionRetry,
        RendererSignal::Ready { ready_at_ms: 1_102 },
    ] {
        let transition = policy.observe(signal);
        assert_eq!(transition.action, TelemetryBootstrapAction::NoOp);
        assert_eq!(transition.state, TelemetryBootstrapState::CaptureAdmitted);
    }

    let captured = policy.settle_capture(CaptureOutcome::Succeeded);
    assert_eq!(captured.state, TelemetryBootstrapState::Captured);
    assert!(captured.state.is_ready());
    assert!(captured.state.capture_is_terminal());
    assert_eq!(captured.action, TelemetryBootstrapAction::NoOp);
}

#[test]
fn retry_before_ready_is_ignored_but_does_not_block_the_first_ready_capture() {
    let mut policy = TelemetryBootstrapPolicy::new(4_u64, 5_000, RuntimeSurface::Browser);

    assert_eq!(
        policy.observe(RendererSignal::ConnectionRetry),
        telemetry_bootstrap_policy::TelemetryBootstrapTransition {
            state: TelemetryBootstrapState::AwaitingReady,
            action: TelemetryBootstrapAction::NoOp,
        }
    );
    assert!(
        policy
            .observe(RendererSignal::Ready { ready_at_ms: 5_050 })
            .action
            .is_capture()
    );
}

#[test]
fn capture_failure_is_absorbed_without_readiness_failure_or_retry_storm() {
    let mut policy = TelemetryBootstrapPolicy::new(5_u64, 2_000, RuntimeSurface::Desktop);
    assert!(
        policy
            .observe(RendererSignal::Ready { ready_at_ms: 2_010 })
            .action
            .is_capture()
    );

    let failed = policy.settle_capture_result::<&str>(&Err("telemetry unavailable"));
    assert_eq!(failed.action, TelemetryBootstrapAction::NoOp);
    assert_eq!(
        failed.state,
        TelemetryBootstrapState::CaptureFailureAbsorbed
    );
    assert!(failed.state.is_ready());
    assert!(failed.state.capture_is_terminal());

    for signal in [
        RendererSignal::Ready { ready_at_ms: 2_011 },
        RendererSignal::ConnectionRetry,
    ] {
        let transition = policy.observe(signal);
        assert_eq!(transition.action, TelemetryBootstrapAction::NoOp);
        assert_eq!(
            transition.state,
            TelemetryBootstrapState::CaptureFailureAbsorbed
        );
    }
    assert_eq!(
        policy.settle_capture(CaptureOutcome::Failed).action,
        TelemetryBootstrapAction::NoOp
    );
}

#[test]
fn separate_lifecycle_identities_admit_independent_single_captures() {
    let mut first = TelemetryBootstrapPolicy::new(10_u64, 10_000, RuntimeSurface::Desktop);
    let mut second = TelemetryBootstrapPolicy::new(11_u64, 20_000, RuntimeSurface::Browser);

    let first_capture = first.observe(RendererSignal::Ready {
        ready_at_ms: 10_100,
    });
    let second_capture = second.observe(RendererSignal::Ready {
        ready_at_ms: 20_200,
    });

    assert_eq!(first.lifecycle_id().get(), 10);
    assert_eq!(second.lifecycle_id().get(), 11);
    assert!(first_capture.action.is_capture());
    assert!(second_capture.action.is_capture());
    assert_eq!(
        first_capture.action.capture_intent().unwrap().surface,
        "desktop_renderer"
    );
    assert_eq!(
        second_capture.action.capture_intent().unwrap().surface,
        "browser_renderer"
    );
    assert_eq!(
        first_capture
            .action
            .capture_intent()
            .unwrap()
            .time_to_ready_ms,
        100
    );
    assert_eq!(
        second_capture
            .action
            .capture_intent()
            .unwrap()
            .time_to_ready_ms,
        200
    );
}
