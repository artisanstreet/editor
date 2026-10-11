use artisan_domain::{
    CompactionObservation, CompactionState, FileAction, FileObservation, ImageObservation,
    ImageOrigin, OBSERVATION_COMMAND_MAX_BYTES, OBSERVATION_DELTA_MAX_BYTES,
    OBSERVATION_DURATION_MAX_MILLIS, OBSERVATION_MESSAGE_MAX_BYTES, OBSERVATION_OUTPUT_MAX_BYTES,
    OBSERVATION_PLAN_MAX_ENTRIES, OBSERVATION_TEXT_MAX_BYTES, Observation as DomainObservation,
    ObservationId, ObservationSequence, PlanEntry, PlanEntryStatus, PlanObservation,
    ReasoningSummaryCompletedObservation, ReasoningSummaryDeltaObservation, RunId,
    SearchObservation, SearchScope, SearchState, TerminalActivityInput,
    TerminalActivityObservation, TerminalActivityState, ToolAction, ToolObservation,
};
use tokio::io::AsyncWrite;
use tokio::sync::mpsc;

#[cfg(test)]
use super::super::observation::TerminalObservation;
use super::super::observation::{
    EngineObservation, ShellJobChange, ShellJobEnd, ShellJobRow, TerminalState, UsageObservation,
    chunk_text,
};
use super::compaction::CodexCompactionStep;
use super::continuation::{
    CodexUsageContext, CodexUsageScope, codex_usage_report, current_unix_millis,
};
use super::protocol::{
    CodexEvent, CodexFileChange, CodexFileKind, CodexImage, CodexPendingTracker, CodexPlanEntry,
    CodexPlanStatus, CodexQuestion, CodexSafety, CodexSearchLifecycle, CodexSubagentRow,
    CodexTerminalLifecycle, CodexToolAction, CodexTurnError, CodexTurnState, prompt_input,
    request_line, write_line,
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

/// Builds the requested question rows for newly asked questions, each in its
/// questionnaire. A question that fails the domain bounds drops fail-closed.
fn question_rows(
    run_id: &RunId,
    frame_sequence: u64,
    questions: &[CodexQuestion],
) -> Vec<DomainObservation> {
    let Some(sequence) = activity_sequence(frame_sequence) else {
        return Vec::new();
    };
    questions
        .iter()
        .enumerate()
        .filter_map(|(index, question)| {
            let id = activity_observation_id(run_id, frame_sequence, &format!("question-{index}"))?;
            let group = ObservationId::parse(question.group_id().to_owned()).ok()?;
            let input = question.to_domain_input().ok()?;
            artisan_domain::QuestionObservation::requested(id, sequence, input)
                .ok()
                .map(|row| DomainObservation::Question(row.with_group(group)))
        })
        .collect()
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

/// Builds the agent rows for the root agent's subagents, one tool row per
/// subagent thread, named as Codex named the agent.
fn subagent_rows(
    run_id: &RunId,
    frame_sequence: u64,
    rows: impl IntoIterator<Item = CodexSubagentRow>,
) -> Vec<DomainObservation> {
    rows.into_iter()
        .flat_map(|row| {
            tool_rows(
                run_id,
                frame_sequence,
                &row.thread_id,
                "subagent",
                row.action,
                row.name.as_deref(),
            )
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

/// Builds the image row for one generated Codex image. An image the engine
/// did not save is written to the Forge's image store first; an image that
/// cannot be placed on disk shows nothing.
fn image_rows(
    run_id: &RunId,
    frame_sequence: u64,
    item_id: &str,
    image: CodexImage,
) -> Vec<DomainObservation> {
    let Some(sequence) = activity_sequence(frame_sequence) else {
        return Vec::new();
    };
    let CodexImage {
        saved_path,
        base64,
        revised_prompt,
    } = image;
    let Some(path) = saved_path.or_else(|| {
        base64.and_then(|payload| {
            super::super::engine_image::persist_inline_image("codex", item_id, &payload)
        })
    }) else {
        return Vec::new();
    };
    let caption = revised_prompt
        .as_deref()
        .and_then(super::super::engine_image::bounded_caption);
    let Some(id) = activity_observation_id(run_id, frame_sequence, &format!("image:{item_id}"))
    else {
        return Vec::new();
    };
    let Ok(image_item) = ObservationId::parse(item_id.to_owned()) else {
        return Vec::new();
    };
    ImageObservation::new(
        id,
        sequence,
        image_item,
        ImageOrigin::Generated,
        path,
        caption,
    )
    .ok()
    .map(DomainObservation::Image)
    .into_iter()
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

/// Builds the compaction row for one paired compaction step.
///
/// Started and Completed rows of one compaction share a run-scoped
/// `compaction_id` derived from the provider item; a completion reported only
/// by `thread/compacted` carries its own frame-scoped identity. Codex reports
/// no token counts for a compaction, so the summary stays empty.
fn compaction_rows(
    run_id: &RunId,
    frame_sequence: u64,
    step: CodexCompactionStep,
) -> Vec<DomainObservation> {
    let Some(sequence) = activity_sequence(frame_sequence) else {
        return Vec::new();
    };
    let (state, item_id, duration_ms) = match step {
        CodexCompactionStep::Started { item_id } => (CompactionState::Started, Some(item_id), None),
        CodexCompactionStep::Completed {
            item_id,
            duration_ms,
        } => (CompactionState::Completed, item_id, duration_ms),
        CodexCompactionStep::Duplicate => return Vec::new(),
    };
    let (slug, compaction_key) = match &item_id {
        Some(item_id) => (
            format!("compaction:{item_id}:{}", state.as_str()),
            item_id.clone(),
        ),
        None => (
            format!("compaction:{}", state.as_str()),
            frame_sequence.to_string(),
        ),
    };
    let Some(id) = activity_observation_id(run_id, frame_sequence, &slug) else {
        return Vec::new();
    };
    let Ok(compaction_id) = ObservationId::parse(format!(
        "{}:codex:compaction:{compaction_key}",
        run_id.as_str()
    )) else {
        return Vec::new();
    };
    let duration_ms = duration_ms.filter(|duration| *duration <= OBSERVATION_DURATION_MAX_MILLIS);
    CompactionObservation::new(id, sequence, state, Some(compaction_id), duration_ms, None)
        .ok()
        .map(DomainObservation::Compaction)
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

/// Sends one shell job change to the thread's terminal board.
async fn emit_shell(
    observations: &mpsc::Sender<EngineObservation>,
    row: ShellJobRow,
) -> Option<TerminalState> {
    observations
        .send(EngineObservation::Shell(row))
        .await
        .is_err()
        .then_some(TerminalState::Interrupted)
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
        CodexEvent::AgentMessageSnapshot {
            thread_id,
            item_id,
            text,
            phase,
        } => {
            if tracker.native_thread_id() != Some(thread_id.as_str()) {
                return None;
            }
            let text = crate::citation_projection::resolve_message(&thread_id, text).await;
            let snapshot = super::super::observation::TextSnapshot::new(
                run_id.clone(),
                frame_sequence,
                item_id,
                text,
            )
            .with_phase(phase);
            if observations
                .send(EngineObservation::TextSnapshot(snapshot))
                .await
                .is_err()
            {
                return Some(TerminalState::Interrupted);
            }
            None
        }
        CodexEvent::ThreadName { thread_id, title } => {
            if tracker.native_thread_id() == Some(thread_id.as_str()) {
                let _ = observations
                    .send(EngineObservation::SummaryTitle {
                        run_id: run_id.clone(),
                        title,
                    })
                    .await;
            }
            None
        }

        CodexEvent::AgentMessageDelta {
            thread_id,
            item_id,
            turn_id,
            delta,
        } => {
            // A subagent's words are its own, never the root reply.
            if !tracker.claims_root(thread_id.as_deref()) {
                return None;
            }
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
        CodexEvent::TurnState {
            thread_id,
            turn_id,
            state,
            refusal,
        } => {
            // A subagent's turn says where that agent stands; it never
            // starts or settles the root turn.
            if let Some(child) = thread_id.filter(|child| !tracker.claims_root(Some(child))) {
                let action = match state {
                    CodexTurnState::Started => CodexToolAction::Progress,
                    CodexTurnState::Completed => CodexToolAction::Completed,
                    CodexTurnState::Failed | CodexTurnState::Cancelled => CodexToolAction::Failed,
                };
                let row = tracker.advance_subagent(&child, action);
                return emit_activity(observations, subagent_rows(run_id, frame_sequence, row))
                    .await;
            }
            // A refusal is said before the turn settles, so the reader
            // learns why it failed.
            if let Some(refusal) = refusal
                && tracker.observe_safety(&turn_id, &refusal)
                && let Some(interrupted) =
                    emit_activity(observations, safety_rows(run_id, frame_sequence, &refusal)).await
            {
                return Some(interrupted);
            }
            *active_turn = Some(turn_id);
            match state {
                CodexTurnState::Completed => Some(TerminalState::Completed),
                CodexTurnState::Failed => Some(TerminalState::Failed),
                CodexTurnState::Cancelled => Some(TerminalState::Cancelled),
                CodexTurnState::Started => None,
            }
        }
        CodexEvent::TokenUsage {
            thread_id,
            turn_id,
            sample,
        } => {
            if !tracker.claims_root(thread_id.as_deref()) {
                return None;
            }
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
            let id = activity_observation_id(run_id, frame_sequence, "approval");
            let sequence = artisan_domain::ObservationSequence::new(frame_sequence);
            let target = artisan_domain::ObservationId::parse(request.approval_id().to_owned());
            let domain = request.to_domain_request();
            let description = request.description().to_owned();
            let (Some(id), Ok(sequence), Ok(target), Ok(domain)) = (id, sequence, target, domain)
            else {
                return None;
            };
            let Ok(row) = artisan_domain::ApprovalObservation::requested(
                id,
                sequence,
                target,
                description,
                domain,
            ) else {
                return None;
            };
            if !tracker.note_approval(request) {
                return None;
            }
            emit_activity(observations, vec![DomainObservation::Approval(row)]).await
        }
        CodexEvent::QuestionRequested(request) => {
            // The request stays open in the tracker until the user answers
            // every question; the questions present as one questionnaire.
            let asked = tracker.note_question_request(&request);
            emit_activity(observations, question_rows(run_id, frame_sequence, &asked)).await
        }
        CodexEvent::AsyncQuestionsRequested { thread_id, request } => {
            // The same questionnaire, answered by a steered user message
            // while the run lasts; a child thread's questions are not ours
            // to answer.
            if tracker.native_thread_id() != Some(thread_id.as_str()) {
                return None;
            }
            let asked = tracker.note_question_request(&request);
            emit_activity(observations, question_rows(run_id, frame_sequence, &asked)).await
        }
        CodexEvent::SubagentDiscovered {
            agent_thread_id,
            parent_thread_id,
            name,
        } => {
            // Never adopt the root turn, never emit root text.
            let row = tracker.note_subagent(&agent_thread_id, &parent_thread_id, name.as_deref());
            emit_activity(observations, subagent_rows(run_id, frame_sequence, row)).await
        }
        CodexEvent::AgentCall {
            thread_id,
            turn_id,
            spawn,
            receivers,
            states,
        } => {
            if !activity_frame_authorized(&thread_id, &turn_id, tracker, active_turn) {
                return None;
            }
            let mut rows = Vec::new();
            if spawn {
                for receiver in &receivers {
                    rows.extend(tracker.note_subagent(receiver, &thread_id, None));
                }
            }
            for (agent, action) in states {
                rows.extend(tracker.advance_subagent(&agent, action));
            }
            emit_activity(observations, subagent_rows(run_id, frame_sequence, rows)).await
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
            // Output streams to the thread's terminal board, never the
            // ledger; the completed item carries the bounded aggregate.
            let row = ShellJobRow {
                run_id: run_id.clone(),
                job_id: item_id,
                change: ShellJobChange::Output(delta.into_bytes()),
            };
            emit_shell(observations, row).await
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
            let change = match state {
                CodexTerminalLifecycle::Started => ShellJobChange::Opened {
                    command: command.clone(),
                    background: false,
                },
                CodexTerminalLifecycle::Completed => {
                    ShellJobChange::Settled(ShellJobEnd::Exited(exit_code))
                }
                CodexTerminalLifecycle::Failed => ShellJobChange::Settled(match exit_code {
                    Some(code) => ShellJobEnd::Exited(Some(code)),
                    None => ShellJobEnd::Failed,
                }),
            };
            let job = ShellJobRow {
                run_id: run_id.clone(),
                job_id: item_id.clone(),
                change,
            };
            if let Some(state) = emit_shell(observations, job).await {
                return Some(state);
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
        CodexEvent::ImageShown {
            thread_id,
            turn_id,
            item_id,
            image,
        } => {
            if !activity_frame_authorized(&thread_id, &turn_id, tracker, active_turn) {
                return None;
            }
            let rows = image_rows(run_id, frame_sequence, &item_id, image);
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
        CodexEvent::Compaction {
            thread_id,
            turn_id,
            signal,
        } => {
            // Compaction runs inside the turn and never settles it.
            if !activity_frame_authorized(&thread_id, &turn_id, tracker, active_turn) {
                return None;
            }
            let step = tracker.compactions_mut().observe(&turn_id, signal);
            emit_activity(observations, compaction_rows(run_id, frame_sequence, step)).await
        }
        CodexEvent::Goal {
            thread_id,
            state,
            objective,
        } => {
            // Only the root thread's goal; it may arrive between turns, so it
            // claims no turn.
            if tracker.native_thread_id() != Some(thread_id.as_str())
                || tracker.is_known_child_thread(&thread_id)
            {
                return None;
            }
            emit_activity(
                observations,
                goal_rows(run_id, frame_sequence, state, objective),
            )
            .await
        }
        CodexEvent::Safety {
            thread_id,
            turn_id,
            safety,
        } => {
            if !activity_frame_authorized(&thread_id, &turn_id, tracker, active_turn)
                || !tracker.observe_safety(&turn_id, &safety)
            {
                return None;
            }
            emit_activity(observations, safety_rows(run_id, frame_sequence, &safety)).await
        }
        CodexEvent::McpServerStatus { name, status } => {
            // Best effort: a full channel drops the update, never the turn.
            let _ = observations.try_send(EngineObservation::Inventory(
                crate::engine_inventory::InventoryReport {
                    skills: None,
                    connectors: crate::engine_inventory::ConnectorReport::Update(
                        artisan_domain::InventoryConnector {
                            name,
                            status,
                            plugin: None,
                        },
                    ),
                },
            ));
            None
        }
        CodexEvent::ActivitySilent
        | CodexEvent::ThreadClosed
        | CodexEvent::OptedOut
        | CodexEvent::UnknownMethod => None,
    }
}

/// Builds one goal report row.
fn goal_rows(
    run_id: &RunId,
    frame_sequence: u64,
    state: artisan_domain::GoalState,
    objective: Option<String>,
) -> Vec<DomainObservation> {
    let Some(sequence) = activity_sequence(frame_sequence) else {
        return Vec::new();
    };
    let Some(id) =
        activity_observation_id(run_id, frame_sequence, &format!("goal:{}", state.as_str()))
    else {
        return Vec::new();
    };
    artisan_domain::GoalObservation::new(id, sequence, state, objective, None)
        .map(|goal| vec![DomainObservation::Goal(goal)])
        .unwrap_or_default()
}

/// Builds one safety report row.
fn safety_rows(
    run_id: &RunId,
    frame_sequence: u64,
    safety: &CodexSafety,
) -> Vec<DomainObservation> {
    let Some(sequence) = activity_sequence(frame_sequence) else {
        return Vec::new();
    };
    let Some(id) = activity_observation_id(
        run_id,
        frame_sequence,
        &format!("safety:{}", safety.action.as_str()),
    ) else {
        return Vec::new();
    };
    // A model name too long to label is dropped, never the report.
    let model = safety
        .model
        .clone()
        .filter(|model| model.len() <= artisan_domain::OBSERVATION_LABEL_MAX_BYTES);
    artisan_domain::SafetyObservation::new(
        id,
        sequence,
        safety.action,
        safety.category,
        safety.provider_code.clone(),
        safety.message.clone(),
        model,
    )
    .map(|row| vec![DomainObservation::Safety(row)])
    .unwrap_or_default()
}

/// Steers a live turn with a follow-up message (`turn/steer`).
///
/// Production verb behind [`AcceptedTurn::steer_message`](super::operation::AcceptedTurn::steer_message):
/// the pump writes the follow-up with the actual provider turn id as
/// `expectedTurnId` and the correlated `turn/steer` result resolves the
/// delivery; a correlated error reply resolves it failed without settling
/// the turn. Proves the follow-up verb against the fixture stdio script
/// without disturbing the authorize-once production flow. `input` is the
/// same ordered `UserInput` array a fresh `turn/start` sends, so images
/// ride as native image items after the text.
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
    images: &[artisan_domain::ImageAttachment],
) -> Result<(), CodexTurnError> {
    let params = serde_json::json!({
        "expectedTurnId": turn_id,
        "input": prompt_input(text, images),
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
    // The app-server v2 vocabulary, as `CodexPendingTracker::approval_reply`
    // answers in production: Codex rejects any other spelling.
    let decision = if approved { "accept" } else { "decline" };
    let line =
        serde_json::json!({ "id": native_id, "result": { "decision": decision } }).to_string();
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

#[cfg(test)]
mod title_tests {
    use super::*;
    #[tokio::test]
    async fn generated_title_ignores_other_native_threads() {
        let (tx, mut rx) = mpsc::channel(4);
        let run = RunId::parse("title-run").unwrap();
        let mut tracker = CodexPendingTracker::new();
        tracker.bind_native_thread("root");
        let mut active = None;
        for thread in ["child", "root"] {
            let frame = format!(
                r#"{{"method":"thread/name/updated","params":{{"threadId":"{thread}","threadName":"List project files"}}}}"#
            );
            let event = super::super::protocol::parse_frame(&frame, 1).unwrap();
            assert!(
                apply_event(event, &run, &mut tracker, &mut active, &tx, 1, None)
                    .await
                    .is_none()
            );
        }
        assert!(
            matches!(rx.try_recv().unwrap(), EngineObservation::SummaryTitle { title, .. } if title.as_str() == "List project files")
        );
        assert!(rx.try_recv().is_err());
    }
}
