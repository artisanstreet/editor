//! Bodies too long to keep inline, read from Forge a window at a time.

use super::*;

#[test]
fn copying_a_long_message_keeps_every_character() {
    for text in [
        "".to_owned(),
        "界🙂".repeat(500),
        "first\n\n  second\t🙂 third ".repeat(200),
    ] {
        let ranges = crate::fold::plain_ranges(&text);
        assert!(ranges.iter().all(|range| range.len() <= 1024));
        assert!(ranges.iter().all(|range| text.is_char_boundary(range.start) && text.is_char_boundary(range.end)));
        let rebuilt = ranges
            .iter()
            .map(|range| &text[range.clone()])
            .collect::<String>();
        assert_eq!(
            rebuilt, text,
            "literal source never drops whitespace at native boundaries"
        );
    }
}

#[gpui::test]
fn a_long_message_reads_only_once_its_end_paints(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let user = ItemId::new();
    let error = ItemId::new();
    let user_text = "A long literal user message with UTF8 界🙂.\n".repeat(8000);
    forge.put(
        &thread,
        &user,
        Status::Done,
        ItemKind::UserMessage {
            text: user_text.into(),
            images: Vec::new(),
            steer: false,
        },
    );
    forge.put(
        &thread,
        &error,
        Status::Failed,
        ItemKind::Error {
            message: "A long error explaining the failed operation.\n"
                .repeat(8000)
                .into(),
            kind: artisan_api::ErrorKind::Execution,
            retryable: false,
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "literal items arrive", |cx| {
        loaded(&conversation, cx).iter().any(|item| item.id == user)
            && loaded(&conversation, cx)
                .iter()
                .any(|item| item.id == error)
    });
    for (id, section) in [
        (&user, crate::fold::PlainSection::User),
        (&error, crate::fold::PlainSection::Error),
    ] {
        // Use the production navigation intent. A raw native scroll alone
        // leaves Following active and the next frame correctly returns to tail.
        conversation.update(cx, |c, cx| {
            if section == crate::fold::PlainSection::Error {
                // The error is the newest Item. Navigate to the latest thread
                // tail, rather than its first leaf where older-page prefetch
                // correctly starts before this test can inspect the source.
                c.jump_to_latest(cx);
            } else {
                let item = c.view.as_ref().unwrap().item(id).unwrap().clone();
                c.jump_to(&item, cx);
            }
        });
        conversation.update(cx, |c, _| {
            let index = c
                .rows
                .iter()
                .rposition(|(row, _)| row.anchor() == Some(id))
                .unwrap();
            c.list.scroll_to(gpui::ListOffset {
                item_ix: index,
                offset_in_item: gpui::px(0.0),
            });
        });
        wait(cx, "latest literal source page", |cx| {
            draw(cx);
            cx.read(|cx| {
                let c = conversation.read(cx);
                let sources = c.shared.stored_sources.borrow();
                let Some(source) = sources.get(&(id.clone(), section.slot())) else { return false; };
                let Some(item) = c.view.as_ref().and_then(|view| view.item(id)) else { return false; };
                source.format() == artisan_api::SourceIndexFormat::Literal

                    && section.text(item).is_some_and(|text| matches!(text,
                        artisan_api::TextContent::Stored(reference) if reference == source.source_ref()))
                    && source.block_count() == 8001
                    && source.loaded_range().end == source.block_count()
            })
        });
        conversation.update(cx, |c, _| {
            let index = c
                .rows
                .iter()
                .rposition(|(row, _)| row.anchor() == Some(id))
                .unwrap();
            c.list.scroll_to(gpui::ListOffset {
                item_ix: index,
                offset_in_item: gpui::px(0.0),
            });
        });
        wait(cx, "actual final literal body ready", |cx| {
            draw(cx);
            cx.read(|cx| {
                let c = conversation.read(cx);
                let row = c
                    .rows
                    .iter()
                    .rev()
                    .find(|(row, _)| row.anchor() == Some(id))
                    .unwrap();
                c.read_evidence(&row.0, cx)
                    == c.view.as_ref().unwrap().item(id).map(|item| item.seq)
            })
        });
        cx.read(|cx| {
            let c = conversation.read(cx);
            let sources = c.shared.stored_sources.borrow();
            let source = sources.get(&(id.clone(), section.slot())).unwrap();
            assert!(
                source.block_count() > 600,
                "original semantic source is never truncated"
            );
            let leaves: Vec<_> = c
                .rows
                .iter()
                .map(|(row, _)| row)
                .filter(|row| matches!(row, Row::PlainBlock { id: row_id, .. } if row_id == id))
                .collect();
            assert_eq!(leaves.len(), source.loaded_range().len());
            assert!(
                leaves.len() <= 64,
                "only the source page enters the native list"
            );
            assert_eq!(
                c.read_evidence(
                    &Row::PlainBlock {
                        id: id.clone(),
                        section,
                        block: 0
                    },
                    cx
                ),
                None,
                "the original first ordinal cannot acknowledge the entire source"
            );
            assert_eq!(
                c.read_evidence(leaves[0], cx),
                None,
                "an early current-page ordinal cannot acknowledge the entire source"
            );
            assert_eq!(
                c.read_evidence(leaves[leaves.len() - 1], cx),
                c.view.as_ref().unwrap().item(id).map(|item| item.seq)
            );
            assert!(
                c.row_views.iter().flatten().count() <= 128,
                "viewport and bounded native cache own mounted views"
            );
            assert!(
                c.rows
                    .iter()
                    .filter(|(row, _)| matches!(row, Row::PlainBlock { .. }))
                    .count()
                    <= 128,
                "two long literal Items register two bounded source pages"
            );
        });
    }
    // Reading at the start of the latest source page loads older leeway. A
    // viewport taller than the 32-row overlap must not fetch back and forth
    // merely because the splice exposes the opposite edge.
    conversation.update(cx, |c, cx| {
        let item = c.view.as_ref().unwrap().item(&user).unwrap().clone();
        c.jump_to(&item, cx);
    });
    wait(cx, "older literal source leeway", |cx| {
        draw(cx);
        cx.read(|cx| {
            conversation
                .read(cx)
                .shared
                .stored_sources
                .borrow()
                .get(&(user.clone(), crate::fold::PlainSection::User.slot()))
                .is_some_and(|source| source.loaded_range().start < 8001 - 64)
        })
    });
    let settled = cx.read(|cx| {
        let c = conversation.read(cx);
        (
            c.shared
                .stored_sources
                .borrow()
                .get(&(user.clone(), crate::fold::PlainSection::User.slot()))
                .unwrap()
                .loaded_range(),
            c.rows[c.list.logical_scroll_top().item_ix].0.key(),
        )
    });
    for _ in 0..6 {
        draw(cx);
        cx.run_until_parked();
        cx.read(|cx| {
            let c = conversation.read(cx);
            assert_eq!(
                c.rows[c.list.logical_scroll_top().item_ix].0.key(),
                settled.1
            );
            assert_eq!(
                c.shared
                    .stored_sources
                    .borrow()
                    .get(&(user.clone(), crate::fold::PlainSection::User.slot()))
                    .unwrap()
                    .loaded_range(),
                settled.0,
                "repainting the same reader position must not reverse source paging"
            );
        });
    }
    conversation.update(cx, |c, cx| {
        let index = c
            .rows
            .iter()
            .rposition(|(row, _)| row.anchor() == Some(&user))
            .unwrap();
        c.list.scroll_to(gpui::ListOffset {
            item_ix: index,
            offset_in_item: gpui::px(0.0),
        });
        cx.notify();
    });
    wait(cx, "reader movement permits newer source paging", |cx| {
        draw(cx);
        cx.read(|cx| {
            conversation
                .read(cx)
                .shared
                .stored_sources
                .borrow()
                .get(&(user.clone(), crate::fold::PlainSection::User.slot()))
                .is_some_and(|source| {
                    source.loaded_range().end == 8001 && source.loaded_range().len() <= 64
                })
        })
    });
}

#[test]
fn stored_messages_and_agent_reports_keep_their_rows() {
    let source = artisan_api::TextContent::Stored(artisan_api::ContentRef {
        id: artisan_api::ContentId("a".repeat(64)),
        bytes: 100_000,
        lines: 1000,
    });
    let mut item = Item {
        id: ItemId::new(),
        thread_id: ThreadId::new(),
        position: 17,
        seq: 41,
        run_id: None,
        parent_id: None,
        status: Status::Done,
        created_at: 0,
        updated_at: 0,
        kind: ItemKind::UserMessage {
            text: source.clone(),
            images: Vec::new(),
            steer: false,
        },
    };
    assert_eq!(crate::fold::PlainSection::User.text(&item), Some(&source));
    item.kind = ItemKind::Agent {
        name: None,
        description: "Task".into(),
        prompt: Some(source.clone()),
        agent_id: None,
        context_used: None,
        model: None,
        background: false,
        result: Some(source.clone()),
        label: None,
    };
    assert_eq!(
        crate::fold::PlainSection::AgentTask.text(&item),
        Some(&source)
    );
    assert_eq!(crate::fold::agent_report(&item), Some(&source));
    assert_eq!(
        crate::fold::TextSection::AgentResult.text(&item),
        Some(&source)
    );
    assert_eq!(
        item.position, 17,
        "source storage never changes Item ordering"
    );
}

#[gpui::test]
fn a_preview_of_a_stored_reply_does_not_mark_it_read(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let id = ItemId::new();
    let text = // A giant unbroken physical line exceeds literal raw-leaf admission.
    // Its descriptor/error preview must never masquerade as complete body paint.
    "Long literal source 界🙂 with preserved whitespace. ".repeat(2000);
    forge.put(
        &thread,
        &id,
        Status::Done,
        ItemKind::UserMessage {
            text: text.into(),
            images: Vec::new(),
            steer: false,
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "stored message remains a real native row", |cx| {
        draw(cx);
        loaded(&conversation, cx).iter().any(|item| item.id == id)
    });
    settle(cx, &conversation);
    cx.read(|cx| {
        let conversation = conversation.read(cx);
        let item = conversation.view.as_ref().unwrap().item(&id).unwrap();
        assert!(matches!(
            &item.kind,
            ItemKind::UserMessage {
                text: artisan_api::TextContent::Stored(_),
                ..
            }
        ));
        let row = conversation
            .rows
            .iter()
            .map(|(row, _)| row)
            .find(|row| row.anchor() == Some(&id))
            .expect("stored source has a transcript row");
        assert_eq!(
            conversation.read_evidence(row, cx),
            None,
            "loading and raw preview are not evidence of final formatted content paint"
        );
    });
}

#[gpui::test]
fn a_long_stored_reply_opens_at_its_end_and_reads_once_its_last_block_paints(
    cx: &mut TestAppContext,
) {
    let forge = Forge::start();
    let thread = forge.thread();
    let text = (0..750)
        .map(|index| {
            format!("Paragraph **{index}** retains its authored *formatting* and source order.\n\n")
        })
        .collect::<String>();
    let message = forge.put(&thread, &ItemId::new(), Status::Done, reply(&text));
    let ItemKind::AssistantMessage {
        text: artisan_api::TextContent::Stored(reference),
        ..
    } = &message.kind
    else {
        panic!("fixture must exercise immutable Stored text, not inline preparation");
    };
    assert!(reference.bytes > artisan_api::INLINE_TEXT_BYTES as u64);
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(
        cx,
        "bounded stored semantic rows with complete logical topology",
        |cx| {
            draw(cx);
            cx.read(|cx| {
                let c = conversation.read(cx);
                let sources = c.shared.stored_sources.borrow();
                sources.get(&(message.id.clone(), crate::fold::Slot::Prose)).is_some_and(|source| {
                    source.source_ref() == reference
                        && source.block_count() == 750
                        && !source.loaded_range().is_empty()
                        && source.loaded_range().len() <= 64
                        && c.rows.iter().filter(|(row, _)| matches!(row, Row::ProseBlock { id, .. } if id == &message.id)).count() == source.loaded_range().len()
                })
            })
        },
    );
    cx.read(|cx| {
        let conversation = conversation.read(cx);
        let sources = conversation.shared.stored_sources.borrow();
        let source = sources
            .get(&(message.id.clone(), crate::fold::Slot::Prose))
            .unwrap();
        assert_eq!(source.source_ref(), reference);
        assert_eq!(source.block_count(), 750);
        assert!(
            conversation.row_views.iter().flatten().count() < 128,
            "source topology must not create 750 mounted views"
        );
        assert_eq!(
            conversation.read_evidence(
                &Row::ProseBlock {
                    id: message.id.clone(),
                    block: 0
                },
                cx
            ),
            None,
            "an indexed early leaf cannot acknowledge the final body"
        );
        let final_key: gpui::SharedString = format!("{}:prose:leaf:749", message.id).into();
        if !conversation
            .shared
            .markdown
            .is_stored_leaf_prepared(&final_key, source, 749)
        {
            assert_eq!(
                conversation.read_evidence(
                    &Row::ProseBlock {
                        id: message.id.clone(),
                        block: 749
                    },
                    cx
                ),
                None,
                "topology alone is not read evidence"
            );
        }
    });
}

#[gpui::test]
fn one_huge_stored_line_shows_its_end_without_laying_out_all_of_it(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let id = ItemId::new();
    let text = "original native glyphs 界🙂 ".repeat(1500);
    assert!(text.len() > 16 * 1024 && text.len() < 64 * 1024);
    let item = forge.put(
        &thread,
        &id,
        Status::Done,
        ItemKind::UserMessage {
            text: text.into(),
            images: Vec::new(),
            steer: false,
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "bounded original native glyph paragraph tail", |cx| {
        draw(cx);
        cx.read(|cx| {
            let c = conversation.read(cx);
            let key = Row::PlainBlock {
                id: id.clone(),
                section: crate::fold::PlainSection::User,
                block: 0,
            }
            .key();
            let states = c.shared.native_windows.borrow();
            let Some(state) = states.get(&key) else {
                return false;
            };
            let last = c
                .rows
                .iter()
                .rev()
                .find(|(row, _)| row.anchor() == Some(&id));
            state.count > 128
                && state.window.end == state.count
                && last.is_some_and(|(row, _)| c.read_evidence(row, cx) == Some(item.seq))
        })
    });
    cx.read(|cx| {
        let c = conversation.read(cx);
        let key = Row::PlainBlock {
            id: id.clone(),
            section: crate::fold::PlainSection::User,
            block: 0,
        }
        .key();
        let sources = c.shared.stored_sources.borrow();
        let source = sources
            .get(&(
                id.clone(),
                crate::fold::Slot::Plain(crate::fold::PlainSection::User),
            ))
            .unwrap();
        assert_eq!(source.format(), artisan_api::SourceIndexFormat::Literal);
        assert_eq!(
            source.block_count(),
            1,
            "the literal remains one semantic hard line"
        );
        let states = c.shared.native_windows.borrow();
        let state = states.get(&key).unwrap();
        assert!(
            state.count > 128,
            "real native wrapping supplies the visual topology"
        );
        let rows: Vec<_> = c
            .rows
            .iter()
            .map(|(row, _)| row)
            .filter(|row| row.anchor() == Some(&id))
            .collect();
        assert_eq!(rows.len(), state.window.len());
        assert!(rows.len() <= crate::row::NATIVE_BODY_WINDOW);
        assert!(c.row_views.iter().flatten().count() <= 128);
        assert_eq!(
            c.read_evidence(rows[0], cx),
            None,
            "early painted glyph rows are not final source proof"
        );
        assert_eq!(rows.last().unwrap().visual(), Some(state.count - 1));
        assert_eq!(c.read_evidence(rows.last().unwrap(), cx), Some(item.seq));
    });
}
