//! Exhaustive direct tests for terminal-transcript consumption policy.

#![allow(dead_code)]

#[path = "../../modules/backend/src/terminal_transcript_consumption_policy.rs"]
mod terminal_transcript_consumption_policy;

use terminal_transcript_consumption_policy::{
    ConditionalDeleteTranscriptAction, NativeSubagentBinding, PendingTranscriptObservation,
    RootRunObservation, RootRunStatus, TerminalTranscriptConsumptionDecision,
    TerminalTranscriptConsumptionInput, TerminalTranscriptConsumptionPolicy,
    TerminalTranscriptInboxObservation, UnprocessedTranscriptGuard,
    terminal_transcript_consumption_decision,
};

const OBSERVATION_ID: &str = "observation:🦀/exact";
const ROOT_RUN_ID: &str = "root-run:α";
const ENGINE_ID: &str = "engine:provider/one";
const NATIVE_THREAD_ID: &str = "native-thread:child/二";

fn pending() -> TerminalTranscriptInboxObservation {
    TerminalTranscriptInboxObservation::pending(PendingTranscriptObservation::new(
        ROOT_RUN_ID,
        ENGINE_ID,
        NATIVE_THREAD_ID,
    ))
}

fn root(status: RootRunStatus) -> RootRunObservation {
    RootRunObservation::new(status)
}

fn binding(
    engine_id: &str,
    root_run_id: &str,
    agent_native_thread_id: &str,
) -> NativeSubagentBinding {
    NativeSubagentBinding::new(engine_id, root_run_id, agent_native_thread_id)
}

fn input(
    inbox: TerminalTranscriptInboxObservation,
    root_run: Option<RootRunObservation>,
    bindings: Vec<NativeSubagentBinding>,
) -> TerminalTranscriptConsumptionInput {
    TerminalTranscriptConsumptionInput::new(OBSERVATION_ID, inbox, root_run, bindings)
}

fn expected_delete() -> TerminalTranscriptConsumptionDecision {
    TerminalTranscriptConsumptionDecision::DeleteUnprocessed(
        ConditionalDeleteTranscriptAction::new(OBSERVATION_ID),
    )
}

#[test]
fn root_status_vocabulary_preserves_exact_active_and_settled_spellings() {
    let expected = [
        (RootRunStatus::Queued, "queued", true),
        (RootRunStatus::Running, "running", true),
        (RootRunStatus::Waiting, "waiting", true),
        (RootRunStatus::Completed, "completed", false),
        (RootRunStatus::Failed, "failed", false),
        (RootRunStatus::Cancelled, "cancelled", false),
        (RootRunStatus::Interrupted, "interrupted", false),
    ];

    assert_eq!(
        RootRunStatus::ACTIVE,
        [
            RootRunStatus::Queued,
            RootRunStatus::Running,
            RootRunStatus::Waiting,
        ]
    );
    assert_eq!(
        RootRunStatus::SETTLED,
        [
            RootRunStatus::Completed,
            RootRunStatus::Failed,
            RootRunStatus::Cancelled,
            RootRunStatus::Interrupted,
        ]
    );
    assert_eq!(
        RootRunStatus::ALL,
        [
            RootRunStatus::Queued,
            RootRunStatus::Running,
            RootRunStatus::Waiting,
            RootRunStatus::Completed,
            RootRunStatus::Failed,
            RootRunStatus::Cancelled,
            RootRunStatus::Interrupted,
        ]
    );

    for (status, spelling, active) in expected {
        assert_eq!(status.as_str(), spelling);
        assert_eq!(RootRunStatus::from_runtime(spelling), status);
        assert_eq!(status.is_active(), active);
        assert_eq!(status.is_settled(), !active);
    }
}

#[test]
fn missing_and_already_processed_inbox_facts_are_noops() {
    for inbox in [
        TerminalTranscriptInboxObservation::missing(),
        TerminalTranscriptInboxObservation::already_processed(),
    ] {
        for status in RootRunStatus::ALL {
            let decision = terminal_transcript_consumption_decision(input(
                inbox.clone(),
                Some(root(status)),
                Vec::new(),
            ));
            assert_eq!(decision, TerminalTranscriptConsumptionDecision::Noop);
            assert!(decision.is_noop());
        }
    }
}

#[test]
fn missing_root_is_a_noop_even_for_a_pending_unbound_observation() {
    let decision = TerminalTranscriptConsumptionPolicy::decide(input(pending(), None, Vec::new()));

    assert_eq!(decision, TerminalTranscriptConsumptionDecision::Noop);
    assert!(decision.delete_action().is_none());
}

#[test]
fn every_active_root_status_is_a_noop_without_considering_bindings() {
    for status in RootRunStatus::ACTIVE {
        let decision = TerminalTranscriptConsumptionPolicy::consume(input(
            pending(),
            Some(root(status)),
            vec![binding(
                "different-engine",
                "different-root",
                "different-thread",
            )],
        ));

        assert_eq!(decision, TerminalTranscriptConsumptionDecision::Noop);
    }
}

#[test]
fn every_known_settled_status_deletes_only_an_unbound_pending_observation() {
    for status in RootRunStatus::SETTLED {
        assert_eq!(
            terminal_transcript_consumption_decision(input(
                pending(),
                Some(root(status)),
                Vec::new(),
            )),
            expected_delete()
        );
    }
}

#[test]
fn an_exact_durable_binding_preserves_the_observation_for_child_projection() {
    for status in RootRunStatus::SETTLED {
        let decision = terminal_transcript_consumption_decision(input(
            pending(),
            Some(root(status)),
            vec![binding(ENGINE_ID, ROOT_RUN_ID, NATIVE_THREAD_ID)],
        ));

        assert_eq!(decision, TerminalTranscriptConsumptionDecision::Noop);
    }
}

#[test]
fn each_partially_mismatched_binding_is_unbound_and_deletes() {
    let cases = [
        (
            "engine mismatch",
            binding("engine:other", ROOT_RUN_ID, NATIVE_THREAD_ID),
        ),
        (
            "root mismatch",
            binding(ENGINE_ID, "root-run:other", NATIVE_THREAD_ID),
        ),
        (
            "native-thread mismatch",
            binding(ENGINE_ID, ROOT_RUN_ID, "native-thread:other"),
        ),
    ];

    for status in RootRunStatus::SETTLED {
        for (label, candidate) in &cases {
            let decision = terminal_transcript_consumption_decision(input(
                pending(),
                Some(root(status.clone())),
                vec![candidate.clone()],
            ));

            assert_eq!(decision, expected_delete(), "{label} must not bind");
        }
    }
}

#[test]
fn one_exact_binding_wins_over_other_partial_candidates() {
    let decision = terminal_transcript_consumption_decision(input(
        pending(),
        Some(root(RootRunStatus::Completed)),
        vec![
            binding("engine:other", ROOT_RUN_ID, NATIVE_THREAD_ID),
            binding(ENGINE_ID, ROOT_RUN_ID, NATIVE_THREAD_ID),
            binding(ENGINE_ID, "root-run:other", NATIVE_THREAD_ID),
        ],
    ));

    assert_eq!(decision, TerminalTranscriptConsumptionDecision::Noop);
}

#[test]
fn delete_action_has_exact_identity_and_unprocessed_guard() {
    let decision = terminal_transcript_consumption_decision(input(
        pending(),
        Some(root(RootRunStatus::Failed)),
        Vec::new(),
    ));
    let action = decision
        .delete_action()
        .expect("settled unbound observation must emit a delete");

    assert_eq!(
        action,
        &ConditionalDeleteTranscriptAction {
            observation_id: OBSERVATION_ID.to_owned(),
            guard: UnprocessedTranscriptGuard::new(),
        }
    );
    assert_eq!(action.observation_id(), OBSERVATION_ID);
    assert!(UnprocessedTranscriptGuard::is_unprocessed());
    assert!(!decision.is_noop());
}

#[test]
fn action_uses_the_consume_identity_without_rewriting_it() {
    let exact_identity = "  observation/identity/é/🦀  ";
    let decision =
        TerminalTranscriptConsumptionPolicy::decide(TerminalTranscriptConsumptionInput::new(
            exact_identity,
            pending(),
            Some(root(RootRunStatus::Interrupted)),
            Vec::<NativeSubagentBinding>::new(),
        ));

    let action = decision
        .delete_action()
        .expect("unbound settled observation");
    assert_eq!(action.observation_id(), exact_identity);
}

#[test]
fn unknown_statuses_follow_the_oracles_non_active_set_membership_rule() {
    let future_settled = RootRunStatus::from_runtime("future-terminal");
    assert_eq!(future_settled.as_str(), "future-terminal");
    assert!(future_settled.is_settled());
    assert_eq!(
        terminal_transcript_consumption_decision(input(
            pending(),
            Some(root(future_settled)),
            Vec::new()
        )),
        expected_delete()
    );

    for active_spelling in ["queued", "running", "waiting"] {
        let status = RootRunStatus::Other(active_spelling.to_owned());
        assert!(status.is_active());
        assert_eq!(
            terminal_transcript_consumption_decision(input(
                pending(),
                Some(root(status)),
                Vec::new(),
            )),
            TerminalTranscriptConsumptionDecision::Noop
        );
    }
}

#[test]
fn repeated_evaluations_are_pure_and_do_not_consume_the_input() {
    let facts = input(
        pending(),
        Some(root(RootRunStatus::Cancelled)),
        vec![binding("engine:other", ROOT_RUN_ID, NATIVE_THREAD_ID)],
    );
    let first = TerminalTranscriptConsumptionPolicy::decide(facts.clone());
    let second = terminal_transcript_consumption_decision(facts.clone());
    let third = TerminalTranscriptConsumptionPolicy::consume(facts);

    assert_eq!(first, expected_delete());
    assert_eq!(first, second);
    assert_eq!(second, third);
}

#[test]
fn policy_marker_is_stateless() {
    let policy = TerminalTranscriptConsumptionPolicy::new();
    assert_eq!(policy, TerminalTranscriptConsumptionPolicy);
}
