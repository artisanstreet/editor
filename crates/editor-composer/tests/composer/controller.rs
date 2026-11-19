use super::{TurnAction, markdown_link, turn_failure};
use artisan_api::{ApiError, ErrorCode, Failure, RunFailure, ThreadFailure, ThreadId};
use editor_core::client::{CallError, CallFailure, ResponseError};

#[test]
fn file_links_escape_markup_in_the_name_and_keep_the_path_one_link() {
    assert_eq!(
        markdown_link("*notes*_[draft].md", "/work/*notes*_[draft].md"),
        r"[\*notes\*\_\[draft\].md](</work/*notes*_[draft].md>)"
    );
    assert_eq!(
        markdown_link("a>b\nc.rs", "/work/a>b\nc.rs"),
        r"[a\>b c.rs](</work/a\>b%0Ac.rs>)"
    );
    assert_eq!(
        markdown_link("main.rs", r"C:\src\main.rs"),
        r"[main.rs](<C:\src\main.rs>)"
    );
    assert_eq!(
        markdown_link("x.rs", r"C:\[x]\x.rs"),
        r"[x.rs](<C:\\[x]\x.rs>)"
    );
}

fn refused(error: ApiError) -> CallError {
    CallError {
        method: reqwest::Method::POST,
        path: "/threads/t1/continue".to_owned(),
        failure: Box::new(CallFailure::Answer(ResponseError::Refused {
            status: reqwest::StatusCode::CONFLICT,
            error,
        })),
    }
}

fn typed(failure: Failure) -> CallError {
    let message = failure.to_string();
    refused(ApiError::new(failure, message))
}

#[test]
fn continuing_a_turn_that_already_finished_or_resumed_is_not_an_error() {
    let thread = ThreadId("t1".into());
    for failure in [
        Failure::Run(RunFailure::NothingToContinue {
            thread: thread.clone(),
        }),
        Failure::Run(RunFailure::AlreadyWorking {
            thread: thread.clone(),
        }),
    ] {
        assert_eq!(
            turn_failure(TurnAction::Continue, &typed(failure.clone())),
            None,
            "{failure:?}"
        );
    }
    assert_eq!(
        turn_failure(
            TurnAction::ClearGoal,
            &typed(Failure::Run(RunFailure::NoGoal { thread }))
        ),
        None
    );
}

#[test]
fn a_typed_refusal_is_shown_in_forge_words_and_a_legacy_one_as_the_call_failure() {
    let missing = typed(Failure::Thread(ThreadFailure::NotFound {
        thread: ThreadId("t1".into()),
    }));
    assert_eq!(
        turn_failure(TurnAction::Stop, &missing).as_deref(),
        Some("Thread t1 doesn't exist on this Forge.")
    );
    let busy = typed(Failure::Run(RunFailure::AlreadyWorking {
        thread: ThreadId("t1".into()),
    }));
    assert_eq!(
        turn_failure(TurnAction::ClearGoal, &busy).as_deref(),
        Some("Thread t1 is already working.")
    );
    let legacy = refused(ApiError::legacy(
        ErrorCode::Conflict,
        "The thread has no stopped turn to continue.".to_owned(),
    ));
    let shown = turn_failure(TurnAction::Continue, &legacy);
    assert_eq!(
        shown.as_deref(),
        Some(
            "POST /threads/t1/continue: Forge refused the call: Forge answered 409 Conflict: The thread has no stopped turn to continue."
        ),
        "a legacy refusal shows the whole cause chain"
    );
}
