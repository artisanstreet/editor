use super::{Item, ItemKind, TextSection, ThreadView, fold, turn_of};
use artisan_api::{
    ContentId, ContentRef, ItemId, Run, RunId, RunStatus, Status, TextContent, ThreadId,
};

fn agent(name: &str, description: TextContent, prompt: Option<TextContent>) -> Item {
    Item {
        id: ItemId("native-agent".into()),
        thread_id: ThreadId("thread".into()),
        position: 1,
        seq: 1,
        run_id: None,
        parent_id: None,
        status: Status::Running,
        created_at: 0,
        updated_at: 0,
        kind: ItemKind::Agent {
            name: Some(name.into()),
            description,
            prompt,
            agent_id: None,
            context_used: None,
            model: None,
            background: false,
            result: None,
            label: None,
            engine: None,
            thread: None,
        },
    }
}

#[test]
fn native_agent_addresses_without_a_prompt_show_the_encryption_fallback() {
    for name in ["/root", "/root/reviewer"] {
        let item = agent(name, name.into(), None);
        assert_eq!(
            TextSection::AgentTask.inline_text(&item),
            Some("[This message is encrypted.]")
        );
    }
    for (name, description) in [
        ("/root/reviewer", "Review the patch"),
        ("reviewer", "reviewer"),
        ("/rooted", "/rooted"),
    ] {
        let item = agent(name, description.into(), None);
        assert_eq!(TextSection::AgentTask.inline_text(&item), Some(description));
    }
}

#[test]
fn agent_task_uses_the_prompt_and_keeps_stored_prompts_out_of_inline_fallbacks() {
    let item = agent("/root", "/root".into(), Some("Inspect the decoder".into()));
    assert_eq!(
        TextSection::AgentTask.inline_text(&item),
        Some("Inspect the decoder")
    );
    let reference = ContentRef {
        id: ContentId("a".repeat(64)),
        bytes: 100,
        lines: 2,
    };
    let item = agent(
        "/root",
        "/root".into(),
        Some(TextContent::Stored(reference.clone())),
    );
    assert_eq!(TextSection::AgentTask.inline_text(&item), None);
    assert_eq!(
        TextSection::AgentTask.text(&item),
        Some(&TextContent::Stored(reference))
    );
}

#[test]
fn missing_native_task_text_has_an_explanation() {
    let item = agent("reviewer", "".into(), None);
    assert_eq!(
        TextSection::AgentTask.inline_text(&item),
        Some("Task message not provided by the engine.")
    );
}

#[test]
fn empty_agent_messages_use_the_summary_without_replacing_reported_text() {
    use artisan_api::{MessageDirection, ToolKind};

    let mut item = agent("reviewer", "Review".into(), None);
    for (text, expected) in [("", "Encrypted handoff"), ("The findings", "The findings")] {
        item.kind = ItemKind::Tool {
            tool: ToolKind::AgentMessage {
                recipient: "/root/reviewer".into(),
                target: None,
                summary: Some("Encrypted handoff".into()),
                text: text.into(),
                direction: MessageDirection::Sent,
            },
            title: None,
            output: None,
            exit_code: None,
            approval: None,
        };
        assert_eq!(TextSection::AgentMessage.inline_text(&item), Some(expected));
    }
}

#[test]
fn a_resumed_agent_shows_its_status_until_it_reports_again() {
    let tail = |status: Status| {
        let mut item = agent("general-purpose", "Task".into(), None);
        item.status = status;
        if let ItemKind::Agent { result, .. } = &mut item.kind {
            *result = Some("Earlier report".into());
        }
        let id = item.id.clone();
        let (mut view, _) = super::ThreadView::open(ThreadId("thread".into()));
        view.context.insert(id.clone(), std::sync::Arc::new(item));
        super::fold(&view, None, Some(&id))
            .rows
            .into_iter()
            .skip(1)
            .collect::<Vec<_>>()
    };
    assert!(matches!(
        tail(Status::Running).as_slice(),
        [super::Row::AgentStatus(_, None)]
    ));
    assert!(matches!(
        tail(Status::Done).as_slice(),
        [super::Row::AgentResult(_)]
    ));
}

#[test]
fn the_status_line_names_what_runs_in_the_foreground() {
    use artisan_api::ToolKind;
    use std::collections::HashMap;

    let shell = |id: &str, title: Option<&str>, background: bool, status: Status| {
        let mut item = agent("general-purpose", "Task".into(), None);
        item.id = ItemId(id.into());
        item.status = status;
        item.kind = ItemKind::Tool {
            tool: ToolKind::Shell {
                command: "make".into(),
                cwd: None,
                background,
                watch: false,
            },
            title: title.map(str::to_owned),
            output: None,
            exit_code: None,
            approval: None,
        };
        item
    };
    let titled = shell("a", Some("Relaunch Unreal Editor"), false, Status::Running);
    let plain = shell("b", None, false, Status::Running);
    let detached = shell("c", Some("Serve"), true, Status::Running);
    let finished = shell("d", Some("Build"), false, Status::Done);
    let mut delegate = agent("general-purpose", "Task".into(), None);
    if let ItemKind::Agent { label, .. } = &mut delegate.kind {
        *label = Some("Sunniva".into());
    }
    let doing = |items: &[&Item]| super::activity::doing(items, &HashMap::new());
    assert_eq!(
        doing(&[&titled, &detached, &finished]).as_deref(),
        Some("Relaunch Unreal Editor")
    );
    assert_eq!(doing(&[&plain]).as_deref(), Some("Running a command"));
    assert_eq!(
        doing(&[&titled, &plain]).as_deref(),
        Some("Running 2 commands")
    );
    assert_eq!(doing(&[&delegate]).as_deref(), Some("Waiting on Sunniva"));
    assert_eq!(
        doing(&[&delegate, &plain]).as_deref(),
        Some("Using 2 tools")
    );
    assert_eq!(doing(&[&detached, &finished]), None);
}

fn run(id: &str, status: RunStatus, started_at: artisan_api::Timestamp, carried: bool) -> Run {
    Run {
        config: None,
        id: RunId(id.into()),
        thread_id: ThreadId("thread".into()),
        engine: artisan_api::EngineId::Claude,
        model: None,
        input_cutoff: 0,
        state: artisan_api::RunOutcome::of(status).map_or(
            artisan_api::RunState::Running,
            |outcome| artisan_api::RunState::Finished {
                outcome,
                error: None,
                ended_at: started_at,
            },
        ),
        usage: artisan_api::Usage::default(),
        session_id: None,
        seq: 1,
        started_at,
        continuation: carried.then_some(artisan_api::Continuation::Restart),
    }
}

fn reply_in(run: &str, position: u64) -> Item {
    Item {
        id: ItemId(format!("reply-{position}")),
        thread_id: ThreadId("thread".into()),
        position,
        seq: position,
        run_id: Some(RunId(run.into())),
        parent_id: None,
        status: Status::Done,
        created_at: 0,
        updated_at: 0,
        kind: ItemKind::AssistantMessage {
            text: format!("Reply {position}.").into(),
            phase: artisan_api::Phase::Final,
        },
    }
}

fn thread_with(runs: Vec<Run>) -> ThreadView {
    let (mut view, _) = ThreadView::open(ThreadId("thread".into()));
    for (index, run) in runs.into_iter().enumerate() {
        let position = u64::try_from(index).expect("a handful of runs") + 1;
        assert!(view.apply_item(reply_in(run.id.as_str(), position), None));
        view.remember_run(run);
    }
    view
}

#[test]
fn a_continuation_started_in_the_same_millisecond_carries_on_the_stopped_turn() {
    let view = thread_with(vec![
        run("run-a", RunStatus::Cancelled, 1_000, false),
        run("run-b", RunStatus::Running, 1_000, true),
    ]);

    assert_eq!(
        turn_of(&view, None, &RunId("run-b".into())),
        RunId("run-a".into())
    );
    let folded = fold(&view, None, None);
    assert!(
        folded.runs[&RunId("run-a".into())].live(),
        "the stopped turn shows its continuation running, not Stopped"
    );
}

#[test]
fn the_latest_of_two_same_millisecond_continuations_sets_the_turn_status() {
    for _ in 0..32 {
        let view = thread_with(vec![
            run("run-a", RunStatus::Cancelled, 1_000, false),
            run("run-b", RunStatus::Cancelled, 2_000, true),
            run("run-c", RunStatus::Running, 2_000, true),
        ]);

        let folded = fold(&view, None, None);
        for id in ["run-a", "run-b", "run-c"] {
            assert!(
                folded.runs[&RunId(id.into())].live(),
                "{id} belongs to a turn whose latest continuation is running"
            );
        }
    }
}

#[test]
fn a_finished_turn_ends_the_continuation_chain() {
    let view = thread_with(vec![
        run("run-a", RunStatus::Cancelled, 1_000, false),
        run("run-b", RunStatus::Cancelled, 2_000, true),
        run("run-c", RunStatus::Done, 3_000, true),
        run("run-d", RunStatus::Running, 4_000, true),
    ]);
    let turns = super::Turns::new(&view, None);
    for (id, root) in [
        ("run-a", "run-a"),
        ("run-b", "run-a"),
        ("run-c", "run-a"),
        ("run-d", "run-d"),
    ] {
        let id = RunId(id.into());
        assert_eq!(turns.of(&id), RunId(root.into()), "{id}");
        assert_eq!(turn_of(&view, None, &id), RunId(root.into()), "{id}");
    }
}

fn prompt_in(run: &str, position: u64) -> Item {
    Item {
        id: ItemId(format!("prompt-{position}")),
        kind: ItemKind::UserMessage {
            text: format!("Prompt {position}.").into(),
            images: Vec::new(),
            steer: false,
            from_agent: None,
            sender: None,
        },
        ..reply_in(run, position)
    }
}

#[test]
fn every_turn_without_tools_gets_its_header_right_after_its_prompt() {
    let (mut view, _) = ThreadView::open(ThreadId("thread".into()));
    for (index, id) in ["run-a", "run-b", "run-c"].into_iter().enumerate() {
        let position = u64::try_from(index).expect("three turns") * 2 + 1;
        assert!(view.apply_item(prompt_in(id, position), None));
        assert!(view.apply_item(reply_in(id, position + 1), None));
        view.remember_run(run(
            id,
            RunStatus::Done,
            1_000 * (position + 1).cast_signed(),
            false,
        ));
    }
    let rows = fold(&view, None, None).rows;
    let shape = rows
        .iter()
        .map(|row| match row {
            super::Row::WorkHeader(run) => format!("header {run}"),
            other => other.anchor().map_or("-", |id| id.as_str()).to_owned(),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        shape,
        [
            "prompt-1",
            "header run-a",
            "reply-2",
            "prompt-3",
            "header run-b",
            "reply-4",
            "prompt-5",
            "header run-c",
            "reply-6",
        ]
    );
}
