//! Persisted engine activity projection into the conversation state machine.
//!
//! [`project_activities`] is the GPUI-free boundary between retained
//! [`EngineObservationState`](crate::engine_observation_state::EngineObservationState)
//! rows and the existing [`ConversationStateEvent`](crate::conversation_state_machine::ConversationStateEvent)
//! / [`SceneFact`](crate::conversation_state_machine::SceneFact) boundary. It
//! performs no I/O, scheduling, retry, clock sampling, logging, or payload
//! decoding: inputs are already-validated domain values paired by the
//! observation state.
//!
//! Rules:
//!
//! - Only attributed rows project. Legacy rows without
//!   [`EngineObservationAttribution`](crate::engine_observation_state::EngineObservationAttribution)
//!   pair their typed payload in the observation state but never fabricate
//!   activity facts here.
//! - Identity is exact Forge identity: the attributed run scopes every row
//!   and scene id, the attributed turn owns every fact, and provider
//!   `turn_id` strings are never cast into [`TurnId`](artisan_domain::TurnId)
//!   nor is the current turn assumed for historical rows.
//! - Transport dedup stays cursor-only in the observation state; this module
//!   only upserts by stable fact id, so reconnect duplicates cannot coalesce
//!   into new cards and two runs sharing provider item names cannot coalesce.
//! - Projection runs against the accepted event and the mounted host's
//!   canonical snapshot. Events arriving before mount or before the canonical
//!   turn exists are retained in the observation state and replay here once
//!   the turn exists; unknown turns are skipped, never fabricated.
//! - One fact per paired typed row: deltas accumulate in the observation
//!   state and project as one cumulative bounded card, never one card per
//!   delta. Plain assistant replies ([`MessagePhase`](artisan_domain::MessagePhase)
//!   text without reasoning/tool evidence) project nothing, so `ProviderWait`
//!   stays a plain-reply/no-activity negative.
//! - Thinking/Working evidence derives from persisted `committed_at` order
//!   and the canonical turn's own creation/update times. `sequence` is never
//!   treated as milliseconds and no clock is sampled here. Terminal
//!   settlement, bounds, reduced motion, and the host timer are untouched.

#![allow(clippy::module_name_repetitions)]

use std::collections::{BTreeMap, BTreeSet};

use artisan_domain::{
    ConversationLifecycle, ConversationSnapshot, ObservationId, RunId, TerminalActivityState,
    ToolAction, TurnId,
};

use crate::conversation_scene::{
    SCENE_ID_MAX_BYTES, SCENE_MAX_ITEMS, SCENE_MAX_TEXT_BYTES, SceneId,
};
use crate::conversation_state_machine::{SceneFact, SceneFactKind};
use crate::engine_observation_state::{EngineObservationState, TimelineRow};

/// Number of activity facts one projection keeps: the scene room the
/// snapshot's durable items leave.
///
/// Activity slides like the rest of the loaded conversation: the newest
/// facts always fit, and the oldest leave the window first. A long thread
/// never stalls its live turn behind history it can no longer show.
#[must_use]
pub fn activity_window(snapshot: &ConversationSnapshot) -> usize {
    SCENE_MAX_ITEMS.saturating_sub(snapshot.items().len())
}

/// Maximum UTF-8 bytes retained in one cumulative reasoning body.
pub const MAX_CUMULATIVE_REASONING_BYTES: usize = SCENE_MAX_TEXT_BYTES;

/// Maximum UTF-8 bytes retained in one tool/activity body.
pub const MAX_ACTIVITY_BODY_BYTES: usize = SCENE_MAX_TEXT_BYTES;

/// Outcome of projecting retained attributed rows against a snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivityProjection {
    /// Facts ready to upsert through `SceneFactCommand::Upsert`.
    ///
    /// Each fact carries a stable run-scoped id, its canonical turn, a
    /// non-colliding ordinal above the durable watermark, and narrow typed
    /// `observed_at_ms` timing from persisted `committed_at`. The upsert is
    /// atomic: identical facts are a no-op, changed facts update in place
    /// while keeping the first accepted ordinal.
    pub facts: Vec<SceneFact>,
    /// Attributed rows skipped because their canonical turn is not yet in
    /// the snapshot. Retained in the observation state for a later replay.
    pub pending: usize,
    /// Attributed rows skipped as foreign (thread mismatch is already
    /// filtered by the state; run mismatches against settled snapshot runs).
    pub rejected: usize,
    /// Oldest projectable rows that slid out of the [`activity_window`].
    /// Facts registered for them earlier are no longer part of the window.
    pub evicted: usize,
}

/// Builds the stable run-scoped scene id for one provider row.
///
/// Returns [`None`] when the scoped text would exceed
/// [`SCENE_ID_MAX_BYTES`]; the caller skips that row rather than fabricating
/// a colliding identity.
#[must_use]
pub fn stable_fact_id(run_id: &RunId, provider_id: &str, prefix: &str) -> Option<SceneId> {
    let text = format!("{prefix}-{}-{provider_id}", run_id.as_str());
    if text.len() > SCENE_ID_MAX_BYTES {
        return None;
    }
    SceneId::parse(text).ok()
}

/// First ordinal of the facts derived from observations.
///
/// Durable turns and items count up from zero; derived facts live far above
/// them, so canonical growth never lands on a fact and a fact never needs
/// moving.
pub const DERIVED_FACT_ORDINAL_BASE: u64 = 1 << 40;

/// The largest delivery sequence an ordinal is derived from; anything
/// beyond it (the live thinking block, which is always newest) shares the
/// last place.
const DERIVED_FACT_MAX_SEQUENCE: u64 = 1 << 60;

/// Scene ordinal of the fact whose first event has `first_delivery_sequence`.
///
/// The sequence is unique in its thread and never changes, so the ordinal
/// does not depend on which other rows are loaded: older turns read on
/// demand, or a settled turn's work read when its section opens, slot in
/// without moving anything. The row that stands in for held-back work takes
/// the place just before the first row it stands for.
fn derived_fact_ordinal(first_delivery_sequence: u64, stand_in: bool) -> u64 {
    let place = first_delivery_sequence.min(DERIVED_FACT_MAX_SEQUENCE) * 2;
    DERIVED_FACT_ORDINAL_BASE + place - u64::from(stand_in && place > 0)
}

/// Activity kind of the one row that stands in for a settled turn's work
/// while it is still on the Forge.
pub const HELD_BACK_ACTIVITY_KIND: &str = "held_back";

/// Scene id of the row that stands in for `turn_id`'s held-back work.
#[must_use]
pub fn held_back_fact_id(turn_id: &TurnId) -> Option<SceneId> {
    let text = format!("obs-held-{}", turn_id.as_str());
    if text.len() > SCENE_ID_MAX_BYTES {
        return None;
    }
    SceneId::parse(text).ok()
}

/// What the stand-in row says while the turn's rows are being read.
fn held_back_text(row_count: u32) -> String {
    if row_count == 1 {
        String::from("Loading 1 step…")
    } else {
        format!("Loading {row_count} steps…")
    }
}

/// Builds the stable run-scoped scene id for one timeline row.
///
/// The run-local sequence scopes the id; the thread-scoped delivery sequence
/// orders, never identifies.
#[must_use]
pub fn stable_timeline_fact_id(
    run_id: &RunId,
    sequence: u64,
    tag: &str,
    delivery_sequence: u64,
) -> Option<SceneId> {
    let text = format!(
        "obs-{}-{tag}-{sequence}-{delivery_sequence}",
        run_id.as_str()
    );
    if text.len() > SCENE_ID_MAX_BYTES {
        return None;
    }
    SceneId::parse(text).ok()
}

/// Truncates renderer text to a UTF-8 byte bound on a char boundary.
#[must_use]
pub fn truncate_bounded(text: &str, maximum: usize) -> String {
    if text.len() <= maximum {
        return text.to_owned();
    }
    let mut end = maximum;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// Projects retained attributed rows into bounded scene facts.
///
/// - `state` supplies paired typed rows with attributed run/turn/time.
/// - `snapshot` is the mounted host's canonical snapshot; only turns present
///   there receive facts. Unknown turns count as `pending` (retain, replay
///   later). Thread mismatch yields empty output.
/// - Legacy rows without attribution never project.
/// - Plain message rows never project.
/// - Ordinals come from each row's first delivery sequence, far above every
///   durable ordinal, so scene order is stable without clock sampling, never
///   collides with durable items, and does not depend on which rows are
///   loaded.
/// - A settled turn whose work rows are still on the Forge projects one row
///   standing in for them.
/// - Only the newest [`activity_window`] facts are kept; older rows count as
///   `evicted`.
/// - Bodies are truncated to scene bounds; only the public reasoning summary
///   is retained.
#[must_use]
#[expect(
    clippy::too_many_lines,
    reason = "one ordered projection pass keeps the watermark, ordering, and body-bounds rules reviewable together"
)]
pub fn project_activities(
    state: &EngineObservationState,
    snapshot: &ConversationSnapshot,
) -> ActivityProjection {
    if snapshot.thread_id() != state.thread_id() {
        return ActivityProjection {
            facts: Vec::new(),
            pending: 0,
            rejected: 0,
            evicted: 0,
        };
    }
    let known_turns: BTreeSet<&TurnId> =
        snapshot.turns().iter().map(|turn| &turn.turn_id).collect();
    let mut known_runs_by_turn: BTreeMap<&TurnId, BTreeSet<&str>> = BTreeMap::new();
    for item in snapshot.items() {
        if let artisan_domain::ConversationItem::AssistantMessage(message) = item {
            known_runs_by_turn
                .entry(&message.turn_id)
                .or_default()
                .insert(message.run_id.as_str());
        }
    }
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut pending = 0usize;
    let mut rejected = 0usize;

    for row in state.reasoning_in_order() {
        let (Some(run), Some(turn), Some(committed_at), Some(delivery_sequence)) = (
            row.attributed_run(),
            row.attributed_turn(),
            row.committed_at(),
            row.delivery_sequence(),
        ) else {
            continue;
        };
        if !known_turns.contains(turn) {
            pending += 1;
            continue;
        }
        if is_foreign_run(&known_runs_by_turn, turn, run) {
            rejected += 1;
            continue;
        }
        let Some(id) = stable_fact_id(run, row.item_id(), "reasoning") else {
            rejected += 1;
            continue;
        };
        candidates.push(Candidate {
            id,
            turn: turn.clone(),
            run: run.clone(),
            committed_at_ms: committed_at.as_millis(),
            first_committed_at_ms: row.first_committed_at().unwrap_or(committed_at).as_millis(),
            delivery_sequence,
            ordinal: derived_fact_ordinal(
                row.first_delivery_sequence().unwrap_or(delivery_sequence),
                false,
            ),
            activity_lifecycle: None,
            kind: CandidateKind::Reasoning {
                body: truncate_bounded(row.text(), MAX_CUMULATIVE_REASONING_BYTES),
            },
        });
    }

    for row in state.tools_in_order() {
        let (Some(run), Some(turn), Some(committed_at), Some(delivery_sequence)) = (
            row.attributed_run(),
            row.attributed_turn(),
            row.committed_at(),
            row.delivery_sequence(),
        ) else {
            continue;
        };
        if !known_turns.contains(turn) {
            pending += 1;
            continue;
        }
        if is_foreign_run(&known_runs_by_turn, turn, run) {
            rejected += 1;
            continue;
        }
        let Some(id) = stable_fact_id(run, row.tool_id(), "tool") else {
            rejected += 1;
            continue;
        };
        let kind = truncate_bounded(row.tool_name(), MAX_ACTIVITY_BODY_BYTES);
        let detail = row
            .detail()
            .map(|detail| truncate_bounded(detail, MAX_ACTIVITY_BODY_BYTES));
        let body = detail.clone().unwrap_or_else(|| kind.clone());
        candidates.push(Candidate {
            id,
            turn: turn.clone(),
            run: run.clone(),
            committed_at_ms: committed_at.as_millis(),
            first_committed_at_ms: row.first_committed_at().unwrap_or(committed_at).as_millis(),
            delivery_sequence,
            ordinal: derived_fact_ordinal(
                row.first_delivery_sequence().unwrap_or(delivery_sequence),
                false,
            ),
            activity_lifecycle: Some(tool_activity_lifecycle(row.action())),
            kind: CandidateKind::Activity { body, kind, detail },
        });
    }

    for row in state.terminals_in_order() {
        let (Some(run), Some(turn), Some(committed_at), Some(delivery_sequence)) = (
            row.attributed_run(),
            row.attributed_turn(),
            row.committed_at(),
            row.delivery_sequence(),
        ) else {
            continue;
        };
        if !known_turns.contains(turn) {
            pending += 1;
            continue;
        }
        if is_foreign_run(&known_runs_by_turn, turn, run) {
            rejected += 1;
            continue;
        }
        let Some(id) = stable_fact_id(run, row.activity_id(), "terminal") else {
            rejected += 1;
            continue;
        };
        let state = row.state();
        let kind = CandidateKind::Activity {
            body: terminal_body(row),
            kind: String::from("terminal_activity"),
            detail: row
                .command()
                .map(|command| truncate_bounded(command, MAX_ACTIVITY_BODY_BYTES)),
        };
        let activity_lifecycle = Some(terminal_activity_lifecycle(state));
        candidates.push(Candidate {
            id,
            turn: turn.clone(),
            run: run.clone(),
            committed_at_ms: committed_at.as_millis(),
            first_committed_at_ms: row.first_committed_at().unwrap_or(committed_at).as_millis(),
            delivery_sequence,
            ordinal: derived_fact_ordinal(
                row.first_delivery_sequence().unwrap_or(delivery_sequence),
                false,
            ),
            activity_lifecycle,
            kind,
        });
    }

    for row in state.approvals_in_order() {
        let (Some(run), Some(turn), Some(committed_at), Some(delivery_sequence)) = (
            row.attributed_run(),
            row.attributed_turn(),
            row.committed_at(),
            row.delivery_sequence(),
        ) else {
            continue;
        };
        if !known_turns.contains(turn) {
            pending += 1;
            continue;
        }
        if is_foreign_run(&known_runs_by_turn, turn, run) {
            rejected += 1;
            continue;
        }
        let (Some(id), Ok(approval_id)) = (
            stable_fact_id(run, row.approval_id(), "approval"),
            ObservationId::parse(row.approval_id()),
        ) else {
            rejected += 1;
            continue;
        };
        candidates.push(Candidate {
            id,
            turn: turn.clone(),
            run: run.clone(),
            committed_at_ms: committed_at.as_millis(),
            first_committed_at_ms: row.first_committed_at().unwrap_or(committed_at).as_millis(),
            delivery_sequence,
            ordinal: derived_fact_ordinal(
                row.first_delivery_sequence().unwrap_or(delivery_sequence),
                false,
            ),
            activity_lifecycle: None,
            kind: CandidateKind::Approval {
                prompt: truncate_bounded(row.description(), MAX_ACTIVITY_BODY_BYTES),
                approval_id,
            },
        });
    }

    for row in state.questions_in_order() {
        let (Some(run), Some(turn), Some(committed_at), Some(delivery_sequence)) = (
            row.attributed_run(),
            row.attributed_turn(),
            row.committed_at(),
            row.delivery_sequence(),
        ) else {
            continue;
        };
        if !known_turns.contains(turn) {
            pending += 1;
            continue;
        }
        if is_foreign_run(&known_runs_by_turn, turn, run) {
            rejected += 1;
            continue;
        }
        let Some(id) = stable_fact_id(run, row.question_id(), "question") else {
            rejected += 1;
            continue;
        };
        candidates.push(Candidate {
            id,
            turn: turn.clone(),
            run: run.clone(),
            committed_at_ms: committed_at.as_millis(),
            first_committed_at_ms: row.first_committed_at().unwrap_or(committed_at).as_millis(),
            delivery_sequence,
            ordinal: derived_fact_ordinal(
                row.first_delivery_sequence().unwrap_or(delivery_sequence),
                false,
            ),
            activity_lifecycle: None,
            kind: CandidateKind::Question {
                prompt: truncate_bounded(row.text(), MAX_ACTIVITY_BODY_BYTES),
                answer: row
                    .answers()
                    .map(|answers| truncate_bounded(&answers.join(", "), MAX_ACTIVITY_BODY_BYTES)),
            },
        });
    }

    for row in state.timeline() {
        let (Some(run), Some(turn), Some(committed_at), Some(delivery_sequence)) = (
            row.attributed_run(),
            row.attributed_turn(),
            row.committed_at(),
            row.delivery_sequence(),
        ) else {
            continue;
        };
        if !known_turns.contains(turn) {
            pending += 1;
            continue;
        }
        if is_foreign_run(&known_runs_by_turn, turn, run) {
            rejected += 1;
            continue;
        }
        let sequence = row.sequence().unwrap_or(delivery_sequence);
        let Some(id) = stable_timeline_fact_id(run, sequence, row.tag(), delivery_sequence) else {
            rejected += 1;
            continue;
        };
        let kind = timeline_kind(row);
        let Some(kind) = kind else { continue };
        candidates.push(Candidate {
            id,
            turn: turn.clone(),
            run: run.clone(),
            committed_at_ms: committed_at.as_millis(),
            first_committed_at_ms: row.first_committed_at().unwrap_or(committed_at).as_millis(),
            delivery_sequence,
            ordinal: derived_fact_ordinal(
                row.first_delivery_sequence().unwrap_or(delivery_sequence),
                false,
            ),
            // Timeline rows carry no lifecycle report: unknown never means
            // live, so these facts never count as tool progress.
            activity_lifecycle: None,
            kind,
        });
    }

    // A settled turn's work rows stay on the Forge until its section opens.
    // One row stands in for them at the place of the first, so the turn
    // keeps its section, and its `Worked for` line, before they are read.
    for held in state.held_back() {
        if !known_turns.contains(&held.turn_id) {
            pending += 1;
            continue;
        }
        if is_foreign_run(&known_runs_by_turn, &held.turn_id, &held.run_id) {
            rejected += 1;
            continue;
        }
        let Some(id) = held_back_fact_id(&held.turn_id) else {
            rejected += 1;
            continue;
        };
        let text = held_back_text(held.row_count);
        candidates.push(Candidate {
            id,
            turn: held.turn_id.clone(),
            run: held.run_id.clone(),
            committed_at_ms: held.first_committed_at.as_millis(),
            first_committed_at_ms: held.first_committed_at.as_millis(),
            delivery_sequence: held.first_delivery_sequence,
            ordinal: derived_fact_ordinal(held.first_delivery_sequence, true),
            // Unknown never means live: the stand-in is not tool progress.
            activity_lifecycle: None,
            kind: CandidateKind::Activity {
                body: text.clone(),
                kind: String::from(HELD_BACK_ACTIVITY_KIND),
                detail: Some(text),
            },
        });
    }

    candidates.sort_by(|left, right| {
        (left.committed_at_ms, left.delivery_sequence)
            .cmp(&(right.committed_at_ms, right.delivery_sequence))
    });
    let evicted = candidates.len().saturating_sub(activity_window(snapshot));

    // A fact's ordinal comes from its own first event, so it keeps its place
    // while the window slides and while older rows are read in later.
    let mut facts = Vec::with_capacity(candidates.len() - evicted);
    for candidate in candidates.into_iter().skip(evicted) {
        let ordinal = candidate.ordinal;
        let kind = match candidate.kind {
            CandidateKind::Reasoning { body } => SceneFactKind::Reasoning { body },
            CandidateKind::Activity { body, kind, detail } => SceneFactKind::Activity {
                body,
                kind: Some(kind),
                detail,
            },
            CandidateKind::Approval {
                prompt,
                approval_id,
            } => SceneFactKind::Approval {
                prompt,
                approval_id,
            },
            CandidateKind::Question { prompt, answer } => {
                SceneFactKind::Question { prompt, answer }
            }
            CandidateKind::Compaction { summary } => SceneFactKind::Compaction { summary },
        };
        let Ok(fact) = SceneFact::new(candidate.id, candidate.turn, ordinal, kind) else {
            rejected += 1;
            continue;
        };
        let mut fact = fact
            .with_run_id(candidate.run.clone())
            .with_observed_at_ms(candidate.committed_at_ms)
            .with_derived();
        if let Some(lifecycle) = candidate.activity_lifecycle {
            fact = fact.with_activity_lifecycle(lifecycle);
        }
        fact.first_observed_at_ms = Some(candidate.first_committed_at_ms);
        facts.push(fact);
    }

    ActivityProjection {
        facts,
        pending,
        rejected,
        evicted,
    }
}

fn is_foreign_run(
    known_runs_by_turn: &BTreeMap<&TurnId, BTreeSet<&str>>,
    turn: &TurnId,
    run: &RunId,
) -> bool {
    match known_runs_by_turn.get(turn) {
        None => false,
        Some(known) => !known.is_empty() && !known.contains(run.as_str()),
    }
}

fn terminal_summary(command: Option<&str>, exit_code: Option<i32>) -> String {
    match (command, exit_code) {
        (Some(command), Some(code)) => format!("{command} (exit {code})"),
        (Some(command), None) => command.to_owned(),
        (None, Some(code)) => format!("activity (exit {code})"),
        (None, None) => String::from("activity"),
    }
}

/// Maps a tool row's own lifecycle report onto renderer-visible liveness.
///
/// Open actions stay `Active`; terminal actions settle. There is no second
/// axis to consult natively and none is invented: a single terminal report
/// settles the row.
fn tool_activity_lifecycle(action: ToolAction) -> ConversationLifecycle {
    match action {
        ToolAction::Started | ToolAction::Progress => ConversationLifecycle::Active,
        ToolAction::Completed => ConversationLifecycle::Completed,
        ToolAction::Failed => ConversationLifecycle::Failed,
    }
}

/// Maps a terminal row's own lifecycle report the same way: output is still
/// open work, completion and failure settle it.
fn terminal_activity_lifecycle(state: TerminalActivityState) -> ConversationLifecycle {
    match state {
        TerminalActivityState::Started | TerminalActivityState::Output => {
            ConversationLifecycle::Active
        }
        TerminalActivityState::Completed => ConversationLifecycle::Completed,
        TerminalActivityState::Failed => ConversationLifecycle::Failed,
    }
}

/// Returns the meaningful terminal detail: accumulated output when the
/// provider emitted any, else the command summary. Raw machine prefixes
/// never reach the displayed body.
fn terminal_body(row: &crate::engine_observation_state::TerminalRow) -> String {
    if row.output().is_empty() {
        return truncate_bounded(
            &terminal_summary(row.command(), row.exit_code()),
            MAX_ACTIVITY_BODY_BYTES,
        );
    }
    truncate_bounded(row.output(), MAX_ACTIVITY_BODY_BYTES)
}

fn timeline_kind(row: &TimelineRow) -> Option<CandidateKind> {
    match row.tag() {
        "file" | "search" | "subagent" | "subagent_transcript" | "native_action" | "plan" => {
            Some(timeline_activity(row))
        }
        "compaction" => Some(CandidateKind::Compaction {
            summary: truncate_bounded(row.summary(), MAX_ACTIVITY_BODY_BYTES),
        }),
        // Native diagnostics and retries explain work; only the run's actual
        // terminal failure belongs in the conversation-level failure surface.
        "process_diagnostic" | "protocol_diagnostic" | "retry" => Some(timeline_activity(row)),
        _ => None,
    }
}

/// Builds one timeline row's activity kind: the stable tag classifies it and
/// the sanitized summary is both the body and the row's detail.
fn timeline_activity(row: &TimelineRow) -> CandidateKind {
    let summary = truncate_bounded(row.summary(), MAX_ACTIVITY_BODY_BYTES);
    CandidateKind::Activity {
        body: summary.clone(),
        kind: row.tag().to_owned(),
        detail: Some(summary),
    }
}

enum CandidateKind {
    Reasoning {
        body: String,
    },
    Activity {
        body: String,
        kind: String,
        detail: Option<String>,
    },
    Approval {
        prompt: String,
        approval_id: ObservationId,
    },
    Question {
        prompt: String,
        answer: Option<String>,
    },
    Compaction {
        summary: String,
    },
}

struct Candidate {
    id: SceneId,
    turn: TurnId,
    run: RunId,
    committed_at_ms: i64,
    first_committed_at_ms: i64,
    delivery_sequence: u64,
    /// Scene ordinal, fixed by where the row's first event sits in the
    /// thread.
    ordinal: u64,
    activity_lifecycle: Option<ConversationLifecycle>,
    kind: CandidateKind,
}

#[cfg(test)]
mod tests {
    use super::project_activities;
    use crate::conversation_state_machine::SceneFactKind;
    use crate::engine_observation_state::EngineObservationState;
    use artisan_domain::{
        ConversationCursor, ConversationLifecycle, ConversationSnapshot, ConversationTurn,
        EngineObservationAttribution, EngineObservationEvent, FileAction, FileObservation,
        Observation, ObservationId, ObservationSequence, Revision, RunId, TerminalActivityInput,
        TerminalActivityObservation, TerminalActivityState, ThreadId, ToolAction, ToolObservation,
        TurnId, TurnOrdinal, UnixMillis,
    };

    fn observation_id(value: &str) -> ObservationId {
        ObservationId::parse(value).expect("fixture observation id is valid")
    }

    fn sequence(value: u64) -> ObservationSequence {
        ObservationSequence::new(value).expect("fixture sequence is valid")
    }

    fn attributed_event(
        thread: &ThreadId,
        run: &RunId,
        turn: &TurnId,
        cursor: u64,
        observation: Observation,
    ) -> EngineObservationEvent {
        EngineObservationEvent {
            thread_id: thread.clone(),
            observation,
            attribution: Some(EngineObservationAttribution {
                run_id: run.clone(),
                turn_id: turn.clone(),
                committed_at: UnixMillis::from_millis(cursor.cast_signed()),
                delivery_sequence: cursor,
            }),
        }
    }

    fn snapshot(thread: &ThreadId, turn: &TurnId) -> ConversationSnapshot {
        ConversationSnapshot::new(
            thread.clone(),
            ConversationCursor::new(0),
            vec![ConversationTurn {
                turn_id: turn.clone(),
                ordinal: TurnOrdinal::new(0),
                revision: Revision::new(0),
                lifecycle: ConversationLifecycle::Active,
                created_at: UnixMillis::from_millis(0),
                updated_at: UnixMillis::from_millis(1),
            }],
            Vec::new(),
            UnixMillis::from_millis(1),
        )
        .expect("fixture snapshot is valid")
    }

    #[test]
    fn tool_rows_carry_provider_kind_and_detail_into_activity_facts() {
        let thread = ThreadId::parse("thread-projection").expect("thread");
        let turn = TurnId::parse("turn-projection").expect("turn");
        let run = RunId::parse("run-projection").expect("run");
        let event = attributed_event(
            &thread,
            &run,
            &turn,
            1,
            Observation::Tool(
                ToolObservation::new(
                    observation_id("obs-tool"),
                    sequence(3),
                    observation_id("tool-1"),
                    String::from("read"),
                    ToolAction::Completed,
                    Some(String::from("src/main.rs")),
                )
                .expect("fixture tool observation is valid"),
            ),
        );
        let mut state = EngineObservationState::new(thread.clone());
        let _ = state.apply(1, &event);

        let projection = project_activities(&state, &snapshot(&thread, &turn));
        assert_eq!(projection.facts.len(), 1);
        let fact = &projection.facts[0];
        match &fact.kind {
            SceneFactKind::Activity { body, kind, detail } => {
                assert_eq!(body, "src/main.rs");
                assert_eq!(kind.as_deref(), Some("read"));
                assert_eq!(detail.as_deref(), Some("src/main.rs"));
            }
            other => panic!("expected an activity fact, got {other:?}"),
        }
        assert_eq!(
            fact.activity_lifecycle,
            Some(ConversationLifecycle::Completed)
        );
    }

    #[test]
    fn terminal_rows_carry_the_raw_command_as_terminal_detail() {
        let thread = ThreadId::parse("thread-terminal").expect("thread");
        let turn = TurnId::parse("turn-terminal").expect("turn");
        let run = RunId::parse("run-terminal").expect("run");
        let event = attributed_event(
            &thread,
            &run,
            &turn,
            1,
            Observation::TerminalActivity(
                TerminalActivityObservation::new(
                    observation_id("obs-terminal"),
                    sequence(4),
                    TerminalActivityInput {
                        activity_id: observation_id("activity-1"),
                        channel: None,
                        command: Some(String::from("cargo test --locked")),
                        shell: None,
                        output: Some(String::from("test result: ok")),
                        exit_code: Some(0),
                        state: TerminalActivityState::Completed,
                    },
                )
                .expect("fixture terminal observation is valid"),
            ),
        );
        let mut state = EngineObservationState::new(thread.clone());
        let _ = state.apply(1, &event);

        let projection = project_activities(&state, &snapshot(&thread, &turn));
        assert_eq!(projection.facts.len(), 1);
        match &projection.facts[0].kind {
            SceneFactKind::Activity { body, kind, detail } => {
                // The accumulated output stays the legacy body; the row
                // presentation reads the raw command detail.
                assert_eq!(body, "test result: ok");
                assert_eq!(kind.as_deref(), Some("terminal_activity"));
                assert_eq!(detail.as_deref(), Some("cargo test --locked"));
            }
            other => panic!("expected an activity fact, got {other:?}"),
        }
    }

    #[test]
    fn timeline_activities_carry_their_tag_and_sanitized_summary() {
        let thread = ThreadId::parse("thread-timeline").expect("thread");
        let turn = TurnId::parse("turn-timeline").expect("turn");
        let run = RunId::parse("run-timeline").expect("run");
        let event = attributed_event(
            &thread,
            &run,
            &turn,
            1,
            Observation::File(
                FileObservation::new(
                    observation_id("obs-file"),
                    sequence(5),
                    String::from("src/main.rs"),
                    FileAction::Modified,
                    Some(10),
                    Some(2),
                )
                .expect("fixture file observation is valid"),
            ),
        );
        let mut state = EngineObservationState::new(thread.clone());
        let _ = state.apply(1, &event);

        let projection = project_activities(&state, &snapshot(&thread, &turn));
        assert_eq!(projection.facts.len(), 1);
        match &projection.facts[0].kind {
            SceneFactKind::Activity { body, kind, detail } => {
                assert_eq!(body, "modified src/main.rs (+10/-2)");
                assert_eq!(kind.as_deref(), Some("file"));
                assert_eq!(detail.as_deref(), Some("modified src/main.rs (+10/-2)"));
            }
            other => panic!("expected an activity fact, got {other:?}"),
        }
    }
}
