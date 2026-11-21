use super::reset_tests::{finish_tail, fixture_at};
use super::*;
use artisan_api::{Phase, Status};
use std::time::Duration;
use test_support::clock::ManualClock;

fn reply(thread: &ThreadId) -> Arc<Item> {
    Arc::new(Item {
        id: ItemId("reply".into()),
        thread_id: thread.clone(),
        position: 2,
        seq: 2,
        run_id: None,
        parent_id: None,
        status: Status::Done,
        created_at: 0,
        updated_at: 0,
        kind: ItemKind::AssistantMessage {
            text: "Done.".into(),
            phase: Phase::Final,
        },
    })
}

#[test]
fn a_watched_reply_stays_new_for_two_seconds_of_the_conversation_clock() {
    let clock = ManualClock::starting_at(1_700_000_000_000);
    let (_directory, _listener, mut core, mut conversation) = fixture_at(clock.clock());
    finish_tail(&mut conversation, &mut core);
    clock.advance(LANDING_SETTLE);
    assert!(conversation.expire_landing());
    let thread = conversation.open.as_ref().unwrap().1.clone();
    let reply = reply(&thread);

    conversation.host_event(&core, &HostEvent::Item(Arc::clone(&reply)), true);

    let fresh = |conversation: &Conversation| {
        conversation
            .view
            .as_ref()
            .unwrap()
            .ui
            .get(&reply.id)
            .is_some_and(|ui| ui.fresh(conversation.clock().now()))
    };
    assert!(
        fresh(&conversation),
        "a reply that just arrived animates in"
    );
    clock.advance(Duration::from_millis(1_999));
    assert!(fresh(&conversation));
    clock.advance(Duration::from_millis(1));
    assert!(
        !fresh(&conversation),
        "two seconds on the conversation clock later the reply is shown as settled"
    );
}
