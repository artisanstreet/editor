//! Surface contract for the conversation transcript: typed actions,
//! observations, block kinds, and selectors.
//!
//! The pure status copy and visibility policy lives beside this file in
//! [`status_copy`]; this module re-exports it so the surface's public paths
//! stay one vocabulary.

use super::*;

#[path = "status_copy.rs"]
mod status_copy;

pub use status_copy::*;

/// Stable debug selector for the conversation surface root.
pub const CONVERSATION_SURFACE_SELECTOR: &str = "artisan-conversation-surface";

/// Stable debug selector derived by [`ScrollArea`] for the transcript viewport.
pub const CONVERSATION_VIEWPORT_SELECTOR: &str = "artisan-conversation-surface-viewport";

/// Stable debug selector for the detached-reader jump control.
pub const JUMP_TO_LATEST_SELECTOR: &str = "artisan-conversation-surface-jump-to-latest";

/// Stable debug selector for the loaded-turn navigator's tick rail.
pub const TURN_NAVIGATOR_SELECTOR: &str = "artisan-conversation-surface-turn-navigator";

/// Stable debug-selector prefix for one navigator control; the target's
/// scene or item identity is appended after a `-` separator.
pub const TURN_NAVIGATOR_CONTROL_PREFIX: &str =
    "artisan-conversation-surface-turn-navigator-control";

/// Stable debug selector for the navigator's floating menu card.
pub const TURN_NAVIGATOR_MENU_SELECTOR: &str = "artisan-conversation-surface-turn-navigator-menu";

/// Stable debug selector for the menu's own capped, scrolling label list.
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

/// Assistant prose measure inside the reading column
/// (`--prose-body-width: prose − 6rem`).
///
/// Only reading blocks stop here. A code fence spans the whole turn column
/// (the reading column less its gutters), so the Markdown renderer is handed
/// the full column and bounds prose to this measure itself.
pub(super) const TRANSCRIPT_PROSE_BODY_WIDTH_PX: f32 = 672.0;

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
    /// The reader is near the start of the loaded turns: older ones, if the
    /// thread has any, should be read so scrolling on finds them there.
    EarlierTurnsWanted,
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

/// A session continuation selects by its own segment anchor so it never
/// repeats the first segment's selector; every other group keeps the
/// first-item derivation with the turn id as the session fallback.
pub(super) fn work_group_selector_id(turn_id: &TurnId, block: &WorkGroupBlock) -> String {
    if let Some(continuation) = &block.continuation {
        return continuation.as_str().to_owned();
    }
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
