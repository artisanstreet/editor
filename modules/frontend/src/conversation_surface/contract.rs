//! Surface contract for the conversation transcript: typed actions,
//! observations, block kinds, selectors, and pure copy helpers.

use super::*;
use artisan_ui::inline_code_text::claude_label_line;

/// Stable debug selector for the conversation surface root.
pub const CONVERSATION_SURFACE_SELECTOR: &str = "artisan-conversation-surface";

/// Stable debug selector derived by [`ScrollArea`] for the transcript viewport.
pub const CONVERSATION_VIEWPORT_SELECTOR: &str = "artisan-conversation-surface-viewport";

/// Stable debug selector for the detached-reader jump control.
pub const JUMP_TO_LATEST_SELECTOR: &str = "artisan-conversation-surface-jump-to-latest";

/// Stable debug selector for the loaded-turn navigator rail.
pub const TURN_NAVIGATOR_SELECTOR: &str = "artisan-conversation-surface-turn-navigator";

/// Stable debug-selector prefix for one navigator control; the target's
/// scene or item identity is appended after a `-` separator.
pub const TURN_NAVIGATOR_CONTROL_PREFIX: &str =
    "artisan-conversation-surface-turn-navigator-control";

/// Stable debug selector for the navigator's own capped label list.
pub const TURN_NAVIGATOR_LIST_SELECTOR: &str = "artisan-conversation-surface-turn-navigator-list";

/// Stable debug selector for the navigator's shared hover pill.
pub const TURN_NAVIGATOR_HOVER_SELECTOR: &str = "artisan-conversation-surface-turn-navigator-hover";

/// Alias for callers that use the shorter root-selector vocabulary.
pub const ROOT_SELECTOR: &str = CONVERSATION_SURFACE_SELECTOR;

/// Alias for callers that use the shorter viewport-selector vocabulary.
pub const VIEWPORT_SELECTOR: &str = CONVERSATION_VIEWPORT_SELECTOR;

/// Stable debug selector for the transcript end-space spacer.
pub const TRANSCRIPT_END_SPACE_SELECTOR: &str = "artisan-conversation-surface-end-space";

/// Transcript reading-column width shared with the thread frame.
///
/// The screen mounts the host at full card width so the navigator rail
/// anchors to the card; each turn root below re-applies this max width with
/// auto margins, keeping the identical centered column for standalone
/// fixtures. Mirrors `PROSE_WIDTH_PX` (`--prose-width: 48rem`).
pub(super) const TRANSCRIPT_PROSE_WIDTH_PX: f32 = 768.0;

/// Transcript reading-column gutters shared with the thread frame (`px-6`).
pub(super) const TRANSCRIPT_GUTTER_PX: f32 = 24.0;

/// Top spacing inside the scroll content, shared with the thread frame.
///
/// The legacy `pt-10` sat statically above the scroll viewport; it lives in
/// the scroll content instead so the full host spans the actual card. It is
/// container padding rather than a child, so turn/spacer child indices —
/// and every identity bound to them — are preserved exactly.
pub(super) const TRANSCRIPT_PAD_TOP_PX: f32 = 40.0;

/// Base transcript end space in px, mirroring
/// `ConversationBaseEndSpacePixels`: the transcript always keeps at least
/// this much scrollable room after its last turn.
pub const TRANSCRIPT_END_SPACE_PX: f32 = 192.0;

/// Turn-to-viewport top inset in px, mirroring
/// `ConversationTurnTopInsetPixels`.
pub const TRANSCRIPT_TURN_TOP_INSET_PX: f64 = 16.0;

/// Computes end-space height with the reference anchoring formula.
///
/// Returns at least the base height, growing so an anchored turn at
/// `item_top` can still reach the top inset of a `viewport_height` viewport
/// once the spacer itself starts at `end_space_top`. Pure and total; live
/// measurement wiring (which turn is anchored, where the spacer paints)
/// belongs to the viewport/host lane, which feeds this surface.
#[must_use]
pub fn end_space_height(viewport_height: f64, item_top: f64, end_space_top: f64) -> f64 {
    f64::from(TRANSCRIPT_END_SPACE_PX)
        .max(item_top + viewport_height - TRANSCRIPT_TURN_TOP_INSET_PX - end_space_top)
}

/// Group name shared by a turn root and its hover-revealed footer.
///
/// The reference footer reveals on `group-hover/turn` and
/// `group-focus-within/turn`; the turn root carries this group and the footer
/// refines to full opacity on group hover (plus its own focus handle for the
/// keyboard path).
pub const TURN_GROUP: &str = "artisan-conversation-turn";

/// Stable debug-selector suffix for the footer copy control.
pub const FOOTER_COPY_SELECTOR_SUFFIX: &str = "footer-copy";

/// Maximum number of typed observations retained before new observations are
/// refused. Dropping the newest observation keeps the outbox bounded and
/// preserves the order of observations already accepted by the controller.
pub const CONVERSATION_SURFACE_MAX_ACTIONS: usize = 256;

/// Maximum number of typed scroll targets retained until a matching painted
/// render.
///
/// These commands are transient and intentionally have a separate bound from
/// the surface action outbox. A retiring surface drops this queue with the
/// rest of its render state.
pub(crate) const CONVERSATION_SURFACE_MAX_SCROLL_TARGETS: usize = 64;

/// A stable scene or item target used by viewport and scroll observations.
///
/// The target deliberately carries identity only. Rendered text, filesystem
/// paths, and other scene payloads never cross the surface action boundary.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ConversationSurfaceTarget {
    /// A scene-owned identity such as a turn, card, work group, or steering
    /// label.
    Scene(SceneId),
    /// A domain item identity used for exact transcript anchoring.
    Item(ItemId),
}

/// A bounded observation of the currently visible transcript identities.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ViewportObservation {
    /// The first visible stable target, if the viewport has one.
    pub first_visible: Option<ConversationSurfaceTarget>,
    /// The last visible stable target, if the viewport has one.
    pub last_visible: Option<ConversationSurfaceTarget>,
    /// Whether the viewport is currently at the transcript's bottom edge.
    ///
    /// This is deliberately explicit. The viewport controller must not infer
    /// follow-tail state from the identity observations or from GPUI scroll
    /// completion.
    pub at_bottom: bool,
}

/// Typed effects emitted by the transcript surface.
///
/// These are requests and observations only. In particular, a disclosure
/// click does not change the scene or any local open/closed bit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConversationSurfaceAction {
    /// Ask the state controller to apply a requested controlled disclosure
    /// value for one stable scene identity.
    DisclosureToggleRequested {
        /// Stable group/card/message identity.
        id: SceneId,
        /// The value requested by the controlled trigger.
        requested_open: bool,
    },
    /// Report the visible identity bounds of the transcript viewport.
    ViewportObserved(ViewportObservation),
    /// Measured content or viewport dimensions changed, without reader input.
    ViewportExtentChanged,
    /// Ask the viewport controller to return to the latest transcript content.
    JumpToLatestRequested,
    /// Direct wheel input interrupted the animated jump.
    BottomScrollInterrupted,
    /// Ask the surrounding controller to move the viewport to a stable target.
    ScrollIntent {
        /// Stable scene or item target.
        target: ConversationSurfaceTarget,
    },
    /// The turn footer became visible through pointer hover or keyboard focus.
    ///
    /// The host should take one clock sample for the relative age (mirroring
    /// [`TurnFooterInput::Hover`](crate::conversation_turn_footer_policy::TurnFooterInput)/`Focus`)
    /// and mirror the formatted age back through
    /// [`Self::set_footer_relative_age`]. There is no timer in this surface.
    TurnFooterRevealed {
        /// Owning turn of the revealed footer.
        turn: TurnId,
    },
    /// The footer copy control was activated with the exact settlement bytes.
    ///
    /// The host owns the clipboard write and mirrors the outcome back through
    /// [`Self::set_footer_copy_message`].
    TurnFooterCopyRequested {
        /// Owning turn of the footer.
        turn: TurnId,
        /// Exact response payload from the footer settlement.
        text: String,
    },
    /// One render pass observed assistant links with no resolved title.
    ///
    /// The host forwards each canonical URL to the transport's bounded
    /// rich-link resolver; results return through
    /// [`ConversationSurface::set_rich_link_title`] or
    /// [`ConversationSurface::set_rich_link_failure`].
    ResolveRichLinks {
        /// Canonical absolute HTTP(S) URLs that need a resolve attempt.
        urls: Vec<String>,
    },
}

/// Every block family that the native renderer knows how to paint.
///
/// This small tag projection is useful for deterministic tests and review: it
/// contains no payload and does not become a second scene tree.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RenderedBlockKind {
    /// User message.
    UserMessage,
    /// Assistant message.
    AssistantMessage,
    /// Grouped reasoning/activity/work-session entries.
    WorkGroup,
    /// Compaction card.
    Compaction,
    /// Changed-files card.
    ChangeSet,
    /// Plan/checklist card.
    Plan,
    /// Approval card.
    Approval,
    /// Question card.
    Question,
    /// Error card.
    Error,
    /// Usage interruption card.
    UsageInterruption,
    /// Model transition card.
    ModelTransition,
    /// Native fact card.
    NativeFact,
    /// Steering label.
    SteeringLabel,
    /// Per-turn status row.
    TurnStatus,
    /// Per-turn footer.
    TurnFooter,
}

/// Returns the closed renderer kind for one scene block.
#[must_use]
pub const fn rendered_block_kind(block: &TurnBlock) -> RenderedBlockKind {
    match block {
        TurnBlock::UserMessage(_) => RenderedBlockKind::UserMessage,
        TurnBlock::AssistantMessage(_) => RenderedBlockKind::AssistantMessage,
        TurnBlock::WorkGroup(_) => RenderedBlockKind::WorkGroup,
        TurnBlock::Compaction(_) => RenderedBlockKind::Compaction,
        TurnBlock::ChangeSet(_) => RenderedBlockKind::ChangeSet,
        TurnBlock::Plan(_) => RenderedBlockKind::Plan,
        TurnBlock::Approval(_) => RenderedBlockKind::Approval,
        TurnBlock::Question(_) => RenderedBlockKind::Question,
        TurnBlock::Error(_) => RenderedBlockKind::Error,
        TurnBlock::UsageInterruption(_) => RenderedBlockKind::UsageInterruption,
        TurnBlock::ModelTransition(_) => RenderedBlockKind::ModelTransition,
        TurnBlock::NativeFact(_) => RenderedBlockKind::NativeFact,
        TurnBlock::SteeringLabel(_) => RenderedBlockKind::SteeringLabel,
        TurnBlock::TurnStatus(_) => RenderedBlockKind::TurnStatus,
        TurnBlock::TurnFooter(_) => RenderedBlockKind::TurnFooter,
    }
}

/// Projects the exact ordered block kinds from the accepted scene.
///
/// The loop intentionally follows `turn_scenes` and `blocks` without sorting,
/// grouping, filtering, or otherwise reconstructing scene policy.
#[must_use]
pub fn ordered_block_kinds(scene: &ConversationScene) -> Vec<RenderedBlockKind> {
    scene
        .turn_scenes()
        .iter()
        .flat_map(|turn| turn.blocks().iter().map(rendered_block_kind))
        .collect()
}

/// Returns a stable selector for one turn.
#[must_use]
pub fn turn_selector(turn_id: &TurnId) -> String {
    format!("{CONVERSATION_SURFACE_SELECTOR}-turn-{}", turn_id.as_str())
}

/// Returns a stable selector for one block in one turn.
///
/// Only stable identities, ordinals represented by explicit indices, and
/// closed variant names enter selectors. Scene text is always rendered as a
/// child value and is never used as an element id or debug selector.
#[must_use]
pub fn block_selector(turn_id: &TurnId, block: &TurnBlock) -> String {
    let turn = turn_selector(turn_id);
    match block {
        TurnBlock::UserMessage(block) => format!("{turn}-block-user-{}", block.id.as_str()),
        TurnBlock::AssistantMessage(block) => {
            format!("{turn}-block-assistant-{}", block.id.as_str())
        }
        TurnBlock::WorkGroup(block) => {
            format!(
                "{turn}-block-work-{}",
                work_group_selector_id(turn_id, block)
            )
        }
        TurnBlock::Compaction(block) => format!("{turn}-block-compaction-{}", block.id.as_str()),
        TurnBlock::ChangeSet(block) => format!("{turn}-block-change-{}", block.id.as_str()),
        TurnBlock::Plan(block) => format!("{turn}-block-plan-{}", block.id.as_str()),
        TurnBlock::Approval(block) => format!("{turn}-block-approval-{}", block.id.as_str()),
        TurnBlock::Question(block) => format!("{turn}-block-question-{}", block.id.as_str()),
        TurnBlock::Error(block) => format!("{turn}-block-error-{}", block.id.as_str()),
        TurnBlock::UsageInterruption(block) => {
            format!("{turn}-block-usage-interruption-{}", block.id.as_str())
        }
        TurnBlock::ModelTransition(block) => {
            format!("{turn}-block-model-transition-{}", block.id.as_str())
        }
        TurnBlock::NativeFact(block) => format!("{turn}-block-native-fact-{}", block.id.as_str()),
        TurnBlock::SteeringLabel(block) => steering_selector(turn_id, &block.id),
        TurnBlock::TurnStatus(_) => status_selector(turn_id),
        TurnBlock::TurnFooter(_) => footer_selector(turn_id),
    }
}

/// Returns a stable selector for one changed-file row.
#[must_use]
pub fn changed_file_selector(card_id: &SceneId, index: usize) -> String {
    format!(
        "{CONVERSATION_SURFACE_SELECTOR}-change-{}-file-{index}",
        card_id.as_str()
    )
}

/// Returns a stable selector for one steering label at its scene position.
#[must_use]
pub fn steering_selector(turn_id: &TurnId, steering_id: &SceneId) -> String {
    format!(
        "{}-steering-{}",
        turn_selector(turn_id),
        steering_id.as_str()
    )
}

/// Returns a stable selector for one turn's status row.
#[must_use]
pub fn status_selector(turn_id: &TurnId) -> String {
    format!("{}-status", turn_selector(turn_id))
}

/// Returns a stable selector for one turn's footer.
#[must_use]
pub fn footer_selector(turn_id: &TurnId) -> String {
    format!("{}-footer", turn_selector(turn_id))
}

pub(super) fn work_group_selector_id(turn_id: &TurnId, block: &WorkGroupBlock) -> String {
    block
        .items
        .first()
        .map(work_item_id)
        .map_or_else(|| turn_id.as_str().to_owned(), |id| id.as_str().to_owned())
}

pub(super) fn work_item_id(item: &WorkItem) -> &SceneId {
    match item {
        WorkItem::Reasoning { id, .. }
        | WorkItem::Activity { id, .. }
        | WorkItem::WorkSession { id, .. } => id,
    }
}

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
pub fn work_group_header_copy(
    label: Option<crate::conversation_scene::WorkGroupLabel>,
) -> Option<String> {
    label.map(format_work_group_label)
}

/// Returns the scene-owned display label for one terminal work-group label.
#[must_use]
pub fn format_work_group_label(label: crate::conversation_scene::WorkGroupLabel) -> String {
    match label {
        crate::conversation_scene::WorkGroupLabel::WorkedFor { millis } => {
            format!("Worked for {}", format_elapsed_millis(millis))
        }
        crate::conversation_scene::WorkGroupLabel::ThoughtFor { millis } => {
            format!("Thought for {}", format_elapsed_millis(millis))
        }
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

/// Resolves the stored shimmer preference against the live system signal.
///
/// An explicit `Reduced` override always wins; otherwise the window's
/// reduced-motion state decides. Settled rows bypass this entirely through
/// the shimmer's inactive path.
#[must_use]
pub const fn effective_status_motion(stored: MotionPolicy, system_reduced: bool) -> MotionPolicy {
    match stored {
        MotionPolicy::Reduced => MotionPolicy::Reduced,
        MotionPolicy::Full => {
            if system_reduced {
                MotionPolicy::Reduced
            } else {
                MotionPolicy::Full
            }
        }
    }
}
///
/// Terminal `WorkedFor`/`ThoughtFor` paint only when the turn has no work
/// group: the scene attaches the same duration as the latest group header and
/// the reference settles to the header alone, so painting both would double
/// the duration line.
/// Returns the live Thinking/Working header owned by at most one work group.
///
/// Terminal labels always win through [`work_group_header_copy`]; this covers
/// the live line only. Any other narration yields no group header, so the
/// turn status row below remains its single owner.
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
/// groups render items only, so the live line paints once per turn.
#[must_use]
pub fn owning_group_index(turn: &TurnScene) -> Option<usize> {
    turn.blocks().iter().rposition(|block| match block {
        TurnBlock::WorkGroup(group) => !group.superseded,
        _ => false,
    })
}

/// Engine policy reducing one raw reasoning summary to its thinking line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SummaryLinePolicy {
    /// Codex and every other engine: the newest headline or the newest
    /// paragraph's first finished sentence ([`summary_line`]).
    Sentence,
    /// Claude public thinking summaries: the first meaningful line, accepted
    /// without terminal punctuation ([`claude_label_line`]).
    FirstLine,
}

impl SummaryLinePolicy {
    /// Resolves the policy from the turn's validated engine label.
    ///
    /// The label is the roster display name every engine-label setter
    /// derives from the run's engine id; an absent or unknown label keeps
    /// the existing sentence policy.
    #[must_use]
    pub fn for_engine_label(engine_label: Option<&str>) -> Self {
        if engine_label
            == Some(crate::native_profile_usage::profile_usage_display_name(
                "claude",
            ))
        {
            Self::FirstLine
        } else {
            Self::Sentence
        }
    }

    /// Reduces one raw summary under this policy.
    #[must_use]
    pub fn reduce(self, summary: &str) -> Option<String> {
        match self {
            Self::Sentence => summary_line(summary),
            Self::FirstLine => claude_label_line(summary),
        }
    }
}

/// Reduces the raw scene summary to the one thinking line, if any.
///
/// The turn's engine label selects the [`SummaryLinePolicy`]; a summary that
/// reduces to nothing yields `None` so the caller falls back to the
/// narration, exactly like the reference. Copy and visibility decisions both
/// call this one function.
#[must_use]
pub fn status_summary_copy(summary: Option<&str>, engine_label: Option<&str>) -> Option<String> {
    let policy = SummaryLinePolicy::for_engine_label(engine_label);
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

/// Computes the exact status row copy: reduced scene summary, engine-named
/// wait, or live narration copy.
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
    match status_summary_copy(reasoning_summary, engine_label) {
        Some(summary) => Some(summary),
        None => match narration {
            TurnNarration::ProviderWait => Some(provider_wait_copy(engine_label)),
            _ => live_status_copy(narration, active_started_at_ms, frame_now_ms),
        },
    }
}

/// Returns whether a turn status block paints a child row.
///
/// Combines the structural visibility rule with the identical-duplicate
/// suppression. Render and scroll-identity code share this decision point:
/// any divergence misaligns measured child bounds with their identities.
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
    if !matches!(narration, TurnNarration::Thinking | TurnNarration::Working) {
        return true;
    }
    !status_duplicates_owner(copy, owner_header)
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
/// Terminal durations prefer the group header. Live Thinking/Working rows are
/// decided by content, not by phase: [`status_duplicates_owner`] suppresses
/// only the identical duplicate, so a summary narration beside an elapsed
/// header still paints. `Quiet` and `StreamingSuppression` never paint.
#[must_use]
pub fn status_row_visible(turn_has_work_group: bool, narration: TurnNarration) -> bool {
    match turn_status_copy(narration) {
        None => false,
        Some(_) => match narration {
            TurnNarration::WorkedFor { .. } | TurnNarration::ThoughtFor { .. } => {
                !turn_has_work_group
            }
            _ => true,
        },
    }
}

/// Returns the per-turn key for footer mirrors and footer focus handles.
#[must_use]
pub fn footer_key(turn_id: &TurnId) -> String {
    format!("turn-footer:{}", turn_id.as_str())
}

/// Applies one footer hover observation to the retained reveal key.
///
/// The footer lives outside its turn group's bounds, so group hover alone
/// drops while the pointer travels down to the controls. Retaining the
/// footer's own hover here keeps it revealed for that trip; leaving the
/// footer, or hovering a different turn's footer, releases it. Returns
/// whether the visible state changed.
pub(super) fn footer_hover_transition(
    revealed: &mut Option<String>,
    key: &str,
    hovered: bool,
) -> bool {
    if hovered {
        if revealed.as_deref() == Some(key) {
            return false;
        }
        *revealed = Some(key.to_owned());
        true
    } else if revealed.as_deref() == Some(key) {
        *revealed = None;
        true
    } else {
        false
    }
}

/// Returns whether a footer block paints a child.
///
/// Only an eligible settlement paints: the reference renders no footer at all
/// for unsettled turns, and this renderer keeps no placeholder or gap slot.
#[must_use]
pub fn footer_has_content(block: &TurnFooterBlock) -> bool {
    block.settlement.is_some()
}

/// Returns the settlement carried by a footer block, if it paints one.
#[must_use]
pub fn footer_settlement(block: &TurnFooterBlock) -> Option<&TurnFooterSettlement> {
    block.settlement.as_ref()
}

/// Host-mirrored view state for one settled turn footer.
///
/// The settlement facts (response bytes, settled timestamp) stay scene-owned;
/// this mirror carries only the adapter-formatted relative age and the
/// reader-facing copy status staged by the controller through
/// [`ConversationSurface::set_footer_relative_age`] and
/// [`ConversationSurface::set_footer_copy_message`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TurnFooterMirror {
    /// Adapter-formatted relative age; empty until the host samples a clock.
    pub relative_age: String,
    /// Conservative throughput estimate; absent when measurement is unreliable.
    pub token_speed: Option<String>,
    /// Reader-facing copy status; empty unless a clipboard write failed.
    pub copy_message: String,
    /// Local feedback starts only after the clipboard adapter reports success.
    pub copied_at: Option<std::time::Instant>,
}

/// Returns the stable status badge text for a changed-file status.
#[must_use]
pub const fn file_change_status_label(status: FileChangeStatus) -> &'static str {
    match status {
        FileChangeStatus::Added => "Added",
        FileChangeStatus::Modified => "Modified",
        FileChangeStatus::Removed => "Removed",
        FileChangeStatus::Renamed => "Renamed",
    }
}

/// Copy confirmation uses the shared icon-swap recipe, holds, then returns.
#[expect(
    clippy::cast_possible_truncation,
    reason = "unit opacity is bounded before narrowing to GPUI f32"
)]
pub(super) fn copy_feedback_progress(elapsed: Duration, motion: MotionPolicy) -> f32 {
    let hold_until = Duration::from_millis(1500);
    let MotionPlan::Animate(animation) = motion.resolve(MotionRecipe::IconSwap) else {
        return if elapsed < hold_until { 1.0 } else { 0.0 };
    };
    let duration = animation.duration().as_secs_f64();
    let progress = if elapsed < animation.duration() {
        elapsed.as_secs_f64() / duration
    } else if elapsed < hold_until {
        1.0
    } else {
        1.0 - (elapsed.saturating_sub(hold_until).as_secs_f64() / duration).min(1.0)
    };
    animation.curve().sample(progress) as f32
}
