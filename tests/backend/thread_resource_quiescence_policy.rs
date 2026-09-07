//! Focused tests for the dependency-free thread-resource quiescence policy.

#![allow(dead_code)]

#[path = "../../modules/backend/src/thread_resource_quiescence_policy.rs"]
mod thread_resource_quiescence_policy;

use thread_resource_quiescence_policy::{
    THREAD_RESOURCE_QUIESCE_PARTICIPANTS, ThreadResourceParticipant,
    ThreadResourceParticipantCompletion, ThreadResourceQuiescenceCompletions,
    ThreadResourceQuiescenceFailure, ThreadResourceQuiescenceInput, ThreadResourceQuiescencePolicy,
    ThreadResourceQuiescenceRequest, classify_thread_resource_quiescence, participant_requests,
};

type Completion = ThreadResourceParticipantCompletion<String, String>;
type Input = ThreadResourceQuiescenceInput<String, String>;
type Failure = ThreadResourceQuiescenceFailure<String>;

const THREAD_ID: &str = " thread-🦀/exact ";

fn success(index: usize) -> Completion {
    Completion::succeeded(format!("success-value-{index}"))
}

fn failure(index: usize) -> Completion {
    Completion::failed(format!("failure-fact-{index}"))
}

fn input_for_failure_mask(mask: u8) -> Input {
    let completion = |index: usize| {
        if mask & (1 << index) == 0 {
            success(index)
        } else {
            failure(index)
        }
    };

    Input::new(
        THREAD_ID,
        ThreadResourceQuiescenceCompletions::new(
            completion(0),
            completion(1),
            completion(2),
            completion(3),
        ),
    )
}

fn expected_failures(mask: u8) -> Vec<(ThreadResourceParticipant, String)> {
    ThreadResourceParticipant::ALL
        .into_iter()
        .enumerate()
        .filter(|(index, _)| mask & (1 << index) != 0)
        .map(|(index, participant)| (participant, format!("failure-fact-{index}")))
        .collect()
}

#[test]
fn participants_match_the_source_set_and_identity_order() {
    assert_eq!(
        ThreadResourceParticipant::ALL,
        [
            ThreadResourceParticipant::AgentGraph,
            ThreadResourceParticipant::AgentOrchestration,
            ThreadResourceParticipant::TerminalSessions,
            ThreadResourceParticipant::PreviewCoordinator,
        ]
    );
    assert_eq!(
        THREAD_RESOURCE_QUIESCE_PARTICIPANTS,
        ThreadResourceParticipant::ALL
    );

    for (index, participant) in ThreadResourceParticipant::ALL.into_iter().enumerate() {
        assert_eq!(participant.index(), index);
    }
    assert_eq!(
        ThreadResourceParticipant::AgentGraph.service_name(),
        "AgentGraphOrchestrator"
    );
    assert_eq!(
        ThreadResourceParticipant::AgentOrchestration.service_name(),
        "AgentOrchestrator"
    );
    assert_eq!(
        ThreadResourceParticipant::TerminalSessions.service_name(),
        "TerminalSessionService"
    );
    assert_eq!(
        ThreadResourceParticipant::PreviewCoordinator.service_name(),
        "PreviewCoordinator"
    );
}

#[test]
fn requests_are_exactly_once_per_participant_and_carry_the_exact_thread_id() {
    let requests = ThreadResourceQuiescencePolicy::requests(THREAD_ID);

    assert_eq!(
        requests
            .iter()
            .map(|request| request.participant)
            .collect::<Vec<_>>(),
        ThreadResourceParticipant::ALL
    );
    assert_eq!(
        requests
            .iter()
            .map(|request| request.thread_id.as_str())
            .collect::<Vec<_>>(),
        vec![THREAD_ID; 4]
    );
    assert_eq!(
        requests
            .iter()
            .map(ThreadResourceQuiescenceRequest::thread_id)
            .collect::<Vec<_>>(),
        vec![THREAD_ID; 4]
    );
}

#[test]
fn every_success_and_failure_combination_is_classified_exhaustively() {
    for mask in 0_u8..16 {
        let result = ThreadResourceQuiescencePolicy::classify(input_for_failure_mask(mask));

        if mask == 0 {
            assert_eq!(result, Ok(()));
            continue;
        }

        let error = result.expect_err("at least one failed completion must fail the aggregate");
        let expected = expected_failures(mask);
        assert_eq!(error.thread_id, THREAD_ID);
        assert_eq!(error.failure_count(), expected.len());
        assert_eq!(
            error
                .failures()
                .iter()
                .map(|failure| (failure.participant, failure.fact.clone()))
                .collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn classification_attempts_all_inputs_after_an_earlier_failure() {
    let requests = participant_requests(THREAD_ID);
    let mut attempted = Vec::new();
    let mut completions = Vec::new();

    for request in requests {
        attempted.push(request.participant);
        let index = request.participant.index();
        completions.push(if index == 0 || index == 3 {
            failure(index)
        } else {
            success(index)
        });
    }

    assert_eq!(attempted, ThreadResourceParticipant::ALL);
    let [
        agent_graph,
        agent_orchestration,
        terminal_sessions,
        preview_coordinator,
    ] = completions
        .try_into()
        .expect("the four request slots were all attempted");
    let error = classify_thread_resource_quiescence(Input::new(
        THREAD_ID,
        ThreadResourceQuiescenceCompletions::new(
            agent_graph,
            agent_orchestration,
            terminal_sessions,
            preview_coordinator,
        ),
    ))
    .expect_err("the aggregate must retain failures from both ends");

    assert_eq!(
        error
            .failures()
            .iter()
            .map(|failure| failure.participant)
            .collect::<Vec<_>>(),
        vec![
            ThreadResourceParticipant::AgentGraph,
            ThreadResourceParticipant::PreviewCoordinator,
        ]
    );
}

#[test]
fn successful_values_are_discarded_and_only_unit_success_is_returned() {
    let result: Result<(), Failure> =
        ThreadResourceQuiescencePolicy::quiesce(ThreadResourceQuiescenceInput::new(
            THREAD_ID,
            ThreadResourceQuiescenceCompletions::new(
                success(0),
                success(1),
                success(2),
                success(3),
            ),
        ));

    assert_eq!(result, Ok(()));
}

#[test]
fn aggregate_failure_preserves_typed_facts_without_completion_order_claims() {
    #[derive(Clone, Debug, Eq, PartialEq)]
    enum Fact {
        GraphUnavailable,
        OrchestrationRejected,
        TerminalCloseFailed,
        PreviewFenceFailed,
    }

    type TypedCompletion = ThreadResourceParticipantCompletion<&'static str, Fact>;
    type TypedInput = ThreadResourceQuiescenceInput<&'static str, Fact>;

    let error = ThreadResourceQuiescencePolicy::classify(TypedInput::new(
        THREAD_ID,
        ThreadResourceQuiescenceCompletions::new(
            TypedCompletion::failed(Fact::GraphUnavailable),
            TypedCompletion::succeeded("orchestration-success"),
            TypedCompletion::failed(Fact::TerminalCloseFailed),
            TypedCompletion::failed(Fact::PreviewFenceFailed),
        ),
    ))
    .expect_err("three typed participant failures must aggregate");

    assert_eq!(
        error.failures,
        vec![
            thread_resource_quiescence_policy::ThreadResourceParticipantFailure::new(
                ThreadResourceParticipant::AgentGraph,
                Fact::GraphUnavailable,
            ),
            thread_resource_quiescence_policy::ThreadResourceParticipantFailure::new(
                ThreadResourceParticipant::TerminalSessions,
                Fact::TerminalCloseFailed,
            ),
            thread_resource_quiescence_policy::ThreadResourceParticipantFailure::new(
                ThreadResourceParticipant::PreviewCoordinator,
                Fact::PreviewFenceFailed,
            ),
        ]
    );
    assert_eq!(error.thread_id, THREAD_ID);
}

#[test]
fn repeated_calls_are_independent_and_do_not_retain_prior_observations() {
    assert_eq!(
        ThreadResourceQuiescencePolicy::classify(input_for_failure_mask(0)),
        Ok::<(), Failure>(())
    );
    let first_failure = ThreadResourceQuiescencePolicy::classify(input_for_failure_mask(0b0010))
        .expect_err("the orchestration failure must be returned");
    assert_eq!(first_failure.failure_count(), 1);
    assert_eq!(
        first_failure.failures[0].participant,
        ThreadResourceParticipant::AgentOrchestration
    );

    assert_eq!(
        ThreadResourceQuiescencePolicy::classify(input_for_failure_mask(0)),
        Ok::<(), Failure>(())
    );
    let second_failure = ThreadResourceQuiescencePolicy::classify(input_for_failure_mask(0b1000))
        .expect_err("the preview failure must be returned");
    assert_eq!(second_failure.failure_count(), 1);
    assert_eq!(
        second_failure.failures[0].participant,
        ThreadResourceParticipant::PreviewCoordinator
    );
}
