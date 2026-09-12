use std::collections::HashMap;

use artisan_domain::{
    EngineModelId, EngineRouteId, MessagePhase, Observation, ObservationId, ObservationSequence,
    RunId, RunUsageBasis, RunUsageReport, RunUsageReportInput, SubagentInput, SubagentObservation,
    SubagentState, SubagentTranscriptObservation, ThreadId, TranscriptAgentMessageDelta,
    TranscriptContent, UnixMillis,
};
use tokio::io::AsyncWrite;
use tokio::sync::mpsc;

#[cfg(test)]
use super::super::observation::TerminalObservation;
use super::super::observation::{EngineObservation, TerminalState, UsageObservation, chunk_text};

#[cfg(test)]
use super::protocol::{CLAUDE_MAX_ANSWERS, approval_response_line, question_response_line};
use super::protocol::{
    ClaudeApprovalRequest, ClaudeEvent, ClaudeQuestion, ClaudeQuestionRequest, ClaudeTurnError,
    ClaudeUsageSample, user_message_line, write_line,
};

/// Builds one validated subagent discovery row.
///
/// Fails closed (no row, discovery still tracked) when a provider identity
/// exceeds the domain ceilings: row identities reuse the full native thread
/// identities verbatim and are never truncated into ambiguity.
fn discovered_subagent_row(
    run_id: &RunId,
    parent_session: &str,
    task_id: &str,
    frame_sequence: u64,
) -> Option<Observation> {
    let id = ObservationId::parse(format!(
        "{}:claude:subagent:{task_id}:{frame_sequence}",
        run_id.as_str()
    ))
    .ok()?;
    let sequence = ObservationSequence::new(frame_sequence).ok()?;
    let input = SubagentInput {
        agent_native_thread_id: ObservationId::parse(task_id).ok()?,
        parent_native_thread_id: ObservationId::parse(parent_session).ok()?,
        state: SubagentState::Discovered,
        activity: None,
        agent_path: None,
        turn_id: None,
    };
    SubagentObservation::new(id, sequence, input)
        .ok()
        .map(Observation::Subagent)
}

/// Projects one child text fragment into a validated transcript row.
///
/// The child stream is keyed by its provider tool invocation until the task
/// lineage packet correlates invocations to agent tasks; only message text
/// projects, never approvals, questions, results, or reasoning.
fn child_transcript_row(
    run_id: &RunId,
    parent_session: &str,
    parent_tool_use_id: &str,
    delta: &str,
    phase: &str,
    frame_sequence: u64,
) -> Option<Observation> {
    let agent_id = ObservationId::parse(parent_tool_use_id).ok()?;
    let parent_id = ObservationId::parse(parent_session).ok()?;
    let phase = MessagePhase::parse(phase).ok()?;
    let item_id = ObservationId::parse(format!(
        "{}:claude:childmsg:{parent_tool_use_id}:{frame_sequence}",
        run_id.as_str()
    ))
    .ok()?;
    let content = TranscriptAgentMessageDelta::new(item_id, phase, delta.to_owned()).ok()?;
    let id = ObservationId::parse(format!(
        "{}:claude:childrow:{parent_tool_use_id}:{frame_sequence}",
        run_id.as_str()
    ))
    .ok()?;
    let sequence = ObservationSequence::new(frame_sequence).ok()?;
    Some(Observation::SubagentTranscript(
        SubagentTranscriptObservation::new(
            id,
            sequence,
            agent_id,
            parent_id,
            TranscriptContent::AgentMessageDelta(content),
        ),
    ))
}

/// In-memory pending interaction tracker for one live Claude turn.
///
/// Permission requests land as pending approvals and `AskUserQuestion` frames
/// land as pending questions. Resolutions apply through the durable resolve
/// path; a deny records the decision with no turn side effect while the run
/// continues. Subagent discoveries emit validated `Discovered` rows and child
/// transcript frames project validated transcript rows; both accumulate for
/// the consumer drain without ever reaching the root turn. Thinking
/// estimates and reasoning settlement are retained as plumbing.
#[expect(
    clippy::struct_excessive_bools,
    reason = "the tracker records independent provider lifecycle flags; folding them into a state enum would not reduce ambiguity"
)]
#[derive(Debug, Default)]
pub(crate) struct ClaudePendingTracker {
    approvals: HashMap<String, ClaudeApprovalRequest>,
    questions: HashMap<String, ClaudeQuestion>,
    subagents: Vec<String>,
    child_frames: Vec<(String, u64)>,
    subagent_rows: Vec<Observation>,
    thinking_tokens: Option<u64>,
    thinking_deltas: u64,
    reasoning_settled: bool,
    permission_denials: usize,
    stream_message_id: Option<String>,
    init_seen: bool,
    result_seen: bool,
    semantic_failure: bool,
    summary_title: Option<String>,
}

impl ClaudePendingTracker {
    /// Creates an empty tracker for one turn.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Notes one approval request; re-noting the same id is a no-op.
    ///
    /// The request is validated through the domain constructor first, so an
    /// out-of-bound provider frame never reaches the durable A-approve rows.
    pub(crate) fn note_approval(&mut self, request: ClaudeApprovalRequest) -> bool {
        if request.to_domain_request().is_err() {
            return false;
        }
        if self.approvals.contains_key(&request.approval_id) {
            return false;
        }
        self.approvals.insert(request.approval_id.clone(), request);
        true
    }

    /// Notes one question request group; re-noting the same id is a no-op.
    ///
    /// Each question is validated through the domain constructor first, so
    /// an out-of-bound provider frame never reaches the durable rows.
    pub(crate) fn note_questions(&mut self, request: &ClaudeQuestionRequest) -> usize {
        let mut added = 0;
        for question in request.questions() {
            if question.to_domain_input().is_err() {
                continue;
            }
            if !self.questions.contains_key(question.question_id()) {
                self.questions
                    .insert(question.question_id().to_owned(), question.clone());
                added += 1;
            }
        }
        added
    }

    /// Notes one subagent lifecycle identity and emits its `Discovered` row.
    ///
    /// Re-noting the same identity is a no-op and emits nothing twice. The
    /// row carries the root session plus the agent thread identity with state
    /// `Discovered`; row construction fails closed (discovery still tracked)
    /// when a provider identity exceeds the domain ceilings.
    pub(crate) fn note_subagent(
        &mut self,
        run_id: &RunId,
        parent_session: &str,
        task_id: &str,
        frame_sequence: u64,
    ) {
        if self.subagents.iter().any(|known| known == task_id) {
            return;
        }
        self.subagents.push(task_id.to_owned());
        if let Some(row) = discovered_subagent_row(run_id, parent_session, task_id, frame_sequence)
        {
            self.subagent_rows.push(row);
        }
    }

    /// Projects one child transcript frame into an isolated transcript row.
    ///
    /// The frame is always counted; a row is stored only when the frame
    /// carried renderer-safe message text. The row never reaches the root
    /// turn: it accumulates for the consumer drain with its own durable
    /// identity and sequencing.
    pub(crate) fn note_child_frame(
        &mut self,
        run_id: &RunId,
        parent_session: &str,
        parent_tool_use_id: &str,
        text: Option<(String, &'static str)>,
        frame_sequence: u64,
    ) {
        self.child_frames
            .push((parent_tool_use_id.to_owned(), frame_sequence));
        let Some((delta, phase)) = text else {
            return;
        };
        if let Some(row) = child_transcript_row(
            run_id,
            parent_session,
            parent_tool_use_id,
            &delta,
            phase,
            frame_sequence,
        ) {
            self.subagent_rows.push(row);
        }
    }

    /// Drains validated subagent rows for the consumer in emission order.
    ///
    /// The fixture driver proves emission plus sequencing here; the
    /// dispatcher packet wires this drain into the live pump beside the text
    /// channel.
    pub(crate) fn take_subagent_rows(&mut self) -> Vec<Observation> {
        std::mem::take(&mut self.subagent_rows)
    }

    /// Preserves the encrypted-thinking estimate (never root text).
    pub(crate) fn note_thinking_tokens(&mut self, estimated_tokens: u64) {
        self.thinking_tokens = Some(estimated_tokens);
    }

    /// Counts one non-empty thinking delta (never root text).
    pub(crate) fn note_reasoning_delta(&mut self) {
        self.thinking_deltas += 1;
    }

    /// Marks reasoning settled by a buffered thinking block, even when its
    /// text arrived empty (encrypted reasoning has no delta to complete).
    pub(crate) fn note_reasoning_settled(&mut self) {
        self.reasoning_settled = true;
    }

    /// Retains the denied-permission count without approval semantics.
    pub(crate) fn note_permission_denials(&mut self, count: usize) {
        self.permission_denials += count;
    }

    /// Retains the generated session title captured at the terminal fence.
    ///
    /// Later captures replace earlier ones, mirroring the TypeScript reader
    /// that keeps the newest `ai-title` record; the title never disturbs the
    /// turn and settles onto the terminal `summary_title`.
    pub(crate) fn note_summary_title(&mut self, title: String) {
        self.summary_title = Some(title);
    }

    /// Returns the captured generated session title, if any arrived.
    pub(crate) fn summary_title(&self) -> Option<&str> {
        self.summary_title.as_deref()
    }

    /// Returns whether the terminal `result` frame arrived (pump-only).
    pub(crate) fn result_seen(&self) -> bool {
        self.result_seen
    }

    /// Returns whether a semantic failure was classified (pump-only).
    pub(crate) fn semantic_failure(&self) -> bool {
        self.semantic_failure
    }

    /// Resolves one approval; returns whether it was pending.
    ///
    /// Test-only until dispatcher delivery wiring lands.
    #[cfg(test)]
    pub(crate) fn resolve_approval(&mut self, approval_id: &str) -> bool {
        self.approvals.remove(approval_id).is_some()
    }

    /// Resolves one question; returns whether it was pending.
    ///
    /// Test-only until dispatcher delivery wiring lands.
    #[cfg(test)]
    pub(crate) fn resolve_question(&mut self, question_id: &str) -> bool {
        self.questions.remove(question_id).is_some()
    }

    /// Returns the number of pending approvals.
    #[cfg(test)]
    pub(crate) fn pending_approvals(&self) -> usize {
        self.approvals.len()
    }

    /// Returns the number of pending questions.
    #[cfg(test)]
    pub(crate) fn pending_questions(&self) -> usize {
        self.questions.len()
    }

    /// Returns the number of discovered subagent lifecycle identities.
    #[cfg(test)]
    pub(crate) fn subagent_count(&self) -> usize {
        self.subagents.len()
    }

    /// Returns the number of isolated child transcript frames.
    #[cfg(test)]
    pub(crate) fn child_frame_count(&self) -> usize {
        self.child_frames.len()
    }

    /// Returns the preserved thinking-token estimate, if any arrived.
    #[cfg(test)]
    pub(crate) fn thinking_tokens(&self) -> Option<u64> {
        self.thinking_tokens
    }

    /// Returns whether reasoning settled without delta text.
    #[cfg(test)]
    pub(crate) fn reasoning_settled(&self) -> bool {
        self.reasoning_settled
    }

    /// Returns whether the init gate accepted the spawned session.
    #[cfg(test)]
    pub(crate) fn init_seen(&self) -> bool {
        self.init_seen
    }

    /// Returns how many non-empty thinking deltas were counted.
    #[cfg(test)]
    pub(crate) fn thinking_deltas(&self) -> u64 {
        self.thinking_deltas
    }

    /// Returns the retained denied-permission count.
    #[cfg(test)]
    pub(crate) fn permission_denial_count(&self) -> usize {
        self.permission_denials
    }
}

/// How one applied event continues the pump.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClaudeApplyOutcome {
    /// Keep pumping; `end_input` closes stdin exactly once (result seen).
    Continue { end_input: bool },
    /// Settle the turn now with this terminal state.
    Terminal(TerminalState),
}

/// Immutable attribution for one Claude usage report.
///
/// Model and thread come from the immutable launch snapshot; the provider
/// session is the authenticated native session, never an envelope claim.
pub(crate) struct ClaudeUsageContext<'a> {
    pub run_id: &'a RunId,
    pub thread_id: &'a ThreadId,
    pub provider_session_id: &'a str,
    pub model_id: &'a EngineModelId,
    pub observed_at: UnixMillis,
}

/// Best-effort usage scope carried beside the text channel.
///
/// `None` (no explicit model or no thread scope) skips usage projection
/// without disturbing the turn: usage never blocks turns.
#[derive(Clone, Debug)]
pub(crate) struct ClaudeUsageAttribution {
    pub thread_id: ThreadId,
    pub model_id: EngineModelId,
}

/// Borrowed usage scope for one pump loop.
#[expect(
    clippy::struct_field_names,
    reason = "fields mirror ClaudeUsageAttribution and the wire usage vocabulary; renaming would obscure the mapping"
)]
pub(crate) struct ClaudeUsageScope<'a> {
    pub thread_id: &'a ThreadId,
    pub model_id: &'a EngineModelId,
    pub provider_session_id: &'a str,
}

/// Builds the cumulative usage report for one sample.
///
/// Fails closed (`None`) when identities or bounds reject: usage is never
/// synthesized from partial identities. The context gauge always replaces
/// the previous report regardless of basis — the codec performs no
/// arithmetic at all. Claude names no provider route, so reports attribute
/// to the engine's own `claude` route namespace; quota windows are never
/// copied here, so no quota is invented. Claude discloses no provider turn
/// identity on usage frames, so none is attributed rather than synthesized.
pub(crate) fn claude_usage_report(
    context: &ClaudeUsageContext<'_>,
    source_sequence: u64,
    sample: &ClaudeUsageSample,
) -> Option<RunUsageReport> {
    let provider_route_id = EngineRouteId::parse("claude").ok()?;
    RunUsageReport::new(RunUsageReportInput {
        run_id: context.run_id.clone(),
        thread_id: context.thread_id.clone(),
        provider_session_id: context.provider_session_id.to_owned(),
        source_sequence,
        model_id: context.model_id.clone(),
        provider_route_id,
        variant_id: None,
        basis: RunUsageBasis::Cumulative,
        provider_turn_id: None,
        input_tokens: sample.input,
        cached_input_tokens: sample.cached_input,
        output_tokens: sample.output,
        context_tokens: sample.context,
        context_window_tokens: None,
        observed_at: context.observed_at,
    })
    .ok()
}

fn current_unix_millis() -> Option<UnixMillis> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    let millis = i64::try_from(duration.as_millis()).ok()?;
    Some(UnixMillis::from_millis(millis))
}

/// Projects one usage sample best-effort onto the shared usage vocabulary.
///
/// Returns `Some(TerminalState::Interrupted)` only when the observation sink
/// closed mid-send. Skipping (no scope, no clock, or unattributable sample)
/// is never terminal: usage never blocks turns.
async fn project_usage_sample(
    observations: &mpsc::Sender<EngineObservation>,
    run_id: &RunId,
    scope: Option<&ClaudeUsageScope<'_>>,
    source_sequence: u64,
    sample: &ClaudeUsageSample,
) -> Option<TerminalState> {
    let scope = scope?;
    let observed_at = current_unix_millis()?;
    let report = claude_usage_report(
        &ClaudeUsageContext {
            run_id,
            thread_id: scope.thread_id,
            provider_session_id: scope.provider_session_id,
            model_id: scope.model_id,
            observed_at,
        },
        source_sequence,
        sample,
    )?;
    if observations
        .send(EngineObservation::Usage(UsageObservation::new(report)))
        .await
        .is_err()
    {
        return Some(TerminalState::Interrupted);
    }
    None
}

/// Kind of one Claude quota window, classified from its provider window.
///
/// Mirrors `parse_claude_cli_usage_windows` in
/// `modules/engines/src/claude/usage.ts`: 300 minutes is a session window,
/// 10,080 a weekly window; anything else is unknown rather than guessed.
/// Claude names no monthly bucket.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg(test)]
pub(crate) enum ClaudeQuotaWindowKind {
    Session,
    Weekly,
    Unknown,
}

/// Classifies one quota window duration.
#[cfg(test)]
pub(crate) fn classify_claude_quota_window_kind(
    window_minutes: Option<u64>,
) -> ClaudeQuotaWindowKind {
    match window_minutes {
        Some(300) => ClaudeQuotaWindowKind::Session,
        Some(10_080) => ClaudeQuotaWindowKind::Weekly,
        _ => ClaudeQuotaWindowKind::Unknown,
    }
}

/// Clamps one percent reading into `0..=100`.
///
/// Absent or non-finite readings become `0`: usage display never blocks on a
/// corrupt gauge and never invents quota from it.
#[cfg(test)]
pub(crate) fn clamp_claude_percent_used(used_percent: Option<f64>) -> f64 {
    match used_percent {
        Some(value) if value.is_finite() => value.clamp(0.0, 100.0),
        _ => 0.0,
    }
}

/// The exact non-billable CLI invocation that reads account usage.
///
/// Mirrors `claude_cli_usage_args` in `modules/engines/src/claude/usage.ts`
/// (`-p /usage` over JSON): the slash command travels in argv and stdin
/// closes immediately, so no prompt is ever billed.
#[cfg(test)]
pub(crate) fn claude_cli_usage_args() -> [&'static str; 4] {
    ["-p", "/usage", "--output-format", "json"]
}

/// One provider-neutral Claude quota window: diagnostics only, never quota.
///
/// Quota windows are read through the non-billable `/usage` surface and
/// classified here; they are never copied into [`RunUsageReport`] and never
/// gate a turn.
#[derive(Clone, Debug, PartialEq)]
#[cfg(test)]
pub(crate) struct ClaudeQuotaWindow {
    pub id: String,
    pub kind: ClaudeQuotaWindowKind,
    pub label: Option<String>,
    pub percent_used: f64,
    pub resets_at: Option<String>,
    pub window_minutes: Option<u64>,
    /// `"shared"` for the session and all-models weekly buckets, `"model"`
    /// for per-model weekly buckets. Mirrors the TypeScript scope rule
    /// without inventing quota attribution.
    pub scope: &'static str,
}

/// Turns a provider-supplied weekly label into a stable id fragment.
///
/// Mirrors `slugify_claude_cli_label` in `modules/engines/src/claude/usage.ts`:
/// lowercase, non-alphanumeric runs become one dash, edge dashes trimmed.
#[cfg(test)]
pub(crate) fn slugify_claude_cli_label(label: &str) -> String {
    let mut slug = String::with_capacity(label.len());
    let mut pending_dash = false;
    for character in label.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            pending_dash = false;
            slug.push(character);
        } else if !slug.is_empty() || pending_dash {
            pending_dash = true;
        }
    }
    slug
}

#[cfg(test)]
const CLAUDE_CLI_MONTHS: [(&str, i64); 12] = [
    ("jan", 1),
    ("feb", 2),
    ("mar", 3),
    ("apr", 4),
    ("may", 5),
    ("jun", 6),
    ("jul", 7),
    ("aug", 8),
    ("sep", 9),
    ("oct", 10),
    ("nov", 11),
    ("dec", 12),
];

#[cfg(test)]
fn claude_cli_month(name: &str) -> Option<i64> {
    let prefix: String = name.chars().take(3).flat_map(char::to_lowercase).collect();
    CLAUDE_CLI_MONTHS
        .iter()
        .find(|(month, _)| *month == prefix)
        .map(|(_, number)| *number)
}

/// Converts a civil date to days since the Unix epoch (Howard Hinnant's
/// algorithm), mirroring the epoch math behind the TypeScript reset parse
/// without a date dependency.
#[cfg(test)]
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let adjusted_year = if month <= 2 { year - 1 } else { year };
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year.rem_euclid(400);
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Converts days since the Unix epoch back to a civil date.
#[cfg(test)]
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_pair = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_pair + 2) / 5 + 1;
    let month = if month_pair < 10 {
        month_pair + 3
    } else {
        month_pair - 9
    };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Parses the provider's English wall-clock reset clause to a UTC instant.
///
/// Mirrors `parse_claude_cli_reset_at` in `modules/engines/src/claude/usage.ts`
/// (`resets <Mon> <D>, <H>[:<MM>] <am|pm> (<Zone>)` at end of line). Only
/// `UTC`/`GMT`/`UT` zones resolve to a real instant: named IANA zones need a
/// zone database this owner does not carry, so they stay `None` rather than
/// becoming an invented instant. `at_ms` is the caller-observed now used for
/// year inference (December rolling into a January reset).
#[cfg(test)]
pub(crate) fn parse_claude_cli_reset_at(line: &str, at_ms: i64) -> Option<String> {
    // Scan left to right for the first `resets <clause>` that fully parses,
    // mirroring the unanchored regex scan in the TypeScript reader. The scan
    // is ASCII-boundary safe: `resets ` is pure ASCII and the cursor always
    // rests on a character boundary.
    let bytes = line.as_bytes();
    let mut index = 0;
    while index + 7 <= bytes.len() {
        if bytes[index..index + 7].eq_ignore_ascii_case(b"resets ")
            && (index == 0 || !bytes[index - 1].is_ascii_alphanumeric())
            && let Some(instant) = parse_reset_clause(&line[index + 7..], at_ms)
        {
            return Some(instant);
        }
        index += line[index..].chars().next()?.len_utf8();
    }
    None
}

#[cfg(test)]
fn parse_reset_clause(clause: &str, at_ms: i64) -> Option<String> {
    let clause = clause.trim_end();
    let (date_part, zone_part) = clause.rsplit_once('(')?;
    let zone = zone_part.strip_suffix(')')?;
    if !zone.eq_ignore_ascii_case("UTC")
        && !zone.eq_ignore_ascii_case("GMT")
        && !zone.eq_ignore_ascii_case("UT")
    {
        return None;
    }
    let mut tokens = date_part.split_whitespace();
    let month_token = tokens.next()?;
    if !month_token
        .chars()
        .all(|character| character.is_ascii_alphabetic())
    {
        return None;
    }
    let day_token = tokens.next()?.strip_suffix(',')?;
    let month = claude_cli_month(month_token)?;
    let day: i64 = day_token.parse().ok()?;
    let (hour_token, minute_token, meridiem_token) = match (tokens.next(), tokens.next()) {
        (Some(time), Some(meridiem)) => {
            let (hour, minute) = match time.split_once(':') {
                Some((hour, minute)) => {
                    if minute.len() != 2 {
                        return None;
                    }
                    (hour, Some(minute))
                }
                None => (time, None),
            };
            (hour, minute, meridiem)
        }
        _ => return None,
    };
    if tokens.next().is_some() {
        return None;
    }
    let hour12: i64 = hour_token.parse().ok()?;
    let minute: i64 = match minute_token {
        Some(text) => text.parse().ok()?,
        None => 0,
    };
    let meridiem = meridiem_token.to_ascii_lowercase();
    if !(1..=31).contains(&day) || !(1..=12).contains(&hour12) || !(0..=59).contains(&minute) {
        return None;
    }
    let hour = match meridiem.as_str() {
        "am" => hour12 % 12,
        "pm" => hour12 % 12 + 12,
        _ => return None,
    };
    let (current_year, current_month, _) = civil_from_days(at_ms.div_euclid(86_400_000));
    let year = current_year + i64::from(current_month == 12 && month == 1);
    if civil_from_days(days_from_civil(year, month, day)) != (year, month, day) {
        return None;
    }
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:00Z"
    ))
}

#[cfg(test)]
fn claude_usage_bucket_window(
    id: String,
    label: Option<String>,
    percent_text: &str,
    line: &str,
    at_ms: i64,
    scope: &'static str,
    window_minutes: u64,
) -> ClaudeQuotaWindow {
    ClaudeQuotaWindow {
        id,
        kind: classify_claude_quota_window_kind(Some(window_minutes)),
        label,
        percent_used: clamp_claude_percent_used(percent_text.parse::<f64>().ok()),
        resets_at: parse_claude_cli_reset_at(line, at_ms),
        window_minutes: Some(window_minutes),
        scope,
    }
}

#[cfg(test)]
fn match_percent_tail(text: &str) -> Option<&str> {
    let end = text
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(text.len());
    if end == 0 {
        return None;
    }
    let (digits, rest) = (&text[..end], &text[end..]);
    let rest = rest.strip_prefix('%')?;
    let rest = rest.trim_start();
    let after_used = rest.strip_prefix("used")?;
    if after_used
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return None;
    }
    Some(digits)
}

/// Parses provider-owned `/usage` CLI text into quota windows.
///
/// Mirrors `parse_claude_cli_usage_windows` in
/// `modules/engines/src/claude/usage.ts` without retaining credentials or raw
/// account data: one `five_hour` session window, one shared `seven_day`
/// weekly window, plus one per-model `seven_day:<slug>` weekly window.
/// Duplicate ids keep the first row; malformed lines yield nothing.
#[cfg(test)]
pub(crate) fn parse_claude_cli_usage_windows(
    result_text: &str,
    at_ms: i64,
) -> Vec<ClaudeQuotaWindow> {
    let mut windows: Vec<ClaudeQuotaWindow> = Vec::new();
    for raw_line in result_text.split('\n') {
        let line = raw_line.trim();
        if let Some(rest) = line.strip_prefix("Current session:") {
            let rest = rest.trim_start();
            if let Some(percent) = match_percent_tail(rest) {
                push_quota_window(
                    &mut windows,
                    claude_usage_bucket_window(
                        "five_hour".to_owned(),
                        None,
                        percent,
                        line,
                        at_ms,
                        "shared",
                        300,
                    ),
                );
            }
            continue;
        }
        // The all-models bucket is checked before the labeled pattern below.
        if let Some(rest) = line.strip_prefix("Current week (all models):") {
            let rest = rest.trim_start();
            if let Some(percent) = match_percent_tail(rest) {
                push_quota_window(
                    &mut windows,
                    claude_usage_bucket_window(
                        "seven_day".to_owned(),
                        None,
                        percent,
                        line,
                        at_ms,
                        "shared",
                        10_080,
                    ),
                );
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("Current week (") {
            let Some(close) = rest.find(')') else {
                continue;
            };
            // The labeled shape requires the exact `(<Label>): N% used`
            // form: the colon follows the parenthesis immediately.
            let label = &rest[..close];
            if label.is_empty() {
                continue;
            }
            let Some(after) = rest[close + 1..].strip_prefix(':') else {
                continue;
            };
            let after = after.trim_start();
            if let Some(percent) = match_percent_tail(after) {
                let slug = slugify_claude_cli_label(label);
                push_quota_window(
                    &mut windows,
                    claude_usage_bucket_window(
                        format!("seven_day:{slug}"),
                        Some(label.to_owned()),
                        percent,
                        line,
                        at_ms,
                        "model",
                        10_080,
                    ),
                );
            }
        }
    }
    windows
}

#[cfg(test)]
fn push_quota_window(windows: &mut Vec<ClaudeQuotaWindow>, window: ClaudeQuotaWindow) {
    if windows.iter().any(|known| known.id == window.id) {
        return;
    }
    windows.push(window);
}

/// Applies one typed event; returns how the pump continues.
///
/// Text deltas chunk onto the shared vocabulary with the verbatim phase
/// carried on the event; the current stream message id (when announced)
/// becomes the explicit part identity so one message and its completion stay
/// grouped. Usage samples project best-effort onto the shared usage
/// vocabulary when a usage scope travels with the pump; without one (or on
/// any attribution failure) they are diagnostics that never disturb the turn.
/// Usage collection never blocks turns: only the observation sink closing is
/// terminal. Session identity mismatches fail the turn closed: only the exact
/// spawned session may speak for it.
#[expect(
    clippy::too_many_lines,
    reason = "single event dispatch table projecting typed events; extracting arms would thread the same sink and tracker"
)]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn apply_event(
    event: ClaudeEvent,
    run_id: &RunId,
    expected_session: &str,
    tracker: &mut ClaudePendingTracker,
    active_turn: &mut Option<String>,
    observations: &mpsc::Sender<EngineObservation>,
    frame_sequence: u64,
    usage: Option<&ClaudeUsageScope<'_>>,
) -> ClaudeApplyOutcome {
    match event {
        ClaudeEvent::Init { session_id } => {
            if session_id != expected_session {
                return ClaudeApplyOutcome::Terminal(TerminalState::Failed);
            }
            tracker.init_seen = true;
            ClaudeApplyOutcome::Continue { end_input: false }
        }
        ClaudeEvent::MessageStart { message_id } => {
            tracker.stream_message_id = Some(message_id);
            ClaudeApplyOutcome::Continue { end_input: false }
        }
        ClaudeEvent::TextDelta {
            delta,
            phase,
            usage: sample,
        } => {
            // Both verbatim phases fold to the shared delta vocabulary here;
            // the phase stays on the typed event for later routing packets.
            let _ = phase;
            if active_turn.is_none() {
                *active_turn = Some(expected_session.to_owned());
            }
            let native_id = format!("claude:{frame_sequence}");
            let part_id = tracker.stream_message_id.clone();
            for chunk in chunk_text(run_id, frame_sequence, &native_id, &delta) {
                let chunk = match part_id.clone() {
                    Some(part) => chunk.with_part_id(part),
                    None => chunk,
                };
                if observations
                    .send(EngineObservation::TextDelta(chunk))
                    .await
                    .is_err()
                {
                    return ClaudeApplyOutcome::Terminal(TerminalState::Interrupted);
                }
            }
            if let Some(sample) = sample.as_ref()
                && let Some(state) =
                    project_usage_sample(observations, run_id, usage, frame_sequence, sample).await
            {
                return ClaudeApplyOutcome::Terminal(state);
            }
            ClaudeApplyOutcome::Continue { end_input: false }
        }
        ClaudeEvent::ThinkingTokens { estimated_tokens } => {
            tracker.note_thinking_tokens(estimated_tokens);
            ClaudeApplyOutcome::Continue { end_input: false }
        }
        ClaudeEvent::ReasoningDelta => {
            tracker.note_reasoning_delta();
            ClaudeApplyOutcome::Continue { end_input: false }
        }
        ClaudeEvent::ReasoningSettled { usage: sample } => {
            tracker.note_reasoning_settled();
            if let Some(sample) = sample.as_ref()
                && let Some(state) =
                    project_usage_sample(observations, run_id, usage, frame_sequence, sample).await
            {
                return ClaudeApplyOutcome::Terminal(state);
            }
            ClaudeApplyOutcome::Continue { end_input: false }
        }
        ClaudeEvent::Usage { sample } => {
            if let Some(state) =
                project_usage_sample(observations, run_id, usage, frame_sequence, &sample).await
            {
                return ClaudeApplyOutcome::Terminal(state);
            }
            ClaudeApplyOutcome::Continue { end_input: false }
        }
        ClaudeEvent::ApprovalRequested(request) => {
            tracker.note_approval(request);
            ClaudeApplyOutcome::Continue { end_input: false }
        }
        ClaudeEvent::QuestionRequested(request) => {
            tracker.note_questions(&request);
            ClaudeApplyOutcome::Continue { end_input: false }
        }
        ClaudeEvent::SubagentLifecycle { task_id } => {
            // Discovery emits its row; the root turn is never adopted and no
            // root text is emitted.
            tracker.note_subagent(run_id, expected_session, &task_id, frame_sequence);
            ClaudeApplyOutcome::Continue { end_input: false }
        }
        ClaudeEvent::ChildTranscript {
            parent_tool_use_id,
            text,
        } => {
            // Projection isolates the child row; the root turn is never
            // adopted and no root text is emitted.
            tracker.note_child_frame(
                run_id,
                expected_session,
                &parent_tool_use_id,
                text,
                frame_sequence,
            );
            ClaudeApplyOutcome::Continue { end_input: false }
        }
        ClaudeEvent::TurnResult {
            success,
            session_id,
            permission_denials,
            usage: sample,
        } => {
            if let Some(session_id) = session_id
                && session_id != expected_session
            {
                return ClaudeApplyOutcome::Terminal(TerminalState::Failed);
            }
            tracker.result_seen = true;
            if !success {
                tracker.semantic_failure = true;
            }
            tracker.note_permission_denials(permission_denials);
            if let Some(sample) = sample.as_ref()
                && let Some(state) =
                    project_usage_sample(observations, run_id, usage, frame_sequence, sample).await
            {
                return ClaudeApplyOutcome::Terminal(state);
            }
            ClaudeApplyOutcome::Continue { end_input: true }
        }
        ClaudeEvent::Unknown => ClaudeApplyOutcome::Continue { end_input: false },
    }
}

/// Steers a live turn with follow-up text (stream-input fold).
///
/// Production verb behind [`AcceptedTurn::steer_text`](super::operation::AcceptedTurn::steer_text):
/// the pump writes the fold line over its owned stdin and the write outcome
/// resolves the delivery. Proves the fold verb against the fixture stdio
/// script without disturbing the authorize-once production flow.
/// Experimental per the adapter: the CLI owns fold timing.
///
/// # Errors
///
/// Returns [`ClaudeTurnError`] when the write fails.
pub(crate) async fn steer_live_turn<W: AsyncWrite + Unpin>(
    stdin: &mut W,
    session_id: &str,
    text: &str,
) -> Result<(), ClaudeTurnError> {
    write_line(stdin, &user_message_line(session_id, text)).await
}

/// Answers one pending approval through the durable decision.
///
/// Test-only until dispatcher delivery wiring lands: deny carries no turn
/// side effect and the run continues either way.
///
/// # Errors
///
/// Returns [`ClaudeTurnError::Configuration`] for an unknown or resolved
/// target and [`ClaudeTurnError::StreamFailed`] when the write fails.
#[cfg(test)]
pub(crate) async fn answer_approval<W: AsyncWrite + Unpin>(
    stdin: &mut W,
    tracker: &mut ClaudePendingTracker,
    request_id: &str,
    approval_id: &str,
    approved: bool,
) -> Result<(), ClaudeTurnError> {
    if !tracker.resolve_approval(approval_id) {
        return Err(ClaudeTurnError::Configuration);
    }
    write_line(stdin, &approval_response_line(request_id, approved)).await
}

/// Answers one question request group through the durable answers.
///
/// Test-only until dispatcher delivery wiring lands. Answers accumulate per
/// question text; the response amends the verbatim request input.
///
/// # Errors
///
/// Returns [`ClaudeTurnError::Configuration`] for an unknown or resolved
/// target and [`ClaudeTurnError::StreamFailed`] when the write fails.
#[cfg(test)]
pub(crate) async fn answer_questions<W: AsyncWrite + Unpin>(
    stdin: &mut W,
    tracker: &mut ClaudePendingTracker,
    request: &ClaudeQuestionRequest,
    answers: &[(String, Vec<String>)],
) -> Result<(), ClaudeTurnError> {
    for (question_id, _) in answers {
        if !tracker.resolve_question(question_id) {
            return Err(ClaudeTurnError::Configuration);
        }
    }
    let mut joined = Vec::new();
    for (question_id, options) in answers.iter().take(CLAUDE_MAX_ANSWERS) {
        let Some(question) = request
            .questions()
            .iter()
            .find(|known| known.question_id() == question_id)
        else {
            return Err(ClaudeTurnError::Configuration);
        };
        joined.push((question.text().to_owned(), options.join(", ")));
    }
    let line = question_response_line(request.request_id(), &request.input, &joined);
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
