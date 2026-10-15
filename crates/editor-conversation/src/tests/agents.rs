//! Subagents: their own view, what they were told and what they report.

use super::*;

/// The lead's messages to an agent read as talking to it: in the lead's
/// work "Talked to Martine" under Agents (one agent, however often it was
/// messaged), and in Martine's own view as messages where they arrived,
/// with a live line while she works.
#[gpui::test]
#[expect(
    clippy::too_many_lines,
    reason = "one scenario: a lead, its agent and a message between them"
)]
fn messages_to_an_agent_show_in_its_view_and_read_as_talking_to_it(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let id = |name: &str| ItemId::from(name.to_owned());
    let put = |id: ItemId, parent: Option<ItemId>, status: Status, kind: ItemKind| {
        forge
            .runtime
            .block_on(forge.store.upsert_item(NewItem {
                id,
                thread_id: thread.clone(),
                run_id: Some(RunId::from("run")),
                parent_id: parent,
                native_id: None,
                status,
                kind,
            }))
            .unwrap()
    };
    let martine = put(
        id("martine"),
        None,
        Status::Running,
        ItemKind::Agent {
            name: None,
            description: "Close renderer gaps".into(),
            prompt: Some("Close the renderer gaps: thin geometry and glass.".into()),
            agent_id: Some("ae5992bd2a38ec98a".into()),
            context_used: None,
            model: None,
            background: true,
            result: None,
            label: None,
        },
    );
    let commentary = |text: &str| ItemKind::AssistantMessage {
        text: text.into(),
        phase: Phase::Commentary,
    };
    put(
        id("start"),
        Some(martine.id.clone()),
        Status::Done,
        commentary("Starting with glass."),
    );
    let message = put(
        id("message"),
        None,
        Status::Done,
        ItemKind::Tool {
            tool: artisan_api::ToolKind::AgentMessage {
                recipient: "ae5992bd2a38ec98a".into(),
                target: Some(martine.id.clone()),
                summary: Some("Use content/variants".into()),
                text: "Map variants already exist: use content/variants for weather.".into(),
            },
            title: None,
            output: None,
            exit_code: None,
            approval: None,
        },
    );
    put(
        id("switch"),
        Some(martine.id.clone()),
        Status::Done,
        commentary("Switching to the variants."),
    );
    put(
        id("build"),
        Some(martine.id.clone()),
        Status::Running,
        shell("cargo build -p render"),
    );

    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the thread", |cx| loaded(&conversation, cx).len() == 2);

    // The lead's work: one chain, counting Martine once.
    let message_item = loaded(&conversation, cx)
        .into_iter()
        .find(|item| item.id == message.id)
        .unwrap();
    assert_eq!(
        crate::activity::category(&message_item),
        Some(crate::activity::ActivityCategory::Subagent),
        "a message to an agent counts under Agents"
    );
    let sent = crate::activity::agent_message(&message_item).unwrap();
    assert_eq!(sent.to.as_ref(), Some(&martine.id));
    let lead_work = loaded(&conversation, cx);
    let chain =
        crate::activity::chain_categories(lead_work.iter().filter(|item| item.parent_id.is_none()));
    assert_eq!(
        crate::activity::chain_clause(chain.into_iter()),
        "Talked to an agent",
        "Martine started and messaged is one agent"
    );

    // Martine's view: her task, her work, the message where it arrived,
    // her work after it, and her live line (a command of hers runs).
    conversation.update(cx, |conversation, cx| {
        conversation.open_agent(Some(martine.id.clone()), cx);
    });
    wait(cx, "her view", |cx| {
        cx.read(|cx| {
            conversation.read(cx).rows.iter().any(|(row, _)| {
                matches!(
                    row,
                    Row::PlainBlock {
                        section: crate::fold::PlainSection::AgentMessage,
                        ..
                    }
                )
            })
        })
    });
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
                section: crate::fold::PlainSection::AgentTask,
                ..
            } => format!("task {id}"),
            Row::PlainBlock {
                id,
                section: crate::fold::PlainSection::AgentMessage,
                ..
            } => format!("message {id}"),
            Row::AgentStatus(_, working) => format!("live working={working}"),
            Row::Work { entries, .. } => format!(
                "work {}",
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
            "task martine",
            "work start",
            "message message",
            "work switch build",
            "live working=true",
        ]
    );
}

/// The context meter reads the open agent's own context and model: an
/// agent's view showed the lead's.
#[gpui::test]
fn an_agents_view_reads_its_own_context(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    let jenny = forge.put(
        &thread,
        &ItemId::new(),
        Status::Running,
        ItemKind::Agent {
            name: None,
            description: "Perf overlay".into(),
            prompt: None,
            agent_id: None,
            context_used: Some(91_464),
            model: Some("claude-haiku-5-5".into()),
            background: true,
            result: None,
            label: None,
        },
    );
    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the thread", |cx| loaded(&conversation, cx).len() == 1);
    let reading = |cx: &mut VisualTestContext| cx.read(|cx| conversation.read(cx).agent_context());
    assert_eq!(reading(cx), None, "the thread itself reads the lead's run");
    conversation.update(cx, |conversation, cx| {
        conversation.open_agent(Some(jenny.id.clone()), cx);
    });
    assert_eq!(
        reading(cx),
        Some(Some((91_464, Some("claude-haiku-5-5".to_owned())))),
        "the agent's view reads its own context"
    );
}

/// Forge names a subagent from Artisan's bank. Its row in the work opens
/// its own view (the task it got, its work, its report) with its name in
/// the trail; closing it goes back to the thread.
#[gpui::test]
#[expect(
    clippy::too_many_lines,
    reason = "one scenario: store a named subagent, open it, go back"
)]
fn a_subagent_opens_into_its_own_view(cx: &mut TestAppContext) {
    let forge = Forge::start();
    let thread = forge.thread();
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        ItemKind::UserMessage {
            text: "build the HUD".into(),
            images: Vec::new(),
            steer: false,
        },
    );
    let agent = ItemId::new();
    forge.put(
        &thread,
        &agent,
        Status::Running,
        ItemKind::Agent {
            name: Some("general-purpose".into()),
            description: "Match UI: HUD and in-match screens".into(),
            prompt: None,
            agent_id: None,
            context_used: None,
            model: None,
            background: true,
            result: None,
            label: None,
        },
    );
    forge
        .runtime
        .block_on(forge.store.upsert_item(NewItem {
            id: ItemId::new(),
            thread_id: thread.clone(),
            run_id: Some(RunId::from("run")),
            parent_id: Some(agent.clone()),
            native_id: None,
            status: Status::Done,
            kind: shell("cargo test -p client"),
        }))
        .unwrap();
    let done = forge.put(
        &thread,
        &agent,
        Status::Done,
        ItemKind::Agent {
            name: Some("general-purpose".into()),
            description: "Match UI: HUD and in-match screens".into(),
            prompt: None,
            agent_id: None,
            context_used: None,
            model: None,
            background: true,
            result: Some("The HUD is in.".into()),
            label: None,
        },
    );
    let ItemKind::Agent {
        label: Some(name), ..
    } = &done.kind
    else {
        panic!("Forge named the agent and kept the name: {done:?}");
    };
    assert!(artisan_api::AGENT_NAMES.contains(&name.as_str()), "{name}");
    forge.put(
        &thread,
        &ItemId::new(),
        Status::Done,
        reply("Started the HUD agent."),
    );
    forge.finish_run(&thread);

    let dir = tempfile::tempdir().unwrap();
    let (core, host, conversation, cx) = editor(&forge, dir.path(), cx);
    open(&core, &host, &thread, cx);
    wait(cx, "the thread", |cx| loaded(&conversation, cx).len() == 3);
    settle(cx, &conversation);
    // The work section is folded under its reply: open it.
    click(cx, "work-header");
    settle(cx, &conversation);
    // The agent sits in the work's chain ("talked to an agent"): open it.
    click(cx, "work-chain");
    settle(cx, &conversation);
    click(cx, "work-agent");
    settle(cx, &conversation);
    assert!(
        cx.read(|cx| conversation.read(cx).agent_task.is_none()),
        "completed agent ancestry must release its loading task"
    );
    let rows = |cx: &mut VisualTestContext| {
        cx.read(|cx| {
            conversation
                .read(cx)
                .rows
                .iter()
                .map(|(row, _)| row.clone())
                .collect::<Vec<_>>()
        })
    };
    let shown = rows(cx);
    assert!(
        matches!(shown.first(), Some(Row::PlainBlock {id, section: crate::fold::PlainSection::AgentTask, block: 0}) if *id == agent),
        "{shown:?}"
    );
    assert!(
        matches!(shown.last(), Some(Row::TextBlock {id, section: crate::fold::TextSection::AgentResult, ..}) if *id == agent),
        "{shown:?}"
    );
    assert!(
        shown
            .iter()
            .any(|row| matches!(row, Row::Work { entries, .. } if entries.len() == 1)),
        "its own work: {shown:?}"
    );
    let trail = cx.read(|cx| conversation.read(cx).agent_trail());
    assert_eq!(trail, vec![(agent.clone(), name.clone())]);

    conversation.update(cx, |conversation, cx| conversation.open_agent(None, cx));
    settle(cx, &conversation);
    assert!(
        rows(cx).iter().any(|row| matches!(
            row,
            Row::PlainBlock {
                section: crate::fold::PlainSection::User,
                block: 0,
                ..
            }
        )),
        "back in the thread"
    );
}

#[test]
fn copying_an_agents_view_keeps_its_task_work_and_report_in_order() {
    let task = crate::fold::selection_order(
        true,
        10,
        crate::fold::Slot::Plain(crate::fold::PlainSection::AgentTask),
    );
    let child = crate::fold::selection_order(true, 25, crate::fold::Slot::Prose);
    let result = crate::fold::selection_order(
        true,
        10,
        crate::fold::Slot::Text(crate::fold::TextSection::AgentResult),
    );
    assert!(
        task < child && child < result,
        "reused Agent position must not put its result before child work"
    );
    assert_eq!(
        crate::fold::selection_order(
            false,
            10,
            crate::fold::Slot::Text(crate::fold::TextSection::AgentResult)
        ),
        vec![10],
        "root preserves canonical Item position"
    );
}
