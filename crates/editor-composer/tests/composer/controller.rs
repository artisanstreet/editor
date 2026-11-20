use super::{TurnAction, markdown_link, turn_problem};
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
            turn_problem(TurnAction::Continue, &typed(failure.clone())),
            None,
            "{failure:?}"
        );
    }
    assert_eq!(
        turn_problem(
            TurnAction::ClearGoal,
            &typed(Failure::Run(RunFailure::NoGoal { thread }))
        ),
        None
    );
}

#[test]
fn a_refused_turn_action_says_what_failed_and_keeps_forges_words_as_the_detail() {
    let missing = typed(Failure::Thread(ThreadFailure::NotFound {
        thread: ThreadId("t1".into()),
    }));
    let shown = turn_problem(TurnAction::Stop, &missing).expect("a failed stop is shown");
    assert_eq!(
        shown.headline(),
        "Couldn't stop the run. Forge didn't recognise it."
    );
    assert!(
        shown
            .detail()
            .is_some_and(|detail| detail.contains("Thread t1 doesn't exist on this Forge.")),
        "{shown:?}"
    );
    let busy = typed(Failure::Run(RunFailure::AlreadyWorking {
        thread: ThreadId("t1".into()),
    }));
    let shown = turn_problem(TurnAction::ClearGoal, &busy).expect("a refused clear is shown");
    assert_eq!(
        shown.headline(),
        "Couldn't clear the goal. Forge ran into a problem."
    );
    assert!(
        shown
            .detail()
            .is_some_and(|detail| detail.contains("Thread t1 is already working.")),
        "{shown:?}"
    );
    let legacy = refused(ApiError::legacy(
        ErrorCode::Conflict,
        "The thread has no stopped turn to continue.".to_owned(),
    ));
    let shown = turn_problem(TurnAction::Continue, &legacy).expect("a legacy refusal is shown");
    assert!(
        !shown.headline().contains("POST /threads"),
        "the call path never reaches the headline: {shown:?}"
    );
    assert_eq!(
        shown.detail(),
        Some(
            "POST /threads/t1/continue: Forge refused the call: Forge answered 409 Conflict: The thread has no stopped turn to continue."
        ),
        "a legacy refusal keeps the whole cause chain as the detail"
    );
}
