//! Claude thinking display: launch policy, mixed-content decoding,
//! thinking-stretch projection onto the shared reasoning observations,
//! streamed-versus-buffered assistant text settling exactly once, and tool
//! calls projecting as work beside commentary.
//!
//! Replays the sanitized captures in `tests/fixtures/claude/` (see its
//! `manifest.json`; `constructed-*` files are labeled edge fixtures derived
//! from the real transport shape) through the same `parse_frame` /
//! `apply_event` pair the live pump uses. No real `claude` binary.

use artisan_domain::{
    ApprovalMode, AssistantMessagePhase, ClaudeEffort, ClaudeSelection, EngineAgentId,
    EngineModelId, EnginePermissionPolicy, EngineProfileId, FilesystemAccess, NetworkAccess,
    OBSERVATION_DELTA_MAX_BYTES, OBSERVATION_MESSAGE_MAX_BYTES, Observation, PermissionId, RunId,
    ThreadId, WebSearchAccess,
};
use artisan_native_engine::{ClaudeThinkingDisplaySupport, claude_thinking_display_support};
use tokio::sync::mpsc;

use super::claude::{
    ClaudeApplyOutcome, ClaudeAssistantContent, ClaudeEvent, ClaudePendingTracker, ClaudeSession,
    ClaudeSettings, ClaudeThinkingDisplay, ClaudeToolUse, ClaudeUsageScope, apply_event,
    parse_frame,
};
use super::observation::EngineObservation;
use super::operation::ClaudeDisplayRefusals;

const TOOLS: &str = include_str!("../fixtures/claude/summarized-tools.jsonl");
const MIXED: &str = include_str!("../fixtures/claude/summarized-mixed.jsonl");
const START: &str = include_str!("../fixtures/claude/summarized-start.jsonl");
const RESUME: &str = include_str!("../fixtures/claude/summarized-resume.jsonl");
const DEFAULT: &str = include_str!("../fixtures/claude/default-start.jsonl");
const INTERRUPTED: &str = include_str!("../fixtures/claude/interrupted.jsonl");
const TWO_STRETCHES: &str = include_str!("../fixtures/claude/constructed-two-stretches.jsonl");
const INTERRUPTED_THINKING: &str =
    include_str!("../fixtures/claude/constructed-interrupted-thinking.jsonl");
const HIGHLIGHT_TITLES: &str =
    include_str!("../fixtures/claude/constructed-highlight-titles.jsonl");
const HIGHLIGHTS_REFUSED: &str = include_str!("../fixtures/claude/highlights-refused.jsonl");

fn selection() -> ClaudeSelection {
    ClaudeSelection::new(
        EngineProfileId::parse("claude-fixture").expect("profile id"),
        Some(EngineModelId::parse("claude-sonnet-5").expect("model id")),
        EnginePermissionPolicy::new(
            PermissionId::parse("permission-claude").expect("permission id"),
            EngineAgentId::parse("agent-claude").expect("agent id"),
            ApprovalMode::Never,
            FilesystemAccess::Workspace,
            NetworkAccess::Enabled,
            WebSearchAccess::Disabled,
        ),
        Some(ClaudeEffort::High),
        None,
        false,
        false,
    )
    .expect("selection")
}

fn display_pair(args: &[String]) -> Option<&str> {
    let at = args.iter().position(|arg| arg == "--thinking-display")?;
    args.get(at + 1).map(String::as_str)
}

#[test]
fn supported_cli_requests_highlights_on_start_and_resume() {
    let display = ClaudeThinkingDisplay::for_support(claude_thinking_display_support("2.1.282"));
    assert_eq!(
        display,
        ClaudeThinkingDisplay::Highlights,
        "the shipped default is hosted highlights; refusal falls back to prose"
    );
    let settings = ClaudeSettings::from_selection(&selection())
        .expect("settings")
        .with_thinking_display(display);
    let start = settings.spawn_args(&ClaudeSession::Start("session-a".to_owned()));
    let resume = settings.spawn_args(&ClaudeSession::Resume("session-a".to_owned()));
    assert_eq!(display_pair(&start), Some("highlights"));
    assert_eq!(display_pair(&resume), Some("highlights"));
    assert!(resume.contains(&"--resume".to_owned()));
    assert_eq!(
        start
            .iter()
            .filter(|arg| *arg == "--thinking-display")
            .count(),
        1
    );
}

#[test]
fn highlights_downgrades_for_a_refused_context() {
    // The default highlights request downgrades to public prose once a
    // context's server refusal has been observed.
    let requested = ClaudeThinkingDisplay::Highlights;
    let settings = ClaudeSettings::from_selection(&selection())
        .expect("settings")
        .with_thinking_display(requested.with_refusal(false));
    for session in [
        ClaudeSession::Start("session-highlights".to_owned()),
        ClaudeSession::Resume("session-highlights".to_owned()),
    ] {
        assert_eq!(
            display_pair(&settings.spawn_args(&session)),
            Some("highlights")
        );
    }
    assert_eq!(
        requested.with_refusal(true),
        ClaudeThinkingDisplay::Summarized
    );
    // A refusal never rewrites an already-public or unrequested policy.
    assert_eq!(
        ClaudeThinkingDisplay::Summarized.with_refusal(true),
        ClaudeThinkingDisplay::Summarized
    );
    assert_eq!(
        ClaudeThinkingDisplay::Unrequested.with_refusal(true),
        ClaudeThinkingDisplay::Unrequested
    );
}

#[test]
fn older_cli_keeps_its_existing_arguments_on_start_and_resume() {
    let support = claude_thinking_display_support("2.1.220 (Claude Code)");
    assert_eq!(support, ClaudeThinkingDisplaySupport::Unsupported);
    let display = ClaudeThinkingDisplay::for_support(support);
    assert_eq!(display, ClaudeThinkingDisplay::Unrequested);
    let base = ClaudeSettings::from_selection(&selection()).expect("settings");
    let settings = base.clone().with_thinking_display(display);
    for session in [
        ClaudeSession::Start("session-b".to_owned()),
        ClaudeSession::Resume("session-b".to_owned()),
    ] {
        let args = settings.spawn_args(&session);
        assert_eq!(display_pair(&args), None);
        assert_eq!(args, base.spawn_args(&session), "arguments unchanged");
    }
}

#[test]
fn mixed_assistant_frame_projects_thinking_text_and_usage_once() {
    let event = parse_frame(
        r#"{"type":"assistant","message":{"id":"msg-mixed","content":[{"type":"thinking","thinking":"**Planning** the read","signature":"SIG-SECRET"},{"type":"text","text":"Reading "},{"type":"tool_use","id":"tool-1","name":"Read","input":{}},{"type":"text","text":"now"},{"type":"redacted_thinking","data":"OPAQUE"},{"type":"thinking","thinking":"Second stretch","signature":"SIG-SECRET"}],"usage":{"input_tokens":10,"output_tokens":4}}}"#,
        1,
    )
    .expect("mixed frame decodes");
    let ClaudeEvent::Assistant(frame) = event else {
        panic!("expected one assistant frame");
    };
    assert_eq!(frame.message_id.as_deref(), Some("msg-mixed"));
    assert_eq!(
        frame.content,
        vec![
            ClaudeAssistantContent::Thinking {
                text: "**Planning** the read".to_owned(),
                title: None
            },
            ClaudeAssistantContent::Text {
                text: "Reading now".to_owned(),
                phase: "commentary"
            },
            ClaudeAssistantContent::ToolUse(ClaudeToolUse {
                id: "tool-1".to_owned(),
                name: "Read".to_owned(),
                input: serde_json::json!({}),
            }),
            ClaudeAssistantContent::Thinking {
                text: "Second stretch".to_owned(),
                title: None
            },
        ]
    );
    assert_eq!(frame.usage.and_then(|usage| usage.input), Some(10));
    assert!(!format!("{frame:?}").contains("SIG-SECRET"));
    assert!(!format!("{frame:?}").contains("OPAQUE"));
}

#[test]
fn stream_blocks_keep_their_index_and_opaque_deltas_stay_bookkeeping() {
    let start = parse_frame(
        r#"{"type":"stream_event","event":{"type":"content_block_start","index":2,"content_block":{"type":"thinking","thinking":"","signature":"SIG"}}}"#,
        1,
    )
    .expect("start decodes");
    assert_eq!(
        start,
        ClaudeEvent::ThinkingStarted {
            index: 2,
            title: None
        }
    );
    let text_start = parse_frame(
        r#"{"type":"stream_event","event":{"type":"content_block_start","index":3,"content_block":{"type":"text","text":""}}}"#,
        2,
    )
    .expect("text start decodes");
    assert_eq!(text_start, ClaudeEvent::Unknown);
    let delta = parse_frame(
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":2,"delta":{"type":"thinking_delta","thinking":"Checking","estimated_tokens":null}}}"#,
        3,
    )
    .expect("delta decodes");
    assert_eq!(
        delta,
        ClaudeEvent::ThinkingDelta {
            index: 2,
            text: "Checking".to_owned()
        }
    );
    for bookkeeping in [
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":2,"delta":{"type":"thinking_delta","thinking":"","estimated_tokens":50}}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":2,"delta":{"type":"signature_delta","signature":"SIG"}}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"no index"}}}"#,
    ] {
        assert_eq!(
            parse_frame(bookkeeping, 4).expect("decodes"),
            ClaudeEvent::Unknown
        );
    }
    let stop = parse_frame(
        r#"{"type":"stream_event","event":{"type":"content_block_stop","index":2}}"#,
        5,
    )
    .expect("stop decodes");
    assert_eq!(stop, ClaudeEvent::ContentBlockStopped { index: 2 });
}

/// Everything one replayed fixture projected.
#[derive(Default)]
struct Replay {
    /// Root assistant text as the dispatcher assembles it: deltas append to
    /// their message part, snapshots replace it.
    parts: Vec<(String, String)>,
    text: String,
    deltas: Vec<(String, String, String, String)>,
    completions: Vec<(String, Option<String>, String)>,
    usage_reports: usize,
    outcomes: Vec<ClaudeApplyOutcome>,
    /// Latest phase each message part carried.
    phases: Vec<(String, AssistantMessagePhase)>,
    /// Tool steps as `(tool id, kind, action, detail)`.
    tools: Vec<(String, String, String, Option<String>)>,
    /// Terminal rows as `(activity id, state, command, output)`.
    terminals: Vec<(String, String, Option<String>, Option<String>)>,
    /// Whether the tracker observed a silently omitted highlights request.
    highlights_refused: bool,
}

impl Replay {
    fn streamed(&self, item: &str) -> String {
        self.deltas
            .iter()
            .filter(|(known, ..)| known == item)
            .map(|(_, delta, ..)| delta.as_str())
            .collect()
    }

    fn part(&mut self, part_id: Option<&str>) -> &mut String {
        let part_id = part_id.unwrap_or(UNANNOUNCED_PART);
        if !self.parts.iter().any(|(known, _)| known == part_id) {
            self.parts.push((part_id.to_owned(), String::new()));
        }
        let (_, text) = self
            .parts
            .iter_mut()
            .find(|(known, _)| known == part_id)
            .expect("part exists");
        text
    }

    fn note_phase(&mut self, part_id: Option<&str>, phase: Option<AssistantMessagePhase>) {
        let (Some(part_id), Some(phase)) = (part_id, phase) else {
            return;
        };
        match self.phases.iter_mut().find(|(known, _)| known == part_id) {
            Some((_, known)) => *known = phase,
            None => self.phases.push((part_id.to_owned(), phase)),
        }
    }

    fn phase(&self, part_id: &str) -> Option<AssistantMessagePhase> {
        self.phases
            .iter()
            .find(|(known, _)| known == part_id)
            .map(|(_, phase)| *phase)
    }
}

/// Part key for text whose stream never announced a message.
const UNANNOUNCED_PART: &str = "unannounced";

fn fixture_session(fixture: &str) -> String {
    fixture
        .lines()
        .find_map(|line| {
            let value: serde_json::Value = serde_json::from_str(line).ok()?;
            (value.get("subtype")?.as_str()? == "init")
                .then(|| value.get("session_id")?.as_str().map(str::to_owned))?
        })
        .expect("fixture announces its session")
}

async fn replay_lines(lines: &[&str], run: &str, display: ClaudeThinkingDisplay) -> Replay {
    let run = RunId::parse(run).expect("run id");
    let session = lines
        .iter()
        .find_map(|line| {
            let value: serde_json::Value = serde_json::from_str(line).ok()?;
            (value.get("subtype")?.as_str()? == "init")
                .then(|| value.get("session_id")?.as_str().map(str::to_owned))?
        })
        .unwrap_or_else(|| "fixture-session".to_owned());
    let thread = ThreadId::parse("thread-thinking").expect("thread id");
    let model = EngineModelId::parse("claude-sonnet-5").expect("model id");
    let scope = ClaudeUsageScope {
        thread_id: &thread,
        model_id: &model,
        provider_session_id: &session,
    };
    let (sender, mut receiver) = mpsc::channel(4096);
    let mut tracker = ClaudePendingTracker::with_thinking_display(display);
    let mut active = None;
    let mut replay = Replay::default();
    for (index, line) in lines.iter().enumerate() {
        let sequence = index as u64 + 1;
        let Ok(event) = parse_frame(line, sequence) else {
            continue;
        };
        replay.outcomes.push(
            apply_event(
                event,
                &run,
                &session,
                &mut tracker,
                &mut active,
                &sender,
                sequence,
                Some(&scope),
            )
            .await,
        );
    }
    replay.highlights_refused = tracker.highlights_refused();
    drop(sender);
    while let Some(observation) = receiver.recv().await {
        match observation {
            EngineObservation::TextDelta(delta) => {
                replay.part(delta.part_id()).push_str(delta.delta());
                replay.note_phase(delta.part_id(), delta.phase());
            }
            EngineObservation::TextSnapshot(snapshot) => {
                let part = replay.part(Some(snapshot.part_id()));
                part.clear();
                part.push_str(snapshot.text());
                replay.note_phase(Some(snapshot.part_id()), snapshot.phase());
            }
            EngineObservation::Activity(Observation::Tool(row)) => {
                replay.tools.push((
                    row.tool_id().as_str().to_owned(),
                    row.tool_name().to_owned(),
                    row.action().as_str().to_owned(),
                    row.detail().map(str::to_owned),
                ));
            }
            EngineObservation::Activity(Observation::TerminalActivity(row)) => {
                replay.terminals.push((
                    row.activity_id().as_str().to_owned(),
                    row.state().as_str().to_owned(),
                    row.command().map(str::to_owned),
                    row.output().map(str::to_owned),
                ));
            }
            EngineObservation::Usage(_) => replay.usage_reports += 1,
            EngineObservation::Activity(Observation::ReasoningSummaryDelta(row)) => {
                replay.deltas.push((
                    row.item_id().as_str().to_owned(),
                    row.delta().to_owned(),
                    row.turn_id().as_str().to_owned(),
                    row.id().as_str().to_owned(),
                ));
            }
            EngineObservation::Activity(Observation::ReasoningSummaryCompleted(row)) => {
                replay.completions.push((
                    row.item_id().as_str().to_owned(),
                    row.text().map(str::to_owned),
                    row.turn_id().as_str().to_owned(),
                ));
            }
            _ => {}
        }
    }
    replay.text = replay.parts.iter().map(|(_, text)| text.as_str()).collect();
    replay
}

async fn replay(fixture: &str, run: &str, display: ClaudeThinkingDisplay) -> Replay {
    let lines: Vec<&str> = fixture.lines().filter(|line| !line.is_empty()).collect();
    replay_lines(&lines, run, display).await
}

fn assistant_frames_with_usage(fixture: &str) -> usize {
    fixture
        .lines()
        .filter(|line| {
            serde_json::from_str::<serde_json::Value>(line).is_ok_and(|value| {
                value["type"] == "assistant" && value["message"].get("usage").is_some()
            })
        })
        .count()
}

fn result_frames(fixture: &str) -> usize {
    fixture
        .lines()
        .filter(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .is_ok_and(|value| value["type"] == "result")
        })
        .count()
}

#[tokio::test]
async fn tool_cycle_capture_streams_then_settles_one_stretch() {
    let replay = replay(TOOLS, "run-tools", ClaudeThinkingDisplay::Summarized).await;
    assert_eq!(replay.completions.len(), 1, "one stretch, one completion");
    let (item, text, turn) = &replay.completions[0];
    assert_eq!(item, "thinking:msg_fixture_02:0");
    assert_eq!(turn, "run-tools", "the run scopes the turn");
    let streamed = replay.streamed(item);
    assert!(streamed.starts_with("Checking the pair sums"));
    assert_eq!(
        text.as_deref(),
        Some(streamed.as_str()),
        "buffered text reconciles the streamed text without appending"
    );
    assert!(replay.deltas.iter().all(|(known, ..)| known == item));
    // The answer text is intact and never mixed with thinking.
    assert!(replay.text.contains("The two values closest to 50"));
    assert!(!replay.text.contains("Checking the pair sums"));
    // Usage projects once per assistant frame plus the terminal totals.
    assert_eq!(
        replay.usage_reports,
        assistant_frames_with_usage(TOOLS) + result_frames(TOOLS)
    );
    assert!(
        replay
            .outcomes
            .iter()
            .all(|outcome| !matches!(outcome, ClaudeApplyOutcome::Terminal(_)))
    );
}

#[tokio::test]
async fn mixed_capture_keeps_both_paragraphs_under_one_item() {
    let replay = replay(MIXED, "run-mixed", ClaudeThinkingDisplay::Summarized).await;
    assert_eq!(replay.completions.len(), 1);
    let (item, text, _) = &replay.completions[0];
    let text = text.as_deref().expect("authoritative text");
    assert!(text.starts_with("I'm checking pairwise sums"));
    assert!(text.contains("\n\nIf interpreting"));
    assert_eq!(replay.streamed(item), text);
    assert!(replay.text.starts_with("In numbers.txt"));
    assert_eq!(
        replay.usage_reports,
        assistant_frames_with_usage(MIXED) + result_frames(MIXED)
    );
}

#[tokio::test]
async fn two_stretches_in_one_message_keep_distinct_block_identities() {
    let replay = replay(TWO_STRETCHES, "run-two", ClaudeThinkingDisplay::Summarized).await;
    let items: Vec<&str> = replay
        .completions
        .iter()
        .map(|(item, ..)| item.as_str())
        .collect();
    assert_eq!(
        items,
        vec!["thinking:msg_fixture_09:0", "thinking:msg_fixture_09:2"]
    );
    assert_eq!(
        replay.completions[1].1.as_deref(),
        Some("**Comparing** the pair sums")
    );
    assert!(replay.text.contains("Beta and gamma sum to 51."));
    assert!(!replay.text.contains("Comparing"));
    // Row identities never collide across stretches or fragments.
    let mut ids: Vec<&str> = replay.deltas.iter().map(|(.., id)| id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), replay.deltas.len());
}

#[tokio::test]
async fn no_thinking_turn_and_unrequested_display_project_no_reasoning() {
    let start = replay(START, "run-start", ClaudeThinkingDisplay::Summarized).await;
    assert!(start.deltas.is_empty() && start.completions.is_empty());
    assert!(start.text.contains("17 minutes"));
    // The flagless capture streams empty thinking; without a requested
    // display its semantics are unknown, so nothing projects.
    let flagless = replay(DEFAULT, "run-default", ClaudeThinkingDisplay::Unrequested).await;
    assert!(flagless.deltas.is_empty() && flagless.completions.is_empty());
    assert!(flagless.text.contains("18 sheep"));
    // A requested display that still streams no public prose settles its
    // signature-only stretch without text: valid, not a parse failure.
    let empty = replay(DEFAULT, "run-empty", ClaudeThinkingDisplay::Summarized).await;
    assert!(empty.deltas.is_empty());
    assert_eq!(
        empty.completions,
        vec![(
            "thinking:msg_fixture_01:0".to_owned(),
            None,
            "run-empty".to_owned()
        )]
    );
}

#[tokio::test]
async fn native_resume_starts_fresh_run_local_tracking() {
    assert_eq!(fixture_session(START), fixture_session(RESUME));
    let first = replay(START, "run-first", ClaudeThinkingDisplay::Summarized).await;
    let resumed = replay(RESUME, "run-resumed", ClaudeThinkingDisplay::Summarized).await;
    assert!(first.completions.is_empty());
    assert_eq!(resumed.completions.len(), 1);
    let (item, text, turn) = &resumed.completions[0];
    assert_eq!(turn, "run-resumed");
    assert!(
        text.as_deref()
            .is_some_and(|text| text.starts_with("With crossing times"))
    );
    assert!(
        resumed
            .deltas
            .iter()
            .all(|(known, _, turn, id)| known == item
                && turn == "run-resumed"
                && id.starts_with("run-resumed:claude:"))
    );
}

#[tokio::test]
async fn interruption_leaves_no_invented_completion() {
    let cut = replay(
        INTERRUPTED_THINKING,
        "run-cut",
        ClaudeThinkingDisplay::Summarized,
    )
    .await;
    assert!(!cut.deltas.is_empty(), "streamed fragments still project");
    assert!(cut.completions.is_empty(), "EOF never settles a stretch");
    let killed = replay(INTERRUPTED, "run-killed", ClaudeThinkingDisplay::Summarized).await;
    assert!(killed.deltas.is_empty() && killed.completions.is_empty());
}

const STRETCH_START: &str = r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":"SIG-SECRET"}}}"#;
const STRETCH_STOP: &str =
    r#"{"type":"stream_event","event":{"type":"content_block_stop","index":0}}"#;
const MESSAGE_START: &str =
    r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"msg-edge"}}}"#;

fn thinking_delta(text: &str) -> String {
    serde_json::json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,
        "delta":{"type":"thinking_delta","thinking":text}}})
    .to_string()
}

fn buffered_thinking(text: &str) -> String {
    serde_json::json!({"type":"assistant","message":{"id":"msg-edge",
        "content":[{"type":"thinking","thinking":text,"signature":"SIG-SECRET"}]}})
    .to_string()
}

#[tokio::test]
async fn authoritative_correction_and_duplicate_frames_settle_once() {
    let delta = thinking_delta("Draft title");
    let buffered = buffered_thinking("Corrected title");
    let lines = [
        MESSAGE_START,
        STRETCH_START,
        delta.as_str(),
        buffered.as_str(),
        buffered.as_str(),
        STRETCH_STOP,
        STRETCH_STOP,
    ];
    let replay = replay_lines(&lines, "run-edge", ClaudeThinkingDisplay::Summarized).await;
    assert_eq!(replay.streamed("thinking:msg-edge:0"), "Draft title");
    assert_eq!(
        replay.completions,
        vec![(
            "thinking:msg-edge:0".to_owned(),
            Some("Corrected title".to_owned()),
            "run-edge".to_owned()
        )]
    );
    // A stop without a buffered frame settles without replacement text.
    let unbuffered = replay_lines(
        &[MESSAGE_START, STRETCH_START, delta.as_str(), STRETCH_STOP],
        "run-stop",
        ClaudeThinkingDisplay::Summarized,
    )
    .await;
    assert_eq!(unbuffered.completions.len(), 1);
    assert_eq!(unbuffered.completions[0].1, None);
    // A buffered block with no streamed start has no verified index.
    let orphan = replay_lines(
        &[MESSAGE_START, buffered.as_str()],
        "run-orphan",
        ClaudeThinkingDisplay::Summarized,
    )
    .await;
    assert!(orphan.completions.is_empty());
}

#[tokio::test]
async fn fragments_respect_unicode_boundaries_and_accumulation_bounds() {
    let wide = "\u{1f9e0}".repeat(OBSERVATION_DELTA_MAX_BYTES / 4 + 3);
    let wide_delta = thinking_delta(&wide);
    let replay_wide = replay_lines(
        &[MESSAGE_START, STRETCH_START, wide_delta.as_str()],
        "run-wide",
        ClaudeThinkingDisplay::Summarized,
    )
    .await;
    assert!(replay_wide.deltas.len() > 1);
    assert!(
        replay_wide
            .deltas
            .iter()
            .all(|(_, delta, ..)| delta.len() <= OBSERVATION_DELTA_MAX_BYTES)
    );
    assert_eq!(replay_wide.streamed("thinking:msg-edge:0"), wide);

    // Accumulation stops at the message ceiling instead of truncating, and
    // an oversized authoritative text settles without replacement.
    let chunk = "a".repeat(OBSERVATION_MESSAGE_MAX_BYTES / 2);
    let chunk_delta = thinking_delta(&chunk);
    let small_delta = thinking_delta("tail");
    let oversized = buffered_thinking(&"b".repeat(OBSERVATION_MESSAGE_MAX_BYTES + 1));
    let lines = [
        MESSAGE_START,
        STRETCH_START,
        chunk_delta.as_str(),
        chunk_delta.as_str(),
        chunk_delta.as_str(),
        small_delta.as_str(),
        oversized.as_str(),
        STRETCH_STOP,
    ];
    let bounded = replay_lines(&lines, "run-bounded", ClaudeThinkingDisplay::Summarized).await;
    let streamed = bounded.streamed("thinking:msg-edge:0");
    assert_eq!(streamed.len(), chunk.len() * 2, "no gap, no truncation");
    assert!(!streamed.contains("tail"));
    assert_eq!(bounded.completions.len(), 1);
    assert_eq!(bounded.completions[0].1, None);
}

#[tokio::test]
async fn child_thinking_and_signatures_never_reach_root_reasoning() {
    let child_start = r#"{"type":"stream_event","parent_tool_use_id":"tool-9","event":{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}}"#;
    let child_delta = r#"{"type":"stream_event","parent_tool_use_id":"tool-9","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"child plan"}}}"#;
    let child_buffered = r#"{"type":"assistant","parent_tool_use_id":"tool-9","message":{"id":"msg-child","content":[{"type":"thinking","thinking":"child plan","signature":"SIG-SECRET"}]}}"#;
    assert!(matches!(
        parse_frame(child_delta, 1).expect("child decodes"),
        ClaudeEvent::ChildTranscript { text: None, .. }
    ));
    let delta = thinking_delta("Root plan");
    let buffered = buffered_thinking("Root plan");
    let lines = [
        MESSAGE_START,
        STRETCH_START,
        child_start,
        child_delta,
        delta.as_str(),
        child_buffered,
        buffered.as_str(),
        STRETCH_STOP,
    ];
    let replay = replay_lines(&lines, "run-child", ClaudeThinkingDisplay::Summarized).await;
    assert_eq!(replay.streamed("thinking:msg-edge:0"), "Root plan");
    assert_eq!(replay.completions.len(), 1);
    assert_eq!(replay.completions[0].1.as_deref(), Some("Root plan"));
    let everything = format!("{:?}{:?}{}", replay.deltas, replay.completions, replay.text);
    assert!(!everything.contains("child plan"));
    assert!(!everything.contains("SIG-SECRET"));
}

/// The authoritative root text per message: every buffered root text block
/// in provider order, keyed by its message id.
fn buffered_root_text(fixture: &str) -> Vec<(String, String)> {
    let mut parts: Vec<(String, String)> = Vec::new();
    for line in fixture.lines().filter(|line| !line.is_empty()) {
        let value: serde_json::Value = serde_json::from_str(line).expect("fixture json");
        if value["type"] != "assistant" || !value["parent_tool_use_id"].is_null() {
            continue;
        }
        let message = &value["message"];
        let text: String = message["content"]
            .as_array()
            .expect("content")
            .iter()
            .filter(|item| item["type"] == "text")
            .filter_map(|item| item["text"].as_str())
            .collect();
        if text.is_empty() {
            continue;
        }
        let id = message["id"].as_str().expect("message id").to_owned();
        match parts.iter_mut().find(|(known, _)| *known == id) {
            Some((_, body)) => body.push_str(&text),
            None => parts.push((id, text)),
        }
    }
    parts
}

#[tokio::test]
async fn real_captures_commit_the_buffered_answer_exactly_once() {
    for (name, fixture, display) in [
        ("tools", TOOLS, ClaudeThinkingDisplay::Summarized),
        ("mixed", MIXED, ClaudeThinkingDisplay::Summarized),
        ("start", START, ClaudeThinkingDisplay::Summarized),
        ("resume", RESUME, ClaudeThinkingDisplay::Summarized),
        ("default", DEFAULT, ClaudeThinkingDisplay::Unrequested),
    ] {
        let expected = buffered_root_text(fixture);
        assert!(!expected.is_empty(), "{name}: capture answers in text");
        let replay = replay(fixture, &format!("run-text-{name}"), display).await;
        assert_eq!(
            replay.parts, expected,
            "{name}: streamed deltas settle to the buffered text exactly once"
        );
    }
}

fn text_delta(index: u64, text: &str) -> String {
    serde_json::json!({"type":"stream_event","event":{"type":"content_block_delta",
        "index":index,"delta":{"type":"text_delta","text":text}}})
    .to_string()
}

fn buffered_text(message: &str, text: &str) -> String {
    serde_json::json!({"type":"assistant","message":{"id":message,
        "content":[{"type":"text","text":text}]}})
    .to_string()
}

fn message_start(message: &str) -> String {
    serde_json::json!({"type":"stream_event","event":{"type":"message_start",
        "message":{"id":message}}})
    .to_string()
}

#[tokio::test]
async fn text_blocks_across_messages_settle_to_the_buffered_text_once() {
    let child_delta = r#"{"type":"stream_event","parent_tool_use_id":"tool-1","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"child says"}}}"#;
    let child_buffered = r#"{"type":"assistant","parent_tool_use_id":"tool-1","message":{"id":"msg-child","content":[{"type":"text","text":"child says"}]}}"#;
    let tool_use = r#"{"type":"assistant","message":{"id":"msg-a","content":[{"type":"tool_use","id":"tool-1","name":"Task","input":{}}]}}"#;
    let result = r#"{"type":"result","subtype":"success","is_error":false}"#;
    let lines: Vec<String> = vec![
        // Thinking, text, and a tool use in one message.
        message_start("msg-a"),
        STRETCH_START.to_owned(),
        thinking_delta("Plan"),
        buffered_thinking("Plan").replace("msg-edge", "msg-a"),
        STRETCH_STOP.to_owned(),
        text_delta(1, "Let me "),
        text_delta(1, "check."),
        buffered_text("msg-a", "Let me check."),
        tool_use.to_owned(),
        // The subagent speaks between the tool cycle's messages.
        child_delta.to_owned(),
        child_buffered.to_owned(),
        // Three text blocks: exact, stopped short, and corrected.
        message_start("msg-b"),
        text_delta(0, "First "),
        text_delta(0, "block."),
        buffered_text("msg-b", "First block."),
        text_delta(1, "Second"),
        buffered_text("msg-b", "Second block."),
        text_delta(2, "Draft"),
        buffered_text("msg-b", "Final."),
        // Partials only: no buffered frame before the result.
        message_start("msg-c"),
        text_delta(0, "Tail"),
        result.to_owned(),
    ];
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    let replay = replay_lines(&lines, "run-blocks", ClaudeThinkingDisplay::Summarized).await;
    assert_eq!(
        replay.parts,
        vec![
            ("msg-a".to_owned(), "Let me check.".to_owned()),
            (
                "msg-b".to_owned(),
                "First block.Second block.Final.".to_owned()
            ),
            ("msg-c".to_owned(), "Tail".to_owned()),
        ]
    );
    assert!(!replay.text.contains("child says"));
    assert_eq!(replay.completions.len(), 1);

    // Buffered frames alone (no partials) land in full, once each.
    let only_a = buffered_text("msg-x", "Only buffered.");
    let only_b = buffered_text("msg-y", " Again.");
    let buffered = replay_lines(
        &[only_a.as_str(), only_b.as_str(), result],
        "run-buffered",
        ClaudeThinkingDisplay::Summarized,
    )
    .await;
    assert_eq!(buffered.text, "Only buffered. Again.");
}

#[tokio::test]
async fn tool_cycle_capture_projects_each_read_as_work() {
    let replay = replay(TOOLS, "run-tool-steps", ClaudeThinkingDisplay::Summarized).await;
    let step = |id: &str, action: &str| {
        (
            id.to_owned(),
            "read".to_owned(),
            action.to_owned(),
            Some("/project/numbers.txt".to_owned()),
        )
    };
    assert_eq!(
        replay.tools,
        vec![
            step("toolu_fixture_01", "started"),
            step("toolu_fixture_01", "completed"),
            step("toolu_fixture_02", "started"),
            step("toolu_fixture_02", "completed"),
        ]
    );
    // The answer after the last tool result is never commentary.
    assert_ne!(
        replay.phase("msg_fixture_03"),
        Some(AssistantMessagePhase::Commentary)
    );
}

fn tool_use_start(index: u64, id: &str, name: &str) -> String {
    serde_json::json!({"type":"stream_event","event":{"type":"content_block_start",
        "index":index,"content_block":{"type":"tool_use","id":id,"name":name,"input":{}}}})
    .to_string()
}

fn buffered_tool_use(message: &str, id: &str, name: &str, input: &serde_json::Value) -> String {
    serde_json::json!({"type":"assistant","message":{"id":message,
        "content":[{"type":"tool_use","id":id,"name":name,"input":input}]}})
    .to_string()
}

fn tool_result(id: &str, output: &str, is_error: bool) -> String {
    serde_json::json!({"type":"user","parent_tool_use_id":null,"message":{"role":"user",
        "content":[{"type":"tool_result","tool_use_id":id,"is_error":is_error,
            "content":[{"type":"text","text":output}]}]}})
    .to_string()
}

#[tokio::test]
async fn narration_before_a_tool_call_is_commentary_and_commands_are_terminal_work() {
    let lines: Vec<String> = vec![
        message_start("msg-plan"),
        text_delta(0, "I'll list "),
        text_delta(0, "the files."),
        buffered_text("msg-plan", "I'll list the files."),
        tool_use_start(1, "toolu-ls", "Bash"),
        text_delta(2, " Then read."),
        buffered_tool_use(
            "msg-plan",
            "toolu-ls",
            "Bash",
            &serde_json::json!({"command":"ls -a","description":"List files"}),
        ),
        tool_result("toolu-ls", "a\nb", false),
        message_start("msg-retry"),
        buffered_tool_use(
            "msg-retry",
            "toolu-bad",
            "Bash",
            &serde_json::json!({"command":"false"}),
        ),
        tool_result("toolu-bad", "exit 1", true),
        tool_result("toolu-unknown", "ignored", false),
        message_start("msg-answer"),
        text_delta(0, "Two files."),
        buffered_text("msg-answer", "Two files."),
    ];
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    let replay = replay_lines(&lines, "run-commentary", ClaudeThinkingDisplay::Summarized).await;
    assert_eq!(
        replay.parts,
        vec![
            (
                "msg-plan".to_owned(),
                "I'll list the files. Then read.".to_owned()
            ),
            ("msg-answer".to_owned(), "Two files.".to_owned()),
        ]
    );
    assert_eq!(
        replay.phase("msg-plan"),
        Some(AssistantMessagePhase::Commentary),
        "text of a message that called a tool is narration"
    );
    assert_eq!(
        replay.phase("msg-answer"),
        Some(AssistantMessagePhase::Unspecified)
    );
    let row = |id: &str, state: &str, command: Option<&str>, output: Option<&str>| {
        (
            id.to_owned(),
            state.to_owned(),
            command.map(str::to_owned),
            output.map(str::to_owned),
        )
    };
    assert_eq!(
        replay.terminals,
        vec![
            row("toolu-ls", "started", Some("ls -a"), None),
            row("toolu-ls", "completed", None, Some("a\nb")),
            row("toolu-bad", "started", Some("false"), None),
            row("toolu-bad", "failed", None, Some("exit 1")),
        ]
    );
    assert!(replay.tools.is_empty(), "commands are terminal work only");
}

#[test]
fn summaries_select_the_newest_non_empty_title() {
    let start = parse_frame(
        r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":"SIG","summaries":[{"summary":"First"},{"summary":"  "},{"summary":"Newest"}]}}}"#,
        1,
    )
    .expect("start decodes");
    assert_eq!(
        start,
        ClaudeEvent::ThinkingStarted {
            index: 0,
            title: Some("Newest".to_owned())
        }
    );
    let buffered = parse_frame(
        r#"{"type":"assistant","message":{"id":"msg-hl","content":[{"type":"thinking","thinking":"","signature":"SIG","summaries":[{"summary":"  "},{"summary":"Buffered title"},{"without":"summary"},{"summary":42}]}]}}"#,
        2,
    )
    .expect("buffered decodes");
    let ClaudeEvent::Assistant(frame) = buffered else {
        panic!("expected one assistant frame");
    };
    assert_eq!(
        frame.content,
        vec![ClaudeAssistantContent::Thinking {
            text: String::new(),
            title: Some("Buffered title".to_owned())
        }]
    );
    assert!(!format!("{frame:?}").contains("SIG"));
}

#[tokio::test]
async fn highlight_titles_project_live_and_settle_on_the_buffered_frame() {
    let replay = replay(
        HIGHLIGHT_TITLES,
        "run-highlight",
        ClaudeThinkingDisplay::Highlights,
    )
    .await;
    assert_eq!(replay.completions.len(), 1);
    let (item, text, _) = &replay.completions[0];
    assert_eq!(item, "thinking:msg_fixture_hl:0");
    assert_eq!(
        text.as_deref(),
        Some("Checking pairwise sums of the values")
    );
    assert_eq!(
        replay.streamed(item),
        "Checking pairwise sums of the values",
        "the block-start title is the live label, settled once by the buffered title"
    );
    assert!(!replay.highlights_refused);
    assert!(replay.text.contains("All six pair sums are distinct."));
}

#[tokio::test]
async fn refused_highlights_settle_empty_and_mark_the_context() {
    let refused = replay(
        HIGHLIGHTS_REFUSED,
        "run-hl-refused",
        ClaudeThinkingDisplay::Highlights,
    )
    .await;
    assert!(refused.deltas.is_empty(), "no titles or prose project");
    assert!(!refused.text.is_empty(), "the answer text still lands");
    assert!(
        refused
            .completions
            .iter()
            .all(|(_, text, _)| text.is_none())
    );
    assert!(
        refused.highlights_refused,
        "empty stretches under a highlights request are a silent server refusal"
    );
    // The same capture under summarized is merely an empty public display.
    let public = replay(
        HIGHLIGHTS_REFUSED,
        "run-hl-public",
        ClaudeThinkingDisplay::Summarized,
    )
    .await;
    assert!(!public.highlights_refused);
    // A no-thinking turn never infers capability loss.
    let no_think = replay(START, "run-hl-nothink", ClaudeThinkingDisplay::Highlights).await;
    assert!(no_think.completions.is_empty());
    assert!(!no_think.highlights_refused);
}

#[tokio::test]
async fn highlights_mode_never_projects_prose_but_its_presence_clears_refusal() {
    let delta = thinking_delta("Unexpected prose");
    let buffered = buffered_thinking("Unexpected prose");
    let lines = [
        MESSAGE_START,
        STRETCH_START,
        delta.as_str(),
        buffered.as_str(),
        STRETCH_STOP,
    ];
    let replay = replay_lines(&lines, "run-hl-prose", ClaudeThinkingDisplay::Highlights).await;
    assert!(
        replay.deltas.is_empty(),
        "prose is not the highlighted label"
    );
    assert_eq!(
        replay.completions,
        vec![(
            "thinking:msg-edge:0".to_owned(),
            None,
            "run-hl-prose".to_owned()
        )]
    );
    assert!(!replay.highlights_refused);
}

#[test]
fn display_refusal_memory_is_keyed_by_context() {
    let mut refusals = ClaudeDisplayRefusals::default();
    let refused = (
        "claude-fixture".to_owned(),
        Some("claude-sonnet-5".to_owned()),
        "2.1.282".to_owned(),
    );
    assert!(!refusals.refused(&refused));
    refusals.mark_refused(refused.clone());
    assert!(refusals.refused(&refused));
    for other in [
        (
            "claude-fixture".to_owned(),
            Some("claude-opus-5".to_owned()),
            "2.1.282".to_owned(),
        ),
        (
            "claude-fixture".to_owned(),
            Some("claude-sonnet-5".to_owned()),
            "2.2.0".to_owned(),
        ),
    ] {
        assert!(!refusals.refused(&other));
    }
}
