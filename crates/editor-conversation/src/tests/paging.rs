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
                    from_agent: None,
                    sender: None,
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
    let page = crate::thread::ROWS_PAGE as usize;
    assert_eq!(
        first,
        stored[stored.len() - page..],
        "opening shows the newest page of rows"
    );

    // The reader reaches the top edge, again and again.
    for expected in (2 * page..stored.len()).step_by(page).chain([stored.len()]) {
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
            from_agent: None,
            sender: None,
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
                from_agent: None,
                sender: None,
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
    landed(cx, &conversation);
    settle(cx, &conversation);
    let target = asks[1].clone();
    // Where the target message sits relative to the top of the transcript.
    let top = |cx: &mut VisualTestContext| {
        let viewport = cx.read(|cx| conversation.read(cx).list.viewport_bounds());
        shown(cx, format!("user:{}:0", target.id))
            .map(|bounds| f32::from(bounds.top() - viewport.top()))
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
        offset == 0.0 && top(cx).is_some_and(|top| top.abs() < 0.5)
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
            from_agent: None,
            sender: None,
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
    landed(cx, &conversation);
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
                        watch: false,
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
#[expect(
    clippy::significant_drop_tightening,
    reason = "`c` is a plain borrow of the entity; there is nothing to release early"
)]
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
                from_agent: None,
                sender: None,
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
    landed(cx, &conversation);
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
#[expect(
    clippy::significant_drop_tightening,
    reason = "`c` is a plain borrow of the entity; there is nothing to release early"
)]
fn scrolling_up_through_long_turns_never_jumps(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    // The questions and replies: rows that never merge, so their movement
    // is what the reader sees.
    let mut anchors = Vec::new();
    for turn in 0..12 {
        let run = RunId::from(format!("run-{turn}").as_str());
        let ask = forge.put_in(
            &run,
            &thread,
            &ItemId::new(),
            Status::Done,
            ItemKind::UserMessage {
                text: format!("question {turn}").into(),
                images: Vec::new(),
                steer: false,
                from_agent: None,
                sender: None,
            },
        );
        anchors.push(format!("user:{}:0", ask.id));
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
        let answer = forge.put_in(&run, &thread, &ItemId::new(), Status::Done, reply(&long));
        anchors.push(format!("prose:{}:0", answer.id));
        // Failed, so its work stays open and loads as the reader nears it.
        forge.end_run_in(&run, &thread, artisan_api::RunStatus::Failed);
    }
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    landed(cx, &conversation);
    assert!(
        loaded(&conversation, cx).len() <= 200,
        "opening must not backfill a whole turn"
    );
    settle(cx, &conversation);
    // Where each visible question and reply sits on screen.
    let visible = |cx: &mut VisualTestContext| {
        anchors
            .iter()
            .filter_map(|anchor| {
                Some((anchor.clone(), f32::from(shown(cx, anchor.clone())?.top())))
            })
            .collect::<HashMap<_, _>>()
    };
    let center = cx.update(|window, _| window.viewport_size()) / 2.0;
    // At the start: every page and every open section's work read, and
    // the list at its top.
    let exhausted = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            let c = conversation.read(cx);
            c.view.as_ref().is_some_and(|view| view.window().exhausted)
                && c.work_reading.is_none()
                && !c
                    .rows
                    .iter()
                    .any(|(row, _)| matches!(row, crate::fold::Row::EarlierWork { .. }))
                && f32::from(c.list.scroll_px_offset_for_scrollbar().y) >= 0.0
        })
    };
    let describe = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            let c = conversation.read(cx);
            let top = c.list.logical_scroll_top();
            let rows: Vec<String> = (top.item_ix.saturating_sub(3)..top.item_ix + 6)
                .filter_map(|ix| {
                    let (row, _) = c.rows.get(ix)?;
                    let b = c.list.bounds_for_item(ix);
                    Some(format!(
                        "{ix}:{}@{:?}/{:?}gap{:?}",
                        row.selector(),
                        b.map(|b| f32::from(b.top())),
                        b.map(|b| f32::from(b.size.height)),
                        c.row_context.gaps.get(ix)
                    ))
                })
                .collect();
            format!(
                "top {top:?} items {} [{}]",
                c.items().count(),
                rows.join(" ")
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
            .view
            .as_ref()
            .is_some_and(|view| view.window().gap.is_some())
    });
    assert!(gap, "the bounded history window exposes its newer boundary");
    assert!(
        loaded(&conversation, cx).len() <= 800,
        "history loading retains a bounded item window: {} {:?}",
        loaded(&conversation, cx).len(),
        cx.read(|cx| conversation
            .read(cx)
            .view
            .as_ref()
            .unwrap()
            .window()
            .state()
            .clone())
    );
    conversation.update(cx, super::super::Conversation::jump_to_latest);
    wait(cx, "return to the latest bounded page", |cx| {
        draw(cx);
        cx.read(|cx| {
            let c = conversation.read(cx);
            !c.view.as_ref().is_some_and(|view| view.window().in_flight)
                && c.view
                    .as_ref()
                    .is_some_and(|view| view.window().gap.is_none())
                && c.items().count() <= 200
        })
    });
}

/// The newest turn is longer than what loaded first: its work shows at
/// once while its start loads, not just the status line.
#[gpui::test]
fn the_newest_turn_shows_before_its_start_loads(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        ItemKind::UserMessage {
            text: "test everything".into(),
            images: Vec::new(),
            steer: false,
            from_agent: None,
            sender: None,
        },
    );
    for _ in 0..250 {
        forge.put(&thread, &ItemId::new(), Status::Done, shell("cargo test"));
    }
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the newest page", |cx| {
        !loaded(&conversation, cx).is_empty()
    });
    draw(cx);
    assert!(
        loaded(&conversation, cx).len() < 251,
        "the turn's start is still loading"
    );
    assert!(
        shown(cx, "work:run:0").is_some(),
        "the loaded work shows under its header"
    );
}

/// Text above the reader growing as it finishes preparing keeps the row
/// the reader is on where it is.
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
    // The reader scrolls up until the earlier commentary sits just above
    // the view.
    let max = |cx: &mut VisualTestContext| {
        cx.read(|cx| f32::from(conversation.read(cx).list.max_offset_for_scrollbar().y))
    };
    let center = cx.update(|window, _| window.viewport_size()) / 2.0;
    let up = max(cx) - 400.0;
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: gpui::point(center.width, center.height),
        delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(up))),
        ..Default::default()
    });
    settle(cx, &conversation);
    assert!(
        shown(cx, format!("prose:{earlier}:0")).is_none(),
        "the earlier commentary is above the view"
    );
    // The paragraph at the top of the view and where it sits.
    let anchor = |cx: &mut VisualTestContext| {
        (0..100)
            .filter_map(|block| {
                let selector = format!("prose:{later}:{block}");
                Some((selector.clone(), f32::from(shown(cx, selector)?.top())))
            })
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .unwrap()
    };
    let before = anchor(cx);
    let height = max(cx);
    let expanded = (0..300)
        .map(|n| format!("Earlier background paragraph {n}."))
        .collect::<Vec<_>>()
        .join("\n\n");
    forge.put_in(
        &run,
        &thread,
        &earlier,
        Status::Done,
        ItemKind::AssistantMessage {
            text: expanded.into(),
            phase: Phase::Commentary,
        },
    );
    // Prepared just above the view, it grows there.
    wait(cx, "the earlier commentary grows above the reader", |cx| {
        draw(cx);
        max(cx) > height + 1000.0
    });
    settle(cx, &conversation);
    let after = anchor(cx);
    assert_eq!(after.0, before.0, "the same paragraph stays on top");
    assert!(
        (after.1 - before.1).abs() < 0.5,
        "the paragraph moved: {before:?} -> {after:?}"
    );
}

/// The navigator is a map of the whole thread from Forge, not of what is
/// loaded: every message is listed, and picking one far above the loaded
/// stretch loads it and shows it.
#[gpui::test]
fn the_navigator_lists_the_whole_thread_and_loads_what_it_picks(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let mut asks = Vec::new();
    for turn in 0..80 {
        let run = RunId::from(format!("turn-{turn}"));
        asks.push(forge.put_in(
            &run,
            &thread,
            &ItemId::new(),
            Status::Done,
            ItemKind::UserMessage {
                text: format!("Question {turn}\nwith a second line").into(),
                images: Vec::new(),
                steer: false,
                from_agent: None,
                sender: None,
            },
        ));
        forge.put_in(
            &run,
            &thread,
            &ItemId::new(),
            Status::Done,
            reply("Answer."),
        );
    }
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    let labels = |cx: &mut VisualTestContext| {
        conversation.update(cx, |conversation, cx| {
            conversation
                .markers(cx)
                .iter()
                .map(|marker| marker.label.to_string())
                .collect::<Vec<_>>()
        })
    };
    wait(cx, "the whole outline", |cx| labels(cx).len() == 80);
    assert!(
        loaded(&conversation, cx).len() < 160,
        "only the tail is loaded"
    );
    assert_eq!(labels(cx)[0], "Question 0");
    assert!(shown(cx, format!("user:{}:0", asks[0].id)).is_none());
    let first = asks[0].position;
    conversation.update(cx, |conversation, cx| conversation.navigate(first, cx));
    wait(cx, "the first message, loaded and in view", |cx| {
        draw(cx);
        shown(cx, format!("user:{}:0", asks[0].id)).is_some()
    });
}

/// One turn's work holds more items than the whole window. Folded, it is
/// one row; scrolling up to it must not trim away the reply under it and
/// leave only a "Load newer messages" gap that reloads and drops it again.
#[gpui::test]
#[expect(
    clippy::significant_drop_tightening,
    reason = "`c` is a plain borrow of the entity; there is nothing to release early"
)]
fn a_run_larger_than_the_window_keeps_its_reply_in_view(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let run = RunId::from("long-run");
    let ask = ItemKind::UserMessage {
        text: "Finish the animation work".into(),
        images: Vec::new(),
        steer: false,
        from_agent: None,
        sender: None,
    };
    forge.put_in(&run, &thread, &ItemId::new(), Status::Done, ask);
    for step in 0..900 {
        forge.put_in(
            &run,
            &thread,
            &ItemId::new(),
            Status::Done,
            shell(&format!("cargo test -p part{step}")),
        );
    }
    let answer = forge.put_in(
        &run,
        &thread,
        &ItemId::new(),
        Status::Done,
        reply("The animation work is done."),
    );
    forge.finish_run_in(&run, &thread);
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the newest page", |cx| {
        draw(cx);
        !loaded(&conversation, cx).is_empty()
    });
    settle(cx, &conversation);
    let center = cx.update(|window, _| window.viewport_size()) / 2.0;
    let window = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            let view = conversation.read(cx).view.as_ref().unwrap();
            (view.window().exhausted, view.window().gap.is_some())
        })
    };
    for _ in 0..200 {
        if window(cx).0 {
            break;
        }
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: gpui::point(center.width, center.height),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(40.0))),
            ..Default::default()
        });
        draw(cx);
        wait(cx, "the page asked for", |cx| {
            draw(cx);
            cx.read(|cx| {
                !conversation
                    .read(cx)
                    .view
                    .as_ref()
                    .is_some_and(|view| view.window().in_flight)
            })
        });
    }
    let state = cx.read(|cx| {
        let c = conversation.read(cx);
        let view = c.view.as_ref().unwrap();
        format!(
            "{:?} oldest {:?} in_flight {} items {} rows {}",
            view.window().state(),
            view.window().oldest,
            view.window().in_flight,
            view.items.len(),
            c.rows.len()
        )
    });
    assert!(
        window(cx).0,
        "scrolling up reaches the run's start: {state}"
    );
    assert!(!window(cx).1, "nothing on screen was trimmed into a gap");
    assert!(
        loaded(&conversation, cx)
            .iter()
            .any(|item| item.id == answer.id),
        "the reply under the folded work stays loaded"
    );
}

/// A turn of thousands of calls opens as its message, its folded work and
/// its reply, read in one small page: closed work is drawn from Forge's
/// summary of it. Opening the section reads its newest work under the
/// header, with the earlier work as a row that loads as it nears.
#[gpui::test]
fn a_turn_of_thousands_of_calls_opens_as_its_rows(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let ask = forge.put(
        &thread,
        &ItemId::from("ask"),
        Status::Done,
        ItemKind::UserMessage {
            text: "Finish the animation work".into(),
            images: Vec::new(),
            steer: false,
            from_agent: None,
            sender: None,
        },
    );
    for step in 0..3000 {
        let kind = if step % 100 == 0 {
            ItemKind::AssistantMessage {
                text: format!("Checking part {step}.").into(),
                phase: Phase::Commentary,
            }
        } else {
            shell(&format!("cargo test -p part{step}"))
        };
        forge.put(&thread, &ItemId::new(), Status::Done, kind);
    }
    let answer = forge.put(
        &thread,
        &ItemId::from("answer"),
        Status::Done,
        reply("The animation work is done."),
    );
    forge.finish_run(&thread);
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    landed(cx, &conversation);
    // From its first frame nothing in it moves: its text was prepared
    // before it showed.
    let first = cx.read(|cx| {
        let c = conversation.read(cx);
        (0..c.rows.len())
            .map(|index| c.list.bounds_for_item(index))
            .collect::<Vec<_>>()
    });
    for _ in 0..20 {
        draw(cx);
        cx.run_until_parked();
        let now = cx.read(|cx| {
            let c = conversation.read(cx);
            (0..c.rows.len())
                .map(|index| c.list.bounds_for_item(index))
                .collect::<Vec<_>>()
        });
        assert_eq!(now, first, "the thread moved after it showed");
    }
    let rows = [
        format!("user:{}:0", ask.id),
        "work:run:0".to_owned(),
        format!("prose:{}:0", answer.id),
    ];
    draw(cx);
    assert_eq!(painted_order(cx, &rows), rows, "the turn shows whole");
    assert_eq!(
        loaded(&conversation, cx).len(),
        2,
        "only the rows are read, not the work under the header"
    );

    click(cx, "work-header");
    wait(cx, "the newest work", |cx| {
        draw(cx);
        cx.debug_bounds("work-chain").is_some()
    });
    settle(cx, &conversation);
    let loaded = loaded(&conversation, cx).len();
    assert!(
        (100..3002).contains(&loaded),
        "opening reads the work a page at a time: {loaded}"
    );
    let earlier = cx.read(|cx| {
        conversation
            .read(cx)
            .rows
            .iter()
            .any(|(row, _)| matches!(row, crate::fold::Row::EarlierWork { .. }))
    });
    assert!(earlier, "the work not read yet waits as a row above it");
}
