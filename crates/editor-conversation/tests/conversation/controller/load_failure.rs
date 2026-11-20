use super::reset_tests::{enqueue, finish_tail, fixture_at, origin};
use super::*;
use editor_core::Clock;
use editor_core::client::CallFailure;

fn unreachable() -> CallError {
    CallError {
        method: reqwest::Method::GET,
        path: "/threads/thread/transcript".into(),
        failure: Box::new(CallFailure::Inconsistent {
            check: "the page ends before it starts",
        }),
    }
}

#[test]
fn a_failed_earlier_page_says_so_and_try_again_asks_for_the_same_page() {
    let (_directory, _listener, mut core, mut conversation) = fixture_at(Clock::system());
    finish_tail(&mut conversation, &mut core);
    let older = Fetch::Older { before: 1 };
    let view = conversation.view.as_mut().unwrap();
    view.viewport.in_flight = true;
    view.viewport.requested = Some(older);
    enqueue(
        &conversation,
        Completion::Page {
            origin: origin(&conversation),
            fetch: older,
            asked: Instant::now(),
            result: Err(unreachable()),
        },
    );
    assert!(conversation.drain(&mut core));
    let failure = conversation
        .load_error
        .clone()
        .expect("the failed page is reported");
    assert_eq!(
        failure.problem().headline(),
        "Couldn't load earlier messages. Forge's answer couldn't be read."
    );
    assert!(
        failure
            .problem()
            .detail()
            .is_some_and(|detail| detail.contains("/threads/thread/transcript")),
        "{failure:?}"
    );
    assert!(failure.retryable());
    assert!(!conversation.view.as_ref().unwrap().viewport.in_flight);

    conversation.retry_load(&core);

    assert!(
        conversation.load_error.is_none(),
        "trying again clears the problem"
    );
    let viewport = &conversation.view.as_ref().unwrap().viewport;
    assert!(viewport.in_flight);
    assert_eq!(
        viewport.requested,
        Some(older),
        "the same page is asked for again"
    );
    assert!(conversation.page_task.is_some());
}
