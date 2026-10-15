//! Work sections: their headers, folding, chains and disclosure.

use super::*;

#[gpui::test]
fn finished_work_folds_under_its_header_and_chains_open_on_click(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        ItemKind::UserMessage {
            text: "build it".into(),
            images: Vec::new(),
            steer: false,
        },
    );
    forge.put(&thread, &ItemId::new(), Status::Done, shell("cargo build"));
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        shell("python3 - <<'PY'\nfrom pathlib import Path\nprint(Path.cwd())\nPY"),
    );
    forge.put(&thread, &ItemId::new(), Status::Done, reply("It builds."));
    forge.finish_run(&thread);
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the thread", |cx| loaded(&conversation, cx).len() == 4);
    draw(cx);

    // The run answered, so its work sits folded under "Worked for …".
    assert!(cx.debug_bounds("work-header").is_some(), "the work header");
    assert!(
        cx.debug_bounds("work-chain").is_none(),
        "the work is folded for the reply"
    );

    // Opening the section shows one collapsed chain for both commands.
    click(cx, "work-header");
    settle(cx, &conversation);
    assert!(cx.debug_bounds("work-chain").is_some(), "the command chain");
    assert!(
        cx.debug_bounds("work-activity").is_none(),
        "a chain starts closed"
    );

    // Opening the chain lists the commands on its rail as it grows open.
    click(cx, "work-chain");
    wait(cx, "the chain's commands", |cx| {
        draw(cx);
        cx.debug_bounds("work-activity").is_some()
    });
    settle(cx, &conversation);
    let activity = cx.debug_bounds("work-activity").unwrap();
    assert!(
        activity.size.height <= gpui::px(28.0),
        "a multiline command still occupies one compact summary row: {activity:?}"
    );
    click(cx, "work-chain");
    wait(cx, "the commands collapsed back to one header", |cx| {
        draw(cx);
        cx.debug_bounds("work-chain").is_some()
            && cx.debug_bounds("work-activity").is_none()
            && cx.read(|cx| conversation.read(cx).disclosures.is_empty())
    });
}

/// A turn's work shows between its message and its reply, with its
/// header: an agent of the turn before, resumed by this run and stored
/// before this turn's message, stays in the turn it sits in instead of
/// pulling this run's header up there (which left the real work folded
/// away with no header). A subagent's own lines never show in the thread,
/// even when its agent row isn't loaded.
#[gpui::test]
#[expect(
    clippy::too_many_lines,
    reason = "one scenario: two turns as Forge stored them, and the rows they make"
)]
fn a_resumed_agent_never_splits_the_turn_that_resumed_it(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let (earlier, later) = (RunId::from("earlier"), RunId::from("later"));
    let user = |text: &str| ItemKind::UserMessage {
        text: text.into(),
        images: Vec::new(),
        steer: false,
    };
    let agent = |description: &str| ItemKind::Agent {
        name: None,
        description: description.into(),
        prompt: None,
        agent_id: None,
        context_used: None,
        model: None,
        background: true,
        result: None,
        label: None,
    };
    let id = |name: &str| ItemId::from(name.to_owned());
    forge.put_in(
        &earlier,
        &thread,
        &id("ask"),
        Status::Done,
        user("Bake the lights"),
    );
    // Launched in the earlier turn, resumed (and so filed) by the later run.
    forge.put_in(
        &later,
        &thread,
        &id("thea"),
        Status::Done,
        agent("Runtime bakes"),
    );
    // A line of another agent, whose agent row is not stored here at all.
    forge
        .runtime
        .block_on(forge.store.upsert_item(NewItem {
            id: id("orphan"),
            thread_id: thread.clone(),
            run_id: Some(earlier.clone()),
            parent_id: Some(id("elsewhere")),
            native_id: None,
            status: Status::Done,
            kind: reply("Cascade 0 is sharper than the old map."),
        }))
        .unwrap();
    forge.put_in(
        &earlier,
        &thread,
        &id("baked"),
        Status::Done,
        reply("Baking is in."),
    );
    forge.put_in(
        &later,
        &thread,
        &id("how"),
        Status::Done,
        user("Okay how is it going"),
    );
    forge.put_in(
        &later,
        &thread,
        &id("move"),
        Status::Done,
        shell("git mv smoke.rs pvs/"),
    );
    forge.put_in(
        &later,
        &thread,
        &id("moved"),
        Status::Done,
        reply("The smoke code moved."),
    );

    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the thread", |cx| loaded(&conversation, cx).len() == 6);
    let rows: Vec<String> = cx.read(|cx| {
        let c = conversation.read(cx);
        // This asserts canonical ordering, independently of native leaf subdivision.
        crate::fold::fold(
            c.view.as_ref().unwrap(),
            c.run(cx).as_ref(),
            c.agent.as_ref(),
        )
        .rows
        .iter()
        .map(|(row, _)| match row {
            Row::PlainBlock {
                id,
                section: crate::fold::PlainSection::User,
                ..
            } => format!("user {id}"),
            Row::ProseBlock { id, block: 0 } => format!("reply {id}"),
            Row::Work {
                run,
                segment,
                entries,
                ..
            } => format!(
                "work {} #{segment}: {}",
                run.as_ref().map(ToString::to_string).unwrap_or_default(),
                entries
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            other => format!("{other:?}"),
        })
        .collect()
    });
    assert_eq!(
        rows,
        [
            "user ask",
            "work earlier #0: thea",
            "reply baked",
            "user how",
            "work later #0: move",
            "reply moved",
        ]
    );
}

/// The header names what the run did, so it doesn't flip between
/// "Thinking" and "Working" each time a command ends and the model thinks.
#[test]
fn a_work_header_keeps_its_verb_while_the_run_goes_back_to_thinking() {
    use crate::phase::{Phase, section_title};
    use artisan_api::RunStatus;
    let minute = 61_000;
    for phase in [
        Phase::Thinking,
        Phase::Working,
        Phase::Replying,
        Phase::Background,
    ] {
        assert_eq!(section_title(phase, true, minute), "Working for 1m 1s");
        assert_eq!(section_title(phase, false, minute), "Thinking for 1m 1s");
    }
    assert_eq!(
        section_title(Phase::Settled(RunStatus::Done), true, minute),
        "Worked for 1m 1s"
    );
    assert_eq!(
        section_title(Phase::Settled(RunStatus::Done), false, minute),
        "Thought for 1m 1s"
    );
}

#[test]
fn chain_headers_count_what_the_work_did() {
    use crate::activity::{ActivityCategory, chain_clause};
    assert_eq!(
        chain_clause(
            [
                ActivityCategory::Command,
                ActivityCategory::Command,
                ActivityCategory::FileRead
            ]
            .into_iter()
        ),
        "Ran 2 commands, read a file"
    );
}

/// A steer stays in the turn's work where it was sent: one work section
/// around it, folding with it, never a turn of its own.
#[test]
fn a_steer_sits_in_the_work_where_it_was_sent() {
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
    let message = |text: &str, steer: bool| ItemKind::UserMessage {
        text: text.into(),
        images: Vec::new(),
        steer,
    };
    let (before, steer, after) = (
        item(2, shell("rg subagent")),
        item(3, message("also fix the cancelled agents", true)),
        item(4, shell("cargo test")),
    );
    let items = vec![
        item(1, message("name the subagents", false)),
        before.clone(),
        steer.clone(),
        after.clone(),
        item(5, reply("Both done.")),
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
    assert_eq!(
        rows.len(),
        3,
        "the message, one work section, the reply: {rows:?}"
    );
    assert!(
        matches!(&rows[1], Row::Work { entries, .. }
            if *entries == vec![before.id.clone(), steer.id.clone(), after.id.clone()]),
        "the steer between the work before and after it: {rows:?}"
    );
}

#[test]
fn canonical_tool_commands_preserve_quoted_whitespace() {
    let command = "bash -lc \"printf 'a   b'\"";
    let item = Item {
        id: ItemId::new(),
        thread_id: ThreadId::new(),
        position: 1,
        seq: 1,
        run_id: None,
        parent_id: None,
        status: Status::Done,
        created_at: 0,
        updated_at: 0,
        kind: shell(command),
    };
    assert_eq!(
        crate::activity::detail_text(&item, crate::activity::ActivityCategory::Command),
        command
    );
}

/// A visible native disclosure clips original glyph rows and reverses in place.
#[gpui::test]
fn opening_work_slides_its_content_open(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let run = RunId::from("short-commentary");
    let id = ItemId::new();
    forge.put_in(
        &run,
        &thread,
        &id,
        Status::Done,
        ItemKind::AssistantMessage {
            text: "First paragraph.\n\nSecond paragraph.\n\nThird paragraph.".into(),
            phase: Phase::Commentary,
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the short commentary", |cx| {
        loaded(&conversation, cx).len() == 1
    });
    conversation.update(cx, |c, cx| c.toggle_work(&run, true, cx));
    settle(cx, &conversation);
    let intrinsic = cx.read(|cx| conversation.read(cx).shared.row_heights.borrow().clone());
    conversation.update(cx, |c, cx| c.toggle_work(&run, false, cx));
    draw(cx);
    let partial_deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let partially_clipped = cx.read(|cx| {
            let c = conversation.read(cx);
            intrinsic.iter().any(|(key, full)| {
                let Some(index) = c.row_context.key_rows.get(key).copied() else {
                    return false;
                };
                if !matches!(c.rows[index].0, Row::ProseBlock { .. }) {
                    return false;
                }
                let Some(bounds) = c.list.bounds_for_item(index) else {
                    return false;
                };
                let end = if index + 1 == c.rows.len() {
                    gpui::px(c.row_context.end_space)
                } else {
                    gpui::px(0.0)
                };
                let visible = bounds.size.height - end;
                visible > gpui::px(0.5)
                    && visible < *full - gpui::px(0.5)
                    && c.shared.row_heights.borrow().get(key) == Some(full)
            })
        });
        if partially_clipped {
            break;
        }
        assert!(
            Instant::now() < partial_deadline,
            "the native disclosure never painted a partially clipped original row"
        );
        std::thread::sleep(Duration::from_millis(10));
        draw(cx);
    }
    assert!(
        cx.debug_bounds("work-header").is_some(),
        "the disclosure header must remain mounted during the flight"
    );
    conversation.update(cx, |c, cx| c.toggle_work(&run, true, cx));
    settle(cx, &conversation);
    cx.read(|cx| {
        let c = conversation.read(cx);
        assert!(
            c.rows
                .iter()
                .any(|(row, _)| matches!(row, Row::ProseBlock { id: item, .. } if *item == id))
        );
    });
}

/// A work disclosure never mounts a whole commentary, list or table subtree.
#[gpui::test]
fn opening_long_work_prepares_only_what_shows(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let run = RunId::from("long-commentary");
    let id = ItemId::new();
    let text = format!(
        "{}\n\n{}\n\n| column | value |\n| --- | --- |\n{}",
        (0..250)
            .map(|n| format!("Paragraph {n} explains a separate part of the change."))
            .collect::<Vec<_>>()
            .join("\n\n"),
        (0..250)
            .map(|n| format!("- Checklist entry {n}"))
            .collect::<Vec<_>>()
            .join("\n"),
        (0..250)
            .map(|n| format!("| row {n} | value {n} | "))
            .collect::<Vec<_>>()
            .join("\n")
    );
    forge.put_in(
        &run,
        &thread,
        &id,
        Status::Done,
        ItemKind::AssistantMessage {
            text: text.into(),
            phase: Phase::Commentary,
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "commentary item", |cx| {
        loaded(&conversation, cx).len() == 1
    });
    conversation.update(cx, |c, cx| c.toggle_work(&run, true, cx));
    wait(cx, "flat commentary leaves", |cx| {
        draw(cx);
        cx.read(|cx| {
            let c = conversation.read(cx);
            let sources = c.shared.stored_sources.borrow();
            sources.get(&(id.clone(), crate::fold::Slot::Prose)).is_some_and(|source| {
                source.block_count() == 751
                    && source.loaded_range().len() <= 64
                    && !source.loaded_range().is_empty()
                    && c.rows.iter().filter(|(row, _)| matches!(row, Row::ProseBlock { id: item, .. } if *item == id)).count() == source.loaded_range().len()
            })
        })
    });
    settle(cx, &conversation);
    cx.read(|cx| {
        let c = conversation.read(cx);
        assert_eq!(
            c.shared.stored_sources.borrow()[&(id.clone(), crate::fold::Slot::Prose)].block_count(),
            751
        );
        assert!(
            c.rows.len() <= 65,
            "one header and at most64 registered semantic leaves"
        );
        let mounted = c.row_views.iter().flatten().count();
        assert!(
            mounted < 128,
            "only viewport and bounded cache mount views: {mounted}/{}",
            c.rows.len()
        );
        // A header contains metadata, not every child's model or rendered element.
        assert!(
            c.rows
                .iter()
                .filter(|(row, _)| matches!(row, Row::Work { .. }))
                .count()
                == 1
        );
    });
    // Capture already shaped native body heights. The flight must clip those
    // original rows; it must not reshape a growing text prefix or compress all
    // children at once.
    let source_window = cx.read(|cx| {
        conversation.read(cx).shared.stored_sources.borrow()
            [&(id.clone(), crate::fold::Slot::Prose)]
            .loaded_range()
    });
    conversation.update(cx, |c, cx| c.toggle_work(&run, false, cx));
    draw(cx);
    cx.read(|cx| {
        let c = conversation.read(cx);
        assert!(
            !c.disclosures.is_empty(),
            "closing retains its native owners during the flight"
        );
        assert!(
            c.rows.len() > 1 && c.rows.len() <= 129,
            "the closing window stays bounded"
        );
    });
    // Reversing a live native flight keeps the source window and header rather
    // than dropping/reloading the descendant models and starting from zero.
    conversation.update(cx, |c, cx| c.toggle_work(&run, true, cx));
    draw(cx);
    cx.read(|cx| {
        let c = conversation.read(cx);
        assert!(!c.disclosures.is_empty(), "the interrupted flight reverses");
        assert_eq!(
            c.shared.stored_sources.borrow()[&(id.clone(), crate::fold::Slot::Prose)]
                .loaded_range(),
            source_window
        );
        assert!(c.row_views.iter().flatten().count() <= 128);
    });
    wait(cx, "the reopened native flight", |cx| {
        draw(cx);
        cx.read(|cx| conversation.read(cx).disclosures.is_empty())
    });
    settle(cx, &conversation);
    conversation.update(cx, |c, cx| c.toggle_work(&run, false, cx));
    wait(cx, "closing to release descendant native owners", |cx| {
        draw(cx);
        cx.read(|cx| {
            let c = conversation.read(cx);
            c.disclosures.is_empty() && c.rows.len() == 1
        })
    });
    cx.read(|cx| {
        let c = conversation.read(cx);
        assert!(matches!(c.rows[0].0, Row::Work { .. }));
        assert!(c.row_views.iter().flatten().count() <= 1);
        assert!(
            c.shared
                .row_heights
                .borrow()
                .keys()
                .all(|key| *key == c.rows[0].0.key()),
            "descendant geometry ownership is released with its native row"
        );
    });
}

#[gpui::test]
fn adjacent_work_and_compaction_share_one_separator_and_keep_disclosure(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        ItemKind::UserMessage {
            text: "build it".into(),
            images: Vec::new(),
            steer: false,
        },
    );
    forge.put(&thread, &ItemId::new(), Status::Done, shell("cargo build"));
    let checkpoint = ItemId::new();
    let compact = || ItemKind::Compaction {
        tokens_before: Some(120_000),
        tokens_after: Some(40_000),
    };
    forge.put(&thread, &checkpoint, Status::Done, compact());
    forge.put(&thread, &ItemId::new(), Status::Done, reply("It builds."));
    forge.finish_run(&thread);
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the compacted turn", |cx| {
        loaded(&conversation, cx).len() == 4
    });
    draw(cx);
    let merged_revision = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            conversation
                .read(cx)
                .rows
                .iter()
                .find_map(|(row, version)| match row.semantic() {
                    Row::Work {
                        compaction: Some(id),
                        ..
                    } if id == &checkpoint => Some(*version),
                    _ => None,
                })
                .expect("checkpoint is metadata on the work separator")
        })
    };
    let initial_revision = merged_revision(cx);
    cx.read(|cx| {
        assert!(
            !conversation
                .read(cx)
                .rows
                .iter()
                .any(|(row, _)| matches!(row.semantic(), Row::Compaction(id) if id == &checkpoint)),
            "the merged checkpoint has no second native separator"
        )
    });
    let header = cx
        .debug_bounds("work-header")
        .expect("worked duration and disclosure remain");
    let indicator = cx
        .debug_bounds("work-compaction")
        .expect("compacted indicator remains");
    assert!(
        indicator.top() >= header.top() && indicator.bottom() <= header.bottom(),
        "the compacted indicator shares the work header"
    );
    assert!(
        cx.debug_bounds("work-chain").is_none(),
        "completed work starts collapsed"
    );
    click(cx, "work-header");
    settle(cx, &conversation);
    assert!(
        cx.debug_bounds("work-chain").is_some(),
        "the merged separator still expands its native work descendants"
    );
    assert!(cx.debug_bounds("work-compaction").is_some());
    forge.put(&thread, &checkpoint, Status::Cancelled, compact());
    wait(cx, "the merged checkpoint status revision", |cx| {
        merged_revision(cx) != initial_revision
    });
    draw(cx);
    assert!(
        cx.debug_bounds("work-compaction").is_some(),
        "status updates retain the combined separator"
    );
    let separate = ItemId::new();
    forge.put(&thread, &separate, Status::Done, compact());
    wait(cx, "a checkpoint separated by the reply", |cx| {
        loaded(&conversation, cx).len() == 5
    });
    cx.read(|cx| {
        assert!(
            conversation
                .read(cx)
                .rows
                .iter()
                .any(|(row, _)| matches!(row.semantic(), Row::Compaction(id) if id == &separate)),
            "a reply between separators prevents merging"
        )
    });
}
