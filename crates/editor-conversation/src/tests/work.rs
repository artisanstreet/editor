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
            && cx.debug_bounds("disclosing").is_none()
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
    let rows = [
        "user:ask:0",
        "work:earlier:0",
        "prose:baked:0",
        "user:how:0",
        "work:later:0",
        "prose:moved:0",
    ]
    .map(str::to_owned);
    wait(cx, "both turns", |cx| {
        draw(cx);
        painted_order(cx, &rows).len() == rows.len()
    });
    assert_eq!(painted_order(cx, &rows), rows);
    assert!(
        shown(cx, "prose:orphan:0").is_none() && shown(cx, "work:later:1").is_none(),
        "the resumed agent stays in the turn it sits in, the orphan line nowhere"
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
#[gpui::test]
fn a_steer_sits_in_the_work_where_it_was_sent(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let message = |text: &str, steer: bool| ItemKind::UserMessage {
        text: text.into(),
        images: Vec::new(),
        steer,
    };
    let ask = forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        message("name the subagents", false),
    );
    let before = forge.put(&thread, &ItemId::new(), Status::Done, shell("rg subagent"));
    let steer = forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        message("also fix the cancelled agents", true),
    );
    let after = forge.put(&thread, &ItemId::new(), Status::Done, shell("cargo test"));
    let answer = forge.put(&thread, &ItemId::new(), Status::Done, reply("Both done."));
    forge.finish_run(&thread);
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    let folded = [
        format!("user:{}:0", ask.id),
        "work:run:0".to_owned(),
        format!("prose:{}:0", answer.id),
    ];
    wait(cx, "the turn", |cx| {
        draw(cx);
        painted_order(cx, &folded).len() == folded.len()
    });
    assert_eq!(painted_order(cx, &folded), folded);
    assert!(
        shown(cx, format!("user:{}:0", steer.id)).is_none(),
        "the steer folds with the work"
    );
    click(cx, "work-header");
    settle(cx, &conversation);
    let open = [
        "work:run:0".to_owned(),
        format!("chain:{}", before.id),
        format!("user:{}:0", steer.id),
        format!("chain:{}", after.id),
        format!("prose:{}:0", answer.id),
    ];
    assert_eq!(
        painted_order(cx, &open),
        open,
        "the steer between the work before and after it"
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
    let line = crate::activity::line(&item, &std::collections::HashMap::new());
    assert_eq!((line.verb.as_str(), line.object.as_str()), ("Ran", command));
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
    let row = format!("prose:{id}:0");
    let full = shown(cx, row.clone())
        .expect("the open commentary")
        .size
        .height;
    conversation.update(cx, |c, cx| c.toggle_work(&run, false, cx));
    let partial_deadline = Instant::now() + Duration::from_secs(2);
    loop {
        draw(cx);
        let height = shown(cx, row.clone()).map(|bounds| bounds.size.height);
        if height.is_some_and(|height| height > gpui::px(0.5) && height < full - gpui::px(0.5)) {
            break;
        }
        assert!(
            Instant::now() < partial_deadline,
            "the commentary never showed partly closed"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        cx.debug_bounds("work-header").is_some(),
        "the disclosure header must remain mounted during the flight"
    );
    conversation.update(cx, |c, cx| c.toggle_work(&run, true, cx));
    settle(cx, &conversation);
    assert_eq!(
        shown(cx, row).map(|bounds| bounds.size.height),
        Some(full),
        "reopened, the commentary shows whole again"
    );
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
    // Opening shows the commentary's end, read from Forge a window at a
    // time: its start is not laid out.
    let last = format!("prose:{id}:750");
    wait(cx, "the commentary's end", |cx| {
        draw(cx);
        shown(cx, last.clone()).is_some()
    });
    settle(cx, &conversation);
    assert!(shown(cx, format!("prose:{id}:0")).is_none());
    // Closing slides it shut under its header; reversing midway slides it
    // open again from where it was.
    conversation.update(cx, |c, cx| c.toggle_work(&run, false, cx));
    draw(cx);
    assert!(cx.debug_bounds("disclosing").is_some(), "closing slides");
    conversation.update(cx, |c, cx| c.toggle_work(&run, true, cx));
    draw(cx);
    assert!(
        cx.debug_bounds("disclosing").is_some(),
        "the flight reverses"
    );
    settle(cx, &conversation);
    assert!(shown(cx, last.clone()).is_some(), "open again at its end");
    conversation.update(cx, |c, cx| c.toggle_work(&run, false, cx));
    wait(cx, "closed under its header", |cx| {
        draw(cx);
        cx.debug_bounds("disclosing").is_none()
            && cx.debug_bounds("work-header").is_some()
            && shown(cx, last.clone()).is_none()
    });
}

/// A compaction during a run is a line of its work, never a divider that
/// splits the run into two sections.
#[gpui::test]
fn a_compaction_during_a_run_is_a_line_of_its_work(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let ask = forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        ItemKind::UserMessage {
            text: "build it".into(),
            images: Vec::new(),
            steer: false,
        },
    );
    let build = forge.put(&thread, &ItemId::new(), Status::Done, shell("cargo build"));
    let checkpoint = ItemId::new();
    let compact = || ItemKind::Compaction {
        tokens_before: Some(966_000),
        tokens_after: Some(15_000),
    };
    forge.put(&thread, &checkpoint, Status::Done, compact());
    let test = forge.put(&thread, &ItemId::new(), Status::Done, shell("cargo test"));
    let answer = forge.put(&thread, &ItemId::new(), Status::Done, reply("It builds."));
    forge.finish_run(&thread);
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    let folded = [
        format!("user:{}:0", ask.id),
        "work:run:0".to_owned(),
        format!("prose:{}:0", answer.id),
    ];
    wait(cx, "the turn", |cx| {
        draw(cx);
        painted_order(cx, &folded).len() == folded.len()
    });
    assert_eq!(painted_order(cx, &folded), folded, "one work section");
    assert!(
        shown(cx, format!("compaction:{checkpoint}")).is_none(),
        "no divider"
    );
    click(cx, "work-header");
    settle(cx, &conversation);
    let open = [
        "work:run:0".to_owned(),
        format!("chain:{}", build.id),
        format!("entry:{checkpoint}"),
        format!("chain:{}", test.id),
        format!("prose:{}:0", answer.id),
    ];
    assert_eq!(
        painted_order(cx, &open),
        open,
        "the compaction is a line of the work between its calls"
    );
    assert!(
        painted(
            cx,
            "work-compaction-text:Compacted the conversation \u{b7} 966k \u{2192} 15k tokens"
                .to_owned()
        )
        .is_some()
    );
    // While it runs, and when it fails.
    let running = forge.put(&thread, &ItemId::new(), Status::Running, compact());
    let failed = forge.put(&thread, &ItemId::new(), Status::Failed, compact());
    wait(cx, "the other compactions", |cx| {
        draw(cx);
        shown(cx, format!("entry:{}", running.id)).is_some()
            && shown(cx, format!("entry:{}", failed.id)).is_some()
    });
    for words in ["Compacting the conversation", "Compaction failed"] {
        assert!(
            painted(cx, format!("work-compaction-text:{words}")).is_some(),
            "{words}"
        );
    }
}
