use artisan_api::{
    ApiError, DraftFailure, ErrorCode, Failure, RunFailure, ThreadFailure, ThreadId,
};
use serde_json::json;

#[test]
fn an_older_forge_refusal_without_a_failure_decodes_as_legacy_keeping_its_code() {
    let refusal: ApiError = serde_json::from_value(json!({
        "code": "not_found",
        "message": "thread t1 not found",
    }))
    .expect("the shape every Forge before typed failures sends");
    assert_eq!(
        refusal.failure,
        Failure::Legacy {
            code: ErrorCode::NotFound
        }
    );
    assert_eq!(refusal.message, "thread t1 not found");
}

#[test]
fn a_failure_this_editor_does_not_know_decodes_as_legacy_instead_of_failing() {
    let refusal: ApiError = serde_json::from_value(json!({
        "code": "conflict",
        "message": "something new",
        "failure": { "family": "run", "kind": "something_new", "thread": "t1" },
    }))
    .expect("an unknown variant still decodes");
    assert_eq!(
        refusal.failure,
        Failure::Legacy {
            code: ErrorCode::Conflict
        }
    );
}

#[test]
fn a_failure_this_editor_does_not_know_leaves_why_in_the_log() {
    let (log, _guard) = test_support::logs::capture();
    let _refusal: ApiError = serde_json::from_value(json!({
        "code": "conflict",
        "message": "something new",
        "failure": { "family": "run", "kind": "something_new", "thread": "t1" },
    }))
    .expect("an unknown variant still decodes");
    let text = log.text();
    assert!(
        text.contains("Conflict") && text.contains("something_new"),
        "the log names the refusal's code and the variant it couldn't read: {text}"
    );
}

#[test]
fn a_typed_refusal_round_trips_with_its_ids_and_limits() {
    for failure in [
        Failure::Thread(ThreadFailure::NotFound {
            thread: ThreadId("t1".into()),
        }),
        Failure::Run(RunFailure::AlreadyWorking {
            thread: ThreadId("t2".into()),
        }),
        Failure::Draft(DraftFailure::Full {
            scope: "thread:t3".into(),
            bytes: 9,
            limit: 8,
        }),
    ] {
        let sent = ApiError::new(failure.clone(), failure.to_string());
        let wire = serde_json::to_value(&sent).expect("encode");
        assert_eq!(
            wire["code"],
            json!(serde_json::to_value(failure.code()).expect("code"))
        );
        let received: ApiError = serde_json::from_value(wire).expect("decode");
        assert_eq!(received, sent);
    }
}

#[test]
fn a_typed_refusal_reads_as_tagged_families_and_kinds() {
    let refusal: ApiError = serde_json::from_value(json!({
        "code": "not_found",
        "message": "Thread t1 doesn't exist on this Forge.",
        "failure": { "family": "thread", "kind": "not_found", "thread": "t1" },
    }))
    .expect("the typed shape");
    assert_eq!(
        refusal.failure,
        Failure::Thread(ThreadFailure::NotFound {
            thread: ThreadId("t1".into())
        })
    );
}
