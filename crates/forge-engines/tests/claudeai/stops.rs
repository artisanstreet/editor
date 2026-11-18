use super::*;
use artisan_api::Phase;

fn ending(reason: &str) -> Reply {
    let mut events = vec![start("m1")];
    events.extend(text(0, "The answer is"));
    events.push(json!({ "type": "message_delta", "delta": { "stop_reason": reason } }));
    events.push(json!({ "type": "message_stop" }));
    Reply::Ends(events)
}

fn reply_phase(log: &[EngineEvent]) -> Option<Phase> {
    log.iter().rev().find_map(|event| match event {
        EngineEvent::Item {
            status: Status::Done,
            kind: ItemKind::AssistantMessage { phase, .. },
            ..
        } => Some(*phase),
        _ => None,
    })
}

async fn ended_by(reason: &str) -> (Outcome, Vec<EngineEvent>) {
    let work = tempfile::tempdir().expect("a dir");
    let (outcome, log, _) = run(
        script(vec![ending(reason)]),
        turn(work.path(), None, Permissions::Auto),
        nobody(),
    )
    .await;
    (outcome, log)
}

#[tokio::test]
async fn a_completed_answer_is_the_final_reply() {
    let (outcome, log) = ended_by("end_turn").await;
    assert_eq!(outcome, Outcome::Done);
    assert_eq!(reply_phase(&log), Some(Phase::Final));
}

#[tokio::test]
async fn an_answer_cut_off_at_the_output_limit_fails_and_is_not_the_reply() {
    let (outcome, log) = ended_by("max_tokens").await;
    assert_eq!(outcome.status(), RunStatus::Failed);
    assert!(
        outcome
            .error()
            .is_some_and(|error| error.contains("output limit")),
        "{outcome:?}"
    );
    assert_eq!(reply_phase(&log), Some(Phase::Commentary));
}

#[tokio::test]
async fn a_refused_answer_ends_the_run_as_refused() {
    let (outcome, log) = ended_by("refusal").await;
    assert_eq!(outcome, Outcome::Refused);
    assert_eq!(reply_phase(&log), Some(Phase::Commentary));
    assert!(log.iter().any(|event| matches!(
        event,
        EngineEvent::Item {
            kind: ItemKind::Error {
                kind: artisan_api::ErrorKind::Refusal,
                ..
            },
            ..
        }
    )));
}

#[tokio::test]
async fn an_unknown_stop_reason_fails_and_names_the_reason() {
    let (outcome, log) = ended_by("pause_turn").await;
    assert_eq!(outcome.status(), RunStatus::Failed);
    assert!(
        outcome
            .error()
            .is_some_and(|error| error.contains("pause_turn")),
        "{outcome:?}"
    );
    assert_eq!(reply_phase(&log), Some(Phase::Commentary));
}
