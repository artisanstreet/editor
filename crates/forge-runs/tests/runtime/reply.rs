use super::{Reply, ReplyFailure};
use artisan_api::{ErrorCode, Failure, RunFailure, ThreadId};

fn refused(reply: serde_json::Value) -> Failure {
    match serde_json::from_value::<Reply>(reply).expect("a generation's reply") {
        Reply::Error(failure) => failure.into_remote().wire(),
        Reply::Ok(value) => panic!("the generation refused, not answered {value}"),
    }
}

#[test]
fn an_older_generation_refusing_without_a_typed_failure_reaches_the_editor_as_legacy() {
    let failure = refused(serde_json::json!({
        "status": "error",
        "value": { "class": "conflict", "message": "The thread is already working." },
    }));
    assert_eq!(
        failure,
        Failure::Legacy {
            code: ErrorCode::Conflict
        }
    );
    let untyped = refused(serde_json::json!({ "status": "error", "value": "it broke" }));
    assert_eq!(
        untyped,
        Failure::Legacy {
            code: ErrorCode::Internal
        }
    );
}

#[test]
fn a_typed_refusal_crosses_generations_with_its_ids() {
    let sent = Failure::Run(RunFailure::AlreadyWorking {
        thread: ThreadId("t1".into()),
    });
    let reply = Reply::Error(ReplyFailure::Typed(super::RemoteFailure {
        class: crate::ErrorClass::Conflict,
        message: sent.to_string(),
        failure: Some(sent.clone()),
    }));
    let wire = serde_json::to_value(&reply).expect("encode");
    assert_eq!(refused(wire), sent);
}

#[test]
fn a_typed_refusal_a_generation_cannot_read_falls_back_to_its_class() {
    let failure = refused(serde_json::json!({
        "status": "error",
        "value": {
            "class": "not_found",
            "message": "gone",
            "failure": { "family": "thread", "kind": "vanished_differently" },
        },
    }));
    assert_eq!(
        failure,
        Failure::Legacy {
            code: ErrorCode::NotFound
        }
    );
}
