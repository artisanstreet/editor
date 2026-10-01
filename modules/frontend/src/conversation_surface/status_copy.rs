//! Status copy and visibility policy for [`ConversationSurface`]: the
//! elapsed narration verbs, the work-group header lines, the thinking
//! summary reduction, and the rules deciding which row paints.
//!
//! Split out of `contract.rs` as the pure copy half of the surface
//! contract; render and scroll-identity code share these exact decisions,
//! so paint and measured identities stay one-to-one.

use artisan_domain::{ConversationLifecycle, EngineId};
use artisan_ui::inline_code_text::{claude_first_clause, claude_label_line, summary_line};

use crate::conversation_scene::{
    TurnBlock, TurnNarration, TurnScene, TurnStatusBlock, WorkGroupLabel,
};

/// Formats an elapsed millisecond value with deterministic whole-second
/// precision. Seconds are always present; minutes appear for non-zero minutes
/// or whenever hours are present.
#[must_use]
pub fn format_elapsed_millis(millis: u64) -> String {
    format_elapsed_seconds(millis / 1_000)
}

pub(super) fn format_elapsed_seconds(total_seconds: u64) -> String {
    let hours = total_seconds / 3_600;
    let minutes = (total_seconds % 3_600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours}h {minutes}m {seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

/// Returns the work-group header copy for one terminal label.
///
/// `None` stays headerless: live groups carry no generic title, since the
/// reference shows its elapsed header whose words the turn status row already
/// provides from scene data. A second title here would double the line.
#[must_use]
pub fn work_group_header_copy(label: Option<WorkGroupLabel>) -> Option<String> {
    label.map(format_work_group_label)
}

/// Returns the scene-owned display label for one terminal work-group label.
#[must_use]
pub fn format_work_group_label(label: WorkGroupLabel) -> String {
    match label {
        crate::conversation_scene::WorkGroupLabel::WorkedFor { millis } => {
            format!("Worked for {}", format_elapsed_millis(millis))
        }
        crate::conversation_scene::WorkGroupLabel::ThoughtFor { millis } => {
            format!("Thought for {}", format_elapsed_millis(millis))
        }
        crate::conversation_scene::WorkGroupLabel::Failed => "Failed".to_owned(),
        crate::conversation_scene::WorkGroupLabel::Interrupted => "Interrupted".to_owned(),
        crate::conversation_scene::WorkGroupLabel::Cancelled => "Cancelled".to_owned(),
    }
}

/// Returns the exact plain-text status narration for a scene status block.
///
/// `Quiet` paints no row: idleness is the absence of status, not a status, so
/// the reference shows no idle row and this renderer keeps no gap slot for
/// one. `StreamingSuppression` intentionally returns `None`, so a renderer can
/// guarantee that no thinking/working row is painted while a streaming
/// assistant message owns the visible progress state.
#[must_use]
pub fn turn_status_copy(narration: TurnNarration) -> Option<String> {
    match narration {
        TurnNarration::Quiet | TurnNarration::StreamingSuppression => None,
        TurnNarration::ProviderWait => Some("Waiting for provider to respond…".to_owned()),
        TurnNarration::Compacting => Some("Compacting the conversation…".to_owned()),
        TurnNarration::Thinking => Some("Thinking".to_owned()),
        TurnNarration::Working => Some("Working".to_owned()),
        TurnNarration::BackgroundWait => Some("Waiting for background agents…".to_owned()),
        TurnNarration::WorkedFor { millis } => {
            Some(format!("Worked for {}", format_elapsed_millis(millis)))
        }
        TurnNarration::ThoughtFor { millis } => {
            Some(format!("Thought for {}", format_elapsed_millis(millis)))
        }
        TurnNarration::Failed => Some("Failed".to_owned()),
        TurnNarration::Interrupted => Some("Interrupted".to_owned()),
        TurnNarration::Cancelled => Some("Cancelled".to_owned()),
    }
}

/// Returns the live status copy for one narration with its authoritative
/// elapsed basis.
///
/// While the narration is `Thinking`/`Working`, an authoritative
/// `active_started_at_ms` basis paired with a host-mirrored `frame_now_ms`
/// renders `Thinking for Xs` / `Working for Xs` with whole-second flooring
/// (`FormatElapsed` parity); the group header counts from the same basis.
/// `ProviderWait` never counts here — the waiting sentence (generic or
/// engine-named via [`provider_wait_copy`]) is the row's whole narration.
/// Either value missing renders the bare narration copy truthfully: the
/// renderer never reads a clock and never resets the basis on rerender. A
/// basis on any other narration is ignored here (scene `build` already
/// rejects it with a typed error).
#[must_use]
pub fn live_status_copy(
    narration: TurnNarration,
    active_started_at_ms: Option<i64>,
    frame_now_ms: Option<i64>,
) -> Option<String> {
    match narration {
        TurnNarration::Thinking | TurnNarration::Working => {
            let verb = match narration {
                TurnNarration::Thinking => "Thinking",
                TurnNarration::Working => "Working",
                _ => unreachable!("elapsed match covers every counted verb"),
            };
            match (active_started_at_ms, frame_now_ms) {
                (Some(started_at_ms), Some(now_ms)) => {
                    let elapsed_ms =
                        u64::try_from(now_ms.saturating_sub(started_at_ms).max(0)).unwrap_or(0);
                    Some(format!("{verb} for {}", format_elapsed_millis(elapsed_ms)))
                }
                (None, _) | (_, None) => turn_status_copy(narration),
            }
        }
        _ => turn_status_copy(narration),
    }
}

/// Returns the live Thinking/Working header owned by at most one work group.
///
/// Terminal labels always win through [`work_group_header_copy`]; this covers
/// the live line only. A thinking stretch that reduced to a label takes the
/// collapsed chip form, counting on the same elapsed basis. Any other
/// narration yields no group header, so the turn status row below remains
/// its single owner.
#[must_use]
pub fn live_group_header_copy(
    narration: TurnNarration,
    active_started_at_ms: Option<i64>,
    frame_now_ms: Option<i64>,
) -> Option<String> {
    match narration {
        TurnNarration::Thinking | TurnNarration::Working => {
            live_status_copy(narration, active_started_at_ms, frame_now_ms)
        }
        _ => None,
    }
}

/// Returns the index of the work group that owns the live header, if any.
///
/// Exactly one group owns it: the latest non-superseded group, nearest the
/// status row it replaces. A superseded session never narrates — the
/// turn-level status row at turn end narrates current work instead. Earlier
/// groups render items only, so the live line paints once per turn. A
/// session continuation (the segment after a mid-run user message) is never
/// an owner: the section's one header sits on its first segment and keeps
/// the live line there for the whole turn, however much work arrives below.
#[must_use]
pub fn owning_group_index(turn: &TurnScene) -> Option<usize> {
    turn.blocks().iter().rposition(|block| match block {
        TurnBlock::WorkGroup(group) => !group.superseded && group.continuation.is_none(),
        _ => false,
    })
}

/// Returns whether a turn is still live, so its work sections stay open.
///
/// A section cannot be collapsed before its turn settles: while the
/// authoritative lifecycle is pending, streaming, active, or waiting, the
/// header carries no chevron and no toggle and the panel is forced open
/// whatever disclosure state is stored. Completed, failed, and cancelled
/// turns settle; so does an interrupted one, which shows its outcome until a
/// resume makes it live again.
#[must_use]
pub fn turn_is_live(turn: &TurnScene) -> bool {
    matches!(
        turn.lifecycle,
        ConversationLifecycle::Pending
            | ConversationLifecycle::Streaming
            | ConversationLifecycle::Active
            | ConversationLifecycle::Waiting
    )
}

/// Titles a turn's work section from the turn's own state when neither a
/// terminal label nor a live Thinking/Working line reached the section.
///
/// The section header always names what the turn is doing or how it ended,
/// never a generic title. A live turn counts on the turn's one elapsed basis
/// whatever its narration (a provider wait, a streaming reply, compaction, a
/// background wait), so the header keeps counting while the status row says
/// what is happening; without a basis it reads the bare verb. A settled turn
/// reads its lifecycle outcome: the duration belongs to the terminal label,
/// so a completed turn that lost its narration reads plain `Worked`.
#[must_use]
pub fn turn_section_title(turn: &TurnScene, frame_now_ms: Option<i64>) -> String {
    match turn.lifecycle {
        ConversationLifecycle::Completed => "Worked".to_owned(),
        ConversationLifecycle::Failed => "Failed".to_owned(),
        ConversationLifecycle::Interrupted => "Interrupted".to_owned(),
        ConversationLifecycle::Cancelled => "Cancelled".to_owned(),
        ConversationLifecycle::Pending
        | ConversationLifecycle::Streaming
        | ConversationLifecycle::Active
        | ConversationLifecycle::Waiting => {
            let basis = turn.blocks().iter().find_map(|block| match block {
                TurnBlock::TurnStatus(status) => status.active_started_at_ms,
                _ => None,
            });
            live_status_copy(TurnNarration::Working, basis, frame_now_ms)
                .unwrap_or_else(|| "Working".to_owned())
        }
    }
}

/// Engine policy reducing one raw reasoning summary to its thinking line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SummaryLinePolicy {
    /// Codex and every other engine: the newest headline or the newest
    /// paragraph's first finished sentence ([`summary_line`]).
    Sentence,
    /// Claude public thinking summaries: the first meaningful line, accepted
    /// without terminal punctuation ([`claude_label_line`]), trimmed to its
    /// first clause ([`claude_first_clause`]) so the line reads as a short
    /// title.
    FirstLine,
}

impl SummaryLinePolicy {
    /// Resolves the policy from the turn's typed engine identity only; display
    /// labels never select it. Absent or other engines keep the sentence one.
    #[must_use]
    pub fn for_engine(engine: Option<EngineId>) -> Self {
        match engine {
            Some(EngineId::Claude) => Self::FirstLine,
            _ => Self::Sentence,
        }
    }

    /// Reduces one raw summary under this policy.
    #[must_use]
    pub fn reduce(self, summary: &str) -> Option<String> {
        match self {
            Self::Sentence => summary_line(summary),
            Self::FirstLine => claude_label_line(summary).map(|label| claude_first_clause(&label)),
        }
    }
}

/// Reduces the raw scene summary to the one thinking line, if any.
///
/// The turn's typed engine selects the [`SummaryLinePolicy`]; a summary
/// that reduces to nothing yields `None` so the caller falls back to the
/// narration, exactly like the reference. Copy and visibility decisions both
/// call this one function.
#[must_use]
pub fn status_summary_copy(summary: Option<&str>, engine: Option<EngineId>) -> Option<String> {
    let policy = SummaryLinePolicy::for_engine(engine);
    summary.and_then(|summary| policy.reduce(summary))
}

/// Returns the exact pre-response provider wait label.
///
/// Mirrors the reference `waiting_label_for`: a known engine names the wait,
/// unattributed work keeps the generic provider line. Elapsed counting lives
/// in the group header, never in this sentence.
#[must_use]
pub fn provider_wait_copy(engine_label: Option<&str>) -> String {
    match engine_label {
        Some(engine) => format!("Waiting for {engine} to respond…"),
        None => "Waiting for provider to respond…".to_owned(),
    }
}

/// Returns the live header owned by the turn's latest work group, if any.
///
/// Combines the ownership rule ([`owning_group_index`]) with the elapsed
/// derivation ([`live_group_header_copy`]) so render and scroll-identity code
/// share one decision point.
#[must_use]
pub fn turn_owner_header(turn: &TurnScene, frame_now_ms: Option<i64>) -> Option<String> {
    owning_group_index(turn)?;
    let (narration, basis) = turn.blocks().iter().find_map(|block| match block {
        TurnBlock::TurnStatus(status) => Some((status.narration, status.active_started_at_ms)),
        _ => None,
    })?;
    live_group_header_copy(narration, basis, frame_now_ms)
}

/// Computes the exact status row copy for one scene status block: summary
/// reduced under the block's typed engine policy, engine-named wait, or live
/// narration. Render and scroll-identity code share this one decision.
#[must_use]
pub fn turn_status_block_copy(status: &TurnStatusBlock, now_ms: Option<i64>) -> Option<String> {
    status_copy(
        status.narration,
        status.active_started_at_ms,
        now_ms,
        status_summary_copy(status.reasoning_summary.as_deref(), status.engine),
        status.engine_label.as_deref(),
    )
}

/// Returns whether one scene status block paints a child row, from the same
/// inputs the copy decision uses.
///
/// Render and scroll-identity code share this one entry point: any
/// divergence misaligns measured child bounds with their identities.
#[must_use]
pub fn turn_status_block_paints(
    turn: Option<&TurnScene>,
    status: &TurnStatusBlock,
    turn_has_work_group: bool,
    now_ms: Option<i64>,
) -> bool {
    let copy = turn_status_block_copy(status, now_ms);
    let owner_header = matches!(
        status.narration,
        TurnNarration::Thinking | TurnNarration::Working
    )
    .then(|| turn.and_then(|turn| turn_owner_header(turn, now_ms)))
    .flatten();
    turn_status_paints(
        turn_has_work_group,
        status.narration,
        copy.as_deref(),
        owner_header.as_deref(),
    )
}

/// Computes status row copy for callers without a typed engine: the summary
/// reduces under the sentence policy, as for every non-Claude engine.
///
/// Unfinished or absent summaries fall back through the narration path, so
/// this returns `None` exactly when no row paints for copy reasons.
#[must_use]
pub fn turn_status_copy_text(
    narration: TurnNarration,
    active_started_at_ms: Option<i64>,
    frame_now_ms: Option<i64>,
    reasoning_summary: Option<&str>,
    engine_label: Option<&str>,
) -> Option<String> {
    let summary = status_summary_copy(reasoning_summary, None);
    status_copy(
        narration,
        active_started_at_ms,
        frame_now_ms,
        summary,
        engine_label,
    )
}

fn status_copy(
    narration: TurnNarration,
    active_started_at_ms: Option<i64>,
    frame_now_ms: Option<i64>,
    summary: Option<String>,
    engine_label: Option<&str>,
) -> Option<String> {
    summary.or_else(|| match narration {
        TurnNarration::ProviderWait => Some(provider_wait_copy(engine_label)),
        _ => live_status_copy(narration, active_started_at_ms, frame_now_ms),
    })
}

/// Returns whether a turn status block paints a child row.
///
/// Combines the structural visibility rule with the header-ownership rule.
/// Render and scroll-identity code share this decision point: any divergence
/// misaligns measured child bounds with their identities.
#[must_use]
pub fn turn_status_paints(
    turn_has_work_group: bool,
    narration: TurnNarration,
    copy: Option<&str>,
    owner_header: Option<&str>,
) -> bool {
    if !status_row_visible(turn_has_work_group, narration) {
        return false;
    }
    match narration {
        TurnNarration::Thinking | TurnNarration::Working => {
            !status_duplicates_owner(copy, owner_header)
        }
        _ => true,
    }
}

/// Returns whether a live status row duplicates the owning group header.
///
/// Distinct lines (an elapsed header beside a summary narration) both paint;
/// identical lines paint once in the header. This keeps the renderer correct
/// under both scene generations: the current scene emits the same words in
/// both places, while a suppressing build omits one side entirely.
#[must_use]
pub fn status_duplicates_owner(status_copy: Option<&str>, owner_header: Option<&str>) -> bool {
    matches!(
        (status_copy, owner_header),
        (Some(status), Some(header)) if status == header
    )
}

/// Returns whether a status row paints for one narration in a turn that may
/// already carry its line in a work-group header.
///
/// Terminal narrations (the durations, `Failed`, `Interrupted`, `Cancelled`)
/// prefer the group header, which titles the collapsed work with the turn's
/// own outcome. Live Working rows are decided by content, not by phase:
/// [`status_duplicates_owner`] suppresses only the identical duplicate, so a
/// summary narration beside an elapsed header still paints. Live Thinking
/// rows always stand down behind the owning header. `Quiet` and
/// `StreamingSuppression` never paint.
#[must_use]
pub fn status_row_visible(turn_has_work_group: bool, narration: TurnNarration) -> bool {
    match turn_status_copy(narration) {
        None => false,
        Some(_) => match narration {
            TurnNarration::WorkedFor { .. }
            | TurnNarration::ThoughtFor { .. }
            | TurnNarration::Failed
            | TurnNarration::Interrupted
            | TurnNarration::Cancelled => !turn_has_work_group,
            _ => true,
        },
    }
}
