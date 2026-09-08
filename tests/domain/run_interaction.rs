//! Finite A-approve backend vocabulary: response commands, intent
//! fingerprints, and target outcomes.

use artisan_domain::{
    Command, InteractionKind, InteractionOutcome, ObservationId, RequestId, RespondApproval,
    RespondQuestion, RunId, RunInteractionError, ThreadId,
};

fn request_id(value: &str) -> RequestId {
    RequestId::parse(value).expect("fixture request id should be valid")
}

fn thread_id() -> ThreadId {
    ThreadId::parse("approve-thread").expect("fixture thread id should be valid")
}

fn run_id() -> RunId {
    RunId::parse("approve-run").expect("fixture run id should be valid")
}

fn approval_id() -> ObservationId {
    ObservationId::parse("approval-1").expect("fixture approval id should be valid")
}

fn question_id() -> ObservationId {
    ObservationId::parse("question-1").expect("fixture question id should be valid")
}

fn approval(approved: bool) -> RespondApproval {
    RespondApproval::new(
        request_id("respond-1"),
        thread_id(),
        run_id(),
        approval_id(),
        approved,
    )
}

fn question(answers: Vec<String>) -> RespondQuestion {
    RespondQuestion::new(
        request_id("respond-2"),
        thread_id(),
        run_id(),
        question_id(),
        answers,
    )
    .expect("fixture answers should be valid")
}

#[test]
fn approval_response_carries_its_explicit_decision() {
    let allow = approval(true);
    let deny = approval(false);
    assert!(allow.approved());
    assert!(!deny.approved());
    assert_eq!(allow.request_id().as_str(), "respond-1");
    assert_eq!(allow.thread_id(), &thread_id());
    assert_eq!(allow.run_id(), &run_id());
    assert_eq!(allow.approval_id(), &approval_id());
    assert_eq!(allow.kind(), InteractionKind::Approval);
}

#[test]
fn question_response_accepts_answers_and_explicit_skips() {
    let answered = question(vec!["first".to_owned(), "second".to_owned()]);
    assert_eq!(
        answered.answers(),
        &["first".to_owned(), "second".to_owned()]
    );
    assert_eq!(answered.question_id(), &question_id());
    assert_eq!(answered.kind(), InteractionKind::Question);

    let skipped = question(Vec::new());
    assert!(skipped.answers().is_empty());
}

#[test]
fn question_answers_reject_overflow_empty_and_oversized_entries() {
    let too_many = vec!["answer".to_owned(); 17];
    assert!(matches!(
        RespondQuestion::new(
            request_id("respond-overflow"),
            thread_id(),
            run_id(),
            question_id(),
            too_many,
        ),
        Err(RunInteractionError::TooManyAnswers {
            count: 17,
            maximum: 16
        })
    ));

    assert!(matches!(
        RespondQuestion::new(
            request_id("respond-empty"),
            thread_id(),
            run_id(),
            question_id(),
            vec!["valid".to_owned(), String::new()],
        ),
        Err(RunInteractionError::EmptyAnswer { index: 1 })
    ));

    assert!(matches!(
        RespondQuestion::new(
            request_id("respond-long"),
            thread_id(),
            run_id(),
            question_id(),
            vec!["x".repeat(1_025)],
        ),
        Err(RunInteractionError::AnswerTooLong {
            index: 0,
            length: 1_025,
            maximum: 1_024,
        })
    ));
}

#[test]
fn intent_fingerprints_distinguish_decisions_and_answers() {
    assert_eq!(approval(true).intent_key(), approval(true).intent_key());
    assert_ne!(approval(true).intent_key(), approval(false).intent_key());

    let first = question(vec!["a".to_owned()]);
    let second = question(vec!["a".to_owned(), "b".to_owned()]);
    assert_ne!(first.intent_key(), second.intent_key());

    // Separator bytes inside one answer never alias two answers.
    let joined = question(vec!["a\0b".to_owned()]);
    let split = question(vec!["a".to_owned(), "b".to_owned()]);
    assert_ne!(joined.intent_key(), split.intent_key());

    // Different targets never share an intent.
    let other_target = RespondApproval::new(
        request_id("respond-1"),
        thread_id(),
        run_id(),
        ObservationId::parse("approval-2").expect("fixture approval id should be valid"),
        true,
    );
    assert_ne!(approval(true).intent_key(), other_target.intent_key());
}

#[test]
fn response_commands_join_the_command_enum_with_stable_ids() {
    let approve = Command::RespondApproval(approval(true));
    assert_eq!(approve.request_id().as_str(), "respond-1");

    let answer = Command::RespondQuestion(question(vec!["yes".to_owned()]));
    assert_eq!(answer.request_id().as_str(), "respond-2");
}

#[test]
fn interaction_kinds_and_outcomes_roundtrip_their_stable_spellings() {
    assert_eq!(InteractionKind::Approval.as_str(), "approval");
    assert_eq!(InteractionKind::Question.as_str(), "question");
    assert_eq!(
        InteractionKind::parse("approval"),
        Ok(InteractionKind::Approval)
    );
    assert_eq!(
        InteractionKind::parse("question"),
        Ok(InteractionKind::Question)
    );
    assert!(InteractionKind::parse("steer").is_err());

    for outcome in [
        InteractionOutcome::Applied,
        InteractionOutcome::UnknownTarget,
        InteractionOutcome::AlreadyResolved,
        InteractionOutcome::WrongRun,
    ] {
        assert_eq!(
            InteractionOutcome::parse(outcome.as_str()),
            Ok(outcome),
            "outcome spelling should round-trip"
        );
    }
    assert_eq!(InteractionOutcome::Applied.as_str(), "applied");
    assert_eq!(InteractionOutcome::UnknownTarget.as_str(), "unknown_target");
    assert_eq!(
        InteractionOutcome::AlreadyResolved.as_str(),
        "already_resolved"
    );
    assert_eq!(InteractionOutcome::WrongRun.as_str(), "wrong_run");
    assert!(InteractionOutcome::parse("duplicate").is_err());
}
