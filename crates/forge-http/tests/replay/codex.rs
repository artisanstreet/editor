use super::replaying;
use artisan_api::{Answer, Decision, EngineId, ItemKind, Permissions, RunStatus, Status};

#[tokio::test(flavor = "multi_thread")]
async fn a_codex_command_approval_answered_by_the_user_reaches_codex_and_the_turn_finishes() {
    let replay = replaying(EngineId::Codex, "approval").await;
    let thread = replay.thread(Permissions::Auto).await;
    let since = replay.head().await;
    replay.send(&thread, "Touch a.txt twice").await;
    let asking = replay
        .events(since, "Codex to ask before running the command", |screen| {
            screen
                .item(&thread, |item| {
                    item.status == Status::AwaitingUser
                        && matches!(item.kind, ItemKind::Tool { .. })
                })
                .is_some()
                || screen.finished(&thread) > 0
        })
        .await;
    let gated = asking
        .item(&thread, |item| item.status == Status::AwaitingUser)
        .unwrap_or_else(|| {
            panic!(
                "the command waits for the user, but the run ended: {:?}",
                asking.runs(&thread)
            )
        })
        .id
        .clone();
    replay
        .answer(
            &gated,
            &Answer::Approval {
                decision: Decision::AllowAlways,
            },
        )
        .await;
    let screen = replay.settled(since, &thread, 1).await;
    assert_eq!(screen.runs(&thread), [(RunStatus::Done, None)]);
    assert_eq!(
        screen.transcript(&thread),
        [
            "you: Touch a.txt twice (Done)",
            "assistant: I’ll run the two requested shell calls separately, requesting escalation for each. (Done)",
            "shell: touch a.txt (Done)",
            "shell: touch a.txt (Done)",
            "assistant: done (Done)",
        ]
    );
}
