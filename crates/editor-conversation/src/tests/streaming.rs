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
    let replies = cx.read(|cx| {
        conversation
            .read(cx)
            .rows
            .iter()
            .filter(|(row, _)| matches!(row, Row::ProseBlock { id: item, block: 0 } if *item == id))
            .count()
    });
    assert_eq!(replies, 1, "one row for the reply");
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
    let animating = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            conversation
                .read(cx)
                .row_views
                .iter()
                .flatten()
                .any(|row| row.read(cx).animating)
        })
    };
    wait(cx, "the thread to land", |cx| {
        draw(cx);
        cx.read(|cx| {
            let c = conversation.read(cx);
            !c.landing() && c.items().count() == 8
        })
    });
    draw(cx);
    let fresh = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            let c = conversation.read(cx);
            let view = c.view.as_ref().unwrap();
            view.items
                .values()
                .filter(|item| view.ui.get(&item.id).is_some_and(ItemUi::fresh))
                .count()
        })
    };
    assert_eq!(
        fresh(cx),
        0,
        "what arrived while it opened would animate in"
    );
    assert!(!animating(cx), "what arrived while it opened animated in");
    cx.update(|window, _| window.activate_window());
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        reply("Arrived while watched, word by word."),
    );
    wait(cx, "the new reply", |cx| {
        draw(cx);
        cx.read(|cx| conversation.read(cx).items().count() == 9)
    });
    settle(cx, &conversation);
    assert_eq!(
        fresh(cx),
        1,
        "only the reply that arrived while watched is new"
    );
}

/// Reduced motion and content without prose must release the next native row.
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
            cx.read(|cx| {
                let c = conversation.read(cx);
                c.rows.iter().enumerate().any(|(index, (row, _))| {
                    row.anchor() == Some(&next)
                        && c.row_views.get(index).is_some_and(Option::is_some)
                        && c.list
                            .bounds_for_item(index)
                            .is_some_and(|bounds| bounds.size.height > gpui::px(0.0))
                })
            })
        });
        cx.read(|cx| {
            assert!(
                conversation
                    .read(cx)
                    .view
                    .as_ref()
                    .unwrap()
                    .ui
                    .get(&separator)
                    .is_some_and(|ui| ui.arrived.is_some()),
                "the separator must arrive while watched"
            );
        });
        settle(cx, &conversation);
    }
}

/// Text streams as commentary inside the live work until the run ends, so
/// the section must stay open: folding it for "replying" left a bare header.
#[test]
fn text_streaming_in_live_work_stays_on_screen() {
    use crate::fold::fold;
    use crate::thread::ThreadView;
    use crate::viewport::Fetch;
    use artisan_api::{ItemsPage, Run, RunStatus, Usage};

    let thread = ThreadId::from("thread");
    let run = RunId::from("run");
    let item = |position: u64, status: Status, kind: ItemKind| Item {
        id: ItemId::new(),
        thread_id: thread.clone(),
        position,
        seq: position,
        run_id: Some(run.clone()),
        parent_id: None,
        status,
        created_at: 0,
        updated_at: 0,
        kind,
    };
    let commentary = item(
        3,
        Status::Running,
        ItemKind::AssistantMessage {
            text: "Looking at what master did".into(),
            phase: Phase::Commentary,
        },
    );
    let items = vec![
        item(
            1,
            Status::Done,
            ItemKind::UserMessage {
                text: "fix it".into(),
                images: Vec::new(),
                steer: false,
            },
        ),
        item(2, Status::Done, shell("rg follow")),
        commentary.clone(),
    ];
    let (mut view, _) = ThreadView::open(thread.clone());
    view.apply_page(
        Fetch::Tail,
        ItemsPage {
            items,
            runs: Vec::new(),
            more: false,
        },
    );
    let live = Run {
        id: run.clone(),
        thread_id: thread.clone(),
        engine: EngineId::Claude,
        model: None,
        input_cutoff: 0,
        status: RunStatus::Running,
        usage: Usage::default(),
        session_id: None,
        error: None,
        seq: 3,
        started_at: 0,
        ended_at: None,
    };
    let folded = fold(&view, Some(&live), None);
    assert!(folded.runs[&run].open, "the live section is open");
    assert!(
        folded.rows.iter().any(|(row, _)| matches!(
            row,
            Row::Work { entries, .. } if entries.contains(&commentary.id)
        )),
        "the streaming text is one of the open section's rows"
    );
}

/// Later tools cannot reclassify an already canonical final message.
#[test]
fn canonical_final_messages_keep_their_phase_when_later_work_arrives() {
    use crate::fold::fold;
    use crate::thread::ThreadView;
    use crate::viewport::Fetch;
    use artisan_api::ItemsPage;

    let thread = ThreadId::from("thread");
    let run = RunId::from("run");
    let item = |position: u64, kind: ItemKind| Item {
        id: ItemId::new(),
        thread_id: thread.clone(),
        position,
        seq: position,
        run_id: Some(run.clone()),
        parent_id: None,
        status: Status::Done,
        created_at: 0,
        updated_at: 0,
        kind,
    };
    let early = item(3, reply("The first result is available."));
    let last = item(5, reply("All tests pass."));
    let items = vec![
        item(
            1,
            ItemKind::UserMessage {
                text: "fix it".into(),
                images: Vec::new(),
                steer: false,
            },
        ),
        item(2, shell("cargo test")),
        early.clone(),
        item(4, shell("cargo test")),
        last.clone(),
    ];
    let (mut view, _) = ThreadView::open(thread.clone());
    view.apply_page(
        Fetch::Tail,
        ItemsPage {
            items,
            runs: Vec::new(),
            more: false,
        },
    );
    let rows: Vec<Row> = fold(&view, None, None)
        .rows
        .into_iter()
        .map(|(row, _)| row)
        .collect();
    let works: Vec<&Row> = rows
        .iter()
        .filter(|row| matches!(row, Row::Work { .. }))
        .collect();
    assert_eq!(
        works.len(),
        2,
        "two work stretches around the first reply: {rows:?}"
    );
    assert_eq!(
        rows[2],
        Row::ProseBlock {
            id: early.id.clone(),
            block: 0
        }
    );
    assert!(
        works
            .iter()
            .all(|row| matches!(row, Row::Work { entries, .. } if !entries.contains(&early.id)))
    );
    assert_eq!(
        rows.last(),
        Some(&Row::ProseBlock {
            id: last.id.clone(),
            block: 0
        }),
        "the last text is the reply"
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
            },
        );
    }
    let message = forge.put(
        &thread,
        &ItemId::new(),
        Status::Running,
        reply("Initial paragraph"),
    );
    let key: gpui::SharedString = message.id.to_string().into();
    let target = artisan_api::LiveTarget::Text {
        item_id: message.id.clone(),
    };
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "initial prose leaf", |cx| {
        draw(cx);
        cx.read(|cx| {
            conversation
                .read(cx)
                .rows
                .iter()
                .any(|(row, _)| matches!(row, Row::ProseBlock { id, .. } if id == &message.id))
        })
    });
    settle(cx, &conversation);
    let markers = cx.read(|cx| conversation.read(cx).navigator_markers.clone().unwrap());
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
    wait(
        cx,
        "replacement is prepared from the current immutable live snapshot",
        |cx| {
            draw(cx);
            cx.read(|cx| {
                let c = conversation.read(cx);
                c.view
                    .as_ref()
                    .and_then(|view| view.live.get(&target))
                    .is_some_and(|source| {
                        source.to_string() == "Replacement paragraph"
                            && c.shared.markdown.is_rope_source_prepared(&key, source)
                    })
            })
        },
    );
    cx.read(|cx| {
        assert!(
            Arc::ptr_eq(
                &markers,
                conversation.read(cx).navigator_markers.as_ref().unwrap()
            ),
            "unchanged body shape must not refold unrelated turns or discard navigator preparation"
        )
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
    wait(
        cx,
        "new semantic leaf changes native list structure",
        |cx| {
            draw(cx);
            cx.read(|cx| {
                let c = conversation.read(cx);
                let leaves = c
                    .rows
                    .iter()
                    .filter(
                        |(row, _)| matches!(row, Row::ProseBlock { id, .. } if id == &message.id),
                    )
                    .count();
                leaves == 2
                    && c.view
                        .as_ref()
                        .and_then(|view| view.live.get(&target))
                        .is_some_and(|source| {
                            c.shared.markdown.is_rope_source_prepared(&key, source)
                        })
            })
        },
    );
    cx.read(|cx| {
        let c = conversation.read(cx);
        let last = c
            .rows
            .iter()
            .rev()
            .find_map(|(row, _)| {
                matches!(row, Row::ProseBlock { id, .. } if id == &message.id).then_some(row)
            })
            .unwrap();
        assert_eq!(c.read_evidence(last, cx), Some(message.seq));
    });
    let retained = "Replacement paragraph\n\nA second paragraph".len() as u64;
    forge.store.publish_live(artisan_api::Event::Live {
        thread_id: thread.clone(),
        target: target.clone(),
        text: String::new().into(),
        append: true,
        coverage: artisan_api::LiveCoverage {
            start: 0,
            total: retained + 1_000_000,
        },
    });
    wait(
        cx,
        "incomplete live coverage reaches the current prepared leaf",
        |cx| {
            draw(cx);
            cx.read(|cx| {
                let c = conversation.read(cx);
                let snapshot = core
                    .read(cx)
                    .host(&host)
                    .unwrap()
                    .read(cx)
                    .live_snapshot(&thread, &target);
                snapshot.is_some_and(|snapshot| snapshot.coverage.total == retained + 1_000_000)
                    && c.view
                        .as_ref()
                        .and_then(|view| view.live.get(&target))
                        .is_some_and(|source| {
                            c.shared.markdown.is_rope_source_prepared(&key, source)
                        })
            })
        },
    );
    cx.read(|cx| {
        let c = conversation.read(cx);
        let last = c
            .rows
            .iter()
            .rev()
            .find_map(|(row, _)| {
                matches!(row, Row::ProseBlock { id, .. } if id == &message.id).then_some(row)
            })
            .unwrap();
        assert_eq!(
            c.read_evidence(last, cx),
            None,
            "a prepared prefix cannot acknowledge omitted live content"
        );
    });
}
