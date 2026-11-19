use super::reset_tests::{enqueue, finish_tail, fixture_at, origin};
use super::*;
use artisan_api::Decision;
use editor_core::Clock;
use editor_core::client::{CallError, CallFailure};

fn approve(conversation: &mut Conversation, core: &Core, id: &ItemId) {
    conversation.answer(
        core,
        id.clone(),
        Answer::Approval {
            decision: Decision::AllowOnce,
        },
    );
}

fn refused(conversation: &Conversation, id: &ItemId, request: AnswerRequest) -> Completion {
    Completion::Answer {
        origin: origin(conversation),
        item: id.clone(),
        request,
        result: Err(CallError {
            method: "POST".parse().expect("POST is an HTTP method"),
            path: format!("/items/{id}/answer"),
            failure: Box::new(CallFailure::Inconsistent {
                check: "the answer was refused",
            }),
        }),
    }
}

fn answer_state(conversation: &Conversation, id: &ItemId) -> Option<AnswerState> {
    conversation.view.as_ref()?.answers.get(id).cloned()
}

#[test]
fn an_answer_in_flight_keeps_its_controls_disabled_after_the_item_leaves_the_window() {
    let (_directory, _listener, mut core, mut conversation) = fixture_at(Clock::system());
    let id = ItemId("asked".into());
    approve(&mut conversation, &core, &id);
    let Some(AnswerState::Sending(request)) = answer_state(&conversation, &id) else {
        panic!("submitting an answer starts sending it");
    };

    finish_tail(&mut conversation, &mut core);
    assert!(conversation.view.as_ref().unwrap().item(&id).is_none());
    assert_eq!(
        answer_state(&conversation, &id),
        Some(AnswerState::Sending(request)),
        "evicting the item must not re-enable its answer controls"
    );

    approve(&mut conversation, &core, &id);
    assert_eq!(
        answer_state(&conversation, &id),
        Some(AnswerState::Sending(request))
    );
    assert_eq!(conversation.answer_tasks.len(), 1);

    enqueue(
        &conversation,
        refused(&conversation, &id, AnswerRequest(request.0.wrapping_add(1))),
    );
    conversation.drain(&mut core);
    assert_eq!(
        answer_state(&conversation, &id),
        Some(AnswerState::Sending(request)),
        "a completion for another request must not settle this one"
    );
    assert!(conversation.answer_tasks.contains_key(&request));

    enqueue(&conversation, refused(&conversation, &id, request));
    conversation.drain(&mut core);
    assert!(
        answer_state(&conversation, &id)
            .as_ref()
            .and_then(AnswerState::error)
            .is_some_and(|error| error.starts_with("Couldn't send the answer"))
    );
    assert!(conversation.answer_tasks.is_empty());
}

#[test]
fn an_answer_forge_refuses_shows_forge_reason_on_the_card() {
    let (_directory, _listener, mut core, mut conversation) = fixture_at(Clock::system());
    let id = ItemId("asked".into());
    approve(&mut conversation, &core, &id);
    let Some(AnswerState::Sending(request)) = answer_state(&conversation, &id) else {
        panic!("submitting an answer starts sending it");
    };
    let failure = artisan_api::Failure::Run(artisan_api::RunFailure::AnswerRefused {
        item: Some(id.clone()),
        reason: artisan_api::AnswerRefusal::AnsweredDifferently,
    });
    let message = failure.to_string();
    enqueue(
        &conversation,
        Completion::Answer {
            origin: origin(&conversation),
            item: id.clone(),
            request,
            result: Err(CallError {
                method: "POST".parse().expect("POST is an HTTP method"),
                path: format!("/items/{id}/answer"),
                failure: Box::new(CallFailure::Answer(
                    editor_core::client::ResponseError::Refused {
                        status: 409.try_into().expect("409 is an HTTP status"),
                        error: artisan_api::ApiError::new(failure, message),
                    },
                )),
            }),
        },
    );
    conversation.drain(&mut core);
    assert_eq!(
        answer_state(&conversation, &id)
            .as_ref()
            .and_then(AnswerState::error),
        Some("Couldn't send the answer: This question was already answered differently.")
    );
}
