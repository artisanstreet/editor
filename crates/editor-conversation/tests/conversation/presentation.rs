use super::{BodyKey, BodyText, MarkdownOwners, PresentationScope};
use crate::conversation::controller::Conversation;
use crate::conversation::fold::{Slot, TextSection};
use crate::conversation::mentions::TITLE_RECHECK;
use aqua_text::document::SelectionDocumentId;
use artisan_api::{CatalogSkill, ItemId, ThreadId};
use editor_core::Screen;
use editor_ui::markdown::MarkdownEngine;
use std::sync::Arc;
use std::time::{Duration, Instant};
use test_support::editor::core::StubbedHost;
use test_support::editor::forge::empty_state;
use test_support::editor::ui::ui_app;

struct Open {
    stub: StubbedHost,
    thread: ThreadId,
    markdown: MarkdownOwners,
    scope: PresentationScope,
    _conversation: Conversation,
}

fn open() -> Open {
    let mut stub = StubbedHost::start(empty_state(), axum::Router::new());
    let thread = ThreadId("open".into());
    let host = stub.host.clone();
    stub.core.navigate(Screen::Thread {
        host: host.clone(),
        thread: thread.clone(),
    });
    let mut conversation = Conversation::new(&stub.core);
    conversation.open_thread(&mut stub.core, host, thread.clone());
    let scope =
        PresentationScope::capture(&conversation, &stub.core).expect("the open thread has a scope");
    let mut markdown = MarkdownOwners::new(SelectionDocumentId(1));
    let mut app = ui_app(600.0, 400.0);
    markdown.rebind(&mut app, Some(scope.clone()));
    Open {
        stub,
        thread,
        markdown,
        scope,
        _conversation: conversation,
    }
}

fn thread(id: &str, title: &str) -> artisan_api::Thread {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "project_id": "project",
        "title": title,
        "config": {"engine": "claude"},
        "activity_seq": 1,
        "read_seq": 1,
        "seq": 1,
        "created_at": 0,
        "updated_at": 0,
        "archived": false,
    }))
    .expect("thread")
}

fn user(item: &str) -> BodyKey {
    BodyKey {
        item: ItemId(item.into()),
        slot: Slot::Text(TextSection::User),
    }
}

fn skill(name: &str) -> CatalogSkill {
    serde_json::from_value(serde_json::json!({
        "id": name,
        "name": name,
        "origin": "user",
        "enabled": true,
        "file_count": 1,
        "byte_count": 1,
        "engines": [],
    }))
    .expect("skill")
}

fn prepare(open: &mut Open, engine: &MarkdownEngine, item: &str, text: &str) {
    open.markdown
        .prepare_on_realization(
            &open.scope,
            user(item),
            item,
            engine,
            BodyText::Borrowed(text),
            1,
        )
        .expect("prepared message");
}

fn remarked(open: &Open) -> Vec<BodyKey> {
    let mut keys: Vec<BodyKey> = open.markdown.mention_epochs().into_keys().collect();
    keys.sort();
    keys
}

const MESSAGES: [(&str, &str); 5] = [
    ("chip", "please run $review now"),
    ("code", "please keep ` $review` literal"),
    ("plain", "nothing to see"),
    ("quote", ">$review"),
    ("table", "| a | b |\n| --- | --- |\n|$review|other|"),
];

#[test]
fn a_new_skill_changes_only_the_messages_that_show_it_as_a_chip() {
    let mut open = open();
    let engine = MarkdownEngine::new().expect("markdown engine");
    let start = Instant::now();
    open.markdown
        .sync_mentions(open.stub.host(), &open.thread, start);
    for (item, text) in MESSAGES {
        prepare(&mut open, &engine, item, text);
    }
    assert!(remarked(&open).is_empty());

    open.stub.host_mut().skill_catalog = Some(Arc::from(vec![skill("review")]));
    open.markdown
        .sync_mentions(open.stub.host(), &open.thread, start);

    assert_eq!(
        remarked(&open),
        [user("chip"), user("quote"), user("table")]
    );
}

#[test]
fn removing_a_skill_unchips_quoted_and_compact_table_mentions() {
    let mut open = open();
    let engine = MarkdownEngine::new().expect("markdown engine");
    let start = Instant::now();
    open.stub.host_mut().skill_catalog = Some(Arc::from(vec![skill("review")]));
    open.markdown
        .sync_mentions(open.stub.host(), &open.thread, start);
    for (item, text) in MESSAGES {
        prepare(&mut open, &engine, item, text);
    }

    open.stub.host_mut().skill_catalog = Some(Arc::from(Vec::new()));
    open.markdown
        .sync_mentions(open.stub.host(), &open.thread, start);

    assert_eq!(
        remarked(&open),
        [user("chip"), user("quote"), user("table")]
    );
}

#[test]
fn an_unrelated_skill_keeps_every_prepared_message_current() {
    let mut open = open();
    let engine = MarkdownEngine::new().expect("markdown engine");
    let start = Instant::now();
    open.stub.host_mut().skill_catalog = Some(Arc::from(vec![skill("review")]));
    open.markdown
        .sync_mentions(open.stub.host(), &open.thread, start);
    for (item, text) in MESSAGES {
        prepare(&mut open, &engine, item, text);
    }
    let leaves: Vec<_> = MESSAGES
        .iter()
        .map(|(item, _)| {
            open.markdown.bodies[&user(item)]
                .presenter
                .projected_leaf_info(0)
                .expect("a prepared leaf")
        })
        .collect();

    open.stub.host_mut().skill_catalog = Some(Arc::from(vec![skill("review"), skill("unused")]));
    open.markdown
        .sync_mentions(open.stub.host(), &open.thread, start);
    for (item, text) in MESSAGES {
        prepare(&mut open, &engine, item, text);
    }

    assert!(remarked(&open).is_empty());
    for ((item, _), leaf) in MESSAGES.iter().zip(&leaves) {
        assert!(
            open.markdown.bodies[&user(item)]
                .presenter
                .projected_leaf_is_current(0, leaf),
            "{item} was prepared again"
        );
    }
}

#[test]
fn a_rename_inside_the_recheck_window_shows_at_its_end_and_an_idle_host_arms_nothing() {
    let mut open = open();
    open.stub
        .host_mut()
        .threads
        .insert(ThreadId("other".into()), thread("other", "Sidebar lag"));
    let start = Instant::now();
    open.markdown
        .sync_mentions(open.stub.host(), &open.thread, start);
    open.markdown.sync_mentions(
        open.stub.host(),
        &open.thread,
        start + Duration::from_millis(200),
    );
    assert_eq!(open.markdown.recheck_at(), None);

    open.stub
        .host_mut()
        .threads
        .insert(ThreadId("other".into()), thread("other", "Sidebar fix"));
    open.markdown.sync_mentions(
        open.stub.host(),
        &open.thread,
        start + Duration::from_millis(500),
    );
    assert!(
        open.markdown
            .vocabulary()
            .mentions("@Sidebar fix")
            .is_empty()
    );
    assert_eq!(open.markdown.recheck_at(), Some(start + TITLE_RECHECK));

    open.markdown
        .sync_mentions(open.stub.host(), &open.thread, start + TITLE_RECHECK);
    assert_eq!(open.markdown.vocabulary().mentions("@Sidebar fix").len(), 1);
    assert_eq!(open.markdown.recheck_at(), None);
}
