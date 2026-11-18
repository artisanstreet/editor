use super::replaying;
use artisan_api::{EngineId, Event, Permissions, RunStatus};

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
