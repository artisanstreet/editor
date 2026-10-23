//! Items and text arriving while the thread is open, and how they reveal.

use super::*;

#[gpui::test]
fn items_stream_into_the_open_thread_only(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let other = forge.thread();
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        ItemKind::UserMessage {
            text: "hi".into(),
            images: Vec::new(),
            steer: false,
            from_agent: None,
            sender: None,
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the thread", |cx| loaded(&conversation, cx).len() == 1);

    // A reply streams in: created, then updated twice.
    let elsewhere = forge.put(&other, &ItemId::new(), Status::Done, reply("not here"));
    let id = ItemId::new();
    forge.put(&thread, &id, Status::Running, reply("Hel"));
    forge.put(&thread, &id, Status::Running, reply("Hello"));
    let last = forge.put(&thread, &id, Status::Done, reply("Hello there"));
    wait(cx, "the finished reply", |cx| {
        loaded(&conversation, cx).contains(&last)
    });

    draw(cx);
    let items = loaded(&conversation, cx);
    assert_eq!(items.len(), 2, "the reply shows once: {items:#?}");
    assert!(
        !items.iter().any(|item| item.id == elsewhere.id),
        "another thread's item stays out"
    );
    assert!(
        shown(cx, format!("prose:{id}:0")).is_some(),
        "the reply shows"
    );
}

/// Only what arrives while the reader watches animates in. Opening a
/// thread whose run is live, items that come over the live stream while it
/// lands are history: they appear at once. One arriving after it landed
/// streams in.
#[gpui::test]
fn only_what_arrives_while_watched_animates_in(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    for turn in 0..3 {
        forge.put(
            &thread,
            &ItemId::new(),
            Status::Done,
            reply(&format!("Answer {turn}.")),
        );
    }
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    // Live items race the first page.
    for step in 0..5 {
        forge.put(
            &thread,
            &ItemId::new(),
            Status::Done,
            reply(&format!("Live {step}.")),
        );
    }
    let animating = |cx: &mut VisualTestContext| cx.debug_bounds("animating").is_some();
    wait(cx, "the thread to land", |cx| {
        draw(cx);
        cx.read(|cx| {
            let c = conversation.read(cx);
            !c.landing() && c.items().count() == 8
        })
    });
    for _ in 0..10 {
        draw(cx);
        assert!(!animating(cx), "what arrived while it opened animated in");
    }
    cx.update(|window, _| window.activate_window());
    let watched = forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        reply("Arrived while watched, word by word."),
    );
    wait(cx, "the new reply to animate in", |cx| {
        draw(cx);
        shown(cx, format!("prose:{}:0", watched.id)).is_some() && animating(cx)
    });
    settle(cx, &conversation);
}

/// Content without prose never holds back the reply after it, with or
/// without reduced motion.
#[gpui::test]
fn fresh_non_prose_content_never_blocks_later_replies(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        reply("Initial reply."),
    );
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the initial reply", |cx| {
        draw(cx);
        loaded(&conversation, cx).len() == 1
    });
    settle(cx, &conversation);
    cx.update(|window, _| window.activate_window());
    for reduced in [true, false] {
        cx.update(|_, cx| cx.set_reduce_motion(reduced));
        let separator = ItemId::new();
        let next = ItemId::new();
        forge.put(&thread, &separator, Status::Done, reply("---"));
        forge.put(
            &thread,
            &next,
            Status::Done,
            reply("This reply must remain visible."),
        );
        wait(cx, "the reply after non-prose content", |cx| {
            draw(cx);
            shown(cx, format!("prose:{next}:0"))
                .is_some_and(|bounds| bounds.size.height > gpui::px(0.0))
        });
        settle(cx, &conversation);
    }
}

/// Text streams as commentary inside the live work until the run ends, so
/// the section must stay open: folding it for "replying" left a bare header.
#[gpui::test]
fn text_streaming_in_live_work_stays_on_screen(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        ItemKind::UserMessage {
            text: "fix it".into(),
            images: Vec::new(),
            steer: false,
            from_agent: None,
            sender: None,
        },
    );
    forge.put(&thread, &ItemId::new(), Status::Done, shell("rg follow"));
    let commentary = forge.put(
        &thread,
        &ItemId::new(),
        Status::Running,
        ItemKind::AssistantMessage {
            text: "Looking at what master did".into(),
            phase: Phase::Commentary,
        },
    );
    forge
        .runtime
        .block_on(forge.store.upsert_run(artisan_api::Run {
            config: None,
            id: RunId::from("run"),
            thread_id: thread.clone(),
            engine: EngineId::Claude,
            model: None,
            input_cutoff: 0,
            status: artisan_api::RunStatus::Running,
            usage: artisan_api::Usage::default(),
            session_id: None,
            error: None,
            seq: 0,
            started_at: 0,
            ended_at: None,
            continuation: None,
        }))
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the streaming commentary in the open work", |cx| {
        draw(cx);
        let _ = &conversation;
        shown(cx, "work:run:0").is_some()
            && shown(cx, format!("prose:{}:0", commentary.id)).is_some()
    });
}

/// Later tools cannot reclassify an already canonical final message.
#[gpui::test]
fn canonical_final_messages_keep_their_phase_when_later_work_arrives(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let ask = forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        ItemKind::UserMessage {
            text: "fix it".into(),
            images: Vec::new(),
            steer: false,
            from_agent: None,
            sender: None,
        },
    );
    forge.put(&thread, &ItemId::new(), Status::Done, shell("cargo test"));
    let early = forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        reply("The first result is available."),
    );
    forge.put(&thread, &ItemId::new(), Status::Done, shell("cargo test"));
    let last = forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        reply("All tests pass."),
    );
    forge.finish_run(&thread);
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    let rows = [
        format!("user:{}:0", ask.id),
        "work:run:0".to_owned(),
        format!("prose:{}:0", early.id),
        format!("prose:{}:0", last.id),
    ];
    wait(cx, "the turn", |cx| {
        draw(cx);
        let _ = &conversation;
        painted_order(cx, &rows).len() == rows.len()
    });
    assert_eq!(
        painted_order(cx, &rows),
        rows,
        "the first reply stays a reply between the work, the last one ends the turn"
    );
}

#[gpui::test]
fn streaming_text_redraws_only_its_own_reply(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    for text in ["First turn", "Second turn"] {
        forge.put(
            &thread,
            &ItemId::new(),
            Status::Done,
            ItemKind::UserMessage {
                text: text.into(),
                images: Vec::new(),
                steer: false,
                from_agent: None,
                sender: None,
            },
        );
    }
    let message = forge.put(
        &thread,
        &ItemId::new(),
        Status::Running,
        reply("Initial paragraph"),
    );
    let target = artisan_api::LiveTarget::Text {
        item_id: message.id.clone(),
    };
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "initial prose leaf", |cx| {
        draw(cx);
        shown(cx, format!("prose:{}:0", message.id)).is_some()
    });
    settle(cx, &conversation);
    forge.store.publish_live(artisan_api::Event::Live {
        thread_id: thread.clone(),
        target: target.clone(),
        text: "Replacement paragraph".into(),
        append: false,
        coverage: artisan_api::LiveCoverage {
            start: 0,
            total: "Replacement paragraph".len() as u64,
        },
    });
    forge.store.publish_live(artisan_api::Event::Live {
        thread_id: thread.clone(),
        target: target.clone(),
        text: "\n\nA second paragraph".into(),
        append: true,
        coverage: artisan_api::LiveCoverage {
            start: 0,
            total: "Replacement paragraph\n\nA second paragraph".len() as u64,
        },
    });
    wait(cx, "the second paragraph shows", |cx| {
        draw(cx);
        shown(cx, format!("prose:{}:1", message.id)).is_some()
    });
    assert!(
        cx.debug_bounds("live-preview").is_none(),
        "the whole reply shows"
    );
    let retained = "Replacement paragraph\n\nA second paragraph".len() as u64;
    forge.store.publish_live(artisan_api::Event::Live {
        thread_id: thread.clone(),
        target,
        text: String::new(),
        append: true,
        coverage: artisan_api::LiveCoverage {
            start: 0,
            total: retained + 1_000_000,
        },
    });
    wait(cx, "the reply says it shows only part of itself", |cx| {
        draw(cx);
        cx.debug_bounds("live-preview").is_some()
    });
    let _ = (&core, &host);
}
