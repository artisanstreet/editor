use artisan_domain::{
    FileAction, FileObservation, OBSERVATION_COMMAND_MAX_BYTES, OBSERVATION_DELTA_MAX_BYTES,
    OBSERVATION_MESSAGE_MAX_BYTES, OBSERVATION_OUTPUT_MAX_BYTES, OBSERVATION_PLAN_MAX_ENTRIES,
    OBSERVATION_TEXT_MAX_BYTES, Observation as DomainObservation, ObservationId,
    ObservationSequence, PlanEntry, PlanEntryStatus, PlanObservation,
    ReasoningSummaryCompletedObservation, ReasoningSummaryDeltaObservation, RunId,
    SearchObservation, SearchScope, SearchState, TerminalActivityInput,
    TerminalActivityObservation, TerminalActivityState, ToolAction, ToolObservation,
};
use tokio::io::AsyncWrite;
use tokio::sync::mpsc;

#[cfg(test)]
use super::super::observation::TerminalObservation;
use super::super::observation::{EngineObservation, TerminalState, UsageObservation, chunk_text};
#[cfg(test)]
use serde_json::Value;

use super::continuation::{
    CodexUsageContext, CodexUsageScope, codex_usage_report, current_unix_millis,
};
#[cfg(test)]
use super::protocol::CODEX_MAX_ANSWERS;
use super::protocol::{
    CodexEvent, CodexFileChange, CodexFileKind, CodexPendingTracker, CodexPlanEntry,
    CodexPlanStatus, CodexSearchLifecycle, CodexTerminalLifecycle, CodexToolAction, CodexTurnError,
    CodexTurnState, request_line, write_line,
};

// ---------------------------------------------------------------------------
// Rich activity normalization (Codex visual parity)
//
// Mirrors `normalise_codex_notification` in
// `modules/engines/src/codex/normalizer.ts` for the supported subset:
// published reasoning-summary text (never hidden reasoning), tool lifecycle,
// command lifecycle plus output, file changes, web search, and plan updates.
// Each emitted [`DomainObservation`] preserves the provider item and turn
// identities verbatim inside its typed payload; the generated observation id
// is deterministic in `(run, frame sequence, emission slug)` so replays
// produce identical ids without collisions, and owner rows carry
// SOURCE-LOCAL FRAME order only (fragments may share one frame's sequence;
// the dispatcher remints the monotonic RUN-local observation sequence and
// identity while the database allocates the separate THREAD-scoped
// attribution `delivery_sequence`). No wall clocks
// are minted: none of the target observation types carry timestamps.
// ---------------------------------------------------------------------------

/// Returns whether one activity frame may emit onto the root channel.
///
/// The frame must claim exactly the pump-bound root native thread: unknown
/// foreign threads never emit even when they share the current turn, and an
/// unbound tracker emits nothing at all, so authority is never guessed from
/// the first rich frame. Turn-scoped frames must then claim exactly the
/// current turn, while an unestablished turn is adopted exactly like the
/// plain delta path so legitimate interleaved root frames before the
/// `turn/start` result still flow. Frames claiming a discovered child thread
/// are never coerced into the root channel.
fn activity_frame_authorized(
    thread_id: &str,
    turn_id: &str,
    tracker: &CodexPendingTracker,
    active_turn: &mut Option<String>,
) -> bool {
    if tracker.native_thread_id() != Some(thread_id) {
        return false;
    }
    if tracker.is_known_child_thread(thread_id) {
        return false;
    }
    match active_turn {
        Some(active) if active == turn_id => true,
        Some(_) => false,
        None => {
            *active_turn = Some(turn_id.to_owned());
            true
        }
    }
}

/// Builds the stable generated observation id for one activity emission.
///
/// Deterministic in `(run, frame sequence, slug)`: the wire supplies item
/// and turn identities but no observation identity. Returns [`None`] when a
/// provider identity violates the shared wire identifier rule or the composed
/// id exceeds its ceiling; the caller then drops the emission fail-closed
/// instead of minting an ambiguous identity.
fn activity_observation_id(
    run_id: &RunId,
    frame_sequence: u64,
    slug: &str,
) -> Option<ObservationId> {
    ObservationId::parse(format!("{}:codex:{frame_sequence}:{slug}", run_id.as_str())).ok()
}

/// Reads one source frame's order as a domain sequence.
///
/// Owner rows carry SOURCE-LOCAL FRAME order only: fragment rows from one
/// frame may share its sequence. The dispatcher remints the monotonic
/// RUN-local observation sequence and identity, and the database allocates
/// the separate THREAD-scoped attribution `delivery_sequence`. Returns
/// [`None`] past the finite range so the emission drops instead of
/// wrapping.
fn activity_sequence(frame_sequence: u64) -> Option<ObservationSequence> {
    ObservationSequence::new(frame_sequence).ok()
}

/// Splits text into lossless UTF-8-boundary fragments of at most
/// `max_bytes` bytes.
///
/// Concatenating the fragments reproduces `text` exactly. Empty text yields
/// no fragments. Mirrors the [`chunk_text`] approach for the domain delta and
/// output ceilings.
fn fragment_text(text: &str, max_bytes: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let bound = max_bytes.max(1);
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut len = 0usize;
    for (byte_idx, ch) in text.char_indices() {
        let ch_len = ch.len_utf8();
        if len + ch_len > bound {
            out.push(text[start..byte_idx].to_owned());
            start = byte_idx;
            len = 0;
        }
        len += ch_len;
    }
    if start < text.len() {
        out.push(text[start..].to_owned());
    }
    out
}

/// Returns whether `payload` is a unified diff at all, decided by a hunk
/// header: the only marker a diff cannot omit. Mirrors `IsUnifiedDiff` in
/// `modules/engines/src/patch/unified-diff.ts`.
fn is_unified_diff(payload: &str) -> bool {
    payload.lines().any(|line| line.starts_with("@@ "))
}

/// Counts the body lines a unified diff adds and removes.
///
/// File headers carry the same marker characters as content, so they are
/// skipped by their trailing space only outside a hunk; inside one the first
/// character is the marker. Mirrors `CountUnifiedDiffLines`.
fn count_unified_diff_lines(diff: &str) -> (u64, u64) {
    let mut added = 0u64;
    let mut deleted = 0u64;
    let mut in_hunk = false;
    for line in diff.split('\n') {
        if line.starts_with("@@") {
            in_hunk = true;
            continue;
        }
        if !in_hunk
            && (line.starts_with("--- ")
                || line.starts_with("+++ ")
                || line.starts_with("diff ")
                || line.starts_with("index "))
        {
            continue;
        }
        if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            deleted += 1;
        }
    }
    (added, deleted)
}

/// Counts the lines of a whole-file write. Mirrors `CountWrittenLines`.
fn count_written_lines(text: &str) -> u64 {
    if text.is_empty() {
        0
    } else {
        text.split('\n').count() as u64
    }
}

/// Counts one reported file change whose payload may be either a unified
/// diff or the file's own content.
///
/// Returns `(None, None)` when the payload is content but the operation
/// modified an existing file: the text says what the file now holds and
/// nothing about what it replaced, so any count would be invented. Absent
/// stays absent rather than becoming zero. Mirrors `CountFileChangeLines`.
fn count_file_change_lines(action: FileAction, payload: &str) -> (Option<u64>, Option<u64>) {
    if is_unified_diff(payload) {
        let (added, deleted) = count_unified_diff_lines(payload);
        return (Some(added), Some(deleted));
    }
    match action {
        FileAction::Created => (Some(count_written_lines(payload)), Some(0)),
        FileAction::Deleted => (Some(0), Some(count_written_lines(payload))),
        _ => (None, None),
    }
}

fn reasoning_delta_rows(
    run_id: &RunId,
    frame_sequence: u64,
    item_id: &str,
    turn_id: &str,
    summary_index: u64,
    delta: &str,
) -> Vec<DomainObservation> {
    let Some(sequence) = activity_sequence(frame_sequence) else {
        return Vec::new();
    };
    fragment_text(delta, OBSERVATION_DELTA_MAX_BYTES)
        .into_iter()
        .enumerate()
        .filter_map(|(index, part)| {
            let id = activity_observation_id(
                run_id,
                frame_sequence,
                &format!("rsum:{item_id}:{summary_index}:{index}"),
            )?;
            let item = ObservationId::parse(item_id.to_owned()).ok()?;
            let turn = ObservationId::parse(turn_id.to_owned()).ok()?;
            ReasoningSummaryDeltaObservation::new(
                id,
                sequence,
                item,
                summary_index,
                part,
                None,
                turn,
            )
            .ok()
            .map(DomainObservation::ReasoningSummaryDelta)
        })
        .collect()
}

fn reasoning_boundary_rows(
    run_id: &RunId,
    frame_sequence: u64,
    item_id: &str,
    turn_id: &str,
    summary_index: u64,
) -> Vec<DomainObservation> {
    let Some(sequence) = activity_sequence(frame_sequence) else {
        return Vec::new();
    };
    let Some(id) = activity_observation_id(
        run_id,
        frame_sequence,
        &format!("rsum:{item_id}:{summary_index}:sep"),
    ) else {
        return Vec::new();
    };
    let (Some(item), Some(turn)) = (
        ObservationId::parse(item_id.to_owned()).ok(),
        ObservationId::parse(turn_id.to_owned()).ok(),
    ) else {
        return Vec::new();
    };
    ReasoningSummaryDeltaObservation::new(
        id,
        sequence,
        item,
        summary_index,
        "\n\n".to_owned(),
        None,
        turn,
    )
    .ok()
    .map(DomainObservation::ReasoningSummaryDelta)
    .into_iter()
    .collect()
}

fn reasoning_settled_rows(
    run_id: &RunId,
    frame_sequence: u64,
    item_id: &str,
    turn_id: &str,
    text: Option<&str>,
) -> Vec<DomainObservation> {
    let Some(sequence) = activity_sequence(frame_sequence) else {
        return Vec::new();
    };
    // An oversize authoritative summary still settles its phase, but without
    // invented truncation: the text is omitted rather than cut.
    let text = text
        .filter(|text| !text.is_empty() && text.len() <= OBSERVATION_MESSAGE_MAX_BYTES)
        .map(str::to_owned);
    let Some(id) = activity_observation_id(run_id, frame_sequence, &format!("rsc:{item_id}"))
    else {
        return Vec::new();
    };
    let (Some(item), Some(turn)) = (
        ObservationId::parse(item_id.to_owned()).ok(),
        ObservationId::parse(turn_id.to_owned()).ok(),
    ) else {
        return Vec::new();
    };
    ReasoningSummaryCompletedObservation::new(id, sequence, item, text, turn)
        .ok()
        .map(DomainObservation::ReasoningSummaryCompleted)
        .into_iter()
        .collect()
}

fn tool_rows(
    run_id: &RunId,
    frame_sequence: u64,
    item_id: &str,
    tool_name: &str,
    action: CodexToolAction,
    detail: Option<&str>,
) -> Vec<DomainObservation> {
    let Some(sequence) = activity_sequence(frame_sequence) else {
        return Vec::new();
    };
    let action = match action {
        CodexToolAction::Started => ToolAction::Started,
        CodexToolAction::Progress => ToolAction::Progress,
        CodexToolAction::Completed => ToolAction::Completed,
        CodexToolAction::Failed => ToolAction::Failed,
    };
    // An oversize progress message is omitted while the lifecycle step is
    // preserved; the detail is optional evidence, never the step itself.
    let detail = detail
        .filter(|detail| !detail.is_empty() && detail.len() <= OBSERVATION_TEXT_MAX_BYTES)
        .map(str::to_owned);
    let Some(id) = activity_observation_id(
        run_id,
        frame_sequence,
        &format!("tool:{item_id}:{}", action.as_str()),
    ) else {
        return Vec::new();
    };
    let Some(tool_id) = ObservationId::parse(item_id.to_owned()).ok() else {
        return Vec::new();
    };
    ToolObservation::new(id, sequence, tool_id, tool_name.to_owned(), action, detail)
        .ok()
        .map(DomainObservation::Tool)
        .into_iter()
        .collect()
}

fn terminal_output_rows(
    run_id: &RunId,
    frame_sequence: u64,
    item_id: &str,
    _turn_id: &str,
    delta: &str,
) -> Vec<DomainObservation> {
    let Some(sequence) = activity_sequence(frame_sequence) else {
        return Vec::new();
    };
    fragment_text(delta, OBSERVATION_OUTPUT_MAX_BYTES)
        .into_iter()
        .enumerate()
        .filter_map(|(index, output)| {
            let id = activity_observation_id(
                run_id,
                frame_sequence,
                &format!("term:{item_id}:out:{index}"),
            )?;
            let activity_id = ObservationId::parse(item_id.to_owned()).ok()?;
            TerminalActivityObservation::new(
                id,
                sequence,
                TerminalActivityInput {
                    activity_id,
                    channel: None,
                    command: None,
                    shell: None,
                    output: Some(output),
                    exit_code: None,
                    state: TerminalActivityState::Output,
                },
            )
            .ok()
            .map(DomainObservation::TerminalActivity)
        })
        .collect()
}

fn terminal_lifecycle_rows(
    run_id: &RunId,
    frame_sequence: u64,
    item_id: &str,
    command: Option<&str>,
    output: Option<&str>,
    exit_code: Option<i32>,
    state: CodexTerminalLifecycle,
) -> Vec<DomainObservation> {
    let Some(sequence) = activity_sequence(frame_sequence) else {
        return Vec::new();
    };
    let state = match state {
        CodexTerminalLifecycle::Started => TerminalActivityState::Started,
        CodexTerminalLifecycle::Completed => TerminalActivityState::Completed,
        CodexTerminalLifecycle::Failed => TerminalActivityState::Failed,
    };
    // Oversize command or aggregated output is omitted while the lifecycle
    // step is preserved; neither bound is stretched by truncation.
    let command = command
        .filter(|command| !command.is_empty() && command.len() <= OBSERVATION_COMMAND_MAX_BYTES)
        .map(str::to_owned);
    let output = output
        .filter(|output| !output.is_empty() && output.len() <= OBSERVATION_OUTPUT_MAX_BYTES)
        .map(str::to_owned);
    let Some(id) = activity_observation_id(
        run_id,
        frame_sequence,
        &format!("term:{item_id}:{}", state.as_str()),
    ) else {
        return Vec::new();
    };
    let Some(activity_id) = ObservationId::parse(item_id.to_owned()).ok() else {
        return Vec::new();
    };
    TerminalActivityObservation::new(
        id,
        sequence,
        TerminalActivityInput {
            activity_id,
            channel: None,
            command,
            shell: None,
            output,
            exit_code,
            state,
        },
    )
    .ok()
    .map(DomainObservation::TerminalActivity)
    .into_iter()
    .collect()
}

fn file_completed_rows(
    run_id: &RunId,
    frame_sequence: u64,
    item_id: &str,
    changes: &[CodexFileChange],
) -> Vec<DomainObservation> {
    let Some(sequence) = activity_sequence(frame_sequence) else {
        return Vec::new();
    };
    changes
        .iter()
        .enumerate()
        .filter_map(|(index, change)| {
            let action = match change.kind {
                CodexFileKind::Add => FileAction::Created,
                CodexFileKind::Delete => FileAction::Deleted,
                CodexFileKind::Update => FileAction::Modified,
            };
            let (lines_added, lines_deleted) = count_file_change_lines(action, &change.diff);
            let id = activity_observation_id(
                run_id,
                frame_sequence,
                &format!("file:{item_id}:{index}"),
            )?;
            FileObservation::new(
                id,
                sequence,
                change.path.clone(),
                action,
                lines_added,
                lines_deleted,
            )
            .ok()
            .map(DomainObservation::File)
        })
        .collect()
}

fn search_rows(
    run_id: &RunId,
    frame_sequence: u64,
    item_id: &str,
    query: &str,
    state: CodexSearchLifecycle,
) -> Vec<DomainObservation> {
    let Some(sequence) = activity_sequence(frame_sequence) else {
        return Vec::new();
    };
    let state = match state {
        CodexSearchLifecycle::Started => SearchState::Started,
        CodexSearchLifecycle::Completed => SearchState::Completed,
    };
    let Some(id) = activity_observation_id(
        run_id,
        frame_sequence,
        &format!("search:{item_id}:{}", state.as_str()),
    ) else {
        return Vec::new();
    };
    let Some(search_id) = ObservationId::parse(item_id.to_owned()).ok() else {
        return Vec::new();
    };
    SearchObservation::new(
        id,
        sequence,
        query.to_owned(),
        Some(SearchScope::Web),
        Some(search_id),
        state,
        None,
    )
    .ok()
    .map(DomainObservation::Search)
    .into_iter()
    .collect()
}

fn plan_rows(
    run_id: &RunId,
    frame_sequence: u64,
    turn_id: &str,
    entries: &[CodexPlanEntry],
) -> Vec<DomainObservation> {
    let Some(sequence) = activity_sequence(frame_sequence) else {
        return Vec::new();
    };
    let Some(turn) = ObservationId::parse(turn_id.to_owned()).ok() else {
        return Vec::new();
    };
    // An entry list the durable plan cannot hold fails closed as a unit
    // rather than persisting a truncated plan as the whole.
    if entries.is_empty() || entries.len() > OBSERVATION_PLAN_MAX_ENTRIES {
        return Vec::new();
    }
    let mut built = Vec::with_capacity(entries.len());
    for entry in entries {
        let Ok(id) = ObservationId::parse(entry.id.clone()) else {
            return Vec::new();
        };
        let status = match entry.status {
            CodexPlanStatus::Pending => PlanEntryStatus::Pending,
            CodexPlanStatus::InProgress => PlanEntryStatus::InProgress,
            CodexPlanStatus::Completed => PlanEntryStatus::Completed,
        };
        match PlanEntry::new(id, status, entry.text.clone()) {
            Ok(entry) => built.push(entry),
            Err(_) => return Vec::new(),
        }
    }
    let Some(id) = activity_observation_id(run_id, frame_sequence, "plan") else {
        return Vec::new();
    };
    PlanObservation::new(id, sequence, built, Some(turn))
        .ok()
        .map(DomainObservation::Plan)
        .into_iter()
        .collect()
}

/// Sends validated activity rows through the bounded observation channel.
///
/// Backpressure semantics match the plain delta path: a closed sink ends the
/// turn as interrupted.
async fn emit_activity(
    observations: &mpsc::Sender<EngineObservation>,
    rows: Vec<DomainObservation>,
) -> Option<TerminalState> {
    for row in rows {
        if observations
            .send(EngineObservation::Activity(row))
            .await
            .is_err()
        {
            return Some(TerminalState::Interrupted);
        }
    }
    None
}

/// Applies one typed event; returns the terminal state when the turn ends.
///
/// Token-usage frames project best-effort onto the shared usage vocabulary
/// when a usage scope travels with the pump; without one (or on any
/// attribution failure) they are diagnostics that never disturb the turn.
/// Usage collection never blocks turns: only the observation sink closing is
/// terminal.
#[expect(
    clippy::too_many_lines,
    reason = "single event dispatch table projecting typed events; extracting arms would thread the same sink and tracker"
)]
pub(crate) async fn apply_event(
    event: CodexEvent,
    run_id: &RunId,
    tracker: &mut CodexPendingTracker,
    active_turn: &mut Option<String>,
    observations: &mpsc::Sender<EngineObservation>,
    frame_sequence: u64,
    usage: Option<&CodexUsageScope<'_>>,
) -> Option<TerminalState> {
    match event {
        CodexEvent::AgentMessageDelta {
            item_id,
            turn_id,
            delta,
        } => {
            if active_turn.is_none() {
                *active_turn = Some(turn_id.clone());
            }
            let native_id = format!("codex:{frame_sequence}");
            for chunk in chunk_text(run_id, frame_sequence, &native_id, &delta) {
                let part_item = item_id.clone();
                let chunk = chunk.with_part_id(part_item);
                if observations
                    .send(EngineObservation::TextDelta(chunk))
                    .await
                    .is_err()
                {
                    return Some(TerminalState::Interrupted);
                }
            }
            None
        }
        CodexEvent::TurnState { turn_id, state } => {
            *active_turn = Some(turn_id);
            match state {
                CodexTurnState::Completed => Some(TerminalState::Completed),
                CodexTurnState::Failed => Some(TerminalState::Failed),
                CodexTurnState::Cancelled => Some(TerminalState::Cancelled),
                CodexTurnState::Started => None,
            }
        }
        CodexEvent::TokenUsage { turn_id, sample } => {
            let scope = usage?;
            let observed_at = current_unix_millis()?;
            let report = codex_usage_report(
                &CodexUsageContext {
                    run_id,
                    thread_id: scope.thread_id,
                    provider_session_id: scope.provider_session_id,
                    model_id: scope.model_id,
                    observed_at,
                },
                Some(turn_id),
                frame_sequence,
                &sample,
            );
            let report = report?;
            if observations
                .send(EngineObservation::Usage(UsageObservation::new(report)))
                .await
                .is_err()
            {
                return Some(TerminalState::Interrupted);
            }
            None
        }
        CodexEvent::ApprovalRequested(request) => {
            tracker.note_approval(request);
            None
        }
        CodexEvent::QuestionRequested(request) => {
            tracker.note_questions(&request);
            None
        }
        CodexEvent::SubagentDiscovered {
            agent_thread_id,
            parent_thread_id,
        } => {
            // Discovery only: never adopt the root turn, never emit root text.
            tracker.note_subagent(&agent_thread_id, &parent_thread_id);
            None
        }
        CodexEvent::ReasoningSummaryDelta {
            thread_id,
            turn_id,
            item_id,
            summary_index,
            delta,
        } => {
            if !activity_frame_authorized(&thread_id, &turn_id, tracker, active_turn) {
                return None;
            }
            let rows = reasoning_delta_rows(
                run_id,
                frame_sequence,
                &item_id,
                &turn_id,
                summary_index,
                &delta,
            );
            emit_activity(observations, rows).await
        }
        CodexEvent::ReasoningSummaryBoundary {
            thread_id,
            turn_id,
            item_id,
            summary_index,
        } => {
            if !activity_frame_authorized(&thread_id, &turn_id, tracker, active_turn) {
                return None;
            }
            let rows =
                reasoning_boundary_rows(run_id, frame_sequence, &item_id, &turn_id, summary_index);
            emit_activity(observations, rows).await
        }
        CodexEvent::ReasoningSettled {
            thread_id,
            turn_id,
            item_id,
            text,
        } => {
            if !activity_frame_authorized(&thread_id, &turn_id, tracker, active_turn) {
                return None;
            }
            let rows =
                reasoning_settled_rows(run_id, frame_sequence, &item_id, &turn_id, text.as_deref());
            emit_activity(observations, rows).await
        }
        CodexEvent::ToolLifecycle {
            thread_id,
            turn_id,
            item_id,
            tool_name,
            action,
            detail,
        } => {
            if !activity_frame_authorized(&thread_id, &turn_id, tracker, active_turn) {
                return None;
            }
            let rows = tool_rows(
                run_id,
                frame_sequence,
                &item_id,
                &tool_name,
                action,
                detail.as_deref(),
            );
            emit_activity(observations, rows).await
        }
        CodexEvent::TerminalOutput {
            thread_id,
            turn_id,
            item_id,
            delta,
        } => {
            if !activity_frame_authorized(&thread_id, &turn_id, tracker, active_turn) {
                return None;
            }
            let rows = terminal_output_rows(run_id, frame_sequence, &item_id, &turn_id, &delta);
            emit_activity(observations, rows).await
        }
        CodexEvent::TerminalLifecycle {
            thread_id,
            turn_id,
            item_id,
            command,
            output,
            exit_code,
            state,
        } => {
            if !activity_frame_authorized(&thread_id, &turn_id, tracker, active_turn) {
                return None;
            }
            let rows = terminal_lifecycle_rows(
                run_id,
                frame_sequence,
                &item_id,
                command.as_deref(),
                output.as_deref(),
                exit_code,
                state,
            );
            emit_activity(observations, rows).await
        }
        CodexEvent::FileCompleted {
            thread_id,
            turn_id,
            item_id,
            changes,
        } => {
            if !activity_frame_authorized(&thread_id, &turn_id, tracker, active_turn) {
                return None;
            }
            let rows = file_completed_rows(run_id, frame_sequence, &item_id, &changes);
            emit_activity(observations, rows).await
        }
        CodexEvent::SearchLifecycle {
            thread_id,
            turn_id,
            item_id,
            query,
            state,
        } => {
            if !activity_frame_authorized(&thread_id, &turn_id, tracker, active_turn) {
                return None;
            }
            let rows = search_rows(run_id, frame_sequence, &item_id, &query, state);
            emit_activity(observations, rows).await
        }
        CodexEvent::PlanUpdated {
            thread_id,
            turn_id,
            entries,
        } => {
            if !activity_frame_authorized(&thread_id, &turn_id, tracker, active_turn) {
                return None;
            }
            let rows = plan_rows(run_id, frame_sequence, &turn_id, &entries);
            emit_activity(observations, rows).await
        }
        CodexEvent::ActivitySilent
        | CodexEvent::ThreadClosed
        | CodexEvent::OptedOut
        | CodexEvent::UnknownMethod => None,
    }
}

/// Steers a live turn with follow-up text (`turn/steer`).
///
/// Production verb behind [`AcceptedTurn::steer_text`](super::operation::AcceptedTurn::steer_text):
/// the pump writes the follow-up with the actual provider turn id as
/// `expectedTurnId` and the correlated `turn/steer` result resolves the
/// delivery; a correlated error reply resolves it failed without settling
/// the turn. Proves the follow-up verb against the fixture stdio script
/// without disturbing the authorize-once production flow.
///
/// # Errors
///
/// Returns [`CodexTurnError`] when the write fails.
pub(crate) async fn steer_live_turn<W: AsyncWrite + Unpin>(
    stdin: &mut W,
    request_id: &mut u64,
    thread_id: &str,
    turn_id: &str,
    text: &str,
) -> Result<(), CodexTurnError> {
    let params = serde_json::json!({
        "expectedTurnId": turn_id,
        "input": [{ "text": text, "text_elements": [], "type": "text" }],
        "threadId": thread_id,
    });
    let line = request_line(*request_id, "turn/steer", &params);
    *request_id += 1;
    write_line(stdin, &line).await
}

/// Interrupts a live turn (`turn/interrupt`) before cancelling the driver.
pub(crate) async fn interrupt_live_turn<W: AsyncWrite + Unpin>(
    stdin: &mut W,
    request_id: &mut u64,
    thread_id: &str,
    turn_id: &str,
) -> Result<(), CodexTurnError> {
    let params = serde_json::json!({ "threadId": thread_id, "turnId": turn_id });
    let line = request_line(*request_id, "turn/interrupt", &params);
    // The pump's cancellation path issues this interrupt with a u64::MAX
    // sentinel id (operation.rs). Saturate so a debug build cannot panic on
    // overflow and a release build cannot wrap to 0 and collide with
    // handshake ids; reusing MAX across repeated cancels is harmless.
    *request_id = request_id.saturating_add(1);
    write_line(stdin, &line).await
}

/// Answers one pending approval through the durable decision.
///
/// Test-only until dispatcher delivery wiring lands: deny carries no turn
/// side effect and the run continues either way.
///
/// # Errors
///
/// Returns [`CodexTurnError::Configuration`] for an unknown or resolved
/// target and [`CodexTurnError::StreamFailed`] when the write fails.
#[cfg(test)]
pub(crate) async fn answer_approval<W: AsyncWrite + Unpin>(
    stdin: &mut W,
    tracker: &mut CodexPendingTracker,
    native_id: &str,
    approval_id: &str,
    approved: bool,
) -> Result<(), CodexTurnError> {
    if !tracker.resolve_approval(approval_id) {
        return Err(CodexTurnError::Configuration);
    }
    let decision = if approved { "approved" } else { "denied" };
    let line =
        serde_json::json!({ "id": native_id, "result": { "decision": decision } }).to_string();
    write_line(stdin, &line).await
}

/// Answers one question request group through the durable answers.
///
/// Test-only until dispatcher delivery wiring lands.
///
/// # Errors
///
/// Returns [`CodexTurnError::Configuration`] for an unknown or resolved
/// target and [`CodexTurnError::StreamFailed`] when the write fails.
#[cfg(test)]
pub(crate) async fn answer_questions<W: AsyncWrite + Unpin>(
    stdin: &mut W,
    tracker: &mut CodexPendingTracker,
    native_id: &str,
    answers: &[(String, Vec<String>)],
) -> Result<(), CodexTurnError> {
    for (question_id, _) in answers {
        if !tracker.resolve_question(question_id) {
            return Err(CodexTurnError::Configuration);
        }
    }
    let mut map = serde_json::Map::new();
    for (question_id, options) in answers.iter().take(CODEX_MAX_ANSWERS) {
        map.insert(
            question_id.clone(),
            serde_json::json!({ "answers": options }),
        );
    }
    let line = serde_json::json!({ "id": native_id, "result": { "answers": Value::Object(map) } })
        .to_string();
    write_line(stdin, &line).await
}

/// Classifies a reaped child exit after close.
///
/// Test-only until the close path reports it: code 0 is a clean close and
/// nonzero is a provider failure. Cancellation and interruption are reported
/// by the driver, never inferred from the code.
#[cfg(test)]
pub(crate) fn classify_exit(status: std::process::ExitStatus) -> TerminalState {
    if status.success() {
        TerminalState::Completed
    } else {
        TerminalState::Failed
    }
}

/// Builds a terminal observation preserving caller identity and state.
///
/// Test-only observation helper for the fixture lifecycle assertions.
#[cfg(test)]
pub(crate) fn terminal_observation(
    run_id: &RunId,
    sequence: u64,
    state: TerminalState,
) -> TerminalObservation {
    TerminalObservation::new(run_id.clone(), sequence, state, None, None)
}
