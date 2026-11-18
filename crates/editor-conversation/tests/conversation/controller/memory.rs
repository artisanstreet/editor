use super::reset_tests::{finish_tail, fixture_at};
use super::*;
use artisan_api::Status;
use editor_core::Clock;

fn reply(id: &str, position: u64) -> Item {
    Item {
        id: ItemId(id.into()),
        thread_id: ThreadId("thread".into()),
        position,
        seq: position,
        run_id: None,
        parent_id: None,
        status: Status::Done,
        created_at: 0,
        updated_at: 0,
        kind: ItemKind::AssistantMessage {
            text: format!("Reply {position}.").into(),
            phase: artisan_api::Phase::Final,
        },
    }
}

#[test]
fn heights_of_rows_that_left_the_transcript_are_dropped() {
    let (_directory, _listener, mut core, mut conversation) = fixture_at(Clock::system());
    finish_tail(&mut conversation, &mut core);
    let current = RowKey::Item(ItemId("current".into()));
    let gone = RowKey::Item(ItemId("scrolled-away".into()));
    conversation.row_heights.insert(current.clone(), 48.0);
    conversation.row_heights.insert(gone.clone(), 120.0);

    assert!(
        conversation
            .view
            .as_mut()
            .unwrap()
            .apply_item(reply("next", 2), None)
    );
    conversation.rebuild(&core);

    assert_eq!(conversation.row_heights.get(&current), Some(&48.0));
    assert!(!conversation.row_heights.contains_key(&gone));
}
