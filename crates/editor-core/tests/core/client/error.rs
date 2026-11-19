use artisan_api::{ApiError, AuthFailure, ErrorCode, Failure, ThreadFailure, ThreadId};
use editor_core::client::{CallError, CallFailure, ForgeFault, ResponseError};
use editor_core::{HostId, LoadThreadError, ThreadUnavailable};
use std::sync::Arc;

fn call(failure: CallFailure) -> CallError {
    CallError {
        method: reqwest::Method::GET,
        path: "/threads/missing".to_owned(),
        failure: Box::new(failure),
    }
}

fn typed(status: reqwest::StatusCode, failure: Failure) -> CallError {
    let message = failure.to_string();
    call(CallFailure::Answer(ResponseError::Refused {
        status,
        error: ApiError::new(failure, message),
    }))
}

fn refused(status: reqwest::StatusCode, code: ErrorCode) -> CallError {
    call(CallFailure::Answer(ResponseError::Refused {
        status,
        error: ApiError::legacy(code, "thread missing not found".to_owned()),
    }))
}

#[test]
fn forge_failures_map_to_what_went_wrong_for_the_reader() {
    let cases = [
        (
            typed(
                reqwest::StatusCode::NOT_FOUND,
                Failure::Thread(ThreadFailure::NotFound {
                    thread: ThreadId("missing".into()),
                }),
            ),
            ForgeFault::Unrecognised,
        ),
        (
            typed(
                reqwest::StatusCode::UNAUTHORIZED,
                Failure::Auth(AuthFailure::NotPaired),
            ),
            ForgeFault::Unauthorized,
        ),
        (
            typed(reqwest::StatusCode::FORBIDDEN, Failure::Internal),
            ForgeFault::Failed,
        ),
        (
            refused(reqwest::StatusCode::NOT_FOUND, ErrorCode::NotFound),
            ForgeFault::Unrecognised,
        ),
        (
            refused(reqwest::StatusCode::FORBIDDEN, ErrorCode::Internal),
            ForgeFault::Unauthorized,
        ),
        (
            refused(reqwest::StatusCode::CONFLICT, ErrorCode::VersionMismatch),
            ForgeFault::Incompatible,
        ),
        (
            refused(
                reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::Internal,
            ),
            ForgeFault::Failed,
        ),
        (
            call(CallFailure::Answer(ResponseError::Unexplained {
                status: reqwest::StatusCode::BAD_GATEWAY,
                body: "upstream".to_owned(),
            })),
            ForgeFault::Failed,
        ),
        (
            call(CallFailure::Incompatible {
                forge_version: "0.1.0".to_owned(),
                protocol: 1,
            }),
            ForgeFault::Incompatible,
        ),
        (
            call(CallFailure::Inconsistent { check: "head" }),
            ForgeFault::Unreadable,
        ),
    ];
    for (error, fault) in cases {
        assert_eq!(error.fault(), fault, "{error:?}");
    }
}

#[test]
fn a_thread_forge_does_not_know_reads_as_a_sentence_without_the_error_chain() {
    let unavailable = ThreadUnavailable::Failed(Arc::new(LoadThreadError {
        host: HostId("mch_capture0000000000000000".to_owned()),
        thread: ThreadId("missing".into()),
        source: refused(reqwest::StatusCode::NOT_FOUND, ErrorCode::NotFound),
    }));
    assert_eq!(
        unavailable.to_string(),
        "Couldn't load this thread. Forge didn't recognise it."
    );
}
