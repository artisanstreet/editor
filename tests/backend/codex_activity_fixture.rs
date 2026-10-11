//! Finite Codex rich-activity fixture stream (visual parity, owner side).
//!
//! NOTE: registration is owned by root (not yet wired into
//! `modules/backend/src/engine_owner/mod.rs` or `Cargo.toml`): until root
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
use super::observation::{EngineObservation, ShellJobChange, TerminalState};
use artisan_domain::{Observation, ObservationId, RunId};

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
            // Command output streams to the terminal board, never a row.
            EngineObservation::Shell(row) => tags.push(match row.change {
                ShellJobChange::Opened { .. } => "shell_opened",
                ShellJobChange::Backgrounded => "shell_backgrounded",
                ShellJobChange::Output(_) => "shell_output",
                ShellJobChange::Settled(_) => "shell_settled",
            }),
            EngineObservation::Terminal(_) => panic!("terminal travels as state, not a row"),
            EngineObservation::SummaryTitle { .. }
            | EngineObservation::TextSnapshot(_)
            | EngineObservation::Usage(_)
            | EngineObservation::Subagent(_)
            | EngineObservation::SubagentTranscript(_)
            | EngineObservation::Inventory(_) => {
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
            "shell_opened",
            "terminal_activity",
            "shell_output",
            "shell_settled",
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

// ---------------------------------------------------------------------------
// Context compaction
// ---------------------------------------------------------------------------

/// Replays `lines` against one bound tracker; returns every compaction row
/// and the per-frame terminal outcomes.
async fn replay_compaction(
    lines: &[String],
) -> (
    Vec<artisan_domain::CompactionObservation>,
    Vec<Option<TerminalState>>,
) {
    let run = activity_run_id();
    let (sender, mut receiver) = mpsc::channel(64);
    let mut tracker = CodexPendingTracker::new();
    tracker.bind_native_thread("t-fixture");
    let mut active: Option<String> = None;
    let mut terminals = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let sequence = u64::try_from(index).expect("small fixture") + 1;
        let event = parse_frame(line, sequence).expect("fixture frame decodes");
        terminals.push(
            apply_event(
                event,
                &run,
                &mut tracker,
                &mut active,
                &sender,
                sequence,
                None,
            )
            .await,
        );
    }
    drop(sender);
    let mut rows = Vec::new();
    while let Ok(observation) = receiver.try_recv() {
        match observation {
            EngineObservation::Activity(Observation::Compaction(row)) => rows.push(row),
            EngineObservation::TextDelta(_) => {}
            _ => panic!("compaction fixtures emit only compaction rows and text"),
        }
    }
    (rows, terminals)
}

fn compaction_item(method: &str, item_id: &str, stamp: &str) -> String {
    notification(
        method,
        &format!(
            r#"{{"threadId":"t-fixture","turnId":"turn-fixture",{stamp}"item":{{"id":"{item_id}","type":"contextCompaction"}}}}"#
        ),
    )
}

fn thread_compacted(turn_id: &str) -> String {
    notification(
        "thread/compacted",
        &format!(r#"{{"threadId":"t-fixture","turnId":"{turn_id}"}}"#),
    )
}

fn turn_started() -> String {
    notification(
        "turn/started",
        r#"{"threadId":"t-fixture","turn":{"id":"turn-fixture","status":"inProgress"}}"#,
    )
}

#[tokio::test]
async fn compaction_item_pair_shares_compaction_id_and_never_settles_the_turn() {
    use artisan_domain::CompactionState;

    let lines = [
        turn_started(),
        compaction_item("item/started", "cmp-1", r#""startedAtMs":1000,"#),
        compaction_item("item/completed", "cmp-1", r#""completedAtMs":4500,"#),
        // The deprecated echo of the same compaction must not settle it again.
        thread_compacted("turn-fixture"),
        notification(
            "item/agentMessage/delta",
            r#"{"delta":"after","itemId":"msg-1","threadId":"t-fixture","turnId":"turn-fixture"}"#,
        ),
        notification(
            "turn/completed",
            r#"{"threadId":"t-fixture","turn":{"id":"turn-fixture","status":"completed"}}"#,
        ),
    ];
    let (rows, terminals) = replay_compaction(&lines).await;

    assert!(
        terminals[..terminals.len() - 1].iter().all(Option::is_none),
        "compaction frames never settle the turn: {terminals:?}"
    );
    assert_eq!(terminals.last(), Some(&Some(TerminalState::Completed)));
    let [started, completed] = rows.as_slice() else {
        panic!("exactly one started and one completed row, got {rows:?}");
    };
    assert_eq!(started.state(), CompactionState::Started);
    assert_eq!(completed.state(), CompactionState::Completed);
    assert_eq!(
        started.compaction_id().map(ObservationId::as_str),
        Some("codex-activity-fixture:codex:compaction:cmp-1")
    );
    assert_eq!(started.compaction_id(), completed.compaction_id());
    assert_ne!(started.id(), completed.id());
    assert_eq!(
        started.id().as_str(),
        "codex-activity-fixture:codex:2:compaction:cmp-1:started"
    );
    assert_eq!(
        completed.id().as_str(),
        "codex-activity-fixture:codex:3:compaction:cmp-1:completed"
    );
    assert_eq!(started.duration_ms(), None);
    assert_eq!(completed.duration_ms(), Some(3500));
    assert_eq!(started.summary(), None);
    assert_eq!(completed.summary(), None, "Codex reports no token counts");
}

#[tokio::test]
async fn compaction_completion_only_emits_one_completed_with_its_own_id() {
    use artisan_domain::CompactionState;

    // Only an item completion arrives.
    let (rows, _) = replay_compaction(&[
        turn_started(),
        compaction_item("item/completed", "cmp-2", r#""completedAtMs":4500,"#),
    ])
    .await;
    let [completed] = rows.as_slice() else {
        panic!("exactly one completed row, got {rows:?}");
    };
    assert_eq!(completed.state(), CompactionState::Completed);
    assert_eq!(
        completed.compaction_id().map(ObservationId::as_str),
        Some("codex-activity-fixture:codex:compaction:cmp-2")
    );
    assert_eq!(completed.duration_ms(), None, "no start, no duration");

    // Only the deprecated notification arrives (older servers).
    let (rows, terminals) =
        replay_compaction(&[turn_started(), thread_compacted("turn-fixture")]).await;
    assert!(terminals.iter().all(Option::is_none));
    let [completed] = rows.as_slice() else {
        panic!("exactly one completed row, got {rows:?}");
    };
    assert_eq!(completed.state(), CompactionState::Completed);
    assert_eq!(
        completed.compaction_id().map(ObservationId::as_str),
        Some("codex-activity-fixture:codex:compaction:2")
    );
    assert_eq!(
        completed.id().as_str(),
        "codex-activity-fixture:codex:2:compaction:completed"
    );
}

#[tokio::test]
async fn thread_compacted_closes_an_open_item_without_double_completion() {
    use artisan_domain::CompactionState;

    let (rows, _) = replay_compaction(&[
        turn_started(),
        compaction_item("item/started", "cmp-3", r#""startedAtMs":1000,"#),
        // The legacy echo arrives before the item completion.
        thread_compacted("turn-fixture"),
        compaction_item("item/completed", "cmp-3", r#""completedAtMs":4500,"#),
        // A foreign turn's compaction never lands on this turn.
        thread_compacted("turn-foreign"),
    ])
    .await;
    let [started, completed] = rows.as_slice() else {
        panic!("exactly one started and one completed row, got {rows:?}");
    };
    assert_eq!(started.state(), CompactionState::Started);
    assert_eq!(completed.state(), CompactionState::Completed);
    assert_eq!(started.compaction_id(), completed.compaction_id());
    assert_eq!(
        completed.id().as_str(),
        "codex-activity-fixture:codex:3:compaction:cmp-3:completed"
    );
}

#[test]
fn compaction_frames_decode_to_compaction_events() {
    use super::codex::{CodexCompactionSignal, CodexEvent};

    let started = parse_frame(
        &compaction_item("item/started", "cmp-4", r#""startedAtMs":7,"#),
        1,
    )
    .expect("frame decodes");
    assert_eq!(
        started,
        CodexEvent::Compaction {
            thread_id: "t-fixture".to_owned(),
            turn_id: "turn-fixture".to_owned(),
            signal: CodexCompactionSignal::ItemStarted {
                item_id: "cmp-4".to_owned(),
                started_at_ms: Some(7),
            },
        }
    );
    let legacy = parse_frame(&thread_compacted("turn-fixture"), 2).expect("frame decodes");
    assert!(matches!(
        legacy,
        CodexEvent::Compaction {
            signal: CodexCompactionSignal::ThreadCompacted,
            ..
        }
    ));
    // Scope-free or id-free frames stay observable without inventing scope.
    for line in [
        notification("thread/compacted", r#"{"threadId":"t-fixture"}"#),
        compaction_item("item/completed", "", ""),
    ] {
        assert!(matches!(
            parse_frame(&line, 3).expect("frame decodes"),
            CodexEvent::UnknownMethod
        ));
    }
}

#[test]
fn image_items_decode_to_shown_images() {
    use super::codex::{CodexEvent, CodexImage};

    // Looking at an image is a step of the work: it shows no image.
    let viewed = parse_frame(
        &notification(
            "item/completed",
            r#"{"threadId":"t-fixture","turnId":"turn-fixture","item":{"id":"view-1","path":"/work/shot.png","type":"imageView"}}"#,
        ),
        1,
    )
    .expect("frame decodes");
    assert_eq!(viewed, CodexEvent::ActivitySilent);
    let generated = parse_frame(
        &notification(
            "item/completed",
            r#"{"threadId":"t-fixture","turnId":"turn-fixture","item":{"id":"gen-1","status":"completed","revisedPrompt":"a lighthouse at dusk","result":"aGk=","failure":null,"savedPath":"/home/u/.codex/images/gen-1.png","type":"imageGeneration"}}"#,
        ),
        2,
    )
    .expect("frame decodes");
    assert_eq!(
        generated,
        CodexEvent::ImageShown {
            thread_id: "t-fixture".to_owned(),
            turn_id: "turn-fixture".to_owned(),
            item_id: "gen-1".to_owned(),
            image: CodexImage {
                saved_path: Some("/home/u/.codex/images/gen-1.png".to_owned()),
                base64: Some("aGk=".to_owned()),
                revised_prompt: Some("a lighthouse at dusk".to_owned()),
            },
        }
    );
    // Failed or empty generations and started items show nothing.
    for (method, item) in [
        (
            "item/completed",
            r#"{"id":"gen-2","status":"failed","revisedPrompt":null,"result":"","failure":{"type":"usageLimitExceeded","limitId":"l","resetsAt":null},"type":"imageGeneration"}"#,
        ),
        (
            "item/completed",
            r#"{"id":"gen-3","status":"completed","revisedPrompt":null,"result":"","failure":null,"type":"imageGeneration"}"#,
        ),
        (
            "item/started",
            r#"{"id":"gen-4","status":"in_progress","revisedPrompt":null,"result":"","failure":null,"type":"imageGeneration"}"#,
        ),
    ] {
        let line = notification(
            method,
            &format!(r#"{{"threadId":"t-fixture","turnId":"turn-fixture","item":{item}}}"#),
        );
        assert_eq!(
            parse_frame(&line, 3).expect("frame decodes"),
            CodexEvent::ActivitySilent
        );
    }
}

#[tokio::test]
async fn generated_inline_image_is_stored_and_observed_by_path() {
    let store = super::engine_image::use_test_state_root();
    let run = activity_run_id();
    let (sender, mut receiver) = mpsc::channel(8);
    let mut tracker = CodexPendingTracker::new();
    tracker.bind_native_thread("t-fixture");
    let mut active: Option<String> = None;
    let line = notification(
        "item/completed",
        &format!(
            r#"{{"threadId":"t-fixture","turnId":"turn-fixture","item":{{"id":"gen-5","status":"completed","revisedPrompt":"one pixel","result":"{}","failure":null,"type":"imageGeneration"}}}}"#,
            super::engine_image::PIXEL_PNG_BASE64
        ),
    );
    let event = parse_frame(&line, 4).expect("frame decodes");
    assert!(
        apply_event(event, &run, &mut tracker, &mut active, &sender, 4, None)
            .await
            .is_none()
    );
    drop(sender);
    let Some(EngineObservation::Activity(Observation::Image(image))) = receiver.recv().await else {
        panic!("a generated image must emit one image row");
    };
    assert_eq!(image.origin(), artisan_domain::ImageOrigin::Generated);
    assert_eq!(image.item_id().as_str(), "gen-5");
    assert_eq!(image.caption(), Some("one pixel"));
    let path = std::path::Path::new(image.path());
    assert_eq!(path.parent(), Some(store));
    assert!(path.is_file(), "the inline image is written to the store");
    assert!(receiver.recv().await.is_none());
}

/// Replays `lines` against a tracker bound to `t-fixture`, returning every
/// emitted observation and each frame's terminal outcome.
async fn replay(lines: &[String]) -> (Vec<EngineObservation>, Vec<Option<TerminalState>>) {
    let run = activity_run_id();
    let (sender, mut receiver) = mpsc::channel(128);
    let mut tracker = CodexPendingTracker::new();
    tracker.bind_native_thread("t-fixture");
    let mut active: Option<String> = None;
    let mut terminals = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let sequence = index as u64 + 1;
        let event = parse_frame(line, sequence).expect("fixture frame decodes");
        terminals.push(
            apply_event(
                event,
                &run,
                &mut tracker,
                &mut active,
                &sender,
                sequence,
                None,
            )
            .await,
        );
    }
    drop(sender);
    let mut observations = Vec::new();
    while let Ok(observation) = receiver.try_recv() {
        observations.push(observation);
    }
    (observations, terminals)
}

/// The subagent rows among `observations`: thread, name and action.
fn agent_rows(observations: &[EngineObservation]) -> Vec<(String, Option<String>, &'static str)> {
    observations
        .iter()
        .filter_map(|observation| match observation {
            EngineObservation::Activity(Observation::Tool(row))
                if row.tool_name() == "subagent" =>
            {
                Some((
                    row.tool_id().as_str().to_owned(),
                    row.detail().map(str::to_owned),
                    row.action().as_str(),
                ))
            }
            _ => None,
        })
        .collect()
}

fn root_text(observations: &[EngineObservation]) -> String {
    observations
        .iter()
        .filter_map(|observation| match observation {
            EngineObservation::TextDelta(delta) => Some(delta.delta()),
            _ => None,
        })
        .collect()
}

fn spawn_call(method: &str, status: &str, receivers: &str, agent_states: &str) -> String {
    notification(
        method,
        &format!(
            r#"{{"threadId":"t-fixture","turnId":"turn-fixture","item":{{"type":"collabAgentToolCall","id":"call-1","tool":"spawnAgent","status":"{status}","senderThreadId":"t-fixture","receiverThreadIds":[{receivers}],"prompt":"Build the R700","agentsStates":{{{agent_states}}}}}}}"#
        ),
    )
}

fn child_started(thread: &str, parent: &str, nickname: &str) -> String {
    notification(
        "thread/started",
        &format!(
            r#"{{"thread":{{"id":"{thread}","parentThreadId":"{parent}","agentNickname":"{nickname}"}}}}"#
        ),
    )
}

#[tokio::test]
async fn subagent_threads_show_as_agents_and_never_speak_for_the_root() {
    let lines = [
        turn_started(),
        spawn_call("item/started", "inProgress", "", ""),
        child_started("child-1", "t-fixture", "Ampere"),
        spawn_call(
            "item/completed",
            "completed",
            r#""child-1""#,
            r#""child-1":{"status":"pendingInit"}"#,
        ),
        notification(
            "turn/started",
            r#"{"threadId":"child-1","turn":{"id":"turn-child","status":"inProgress"}}"#,
        ),
        notification(
            "item/agentMessage/delta",
            r#"{"delta":"I own only the R700 files.","itemId":"msg-c","threadId":"child-1","turnId":"turn-child"}"#,
        ),
        notification(
            "thread/tokenUsage/updated",
            r#"{"threadId":"child-1","turnId":"turn-child","tokenUsage":{"total":{"inputTokens":40,"outputTokens":9},"last":{"totalTokens":49},"modelContextWindow":200000}}"#,
        ),
        notification(
            "item/agentMessage/delta",
            r#"{"delta":"The agent is working.","itemId":"msg-1","threadId":"t-fixture","turnId":"turn-fixture"}"#,
        ),
        notification(
            "turn/completed",
            r#"{"threadId":"child-1","turn":{"id":"turn-child","status":"completed"}}"#,
        ),
        notification(
            "turn/completed",
            r#"{"threadId":"t-fixture","turn":{"id":"turn-fixture","status":"completed"}}"#,
        ),
    ];
    let (observations, terminals) = replay(&lines).await;

    assert!(
        terminals[..terminals.len() - 1].iter().all(Option::is_none),
        "a subagent's turn never settles the root turn: {terminals:?}"
    );
    assert_eq!(terminals.last(), Some(&Some(TerminalState::Completed)));
    assert_eq!(root_text(&observations), "The agent is working.");
    assert!(
        !observations
            .iter()
            .any(|observation| matches!(observation, EngineObservation::Usage(_))),
        "a subagent's usage is not the root's"
    );
    let ampere = Some("Ampere".to_owned());
    assert_eq!(
        agent_rows(&observations),
        [
            ("child-1".to_owned(), ampere.clone(), "started"),
            ("child-1".to_owned(), ampere.clone(), "progress"),
            ("child-1".to_owned(), ampere, "completed"),
        ]
    );
}

#[tokio::test]
async fn a_late_agent_name_renames_its_row_and_nested_agents_stay_hidden() {
    let lines = [
        turn_started(),
        spawn_call("item/completed", "completed", r#""child-1""#, ""),
        child_started("child-1", "t-fixture", "Ampere"),
        child_started("grandchild-1", "child-1", "Noether"),
        notification(
            "turn/completed",
            r#"{"threadId":"grandchild-1","turn":{"id":"turn-grandchild","status":"failed"}}"#,
        ),
        notification(
            "item/completed",
            r#"{"threadId":"t-fixture","turnId":"turn-fixture","item":{"type":"collabAgentToolCall","id":"call-2","tool":"wait","status":"completed","senderThreadId":"t-fixture","receiverThreadIds":["child-1"],"agentsStates":{"child-1":{"status":"errored","message":"boom"}}}}"#,
        ),
    ];
    let (observations, terminals) = replay(&lines).await;

    assert!(terminals.iter().all(Option::is_none), "{terminals:?}");
    assert_eq!(
        agent_rows(&observations),
        [
            ("child-1".to_owned(), None, "started"),
            ("child-1".to_owned(), Some("Ampere".to_owned()), "started"),
            ("child-1".to_owned(), Some("Ampere".to_owned()), "failed"),
        ]
    );
}
