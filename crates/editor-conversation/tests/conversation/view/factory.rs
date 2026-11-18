use super::{answer_text, message_author, plain_block};
use crate::conversation::fold::{Row, Slot, TextSection};
use artisan_api::{EngineId, Item, ItemId, ItemKind, Sender, Status, TextContent, ThreadId};

#[test]
fn an_answer_keeps_the_words_you_answered_inside_it() {
    assert_eq!(
        answer_text("You answered: Print \"You answered: yes\" literally"),
        "Print \"You answered: yes\" literally"
    );
    assert_eq!(
        answer_text("Print \"You answered: yes\" literally"),
        "Print \"You answered: yes\" literally"
    );
}

#[test]
fn a_long_inline_error_shows_each_part_once() {
    let error = (0..400)
        .map(|line| format!("error[{line}]: the build broke"))
        .collect::<Vec<_>>()
        .join(" ");
    let blocks = (0..)
        .map_while(|block| plain_block(&error, block))
        .collect::<Vec<_>>();
    assert!(blocks.len() > 3, "{}", blocks.len());
    assert!(blocks.iter().all(|block| block.len() <= 1024));
    assert_eq!(blocks.concat(), error);
}

fn message(kind: ItemKind) -> Item {
    Item {
        id: ItemId("message".into()),
        thread_id: ThreadId("thread".into()),
        position: 1,
        seq: 1,
        run_id: None,
        parent_id: None,
        status: Status::Done,
        created_at: 0,
        updated_at: 0,
        kind,
    }
}

fn block(slot: Slot, block: usize) -> Row {
    Row::Text {
        id: ItemId("message".into()),
        slot,
        block,
        run: None,
    }
}

fn sent(sender: Option<Sender>) -> Item {
    message(ItemKind::UserMessage {
        text: TextContent::Inline("Say hello".into()),
        images: Vec::new(),
        steer: false,
        from_agent: None,
        sender,
    })
}

#[test]
fn a_message_is_read_with_its_author_once_at_its_start() {
    let user = sent(None);
    let user_row = Slot::Text(TextSection::User);
    assert_eq!(
        message_author(&block(user_row, 0), Some(&user), None),
        Some("You")
    );
    assert_eq!(message_author(&block(user_row, 1), Some(&user), None), None);
    let forwarded = sent(Some(Sender {
        thread: ThreadId("other".into()),
        engine: EngineId::Codex,
        title: None,
    }));
    assert_eq!(
        message_author(&block(user_row, 0), Some(&forwarded), None),
        Some("Codex")
    );
    let reply = message(ItemKind::AssistantMessage {
        text: TextContent::Inline("Hello".into()),
        phase: artisan_api::Phase::Final,
    });
    assert_eq!(
        message_author(&block(Slot::Prose, 0), Some(&reply), Some(EngineId::Claude)),
        Some("Claude Code")
    );
    assert_eq!(
        message_author(&block(Slot::Prose, 1), Some(&reply), Some(EngineId::Claude)),
        None
    );
    assert_eq!(
        message_author(&block(Slot::Prose, 0), Some(&reply), None),
        Some("Assistant"),
        "a reply whose run is not loaded still has an author"
    );
}
