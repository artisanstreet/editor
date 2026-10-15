//! Read state: what the reader saw marks the thread read.

use super::*;

#[gpui::test]
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

#[gpui::test]
fn reading_the_final_reply_marks_the_thread_read(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let item = forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        reply("Current final reply"),
    );
    let run = forge
        .runtime
        .block_on(forge.store.upsert_run(artisan_api::Run {
            id: item.run_id.clone().unwrap(),
            thread_id: thread.clone(),
            engine: EngineId::Claude,
            model: None,
            input_cutoff: 0,
            status: artisan_api::RunStatus::Done,
            usage: Default::default(),
            session_id: None,
            error: None,
            seq: 0,
            started_at: 0,
            ended_at: Some(1),
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
