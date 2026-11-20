use super::replaying;
use artisan_api::{EngineId, Event, ItemKind, Permissions, RunStatus, Status};

#[tokio::test(flavor = "multi_thread")]
async fn a_claude_reply_reaches_the_client_and_a_resumed_stream_shows_the_same_thread() {
    let replay = replaying(EngineId::Claude, "reply").await;
    let thread = replay.thread(Permissions::Auto).await;
    let since = replay.head().await;
    replay.send(&thread, "Say hello").await;
    let screen = replay.settled(since, &thread, 1).await;
    assert_eq!(screen.runs(&thread), [(RunStatus::Done, None)]);
    let shown = [
        "you: Say hello (Done)",
        "assistant: Hello from the replay. (Done)",
    ];
    assert_eq!(screen.transcript(&thread), shown);
    let off_clock = screen.off_the_replay_clock();
    assert!(
        off_clock.is_empty(),
        "Forge stamps the replayed turn with the replay's clock, not {off_clock:?}"
    );
    let seqs: Vec<_> = screen
        .envelopes()
        .iter()
        .filter_map(|envelope| envelope.seq)
        .collect();
    assert!(
        seqs.windows(2).all(|pair| pair[0] < pair[1]),
        "stored changes arrive in seq order: {seqs:?}"
    );

    let resumed = replay
        .events(since, "the same thread after reconnecting", |screen| {
            screen.transcript(&thread) == shown && screen.finished(&thread) == 1
        })
        .await;
    assert_eq!(resumed.runs(&thread), [(RunStatus::Done, None)]);

    let ahead = replay.head().await + 1_000;
    let reset = replay
        .events(ahead, "a reset for a cursor past Forge's head", |screen| {
            screen
                .envelopes()
                .iter()
                .any(|envelope| envelope.event == Event::Reset)
        })
        .await;
    assert!(
        matches!(
            reset.envelopes().first().map(|envelope| &envelope.event),
            Some(Event::Hello { .. })
        ),
        "a stream opens with Hello"
    );
    assert_eq!(replay.launches(), 1, "one turn runs the engine once");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_malformed_claude_permission_request_is_refused_and_the_turn_finishes() {
    let replay = replaying(EngineId::Claude, "malformed-permission").await;
    let thread = replay.thread(Permissions::Auto).await;
    let since = replay.head().await;
    replay.send(&thread, "Create a file").await;
    let screen = replay.settled(since, &thread, 1).await;
    assert_eq!(screen.runs(&thread), [(RunStatus::Done, None)]);
    assert_eq!(
        screen.transcript(&thread),
        [
            "you: Create a file (Done)",
            "error: Claude Bash omitted a valid command (Done)",
            "assistant: The command was refused, so nothing was created. (Done)",
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn continuing_a_stopped_claude_turn_does_not_send_its_delivered_message_again() {
    let replay = replaying(EngineId::Claude, "stop-before-acknowledgement").await;
    let thread = replay.thread(Permissions::Auto).await;
    let since = replay.head().await;
    replay.send(&thread, "Refactor the parser").await;
    replay
        .events(since, "Claude to start the session", |screen| {
            screen.started(&thread)
        })
        .await;
    replay.stop(&thread).await;
    let stopped = replay.settled(since, &thread, 1).await;
    assert_eq!(stopped.runs(&thread), [(RunStatus::Cancelled, None)]);

    replay.carry_on(&thread).await;
    let screen = replay.settled(since, &thread, 2).await;
    assert_eq!(
        screen.runs(&thread),
        [(RunStatus::Cancelled, None), (RunStatus::Done, None)],
        "the continued turn takes up the message Claude already had"
    );
    assert_eq!(
        screen.transcript(&thread),
        [
            "you: Refactor the parser (Done)",
            "assistant: Picked the parser refactor back up. (Done)",
        ]
    );
    assert_eq!(replay.launches(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claude_turn_leaves_a_complete_trace_of_its_frames() {
    let replay = replaying(EngineId::Claude, "reply").await;
    let thread = replay.thread(Permissions::Auto).await;
    let since = replay.head().await;
    replay.send(&thread, "Say hello").await;
    replay.settled(since, &thread, 1).await;
    let trace = replay.trace(&thread).await;
    let frames = |kind: &str, text: &str| {
        trace.iter().any(|record| {
            record["kind"] == kind
                && record["text"]
                    .as_str()
                    .is_some_and(|frame| frame.contains(text))
        })
    };
    assert!(frames("in", "Say hello"), "the frame Forge sent: {trace:?}");
    assert!(
        frames("out", "Hello from the replay."),
        "the frame Claude wrote: {trace:?}"
    );
    let ends: Vec<_> = trace
        .iter()
        .filter(|record| record["kind"] == "end")
        .collect();
    assert_eq!(ends.len(), 1, "{trace:?}");
    assert_eq!(ends[0]["dropped"], 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_background_agent_spawned_through_the_gateway_reports_back_to_its_parent() {
    let replay = replaying(EngineId::Claude, "background-agent").await;
    let report = replay.gate("report");
    let thread = replay.thread(Permissions::Auto).await;
    let since = replay.head().await;
    replay
        .send(&thread, "Have an agent review the parser")
        .await;
    let spawned = replay.settled(since, &thread, 1).await;
    assert_eq!(spawned.runs(&thread), [(RunStatus::Done, None)]);
    let agent = spawned
        .item(&thread, |item| matches!(item.kind, ItemKind::Agent { .. }))
        .expect("the parent shows the agent it started")
        .clone();
    let ItemKind::Agent {
        thread: Some(child),
        background: true,
        ..
    } = &agent.kind
    else {
        panic!("the agent runs in the background on its own thread: {agent:#?}");
    };
    assert_eq!(agent.status, Status::Running, "the agent is still working");

    report.open();
    let reported = replay.settled(since, &thread, 2).await;

    assert_eq!(
        reported.runs(&thread),
        [(RunStatus::Done, None), (RunStatus::Done, None)],
        "the agent's report starts the parent's next turn"
    );
    assert_eq!(
        reported.runs(child),
        [(RunStatus::Done, None)],
        "the agent's own turn finished"
    );
    assert_eq!(
        reported.transcript(child),
        [
            "you: Review the parser for unhandled input. (Done)",
            "assistant: The parser rejects every malformed input. (Done)",
        ]
    );
    let parent = reported.transcript(&thread);
    assert!(
        parent.contains(&"assistant: The review found nothing to fix. (Done)".to_owned()),
        "the parent answered the report: {parent:#?}"
    );
    let report = reported
        .item(&thread, |item| {
            matches!(
                &item.kind,
                ItemKind::UserMessage {
                    from_agent: Some(_),
                    ..
                }
            )
        })
        .expect("the report arrives as a message from the agent");
    assert!(
        matches!(&report.kind, ItemKind::UserMessage { text, .. }
            if text.inline().is_some_and(|text| text.ends_with("The parser rejects every malformed input."))),
        "{report:#?}"
    );
    assert_eq!(
        reported
            .item(&thread, |item| item.id == agent.id)
            .map(|item| item.status),
        Some(Status::Done),
        "the parent shows the agent finished"
    );
    assert_eq!(replay.launches(), 3, "parent, agent, then the parent again");
}
