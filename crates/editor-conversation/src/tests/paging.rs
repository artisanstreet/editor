//! Opening a long thread, paging through it and scrolling it: the reader's place never jumps.

use super::*;

#[gpui::test]
fn scrolling_up_a_long_thread_loads_all_of_it_once(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    // 125 turns: a question and its reply each.
    let stored: Vec<Item> = (0..250)
        .map(|index| {
            let kind = if index % 2 == 0 {
                ItemKind::UserMessage {
                    text: format!("question {index}").into(),
                    images: Vec::new(),
                    steer: false,
                }
            } else {
                reply(&format!("reply {index}"))
            };
            forge.put(&thread, &ItemId::new(), Status::Done, kind)
        })
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);

    open(&core, &host, &thread, cx);
    wait(cx, "the newest page", |cx| {
        !loaded(&conversation, cx).is_empty()
    });
    draw(cx);
    let first = loaded(&conversation, cx);
    assert_eq!(first, stored[150..], "opening shows the newest page");

    // The reader reaches the top edge, again and again.
    for expected in [200, 250] {
        conversation.update(cx, |conversation, cx| {
            conversation.viewport(vec![Input::TopReached], cx);
        });
        wait(cx, "an older page", |cx| {
            loaded(&conversation, cx).len() == expected
        });
    }
    conversation.update(cx, |conversation, cx| {
        conversation.viewport(vec![Input::TopReached], cx);
    });
    cx.run_until_parked();
    assert_eq!(
        loaded(&conversation, cx),
        stored,
        "the whole thread, in order, with no gaps or duplicates"
    );
    let rows = cx.read(|cx| conversation.read(cx).rows.len());
    assert_eq!(rows, 250, "one reply row per item");
}

/// Opening a long thread hides it under the wordmark until it stops
/// moving, and shows it in place: nothing shifts afterwards. It reads only
/// the newest turns, not the whole thread: a turn's work is one row, so
/// reading ahead by rows used to load page after page.
#[gpui::test]
fn a_long_thread_opens_in_place_without_reading_all_of_it(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let mut total = 0;
    for turn in 0..8 {
        let run = RunId::from(format!("turn-{turn}"));
        let ask = ItemKind::UserMessage {
            text: format!("Turn {turn}: build the next part").into(),
            images: Vec::new(),
            steer: false,
        };
        forge.put_in(&run, &thread, &ItemId::new(), Status::Done, ask);
        for step in 0..150 {
            let kind = if step % 10 == 0 {
                ItemKind::AssistantMessage {
                    text: format!("Step {step} of turn {turn}: checking the build.").into(),
                    phase: Phase::Commentary,
                }
            } else {
                shell(&format!("cargo test -p part{step}"))
            };
            forge.put_in(&run, &thread, &ItemId::new(), Status::Done, kind);
        }
        forge.put_in(
            &run,
            &thread,
            &ItemId::new(),
            Status::Done,
            reply(
                &(0..30)
                    .map(|line| {
                        format!("Paragraph {line} of turn {turn}'s answer, long enough to wrap.")
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            ),
        );
        total += 152;
    }
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    let state = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            let c = conversation.read(cx);
            (
                c.landing(),
                c.view.as_ref().is_some_and(|view| view.window().in_flight),
                {
                    let top = c.list.logical_scroll_top();
                    (top.item_ix, f32::from(top.offset_in_item))
                },
                f32::from(c.list.max_offset_for_scrollbar().y),
            )
        })
    };
    // Hidden while pages are still coming in.
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        draw(cx);
        cx.run_until_parked();
        let (landing, in_flight, ..) = state(cx);
        if in_flight {
            assert!(
                landing,
                "the transcript shows while a page is still loading"
            );
        }
        if !landing {
            break;
        }
        assert!(Instant::now() < deadline, "it never landed");
        std::thread::sleep(Duration::from_millis(5));
    }
    settle(cx, &conversation);
    let (_, _, top, max) = state(cx);
    for _ in 0..30 {
        draw(cx);
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(5));
        let (_, _, now_top, now_max) = state(cx);
        assert_eq!(
            (now_top, now_max),
            (top, max),
            "the transcript moved after it showed"
        );
    }
    let loaded = loaded(&conversation, cx).len();
    assert!(
        loaded < total / 2,
        "opening read {loaded} of {total} items: more than a few screens"
    );
}

/// Jumping to a message from the menu doesn't teleport: the view lands a
/// little short of it and glides the rest quickly.
#[gpui::test]
fn a_jump_to_a_message_glides_its_last_stretch(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let mut asks = Vec::new();
    for turn in 0..12 {
        let run = RunId::from(format!("turn-{turn}"));
        let ask = forge.put_in(
            &run,
            &thread,
            &ItemId::new(),
            Status::Done,
            ItemKind::UserMessage {
                text: format!("Question {turn}").into(),
                images: Vec::new(),
                steer: false,
            },
        );
        asks.push(ask);
        let answer = (0..12)
            .map(|line| format!("Paragraph {line} of answer {turn}, long enough to wrap."))
            .collect::<Vec<_>>()
            .join("\n\n");
        forge.put_in(&run, &thread, &ItemId::new(), Status::Done, reply(&answer));
    }
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the thread", |cx| loaded(&conversation, cx).len() == 24);
    settle(cx, &conversation);
    let target = asks[1].clone();
    let top = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            let c = conversation.read(cx);
            let top = c.list.logical_scroll_top();
            let row = c
                .rows
                .get(top.item_ix)
                .and_then(|(row, _)| row.anchor().cloned());
            (row, f32::from(top.offset_in_item))
        })
    };
    conversation.update(cx, |conversation, cx| conversation.jump_to(&target, cx));
    draw(cx);
    let shift = conversation.update(cx, |conversation, _| conversation.arrival_offset());
    assert!(
        shift < -1.0,
        "jumping up, the transcript starts a little above its place and moves down into it, not cut ({shift})"
    );
    wait(cx, "the jump to arrive on the message", |cx| {
        draw(cx);
        let offset = conversation.update(cx, |conversation, _| conversation.arrival_offset());
        offset == 0.0 && top(cx) == (Some(target.id.clone()), 0.0)
    });
}

/// Background work keeps updating items while the reader sits still: the
/// transcript must not move, scrolled up or at the end.
#[gpui::test]
fn updates_while_reading_never_move_the_transcript(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        ItemKind::UserMessage {
            text: "merge the branches".into(),
            images: Vec::new(),
            steer: false,
        },
    );
    let tool = ItemId::new();
    forge.put(&thread, &tool, Status::Running, shell("cargo test"));
    let long = (0..60)
        .map(|line| format!("Paragraph {line} of the summary, long enough to wrap a little."))
        .collect::<Vec<_>>()
        .join("\n\n");
    forge.put(&thread, &ItemId::new(), Status::Done, reply(&long));
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the thread", |cx| loaded(&conversation, cx).len() == 3);
    settle(cx, &conversation);
    let offset = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            f32::from(
                conversation
                    .read(cx)
                    .list
                    .scroll_px_offset_for_scrollbar()
                    .y,
            )
        })
    };
    for scrolled_up in [false, true] {
        if scrolled_up {
            // The reader scrolls up a little with a trackpad.
            let center = cx.update(|window, _| window.viewport_size()) / 2.0;
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: gpui::point(center.width, center.height),
                delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(300.0))),
                ..Default::default()
            });
            settle(cx, &conversation);
        }
        let before = offset(cx);
        let mut seen = Vec::new();
        for step in 0..8 {
            forge.put(
                &thread,
                &tool,
                Status::Running,
                ItemKind::Tool {
                    tool: artisan_api::ToolKind::Shell {
                        command: "cargo test".into(),
                        cwd: None,
                        background: false,
                    },
                    title: Some(format!("running test {step}")),
                    output: None,
                    exit_code: None,
                    approval: None,
                },
            );
            wait(cx, "the update", |cx| {
                loaded(&conversation, cx).iter().any(|item| {
                    item.id == tool
                        && format!("{:?}", item.kind).contains(&format!("running test {step}"))
                })
            });
            for _ in 0..4 {
                draw(cx);
                seen.push(offset(cx));
            }
        }
        assert!(
            seen.iter().all(|at| (at - before).abs() < 0.5),
            "scrolled up: {scrolled_up}; offset {before} moved: {seen:?}"
        );
    }
}

/// A mouse wheel glides the transcript one way, to the end and back, while
/// background work keeps updating it: no frame jumps back.
#[gpui::test]
fn a_wheel_glide_never_jumps_back(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let mut tool = ItemId::new();
    for turn in 0..6 {
        forge.put(
            &thread,
            &ItemId::new(),
            Status::Done,
            ItemKind::UserMessage {
                text: format!("question {turn}").into(),
                images: Vec::new(),
                steer: false,
            },
        );
        tool = ItemId::new();
        forge.put(&thread, &tool, Status::Running, shell("cargo test"));
        let long = (0..12)
            .map(|line| format!("Paragraph {line} of answer {turn}, long enough to wrap a little."))
            .collect::<Vec<_>>()
            .join("\n\n");
        forge.put(&thread, &ItemId::new(), Status::Done, reply(&long));
    }
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the thread", |cx| loaded(&conversation, cx).len() == 18);
    settle(cx, &conversation);
    // Where the reader is: the first visible row's top on screen.
    let visual = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            let c = conversation.read(cx);
            let top = c.list.logical_scroll_top();
            let bounds = c.list.bounds_for_item(top.item_ix);
            (
                top.item_ix,
                f32::from(top.offset_in_item),
                bounds.map(|b| f32::from(b.top())),
            )
        })
    };
    let center = cx.update(|window, _| window.viewport_size()) / 2.0;
    let mut update = 0;
    for (direction, lines, ticks) in [
        ("up", 3.0, 8),
        ("down", -3.0, 20),
        ("up", 1.0, 2),
        ("down", -1.0, 4),
    ] {
        for _ in 0..ticks {
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: gpui::point(center.width, center.height),
                delta: gpui::ScrollDelta::Lines(gpui::point(0.0, lines)),
                ..Default::default()
            });
        }
        let mut seen = vec![visual(cx)];
        for frame in 0..40 {
            if frame % 5 == 0 {
                update += 1;
                let mut kind = shell("cargo test");
                if let ItemKind::Tool { title, .. } = &mut kind {
                    *title = Some(format!("update {update}"));
                }
                forge.put(&thread, &tool, Status::Running, kind);
            }
            std::thread::sleep(Duration::from_millis(8));
            draw(cx);
            cx.run_until_parked();
            seen.push(visual(cx));
        }
        // Rows move up the screen scrolling down, and down scrolling up.
        let back = seen.windows(2).any(|pair| {
            let ((ia, oa, _), (ib, ob, _)) = (pair[0], pair[1]);
            let (a, b) = ((ia, oa), (ib, ob));
            if lines > 0.0 {
                b.0 > a.0 || (b.0 == a.0 && b.1 > a.1 + 0.5)
            } else {
                b.0 < a.0 || (b.0 == a.0 && b.1 < a.1 - 0.5)
            }
        });
        assert!(!back, "scrolling {direction} jumped back: {seen:?}");
    }
}

/// Long agentic turns span many pages. Opening at the end and scrolling up
/// through them, older pages load in behind the reader: what is on screen
/// never jumps, and no gap opens below.
#[gpui::test]
#[expect(
    clippy::too_many_lines,
    reason = "one scenario: build the thread, open it, scroll it all the way up"
)]
fn scrolling_up_through_long_turns_never_jumps(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    for turn in 0..12 {
        let run = RunId::from(format!("run-{turn}").as_str());
        forge.put_in(
            &run,
            &thread,
            &ItemId::new(),
            Status::Done,
            ItemKind::UserMessage {
                text: format!("question {turn}").into(),
                images: Vec::new(),
                steer: false,
            },
        );
        for step in 0..130 {
            forge.put_in(
                &run,
                &thread,
                &ItemId::new(),
                Status::Done,
                shell(&format!("step {step}")),
            );
            if step % 20 == 0 {
                forge.put_in(
                    &run,
                    &thread,
                    &ItemId::new(),
                    Status::Done,
                    ItemKind::AssistantMessage {
                        text: format!("Checking part {step} of turn {turn}.").into(),
                        phase: Phase::Commentary,
                    },
                );
            }
        }
        let long = (0..10)
            .map(|line| format!("Paragraph {line} of answer {turn}, long enough to wrap a little."))
            .collect::<Vec<_>>()
            .join("\n\n");
        forge.put_in(&run, &thread, &ItemId::new(), Status::Done, reply(&long));
        forge.finish_run_in(&run, &thread);
    }
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the bounded latest page", |cx| {
        draw(cx);
        cx.read(|cx| {
            conversation.read(cx).view.as_ref().is_some_and(|view| {
                !view.items.is_empty() && !view.window().in_flight && !view.window().exhausted
            })
        })
    });
    assert!(
        loaded(&conversation, cx).len() <= 200,
        "opening must not backfill a whole turn"
    );
    settle(cx, &conversation);
    // Where each visible question and reply sits on screen: rows that
    // never merge, so their movement is what the reader sees.
    let visible = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            let c = conversation.read(cx);
            c.rows
                .iter()
                .enumerate()
                .filter(|(_, (row, _))| {
                    matches!(
                        row,
                        Row::PlainBlock {
                            section: crate::fold::PlainSection::User,
                            block: 0,
                            ..
                        } | Row::ProseBlock { block: 0, .. }
                    )
                })
                .filter_map(|(index, (row, _))| {
                    let at = c.list.bounds_for_item(index)?;
                    Some((row.key(), f32::from(at.top())))
                })
                .collect::<HashMap<_, _>>()
        })
    };
    let center = cx.update(|window, _| window.viewport_size()) / 2.0;
    let exhausted = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            conversation
                .read(cx)
                .view
                .as_ref()
                .is_some_and(|view| view.window().exhausted)
        })
    };
    let describe = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            let c = conversation.read(cx);
            let top = c.list.logical_scroll_top();
            format!(
                "top {top:?} rows {} items {}",
                c.rows.len(),
                c.items().count()
            )
        })
    };
    // A trackpad moves the list by exactly what it is given, so any other
    // movement is the transcript jumping under the reader.
    let step = 40.0;
    let mut frames = 0;
    while !exhausted(cx) || frames < 60 {
        assert!(frames < 3000, "scrolling up never reached the start");
        // Hidden while it lands: nothing on screen to jump.
        let shown = |cx: &mut VisualTestContext| cx.read(|cx| !conversation.read(cx).landing());
        let before = if shown(cx) {
            visible(cx)
        } else {
            HashMap::new()
        };
        let was = describe(cx);
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: gpui::point(center.width, center.height),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(step))),
            ..Default::default()
        });
        draw(cx);
        cx.run_until_parked();
        draw(cx);
        frames += 1;
        if !shown(cx) {
            continue;
        }
        for (key, now) in visible(cx) {
            if let Some(at) = before.get(&key) {
                let moved = now - at;
                assert!(
                    (-0.5..=step + 0.5).contains(&moved),
                    "{key:?} jumped from {at} to {now}\nbefore {was}\nafter  {}",
                    describe(cx)
                );
            }
        }
    }
    let gap = cx.read(|cx| {
        conversation
            .read(cx)
            .rows
            .iter()
            .any(|(row, _)| matches!(row, Row::Gap))
    });
    assert!(gap, "the bounded history window exposes its newer boundary");
    assert!(
        loaded(&conversation, cx).len() <= 800,
        "history loading retains a bounded item window"
    );
    conversation.update(cx, |c, cx| c.jump_to_latest(cx));
    wait(cx, "return to the latest bounded page", |cx| {
        draw(cx);
        cx.read(|cx| {
            let c = conversation.read(cx);
            !c.view.as_ref().is_some_and(|view| view.window().in_flight)
                && c.rows.iter().all(|(row, _)| !matches!(row, Row::Gap))
                && c.items().count() <= 200
        })
    });
}

/// The newest turn is longer than what loaded first: its work shows at
/// once while its start loads, not just the status line.
#[test]
fn the_newest_turn_shows_before_its_start_loads() {
    use crate::fold::fold;
    use crate::thread::ThreadView;
    use crate::viewport::Fetch;
    use artisan_api::ItemsPage;

    let thread = ThreadId::from("thread");
    let items: Vec<Item> = (1000..1100)
        .map(|position| Item {
            id: ItemId::new(),
            thread_id: thread.clone(),
            position,
            seq: position,
            run_id: Some(RunId::from("run")),
            parent_id: None,
            status: Status::Done,
            created_at: 0,
            updated_at: 0,
            kind: shell("cargo test"),
        })
        .collect();
    let (mut view, _) = ThreadView::open(thread.clone());
    view.apply_page(
        Fetch::Tail,
        ItemsPage {
            items,
            runs: Vec::new(),
            more: true,
        },
    );
    assert_eq!(
        view.shown().count(),
        100,
        "partial turns are usable immediately"
    );
    let rows = fold(&view, None, None).rows;
    assert!(
        rows.iter()
            .any(|(row, _)| matches!(row, Row::Work { entries, .. } if entries.len() == 100)),
        "the loaded work shows: {rows:?}"
    );
}

/// Finishing preparation above the reader retains the native visible row and its pixel offset.
#[gpui::test]
fn background_work_preparation_preserves_the_reading_anchor(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let run = RunId::from("prepare-above-reader");
    let earlier = ItemId::new();
    forge.put_in(
        &run,
        &thread,
        &earlier,
        Status::Done,
        ItemKind::AssistantMessage {
            text: "Earlier commentary.".into(),
            phase: Phase::Commentary,
        },
    );
    let later = ItemId::new();
    let long = (0..100)
        .map(|n| format!("Visible paragraph {n}, enough content to scroll away from the tail."))
        .collect::<Vec<_>>()
        .join("\n\n");
    forge.put_in(
        &run,
        &thread,
        &later,
        Status::Done,
        ItemKind::AssistantMessage {
            text: long.into(),
            phase: Phase::Commentary,
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "both commentary items", |cx| {
        loaded(&conversation, cx).len() == 2
    });
    conversation.update(cx, |c, cx| c.toggle_work(&run, true, cx));
    settle(cx, &conversation);
    let center = cx.update(|window, _| window.viewport_size()) / 2.0;
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: gpui::point(center.width, center.height),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(300.0))),
        ..Default::default()
    });
    settle(cx, &conversation);
    let anchor = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            let c = conversation.read(cx);
            let top = c.list.logical_scroll_top();
            (c.rows[top.item_ix].0.key(), f32::from(top.offset_in_item))
        })
    };
    let before = anchor(cx);
    assert!(matches!(&before.0, crate::fold::RowKey::ProseBlock(id, _) if *id == later));
    let expanded: Arc<str> = Arc::from(
        (0..300)
            .map(|n| format!("Earlier background paragraph {n}."))
            .collect::<Vec<_>>()
            .join("\n\n"),
    );
    forge.put_in(
        &run,
        &thread,
        &earlier,
        Status::Done,
        ItemKind::AssistantMessage {
            text: expanded.to_string().into(),
            phase: Phase::Commentary,
        },
    );
    wait(cx, "expanded offscreen item revision", |cx| {
        loaded(&conversation, cx).iter().any(|item| item.id == earlier && matches!(&item.kind, ItemKind::AssistantMessage { text, .. } if text.inline() == Some(expanded.as_ref())))
    });
    conversation.update(cx, |c, cx| {
        let owner = cx.entity().downgrade();
        let _ =
            c.shared
                .markdown
                .prepare_source(earlier.to_string().into(), expanded, cx, move |cx| {
                    let _ = owner.update(cx, Conversation::rebuild);
                });
    });
    wait(cx, "offscreen preparation publication", |cx| {
        draw(cx);
        cx.read(|cx| {
            conversation
                .read(cx)
                .rows
                .iter()
                .filter(|(row, _)| matches!(row, Row::ProseBlock { id, .. } if *id == earlier))
                .count()
                == 300
        })
    });
    settle(cx, &conversation);
    let after = anchor(cx);
    assert_eq!(
        after.0, before.0,
        "same visible native row across preparation"
    );
    assert!(
        (after.1 - before.1).abs() < 0.5,
        "pixel anchor moved: {before:?} -> {after:?}"
    );
}
