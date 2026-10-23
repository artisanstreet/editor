//! Read state: what the reader saw marks the thread read.

use super::*;

#[gpui::test]
#[expect(
    clippy::too_many_lines,
    reason = "one scenario: mark unread, repaint, then show new content"
)]
fn repaint_preserves_manual_unread_until_new_content_is_seen(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let first = forge
        .runtime
        .block_on(forge.store.insert_user_message(NewItem {
            id: ItemId::new(),
            thread_id: thread.clone(),
            run_id: None,
            parent_id: None,
            native_id: None,
            status: Status::Done,
            kind: ItemKind::UserMessage {
                text: "First visible input".into(),
                images: Vec::new(),
                steer: false,
                from_agent: None,
                sender: None,
            },
        }))
        .unwrap()
        .0;
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    cx.update(|window, _| window.activate_window());
    open(&core, &host, &thread, cx);
    wait(
        cx,
        "first input is actually painted and acknowledged",
        |cx| {
            draw(cx);
            let stored = forge
                .runtime
                .block_on(forge.store.thread(&thread))
                .unwrap()
                .unwrap();
            stored.read_seq >= first.seq && !stored.marked_unread
        },
    );
    forge
        .runtime
        .block_on(forge.store.mark_unread(&thread))
        .unwrap()
        .unwrap();
    wait(cx, "manual unread event reaches the Client", |cx| {
        cx.read(|cx| {
            core.read(cx)
                .host(&host)
                .unwrap()
                .read(cx)
                .threads
                .get(&thread)
                .unwrap()
                .marked_unread
        })
    });
    for _ in 0..20 {
        draw(cx);
        cx.run_until_parked();
    }
    assert!(
        forge
            .runtime
            .block_on(forge.store.thread(&thread))
            .unwrap()
            .unwrap()
            .marked_unread,
        "repainting the same input must preserve the explicit unread mark"
    );
    let next = forge
        .runtime
        .block_on(forge.store.insert_user_message(NewItem {
            id: ItemId::new(),
            thread_id: thread.clone(),
            run_id: None,
            parent_id: None,
            native_id: None,
            status: Status::Done,
            kind: ItemKind::UserMessage {
                text: "New visible input".into(),
                images: Vec::new(),
                steer: false,
                from_agent: None,
                sender: None,
            },
        }))
        .unwrap()
        .0;
    wait(
        cx,
        "new visible evidence advances the acknowledgement",
        |cx| {
            draw(cx);
            let stored = forge
                .runtime
                .block_on(forge.store.thread(&thread))
                .unwrap()
                .unwrap();
            stored.read_seq >= next.seq && !stored.marked_unread
        },
    );
    assert!(
        loaded(&conversation, cx)
            .iter()
            .any(|item| item.id == next.id)
    );
}

/// Reading the final reply marks the thread read, even when a background
/// command that started before it was cancelled after it as the run ended:
/// that newer change sits higher up and must not keep the dot on.
#[gpui::test]
fn reading_the_final_reply_marks_the_thread_read(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let watcher = ItemId::new();
    forge.put(
        &thread,
        &watcher,
        Status::Running,
        shell("tail -f build.log"),
    );
    let item = forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        reply("Current final reply"),
    );
    let cancelled = forge.put(
        &thread,
        &watcher,
        Status::Cancelled,
        shell("tail -f build.log"),
    );
    assert!(cancelled.seq > item.seq && cancelled.position < item.position);
    let run = forge
        .runtime
        .block_on(forge.store.upsert_run(artisan_api::Run {
            config: None,
            id: item.run_id.clone().unwrap(),
            thread_id: thread.clone(),
            engine: EngineId::Claude,
            model: None,
            input_cutoff: 0,
            status: artisan_api::RunStatus::Done,
            usage: artisan_api::Usage::default(),
            session_id: None,
            error: None,
            seq: 0,
            started_at: 0,
            ended_at: Some(1),
            continuation: None,
        }))
        .unwrap();
    assert!(
        run.seq > item.seq,
        "settlement is a later activity boundary than its reply"
    );
    let dir = tempfile::tempdir().unwrap();
    let (core, host, _conversation, cx) = editor(&forge, dir.path(), cx);
    cx.update(|window, _| window.activate_window());
    open(&core, &host, &thread, cx);
    wait(
        cx,
        "current final content acknowledges the completed turn",
        |cx| {
            draw(cx);
            let stored = forge
                .runtime
                .block_on(forge.store.thread(&thread))
                .unwrap()
                .unwrap();
            stored.read_seq >= run.seq && !stored.is_unread()
        },
    );
}

/// A turn's note is stored after its run settles and shows only in the
/// composer strip: opening the thread afterwards still marks it read.
#[gpui::test]
fn a_note_after_the_run_settles_does_not_keep_the_thread_unread(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    forge.put(&thread, &ItemId::new(), Status::Done, reply("Final reply"));
    forge.finish_run(&thread);
    let settled = forge
        .runtime
        .block_on(forge.store.thread(&thread))
        .unwrap()
        .unwrap()
        .activity_seq;
    let note = forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        ItemKind::Notice {
            kind: artisan_api::NoticeKind::YouShouldKnow { heads_up: true },
            text: "The agent told the other thread to merge to master.".into(),
        },
    );
    assert!(note.seq > settled, "the note lands after settlement");
    let dir = tempfile::tempdir().unwrap();
    let (core, host, _conversation, cx) = editor(&forge, dir.path(), cx);
    cx.update(|window, _| window.activate_window());
    open(&core, &host, &thread, cx);
    wait(cx, "the final reply acknowledges the settled turn", |cx| {
        draw(cx);
        let stored = forge
            .runtime
            .block_on(forge.store.thread(&thread))
            .unwrap()
            .unwrap();
        stored.read_seq >= settled && !stored.is_unread()
    });
}

/// A run that fails before writing anything (an engine that can't resume
/// its session) ends at the message that started it: reading that message
/// clears the failure's dot.
#[gpui::test]
fn reading_a_run_that_failed_without_output_marks_the_thread_read(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        reply("Earlier reply"),
    );
    forge.finish_run(&thread);
    let message = forge
        .runtime
        .block_on(forge.store.upsert_item(NewItem {
            id: ItemId::new(),
            thread_id: thread.clone(),
            run_id: None,
            parent_id: None,
            native_id: None,
            status: Status::Running,
            kind: ItemKind::UserMessage {
                text: "Try again".into(),
                images: Vec::new(),
                steer: false,
                from_agent: None,
                sender: None,
            },
        }))
        .unwrap();
    let run = forge
        .runtime
        .block_on(forge.store.upsert_run(artisan_api::Run {
            config: None,
            id: RunId::from("failed"),
            thread_id: thread.clone(),
            engine: EngineId::Codex,
            model: None,
            input_cutoff: message.seq,
            status: artisan_api::RunStatus::Failed,
            usage: artisan_api::Usage::default(),
            session_id: None,
            error: Some("Codex thread/resume failed".into()),
            seq: 0,
            started_at: 0,
            ended_at: Some(1),
            continuation: None,
        }))
        .unwrap();
    assert!(
        run.seq > message.seq,
        "the failure is newer than the message"
    );
    let dir = tempfile::tempdir().unwrap();
    let (core, host, _conversation, cx) = editor(&forge, dir.path(), cx);
    cx.update(|window, _| window.activate_window());
    open(&core, &host, &thread, cx);
    wait(cx, "reading the message acknowledges the failure", |cx| {
        draw(cx);
        let stored = forge
            .runtime
            .block_on(forge.store.thread(&thread))
            .unwrap()
            .unwrap();
        stored.read_seq >= run.seq && !stored.is_unread()
    });
}
