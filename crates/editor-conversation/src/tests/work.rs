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
            from_agent: None,
            sender: None,
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
    landed(cx, &conversation);
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
fn a_resumed_agent_never_splits_the_turn_that_resumed_it(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let (earlier, later) = (RunId::from("earlier"), RunId::from("later"));
    let user = |text: &str| ItemKind::UserMessage {
        text: text.into(),
        images: Vec::new(),
        steer: false,
        from_agent: None,
        sender: None,
    };
    let agent = |description: &str| ItemKind::Agent {
        engine: None,
        thread: None,
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
    landed(cx, &conversation);
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

/// A turn Forge carried on after a restart reads as one turn: one header,
/// titled from how the turn ended and timed from its start, never
/// "Interrupted" over the first half and a second header over the rest.
#[gpui::test]
fn a_carried_on_turn_keeps_one_header(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let (first, carried) = (RunId::from("first"), RunId::from("carried"));
    let id = |name: &str| ItemId::from(name.to_owned());
    forge.put_in(
        &first,
        &thread,
        &id("ask"),
        Status::Done,
        ItemKind::UserMessage {
            text: "check the ubuntu tools".into(),
            images: Vec::new(),
            steer: false,
            from_agent: None,
            sender: None,
        },
    );
    forge.put_in(
        &first,
        &thread,
        &id("which"),
        Status::Done,
        shell("which cargo"),
    );
    forge.put_in(
        &carried,
        &thread,
        &id("check"),
        Status::Done,
        shell("cargo check"),
    );
    forge.put_in(
        &carried,
        &thread,
        &id("done"),
        Status::Done,
        reply("It checks."),
    );
    let run = |id: &RunId, status, started_at, ended_at, continuation| artisan_api::Run {
        config: None,
        id: id.clone(),
        thread_id: thread.clone(),
        engine: EngineId::Claude,
        model: None,
        input_cutoff: 0,
        status,
        usage: artisan_api::Usage::default(),
        session_id: None,
        error: None,
        seq: 0,
        started_at,
        ended_at: Some(ended_at),
        continuation,
    };
    for run in [
        run(
            &first,
            artisan_api::RunStatus::Interrupted,
            1_000,
            3_000,
            None,
        ),
        run(
            &carried,
            artisan_api::RunStatus::Done,
            4_000,
            62_000,
            Some(artisan_api::Continuation::Restart),
        ),
    ] {
        forge.runtime.block_on(forge.store.upsert_run(run)).unwrap();
    }
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    landed(cx, &conversation);
    let rows = ["user:ask:0", "work:first:0", "prose:done:0"].map(str::to_owned);
    wait(cx, "the turn", |cx| {
        draw(cx);
        painted_order(cx, &rows).len() == rows.len()
    });
    assert_eq!(painted_order(cx, &rows), rows);
    assert!(
        shown(cx, "work:carried:0").is_none() && shown(cx, "work:first:1").is_none(),
        "the carried-on run opened a second section"
    );
    let title = cx.read(|cx| {
        let summary = conversation.read(cx).row_context.runs[&first].clone();
        let elapsed =
            u64::try_from(summary.ended_at.unwrap() - summary.started_at.unwrap()).unwrap();
        crate::phase::section_title(summary.phase, summary.worked, elapsed)
    });
    assert_eq!(title, "Worked for 1m 1s");
}

/// A run that streamed in live and was carried on after a restart stays in
/// its turn: its last command and the carried run's first one read as one
/// "Ran 2 commands", not two neighbouring "Ran a command" rows.
#[gpui::test]
fn commands_either_side_of_a_live_restart_share_one_chain(cx: &mut TestAppContext) {
    use artisan_api::RunStatus;
    let forge = Forge::start();
    let thread = forge.thread();
    let (origin, first, carried) = (
        RunId::from("origin"),
        RunId::from("first"),
        RunId::from("carried"),
    );
    let id = |name: &str| ItemId::from(name.to_owned());
    let run = |id: &RunId, status, started_at, ended_at, continuation| artisan_api::Run {
        config: None,
        id: id.clone(),
        thread_id: thread.clone(),
        engine: EngineId::Claude,
        model: None,
        input_cutoff: 0,
        status,
        usage: artisan_api::Usage::default(),
        session_id: None,
        error: None,
        seq: 0,
        started_at,
        ended_at,
        continuation,
    };
    forge.put_in(
        &origin,
        &thread,
        &id("ask"),
        Status::Done,
        ItemKind::UserMessage {
            text: "measure the capped forge".into(),
            images: Vec::new(),
            steer: false,
            from_agent: None,
            sender: None,
        },
    );
    let upsert = |run| forge.runtime.block_on(forge.store.upsert_run(run)).unwrap();
    // An earlier restart already carried the turn on: the open page holds
    // only the message's run, the live run is the host's latest.
    upsert(run(&origin, RunStatus::Interrupted, 0, Some(500), None));
    upsert(run(
        &first,
        RunStatus::Running,
        1_000,
        None,
        Some(artisan_api::Continuation::Restart),
    ));
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    landed(cx, &conversation);
    // The first run's work arrives live, then Forge restarts and carries
    // the turn on in a new run.
    forge.put_in(
        &first,
        &thread,
        &id("install"),
        Status::Done,
        shell("ae install"),
    );
    forge.put_in(
        &first,
        &thread,
        &id("wait"),
        Status::Cancelled,
        shell("until grep -q done log"),
    );
    upsert(run(
        &first,
        RunStatus::Interrupted,
        1_000,
        Some(3_000),
        Some(artisan_api::Continuation::Restart),
    ));
    upsert(run(
        &carried,
        RunStatus::Running,
        4_000,
        None,
        Some(artisan_api::Continuation::Restart),
    ));
    forge.put_in(
        &carried,
        &thread,
        &id("check"),
        Status::Done,
        shell("grep log"),
    );
    wait(cx, "the carried command", |cx| {
        loaded(&conversation, cx).len() == 4
    });
    wait(cx, "one chain", |cx| {
        draw(cx);
        shown(cx, "chain:install").is_some()
    });
    assert!(
        shown(cx, "work:first:0").is_none()
            && shown(cx, "work:carried:0").is_none()
            && shown(cx, "chain:check").is_none(),
        "the carried run's command split from the turn it carries on"
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
    // Settled, a section is only ever worked, thought, stopped or failed.
    for (status, title) in [
        (RunStatus::Cancelled, "Stopped after 1m 1s"),
        (RunStatus::Interrupted, "Stopped after 1m 1s"),
        (RunStatus::Failed, "Failed after 1m 1s"),
        (RunStatus::Refused, "Failed after 1m 1s"),
    ] {
        assert_eq!(section_title(Phase::Settled(status), true, minute), title);
    }
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
    // Every repeat is a counted noun, never "searched X 3 times".
    assert_eq!(
        chain_clause(
            [
                ActivityCategory::FileSearch,
                ActivityCategory::FileSearch,
                ActivityCategory::ThreadSearch,
                ActivityCategory::ThreadSearch,
                ActivityCategory::ThreadRead,
                ActivityCategory::ThreadRead,
                ActivityCategory::WebSearch,
                ActivityCategory::WebSearch,
                ActivityCategory::WebPage,
                ActivityCategory::WebPage,
            ]
            .into_iter()
        ),
        "Ran 2 searches, ran 2 thread searches, read 2 threads, ran 2 web searches, \
         read 2 web pages"
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
        from_agent: None,
        sender: None,
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
    // The messages' markdown prepares off the main thread; the header
    // stops moving once it has.
    settle(cx, &conversation);
    draw(cx);
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

/// Codex drops a reply a steer interrupts and answers again; the dropped
/// one ends empty. It leaves no blank row and doesn't split the work around
/// it into two.
#[gpui::test]
fn a_reply_that_ended_empty_leaves_no_gap_in_the_work(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let ask = forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        ItemKind::UserMessage {
            text: "is this it?".into(),
            images: Vec::new(),
            steer: false,
            from_agent: None,
            sender: None,
        },
    );
    let before = forge.put(&thread, &ItemId::new(), Status::Done, shell("rg crawl"));
    let dropped = forge.put(&thread, &ItemId::new(), Status::Cancelled, reply(""));
    let after = forge.put(&thread, &ItemId::new(), Status::Done, shell("cargo test"));
    let answer = forge.put(&thread, &ItemId::new(), Status::Done, reply("Yes."));
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
    assert!(shown(cx, "work:run:1").is_none(), "one stretch of work");
    assert!(shown(cx, format!("prose:{}:0", dropped.id)).is_none());
    settle(cx, &conversation);
    draw(cx);
    click(cx, "work-header");
    settle(cx, &conversation);
    let open = [
        "work:run:0".to_owned(),
        format!("chain:{}", before.id),
        format!("prose:{}:0", answer.id),
    ];
    assert_eq!(painted_order(cx, &open), open);
    assert!(
        shown(cx, format!("chain:{}", after.id)).is_none(),
        "both commands share one chain"
    );
    assert!(shown(cx, format!("prose:{}:0", dropped.id)).is_none());
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

/// A compaction looks the same wherever it happens: the divider. Mid-run it
/// is part of the work: it folds away with it, and in the open work it sits
/// between the calls around it, under the run's one header.
#[gpui::test]
fn a_compaction_is_the_divider_wherever_it_happens(cx: &mut TestAppContext) {
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
            from_agent: None,
            sender: None,
        },
    );
    let build = forge.put(&thread, &ItemId::new(), Status::Done, shell("cargo build"));
    let checkpoint = ItemId::new();
    forge.put(
        &thread,
        &checkpoint,
        Status::Done,
        ItemKind::Compaction {
            summary: None,
            tokens_before: Some(966_000),
            tokens_after: Some(15_000),
            switch: None,
        },
    );
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
    assert_eq!(painted_order(cx, &folded), folded);
    assert!(
        shown(cx, format!("compaction:{checkpoint}")).is_none(),
        "the divider folds away with the work"
    );
    click(cx, "work-header");
    settle(cx, &conversation);
    let open = [
        "work:run:0".to_owned(),
        format!("chain:{}", build.id),
        format!("compaction:{checkpoint}"),
        format!("chain:{}", test.id),
        format!("prose:{}:0", answer.id),
    ];
    assert_eq!(
        painted_order(cx, &open),
        open,
        "the divider sits between the calls around it"
    );
    assert!(
        shown(cx, "work:run:1".to_owned()).is_none(),
        "the work after it has no header of its own"
    );
    assert!(
        cx.debug_bounds("work-compaction").is_none(),
        "no second look for a compaction"
    );
}

/// A steer sent while the context compacts waits behind the divider: the
/// run reads as compacting, not "Thinking", until the compaction ends.
#[gpui::test]
fn a_steer_mid_compaction_keeps_the_run_compacting(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let message = |text: &str, steer| ItemKind::UserMessage {
        text: text.into(),
        images: Vec::new(),
        steer,
        from_agent: None,
        sender: None,
    };
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        message("build it", false),
    );
    forge.put(&thread, &ItemId::new(), Status::Done, shell("cargo build"));
    let compaction = forge.put(
        &thread,
        &ItemId::new(),
        Status::Running,
        ItemKind::Compaction {
            summary: None,
            tokens_before: Some(966_000),
            tokens_after: None,
            switch: None,
        },
    );
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        message("use subagents", true),
    );
    forge
        .runtime
        .block_on(forge.store.upsert_run(artisan_api::Run {
            config: None,
            id: RunId::from("run"),
            thread_id: thread.clone(),
            engine: EngineId::Codex,
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
    wait(cx, "the steer", |cx| loaded(&conversation, cx).len() == 4);
    let phase = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            conversation
                .read(cx)
                .row_context
                .runs
                .get(&RunId::from("run"))
                .map(|summary| summary.phase)
        })
    };
    assert_eq!(phase(cx), Some(crate::phase::Phase::Compacting));
    assert_eq!(
        crate::phase::status_copy(crate::phase::Phase::Compacting),
        None,
        "no status line competes with the divider"
    );
    // The compaction ends: the steer's turn thinks.
    forge.put(
        &thread,
        &compaction.id,
        Status::Done,
        ItemKind::Compaction {
            summary: None,
            tokens_before: Some(966_000),
            tokens_after: Some(15_000),
            switch: None,
        },
    );
    wait(cx, "the compaction to end", |cx| {
        phase(cx) == Some(crate::phase::Phase::Thinking)
    });
}

/// Opening or closing a work section moves the reply below only while the
/// section slides: the frame its flight ends, the reply stays where it was.
#[gpui::test]
fn the_reply_rests_where_a_disclosure_leaves_it(cx: &mut TestAppContext) {
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
            from_agent: None,
            sender: None,
        },
    );
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        ItemKind::AssistantMessage {
            text: "First I look around.\n\nThen I build.".into(),
            phase: Phase::Commentary,
        },
    );
    for n in 0..4 {
        forge.put(
            &thread,
            &ItemId::new(),
            Status::Done,
            shell(&format!("cargo build -p crate{n}")),
        );
    }
    let answer = ItemId::new();
    forge.put(
        &thread,
        &answer,
        Status::Done,
        reply("It builds.\n\nAll good."),
    );
    forge.finish_run(&thread);
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    landed(cx, &conversation);
    settle(cx, &conversation);
    // Where the reply and the last row below it sit.
    let reply_top = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            let c = conversation.read(cx);
            let index = *c
                .row_context
                .key_rows
                .get(&crate::fold::RowKey::Item(answer.clone()))?;
            let top = |ix| {
                c.list
                    .bounds_for_item(ix)
                    .map(|bounds| f32::from(bounds.top()))
            };
            Some((top(index)?, top(c.rows.len() - 1)?))
        })
    };
    for toggle in ["open", "close"] {
        click(cx, "work-header");
        let mut last: Option<(f32, f32)> = None;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            cx.run_until_parked();
            draw(cx);
            // Cached rows drop their test marks: ask the conversation.
            let disclosing = cx.read(|cx| !conversation.read(cx).disclosures.is_empty());
            let top = reply_top(cx);
            if !disclosing {
                let (before, after) = (
                    last.unwrap_or_else(|| panic!("{toggle}: the flight painted the reply")),
                    top.expect("the reply"),
                );
                assert!(
                    (after.0 - before.0).abs() < 1.0 && (after.1 - before.1).abs() < 1.0,
                    "{toggle}: the reply moved from {before:?} to {after:?} as the flight ended"
                );
                break;
            }
            last = top.or(last);
            assert!(
                Instant::now() < deadline,
                "{toggle}: the flight never ended"
            );
            std::thread::sleep(Duration::from_millis(8));
        }
    }
}

/// The commands that open a work section are a collapsible of their own.
/// Toggling it moves only what it holds: the section's header stays put.
#[gpui::test]
#[expect(
    clippy::significant_drop_tightening,
    reason = "`c` is a plain borrow of the entity; there is nothing to release early"
)]
fn toggling_the_first_commands_leaves_their_section_header(cx: &mut TestAppContext) {
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
            from_agent: None,
            sender: None,
        },
    );
    for n in 0..3 {
        forge.put(
            &thread,
            &ItemId::new(),
            Status::Done,
            shell(&format!("cargo build -p crate{n}")),
        );
    }
    forge.put(&thread, &ItemId::new(), Status::Done, reply("It builds."));
    forge.finish_run(&thread);
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    landed(cx, &conversation);
    settle(cx, &conversation);
    let flying = |cx: &mut VisualTestContext| {
        cx.run_until_parked();
        draw(cx);
        cx.read(|cx| !conversation.read(cx).disclosures.is_empty())
    };
    let header = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            let c = conversation.read(cx);
            let index = c
                .rows
                .iter()
                .position(|(row, _)| matches!(row, crate::fold::Row::Work { .. }))?;
            let bounds = c.list.bounds_for_item(index)?;
            Some((f32::from(bounds.top()), f32::from(bounds.size.height)))
        })
    };
    let ride =
        |cx: &mut VisualTestContext, what: &str, each: &mut dyn FnMut(&mut VisualTestContext)| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while flying(cx) {
                each(cx);
                assert!(Instant::now() < deadline, "{what}: the flight never ended");
                std::thread::sleep(Duration::from_millis(8));
            }
        };
    click(cx, "work-header");
    ride(cx, "the section", &mut |_| {});
    let rest = header(cx).expect("the section header");
    for toggle in ["open", "close"] {
        click(cx, "work-chain");
        ride(cx, toggle, &mut |cx| {
            let now = header(cx).expect("the section header");
            assert!(
                (now.0 - rest.0).abs() < 1.0 && (now.1 - rest.1).abs() < 1.0,
                "{toggle}: the commands moved their section header from {rest:?} to {now:?}"
            );
        });
    }
}

/// Unfolding commands that arrived a moment ago never hides the live
/// status line after them: they are history once the reader opens them.
#[gpui::test]
fn opening_fresh_commands_keeps_the_working_line(cx: &mut TestAppContext) {
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
            from_agent: None,
            sender: None,
        },
    );
    forge.put(&thread, &ItemId::new(), Status::Done, shell("cargo check"));
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
    wait(cx, "the live work", |cx| {
        loaded(&conversation, cx).len() == 2
    });
    settle(cx, &conversation);
    cx.update(|window, _| window.activate_window());
    // Two more commands arrive while the reader watches.
    for command in ["cargo build", "cargo test"] {
        forge.put(&thread, &ItemId::new(), Status::Done, shell(command));
    }
    wait(cx, "the fresh commands", |cx| {
        draw(cx);
        loaded(&conversation, cx).len() == 4 && cx.debug_bounds("work-chain").is_some()
    });
    let status = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            let c = conversation.read(cx);
            let index = *c
                .row_context
                .key_rows
                .get(&crate::fold::RowKey::Status(RunId::from("run")))?;
            c.list
                .bounds_for_item(index)
                .map(|bounds| f32::from(bounds.size.height))
        })
    };
    let before = status(cx).expect("the working line");
    click(cx, "work-chain");
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(600) {
        cx.run_until_parked();
        draw(cx);
        // Hidden, the last row keeps only the end space under the transcript.
        let height = status(cx).expect("the working line");
        assert!(
            (height - before).abs() < 1.0,
            "opening the commands hid the working line ({height} px, was {before} px, {:?} in)",
            start.elapsed()
        );
        std::thread::sleep(Duration::from_millis(8));
    }
}

/// The latest turn's note shows on the composer's strip ahead of the goal,
/// never in the transcript; the next turn clears it, and closing it brings
/// the goal back.
#[gpui::test]
fn the_latest_note_takes_the_goal_strip_until_closed_or_the_next_turn(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        ItemKind::Notice {
            kind: artisan_api::NoticeKind::Goal {
                state: "active".into(),
            },
            text: "Ship cascading deletes.".into(),
        },
    );
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        ItemKind::UserMessage {
            text: "add cascading deletes".into(),
            images: Vec::new(),
            steer: false,
            from_agent: None,
            sender: None,
        },
    );
    forge.put(&thread, &ItemId::new(), Status::Done, reply("Done."));
    forge.finish_run(&thread);
    let note = |run: &str, text: &str| {
        let id = ItemId::new();
        forge.put_in(
            &RunId::from(run),
            &thread,
            &id,
            Status::Done,
            ItemKind::Notice {
                kind: artisan_api::NoticeKind::YouShouldKnow { heads_up: true },
                text: text.into(),
            },
        );
        id
    };
    let first = note("run", "Deleting a customer now deletes their orders.");
    let dir = tempfile::tempdir().unwrap();
    let (core, host, _conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the note on the strip", |cx| {
        draw(cx);
        cx.debug_bounds("composer-note-label").is_some()
    });
    assert!(
        cx.debug_bounds("composer-goal-label").is_none(),
        "the note takes the goal's place"
    );
    assert!(
        shown(cx, format!("note:{first}:0")).is_none(),
        "the note is not a card in the transcript"
    );

    // The next turn clears it; the goal shows again.
    forge.put_in(
        &RunId::from("run-2"),
        &thread,
        &ItemId::new(),
        Status::Done,
        reply("Next turn."),
    );
    forge.finish_run_in(&RunId::from("run-2"), &thread);
    wait(cx, "the goal back after the next turn", |cx| {
        draw(cx);
        cx.debug_bounds("composer-note-label").is_none()
            && cx.debug_bounds("composer-goal-label").is_some()
    });

    // That turn's own note, closed, gives the strip back to the goal.
    note("run-2", "Migrations now run when the server starts.");
    wait(cx, "the new note", |cx| {
        draw(cx);
        cx.debug_bounds("composer-note-label").is_some()
    });
    let label = cx.debug_bounds("composer-note-label").unwrap();
    let close = cx.debug_bounds("composer-note-close").unwrap();
    assert!(
        close.right() > label.right() - gpui::px(40.0),
        "the close mark ends the strip: {close:?} in {label:?}"
    );
    click(cx, "composer-note-close");
    wait(cx, "the goal back after closing", |cx| {
        draw(cx);
        cx.debug_bounds("composer-note-label").is_none()
            && cx.debug_bounds("composer-goal-label").is_some()
    });
}

/// Opening a thread that has a goal finds its strip already there: it
/// doesn't grow out of the tab as if the goal had just been set. A note
/// arriving later swaps into it without growing it again.
#[gpui::test]
fn the_strip_is_already_there_when_a_thread_opens(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        ItemKind::Notice {
            kind: artisan_api::NoticeKind::Goal {
                state: "active".into(),
            },
            text: "Ship cascading deletes.".into(),
        },
    );
    forge.put(&thread, &ItemId::new(), Status::Done, reply("Done."));
    forge.finish_run(&thread);
    let dir = tempfile::tempdir().unwrap();
    let (core, host, _conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    let strip = |cx: &mut VisualTestContext| {
        draw(cx);
        cx.debug_bounds("tab-card-strip")
            .map(|bounds| bounds.size.width)
    };
    wait(cx, "the goal's strip", |cx| strip(cx).is_some());
    let first = strip(cx).unwrap();
    // Past the whole growth.
    std::thread::sleep(Duration::from_millis(450));
    assert_eq!(
        strip(cx).unwrap(),
        first,
        "the strip was whole on its first frame"
    );

    forge.put_in(
        &RunId::from("run-2"),
        &thread,
        &ItemId::new(),
        Status::Done,
        ItemKind::Notice {
            kind: artisan_api::NoticeKind::YouShouldKnow { heads_up: true },
            text: "Deleting a customer now deletes their orders.".into(),
        },
    );
    forge.finish_run_in(&RunId::from("run-2"), &thread);
    wait(cx, "the note", |cx| {
        draw(cx);
        cx.debug_bounds("composer-note-label").is_some()
    });
    assert_eq!(
        strip(cx).unwrap(),
        first,
        "the note swaps into the strip that is already there"
    );
}

/// A run has its header from the moment it's accepted: while it only
/// thinks, before any work, and still once it ends without doing any.
#[gpui::test]
fn an_accepted_run_has_its_header_before_any_work(cx: &mut TestAppContext) {
    use artisan_api::RunStatus;
    let forge = Forge::start();
    let thread = forge.thread();
    let run = RunId::from("run");
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        ItemKind::UserMessage {
            text: "look into it".into(),
            images: Vec::new(),
            steer: false,
            from_agent: None,
            sender: None,
        },
    );
    forge.end_run_in(&run, &thread, RunStatus::Running);
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    landed(cx, &conversation);
    wait(cx, "the header", |cx| {
        draw(cx);
        shown(cx, "work:run:0").is_some()
    });
    assert_eq!(
        painted_order(cx, &["status:run".to_owned(), "work:run:0".to_owned()]),
        ["work:run:0", "status:run"],
        "the header sits above the thinking line"
    );

    // It ended without doing anything: the header stays, now saying so.
    forge.end_run_in(&run, &thread, RunStatus::Failed);
    wait(cx, "the run settled", |cx| {
        draw(cx);
        shown(cx, "status:run").is_none()
    });
    assert!(shown(cx, "work:run:0").is_some(), "the settled header");
}
