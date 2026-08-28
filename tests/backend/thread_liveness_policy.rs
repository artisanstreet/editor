//! Focused tests for the dependency-free thread liveness policy.

#![allow(dead_code)]

#[path = "../../modules/backend/src/thread_liveness_policy.rs"]
mod thread_liveness_policy;

use thread_liveness_policy::{
    FactObservation, ThreadLivenessAdmissionReason, ThreadLivenessDecision, ThreadLivenessFact,
    ThreadLivenessObservation, ThreadLivenessPolicy, ThreadLivenessRejectionReason,
    evaluate_thread_liveness,
};

#[test]
fn complete_truth_table_covers_all_eight_fact_combinations() {
    let cases = [
        (
            false,
            false,
            false,
            Some(ThreadLivenessRejectionReason::ThreadRowAbsent),
        ),
        (
            false,
            false,
            true,
            Some(ThreadLivenessRejectionReason::ThreadRowAbsent),
        ),
        (
            false,
            true,
            false,
            Some(ThreadLivenessRejectionReason::ThreadRowAbsent),
        ),
        (
            false,
            true,
            true,
            Some(ThreadLivenessRejectionReason::ThreadRowAbsent),
        ),
        (true, false, false, None),
        (
            true,
            false,
            true,
            Some(ThreadLivenessRejectionReason::TombstonePresent),
        ),
        (
            true,
            true,
            false,
            Some(ThreadLivenessRejectionReason::ErasureClaimPresent),
        ),
        (
            true,
            true,
            true,
            Some(ThreadLivenessRejectionReason::ErasureClaimPresent),
        ),
    ];

    for (thread_row_present, erasure_claim_present, tombstone_present, expected_reason) in cases {
        let decision = evaluate_thread_liveness(
            "thread-truth-table",
            ThreadLivenessObservation::complete(
                thread_row_present,
                erasure_claim_present,
                tombstone_present,
            ),
        );

        assert_eq!(decision.thread_id(), "thread-truth-table");
        if let Some(reason) = expected_reason {
            assert!(!decision.allows_durable_write());
            assert_eq!(
                decision,
                ThreadLivenessDecision::Rejected {
                    thread_id: "thread-truth-table".to_owned(),
                    reason,
                }
            );
        } else {
            assert!(decision.allows_durable_write());
            assert_eq!(
                decision,
                ThreadLivenessDecision::Admitted {
                    thread_id: "thread-truth-table".to_owned(),
                    reason: ThreadLivenessAdmissionReason::AllRequiredFactsConfirmLive,
                }
            );
        }
    }
}

#[test]
fn incomplete_observations_reject_each_independent_fact() {
    let cases = [
        (
            ThreadLivenessObservation::new(
                FactObservation::Incomplete,
                FactObservation::Absent,
                FactObservation::Absent,
            ),
            ThreadLivenessFact::ThreadRow,
        ),
        (
            ThreadLivenessObservation::new(
                FactObservation::Present,
                FactObservation::Incomplete,
                FactObservation::Absent,
            ),
            ThreadLivenessFact::ErasureClaim,
        ),
        (
            ThreadLivenessObservation::new(
                FactObservation::Present,
                FactObservation::Absent,
                FactObservation::Incomplete,
            ),
            ThreadLivenessFact::Tombstone,
        ),
    ];

    for (observations, fact) in cases {
        assert!(!observations.is_complete());
        let decision = evaluate_thread_liveness("thread-incomplete", observations);
        assert_eq!(
            decision,
            ThreadLivenessDecision::Rejected {
                thread_id: "thread-incomplete".to_owned(),
                reason: ThreadLivenessRejectionReason::ObservationIncomplete { fact },
            }
        );
    }
}

#[test]
fn failed_observations_reject_each_independent_fact() {
    let cases = [
        (
            ThreadLivenessObservation::new(
                FactObservation::Failed,
                FactObservation::Absent,
                FactObservation::Absent,
            ),
            ThreadLivenessFact::ThreadRow,
        ),
        (
            ThreadLivenessObservation::new(
                FactObservation::Present,
                FactObservation::Failed,
                FactObservation::Absent,
            ),
            ThreadLivenessFact::ErasureClaim,
        ),
        (
            ThreadLivenessObservation::new(
                FactObservation::Present,
                FactObservation::Absent,
                FactObservation::Failed,
            ),
            ThreadLivenessFact::Tombstone,
        ),
    ];

    for (observations, fact) in cases {
        assert!(!observations.is_complete());
        let decision = evaluate_thread_liveness("thread-failed", observations);
        assert_eq!(
            decision,
            ThreadLivenessDecision::Rejected {
                thread_id: "thread-failed".to_owned(),
                reason: ThreadLivenessRejectionReason::ObservationFailed { fact },
            }
        );
    }
}

#[test]
fn incomplete_or_failed_evidence_cannot_be_confused_with_observed_absence() {
    let observed_absence = evaluate_thread_liveness(
        "thread-evidence",
        ThreadLivenessObservation::complete(false, false, false),
    );
    assert_eq!(
        observed_absence,
        ThreadLivenessDecision::Rejected {
            thread_id: "thread-evidence".to_owned(),
            reason: ThreadLivenessRejectionReason::ThreadRowAbsent,
        }
    );

    let missing_evidence = evaluate_thread_liveness(
        "thread-evidence",
        ThreadLivenessObservation::new(
            FactObservation::Incomplete,
            FactObservation::Absent,
            FactObservation::Absent,
        ),
    );
    assert_eq!(
        missing_evidence,
        ThreadLivenessDecision::Rejected {
            thread_id: "thread-evidence".to_owned(),
            reason: ThreadLivenessRejectionReason::ObservationIncomplete {
                fact: ThreadLivenessFact::ThreadRow,
            },
        }
    );

    let failed_evidence = evaluate_thread_liveness(
        "thread-evidence",
        ThreadLivenessObservation::new(
            FactObservation::Failed,
            FactObservation::Absent,
            FactObservation::Absent,
        ),
    );
    assert_eq!(
        failed_evidence,
        ThreadLivenessDecision::Rejected {
            thread_id: "thread-evidence".to_owned(),
            reason: ThreadLivenessRejectionReason::ObservationFailed {
                fact: ThreadLivenessFact::ThreadRow,
            },
        }
    );
}

#[test]
fn decision_keeps_exact_supplied_thread_id_without_normalization() {
    let supplied_thread_id = "  Thread/é/🦀 ";
    let decision = evaluate_thread_liveness(
        supplied_thread_id,
        ThreadLivenessObservation::complete(true, false, false),
    );

    assert_eq!(decision.thread_id(), supplied_thread_id);
    assert_eq!(
        decision,
        ThreadLivenessDecision::Admitted {
            thread_id: supplied_thread_id.to_owned(),
            reason: ThreadLivenessAdmissionReason::AllRequiredFactsConfirmLive,
        }
    );
}

#[test]
fn policy_evaluation_is_stateless_and_repeatable() {
    let policy = ThreadLivenessPolicy::new();
    assert_eq!(policy, ThreadLivenessPolicy);
    let observations = ThreadLivenessObservation::complete(true, false, false);

    let first = ThreadLivenessPolicy::evaluate("thread-repeat", observations);
    let second = ThreadLivenessPolicy::evaluate("thread-repeat", observations);

    assert_eq!(first, second);
    assert!(first.allows_durable_write());
    assert!(observations.is_complete());
}

#[test]
fn fact_observation_distinguishes_completed_boolean_results() {
    assert!(FactObservation::observed(true).is_complete());
    assert!(FactObservation::observed(false).is_complete());
    assert_ne!(
        FactObservation::observed(false),
        FactObservation::Incomplete
    );
    assert_ne!(FactObservation::observed(false), FactObservation::Failed);
}
