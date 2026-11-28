use crate::conversation::controller::Conversation;
use crate::conversation::view::factory::realize::harness::{Transcript, deliver};
use artisan_api::{
    BlobHash, Event, Item, ItemId, ItemKind, LiveCoverage, LiveTarget, Phase, Status, TextContent,
    ThreadId,
};
use editor_core::{Core, CoreEvent, HostEvent, Screen};
use test_support::editor::core::StubbedHost;
use test_support::editor::forge::{StubForge, empty_state};
use test_support::items::{item, user_message};

struct Streaming {
    core: Core,
    forge: StubForge,
    conversation: Conversation,
    thread: ThreadId,
    target: LiveTarget,
    _directory: tempfile::TempDir,
}

impl Streaming {
    fn start() -> Self {
        let StubbedHost {
            mut core,
            forge,
            host,
            directory,
        } = StubbedHost::start(empty_state(), axum::Router::new());
        let thread = ThreadId("streaming-thread".into());
        let reply = ItemId("streaming-reply".into());
        core.navigate(Screen::Thread {
            host: host.clone(),
            thread: thread.clone(),
        });
        let mut conversation = Conversation::new(&core);
        conversation.open_thread(&mut core, host, thread.clone());
        forge.publish(Event::Item {
            item: item(
                "question",
                thread.clone(),
                1,
                user_message("Where is the plan?"),
            ),
        });
        forge.publish(Event::Item {
            item: Item {
                status: Status::Running,
                ..item(
                    reply.clone(),
                    thread.clone(),
                    2,
                    ItemKind::AssistantMessage {
                        text: TextContent::Inline(String::new()),
                        phase: Phase::Final,
                    },
                )
            },
        });
        deliver(
            &mut core,
            &mut conversation,
            "the started reply",
            |event| matches!(event, CoreEvent::Host { event: HostEvent::Item(item), .. } if item.id == reply),
        );
        Self {
            core,
            forge,
            conversation,
            thread,
            target: LiveTarget::Text { item_id: reply },
            _directory: directory,
        }
    }

    fn stream(&mut self, text: &str) {
        let total = u64::try_from(text.len()).expect("the test text fits in u64");
        self.forge.publish(Event::Live {
            thread_id: self.thread.clone(),
            target: self.target.clone(),
            text: text.into(),
            append: false,
            coverage: LiveCoverage { start: 0, total },
        });
        let target = self.target.clone();
        deliver(
            &mut self.core,
            &mut self.conversation,
            "the streamed snapshot",
            |event| matches!(event, CoreEvent::Host { event: HostEvent::Live { target: actual, .. }, .. } if actual == &target),
        );
    }
}

#[test]
fn a_reply_that_has_not_streamed_a_block_yet_keeps_the_transcript_rendering() {
    let mut streaming = Streaming::start();
    let mut transcript = Transcript::open(&streaming.conversation, &streaming.core);
    let sync = transcript.render(&mut streaming.conversation, &streaming.core);
    assert!(
        transcript
            .texts()
            .iter()
            .any(|text| text.contains("Where is the plan?")),
        "the question shows while the reply has no text yet"
    );
    assert!(
        !sync.pending,
        "an empty reply does not keep the reveal waiting: {}",
        sync.reason
    );

    streaming.stream("The plan is in the brief.");
    transcript.render(&mut streaming.conversation, &streaming.core);
    let painted = transcript.texts();
    assert!(
        painted
            .iter()
            .any(|text| text.contains("The plan is in the brief.")),
        "the streamed words render: {painted:?}"
    );
}

#[test]
fn a_reply_whose_streamed_text_is_replaced_by_blank_lines_stops_waiting_for_the_old_words() {
    let mut streaming = Streaming::start();
    let mut transcript = Transcript::open(&streaming.conversation, &streaming.core);
    streaming.stream("The plan is in the brief.");
    transcript.render(&mut streaming.conversation, &streaming.core);
    assert!(
        transcript
            .texts()
            .iter()
            .any(|text| text.contains("The plan is in the brief.")),
        "the streamed words render first"
    );

    streaming.stream("  \n");
    let sync = transcript.render(&mut streaming.conversation, &streaming.core);
    assert!(
        !transcript
            .texts()
            .iter()
            .any(|text| text.contains("The plan is in the brief.")),
        "the replaced words are gone"
    );
    assert!(
        !sync.pending,
        "the emptied reply must not keep requesting frames for its old words: {}",
        sync.reason
    );
    let settled = transcript.render(&mut streaming.conversation, &streaming.core);
    assert!(!settled.pending, "{}", settled.reason);

    streaming.stream("The plan moved to the wiki.");
    transcript.render(&mut streaming.conversation, &streaming.core);
    assert!(
        transcript
            .texts()
            .iter()
            .any(|text| text.contains("The plan moved to the wiki.")),
        "words streamed after the blank snapshot render"
    );
}

#[test]
fn a_message_of_only_blank_lines_still_shows_its_attached_images() {
    let StubbedHost {
        mut core,
        forge,
        host,
        directory: _directory,
    } = StubbedHost::start(empty_state(), axum::Router::new());
    let thread = ThreadId("image-thread".into());
    let message = ItemId("image-message".into());
    core.navigate(Screen::Thread {
        host: host.clone(),
        thread: thread.clone(),
    });
    let mut conversation = Conversation::new(&core);
    conversation.open_thread(&mut core, host, thread.clone());
    forge.publish(Event::Item {
        item: item(
            message.clone(),
            thread,
            1,
            ItemKind::UserMessage {
                text: TextContent::Inline("  \n".into()),
                images: vec![BlobHash("a".repeat(64))],
                steer: false,
                from_agent: None,
                sender: None,
            },
        ),
    });
    deliver(
        &mut core,
        &mut conversation,
        "the image message",
        |event| matches!(event, CoreEvent::Host { event: HostEvent::Item(item), .. } if item.id == message),
    );
    let mut transcript = Transcript::open(&conversation, &core);
    transcript.render(&mut conversation, &core);
    assert!(
        transcript.shows_selector(&format!("sent-image-{message}-0")),
        "the attached image shows even though the text is blank"
    );
}

#[test]
fn a_reply_that_shrinks_through_blank_lines_renders_its_one_remaining_block() {
    let mut streaming = Streaming::start();
    let mut transcript = Transcript::open(&streaming.conversation, &streaming.core);
    streaming.stream("First part.\n\nSecond part.\n\nThird part.");
    transcript.render(&mut streaming.conversation, &streaming.core);
    transcript.render(&mut streaming.conversation, &streaming.core);
    assert!(
        transcript
            .texts()
            .iter()
            .any(|text| text.contains("Third part.")),
        "all three blocks render first"
    );

    streaming.stream("  \n");
    transcript.render(&mut streaming.conversation, &streaming.core);
    transcript.render(&mut streaming.conversation, &streaming.core);

    streaming.stream("Only part.");
    transcript.render(&mut streaming.conversation, &streaming.core);
    let sync = transcript.render(&mut streaming.conversation, &streaming.core);
    let painted = transcript.texts();
    assert!(
        painted.iter().any(|text| text.contains("Only part.")),
        "the one remaining block renders: {painted:?}"
    );
    assert!(
        !painted.iter().any(|text| text.contains("Second part.")),
        "the dropped blocks are gone: {painted:?}"
    );
    assert!(!sync.pending, "{}", sync.reason);
}

#[test]
fn a_streamed_tight_list_item_followed_by_a_blockquote_renders_without_error() {
    let mut streaming = Streaming::start();
    let mut transcript = Transcript::open(&streaming.conversation, &streaming.core);
    let prefix = "### Two Ways\n\n1. **First step**\n   - Intro copy:\n";
    let full = concat!(
        "### Two Ways\n\n",
        "1. **First step**\n",
        "   - Intro copy:\n",
        "     > **Help improve Artisan Editor**\n",
        "     > Zero code ever leaves your machine.\n",
        "   - Bonus item.\n",
    );
    streaming.stream(prefix);
    transcript.render(&mut streaming.conversation, &streaming.core);

    streaming.stream(full);
    transcript.render(&mut streaming.conversation, &streaming.core);
    transcript.render(&mut streaming.conversation, &streaming.core);

    let painted = transcript.texts();
    assert!(
        painted
            .iter()
            .any(|text| text.contains("Help improve Artisan Editor")),
        "the nested blockquote renders: {painted:?}"
    );
    assert!(
        painted.iter().any(|text| text.contains("Bonus item.")),
        "items after the nested blockquote render: {painted:?}"
    );
    assert!(
        !painted
            .iter()
            .any(|text| text.contains("present conversation body")),
        "no body presentation error is shown: {painted:?}"
    );
}
