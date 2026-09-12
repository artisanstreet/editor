//! Finite Codex rich-activity fixture stream (visual parity, owner side).
//!
//! NOTE: registration is owned by root (not yet wired into
//! `modules/backend/src/engine_owner/mod.rs` or `BUILD.bazel`): until root
//! adds the `#[path]` module plus the `tests/backend` export and the backend
//! `srcs` entry, this file does not compile into any target. It is written
//! as a sibling of `engine_owner_codex.rs`, so `super::codex` and
//! `super::observation` resolve exactly like that file once registered.
//!
//! The test below replays one ordered wire stream mixing every supported
//! activity shape with the plain delta path: reasoning summary, tool
//! begin/progress/complete, command start/output/complete, file change,
//! search, plan, one foreign-turn frame that must vanish, and the terminal
//! turn event. It proves source-order emission, stable generated ids, and an
//! intact terminal without touching the dispatcher `Activity` arm (separate
//! packet) or any native gate.

use tokio::sync::mpsc;

use super::codex::{CodexPendingTracker, apply_event, parse_frame};
use super::observation::{EngineObservation, TerminalState};
use artisan_domain::{Observation, RunId};

fn activity_run_id() -> RunId {
    RunId::parse("codex-activity-fixture").expect("run id")
}

/// Builds one method notification line from a params object literal.
fn notification(method: &str, params: &str) -> String {
    format!(r#"{{"method":"{method}","params":{params}}}"#)
}

#[expect(
    clippy::too_many_lines,
    reason = "single linear fixture body; extraction would duplicate the shared test wiring"
)]
#[tokio::test]
async fn fixture_activity_stream_emits_in_source_order_with_stable_ids() {
    let lines = [
        notification(
            "turn/started",
            r#"{"threadId":"t-fixture","turn":{"id":"turn-fixture","status":"inProgress"}}"#,
        ),
        notification(
            "item/agentMessage/delta",
            r#"{"delta":"hello ","itemId":"msg-1","threadId":"t-fixture","turnId":"turn-fixture"}"#,
        ),
        notification(
            "item/reasoning/summaryTextDelta",
            r#"{"delta":"checking the plan","itemId":"rsn-1","summaryIndex":0,"threadId":"t-fixture","turnId":"turn-fixture"}"#,
        ),
        // A foreign turn interleaves mid-stream and must vanish entirely.
        notification(
            "item/reasoning/summaryTextDelta",
            r#"{"delta":"elsewhere","itemId":"rsn-9","summaryIndex":0,"threadId":"t-fixture","turnId":"turn-foreign"}"#,
        ),
        notification(
            "item/started",
            r#"{"threadId":"t-fixture","turnId":"turn-fixture","item":{"id":"tool-1","server":"fs","status":"inProgress","tool":"read","type":"mcpToolCall"}}"#,
        ),
        notification(
            "item/mcpToolCall/progress",
            r#"{"itemId":"tool-1","message":"reading","threadId":"t-fixture","turnId":"turn-fixture"}"#,
        ),
        notification(
            "item/completed",
            r#"{"threadId":"t-fixture","turnId":"turn-fixture","item":{"id":"tool-1","server":"fs","status":"completed","tool":"read","type":"mcpToolCall"}}"#,
        ),
        notification(
            "item/started",
            r#"{"threadId":"t-fixture","turnId":"turn-fixture","item":{"command":"ls","id":"cmd-1","status":"inProgress","type":"commandExecution"}}"#,
        ),
        notification(
            "item/commandExecution/outputDelta",
            r#"{"delta":"a\nb\n","itemId":"cmd-1","threadId":"t-fixture","turnId":"turn-fixture"}"#,
        ),
        notification(
            "item/completed",
            r#"{"threadId":"t-fixture","turnId":"turn-fixture","item":{"aggregatedOutput":"a\nb\n","command":"ls","exitCode":0,"id":"cmd-1","status":"completed","type":"commandExecution"}}"#,
        ),
        notification(
            "item/completed",
            r#"{"threadId":"t-fixture","turnId":"turn-fixture","item":{"changes":[{"diff":"+++ added\n","kind":{"type":"add"},"path":"note.txt"}],"id":"file-1","status":"completed","type":"fileChange"}}"#,
        ),
        notification(
            "item/completed",
            r#"{"threadId":"t-fixture","turnId":"turn-fixture","item":{"id":"search-1","query":"codex parity","type":"webSearch"}}"#,
        ),
        notification(
            "turn/plan/updated",
            r#"{"explanation":null,"plan":[{"status":"completed","step":"survey"},{"status":"inProgress","step":"implement"}],"threadId":"t-fixture","turnId":"turn-fixture"}"#,
        ),
        notification(
            "item/completed",
            r#"{"threadId":"t-fixture","turnId":"turn-fixture","item":{"id":"rsn-1","summary":["checked the plan"],"type":"reasoning"}}"#,
        ),
        notification(
            "turn/completed",
            r#"{"threadId":"t-fixture","turn":{"id":"turn-fixture","status":"completed"}}"#,
        ),
    ];

    let run = activity_run_id();
    let (sender, mut receiver) = mpsc::channel(128);
    let mut tracker = CodexPendingTracker::new();
    // Root thread authority, mirroring the production pump binding from
    // thread/start before the turn/start wait.
    tracker.bind_native_thread("t-fixture");
    let mut active: Option<String> = None;
    let mut terminal = None;
    let mut sequence = 0u64;
    for line in &lines {
        sequence += 1;
        let event = parse_frame(line, sequence).expect("fixture frame decodes");
        if let Some(state) = apply_event(
            event,
            &run,
            &mut tracker,
            &mut active,
            &sender,
            sequence,
            None,
        )
        .await
        {
            terminal = Some(state);
        }
    }
    drop(sender);

    let mut tags = Vec::new();
    let mut activity_sequences = Vec::new();
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut stable_ids = Vec::new();
    while let Ok(observation) = receiver.try_recv() {
        match observation {
            EngineObservation::TextDelta(delta) => {
                tags.push("text");
                text.push_str(delta.delta());
            }
            EngineObservation::Activity(row) => {
                let tag = match &row {
                    Observation::ReasoningSummaryDelta(delta) => {
                        reasoning.push_str(delta.delta());
                        "reasoning_summary_delta"
                    }
                    Observation::ReasoningSummaryCompleted(_) => "reasoning_summary_completed",
                    Observation::Tool(_) => "tool",
                    Observation::TerminalActivity(_) => "terminal_activity",
                    Observation::File(_) => "file",
                    Observation::Search(_) => "search",
                    Observation::Plan(_) => "plan",
                    other => panic!("fixture emits only activity rows, got {}", other.tag()),
                };
                tags.push(tag);
                activity_sequences.push(row.sequence().get());
                stable_ids.push(row.observation_id().as_str().to_owned());
            }
            EngineObservation::Terminal(_) => panic!("terminal travels as state, not a row"),
            EngineObservation::TextSnapshot(_)
            | EngineObservation::Usage(_)
            | EngineObservation::Subagent(_)
            | EngineObservation::SubagentTranscript(_) => {
                panic!("fixture emits no other owner rows")
            }
        }
    }

    assert_eq!(terminal, Some(TerminalState::Completed));
    assert_eq!(text, "hello ", "plain delta path untouched");
    assert_eq!(
        reasoning, "checking the plan",
        "foreign turn text never lands"
    );
    assert_eq!(
        tags,
        vec![
            "text",
            "reasoning_summary_delta",
            "tool",
            "tool",
            "tool",
            "terminal_activity",
            "terminal_activity",
            "terminal_activity",
            "file",
            "search",
            "plan",
            "reasoning_summary_completed",
        ],
        "every supported shape emits in source order"
    );
    assert!(
        activity_sequences.windows(2).all(|pair| pair[0] < pair[1]),
        "source-local sequences increase monotonically: {activity_sequences:?}"
    );
    let mut deduped = stable_ids.clone();
    deduped.sort();
    deduped.dedup();
    assert_eq!(
        stable_ids.len(),
        deduped.len(),
        "generated observation ids never collide within the stream"
    );
    assert!(
        stable_ids
            .iter()
            .all(|id| id.starts_with("codex-activity-fixture:codex:")),
        "ids carry the stable run-scoped prefix: {stable_ids:?}"
    );
}
