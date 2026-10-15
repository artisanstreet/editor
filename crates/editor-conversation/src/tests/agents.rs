//! Subagents: their own view, what they were told and what they report.

use super::*;

/// The lead's messages to an agent name the agent by its task: in the
/// lead's work "Messaged Close renderer gaps", and in Martine's own view as
/// messages where they arrived, with a live line while she works.
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
                direction: artisan_api::MessageDirection::Sent,
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

    // The lead's work: it delegated a task and messaged its agent by name
    // of that task.
    let message_item = loaded(&conversation, cx)
        .into_iter()
        .find(|item| item.id == message.id)
        .unwrap();
    let agents = conversation.read_with(cx, |conversation, _| {
        conversation
            .view
            .as_ref()
            .map(super::super::agent_names)
            .unwrap()
    });
    let line = crate::activity::line(&message_item, &agents);
    assert_eq!(
        (line.verb.as_str(), line.object.as_str()),
        ("Messaged", "Close renderer gaps")
    );
    let lead_work = loaded(&conversation, cx);
    let chain =
        crate::activity::chain_categories(lead_work.iter().filter(|item| item.parent_id.is_none()));
    assert_eq!(
        crate::activity::chain_clause(chain.into_iter()),
        "Delegated a task, exchanged a message"
    );

    // Martine's view: her task, her work, the message where it arrived,
    // her work after it, and her live line (a command of hers runs).
    conversation.update(cx, |conversation, cx| {
        conversation.open_agent(Some(martine.id.clone()), cx);
    });
    let her_view = [
        "agent-task:martine:0",
        "work:start",
        "agent-message:message:0",
        "prose:switch:0",
        "agent-status:martine:working",
    ]
    .map(str::to_owned);
    wait(cx, "her view", |cx| {
        draw(cx);
        painted_order(cx, &her_view).len() == her_view.len()
    });
    assert_eq!(painted_order(cx, &her_view), her_view);
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
    let user = ItemId::new();
    forge.put(
        &thread,
        &user,
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
    // The agent sits in the work's chain ("delegated a task"): open it.
    click(cx, "work-chain");
    settle(cx, &conversation);
    // Like a command, its row reads "Delegated" and the task it got: no
    // name, no status.
    assert!(
        painted(
            cx,
            "work-agent-text:Delegated Match UI: HUD and in-match screens".to_owned()
        )
        .is_some(),
        "the agent's row reads \"Delegated\" then its task, and nothing else"
    );
    click(cx, "work-agent");
    wait(cx, "its task, its work and its report", |cx| {
        draw(cx);
        let child = cx.read(|cx| {
            conversation
                .read(cx)
                .items()
                .find(|item| item.parent_id.as_ref() == Some(&agent))
                .map(|item| item.id.clone())
        });
        let Some(child) = child else { return false };
        let its_view = [
            format!("agent-task:{agent}:0"),
            format!("work:{child}"),
            format!("result-view:{agent}:0"),
        ];
        painted_order(cx, &its_view) == its_view
    });
    let trail = cx.read(|cx| conversation.read(cx).agent_trail());
    assert_eq!(trail, vec![(agent.clone(), name.clone())]);

    conversation.update(cx, |conversation, cx| conversation.open_agent(None, cx));
    wait(cx, "back in the thread", |cx| {
        draw(cx);
        shown(cx, format!("user:{user}:0")).is_some()
    });
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
