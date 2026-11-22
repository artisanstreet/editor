use super::reset_tests::{enqueue, finish_tail, fixture_at, origin};
use super::*;
use crate::conversation::thread::{AnswerRequest, AnswerState, ItemAnswer};
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
    conversation.view.as_ref()?.answer_state(id)
}

fn in_flight(conversation: &Conversation, id: &ItemId) -> Option<AnswerRequest> {
    match conversation.view.as_ref()?.answers.get(id)? {
        ItemAnswer::Sending { request, .. } => Some(*request),
        ItemAnswer::Answered { .. } | ItemAnswer::Failed(_) => None,
    }
}

fn sending(conversation: &Conversation) -> usize {
    conversation.view.as_ref().map_or(0, |view| {
        view.answers
            .values()
            .filter(|answer| matches!(answer, ItemAnswer::Sending { .. }))
            .count()
    })
}

#[test]
fn an_answer_in_flight_keeps_its_controls_disabled_after_the_item_leaves_the_window() {
    let (_directory, _listener, mut core, mut conversation) = fixture_at(Clock::system());
    let id = ItemId("asked".into());
    approve(&mut conversation, &core, &id);
    assert_eq!(
        answer_state(&conversation, &id),
        Some(AnswerState::Sending),
        "submitting an answer starts sending it"
    );
    let request = in_flight(&conversation, &id).expect("the answer is in flight");

    finish_tail(&mut conversation, &mut core);
    assert!(conversation.view.as_ref().unwrap().item(&id).is_none());
    assert_eq!(
        answer_state(&conversation, &id),
        Some(AnswerState::Sending),
        "evicting the item must not re-enable its answer controls"
    );

    approve(&mut conversation, &core, &id);
    assert_eq!(answer_state(&conversation, &id), Some(AnswerState::Sending));
    assert_eq!(sending(&conversation), 1);
    assert_eq!(in_flight(&conversation, &id), Some(request));

    enqueue(
        &conversation,
        refused(&conversation, &id, AnswerRequest(request.0.wrapping_add(1))),
    );
    conversation.drain(&mut core);
    assert_eq!(
        answer_state(&conversation, &id),
        Some(AnswerState::Sending),
        "a completion for another request must not settle this one"
    );
    assert_eq!(in_flight(&conversation, &id), Some(request));

    enqueue(&conversation, refused(&conversation, &id, request));
    conversation.drain(&mut core);
    assert!(
        answer_state(&conversation, &id)
            .as_ref()
            .and_then(AnswerState::error)
            .is_some_and(|error| error.starts_with("Couldn't send the answer"))
    );
    assert_eq!(sending(&conversation), 0);
}

#[test]
fn an_answer_forge_refuses_shows_forge_reason_on_the_card() {
    let (_directory, _listener, mut core, mut conversation) = fixture_at(Clock::system());
    let id = ItemId("asked".into());
    approve(&mut conversation, &core, &id);
    assert_eq!(
        answer_state(&conversation, &id),
        Some(AnswerState::Sending),
        "submitting an answer starts sending it"
    );
    let request = in_flight(&conversation, &id).expect("the answer is in flight");
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

fn asked(status: artisan_api::Status, seq: Seq) -> Item {
    Item {
        status,
        ..test_support::items::item(
            "asked",
            ThreadId("thread".into()),
            seq,
            test_support::items::read_tool("src/main.rs"),
        )
    }
}

fn answerable(conversation: &Conversation, id: &ItemId) -> bool {
    let view = conversation.view.as_ref().expect("the thread is open");
    view.item(id)
        .is_some_and(|item| item.status == artisan_api::Status::AwaitingUser)
        && !view.answer_blocks(id)
}

fn land_asked(conversation: &mut Conversation, core: &mut Core) -> ItemId {
    enqueue(
        conversation,
        Completion::Page {
            origin: origin(conversation),
            fetch: Fetch::Tail,
            asked: Instant::now(),
            result: Ok(ItemsPage {
                items: vec![asked(artisan_api::Status::AwaitingUser, 1)],
                runs: Vec::new(),
                more: false,
                work: Vec::new(),
            }),
        },
    );
    conversation.drain(core);
    let id = ItemId("asked".into());
    assert!(
        answerable(conversation, &id),
        "the request waits for an answer"
    );
    id
}

fn reply(conversation: &mut Conversation, core: &mut Core, id: &ItemId) {
    approve(conversation, core, id);
    let request = in_flight(conversation, id).expect("the answer is in flight");
    enqueue(
        conversation,
        Completion::Answer {
            origin: origin(conversation),
            item: id.clone(),
            request,
            result: Ok(()),
        },
    );
    conversation.drain(core);
}

fn forge_item(conversation: &mut Conversation, core: &mut Core, item: Item) {
    let host = conversation.open.as_ref().unwrap().0.clone();
    conversation.core_event(
        core,
        &CoreEvent::Host {
            host,
            event: HostEvent::Item(Arc::new(item)),
        },
        false,
    );
}

#[test]
fn an_accepted_answer_stays_answered_until_forge_resolves_the_request() {
    let (_directory, _listener, mut core, mut conversation) = fixture_at(Clock::system());
    let id = land_asked(&mut conversation, &mut core);

    reply(&mut conversation, &mut core, &id);
    assert!(
        !answerable(&conversation, &id),
        "Forge accepted the answer, so the card must not offer it again before the item updates"
    );
    approve(&mut conversation, &core, &id);
    assert_eq!(
        in_flight(&conversation, &id),
        None,
        "an accepted answer is not sent twice"
    );

    forge_item(
        &mut conversation,
        &mut core,
        asked(artisan_api::Status::Done, 2),
    );
    assert_eq!(
        conversation
            .view
            .as_ref()
            .and_then(|view| view.item(&id))
            .map(|item| item.status),
        Some(artisan_api::Status::Done)
    );
    assert_eq!(
        answer_state(&conversation, &id),
        None,
        "the resolved item replaces the local answered state"
    );
}

#[test]
fn a_request_resolved_before_its_reply_arrives_keeps_nothing_answered() {
    let (_directory, _listener, mut core, mut conversation) = fixture_at(Clock::system());
    let id = land_asked(&mut conversation, &mut core);
    approve(&mut conversation, &core, &id);
    let request = in_flight(&conversation, &id).expect("the answer is in flight");

    forge_item(
        &mut conversation,
        &mut core,
        asked(artisan_api::Status::Done, 2),
    );
    enqueue(
        &conversation,
        Completion::Answer {
            origin: origin(&conversation),
            item: id.clone(),
            request,
            result: Ok(()),
        },
    );
    conversation.drain(&mut core);
    assert_eq!(answer_state(&conversation, &id), None);
    assert!(
        conversation
            .view
            .as_ref()
            .is_some_and(|view| view.answers.get(&id).is_none()),
        "a reply for an already resolved request leaves no local answer behind"
    );
}

#[test]
fn a_request_forge_raises_again_can_be_answered_again() {
    let (_directory, _listener, mut core, mut conversation) = fixture_at(Clock::system());
    let id = land_asked(&mut conversation, &mut core);
    reply(&mut conversation, &mut core, &id);
    assert!(!answerable(&conversation, &id));

    forge_item(
        &mut conversation,
        &mut core,
        asked(artisan_api::Status::AwaitingUser, 2),
    );
    assert!(
        answerable(&conversation, &id),
        "Forge asked again after the answer, so the card offers its controls again"
    );
    approve(&mut conversation, &core, &id);
    assert!(
        in_flight(&conversation, &id).is_some(),
        "the new answer is sent"
    );
}

#[test]
fn an_answered_request_in_an_agent_view_settles_when_the_agent_context_updates() {
    let (_directory, _listener, mut core, mut conversation) = fixture_at(Clock::system());
    let id = land_asked(&mut conversation, &mut core);
    conversation.open_agent(&core, Some(id.clone()));
    assert!(
        answerable(&conversation, &id),
        "the agent view shows the request it was opened on"
    );
    reply(&mut conversation, &mut core, &id);
    assert!(!answerable(&conversation, &id));

    enqueue(
        &conversation,
        Completion::Agent {
            origin: origin(&conversation),
            context: BTreeMap::from([(id.clone(), Arc::new(asked(artisan_api::Status::Done, 2)))]),
            error: None,
            effects: Vec::new(),
        },
    );
    conversation.drain(&mut core);
    assert_eq!(
        conversation
            .view
            .as_ref()
            .and_then(|view| view.item(&id))
            .map(|item| item.status),
        Some(artisan_api::Status::Done)
    );
    assert_eq!(
        answer_state(&conversation, &id),
        None,
        "the resolved context item replaces the local answered state"
    );

    enqueue(
        &conversation,
        Completion::Agent {
            origin: origin(&conversation),
            context: BTreeMap::from([(
                id.clone(),
                Arc::new(asked(artisan_api::Status::AwaitingUser, 3)),
            )]),
            error: None,
            effects: Vec::new(),
        },
    );
    conversation.drain(&mut core);
    assert!(
        answerable(&conversation, &id),
        "a request raised again in the agent context can be answered again"
    );
}
