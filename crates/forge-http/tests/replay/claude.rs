use super::{on_this_host, replaying};
use artisan_api::{EngineId, Event, Permissions, RunStatus};

#[tokio::test(flavor = "multi_thread")]
async fn a_claude_reply_reaches_the_client_and_a_resumed_stream_shows_the_same_thread() {
    let replay = replaying(EngineId::Claude, "reply").await;
    let thread = replay.thread(Permissions::Auto).await;
    let since = replay.head().await;
    replay.send(&thread, "Say hello").await;
    let screen = replay.settled(since, &thread, 1).await;
    assert_eq!(screen.runs(&thread), [(RunStatus::Done, None)]);
    let shown = on_this_host(&[
        "you: Say hello (Done)",
        "assistant: Hello from the replay. (Done)",
    ]);
    assert_eq!(screen.transcript(&thread), shown);
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
        on_this_host(&[
            "you: Create a file (Done)",
            "error: Claude Bash omitted a valid command (Done)",
            "assistant: The command was refused, so nothing was created. (Done)",
        ])
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
        on_this_host(&[
            "you: Refactor the parser (Done)",
            "assistant: Picked the parser refactor back up. (Done)",
        ]),
        "the continued turn says once that it isn't sandboxed"
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
