//! Native GPUI conversation transcript surface.
//!
//! [`ConversationSurface`] is a deliberately thin renderer over the accepted
//! [`ConversationScene`](crate::conversation_scene::ConversationScene). The
//! scene owns ordering, grouping, disclosure values, narration, and bounded
//! display text; this module only paints those already-decided values and
//! reports typed interaction observations back to its controller.
//!
//! No durable state, domain records, network work, or clock reads belong here.
//! Message-body Markdown parsing is delegated to the shared renderer, and
//! local disclosure state never becomes a second source of truth. A
//! replacement scene is the only source of truth after a disclosure request
//! has been emitted.

#![allow(clippy::module_name_repetitions)]

use artisan_assets::AssetId;
use artisan_domain::{
    Command, ConversationLifecycle, ItemId, OBSERVATION_ANSWER_MAX_BYTES, ObservationId, RequestId,
    RunId, ThreadId, TurnId,
};
use artisan_protocol::{
    ErrorCode, ErrorDetail, ProtocolFailure, RespondApprovalReceipt, RespondQuestionReceipt,
};
use artisan_ui::alert::{Alert, AlertStyle, AlertVariant};
use artisan_ui::asset_seam::asset_glyph;
use artisan_ui::badge::{BadgeStyle, outline_badge};
use artisan_ui::gradient::{hover_fill_gradient, vertical_gradient};
use artisan_ui::inline_code_text::{inline_runs, summary_line};
use artisan_ui::button::{
    AccessibleLabel, Button, ButtonContent, ButtonSize, ButtonVariant, FocusVisibility,
};
use artisan_ui::card::{CardStyle, compact_card, compact_card_content};
use artisan_ui::collapsible::Collapsible;
use artisan_ui::input_state::TextInputState;
use artisan_ui::markdown_renderer::{MarkdownBodyTone, MarkdownRenderer, RichLinkTitleSource};
use artisan_ui::motion::{MotionCurve, MotionDuration, MotionPlan, MotionPolicy, MotionRecipe};
use artisan_ui::scroll_area::ScrollArea;
use artisan_ui::selectable_text::SelectableText;
use artisan_ui::separator::{SeparatorAxis, separator};
use artisan_ui::shimmer_text::ShimmerText;
use artisan_ui::theme::{
    ArtisanTheme, ProseTypography, RadiusStep, RadiusTokens, SurfaceStep, ThemeMode,
};
use gpui::{
    Animation, AnimationExt, AnyElement, BoxShadow, Context, Div, ElementId, Entity, Filter,
    FocusHandle, FontWeight, IntoElement, Modifiers, MouseMoveEvent, Render, ScrollAnchor,
    ScrollHandle, ScrollWheelEvent, SharedString, Stateful, Window, canvas, deferred, div, point,
    prelude::{
        InteractiveElement as _, ParentElement as _, StatefulInteractiveElement as _, Styled as _,
    },
    px,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use crate::approval_presentation::ApprovalKind as PresentationApprovalKind;
use crate::conversation_scene::{
    ActivityCategory, ChangeSetBlock, CompactionBlock, ConversationScene, ErrorBlock,
    FileChangeStatus, ModelTransitionBlock, NativeFactBlock, PlanBlock, QuestionBlock,
    SceneDisclosure, SceneFileChange, SceneId, SessionDetail, SteeringBlock, TurnBlock,
    TurnFooterBlock, TurnFooterSettlement, TurnNarration, TurnScene, UsageInterruptionBlock,
    UserMessageBlock, WorkGroupBlock, WorkItem, activity_category, activity_presentation_label,
};
use crate::conversation_scroll_position::conversation_is_following;
use crate::conversation_turn_navigator::{
    ConversationSnapshotInput, ConversationTurnInput, ConversationTurnOffset,
    LoadedConversationItemInput, active_conversation_turn, conversation_turn_markers,
};
use crate::engine_approve_ui::{
    APPROVAL_DENY_LABEL, APPROVAL_DENYING_LABEL, AnswerFlight, AnswerKind, AnswerPairing,
    AnswerSettlement, QUESTION_ANSWER_LABEL, QUESTION_INPUT_PLACEHOLDER, RespondApprovalAction,
    RespondQuestionAction, approval_command, mint_answer_request_id, pair_answer_failure,
    pair_approval_answer, pair_question_answer, pending_approval_label, question_command,
};
use crate::conversation_turn_footer_policy::{COPY_RESPONSE_LABEL, TURN_ACTIONS_LABEL};
use crate::engine_observation_state::EngineObservationState;
use crate::native_composer_material::{
    GlassStrength, glass_blur_radius, glass_card_shadows, glass_foreground_base,
    glass_highlight_layer, glass_material_layer,
};
use crate::native_model_selector::{
    HoverRect, PickerScrollState, SlidingHoverState,
};
use crate::native_transport_service::{CommandSendError, NativeTransportCommand};
use crate::rich_link_titles::RichLinkTitleTable;

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
const TRANSCRIPT_PROSE_WIDTH_PX: f32 = 768.0;

/// Transcript reading-column gutters shared with the thread frame (`px-6`).
const TRANSCRIPT_GUTTER_PX: f32 = 24.0;

/// Top spacing inside the scroll content, shared with the thread frame.
///
/// The legacy `pt-10` sat statically above the scroll viewport; it lives in
/// the scroll content instead so the full host spans the actual card. It is
/// container padding rather than a child, so turn/spacer child indices —
/// and every identity bound to them — are preserved exactly.
const TRANSCRIPT_PAD_TOP_PX: f32 = 40.0;

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
    f64::from(TRANSCRIPT_END_SPACE_PX).max(
        item_top + viewport_height - TRANSCRIPT_TURN_TOP_INSET_PX - end_space_top,
    )
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
    /// Ask the viewport controller to return to the latest transcript content.
    JumpToLatestRequested,
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

fn work_group_selector_id(turn_id: &TurnId, block: &WorkGroupBlock) -> String {
    block
        .items
        .first()
        .map(work_item_id)
        .map_or_else(|| turn_id.as_str().to_owned(), |id| id.as_str().to_owned())
}

fn work_item_id(item: &WorkItem) -> &SceneId {
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

fn format_elapsed_seconds(total_seconds: u64) -> String {
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
        TurnNarration::Quiet => None,
        TurnNarration::ProviderWait => Some("Waiting for provider to respond…".to_owned()),
        TurnNarration::Compacting => Some("Compacting the conversation…".to_owned()),
        TurnNarration::Thinking => Some("Thinking".to_owned()),
        TurnNarration::Working => Some("Working".to_owned()),
        TurnNarration::StreamingSuppression => None,
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
pub const fn effective_status_motion(
    stored: MotionPolicy,
    system_reduced: bool,
) -> MotionPolicy {
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

/// Reduces the raw scene summary to the one thinking line, if finished.
///
/// Unfinished phases yield `None` so the caller falls back to the narration,
/// exactly like the reference.
#[must_use]
pub fn status_summary_copy(summary: Option<&str>) -> Option<String> {
    summary.and_then(summary_line)
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
    if owning_group_index(turn).is_none() {
        return None;
    }
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
    match status_summary_copy(reasoning_summary) {
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
    if !matches!(
        narration,
        TurnNarration::Thinking | TurnNarration::Working
    ) {
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
fn footer_hover_transition(revealed: &mut Option<String>, key: &str, hovered: bool) -> bool {
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
    /// Reader-facing copy status; empty unless a clipboard write failed.
    pub copy_message: String,
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

/// Native GPUI transcript surface over one immutable replacement scene.
pub struct ConversationSurface {
    scene: ConversationScene,
    message_images: Option<Entity<crate::native_message_images::NativeMessageImages>>,
    message_images_observation: Option<gpui::Subscription>,
    theme_mode: ThemeMode,
    markdown_renderer: MarkdownRenderer,
    /// Bounded resolved rich-link titles for this surface's markdown links.
    rich_link_titles: RichLinkTitleTable,
    /// Destinations observed as unresolved by the current render pass.
    ///
    /// Render-local: the probe pushes here while the renderer flattens links,
    /// and the render tail queues each new destination exactly once before
    /// notifying the host.
    rich_link_missing: RefCell<Vec<String>>,
    scroll_handle: ScrollHandle,
    transcript_focus: FocusHandle,
    disclosure_focus: FocusHandle,
    jump_to_latest_focus: FocusHandle,
    answer_focus: FocusHandle,
    jump_to_latest_visible: bool,
    last_viewport_observation: Option<ViewportObservation>,
    pending_viewport_observation: Option<ViewportObservation>,
    last_viewport_geometry: Option<ViewportGeometry>,
    viewport_observation_scheduled: bool,
    viewport_next_frame_scheduled: bool,
    actions: Vec<ConversationSurfaceAction>,
    pending_scroll_targets: Vec<ConversationSurfaceTarget>,
    /// Targets matched against painted anchors by the latest render.
    ///
    /// Render drains these from the FIFO queue and executes the GPUI anchor
    /// scroll. The same-frame prepaint listeners consume this handoff to
    /// write the exact anchor-equivalent offset synchronously, which is the
    /// only write the test harness pumps. Entries are render-local: each
    /// render clears leftovers, and retirement clears them with the queue.
    executed_scroll_targets: Vec<ConversationSurfaceTarget>,
    scroll_anchors: Vec<RenderedScrollAnchor>,
    scroll_anchor_paint_token: Option<Rc<()>>,
    /// Focus handles for loaded-turn navigator controls, keyed by stable
    /// target identity (`item:<id>` or `scene:<id>`).
    ///
    /// Handles persist while their target remains in the scene and are
    /// pruned on scene replacement; a focused control that disappears
    /// returns focus to the transcript.
    navigator_focus: HashMap<String, FocusHandle>,
    /// Whether the turn-navigator rail is expanded from ticks into labels.
    ///
    /// The reference hides labels until rail hover or row focus; at rest only
    /// the tick column paints, so full message texts never float beside the
    /// transcript.
    navigator_expanded: bool,
    /// Dedicated scroll handle for the navigator's own capped label list.
    ///
    /// Long threads scroll the rail independently of the transcript through
    /// the same bounded tracking the thread screen uses for viewports.
    navigator_scroll: ScrollHandle,
    /// Shared hover pill for navigator rows, reused from the model picker.
    ///
    /// Row probes measure into this state and the pill paints the retained
    /// flight; hovering the rail selects, leaving it hides. Reference pill
    /// behavior without a second motion invention.
    navigator_hover: Rc<RefCell<SlidingHoverState>>,
    /// Measured navigator list bounds backing pill-relative coordinates.
    ///
    /// Window-space bounds like the picker's surface bounds, so row origins
    /// subtract to list-relative pill rects with plain pixel arithmetic.
    navigator_hover_surface: Rc<RefCell<Option<gpui::Bounds<gpui::Pixels>>>>,
    /// Focused navigator row identity; selecting the pill only on arrival
    /// keeps a later trigger-zone clear from being reselected every render,
    /// even while the row retains keyboard focus.
    navigator_focused_key: Option<String>,
    /// Width-motion generation, bumped only when rail hover flips expansion.
    ///
    /// The open-keyed width clock replays on this generation, never on
    /// mount: generation zero paints the static width outright.
    navigator_width_generation: u64,
    /// Currently painted rail width, retained across hover reversals.
    ///
    /// The width animator writes every painted frame (the picker's
    /// `apply_progress` pattern), so an interrupted flight reverses from
    /// the displayed width instead of jumping to a fixed endpoint.
    navigator_width_px: Rc<RefCell<f32>>,
    /// Transition start width, frozen when the width generation bumps.
    ///
    /// The open-keyed clock replays from this value for the whole
    /// generation; resampling the retained width every render would bend
    /// the interpolation path mid-flight.
    navigator_width_from: f32,
    /// Bounded wheel-smoothing state for the transcript scroll offset.
    ///
    /// Reuses the model picker's [`PickerScrollState`] verbatim: discrete
    /// wheel ticks accumulate into one target and settle through bounded
    /// interpolation frames instead of jumping per tick.
    transcript_scroll: PickerScrollState,
    /// Whether a transcript smoothing frame is already scheduled.
    transcript_scroll_frame_scheduled: bool,
    /// Host-mirrored frame time in millis for live Thinking/Working elapsed.
    ///
    /// This is a paint-time mirror only: the surface never reads a clock and
    /// never resets the scene's authoritative `active_started_at_ms` basis.
    /// `None` renders the bare verb truthfully until the host supplies time.
    active_now_ms: Option<i64>,
    /// Motion preference for the live status shimmer and work-session
    /// disclosure flights. Defaults to `Full`; an explicit `Reduced` always
    /// wins over the system signal (see [`effective_status_motion`]). The fork
    /// exposes no OS query beyond the live window signal read at render time,
    /// so a stored `Full` follows `cx.reduce_motion()`.
    status_motion: MotionPolicy,
    /// Explicit activity-chain disclosure overrides, keyed by chain identity
    /// (the first activity's scene id).
    ///
    /// The reference keeps `open_groups` as local component state: an unset
    /// chain falls back to its failure/liveness default every render, and
    /// only a user toggle pins a value. The work-session disclosure stays
    /// scene-owned; this map is presentation-only and never mutates the
    /// scene. Entries persist while the surface owns them, exactly like the
    /// reference component state; the set is bounded by user gestures.
    trace_groups_open: RefCell<HashMap<String, bool>>,
    /// Host-mirrored footer view state keyed by [`footer_key`].
    footer_mirrors: HashMap<String, TurnFooterMirror>,
    /// Per-turn footer copy-button focus handles keyed by [`footer_key`].
    ///
    /// Handles persist while their turn remains in the scene and are pruned
    /// on render; a focused control that disappears returns focus to the
    /// transcript, mirroring `navigator_focus`.
    footer_focus: HashMap<String, FocusHandle>,
    /// Footer the pointer is currently over, keyed by [`footer_key`].
    ///
    /// The footer sits outside the turn group's bounds, so GPUI's group hover
    /// drops while the pointer travels down to it. Tracking the footer's own
    /// hover keeps it revealed for the trip and while its controls are used.
    footer_revealed: Option<String>,
    /// Focus handles for free-form question input rows, keyed by block
    /// identity text.
    ///
    /// Handles are rebuilt on scene replacement, retaining generations for
    /// surviving rows (mirroring `navigator_focus`), so per-row keystrokes
    /// route to the row that owns them while unrelated updates never steal
    /// focus.
    question_focus: HashMap<String, FocusHandle>,
    /// Explicit live answer context for engine approval/question rows.
    ///
    /// The owning thread and live run are supplied explicitly by the
    /// controller through [`Self::set_answer_context`]; nothing ambient is
    /// read. Submit affordances stay disabled until both are present, so a
    /// gesture can never synthesize a decision without its owner.
    ///
    /// Block identities submit as the approval/question identity: the scene
    /// projection packet must carry engine interaction ids into these block
    /// ids ([`SceneId`] already converts domain identities losslessly). Until
    /// then the controller sets context only for engine-backed surfaces.
    answer_thread: Option<ThreadId>,
    answer_run: Option<RunId>,
    /// Per-row approval submit gates keyed by block identity text.
    ///
    /// Each gate mirrors [`AnswerFlight`]: at most one answer attempt is in
    /// flight per row, so a second gesture cannot mint a second request
    /// identity while the first awaits its receipt.
    approval_gates: HashMap<String, ApprovalAnswerGate>,
    /// Per-row question submit gates keyed by block identity text.
    question_gates: HashMap<String, QuestionAnswerGate>,
    /// Offered question options per row, supplied by the controller from the
    /// engine observation rows.
    ///
    /// A row without cached options renders free-form entry, mirroring
    /// [`crate::engine_approve_ui::question_answer_view`]: a provider that
    /// enumerated its answers asks for a choice, otherwise prose is asked.
    question_choices: HashMap<String, QuestionChoiceCache>,
    /// Built answer commands awaiting controller drain to transport.
    ///
    /// Button gestures mint a fresh request identity and build the domain
    /// command through the existing [`approval_command`]/[`question_command`]
    /// constructors; the controller drains this outbox toward the existing
    /// transport/request path. GPUI action values travel alongside each
    /// command inside [`AnswerDispatch`].
    pending_answer_dispatches: Vec<AnswerDispatch>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ViewportGeometry {
    scroll_top: f64,
    viewport_height: f64,
    scroll_height: f64,
}

struct RenderedScrollAnchor {
    scene_id: Option<SceneId>,
    item_id: Option<ItemId>,
    anchor: ScrollAnchor,
    painted: bool,
}

impl RenderedScrollAnchor {
    fn matches(&self, target: &ConversationSurfaceTarget) -> bool {
        match target {
            ConversationSurfaceTarget::Scene(scene_id) => self.scene_id.as_ref() == Some(scene_id),
            ConversationSurfaceTarget::Item(item_id) => self.item_id.as_ref() == Some(item_id),
        }
    }

    fn has_identity(&self) -> bool {
        self.scene_id.is_some() || self.item_id.is_some()
    }

    fn same_identity(&self, scene_id: Option<&SceneId>, item_id: Option<&ItemId>) -> bool {
        self.has_identity()
            && self.scene_id.as_ref() == scene_id
            && self.item_id.as_ref() == item_id
    }
}

const SCROLL_ANCHOR_ELEMENT_PREFIX: &str = "artisan-conversation-scroll-anchor";

struct ScrollAnchorRegistry<'a> {
    handle: &'a ScrollHandle,
    previous: &'a [RenderedScrollAnchor],
    next_element_id: usize,
    rendered: &'a mut Vec<RenderedScrollAnchor>,
}

impl ScrollAnchorRegistry<'_> {
    fn attach(
        &mut self,
        element: Div,
        scene_id: Option<&SceneId>,
        item_id: Option<&ItemId>,
    ) -> Stateful<Div> {
        let scene_id = scene_id.cloned();
        let item_id = item_id.cloned();
        let previous = self
            .previous
            .iter()
            .find(|rendered| rendered.same_identity(scene_id.as_ref(), item_id.as_ref()));
        let (anchor, painted) = previous.map_or_else(
            || (ScrollAnchor::for_handle(self.handle.clone()), false),
            |rendered| (rendered.anchor.clone(), rendered.painted),
        );
        self.rendered.push(RenderedScrollAnchor {
            scene_id: scene_id.clone(),
            item_id: item_id.clone(),
            anchor: anchor.clone(),
            painted,
        });
        let element_id = self.next_element_id;
        self.next_element_id = self
            .next_element_id
            .checked_add(1)
            .expect("conversation scroll anchor element id space exhausted");
        let internal_id = ElementId::named_usize(SCROLL_ANCHOR_ELEMENT_PREFIX, element_id);
        element.id(internal_id).anchor_scroll(Some(anchor))
    }

    fn anchor_for_item(&self, item_id: &ItemId) -> Option<(ScrollAnchor, bool)> {
        self.rendered
            .iter()
            .find(|rendered| rendered.item_id.as_ref() == Some(item_id))
            .map(|rendered| (rendered.anchor.clone(), rendered.painted))
    }

    fn register_item_alias(&mut self, item_id: ItemId, anchor: ScrollAnchor, painted: bool) {
        self.rendered.push(RenderedScrollAnchor {
            scene_id: None,
            item_id: Some(item_id),
            anchor,
            painted,
        });
    }
}

/// The collapsed disclosure panel's inner blur, mirroring `--blur-small`.
const WORK_GROUP_PANEL_BLUR_PX: f32 = 2.0;

/// One work-session disclosure transition: direction plus start fraction.
///
/// The fraction is the expanded share of the panel's measured content height
/// in `0.0..=1.0`. A flight always moves toward [`Self::target`] on the
/// reference 250 ms `cubic-bezier(0.22, 1, 0.36, 1)` clock.
#[derive(Clone, Copy, Debug, PartialEq)]
struct DisclosureFlight {
    /// The open value the flight is moving to.
    open: bool,
    /// The expanded fraction the flight starts from.
    ///
    /// Captured from the last painted frame so an interrupted flight reverses
    /// from the displayed height instead of snapping to an endpoint.
    from: f32,
}

impl DisclosureFlight {
    /// Returns the expanded fraction this flight settles on.
    const fn target(self) -> f32 {
        if self.open { 1.0 } else { 0.0 }
    }
}

/// Window-local disclosure panel state for one work group.
///
/// The reference tweens `grid-template-rows` from zero to the content height;
/// GPUI has no intrinsic track to tween, so the panel's measured content height
/// is the flight target and the panel is clipped to the animated height. A
/// mount rests at its final state exactly like a browser mounting `data-open`
/// without a transition; only an actual open-value change arms a flight, and
/// reduced motion records the change without arming one.
struct DisclosurePanelState {
    /// Intrinsic height of the mounted detail rows in px.
    ///
    /// Updated from prepaint child bounds while content is mounted, so the
    /// last measurement survives a collapsed panel that unmounts its rows.
    content_height: f32,
    /// Open value of the last rendered frame; the flip detector.
    painted_open: bool,
    /// The in-flight transition, if any.
    flight: Option<DisclosureFlight>,
    /// Last painted expanded fraction in `0.0..=1.0`.
    ///
    /// The height animator writes every painted frame, so the next flight can
    /// start from the displayed fraction.
    progress: Rc<Cell<f32>>,
}

impl DisclosurePanelState {
    /// Creates the mount state for one group; no flight is armed at mount.
    fn new(open: bool) -> Self {
        Self {
            content_height: 0.0,
            painted_open: open,
            flight: None,
            progress: Rc::new(Cell::new(if open { 1.0 } else { 0.0 })),
        }
    }
}

/// Builds one disclosure flight clock: 250 ms
/// `cubic-bezier(0.22, 1, 0.36, 1)`, sampled by [`MotionCurve::SmoothOut`].
///
/// The curve is sampled inside the animator rather than installed as GPUI
/// easing, mirroring the other local clocks in this surface. The panel and its
/// inner share the clock shape, so height, opacity, and blur all ride the
/// reference's single 250 ms ease.
fn disclosure_flight_animation(plan: MotionPlan) -> Animation {
    let MotionPlan::Animate(animation) = plan else {
        unreachable!("a disclosure flight only runs under full motion");
    };
    let curve = animation.curve();
    Animation::new(animation.duration())
        .with_easing(move |progress: f32| curve.sample(f64::from(progress)) as f32)
}

/// Attaches the shared accordion panel (height, opacity, blur) to one
/// disclosure root.
///
/// `selector` is the owning element's selector; the panel state, content, and
/// animator identities derive from it exactly as the work-session disclosure
/// always has, so the session panel and every activity chain ride one clock
/// implementation. `controlled` follows the existing work-group policy:
/// uncontrolled roots paint their content statically, while controlled roots
/// keep content mounted only while it is visible or a collapse is still
/// measuring. The caller owns the root and panel debug selectors and the
/// trigger child; this helper owns the content selector and flight.
#[allow(clippy::too_many_arguments)]
fn disclosure_flight_panel(
    mut disclosure: Div,
    mut panel: Div,
    content: AnyElement,
    selector: &str,
    open: bool,
    controlled: bool,
    motion: MotionPolicy,
    window: &mut Window,
    cx: &mut Context<ConversationSurface>,
) -> AnyElement {
    let inner = div()
        .w_full()
        .min_w_0()
        .flex_shrink_0()
        .debug_selector({
            let content_selector = format!("{selector}-disclosure-content");
            move || content_selector.clone()
        })
        .child(content);

    if controlled {
        let panel_state = window.use_keyed_state(
            ElementId::Name(SharedString::from(format!(
                "{selector}-disclosure-panel-state"
            ))),
            cx,
            |_, _| DisclosurePanelState::new(open),
        );
        let (painted_open, mut flight, progress, content_height) = {
            let state = panel_state.read(cx);
            (
                state.painted_open,
                state.flight,
                state.progress.clone(),
                state.content_height,
            )
        };
        if painted_open != open {
            let from = progress.get().clamp(0.0, 1.0);
            flight = match motion {
                MotionPolicy::Full => Some(DisclosureFlight { open, from }),
                MotionPolicy::Reduced => None,
            };
            panel_state.update(cx, |state, _| {
                state.painted_open = open;
                state.flight = flight;
            });
        }
        let animated =
            matches!(flight, Some(armed) if armed.open == open) && motion == MotionPolicy::Full;
        if !animated && flight.is_some() {
            // A policy switch midway through a flight (or a reduced-motion
            // render) settles at the final state now; the clock is dropped
            // so it can never replay when full motion returns.
            flight = None;
            panel_state.update(cx, |state, _| state.flight = None);
        }
        let content_mounted = open || matches!(flight, Some(armed) if !armed.open);

        // Height measurement rides the panel's prepaint boundary, after
        // this frame's animator has written its fraction: a completed
        // flight disarms here, which unmounts a settled collapse's rows on
        // the next frame without ever dropping the stored target height.
        let panel_state_for_prepaint = panel_state.clone();
        panel = panel.on_children_prepainted(move |children_bounds, _, app| {
            let Some(content_bounds) = children_bounds.first() else {
                return;
            };
            panel_state_for_prepaint.update(app, |state, state_cx| {
                state.content_height = f32::from(content_bounds.size.height);
                if state
                    .flight
                    .is_some_and(|flight| (state.progress.get() - flight.target()).abs() <= 1e-4)
                {
                    state.flight = None;
                    state_cx.notify();
                }
            });
        });

        if content_mounted {
            if animated {
                let armed = flight.expect("the animated branch always carries a flight");
                let (from, target) = (armed.from, armed.target());
                let full_height = content_height;
                let progress_for_height = progress.clone();
                let inner_animation =
                    disclosure_flight_animation(motion.resolve(MotionRecipe::AccordionExpand));
                let inner = inner
                    .with_animations(
                        ElementId::Name(SharedString::from(format!(
                            "{selector}-disclosure-content-{}",
                            if open { "open" } else { "closed" }
                        ))),
                        vec![inner_animation],
                        move |inner, _index, value| {
                            let fraction = from + (target - from) * value;
                            if fraction >= 1.0 {
                                inner.opacity(1.0)
                            } else {
                                inner.opacity(fraction).filter(vec![Filter::Blur(px(
                                    WORK_GROUP_PANEL_BLUR_PX * (1.0 - fraction),
                                ))])
                            }
                        },
                    )
                    .into_any_element();
                let panel_animation =
                    disclosure_flight_animation(motion.resolve(MotionRecipe::AccordionExpand));
                disclosure = disclosure.child(
                    panel
                        .child(inner)
                        .overflow_hidden()
                        .with_animations(
                            ElementId::Name(SharedString::from(format!(
                                "{selector}-disclosure-panel-{}",
                                if open { "open" } else { "closed" }
                            ))),
                            vec![panel_animation],
                            move |panel, _index, value| {
                                let fraction = from + (target - from) * value;
                                progress_for_height.set(fraction);
                                panel.h(px(full_height * fraction))
                            },
                        )
                        .into_any_element(),
                );
            } else {
                // A settled flight disarms above, so mounted content
                // outside a flight is always the open side.
                debug_assert!(open, "mounted content outside a flight is open");
                progress.set(1.0);
                disclosure = disclosure.child(panel.child(inner));
            }
        } else {
            progress.set(if open { 1.0 } else { 0.0 });
            disclosure = disclosure.child(panel.h(px(0.0)).overflow_hidden());
        }
    } else if open {
        disclosure = disclosure.child(panel.child(inner));
    } else {
        disclosure = disclosure.child(panel.h(px(0.0)).overflow_hidden());
    }

    disclosure.into_any_element()
}

/// One loaded user-message control in the turn navigator rail.
struct NavigatorMarker {
    /// Visible policy label; never crosses the action boundary.
    label: String,
    /// Exact scroll target; identity only, never body text.
    target: ConversationSurfaceTarget,
}

/// Window-local rail geometry for the turn navigator.
///
/// The rail centers vertically in the card from live measurement: both
/// windows sharing one surface converge independently, exactly like the
/// transcript end-space height.
#[derive(Clone, Copy, Debug, Default)]
struct TurnNavigatorMetrics {
    /// Rail top offset in px within the surface root.
    top_px: f32,
    /// Viewport height in px at the last measurement.
    viewport_px: f32,
}

/// Samples the reference dropdown easing for navigator motion clocks.
///
/// `cubic-bezier(0.22, 1, 0.36, 1)` (`theme.css:140`), shared with the model
/// picker hover flights rather than re-solved per surface.
fn navigator_smooth_out(progress: f32) -> f32 {
    MotionCurve::SmoothOut.sample(f64::from(progress)) as f32
}

/// One recognised shell wrapper: how its executable is named and how it takes
/// a command. Ported from the reference `shell-command.ts`.
struct ShellWrapper {
    flags: &'static [&'static str],
    names: &'static [&'static str],
}

/// The wrappers a reader means, in the reference's exact order.
const SHELL_WRAPPERS: &[ShellWrapper] = &[
    ShellWrapper {
        flags: &["-command", "-c"],
        names: &["pwsh", "powershell"],
    },
    ShellWrapper {
        flags: &["-c", "-lc", "-ic", "-lic"],
        names: &["bash", "sh", "zsh", "dash"],
    },
    ShellWrapper {
        flags: &["/c", "/k"],
        names: &["cmd"],
    },
];

/// Splits one leading argument, honouring the quotes a path with spaces needs.
///
/// Returns `(rest, value)` exactly like the reference: the rest keeps its
/// leading whitespace (the next split trims it) and an unterminated quote
/// falls back to the whitespace split.
fn take_shell_argument(input: &str) -> (&str, &str) {
    let text = input.trim_start();
    let quote = if text.starts_with('"') {
        Some('"')
    } else if text.starts_with('\'') {
        Some('\'')
    } else {
        None
    };

    if let Some(quote) = quote
        && let Some(offset) = text[1..].find(quote)
    {
        let close = 1 + offset;
        return (&text[close + 1..], &text[1..close]);
    }

    match text.find(char::is_whitespace) {
        Some(break_at) => (&text[break_at..], &text[..break_at]),
        None => ("", text),
    }
}

/// The executable's own name, without its directory or extension.
fn shell_executable_name(path: &str) -> String {
    let file = match path.rfind(|character| character == '/' || character == '\\') {
        Some(separator) => &path[separator + 1..],
        None => path,
    };
    let lowered = file.to_lowercase();
    lowered.strip_suffix(".exe").unwrap_or(&lowered).to_owned()
}

/// Drops one matching pair of surrounding quotes, which a shell would have
/// eaten.
fn unquote_shell_argument(text: &str) -> &str {
    let first = text.chars().next();
    match first {
        Some(first @ ('"' | '\'')) if text.len() > 1 && text.ends_with(first) => {
            &text[1..text.len() - 1]
        }
        _ => text,
    }
}

/// Collapses every whitespace run to one space and trims, exactly like the
/// reference `replaceAll(/\s+/g, " ").trim()`.
fn collapse_whitespace(text: &str) -> String {
    let mut collapsed = String::with_capacity(text.len());
    let mut pending_space = false;
    for character in text.chars() {
        if character.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space && !collapsed.is_empty() {
            collapsed.push(' ');
        }
        pending_space = false;
        collapsed.push(character);
    }
    collapsed
}

/// The command a reader means, recovered from the invocation an engine
/// reports. Ported from the reference `PresentShellCommand`.
///
/// A run arrives as the full argv it was spawned with, which on Windows
/// begins with an absolute path into `WindowsApps` before `-Command` and the
/// actual work. In a truncated row every visible character would be that
/// path, so the line would say less than the word it replaced. Anything not
/// recognised is returned as it came, because a command this does not
/// understand is still more useful whole than guessed at.
#[must_use]
pub fn present_shell_command(command: &str) -> String {
    let collapsed = collapse_whitespace(command);
    let (rest, executable) = take_shell_argument(&collapsed);
    let name = shell_executable_name(executable);
    let Some(wrapper) = SHELL_WRAPPERS
        .iter()
        .find(|candidate| candidate.names.contains(&name.as_str()))
    else {
        return collapsed;
    };

    let (body, flag) = take_shell_argument(rest);
    if !wrapper.flags.contains(&flag.to_lowercase().as_str()) {
        return collapsed;
    }

    let body = unquote_shell_argument(body.trim());

    if body.is_empty() {
        collapsed
    } else {
        body.to_owned()
    }
}

/// Whether one lifecycle is a failure the trace must explain.
const fn lifecycle_is_failed(lifecycle: ConversationLifecycle) -> bool {
    matches!(
        lifecycle,
        ConversationLifecycle::Failed
            | ConversationLifecycle::Cancelled
            | ConversationLifecycle::Interrupted
    )
}

/// Whether one lifecycle is live work; the set the scene build treats as
/// potentially still arriving.
const fn lifecycle_is_live(lifecycle: ConversationLifecycle) -> bool {
    matches!(
        lifecycle,
        ConversationLifecycle::Pending
            | ConversationLifecycle::Streaming
            | ConversationLifecycle::Active
            | ConversationLifecycle::Waiting
    )
}

/// The muted row text for one activity: the normalized shell command for
/// terminal details, the raw detail otherwise, else the presentation label.
///
/// A terminal detail whose normalization collapses to nothing falls back to
/// the label, so the row never paints an empty second column.
#[must_use]
fn activity_detail_text(
    kind: &str,
    detail: Option<&str>,
    lifecycle: Option<ConversationLifecycle>,
) -> String {
    match detail {
        Some(detail) if kind == "terminal_activity" => {
            let presented = present_shell_command(detail);
            if presented.is_empty() {
                activity_presentation_label(kind, lifecycle).unwrap_or_default()
            } else {
                presented
            }
        }
        Some(detail) => detail.to_owned(),
        None => activity_presentation_label(kind, lifecycle).unwrap_or_default(),
    }
}

/// Builds one chain header's clause sentence from its activity kinds.
///
/// Composition is counted in first-appearance order so the header describes
/// the work rather than measuring it: "Ran 1 command, read 2 files". The
/// first clause is capitalized, later ones join with a comma, exactly like
/// the reference `GroupClauses`.
#[must_use]
fn activity_chain_clause<'a>(kinds: impl Iterator<Item = &'a str>) -> String {
    let mut composition: Vec<(ActivityCategory, usize)> = Vec::new();
    for kind in kinds {
        let category = activity_category(kind);
        match composition.iter_mut().find(|(known, _)| *known == category) {
            Some((_, count)) => *count += 1,
            None => composition.push((category, 1)),
        }
    }

    let mut clause_text = String::new();
    for (index, (category, count)) in composition.iter().enumerate() {
        let clause = category.count_label(*count);
        if index == 0 {
            let mut characters = clause.chars();
            if let Some(first) = characters.next() {
                clause_text.extend(first.to_uppercase());
                clause_text.push_str(characters.as_str());
            }
        } else {
            clause_text.push_str(", ");
            clause_text.push_str(&clause);
        }
    }
    clause_text
}

/// The Tabler glyph naming one activity category, matching the reference
/// `CategoryIcon`.
const fn activity_category_icon(category: ActivityCategory) -> AssetId {
    match category {
        ActivityCategory::Command | ActivityCategory::Test | ActivityCategory::Typecheck => {
            AssetId::TABLER_TERMINAL_2
        }
        ActivityCategory::FileRead => AssetId::TABLER_FILE_TEXT,
        ActivityCategory::FileEdit => AssetId::TABLER_FILE_PENCIL,
        ActivityCategory::FileDelete => AssetId::TABLER_FILE_X,
        ActivityCategory::FileSearch => AssetId::TABLER_FILE_SEARCH,
        ActivityCategory::WebSearch => AssetId::TABLER_WORLD_SEARCH,
        _ => AssetId::TABLER_TOOL,
    }
}

/// The chain header's glyph: a homogeneous chain is represented by its tool
/// category, while mixed work uses the group glyph because no single category
/// can honestly name its contents (reference `GroupIcon`).
#[must_use]
fn activity_chain_icon<'a>(kinds: impl Iterator<Item = &'a str>) -> AssetId {
    let mut categories: Vec<ActivityCategory> = Vec::new();
    for kind in kinds {
        let category = activity_category(kind);
        if !categories.contains(&category) {
            categories.push(category);
        }
    }
    match categories.as_slice() {
        [only] => activity_category_icon(*only),
        _ => AssetId::TABLER_LIST_DETAILS,
    }
}

/// Resolves one activity chain's default disclosure state.
///
/// The reference defaults an activity group closed unless the chain failed or
/// is live. Native signals are the member lifecycles plus the owning turn: a
/// group that owns the turn's live Thinking/Working line counts as live while
/// the turn is not terminal (the closest native analogue of the reference
/// `work_active` flag), and a failed, cancelled, or interrupted turn opens
/// its chain so the failure can explain itself. Unknown liveness never counts
/// as live.
#[must_use]
fn activity_chain_disclosure_state(
    rows: &[(u64, DetailRow<'_>)],
    live_header: bool,
    turn_lifecycle: ConversationLifecycle,
) -> (bool, bool) {
    let mut live = live_header && !turn_lifecycle.is_terminal();
    let mut failed = lifecycle_is_failed(turn_lifecycle);
    for (_, row) in rows {
        if let DetailRow::Activity {
            lifecycle: Some(lifecycle),
            ..
        } = row
        {
            live |= lifecycle_is_live(*lifecycle);
            failed |= lifecycle_is_failed(*lifecycle);
        }
    }
    (live, failed)
}

/// Returns whether one work group paints actual visible trace content.
///
/// This is the native `has_visible_details` trace predicate
/// (`work_session_disclosure`): session-title markers and empty bodies are
/// signals, not content, so a `Thought for …` group with no rows paints no
/// disclosure control at all instead of a blank collapsible. A kinded
/// activity counts as content even with an empty body, because its category
/// label and fallback presentation always paint.
fn work_group_has_visible_details(block: &WorkGroupBlock) -> bool {
    ordered_detail_rows(block).iter().any(|(_, row)| match row {
        DetailRow::Assistant { body, .. } => !body.trim().is_empty(),
        DetailRow::Activity {
            body, kind, detail, ..
        } => {
            kind.is_some()
                || !body.trim().is_empty()
                || detail.is_some_and(|detail| !detail.trim().is_empty())
        }
        DetailRow::Compaction { summary, .. } => !summary.trim().is_empty(),
        DetailRow::NativeFact { text, .. } => !text.trim().is_empty(),
        DetailRow::SessionTitle { .. } => false,
    })
}

/// Returns the stable focus-map key for one navigator target
/// (`item:<id>` or `scene:<id>`).
fn navigator_focus_key(target: &ConversationSurfaceTarget) -> String {
    match target {
        ConversationSurfaceTarget::Item(id) => format!("item:{}", id.as_str()),
        ConversationSurfaceTarget::Scene(id) => format!("scene:{}", id.as_str()),
    }
}

/// Returns the raw scene or item identity carried by a navigator target.
fn navigator_target_slug(target: &ConversationSurfaceTarget) -> &str {
    match target {
        ConversationSurfaceTarget::Item(id) => id.as_str(),
        ConversationSurfaceTarget::Scene(id) => id.as_str(),
    }
}

/// Derives the loaded-turn navigator markers from the current scene only.
///
/// The existing `conversation_turn_markers` policy supplies ordering,
/// labels, and the two-marker minimum. Durable user-message identities
/// become exact `Item` targets; anything else that survives the policy
/// keeps its exact render-only `Scene` identity. Window markers are never
/// supplied: this surface renders only loaded turns.
#[must_use]
fn loaded_turn_navigator_markers(scene: &ConversationScene) -> Vec<NavigatorMarker> {
    let mut turns = Vec::new();
    let mut items = Vec::new();
    let mut ordinal: u64 = 0;
    for turn_scene in scene.turn_scenes() {
        turns.push(ConversationTurnInput::new(
            turn_scene.turn_id.as_str(),
            turn_scene.ordinal,
        ));
        for block in turn_scene.blocks() {
            if let TurnBlock::UserMessage(message) = block {
                items.push(LoadedConversationItemInput::user_message(
                    message.id.as_str(),
                    turn_scene.turn_id.as_str(),
                    ordinal,
                    message.body.clone(),
                ));
                ordinal = ordinal.saturating_add(1);
            }
        }
    }
    let snapshot = ConversationSnapshotInput::new(turns, items, None);
    conversation_turn_markers(&snapshot)
        .into_iter()
        .filter_map(|marker| {
            let target = ItemId::parse(marker.id.as_str())
                .ok()
                .map(ConversationSurfaceTarget::Item)
                .or_else(|| {
                    SceneId::parse(marker.id.as_str())
                        .ok()
                        .map(ConversationSurfaceTarget::Scene)
                })?;
            Some(NavigatorMarker {
                label: marker.label,
                target,
            })
        })
        .collect()
}

fn item_id_for_scene_id(id: &SceneId) -> Option<ItemId> {
    ItemId::parse(id.as_str()).ok()
}

/// Returns the block identities of every question row in the scene, in
/// render order, for per-row input focus retention.
fn question_block_ids(scene: &ConversationScene) -> Vec<String> {
    let mut ids = Vec::new();
    for turn_scene in scene.turn_scenes() {
        for block in turn_scene.blocks() {
            if let TurnBlock::Question(question) = block {
                ids.push(question.id.as_str().to_owned());
            }
        }
    }
    ids
}

/// Returns the scene identity that owns the transcript position of one work
/// group card. This is the single source for the group-card anchor identity
/// shared by rendering and scroll-target resolution.
fn work_group_anchor_id(turn_id: &TurnId, block: &WorkGroupBlock) -> Option<SceneId> {
    block
        .items
        .first()
        .map(work_item_id)
        .cloned()
        .or_else(|| SceneId::parse(turn_id.as_str()).ok())
}

fn text_block_scroll_identity(id: &SceneId) -> (Option<SceneId>, Option<ItemId>) {
    (Some(id.clone()), item_id_for_scene_id(id))
}

/// Returns the anchor identity pair painted for one transcript child.
///
/// The pair mirrors the exact arguments passed to
/// [`ScrollAnchorRegistry::attach`] for that child, so prepaint listeners can
/// resolve executed scroll targets against measured child bounds. Unanchored
/// rows report no identity and never match.
fn block_scroll_identity(turn_id: &TurnId, block: &TurnBlock) -> (Option<SceneId>, Option<ItemId>) {
    match block {
        TurnBlock::UserMessage(block) => text_block_scroll_identity(&block.id),
        TurnBlock::AssistantMessage(block) => text_block_scroll_identity(&block.id),
        TurnBlock::WorkGroup(block) => (
            block
                .session
                .clone()
                .or_else(|| work_group_anchor_id(turn_id, block)),
            None,
        ),
        TurnBlock::Compaction(block) => text_block_scroll_identity(&block.id),
        TurnBlock::ChangeSet(block) => text_block_scroll_identity(&block.id),
        TurnBlock::Plan(block) => text_block_scroll_identity(&block.id),
        TurnBlock::Approval(block) => text_block_scroll_identity(&block.id),
        TurnBlock::Question(block) => text_block_scroll_identity(&block.id),
        TurnBlock::Error(block) => text_block_scroll_identity(&block.id),
        TurnBlock::UsageInterruption(block) => text_block_scroll_identity(&block.id),
        TurnBlock::ModelTransition(block) => text_block_scroll_identity(&block.id),
        TurnBlock::NativeFact(block) => text_block_scroll_identity(&block.id),
        TurnBlock::SteeringLabel(block) => (Some(block.id.clone()), None),
        TurnBlock::TurnStatus(_) | TurnBlock::TurnFooter(_) => (None, None),
    }
}

/// Returns whether a queued scroll target addresses one measured identity.
///
/// This mirrors [`RenderedScrollAnchor::matches`] for the transient
/// render-to-prepaint handoff, which carries plain identity pairs instead of
/// anchor objects.
fn scroll_target_matches_identity(
    target: &ConversationSurfaceTarget,
    identity: &(Option<SceneId>, Option<ItemId>),
) -> bool {
    match target {
        ConversationSurfaceTarget::Scene(scene_id) => identity.0.as_ref() == Some(scene_id),
        ConversationSurfaceTarget::Item(item_id) => identity.1.as_ref() == Some(item_id),
    }
}

struct TextBlockRender<'a> {
    id: &'a SceneId,
    disclosure: Option<SceneDisclosure>,
    selector: String,
    title: &'static str,
    body: &'a str,
}

struct ControlledCardOptions {
    id: SceneId,
    item_id: Option<ItemId>,
    disclosure: Option<SceneDisclosure>,
    selector: String,
    style: CardStyle,
}

/// Render-scoped rich-link title probe.
///
/// Reads the surface's retained titles and records every unresolved HTTP(S)
/// destination for the render-tail flush. It never fetches and never blocks;
/// the authored label renders whenever [`Self::resolved_title`] is `None`.
struct SurfaceRichLinkTitles<'a> {
    titles: &'a RichLinkTitleTable,
    missing: &'a RefCell<Vec<String>>,
    now_ms: i64,
}

impl RichLinkTitleSource for SurfaceRichLinkTitles<'_> {
    fn resolved_title(&self, destination: &str) -> Option<SharedString> {
        if let Some(title) = self.titles.lookup(destination) {
            return Some(title);
        }
        if self
            .titles
            .request_candidate(destination, self.now_ms)
            .is_some()
        {
            self.missing.borrow_mut().push(destination.to_owned());
        }
        None
    }
}

impl ConversationSurface {
    /// Creates a surface with keyboard-focusable transcript and disclosure
    /// handles. The surface starts with the supplied scene and no actions.
    #[must_use]
    pub fn new(scene: ConversationScene, theme_mode: ThemeMode, cx: &mut Context<Self>) -> Self {
        let mut surface = Self {
            scene,
            message_images: None,
            message_images_observation: None,
            theme_mode,
            markdown_renderer: MarkdownRenderer::new(),
            rich_link_titles: RichLinkTitleTable::new(),
            rich_link_missing: RefCell::new(Vec::new()),
            scroll_handle: ScrollHandle::new(),
            transcript_focus: cx.focus_handle().tab_index(0).tab_stop(true),
            disclosure_focus: cx.focus_handle().tab_index(1).tab_stop(true),
            jump_to_latest_focus: cx.focus_handle().tab_index(2).tab_stop(true),
            answer_focus: cx.focus_handle().tab_index(3).tab_stop(true),
            jump_to_latest_visible: false,
            last_viewport_observation: Some(ViewportObservation {
                first_visible: None,
                last_visible: None,
                at_bottom: true,
            }),
            pending_viewport_observation: None,
            last_viewport_geometry: None,
            viewport_observation_scheduled: false,
            viewport_next_frame_scheduled: false,
            actions: Vec::new(),
            pending_scroll_targets: Vec::with_capacity(CONVERSATION_SURFACE_MAX_SCROLL_TARGETS),
            executed_scroll_targets: Vec::new(),
            scroll_anchors: Vec::new(),
            scroll_anchor_paint_token: None,
            navigator_focus: HashMap::new(),
            navigator_expanded: false,
            navigator_scroll: ScrollHandle::new(),
            navigator_hover: Rc::new(RefCell::new(SlidingHoverState::default())),
            navigator_hover_surface: Rc::new(RefCell::new(None)),
            navigator_focused_key: None,
            navigator_width_generation: 0,
            navigator_width_px: Rc::new(RefCell::new(40.0)),
            navigator_width_from: 40.0,
            transcript_scroll: PickerScrollState::default(),
            transcript_scroll_frame_scheduled: false,
            active_now_ms: None,
            status_motion: MotionPolicy::Full,
            trace_groups_open: RefCell::new(HashMap::new()),
            footer_mirrors: HashMap::new(),
            footer_focus: HashMap::new(),
            footer_revealed: None,
            question_focus: HashMap::new(),
            answer_thread: None,
            answer_run: None,
            approval_gates: HashMap::new(),
            question_gates: HashMap::new(),
            question_choices: HashMap::new(),
            pending_answer_dispatches: Vec::new(),
        };
        surface.sync_question_focus(cx);
        surface
    }

    /// Returns the currently accepted scene without cloning it.
    #[must_use]
    pub fn scene(&self) -> &ConversationScene {
        &self.scene
    }

    /// Returns the selected theme mode.
    #[must_use]
    pub const fn theme_mode(&self) -> ThemeMode {
        self.theme_mode
    }

    /// Returns the shared GPUI scroll handle owned by the surface.
    #[must_use]
    pub fn scroll_handle(&self) -> &ScrollHandle {
        &self.scroll_handle
    }

    /// Shares the application's bounded image cache with transcript cells.
    pub fn set_message_images(
        &mut self,
        images: Entity<crate::native_message_images::NativeMessageImages>,
        cx: &mut Context<Self>,
    ) {
        self.message_images_observation = Some(cx.observe(&images, |_, _, cx| cx.notify()));
        self.message_images = Some(images);
        cx.notify();
    }

    /// Returns the focus handle tracked by the transcript viewport.
    #[must_use]
    pub fn transcript_focus_handle(&self) -> &FocusHandle {
        &self.transcript_focus
    }

    /// Returns the focus handle used by controlled disclosure triggers.
    #[must_use]
    pub fn disclosure_focus_handle(&self) -> &FocusHandle {
        &self.disclosure_focus
    }

    /// Returns the focus handle retained for one navigator control, if its
    /// target is currently rendered.
    #[must_use]
    pub fn navigator_focus_handle(
        &self,
        target: &ConversationSurfaceTarget,
    ) -> Option<FocusHandle> {
        self.navigator_focus
            .get(&navigator_focus_key(target))
            .cloned()
    }

    /// Returns the focus handle retained for one free-form question row, if
    /// the row is currently rendered.
    #[must_use]
    pub fn question_focus_handle(&self, block_id: &str) -> Option<FocusHandle> {
        self.question_focus.get(block_id).cloned()
    }

    /// Rebuilds per-row question focus handles from the accepted scene.
    ///
    /// Generations survive for rows still present, so unrelated scene
    /// updates never steal focus; handles for removed rows drop with the
    /// rebuild, so drafts and focus never leak across questions.
    fn sync_question_focus(&mut self, cx: &mut Context<Self>) {
        let mut next = HashMap::with_capacity(self.question_focus.len());
        for block_id in question_block_ids(&self.scene) {
            let handle = self
                .question_focus
                .remove(&block_id)
                .unwrap_or_else(|| cx.focus_handle().tab_index(4).tab_stop(true));
            next.insert(block_id, handle);
        }
        self.question_focus = next;
    }

    /// Mirrors the controller's detached-reader affordance into rendering.
    ///
    /// The viewport controller remains the sole authority for whether the
    /// button should be visible; this flag is only a paint-time mirror.
    pub fn set_jump_to_latest_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.jump_to_latest_visible != visible {
            self.jump_to_latest_visible = visible;
            cx.notify();
        }
    }

    /// Requests the existing GPUI scroll handle to move to the transcript end.
    ///
    /// Completion is intentionally not synthesized here. The next physical
    /// viewport observation is the controller's completion signal.
    pub fn scroll_to_bottom(&mut self, cx: &mut Context<Self>) {
        self.scroll_handle.scroll_to_bottom();
        cx.notify();
    }

    /// Replaces the accepted scene. Disclosure state is not changed locally;
    /// the next replacement scene remains authoritative.
    pub fn replace_scene(&mut self, scene: ConversationScene, cx: &mut Context<Self>) {
        self.scene = scene;
        let live_keys: Vec<String> = self
            .scene
            .turn_scenes()
            .iter()
            .map(|turn| footer_key(&turn.turn_id))
            .collect();
        self.footer_mirrors
            .retain(|key, _| live_keys.iter().any(|live| live == key));
        self.footer_focus
            .retain(|key, _| live_keys.iter().any(|live| live == key));
        self.sync_question_focus(cx);
        cx.notify();
    }

    /// Mirrors one host clock sample for live Thinking/Working elapsed paint.
    ///
    /// The value is display input only; the authoritative elapsed basis stays
    /// scene-owned and is never reset here.
    pub fn set_active_now_ms(&mut self, now_ms: Option<i64>, cx: &mut Context<Self>) {
        if self.active_now_ms != now_ms {
            self.active_now_ms = now_ms;
            cx.notify();
        }
    }

    /// Returns the motion policy applied to the live status shimmer and the
    /// work-session disclosure flights.
    #[must_use]
    pub const fn status_motion(&self) -> MotionPolicy {
        self.status_motion
    }

    /// Mirrors an explicit reduced-motion preference for the live status
    /// shimmer and the work-session disclosure flights.
    ///
    /// `Full` (the default) follows the window's reduced-motion signal at
    /// render time; `Reduced` forces immediate static presentation regardless
    /// of it. Settled status rows never animate regardless of this preference.
    pub fn set_status_motion(&mut self, motion: MotionPolicy, cx: &mut Context<Self>) {
        if self.status_motion != motion {
            self.status_motion = motion;
            cx.notify();
        }
    }

    /// Mirrors the adapter-formatted relative age for one settled footer.
    pub fn set_footer_relative_age(
        &mut self,
        turn_id: &TurnId,
        relative_age: String,
        cx: &mut Context<Self>,
    ) {
        let mirror = self
            .footer_mirrors
            .entry(footer_key(turn_id))
            .or_default();
        if mirror.relative_age != relative_age {
            mirror.relative_age = relative_age;
            cx.notify();
        }
    }

    /// Mirrors the reader-facing copy status for one settled footer.
    ///
    /// An empty message clears a previous failure notice.
    pub fn set_footer_copy_message(
        &mut self,
        turn_id: &TurnId,
        message: String,
        cx: &mut Context<Self>,
    ) {
        let mirror = self
            .footer_mirrors
            .entry(footer_key(turn_id))
            .or_default();
        if mirror.copy_message != message {
            mirror.copy_message = message;
            cx.notify();
        }
    }

    /// Ensures per-turn footer focus handles and prunes handles whose turns
    /// left the scene, returning focus to the transcript when a focused
    /// control disappears.
    fn sync_footer_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for turn in self.scene.turn_scenes() {
            let key = footer_key(&turn.turn_id);
            self.footer_focus
                .entry(key)
                .or_insert_with(|| cx.focus_handle().tab_stop(true));
        }
        let live_keys: Vec<String> = self
            .scene
            .turn_scenes()
            .iter()
            .map(|turn| footer_key(&turn.turn_id))
            .collect();
        let stale_keys: Vec<String> = self
            .footer_focus
            .keys()
            .filter(|key| !live_keys.iter().any(|live| live == *key))
            .cloned()
            .collect();
        for key in stale_keys {
            if let Some(handle) = self.footer_focus.remove(&key)
                && handle.is_focused(window)
            {
                self.transcript_focus.focus(window, cx);
            }
        }
    }

    /// Sets the shared theme mode and repaints the surface when it changes.
    pub fn set_theme_mode(&mut self, theme_mode: ThemeMode, cx: &mut Context<Self>) {
        if self.theme_mode != theme_mode {
            self.theme_mode = theme_mode;
            cx.notify();
        }
    }

    /// Mirrors one resolved rich-link title and repaints when it is new.
    ///
    /// The URL is the backend's echoed canonical URL; the table re-applies
    /// the shared canonical policy before retaining it.
    pub fn set_rich_link_title(
        &mut self,
        requested_url: &str,
        page_name: &SharedString,
        expires_at_ms: i64,
        cx: &mut Context<Self>,
    ) {
        let before = self.rich_link_titles.lookup(requested_url);
        self.rich_link_titles
            .resolve(requested_url, page_name.clone(), expires_at_ms);
        if before.as_ref() != Some(page_name) {
            cx.notify();
        }
    }

    /// Records one failed rich-link resolution; the authored label stays.
    pub fn set_rich_link_failure(&mut self, requested_url: &str, cx: &mut Context<Self>) {
        self.rich_link_titles.fail(requested_url);
        cx.notify();
    }

    /// Returns one render-scoped probe over the surface title table.
    fn rich_link_probe(&self, now_ms: i64) -> SurfaceRichLinkTitles<'_> {
        SurfaceRichLinkTitles {
            titles: &self.rich_link_titles,
            missing: &self.rich_link_missing,
            now_ms,
        }
    }

    /// Queues every newly missing rich-link destination exactly once.
    ///
    /// Runs at the end of one render pass: the probe recorded misses while
    /// the renderer flattened links, and this drain marks them pending and
    /// hands the host one bounded resolver action. The extra notification is
    /// guarded by the table's pending state, so a second frame queues nothing
    /// and the loop ends.
    fn flush_rich_link_requests(&mut self, cx: &mut Context<Self>) {
        let missing = std::mem::take(&mut *self.rich_link_missing.borrow_mut());
        if missing.is_empty() {
            return;
        }
        let now_ms = crate::conversation_host::host_now_millis();
        let mut queued = Vec::new();
        for destination in missing {
            if let Some(url) = self.rich_link_titles.queue(&destination, now_ms) {
                queued.push(url);
            }
        }
        if queued.is_empty() {
            return;
        }
        if self.enqueue_action(ConversationSurfaceAction::ResolveRichLinks { urls: queued }) {
            cx.notify();
        }
    }

    /// Sets the explicit live answer context for engine approval/question rows.
    ///
    /// Both identities join each submit at dispatch; nothing ambient is read
    /// elsewhere. Submit affordances stay disabled until the controller
    /// supplies the live owning thread and run.
    pub fn set_answer_context(
        &mut self,
        thread_id: ThreadId,
        run_id: RunId,
        cx: &mut Context<Self>,
    ) {
        self.answer_thread = Some(thread_id);
        self.answer_run = Some(run_id);
        cx.notify();
    }

    /// Clears the live answer context, disabling submit affordances.
    pub fn clear_answer_context(&mut self, cx: &mut Context<Self>) {
        self.answer_thread = None;
        self.answer_run = None;
        cx.notify();
    }

    /// Mirrors one engine row's offered choices for choice rendering.
    ///
    /// Rows without cached options render free-form entry. Only labels (plus
    /// optional descriptions) are mirrored; the authoritative option views
    /// remain in [`EngineObservationState`].
    pub fn set_question_choices(
        &mut self,
        block_id: String,
        multi_select: bool,
        options: Vec<(String, Option<String>)>,
        cx: &mut Context<Self>,
    ) {
        self.question_choices.insert(
            block_id,
            QuestionChoiceCache {
                multi_select,
                options,
            },
        );
        cx.notify();
    }

    /// Stages one row's free-form draft exactly as supplied.
    ///
    /// GPUI text-entry binding for this draft lands with the controller input
    /// packet; until then the controller (and tests) stage drafts through
    /// this setter and the Answer control submits them.
    pub fn set_question_draft(&mut self, block_id: String, draft: String, cx: &mut Context<Self>) {
        self.question_gates
            .entry(block_id)
            .or_default()
            .set_draft(draft);
        cx.notify();
    }

    /// Returns built answer dispatches awaiting controller drain.
    #[must_use]
    pub fn pending_answer_dispatches(&self) -> &[AnswerDispatch] {
        &self.pending_answer_dispatches
    }

    /// Drains built answer dispatches in FIFO order.
    pub fn take_answer_dispatches(&mut self) -> Vec<AnswerDispatch> {
        std::mem::take(&mut self.pending_answer_dispatches)
    }

    /// Drains pending answer dispatches toward transport through the mirrored
    /// send path.
    ///
    /// Takes the outbox via [`Self::take_answer_dispatches`] (`mem::take`
    /// semantics already in the accessor: the queue is never re-taken or
    /// cloned) and hands each domain command with its already-minted request
    /// id to `send` at most once per call. `send` mirrors
    /// [`NativeTransportService::submit`](crate::native_transport_service::NativeTransportService::submit):
    /// it reports [`CommandSendError::Busy`] when its bounded queue is full
    /// and [`CommandSendError::Stopped`] after the service has stopped.
    ///
    /// Failed dispatches are re-queued in order with the existing retry
    /// message from [`pair_answer_failure`], so rows stay pending (their
    /// gates remain in flight until a receipt pairs through the existing
    /// settle-in-place pairing) with no silent drop and no same-call retry:
    /// one drain attempt per controller tick scope. Resolutions continue to
    /// pair through the existing receipt path, never through the outbox.
    ///
    /// `submit` is the transport submit entry point (live:
    /// [`NativeTransportService::submit`](crate::native_transport_service::NativeTransportService::submit)):
    /// each taken dispatch is mapped to its
    /// [`NativeTransportCommand`] through [`answer_transport_command`] with
    /// its already-minted request id and handed over by value exactly like
    /// the composer send path.
    pub fn drain_pending_answer_dispatches(
        &mut self,
        submit: &mut impl FnMut(NativeTransportCommand) -> Result<(), CommandSendError>,
    ) -> AnswerDrainReport {
        let queue = self.take_answer_dispatches();
        let (requeue, report) = drain_answer_queue(queue, submit);
        self.pending_answer_dispatches.extend(requeue);
        report
    }

    /// Attempts one approval gesture for a rendered row.
    ///
    /// Returns false when the row has no live context, no parsable approval
    /// identity, or an outstanding flight; in all three cases nothing is
    /// minted or dispatched. Every admitted attempt mints a fresh request
    /// identity.
    pub fn submit_approval_gesture(
        &mut self,
        block_key: &str,
        approval_id: &ObservationId,
        approved: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let (Some(thread_id), Some(run_id)) = (self.answer_thread.clone(), self.answer_run.clone())
        else {
            return false;
        };
        let admitted = self
            .approval_gates
            .entry(block_key.to_owned())
            .or_default()
            .begin(thread_id, run_id, approval_id.clone(), approved)
            .map(|attempt| AnswerDispatch {
                action: AnswerDispatchAction::Approval(attempt.action),
                command: attempt.command,
                request_id: attempt.request_id,
            });
        if let Some(dispatch) = admitted {
            self.pending_answer_dispatches.push(dispatch);
            cx.notify();
            true
        } else {
            false
        }
    }

    /// Attempts one staged question submission for a rendered row.
    ///
    /// Choice rows submit the staged selection (single-select choices submit
    /// through [`Self::submit_question_option_gesture`] at click time);
    /// free-form rows submit the staged draft. Empty submissions are rejected
    /// client-side with the row staying pending.
    pub fn submit_question_gesture(&mut self, block_key: &str, cx: &mut Context<Self>) -> bool {
        let (answer_context, is_choice) = (
            self.answer_thread.clone().zip(self.answer_run.clone()),
            self.question_choices
                .get(block_key)
                .is_some_and(|choices| choices.is_choice()),
        );
        let (Some((thread_id, run_id)), Some(question_id)) =
            (answer_context, ObservationId::parse(block_key).ok())
        else {
            return false;
        };
        let gate = self.question_gates.entry(block_key.to_owned()).or_default();
        let admitted = if is_choice {
            gate.submit_selected(thread_id, run_id, question_id)
        } else {
            gate.submit_freeform(thread_id, run_id, question_id)
        }
        .map(|attempt| AnswerDispatch {
            action: AnswerDispatchAction::Question(attempt.action),
            command: attempt.command,
            request_id: attempt.request_id,
        });
        if let Some(dispatch) = admitted {
            self.pending_answer_dispatches.push(dispatch);
            cx.notify();
            true
        } else {
            false
        }
    }

    /// Handles one keystroke for a focused free-form question row.
    ///
    /// Only pending free-form rows handle keys: choice rows keep their
    /// buttons, and rows with an outstanding flight ignore everything, so a
    /// second gesture can never mint a second identity. `enter` submits the
    /// staged draft through the existing [`Self::submit_question_gesture`]
    /// path (fresh request id, empty drafts rejected); `backspace` deletes
    /// one character; `escape` returns focus to the transcript without
    /// submitting; other unmodified single-character keys (plus `space`)
    /// append. Modified keys are ignored so shortcuts and focus navigation
    /// keep working.
    #[must_use]
    pub fn handle_question_key(
        &mut self,
        block_key: &str,
        key: &str,
        modifiers: &Modifiers,
        cx: &mut Context<Self>,
    ) -> QuestionKeyOutcome {
        let freeform = !self
            .question_choices
            .get(block_key)
            .is_some_and(QuestionChoiceCache::is_choice);
        let pending = !self
            .question_gates
            .get(block_key)
            .is_some_and(QuestionAnswerGate::is_in_flight);
        if !(freeform && pending) {
            return QuestionKeyOutcome::Ignored;
        }
        if key == "escape" {
            return QuestionKeyOutcome::FocusTranscript;
        }
        if key == "enter" && !modifiers.modified() {
            return if self.submit_question_gesture(block_key, cx) {
                QuestionKeyOutcome::Submitted
            } else {
                QuestionKeyOutcome::Ignored
            };
        }
        if key == "backspace" && !modifiers.modified() {
            let gate = self.question_gates.entry(block_key.to_owned()).or_default();
            if gate.delete_backward() {
                cx.notify();
                return QuestionKeyOutcome::Edited;
            }
            return QuestionKeyOutcome::Ignored;
        }
        // The spacebar reports its key name, not a blank character; every
        // other named key (tab, arrows, function keys) is ignored so focus
        // navigation keeps working.
        let text = if key == "space" { " " } else { key };
        let mut chars = text.chars();
        let printable = match chars.next() {
            Some(first) => chars.next().is_none() && !first.is_control(),
            None => false,
        };
        let plain = !modifiers.control && !modifiers.alt && !modifiers.platform;
        if plain && printable {
            let gate = self.question_gates.entry(block_key.to_owned()).or_default();
            if gate.insert_text(text) {
                cx.notify();
                return QuestionKeyOutcome::Edited;
            }
        }
        QuestionKeyOutcome::Ignored
    }

    /// Attempts one single-select option choice the moment it is clicked.
    ///
    /// Multi-select choices only stage through the gate; they confirm through
    /// [`Self::submit_question_gesture`].
    pub fn submit_question_option_gesture(
        &mut self,
        block_key: &str,
        option: String,
        cx: &mut Context<Self>,
    ) -> bool {
        let multi_select = self
            .question_choices
            .get(block_key)
            .is_some_and(|choices| choices.multi_select);
        if multi_select {
            self.question_gates
                .entry(block_key.to_owned())
                .or_default()
                .toggle_option(option, true);
            cx.notify();
            return true;
        }
        let (Some(thread_id), Some(run_id), Some(question_id)) = (
            self.answer_thread.clone(),
            self.answer_run.clone(),
            ObservationId::parse(block_key).ok(),
        ) else {
            return false;
        };
        let admitted = self
            .question_gates
            .entry(block_key.to_owned())
            .or_default()
            .submit_single(thread_id, run_id, question_id, option)
            .map(|attempt| AnswerDispatch {
                action: AnswerDispatchAction::Question(attempt.action),
                command: attempt.command,
                request_id: attempt.request_id,
            });
        if let Some(dispatch) = admitted {
            self.pending_answer_dispatches.push(dispatch);
            cx.notify();
            true
        } else {
            false
        }
    }

    /// Returns pending typed actions in FIFO order without draining them.
    #[must_use]
    pub fn pending_actions(&self) -> &[ConversationSurfaceAction] {
        &self.actions
    }

    /// Borrows the oldest pending action without removing it.
    ///
    /// The host uses this head peek to make downstream capacity decisions
    /// before acknowledging an action. That keeps a refused action at the
    /// surface boundary instead of losing a drained tail.
    #[must_use]
    pub fn next_action(&self) -> Option<&ConversationSurfaceAction> {
        self.actions.first()
    }

    /// Removes and returns exactly the oldest pending action.
    ///
    /// This one-action operation is intentionally separate from
    /// [`Self::take_actions`]. Host routing can therefore acknowledge actions
    /// one at a time after the controller or outer effect outbox accepts them.
    pub fn take_next_action(&mut self) -> Option<ConversationSurfaceAction> {
        if self.actions.is_empty() {
            None
        } else {
            Some(self.actions.remove(0))
        }
    }

    /// Drains pending actions in FIFO order.
    pub fn take_actions(&mut self) -> Vec<ConversationSurfaceAction> {
        std::mem::take(&mut self.actions)
    }

    /// Queues a viewport observation when the bounded outbox has capacity.
    pub fn observe_viewport(
        &mut self,
        observation: ViewportObservation,
        cx: &mut Context<Self>,
    ) -> bool {
        self.retry_pending_viewport_observation(cx);

        if self.last_viewport_observation.as_ref() == Some(&observation) {
            return false;
        }
        if self.pending_viewport_observation.as_ref() == Some(&observation) {
            return false;
        }
        if self.pending_viewport_observation.is_some() {
            return false;
        }

        if self.enqueue_action(ConversationSurfaceAction::ViewportObserved(
            observation.clone(),
        )) {
            self.last_viewport_observation = Some(observation);
            cx.notify();
            true
        } else {
            self.pending_viewport_observation = Some(observation);
            false
        }
    }

    /// Queues a scroll intent without mutating the GPUI handle or scene.
    pub fn request_scroll(
        &mut self,
        target: ConversationSurfaceTarget,
        cx: &mut Context<Self>,
    ) -> bool {
        self.retry_pending_viewport_observation(cx);
        if self.pending_viewport_observation.is_some() {
            return false;
        }
        let queued = self.enqueue_action(ConversationSurfaceAction::ScrollIntent { target });
        if queued {
            cx.notify();
        }
        queued
    }

    /// Queues one host-owned scroll target for the next render.
    ///
    /// The target queue is deliberately separate from surface actions: a
    /// host effect is acknowledged only when this bounded transient queue has
    /// room, and execution never dispatches a controller event.
    pub(crate) fn schedule_scroll_target(
        &mut self,
        target: ConversationSurfaceTarget,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.pending_scroll_targets.len() >= CONVERSATION_SURFACE_MAX_SCROLL_TARGETS {
            return false;
        }
        self.pending_scroll_targets.push(target);
        cx.notify();
        true
    }

    /// Handles one transcript wheel event with the model picker's smoothing.
    ///
    /// The content wrapper is the first bubble listener inside the scroll
    /// container, so stopping propagation here keeps the viewport's built-in
    /// immediate jump from applying on top. Precise trackpad deltas and
    /// reduced motion take the direct path; coarse wheel ticks accumulate
    /// into one bounded target and settle through interpolation frames.
    fn handle_transcript_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        let delta = f32::from(event.delta.pixel_delta(window.line_height()).y);
        if delta.abs() <= f32::EPSILON {
            return;
        }
        let offset = self.scroll_handle.offset();
        let current = f32::from(offset.y);
        let maximum = f32::from(self.scroll_handle.max_offset().y).max(0.0);
        if event.delta.precise() || cx.reduce_motion() {
            let next = (current + delta).clamp(-maximum, 0.0);
            self.scroll_handle.set_offset(point(offset.x, px(next)));
            self.transcript_scroll.cancel_to(next, maximum);
            cx.notify();
            return;
        }
        self.transcript_scroll.push(current, delta, maximum);
        self.schedule_transcript_scroll_frame(window, cx);
    }

    /// Schedules one transcript smoothing frame while a target is outstanding.
    fn schedule_transcript_scroll_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.transcript_scroll.active() || self.transcript_scroll_frame_scheduled {
            return;
        }
        self.transcript_scroll_frame_scheduled = true;
        cx.on_next_frame(window, |surface, window, cx| {
            surface.advance_transcript_scroll(window, cx);
        });
    }

    /// Applies one bounded smoothing step toward the wheel target.
    fn advance_transcript_scroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.transcript_scroll_frame_scheduled = false;
        let offset = self.scroll_handle.offset();
        let maximum = f32::from(self.scroll_handle.max_offset().y).max(0.0);
        if let Some(next) = self.transcript_scroll.step(f32::from(offset.y), maximum) {
            self.scroll_handle.set_offset(point(offset.x, px(next)));
            cx.notify();
        }
        self.schedule_transcript_scroll_frame(window, cx);
    }

    /// Selects the reader's current marker from painted turn geometry.
    ///
    /// Pure over its inputs: turn roots arrive in scene order with the
    /// spacer last. A turn counts as reached once its top passes the
    /// reference 96 px threshold below the viewport top; the last reached
    /// marker-bearing turn wins, else the first marker. Callers keep the
    /// result in window-local state with a change guard, so two windows
    /// sharing one surface converge independently and never ping-pong.
    ///
    /// Turn tops resolve to viewport coordinates through
    /// [`navigator_turn_top_viewport`]: content origin in window coordinates
    /// (`bounds.origin - element_offset`, the same quantity
    /// [`apply_painted_scroll_offset`](Self::apply_painted_scroll_offset)
    /// reproduces), re-based onto the viewport origin the host header
    /// offsets in the full app.
    fn navigator_active_for_geometry(
        scene: &ConversationScene,
        scroll_handle: &ScrollHandle,
        children_bounds: &[gpui::Bounds<gpui::Pixels>],
        window: &Window,
    ) -> Option<String> {
        let markers = loaded_turn_navigator_markers(scene);
        if markers.is_empty() {
            return None;
        }
        let scroll_top = -f64::from(scroll_handle.offset().y);
        let element_offset = f64::from(window.element_offset().y);
        let viewport_top = f64::from(scroll_handle.bounds().origin.y);
        let mut reached: Vec<(String, f64)> = Vec::new();
        for (index, turn) in scene.turn_scenes().iter().enumerate() {
            let Some(bounds) = children_bounds.get(index) else {
                continue;
            };
            let marker = turn.blocks().iter().find_map(|block| match block {
                TurnBlock::UserMessage(message) => {
                    item_id_for_scene_id(&message.id).map(|id| id.as_str().to_owned())
                }
                _ => None,
            });
            let Some(slug) = marker else { continue };
            if !markers
                .iter()
                .any(|marker| navigator_target_slug(&marker.target) == slug)
            {
                continue;
            }
            let content_top = f64::from(bounds.origin.y) - element_offset;
            reached.push((
                slug,
                Self::navigator_turn_top_viewport(content_top, scroll_top, viewport_top),
            ));
        }
        let offsets: Vec<ConversationTurnOffset<'_>> = reached
            .iter()
            .map(|(id, top)| ConversationTurnOffset::new(id.as_str(), *top))
            .collect();
        active_conversation_turn(&offsets).map(str::to_owned)
    }

    /// Rebases one content top onto the viewport origin.
    ///
    /// `content_top` is the turn origin in window coordinates, `scroll_top`
    /// the scrolled distance, and `viewport_top` the viewport origin the
    /// host header offsets in the full app (zero in standalone tests, which
    /// is why geometry tests must cover a nonzero origin explicitly).
    fn navigator_turn_top_viewport(content_top: f64, scroll_top: f64, viewport_top: f64) -> f64 {
        content_top - scroll_top - viewport_top
    }

    /// Measures end-space height from live prepaint geometry, if measurable.
    ///
    /// Pure over its inputs: `children_bounds` holds every transcript child
    /// in order with the spacer last, so all but the last entry are turns
    /// and the last entry is the spacer itself. The last turn anchors the
    /// formula; an empty transcript yields `None` and the caller keeps
    /// whatever its window state holds. Origins cancel the element offset
    /// by subtraction, exactly like the painted scroll-offset path, so
    /// window and content spaces agree.
    fn measured_end_space_height(
        children_bounds: &[gpui::Bounds<gpui::Pixels>],
        viewport_height: f64,
        element_offset_y: f64,
    ) -> Option<f32> {
        let (_, turns) = children_bounds.split_last()?;
        let last = turns.last()?;
        let item_top = f64::from(last.origin.y) - element_offset_y;
        let end_space_top = f64::from(
            children_bounds
                .last()
                .expect("split yielded a last child")
                .origin
                .y,
        ) - element_offset_y;
        let measured = end_space_height(viewport_height, item_top, end_space_top);
        if !measured.is_finite() {
            return None;
        }
        Some(measured as f32)
    }

    /// Releases render-only scroll custody when the owning host retires.
    ///
    /// The queue contains only typed targets. The registry and its deferred
    /// paint token are also transient, so neither can outlive this surface's
    /// host ownership or revive a replacement surface.
    pub(crate) fn release_transient_scroll_custody(&mut self) {
        self.pending_scroll_targets.clear();
        self.executed_scroll_targets.clear();
        self.scroll_anchors.clear();
        self.scroll_anchor_paint_token = None;
    }

    /// Writes the exact anchor-equivalent offset for one executed target.
    ///
    /// GPUI records each anchor origin during Div prepaint as
    /// `bounds.origin - window.element_offset()` (gpui-0.2.2
    /// `src/elements/div.rs:1368-1369`) and `ScrollAnchor::scroll_to`
    /// applies `viewport_bounds.origin - last_origin` from an
    /// `on_next_frame` callback (`div.rs:3029-3037`). Only the `ScrollArea`
    /// viewport pushes a nonzero element offset on this path
    /// (`src/window.rs:2410-2412`), so subtracting the listener-observed
    /// offset from the measured child origin reproduces that private origin
    /// exactly. The write lands inside the test-harness dirty draw
    /// (`src/app.rs:1247`), where `on_next_frame` callbacks never run
    /// (`src/platform/test/window.rs:235`).
    fn apply_painted_scroll_offset(
        &self,
        child_origin: gpui::Point<gpui::Pixels>,
        window: &Window,
    ) {
        let viewport_origin = self.scroll_handle.bounds().origin;
        let content_origin = child_origin - window.element_offset();
        let max_offset = self.scroll_handle.max_offset();
        let mut offset = viewport_origin - content_origin;
        offset.x = offset.x.clamp(-max_offset.x, px(0.0));
        offset.y = offset.y.clamp(-max_offset.y, px(0.0));
        self.scroll_handle.set_offset(offset);
    }

    /// Applies and retires the handoff against one listener's children.
    ///
    /// Targets apply in queue order so the last queued target wins, exactly
    /// like the equivalent chain of `ScrollAnchor::scroll_to` callbacks.
    /// Targets with no identity at this level stay queued for the remaining
    /// levels of the same frame; render clears any true orphans.
    fn apply_executed_scroll_targets(
        &mut self,
        identities: &[(Option<SceneId>, Option<ItemId>)],
        children_bounds: &[gpui::Bounds<gpui::Pixels>],
        window: &Window,
    ) {
        if self.executed_scroll_targets.is_empty() {
            return;
        }
        let stashed = std::mem::take(&mut self.executed_scroll_targets);
        for target in stashed {
            let Some((_, bounds)) = identities
                .iter()
                .zip(children_bounds.iter())
                .find(|(identity, _)| scroll_target_matches_identity(&target, identity))
            else {
                self.executed_scroll_targets.push(target);
                continue;
            };
            self.apply_painted_scroll_offset(bounds.origin, window);
        }
    }

    /// Executes queued scroll targets against freshly rendered anchors.
    ///
    /// Painted matches run the GPUI anchor scroll and join the prepaint
    /// handoff; unpainted matches stay queued behind the paint gate and
    /// unknown targets drop as no-ops. Returns whether any scroll executed.
    fn drain_painted_scroll_targets(
        &mut self,
        rendered_anchors: &[RenderedScrollAnchor],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let pending_targets = std::mem::take(&mut self.pending_scroll_targets);
        // The handoff is render-local: leftovers from a draw whose prepaint
        // never resolved them must not leak into a later frame.
        self.executed_scroll_targets.clear();
        let mut retained_targets = Vec::with_capacity(pending_targets.len());
        let mut waiting_for_paint = false;
        let mut scroll_executed = false;
        for target in pending_targets {
            if waiting_for_paint {
                retained_targets.push(target);
                continue;
            }

            match rendered_anchors
                .iter()
                .find(|rendered| rendered.matches(&target))
            {
                Some(rendered) if rendered.painted => {
                    rendered.anchor.scroll_to(window, cx);
                    self.executed_scroll_targets.push(target);
                    scroll_executed = true;
                }
                Some(_) => {
                    retained_targets.push(target);
                    waiting_for_paint = true;
                }
                None => {}
            }
        }
        self.pending_scroll_targets = retained_targets;
        scroll_executed
    }

    /// Returns the transcript-level identity pair for every turn root.
    ///
    /// Turn roots are the transcript's direct children in scene order.
    fn turn_scroll_identities(&self) -> Vec<(Option<SceneId>, Option<ItemId>)> {
        self.scene
            .turn_scenes()
            .iter()
            .map(|turn| (SceneId::parse(turn.turn_id.as_str()).ok(), None))
            .collect()
    }

    /// Retries one geometry observation retained when the action queue was
    /// full. This is `pub(crate)` so the host can retry after draining its
    /// downstream effect queue without inventing another surface action.
    pub(crate) fn retry_pending_viewport_observation(&mut self, cx: &mut Context<Self>) {
        let Some(observation) = self.pending_viewport_observation.take() else {
            return;
        };

        if self.last_viewport_observation.as_ref() == Some(&observation) {
            return;
        }
        if self.enqueue_action(ConversationSurfaceAction::ViewportObserved(
            observation.clone(),
        )) {
            self.last_viewport_observation = Some(observation);
            cx.notify();
        } else {
            self.pending_viewport_observation = Some(observation);
        }
    }

    fn enqueue_action(&mut self, action: ConversationSurfaceAction) -> bool {
        if self.actions.len() >= CONVERSATION_SURFACE_MAX_ACTIONS {
            return false;
        }
        self.actions.push(action);
        true
    }

    fn observe_current_viewport(&mut self, cx: &mut Context<Self>) {
        self.retry_pending_viewport_observation(cx);

        let offset = self.scroll_handle.offset();
        let bounds = self.scroll_handle.bounds();
        let max_offset = self.scroll_handle.max_offset();
        let scroll_top = -f64::from(offset.y);
        let viewport_height = f64::from(bounds.size.height);
        let scroll_height = viewport_height + f64::from(max_offset.y);
        let geometry = ViewportGeometry {
            scroll_top,
            viewport_height,
            scroll_height,
        };
        if self.last_viewport_geometry == Some(geometry) {
            return;
        }

        self.last_viewport_geometry = Some(geometry);
        let observation = ViewportObservation {
            first_visible: None,
            last_visible: None,
            at_bottom: conversation_is_following(scroll_top, scroll_height, viewport_height),
        };
        let _ = self.observe_viewport(observation, cx);
    }

    fn schedule_viewport_observation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.viewport_observation_scheduled {
            self.viewport_observation_scheduled = true;
            let entity = cx.entity().downgrade();
            // Read after GPUI has painted the current layout so the initial
            // geometry is observable even when an otherwise idle platform
            // does not deliver a later frame callback.
            window.defer(cx, move |_, app| {
                let _ = entity.update(app, |surface, cx| {
                    surface.viewport_observation_scheduled = false;
                    surface.observe_current_viewport(cx);
                });
            });
        }

        if self.viewport_next_frame_scheduled {
            return;
        }
        self.viewport_next_frame_scheduled = true;
        let entity = cx.entity().downgrade();
        window.on_next_frame(move |_, app| {
            let _ = entity.update(app, |surface, cx| {
                surface.viewport_next_frame_scheduled = false;
                surface.observe_current_viewport(cx);
            });
        });
    }

    fn render_turn(
        &self,
        turn: &TurnScene,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
        window: &mut Window,
        status_motion: MotionPolicy,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selector = turn_selector(&turn.turn_id);
        let turn_element = div()
            .w_full()
            .max_w(px(TRANSCRIPT_PROSE_WIDTH_PX))
            .mx_auto()
            .px(px(TRANSCRIPT_GUTTER_PX))
            .relative()
            .group(TURN_GROUP)
            .flex()
            .flex_col()
            // Reference intra-turn rhythm is `gap-[1lh]`: no app override in
            // lib/styles, so Tailwind preflight 1.5 on the 16 px base applies
            // and one line height is 24 px.
            .gap(theme.spacing.steps(6.0));
        // At most one group owns the live Thinking/Working line: the latest
        // group headers it once from the same accepted narration and clock,
        // and the separate status row below stands down unless it narrates
        // something distinct. Terminal labels always win over the live line
        // inside the group.
        let live_header: Option<String> = turn_owner_header(turn, self.active_now_ms);
        let live_owner = if live_header.is_some() {
            owning_group_index(turn)
        } else {
            None
        };
        // Identities mirror the children pushed below in block order. A
        // suppressed status row and an unsettled footer paint no child, so
        // neither contributes a slot.
        let turn_has_work_group = turn
            .blocks()
            .iter()
            .any(|block| matches!(block, TurnBlock::WorkGroup(_)));
        let child_identities: Vec<(Option<SceneId>, Option<ItemId>)> = turn
            .blocks()
            .iter()
            .filter_map(|block| {
                if let TurnBlock::TurnStatus(status) = block {
                    // Same paint decision as render_status: structural rule
                    // plus identical-duplicate suppression, so measured
                    // children and identities stay one-to-one.
                    let copy = turn_status_copy_text(
                        status.narration,
                        status.active_started_at_ms,
                        self.active_now_ms,
                        status.reasoning_summary.as_deref(),
                        status.engine_label.as_deref(),
                    );
                    let owner = if matches!(
                        status.narration,
                        TurnNarration::Thinking | TurnNarration::Working
                    ) {
                        turn_owner_header(turn, self.active_now_ms)
                    } else {
                        None
                    };
                    if !turn_status_paints(
                        turn_has_work_group,
                        status.narration,
                        copy.as_deref(),
                        owner.as_deref(),
                    ) {
                        return None;
                    }
                }
                if let TurnBlock::TurnFooter(footer) = block
                    && !footer_has_content(footer)
                {
                    return None;
                }
                Some(block_scroll_identity(&turn.turn_id, block))
            })
            .collect();
        let surface = entity.downgrade();
        let turn_element =
            turn_element.on_children_prepainted(move |children_bounds, window, app| {
                let _ = surface.update(app, |surface, _| {
                    surface.apply_executed_scroll_targets(
                        &child_identities,
                        &children_bounds,
                        window,
                    );
                });
            });
        let turn_element = anchors.attach(
            turn_element,
            SceneId::parse(turn.turn_id.as_str()).ok().as_ref(),
            None,
        );
        let mut turn_element = turn_element.debug_selector(move || selector.clone());

        for (block_index, block) in turn.blocks().iter().enumerate() {
            if let Some(element) = self.render_block(
                &turn.turn_id,
                turn.lifecycle,
                block,
                entity,
                theme,
                anchors,
                &mut *window,
                status_motion,
                block_index,
                &live_header,
                live_owner,
                cx,
            ) {
                turn_element = turn_element.child(element);
            }
        }

        turn_element.into_any_element()
    }

    fn render_block(
        &self,
        turn_id: &TurnId,
        turn_lifecycle: ConversationLifecycle,
        block: &TurnBlock,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
        window: &mut Window,
        status_motion: MotionPolicy,
        block_index: usize,
        turn_live_header: &Option<String>,
        live_owner_index: Option<usize>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let selector = block_selector(turn_id, block);
        match block {
            TurnBlock::UserMessage(block) => {
                Some(self.render_user_message(block, selector, theme, cx))
            }
            TurnBlock::AssistantMessage(block) => {
                Some(self.render_assistant_message(block, selector, entity, theme, anchors))
            }
            TurnBlock::WorkGroup(block) => {
                // Only the owning (latest) group headers the live line, so
                // it paints exactly once per turn.
                let owned = if live_owner_index == Some(block_index) {
                    turn_live_header.clone()
                } else {
                    None
                };
                // Mounted-working freezes in retained window state on first
                // mount: history that later goes live must never enter.
                let mounted_working = {
                    let working_now = owned.is_some();
                    let state = window.use_keyed_state(
                        ElementId::Name(SharedString::from(format!("{selector}-mounted-working"))),
                        cx,
                        move |_, _| working_now,
                    );
                    *state.read(cx)
                };
                Some(self.render_work_group(
                    turn_id,
                    turn_lifecycle,
                    block,
                    selector,
                    entity,
                    theme,
                    anchors,
                    owned,
                    status_motion,
                    mounted_working,
                    &mut *window,
                    cx,
                ))
            }
            TurnBlock::Compaction(block) => {
                Some(self.render_compaction(block, selector, entity, theme, anchors))
            }
            TurnBlock::ChangeSet(block) => {
                Some(self.render_change_set(block, selector, entity, theme, anchors))
            }
            TurnBlock::Plan(block) => {
                Some(self.render_plan(block, selector, entity, theme, anchors))
            }
            TurnBlock::Approval(block) => {
                Some(self.render_approval(block, selector, entity, theme, anchors))
            }
            TurnBlock::Question(block) => {
                Some(self.render_question(block, selector, entity, theme, anchors))
            }
            TurnBlock::Error(block) => {
                Some(self.render_error(block, selector, entity, theme, anchors))
            }
            TurnBlock::UsageInterruption(block) => {
                Some(self.render_usage_interruption(block, selector, entity, theme, anchors))
            }
            TurnBlock::ModelTransition(block) => {
                Some(self.render_model_transition(block, selector, entity, theme, anchors))
            }
            TurnBlock::NativeFact(block) => {
                Some(self.render_native_fact(block, selector, entity, theme, anchors))
            }
            TurnBlock::SteeringLabel(block) => {
                Some(Self::render_steering(block, selector, theme, anchors))
            }
            TurnBlock::TurnStatus(block) => {
                self.render_status(turn_id, block, selector, theme, status_motion)
            }
            TurnBlock::TurnFooter(block) => {
                self.render_footer(turn_id, block, selector, entity, theme, window)
            }
        }
    }

    fn render_user_message(
        &self,
        block: &UserMessageBlock,
        selector: String,
        theme: &ArtisanTheme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Parity with conversation-message.svelte user branch: right-aligned
        // gradient bubble, rounded-2xl, plain pre-wrap paragraph, no title.
        // Reference `bg-linear-to-t from-surface-850 to-surface-775` lays the
        // from-stop at the bottom, so the native face runs S775 (top) to
        // S850 (bottom) through the existing two-stop GPUI gradient.
        let body_selector = format!("{selector}-body");
        let mut message = div().w_full().flex().flex_col().items_end().gap(px(8.0));
        if let Some(images) = self.message_images.as_ref() {
            let mut tray = div()
                .flex()
                .flex_wrap()
                .justify_end()
                .gap(px(8.0))
                .max_w(px(576.0));
            for reference in &block.attachments {
                let tile = images.update(cx, |images, cx| {
                    images
                        .render_thumbnail(reference, *theme, cx)
                        .into_any_element()
                });
                tray = tray.child(tile);
            }
            if !block.attachments.is_empty() {
                message = message.child(tray);
            }
        }
        if !block.body.is_empty() {
            // The body text is the shared selectable element: retained state
            // (selection, drag latch, focus) lives in framework element state
            // under the stable body id across frames, with no caller maps or
            // focus handles. Styling stays on the container (prose size,
            // 28 px line height, 410 weight, bubble metrics, gradient face
            // with the reference card shadow beneath), so no duplicate body,
            // glyph, or padding is introduced.
            let body_id = SharedString::from(body_selector.clone());
            message = message.child(
                div()
                    .max_w(px(576.0))
                    .rounded(RadiusTokens::value(RadiusStep::X2l))
                    .bg(vertical_gradient(
                        SurfaceStep::S775.oklch(),
                        SurfaceStep::S850.oklch(),
                    ))
                    .shadow(
                        theme
                            .elevation
                            .card_shadow
                            .iter()
                            .map(|layer| layer.to_box_shadow())
                            .collect::<Vec<_>>(),
                    )
                    .px(px(16.0))
                    .py(px(12.0))
                    .debug_selector(move || selector.clone())
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .text_size(px(ProseTypography::BODY_SIZE_PX))
                            .line_height(px(ProseTypography::BODY_LINE_PX))
                            .font_weight(ProseTypography::BODY_WEIGHT)
                            .letter_spacing(px(ProseTypography::BODY_TRACKING_PX))
                            .whitespace_normal()
                            .debug_selector(move || body_selector.clone())
                            .child(SelectableText::retained(
                                body_id,
                                block.body.clone(),
                                *theme,
                                Vec::new(),
                            )),
                    ),
            );
        }
        message.into_any_element()
    }

    fn render_assistant_message(
        &self,
        block: &crate::conversation_scene::AssistantMessageBlock,
        selector: String,
        _entity: &Entity<Self>,
        theme: &ArtisanTheme,
        _anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        // Parity with conversation-message.svelte assistant branch: chromeless
        // markdown at prose width, no card, no title. The reply body reads in
        // the foreground token per product direction; detail prose keeps the
        // reference muted body.
        let rich_link_titles =
            self.rich_link_probe(crate::conversation_host::host_now_millis());
        let rendered_body = self.markdown_renderer.render_source_with_tone_and_titles(
            &block.body,
            *theme,
            selector.clone(),
            MarkdownBodyTone::Foreground,
            &rich_link_titles,
        );
        div()
            .w_full()
            .max_w(px(672.0))
            .debug_selector(move || selector.clone())
            .child(rendered_body)
            .into_any_element()
    }

    fn render_text_block(
        &self,
        params: TextBlockRender<'_>,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        let TextBlockRender {
            id,
            disclosure,
            selector,
            title,
            body,
        } = params;
        let style = CardStyle::resolve(*theme);
        self.render_controlled_card(
            ControlledCardOptions {
                id: id.clone(),
                item_id: item_id_for_scene_id(id),
                disclosure,
                selector,
                style,
            },
            compact_card_content(style).child(card_heading(title, theme)),
            compact_card_content(style).child(body_text(body, theme)),
            entity,
            anchors,
        )
    }

    fn render_work_group(
        &self,
        turn_id: &TurnId,
        turn_lifecycle: ConversationLifecycle,
        block: &WorkGroupBlock,
        selector: String,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
        live_header: Option<String>,
        status_motion: MotionPolicy,
        mounted_working: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // The stable anchor prefers the session id (disclosure/scroll key);
        // legacy positional groups fall back to the first-item derivation.
        let group_id = block
            .session
            .clone()
            .or_else(|| work_group_anchor_id(turn_id, block));
        // Terminal duration wins; otherwise the owning group headers the
        // turn's live Thinking/Working line once (see render_turn). Earlier
        // groups and the separate status row stand down, so the line paints
        // exactly once per turn.
        let terminal = work_group_header_copy(block.label);
        let owns_live_header = live_header.is_some();
        let header = terminal.or(live_header);
        // Engine handoffs fold into the header far end, never as standalone
        // timeline rows while a session hosts them.
        let transition = block
            .transition
            .as_ref()
            .map(|handoff| format!("{} → {}", handoff.from_model, handoff.to_model));

        // Controlled state is never overridden: Closed hides the panel in
        // every case, and the toggle always flows through the existing
        // disclosure action. Uncontrolled groups always show their items,
        // exactly like the previous static branch did. Control additionally
        // requires actual visible trace content: an empty `Thought for …`
        // group paints its header with no chevron at all, never a blank
        // disclosure.
        let controlled = group_id.is_some()
            && block.disclosure.is_some()
            && work_group_has_visible_details(block);
        let open = !controlled || !matches!(block.disclosure, Some(SceneDisclosure::Closed));
        let items_mounted =
            !controlled || !matches!(block.disclosure, Some(SceneDisclosure::Closed));

        let mut items = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(2.0));
        let rows = ordered_detail_rows(block);
        // Consecutive activity rows form one collapsible trace chain, exactly
        // like the reference segmentation: anything else (assistant prose,
        // compaction, native fact) is a seam that ends the chain. Chains keep
        // their rows as their own children so each row's measured bound stays
        // reachable by the scroll-target handoff below.
        let mut child_identities: Vec<(Option<SceneId>, Option<ItemId>)> = Vec::new();
        let mut chain_index = 0usize;
        let mut row_index = 0usize;
        while row_index < rows.len() {
            let is_activity = matches!(rows[row_index].1, DetailRow::Activity { .. });
            if is_activity {
                let start = row_index;
                while row_index < rows.len()
                    && matches!(rows[row_index].1, DetailRow::Activity { .. })
                {
                    row_index += 1;
                }
                let chain_rows = &rows[start..row_index];
                let (live, failed) =
                    activity_chain_disclosure_state(chain_rows, owns_live_header, turn_lifecycle);
                items = items.child(self.render_trace_chain(
                    &selector,
                    chain_index,
                    chain_rows,
                    live,
                    failed,
                    status_motion,
                    entity,
                    theme,
                    anchors,
                    items_mounted,
                    window,
                    cx,
                ));
                // The chain's own prepaint listener resolves its row bounds;
                // at this level the chain is opaque to row targets so a
                // collapsed chain cannot resolve one against its header.
                child_identities.push((None, None));
                chain_index += 1;
            } else {
                items = items.child(self.render_detail_row(
                    rows[row_index].1,
                    rows[row_index].0,
                    &selector,
                    entity,
                    theme,
                    anchors,
                    items_mounted,
                ));
                let id = rows[row_index].1.scene_id();
                child_identities.push((Some(id.clone()), item_id_for_scene_id(id)));
                row_index += 1;
            }
        }
        if header.is_some() {
            items = items.pt(theme.spacing.steps(2.0));
        }
        // Identities mirror painted children in order, so executed scroll
        // targets resolve against measured child bounds one-to-one.
        let surface = entity.downgrade();
        items = items.on_children_prepainted(move |children_bounds, window, app| {
            let _ = surface.update(app, |surface, _| {
                surface.apply_executed_scroll_targets(&child_identities, &children_bounds, window);
            });
        });

        // Plain section on the transcript surface: reference work sessions
        // and activity rows render without card chrome.
        let section = div().w_full().min_w_0().flex().flex_col();
        let mut section = anchors.attach(section, group_id.as_ref(), None);
        section = section.debug_selector({
            let selector = selector.clone();
            move || selector.clone()
        });
        // One header element for the group's whole life: the reference
        // keeps a single session header carrying both the entrance and the
        // divider, so plain and disclosable renders share this construction.
        // Splitting the headers would remount the row the instant the first
        // detail arrived.
        let header_row = Self::work_group_header_row(
            header,
            transition,
            open,
            controlled,
            &selector,
            status_motion,
            mounted_working,
            theme,
        );
        // The disclosure root wraps header plus panel in every state so the
        // header ancestry never changes; registering disclosure later must
        // not remount the row. `Collapsible` cannot own this slot: it unmounts
        // closed content and holds no clock, which is exactly the flash the
        // reference `t-acc-panel` lip removes.
        let disclosure_selector = format!("{selector}-disclosure");
        let base_trigger = div()
            .min_w_0()
            .child(header_row)
            .debug_selector({
                let trigger_selector = format!("{disclosure_selector}-trigger");
                move || trigger_selector.clone()
            });
        let trigger: AnyElement = if controlled {
            // The toggle callback exists only for controlled groups; the
            // wrapper itself stays mounted in every case. Uncontrolled groups
            // have no scene identity to address, and their trigger is inert.
            let surface = entity.downgrade();
            let action_id = group_id
                .clone()
                .expect("controlled groups carry a scene identity");
            let requested_open = !open;
            base_trigger
                .id(ElementId::Name(SharedString::from(format!(
                    "{disclosure_selector}-trigger"
                ))))
                .track_focus(&self.disclosure_focus)
                .on_click(move |event, _, app| {
                    if !event.standard_click() {
                        return;
                    }
                    let action = ConversationSurfaceAction::DisclosureToggleRequested {
                        id: action_id.clone(),
                        requested_open,
                    };
                    let _ = surface.update(app, |surface, cx| {
                        if surface.enqueue_action(action) {
                            cx.notify();
                        }
                    });
                })
                .into_any_element()
        } else {
            base_trigger.into_any_element()
        };
        let disclosure = div()
            .flex()
            .flex_col()
            .min_w_0()
            .child(trigger)
            .debug_selector({
                let root_selector = disclosure_selector.clone();
                move || root_selector.clone()
            });

        // The panel is the reference lip as a real clipped height. Its content
        // stays mounted only while it can be seen or measured (open, or a
        // collapse still in flight), so settled history keeps the reference's
        // unmounted-details policy.
        let panel = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .debug_selector({
                let panel_selector = format!("{selector}-disclosure-panel");
                move || panel_selector.clone()
            });

        let disclosure = disclosure_flight_panel(
            disclosure,
            panel,
            items.into_any_element(),
            &selector,
            open,
            controlled,
            status_motion,
            window,
            cx,
        );

        section.child(disclosure).into_any_element()
    }

    /// Renders one contiguous activity chain with the reference trace header,
    /// left rail, and rows.
    ///
    /// The header carries the category icon, the counted clause sentence, and
    /// the chevron; the rows indent under a 2 px rail inside the shared
    /// accordion panel. Disclosure is surface-local state keyed by the first
    /// activity's identity, exactly like the reference `open_groups`: an
    /// unset chain re-evaluates its failed/live default every render, and a
    /// user toggle pins the value for the surface's life.
    #[allow(clippy::too_many_arguments)]
    fn render_trace_chain(
        &self,
        group_selector: &str,
        chain_index: usize,
        rows: &[(u64, DetailRow<'_>)],
        live: bool,
        failed: bool,
        motion: MotionPolicy,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
        mounted: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selector = format!("{group_selector}-trace-{chain_index}");
        let chain_key = rows
            .first()
            .map(|(_, row)| row.scene_id().as_str().to_owned())
            .unwrap_or_default();
        let open = self
            .trace_groups_open
            .borrow()
            .get(&chain_key)
            .copied()
            .unwrap_or(live || failed);
        let header_color = if open {
            theme.colors.foreground
        } else {
            theme.colors.muted_foreground
        };
        let kinds: Vec<&str> = rows
            .iter()
            .map(|(_, row)| match row {
                DetailRow::Activity { kind, .. } => (*kind).unwrap_or("tool"),
                _ => "tool",
            })
            .collect();
        let clause = activity_chain_clause(kinds.iter().copied());
        let icon = activity_chain_icon(kinds.iter().copied());

        let header = div()
            .id(ElementId::Name(SharedString::from(format!(
                "{selector}-trigger"
            ))))
            .flex()
            .flex_row()
            .items_center()
            .gap(theme.spacing.steps(2.0))
            .w_full()
            .min_w_0()
            .py(theme.spacing.steps(0.5))
            .cursor_pointer()
            .track_focus(&self.disclosure_focus)
            .aria_label("Toggle activity details")
            .debug_selector({
                let trigger_selector = format!("{selector}-trigger");
                move || trigger_selector.clone()
            })
            .on_click({
                let surface = entity.downgrade();
                let chain_key = chain_key.clone();
                let requested_open = !open;
                move |event, _, app| {
                    if !event.standard_click() {
                        return;
                    }
                    let _ = surface.update(app, |surface, cx| {
                        surface
                            .trace_groups_open
                            .borrow_mut()
                            .insert(chain_key.clone(), requested_open);
                        cx.notify();
                    });
                }
            })
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(header_color.to_paint())
                    .child(asset_glyph(icon).size(px(16.0))),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_size(px(ProseTypography::BODY_SIZE_PX))
                    .line_height(theme.spacing.steps(6.0))
                    .text_color(header_color.to_paint())
                    .child(clause),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(header_color.to_paint())
                    .child(
                        asset_glyph(if open {
                            AssetId::TABLER_CHEVRON_DOWN
                        } else {
                            AssetId::TABLER_CHEVRON_RIGHT
                        })
                        .size(px(14.0)),
                    ),
            );

        // The reference `pl-6` rail column: one 16 px absolute rail with a
        // centered 2 px `border/60` line, and the rows flowing beside it.
        let mut rows_column = div()
            .relative()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(1.0))
            .pl(theme.spacing.steps(6.0))
            .child(
                div()
                    .absolute()
                    .top(px(0.0))
                    .bottom(px(0.0))
                    .left(px(0.0))
                    .w(theme.spacing.steps(4.0))
                    .child(
                        div()
                            .absolute()
                            .top(px(0.0))
                            .bottom(px(0.0))
                            .left(theme.spacing.steps(1.75))
                            .w(px(2.0))
                            .bg(theme.colors.border.with_alpha(0.6).to_paint())
                            .debug_selector({
                                let rail_selector = format!("{selector}-rail");
                                move || rail_selector.clone()
                            }),
                    ),
            );
        // The absolute rail is the first child, so it takes the leading
        // identity slot and the rows line up one-to-one behind it.
        let mut row_identities: Vec<(Option<SceneId>, Option<ItemId>)> = vec![(None, None)];
        for (ordinal, row) in rows {
            rows_column = rows_column.child(self.render_detail_row(
                *row,
                *ordinal,
                group_selector,
                entity,
                theme,
                anchors,
                mounted,
            ));
            let id = row.scene_id();
            row_identities.push((Some(id.clone()), item_id_for_scene_id(id)));
        }
        let surface = entity.downgrade();
        rows_column = rows_column.on_children_prepainted(move |children_bounds, window, app| {
            let _ = surface.update(app, |surface, _| {
                surface.apply_executed_scroll_targets(&row_identities, &children_bounds, window);
            });
        });

        let disclosure = div()
            .flex()
            .flex_col()
            .min_w_0()
            .child(header)
            .debug_selector({
                let root_selector = format!("{selector}-disclosure");
                move || root_selector.clone()
            });
        let panel = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .debug_selector({
                let panel_selector = format!("{selector}-disclosure-panel");
                move || panel_selector.clone()
            });

        disclosure_flight_panel(
            disclosure,
            panel,
            rows_column.into_any_element(),
            &selector,
            open,
            true,
            motion,
            window,
            cx,
        )
    }

    /// Renders one ordered detail row with its scroll anchor.
    ///
    /// Per-row disclosure stays data-only: visibility follows the group
    /// control, matching the reference grouping, which never shows nested
    /// toggles. Assistant details render full markdown like top-level
    /// replies; compaction and native facts reuse the native card
    /// presentation statically.
    fn render_detail_row(
        &self,
        row: DetailRow<'_>,
        ordinal: u64,
        group_selector: &str,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
        mounted: bool,
    ) -> AnyElement {
        let selector = format!("{group_selector}-detail-{ordinal}");
        let row_id = row.scene_id().clone();
        match &row {
            DetailRow::Compaction { id, summary, .. } => self.render_text_block(
                TextBlockRender {
                    id,
                    disclosure: None,
                    selector,
                    title: "Compaction",
                    body: summary,
                },
                entity,
                theme,
                anchors,
            ),
            DetailRow::NativeFact { id, text, .. } => self.render_text_block(
                TextBlockRender {
                    id,
                    disclosure: None,
                    selector,
                    title: "Native fact",
                    body: text,
                },
                entity,
                theme,
                anchors,
            ),
            row => {
                let content: AnyElement = match &row {
                    DetailRow::Assistant { body, .. } => {
                        let rich_link_titles =
                            self.rich_link_probe(crate::conversation_host::host_now_millis());
                        let rendered = self.markdown_renderer.render_source_with_tone_and_titles(
                            body,
                            *theme,
                            format!("{selector}-markdown"),
                            MarkdownBodyTone::Muted,
                            &rich_link_titles,
                        );
                        div()
                            .w_full()
                            .max_w(px(672.0))
                            .child(rendered)
                            .into_any_element()
                    }
                    // The reference activity row: the category label in the
                    // foreground, then the detail — normalized and mono for
                    // terminal commands, raw and truncated otherwise — or the
                    // presentation label when no detail was disclosed. Rows
                    // without a provider kind keep the legacy flat body.
                    DetailRow::Activity {
                        body,
                        kind,
                        detail,
                        lifecycle,
                        ..
                    } => match kind {
                        Some(kind) => {
                            let category = activity_category(kind);
                            let row_text = activity_detail_text(kind, *detail, *lifecycle);
                            let detail_element: AnyElement = if *kind == "terminal_activity" {
                                // The monospace face already says "shell": the
                                // normalized command truncates on one line at
                                // the reference `font-mono text-sm` recipe.
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .font_family(theme.typography.mono.family)
                                    .text_size(theme.typography.control_text)
                                    .text_color(theme.colors.muted_foreground.to_paint())
                                    .child(row_text)
                                    .into_any_element()
                            } else {
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .text_color(theme.colors.muted_foreground.to_paint())
                                    .child(row_text)
                                    .into_any_element()
                            };
                            div()
                                .w_full()
                                .min_w_0()
                                .py(theme.spacing.steps(0.5))
                                .text_size(px(ProseTypography::BODY_SIZE_PX))
                                .line_height(theme.spacing.steps(6.0))
                                .font_weight(ProseTypography::BODY_WEIGHT)
                                .letter_spacing(px(ProseTypography::body_tracking_px(
                                    ProseTypography::BODY_SIZE_PX,
                                )))
                                .text_color(theme.colors.muted_foreground.to_paint())
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap(theme.spacing.steps(2.0))
                                        .min_w_0()
                                        .child(
                                            div()
                                                .flex_shrink_0()
                                                .text_color(theme.colors.foreground.to_paint())
                                                .child(category.label()),
                                        )
                                        .child(detail_element),
                                )
                                .into_any_element()
                        }
                        None => div()
                            .w_full()
                            .min_w_0()
                            .text_size(theme.typography.control_text)
                            .font_weight(ProseTypography::BODY_WEIGHT)
                            .letter_spacing(px(ProseTypography::body_tracking_px(14.0)))
                            .text_color(theme.colors.foreground.to_paint())
                            .child(body.to_string())
                            .into_any_element(),
                    },
                    // Session titles render muted at base size (reference
                    // header tone); counting lives in the group header and
                    // status row.
                    DetailRow::SessionTitle { title, .. } => div()
                        .w_full()
                        .min_w_0()
                        .text_size(px(ProseTypography::BODY_SIZE_PX))
                        .line_height(theme.spacing.steps(6.0))
                        .font_weight(ProseTypography::BODY_WEIGHT)
                        .letter_spacing(px(ProseTypography::BODY_TRACKING_PX))
                        .text_color(theme.colors.muted_foreground.to_paint())
                        .child(title.to_string())
                        .into_any_element(),
                    DetailRow::Compaction { .. } | DetailRow::NativeFact { .. } => {
                        unreachable!("card rows render above")
                    }
                };
                if mounted {
                    let mut element = anchors.attach(
                        div().w_full().min_w_0(),
                        Some(&row_id),
                        item_id_for_scene_id(&row_id).as_ref(),
                    );
                    element = element.debug_selector(move || selector.clone());
                    element.child(content).into_any_element()
                } else {
                    div()
                        .w_full()
                        .min_w_0()
                        .debug_selector(move || selector.clone())
                        .child(content)
                        .into_any_element()
                }
            }
        }
    }

    /// Builds the single session header row shared by plain and disclosable
    /// renders, mirroring `conversation-work-session.svelte` §469:
    /// `relative flex w-full items-center justify-between gap-3 pb-2` with
    /// the label (or disclosure chevron) at the near end, an engine handoff
    /// at the far end, and the 1 px settled divider pinned to the bottom
    /// edge. Controlled groups carry the label tone on the chevron;
    /// uncontrolled text stays static; a headerless controlled group keeps
    /// the chevron-only affordance with an honest accessible name —
    /// disclosure chrome, never invented content.
    fn work_group_header_row(
        label: Option<String>,
        transition: Option<String>,
        open: bool,
        controlled: bool,
        selector: &str,
        motion: MotionPolicy,
        mounted_working: bool,
        theme: &ArtisanTheme,
    ) -> AnyElement {
        let near: AnyElement = match (controlled, label) {
            (true, Some(label)) => div()
                .flex()
                .flex_row()
                .items_center()
                .gap(theme.spacing.steps(1.0))
                .min_w_0()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(label)
                .child(Self::work_group_chevron(open, theme))
                .into_any_element(),
            (true, None) => div()
                .id(format!("{selector}-work-trigger"))
                .flex()
                .flex_row()
                .items_center()
                .text_color(theme.colors.muted_foreground.to_paint())
                .aria_label("Toggle work details")
                .child(Self::work_group_chevron(open, theme))
                .into_any_element(),
            (false, Some(label)) => div()
                .min_w_0()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(label)
                .into_any_element(),
            (false, None) => div().into_any_element(),
        };
        let mut header = div()
            .relative()
            .w_full()
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(theme.spacing.steps(3.0))
            .text_size(px(ProseTypography::BODY_SIZE_PX))
            .line_height(theme.spacing.steps(6.0))
            .font_weight(ProseTypography::BODY_WEIGHT)
            .letter_spacing(px(ProseTypography::body_tracking_px(
                ProseTypography::BODY_SIZE_PX,
            )))
            .text_color(theme.colors.muted_foreground.to_paint())
            .pb(theme.spacing.steps(2.0))
            .debug_selector({
                let selector = format!("{selector}-header");
                move || selector.clone()
            })
            .child(near);
        if let Some(handoff) = transition {
            header = header.child(
                div()
                    .flex_shrink_0()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(handoff),
            );
        }
        // The settled divider: the reference `t-settle-underline` base rule,
        // always painted at its exact end geometry. Growing it from the
        // measured label width has no text-measurement primitive here, so no
        // width tween is faked; the rule rides inside the entrance below
        // while one plays.
        header = header.child(
            separator(theme.colors.border.to_paint(), SeparatorAxis::Horizontal)
                .absolute()
                .bottom(px(0.0))
                .left(px(0.0)),
        );
        // Mounted-working entrance only: the reference `status-swap-enter`
        // plays solely for headers mounted while working, and history
        // arriving settled stays static. The frozen flag (retained window
        // state from first mount) means history that later goes live never
        // enters. The selector-keyed chain plays once per group —
        // re-renders never restart it — and settling leaves it at its end
        // state structurally. Hold plus enter mirror the reference 150 ms
        // delay and 150 ms EaseInOut run; opacity, relative 4 px rise (layout
        // neutral, like the reference translate), and 2 px blur all ride the
        // same eased clock. Reduced motion skips the wrapper and rests at
        // the unfiltered state.
        if !mounted_working {
            return header.into_any_element();
        }
        match motion.resolve(MotionRecipe::TextSwap) {
            MotionPlan::Immediate => header.into_any_element(),
            MotionPlan::Animate(animation) => header
                .opacity(0.0)
                .with_animations(
                    ElementId::Name(SharedString::from(format!("{selector}-header-enter"))),
                    vec![
                        Animation::new(MotionDuration::Quick.as_duration()),
                        animation.gpui_clock(),
                    ],
                    |header, index, value| {
                        if index == 0 {
                            header
                                .opacity(0.0)
                                .top(px(-4.0))
                                .filter(vec![Filter::Blur(px(2.0))])
                        } else {
                            let eased = MotionCurve::EaseInOut.sample(f64::from(value)) as f32;
                            header
                                .opacity(eased)
                                .top(px(-4.0 * (1.0 - eased)))
                                .filter(vec![Filter::Blur(px(2.0 * (1.0 - eased)))])
                        }
                    },
                )
                .into_any_element(),
        }
    }

    /// Disclosure chevron for one session header: the reference `size-4`
    /// `ChevronRight` rotated 90 degrees when open. Rotation rides the SVG
    /// render transformation, which [`AssetGlyph`] does not forward yet, so
    /// open swaps in the down glyph at the same 16 px muted geometry — the
    /// accordion lane's own discrete mapping — with no tween claimed. The
    /// exact rotation needs a minimal shared forwarding,
    /// `AssetGlyph::with_transformation(Transformation)`, owned by the
    /// asset-seam lane; with it the chevron becomes one right glyph under
    /// `Transformation::rotate` on the 250 ms `AccordionChevron` clock.
    fn work_group_chevron(open: bool, theme: &ArtisanTheme) -> AnyElement {
        asset_glyph(if open {
            AssetId::TABLER_CHEVRON_DOWN
        } else {
            AssetId::TABLER_CHEVRON_RIGHT
        })
        .size(px(16.0))
        .text_color(theme.colors.muted_foreground.to_paint())
        .into_any_element()
    }

    fn render_compaction(
        &self,
        block: &CompactionBlock,
        selector: String,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        self.render_text_block(
            TextBlockRender {
                id: &block.id,
                disclosure: block.disclosure,
                selector,
                title: "Compaction",
                body: &block.summary,
            },
            entity,
            theme,
            anchors,
        )
    }

    fn render_change_set(
        &self,
        block: &ChangeSetBlock,
        selector: String,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        let style = CardStyle::resolve(*theme);
        let header = card_heading(format!("Changed files ({})", block.files.len()), theme);
        let mut rows = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(2.0));
        for (index, file) in block.files.iter().enumerate() {
            rows = rows.child(changed_file_row(&block.id, index, file, theme));
            if index + 1 < block.files.len() {
                rows = rows.child(separator(
                    theme.colors.border.to_paint(),
                    SeparatorAxis::Horizontal,
                ));
            }
        }
        self.render_controlled_card(
            ControlledCardOptions {
                id: block.id.clone(),
                item_id: item_id_for_scene_id(&block.id),
                disclosure: block.disclosure,
                selector,
                style,
            },
            compact_card_content(style).child(header),
            compact_card_content(style).child(rows),
            entity,
            anchors,
        )
    }

    fn render_plan(
        &self,
        block: &PlanBlock,
        selector: String,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        let style = CardStyle::resolve(*theme);
        let mut entries = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(2.0));
        for entry in &block.entries {
            entries = entries.child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(theme.spacing.steps(2.0))
                    .child(div().flex_shrink_0().child("•"))
                    .child(body_text(entry, theme)),
            );
        }
        let content = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(2.0))
            .child(body_text(&block.title, theme))
            .child(entries);
        self.render_controlled_card(
            ControlledCardOptions {
                id: block.id.clone(),
                item_id: item_id_for_scene_id(&block.id),
                disclosure: block.disclosure,
                selector,
                style,
            },
            compact_card_content(style).child(card_heading("Plan", theme)),
            compact_card_content(style).child(content),
            entity,
            anchors,
        )
    }

    fn render_approval(
        &self,
        block: &crate::conversation_scene::ApprovalBlock,
        selector: String,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        let style = CardStyle::resolve(*theme);
        let key = block.id.as_str().to_owned();
        let approval_id = ObservationId::parse(block.id.as_str()).ok();
        let gate = self.approval_gates.get(&key);
        let in_flight = gate.is_some_and(ApprovalAnswerGate::is_in_flight);
        let pending = gate.and_then(ApprovalAnswerGate::pending_decision);
        let failure = gate.and_then(ApprovalAnswerGate::failure_message);
        let ready =
            approval_id.is_some() && self.answer_thread.is_some() && self.answer_run.is_some();
        let disabled = !ready || in_flight;
        let approve_label = if pending == Some(true) {
            pending_approval_label(&PresentationApprovalKind::Action, false)
        } else {
            "Approve"
        };
        let deny_label = if pending == Some(false) {
            APPROVAL_DENYING_LABEL
        } else {
            APPROVAL_DENY_LABEL
        };

        let surface = entity.downgrade();
        let approve_key = key.clone();
        let approve_id = approval_id.clone();
        let approve_button = Button::new(
            SharedString::from(format!("{selector}-approve")),
            self.answer_focus.clone(),
            *theme,
            MotionPolicy::Reduced,
            ButtonVariant::Default,
            ButtonSize::Small,
            ButtonContent::text(approve_label),
        )
        .expect("static approval confirm button configuration is valid")
        .focus_visibility(FocusVisibility::Visible)
        .debug_selector(format!("{selector}-{APPROVAL_CONFIRM_SELECTOR_SUFFIX}"))
        .disabled(disabled)
        .on_activate(move |_, _, app| {
            if let Some(approval_id) = approve_id.clone() {
                let _ = surface.update(app, |surface, cx| {
                    surface.submit_approval_gesture(&approve_key, &approval_id, true, cx);
                });
            }
        });

        let surface = entity.downgrade();
        let deny_key = key.clone();
        let deny_button = Button::new(
            SharedString::from(format!("{selector}-deny")),
            self.answer_focus.clone(),
            *theme,
            MotionPolicy::Reduced,
            ButtonVariant::Outline,
            ButtonSize::Small,
            ButtonContent::text(deny_label),
        )
        .expect("static approval deny button configuration is valid")
        .focus_visibility(FocusVisibility::Visible)
        .debug_selector(format!("{selector}-{APPROVAL_DENY_SELECTOR_SUFFIX}"))
        .disabled(disabled)
        .on_activate(move |_, _, app| {
            if let Some(approval_id) = approval_id.clone() {
                let _ = surface.update(app, |surface, cx| {
                    surface.submit_approval_gesture(&deny_key, &approval_id, false, cx);
                });
            }
        });

        let mut details = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(2.0))
            .child(body_text(&block.prompt, theme))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(theme.spacing.steps(2.0))
                    .child(approve_button)
                    .child(deny_button),
            );
        if let Some(failure) = failure {
            let failure_selector = format!("{selector}-{APPROVAL_FAILURE_SELECTOR_SUFFIX}");
            details = details
                .child(body_text(failure, theme).debug_selector(move || failure_selector.clone()));
        }
        self.render_controlled_card(
            ControlledCardOptions {
                id: block.id.clone(),
                item_id: item_id_for_scene_id(&block.id),
                disclosure: block.disclosure,
                selector,
                style,
            },
            compact_card_content(style).child(card_heading("Approval requested", theme)),
            compact_card_content(style).child(details),
            entity,
            anchors,
        )
    }

    fn render_question(
        &self,
        block: &QuestionBlock,
        selector: String,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        let style = CardStyle::resolve(*theme);
        let key = block.id.as_str().to_owned();
        let choices = self.question_choices.get(&key);
        let gate = self.question_gates.get(&key);
        let in_flight = gate.is_some_and(QuestionAnswerGate::is_in_flight);
        let failure = gate.and_then(QuestionAnswerGate::failure_message);
        let ready = ObservationId::parse(block.id.as_str()).is_ok()
            && self.answer_thread.is_some()
            && self.answer_run.is_some();
        let is_choice = choices.is_some_and(QuestionChoiceCache::is_choice);

        let mut details = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(2.0))
            .child(body_text(&block.prompt, theme));

        if is_choice {
            let cache = choices.expect("choice presence was checked");
            let no_selection: &[String] = &[];
            let selected: &[String] = gate.map_or(no_selection, QuestionAnswerGate::selected);
            let mut options = div()
                .w_full()
                .flex()
                .flex_col()
                .gap(theme.spacing.steps(2.0));
            for (index, (label, description)) in cache.options.iter().enumerate() {
                let chosen = selected.iter().any(|known| known == label);
                let surface = entity.downgrade();
                let option_key = key.clone();
                let option_label = label.clone();
                let option_selector =
                    format!("{selector}-{QUESTION_OPTION_SELECTOR_SUFFIX}-{index}");
                let option_button = Button::new(
                    SharedString::from(option_selector.clone()),
                    self.answer_focus.clone(),
                    *theme,
                    MotionPolicy::Reduced,
                    if chosen {
                        ButtonVariant::Default
                    } else {
                        ButtonVariant::Outline
                    },
                    ButtonSize::Small,
                    ButtonContent::text(label.clone()),
                )
                .expect("question option button configuration is valid")
                .focus_visibility(FocusVisibility::Visible)
                .debug_selector(option_selector)
                .disabled(!ready || in_flight)
                .on_activate(move |_, _, app| {
                    let _ = surface.update(app, |surface, cx| {
                        surface.submit_question_option_gesture(
                            &option_key,
                            option_label.clone(),
                            cx,
                        );
                    });
                });
                let mut option_row = div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(theme.spacing.steps(1.0))
                    .child(option_button);
                if let Some(description) = description {
                    option_row = option_row.child(body_text(description, theme));
                }
                options = options.child(option_row);
            }
            details = details.child(options);
            if cache.multi_select {
                let surface = entity.downgrade();
                let answer_key = key.clone();
                let answer_button = Button::new(
                    SharedString::from(format!("{selector}-answer")),
                    self.answer_focus.clone(),
                    *theme,
                    MotionPolicy::Reduced,
                    ButtonVariant::Default,
                    ButtonSize::Small,
                    ButtonContent::text(QUESTION_ANSWER_LABEL),
                )
                .expect("static question answer button configuration is valid")
                .focus_visibility(FocusVisibility::Visible)
                .debug_selector(format!("{selector}-{QUESTION_ANSWER_SELECTOR_SUFFIX}"))
                .disabled(!ready || in_flight || selected.is_empty())
                .on_activate(move |_, _, app| {
                    let _ = surface.update(app, |surface, cx| {
                        surface.submit_question_gesture(&answer_key, cx);
                    });
                });
                details = details.child(answer_button);
            }
        } else {
            let draft = gate.map_or("", QuestionAnswerGate::draft);
            let draft_text = if draft.trim().is_empty() {
                QUESTION_INPUT_PLACEHOLDER.to_owned()
            } else {
                draft.to_owned()
            };
            let input_selector = format!("{selector}-input");
            let row_focus = self
                .question_focus
                .get(&key)
                .cloned()
                .unwrap_or_else(|| self.answer_focus.clone());
            let surface = entity.downgrade();
            let key_id = key.clone();
            let transcript = self.transcript_focus.clone();
            let input = div()
                .track_focus(&row_focus)
                .tab_index(0)
                .debug_selector(move || input_selector.clone())
                .child(body_text(&draft_text, theme))
                .on_key_down(move |event, window, app| {
                    let key = event.keystroke.key.as_str().to_owned();
                    let transcript = transcript.clone();
                    let _ = surface.update(app, |surface, cx| {
                        if surface.handle_question_key(
                            &key_id,
                            &key,
                            &event.keystroke.modifiers,
                            cx,
                        ) == QuestionKeyOutcome::FocusTranscript
                        {
                            window.focus(&transcript, cx);
                        }
                    });
                });
            details = details.child(input);
            let surface = entity.downgrade();
            let answer_key = key.clone();
            let answer_button = Button::new(
                SharedString::from(format!("{selector}-answer")),
                self.answer_focus.clone(),
                *theme,
                MotionPolicy::Reduced,
                ButtonVariant::Default,
                ButtonSize::Small,
                ButtonContent::text(QUESTION_ANSWER_LABEL),
            )
            .expect("static question answer button configuration is valid")
            .focus_visibility(FocusVisibility::Visible)
            .debug_selector(format!("{selector}-{QUESTION_ANSWER_SELECTOR_SUFFIX}"))
            .disabled(!ready || in_flight || draft.trim().is_empty())
            .on_activate(move |_, _, app| {
                let _ = surface.update(app, |surface, cx| {
                    surface.submit_question_gesture(&answer_key, cx);
                });
            });
            details = details.child(answer_button);
        }
        if let Some(failure) = failure {
            let failure_selector = format!("{selector}-{QUESTION_FAILURE_SELECTOR_SUFFIX}");
            details = details
                .child(body_text(failure, theme).debug_selector(move || failure_selector.clone()));
        }
        self.render_controlled_card(
            ControlledCardOptions {
                id: block.id.clone(),
                item_id: item_id_for_scene_id(&block.id),
                disclosure: block.disclosure,
                selector,
                style,
            },
            compact_card_content(style).child(card_heading("Question", theme)),
            compact_card_content(style).child(details),
            entity,
            anchors,
        )
    }

    /// Renders one error as the reference's single destructive card.
    ///
    /// The alert carries its own face, `role="alert"` semantics, icon,
    /// title, and description: no outer wrapper card and no second heading.
    /// The recipe specializes the existing destructive style to the reference
    /// card (`rounded-xl`, destructive border/tint, tight paddings/gaps,
    /// muted description) without touching the shared global. Error facts
    /// stay mounted in every disclosure state (the reference shows no
    /// disclosure control for errors), while the stable anchor and debug
    /// selector preserve scroll and test addressing. No copy action: the
    /// scene block carries only the message.
    fn render_error(
        &self,
        block: &ErrorBlock,
        selector: String,
        _entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        let mut style = AlertStyle::resolve(*theme, AlertVariant::Destructive);
        style.corner_radius = RadiusTokens::value(RadiusStep::Xl);
        style.horizontal_padding = theme.spacing.steps(3.5);
        style.vertical_padding = theme.spacing.steps(3.0);
        style.content_gap = theme.spacing.steps(1.5);
        style.icon_gap = theme.spacing.steps(2.0);
        style.border_color = theme.colors.destructive.with_alpha(0.25).to_paint();
        style.background = theme.colors.destructive.with_alpha(0.05).to_paint();
        style.description_foreground = theme.colors.muted_foreground.to_paint();
        let alert = Alert::new(style)
            .icon(AssetId::TABLER_CIRCLE_X)
            .title("Error")
            .description(block.message.clone())
            .debug_selector(format!("{selector}-alert"));
        let mut element = anchors.attach(
            div().w_full().min_w_0(),
            Some(&block.id),
            item_id_for_scene_id(&block.id).as_ref(),
        );
        element = element.debug_selector(move || selector.clone());
        element.child(alert).into_any_element()
    }

    fn render_usage_interruption(
        &self,
        block: &UsageInterruptionBlock,
        selector: String,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        let style = CardStyle::resolve(*theme);
        let alert = Alert::from_theme(*theme, AlertVariant::Default)
            .title("Usage interruption")
            .description(block.detail.clone())
            .debug_selector(format!("{selector}-alert"));
        self.render_controlled_card(
            ControlledCardOptions {
                id: block.id.clone(),
                item_id: item_id_for_scene_id(&block.id),
                disclosure: block.disclosure,
                selector,
                style,
            },
            compact_card_content(style).child(card_heading("Usage interruption", theme)),
            alert,
            entity,
            anchors,
        )
    }

    fn render_model_transition(
        &self,
        block: &ModelTransitionBlock,
        selector: String,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        let text = format!("{} → {}", block.from_model, block.to_model);
        self.render_text_block(
            TextBlockRender {
                id: &block.id,
                disclosure: block.disclosure,
                selector,
                title: "Model transition",
                body: &text,
            },
            entity,
            theme,
            anchors,
        )
    }

    fn render_native_fact(
        &self,
        block: &NativeFactBlock,
        selector: String,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        self.render_text_block(
            TextBlockRender {
                id: &block.id,
                disclosure: block.disclosure,
                selector,
                title: "Native fact",
                body: &block.text,
            },
            entity,
            theme,
            anchors,
        )
    }

    fn render_steering(
        block: &SteeringBlock,
        selector: String,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        let user_message_anchor = anchors.anchor_for_item(&block.anchor);
        let label = div()
            .w_full()
            .min_w_0()
            .text_size(theme.typography.label_text)
            .text_color(theme.colors.muted_foreground.to_paint())
            .whitespace_normal()
            .child(block.label.clone());
        let label = anchors.attach(label, Some(&block.id), None);
        let label = label.debug_selector(move || selector.clone());
        if let Some((anchor, painted)) = user_message_anchor {
            anchors.register_item_alias(block.anchor.clone(), anchor, painted);
        }
        label.into_any_element()
    }

    fn render_status(
        &self,
        turn_id: &TurnId,
        block: &crate::conversation_scene::TurnStatusBlock,
        selector: String,
        theme: &ArtisanTheme,
        status_motion: MotionPolicy,
    ) -> Option<AnyElement> {
        // The terminal duration prefers the work-group header when the turn
        // carries the group; the reference settles to the header alone.
        let turn_scene = self.scene.turn_scene(turn_id);
        let turn_has_work_group = turn_scene.is_some_and(|turn| {
            turn.blocks()
                .iter()
                .any(|block| matches!(block, TurnBlock::WorkGroup(_)))
        });
        // The thinking line is the scene summary reduced to one line when
        // one rides the block; settled rows never carry it (builder
        // guarantee). An unfinished phase reduces to nothing and falls back
        // to the narration, exactly like the reference. Otherwise the
        // narration supplies the verb, with the engine-named wait for a
        // known provider.
        let summary: Option<String> =
            status_summary_copy(block.reasoning_summary.as_deref());
        let has_summary = summary.is_some();
        let copy = turn_status_copy_text(
            block.narration,
            block.active_started_at_ms,
            self.active_now_ms,
            block.reasoning_summary.as_deref(),
            block.engine_label.as_deref(),
        )?;
        // A live line identical to the owning group header paints once, in
        // the header; a distinct narration (a summary counts) still paints.
        // Render and scroll identities share this exact decision.
        let owner_header = if matches!(
            block.narration,
            TurnNarration::Thinking | TurnNarration::Working
        ) {
            turn_scene.and_then(|turn| turn_owner_header(turn, self.active_now_ms))
        } else {
            None
        };
        if !turn_status_paints(
            turn_has_work_group,
            block.narration,
            Some(copy.as_str()),
            owner_header.as_deref(),
        ) {
            return None;
        }
        // Parity with the work-session status line: base-size muted copy on a
        // half-rem vertical rhythm. The effective motion resolves the live
        // window signal at render time (see `effective_status_motion`); the
        // shimmer animates only for live rows under `Full` and stays
        // immediate for settled history and reduced motion. A summary sweeps
        // with the summary cadence and parses inline marks through the
        // frozen text-runs contract (faces survive the band identically
        // under Full and Reduced); verbs keep the verb cadence.
        let live = matches!(
            block.narration,
            TurnNarration::Thinking
                | TurnNarration::Working
                | TurnNarration::ProviderWait
                | TurnNarration::Compacting
                | TurnNarration::BackgroundWait
        );
        let content: AnyElement = if has_summary {
            // Faces ride the shared shimmer through the frozen text-runs
            // contract: the sweep recolors while family and zero tracking
            // compile at layout, identically under Full and Reduced, with
            // selection retained per stable id.
            let runs = inline_runs(&copy, *theme);
            ShimmerText::new(runs.text, *theme, status_motion)
                .text_runs(
                    format!("{selector}-summary"),
                    runs.highlights,
                    runs.overrides,
                )
                .active(live)
                .delay_seconds(0.0)
                .duration_seconds(2.0)
                .text_color(status_color(theme, block.narration))
                .into_element()
        } else {
            ShimmerText::new(copy, *theme, status_motion)
                .active(live)
                .delay_seconds(1.5)
                .duration_seconds(3.0)
                .text_color(status_color(theme, block.narration))
                .into_element()
        };
        let mut status = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .my(theme.spacing.steps(2.0))
            .text_size(px(ProseTypography::BODY_SIZE_PX))
            .line_height(px(ProseTypography::BODY_LINE_PX))
            .font_weight(ProseTypography::BODY_WEIGHT)
            .letter_spacing(px(ProseTypography::BODY_TRACKING_PX))
            .text_color(theme.colors.muted_foreground.to_paint())
            .child(content);
        status = status.debug_selector(move || selector.clone());
        Some(status.into_any_element())
    }

    /// Renders the hover/focus-only settled footer for one turn.
    ///
    /// Parity with `conversation-turn-footer.svelte`: an absolute row below
    /// the turn (`top_full` plus a quarter-rem margin), invisible until turn
    /// hover or copy-button focus, carrying the ghost copy control and the
    /// relative age. Only an eligible settlement paints; unsettled turns keep
    /// no placeholder and no gap slot. Hover and focus emit
    /// [`ConversationSurfaceAction::TurnFooterRevealed`] so the host can take
    /// its one clock sample; the copy gesture emits
    /// [`ConversationSurfaceAction::TurnFooterCopyRequested`] with the exact
    /// settlement bytes.
    fn render_footer(
        &self,
        turn_id: &TurnId,
        block: &TurnFooterBlock,
        selector: String,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        window: &mut Window,
    ) -> Option<AnyElement> {
        let settlement = footer_settlement(block)?;
        let key = footer_key(turn_id);
        let mirror = self.footer_mirrors.get(&key);
        let relative_age = mirror.map_or("", |staged| staged.relative_age.as_str());
        let copy_message = mirror.map_or("", |staged| staged.copy_message.as_str());
        let handle = self
            .footer_focus
            .get(&key)
            .cloned()
            .unwrap_or_else(|| self.answer_focus.clone());
        let focused = handle.is_focused(window);

        let surface = entity.downgrade();
        let copy_turn = turn_id.clone();
        let copy_text = settlement.response_text().to_owned();
        let copy_button = Button::new(
            SharedString::from(format!("{selector}-copy")),
            handle,
            *theme,
            MotionPolicy::Reduced,
            ButtonVariant::Ghost,
            ButtonSize::IconSmall,
            ButtonContent::icon_only(
                AssetId::TABLER_COPY,
                AccessibleLabel::new(COPY_RESPONSE_LABEL)
                    .expect("static footer copy label is valid"),
            ),
        )
        .expect("static footer copy button configuration is valid")
        .focus_visibility(FocusVisibility::Visible)
        .tint(
            theme.colors.muted_foreground.to_paint(),
            theme.colors.foreground.to_paint(),
        )
        .debug_selector(format!("{selector}-{FOOTER_COPY_SELECTOR_SUFFIX}"))
        .on_activate(move |_, _, app| {
            let _ = surface.update(app, |surface, cx| {
                if surface.enqueue_action(ConversationSurfaceAction::TurnFooterCopyRequested {
                    turn: copy_turn.clone(),
                    text: copy_text.clone(),
                }) {
                    cx.notify();
                }
            });
        });

        let hover_surface = entity.downgrade();
        let reveal_turn = turn_id.clone();
        let reveal_key = key.clone();
        let message_selector = format!("{selector}-copy-message");
        let time_selector = format!("{selector}-time-{}", settlement.settled_at_ms());
        let mut footer = div()
            .id(format!("{selector}-footer"))
            .absolute()
            .left(px(0.0))
            .top_full()
            .mt(theme.spacing.steps(1.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(theme.spacing.steps(1.0))
            .text_size(theme.typography.control_text)
            .font_weight(ProseTypography::BODY_WEIGHT)
            .letter_spacing(px(ProseTypography::body_tracking_px(14.0)))
            .text_color(theme.colors.muted_foreground.to_paint())
            .opacity(if focused || self.footer_revealed.as_deref() == Some(key.as_str()) {
                1.0
            } else {
                0.0
            })
            .group_hover(TURN_GROUP, |hover| hover.opacity(1.0))
            .aria_label(TURN_ACTIONS_LABEL)
            .debug_selector(move || selector.clone())
            .on_hover(move |hovered, _, app| {
                let _ = hover_surface.update(app, |surface, cx| {
                    let mut changed = footer_hover_transition(
                        &mut surface.footer_revealed,
                        &reveal_key,
                        *hovered,
                    );
                    if *hovered
                        && surface.enqueue_action(ConversationSurfaceAction::TurnFooterRevealed {
                            turn: reveal_turn.clone(),
                        })
                    {
                        changed = true;
                    }
                    if changed {
                        cx.notify();
                    }
                });
            })
            .child(copy_button);
        if !copy_message.is_empty() {
            footer = footer.child(
                div()
                    .text_color(theme.colors.destructive.to_paint())
                    .debug_selector(move || message_selector.clone())
                    .child(copy_message.to_owned()),
            );
        }
        if !relative_age.is_empty() {
            footer = footer.child(
                div()
                    .debug_selector(move || time_selector.clone())
                    .child(relative_age.to_owned()),
            );
        }
        Some(footer.into_any_element())
    }

    fn render_controlled_card(
        &self,
        options: ControlledCardOptions,
        trigger: impl IntoElement,
        content: impl IntoElement,
        entity: &Entity<Self>,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        let ControlledCardOptions {
            id,
            item_id,
            disclosure,
            selector,
            style,
        } = options;
        let disabled = disclosure.is_none();
        let open = !matches!(disclosure, Some(SceneDisclosure::Closed));
        let disclosure_selector = format!("{selector}-disclosure");
        let mut collapsible = Collapsible::new(
            SharedString::from(disclosure_selector.clone()),
            self.disclosure_focus.clone(),
            open,
            trigger,
            content,
        )
        .disabled(disabled)
        .force_mount(disabled)
        .debug_selector(disclosure_selector);

        if !disabled {
            let surface = entity.downgrade();
            let action_id = id.clone();
            collapsible = collapsible.on_change(move |requested_open, _, _, app| {
                let action = ConversationSurfaceAction::DisclosureToggleRequested {
                    id: action_id.clone(),
                    requested_open,
                };
                let _ = surface.update(app, |surface, cx| {
                    if surface.enqueue_action(action) {
                        cx.notify();
                    }
                });
            });
        }

        let card = compact_card(style).w_full();
        let card = anchors.attach(card, Some(&id), item_id.as_ref());
        let card = card.debug_selector(move || selector.clone());
        card.child(collapsible).into_any_element()
    }
}

/// Stable debug-selector suffix for the approval confirm control.
pub const APPROVAL_CONFIRM_SELECTOR_SUFFIX: &str = "approve-submit";

/// Stable debug-selector suffix for the approval deny control.
pub const APPROVAL_DENY_SELECTOR_SUFFIX: &str = "deny-submit";

/// Stable debug-selector suffix for the question answer control.
pub const QUESTION_ANSWER_SELECTOR_SUFFIX: &str = "question-answer";

/// Stable debug-selector suffix for one question option control; the option
/// index is appended after a `-` separator.
pub const QUESTION_OPTION_SELECTOR_SUFFIX: &str = "question-option";

/// Stable debug-selector suffix for the question failure row.
pub const QUESTION_FAILURE_SELECTOR_SUFFIX: &str = "question-failure";

/// Stable debug-selector suffix for the approval failure row.
pub const APPROVAL_FAILURE_SELECTOR_SUFFIX: &str = "approval-failure";

/// Which existing GPUI answer action a dispatched attempt carries.
///
/// No new actions are introduced: this enum only retains the already
/// registered [`RespondApprovalAction`]/[`RespondQuestionAction`] value that
/// a button gesture dispatched, alongside the domain command built for it.
#[derive(Clone, Debug, PartialEq)]
pub enum AnswerDispatchAction {
    /// One explicit approval gesture (`approved` is always stated).
    Approval(RespondApprovalAction),
    /// One explicit question gesture with the chosen answers.
    Question(RespondQuestionAction),
}

impl AnswerDispatchAction {
    /// Returns the stable action name shared with the GPUI contract.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Approval(_) => "respond_approval",
            Self::Question(_) => "respond_question",
        }
    }
}

/// One button gesture dispatched toward the existing transport/request path.
///
/// The command already carries its freshly minted request identity; the
/// controller drains [`ConversationSurface::take_answer_dispatches`] toward
/// transport. App-level `dispatch_action` binding for the carried GPUI action
/// value lands with the controller transport packet; until then the outbox
/// carries the exact action values the buttons dispatched.
#[derive(Debug)]
pub struct AnswerDispatch {
    /// The existing GPUI answer action value the gesture dispatched.
    pub action: AnswerDispatchAction,
    /// The domain command built through the existing constructors.
    pub command: Command,
    /// The freshly minted request identity carried by the command.
    pub request_id: RequestId,
}

/// Offered question options cached per row for choice rendering.
///
/// The full option views remain in [`EngineObservationState`]; this cache
/// carries only the labels (plus optional descriptions) the controller
/// mirrors for the choice buttons, with the multi-select policy.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestionChoiceCache {
    /// Whether more than one option may be chosen before confirming.
    pub multi_select: bool,
    /// Offered answers in provider order as `(label, description)` pairs.
    pub options: Vec<(String, Option<String>)>,
}

impl QuestionChoiceCache {
    /// Returns whether the row offers choices rather than free-form entry.
    #[must_use]
    pub fn is_choice(&self) -> bool {
        !self.options.is_empty()
    }
}

/// One approval answer attempt admitted by [`ApprovalAnswerGate`].
///
/// The attempt carries the dispatched GPUI action value, the domain command
/// built with a freshly minted request identity, and that identity for
/// receipt correlation. At most one attempt exists per row while its flight
/// is outstanding.
#[derive(Debug)]
pub struct ApprovalAnswerAttempt {
    /// The dispatched `respond_approval` action value.
    pub action: RespondApprovalAction,
    /// The domain command built through [`approval_command`].
    pub command: Command,
    /// The freshly minted request identity carried by the command.
    pub request_id: RequestId,
}

/// Single-flight gate plus pending/failure presentation for one approval row.
///
/// Mirrors `submitted_decision` in `conversation-approval.svelte`: at most
/// one answer attempt is in flight per row, so a second gesture cannot mint
/// a second request identity while the first awaits its receipt. A settled
/// attempt keeps the gate closed until the row resolves in place through the
/// subscription; a failed attempt reopens the gate and surfaces the retry
/// message from the existing pairing policy.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ApprovalAnswerGate {
    flight: AnswerFlight,
    pending_decision: Option<bool>,
    failure: Option<String>,
    last_request_id: Option<RequestId>,
}

impl ApprovalAnswerGate {
    /// Creates a gate with no answer in flight.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns whether an answer attempt is awaiting its receipt.
    #[must_use]
    pub fn is_in_flight(&self) -> bool {
        self.flight.is_in_flight()
    }

    /// Returns the submitted decision awaiting settlement, if any.
    #[must_use]
    pub const fn pending_decision(&self) -> Option<bool> {
        self.pending_decision
    }

    /// Returns the surfaced retry/diagnostic message, if any.
    #[must_use]
    pub fn failure_message(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// Returns the request identity of the latest admitted attempt, if any.
    #[must_use]
    pub fn last_request_id(&self) -> Option<&RequestId> {
        self.last_request_id.as_ref()
    }

    /// Attempts one explicit approval gesture.
    ///
    /// This is one authenticated user gesture: `approved` is always stated,
    /// never defaulted. A refused attempt (flight outstanding or identity
    /// exhaustion) mints nothing and dispatches nothing.
    pub fn begin(
        &mut self,
        thread_id: ThreadId,
        run_id: RunId,
        approval_id: ObservationId,
        approved: bool,
    ) -> Option<ApprovalAnswerAttempt> {
        if !self.flight.begin() {
            return None;
        }
        let request_id = match mint_answer_request_id() {
            Ok(request_id) => request_id,
            Err(_) => {
                self.flight.settle();
                return None;
            }
        };
        let action = RespondApprovalAction {
            run_id,
            approval_id,
            approved,
        };
        let command = approval_command(thread_id, &action, request_id.clone());
        self.pending_decision = Some(approved);
        self.failure = None;
        self.last_request_id = Some(request_id.clone());
        Some(ApprovalAnswerAttempt {
            action,
            command,
            request_id,
        })
    }

    /// Pairs one correlated approval receipt onto the existing policy.
    ///
    /// No new pairing logic is introduced: this delegates to
    /// [`pair_approval_answer`]. A settled attempt keeps the gate closed
    /// until the row resolves in place; any other outcome reopens the gate
    /// and surfaces the renderer-safe message.
    pub fn settle_receipt(
        &mut self,
        state: &EngineObservationState,
        command: &artisan_domain::RespondApproval,
        receipt: &RespondApprovalReceipt,
    ) -> AnswerPairing {
        let pairing = pair_approval_answer(state, command, receipt);
        if pairing.is_settled() {
            self.failure = None;
        } else {
            self.flight.settle();
            self.pending_decision = None;
            self.failure = pairing.settlement.message().map(str::to_owned);
        }
        pairing
    }

    /// Pairs one answer failure onto the existing policy.
    ///
    /// The retry message comes from [`pair_answer_failure`]; the gate
    /// reopens so the same answer may be retried explicitly with a freshly
    /// minted identity.
    pub fn settle_failure(
        &mut self,
        request_id: &RequestId,
        failure: &ProtocolFailure,
    ) -> AnswerSettlement {
        let settlement = pair_answer_failure(AnswerKind::Approval, request_id, failure);
        self.flight.settle();
        self.pending_decision = None;
        self.failure = settlement.message().map(str::to_owned);
        settlement
    }
}

/// What one question-row keystroke did.
///
/// Returned by [`ConversationSurface::handle_question_key`] so renderers can
/// route focus without re-deriving the decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuestionKeyOutcome {
    /// The key changed nothing: wrong row kind, outstanding flight,
    /// modified shortcut, unhandled key, or a refused intake/submit.
    Ignored,
    /// Typed or deleted text staged into the row draft.
    Edited,
    /// `enter` dispatched the staged draft through the existing question
    /// submit gesture with a fresh request id.
    Submitted,
    /// `escape` asks the renderer to return focus to the transcript without
    /// submitting.
    FocusTranscript,
}

/// Shapes raw staged text into the single-line free-form draft form.
///
/// Canonicalizes through the shared input intake, then strips newlines for
/// legacy single-line `Input` parity (the browser input drops them; Enter
/// submits instead of inserting a break here).
fn shape_freeform_draft(text: &str) -> String {
    artisan_ui::input_state::normalize_input(text)
        .chars()
        .filter(|character| *character != '\n')
        .collect()
}

/// One question answer attempt admitted by [`QuestionAnswerGate`].
///
/// Carries the dispatched `respond_question` action value, the domain command
/// built with a freshly minted request identity, and that identity for
/// receipt correlation.
#[derive(Debug)]
pub struct QuestionAnswerAttempt {
    /// The dispatched `respond_question` action value.
    pub action: RespondQuestionAction,
    /// The domain command built through [`question_command`].
    pub command: Command,
    /// The freshly minted request identity carried by the command.
    pub request_id: RequestId,
}

/// Single-flight gate plus selection/draft/failure state for one question row.
///
/// Mirrors `conversation-prompt.svelte`: a single-select choice submits the
/// moment it is clicked, while a multi-select choice stages a selection until
/// the Answer control confirms it. Free-form rows submit the typed draft;
/// empty drafts are rejected client-side with the row staying pending, so
/// nothing is minted or dispatched. A settled attempt keeps the gate closed
/// until the row resolves; a failed attempt reopens it with the retry message
/// from the existing pairing policy.
///
/// The free-form draft is backed by [`TextInputState`](artisan_ui::input_state::TextInputState):
/// keystroke intake is canonicalized on entry (zero-width spaces removed,
/// CR/CRLF folded) exactly like the shared input seam, while the
/// single-line caller policy additionally strips newlines (legacy `Input`
/// parity: the browser single-line input drops them) and clamps the buffer
/// to [`OBSERVATION_ANSWER_MAX_BYTES`] UTF-8 bytes, the same bound the
/// answer validation enforces at submit. `TextInputState` carries no
/// `PartialEq`, so this gate deliberately omits it; no caller compares
/// gates.
#[derive(Clone, Debug, Default)]
pub struct QuestionAnswerGate {
    flight: AnswerFlight,
    selected: Vec<String>,
    draft: TextInputState,
    failure: Option<String>,
    last_request_id: Option<RequestId>,
}

impl QuestionAnswerGate {
    /// Creates a gate with no selection, draft, or flight.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns whether an answer attempt is awaiting its receipt.
    #[must_use]
    pub fn is_in_flight(&self) -> bool {
        self.flight.is_in_flight()
    }

    /// Returns the staged multi-select choices in selection order.
    #[must_use]
    pub fn selected(&self) -> &[String] {
        &self.selected
    }

    /// Returns the staged free-form draft in canonical input form.
    #[must_use]
    pub fn draft(&self) -> &str {
        self.draft.value()
    }

    /// Returns the surfaced retry/diagnostic message, if any.
    #[must_use]
    pub fn failure_message(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// Returns the request identity of the latest admitted attempt, if any.
    #[must_use]
    pub fn last_request_id(&self) -> Option<&RequestId> {
        self.last_request_id.as_ref()
    }

    /// Stages one multi-select choice toggle without dispatching.
    ///
    /// Single-select rows never stage: they submit immediately through
    /// [`Self::submit_single`].
    pub fn toggle_option(&mut self, option: String, multi_select: bool) {
        if !multi_select {
            return;
        }
        if let Some(position) = self.selected.iter().position(|known| known == &option) {
            self.selected.remove(position);
        } else {
            self.selected.push(option);
        }
    }

    /// Replaces the staged free-form draft exactly as supplied.
    ///
    /// The text passes through the single-line shaping (canonical input
    /// form, newlines stripped); the byte bound is enforced at keystroke
    /// intake and at submit validation, not here, so controller-staged
    /// drafts arrive intact for the existing submit path to judge.
    pub fn set_draft(&mut self, draft: String) {
        self.draft.set_value(&shape_freeform_draft(&draft));
    }

    /// Appends keystroke text to the staged free-form draft.
    ///
    /// Returns whether the draft changed: empty text and appends that would
    /// exceed [`OBSERVATION_ANSWER_MAX_BYTES`] UTF-8 bytes are refused with
    /// the buffer untouched, so the draft stays within the bound the answer
    /// validation enforces.
    #[must_use]
    pub fn insert_text(&mut self, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        let mut next = self.draft.value().to_owned();
        next.push_str(text);
        let next = shape_freeform_draft(&next);
        if next.len() > OBSERVATION_ANSWER_MAX_BYTES {
            return false;
        }
        self.draft.set_value(&next)
    }

    /// Deletes the last character of the staged free-form draft.
    ///
    /// Returns whether the draft changed; an already-empty draft reports
    /// `false`. There is no caret or selection model, matching the shared
    /// input seam limits.
    #[must_use]
    pub fn delete_backward(&mut self) -> bool {
        let mut next = self.draft.value().to_owned();
        if next.pop().is_none() {
            return false;
        }
        self.draft.set_value(&next);
        true
    }

    /// Submits one single-select choice immediately.
    ///
    /// A refused attempt (flight outstanding or identity/bounds failure)
    /// mints nothing and dispatches nothing.
    pub fn submit_single(
        &mut self,
        thread_id: ThreadId,
        run_id: RunId,
        question_id: ObservationId,
        option: String,
    ) -> Option<QuestionAnswerAttempt> {
        self.submit_answers(thread_id, run_id, question_id, vec![option])
    }

    /// Submits the staged multi-select choices.
    ///
    /// An empty selection is rejected client-side: the row stays pending and
    /// nothing is minted or dispatched.
    pub fn submit_selected(
        &mut self,
        thread_id: ThreadId,
        run_id: RunId,
        question_id: ObservationId,
    ) -> Option<QuestionAnswerAttempt> {
        let answers = self.selected.clone();
        self.submit_answers(thread_id, run_id, question_id, answers)
    }

    /// Submits an explicit choice list without touching staged state.
    ///
    /// An empty list is rejected client-side, mirroring the legacy guard
    /// that never synthesizes an answer; the empty list remains reserved for
    /// an explicit skip gesture, which has no button on this surface.
    pub fn submit_choice(
        &mut self,
        thread_id: ThreadId,
        run_id: RunId,
        question_id: ObservationId,
        answers: Vec<String>,
    ) -> Option<QuestionAnswerAttempt> {
        self.submit_answers(thread_id, run_id, question_id, answers)
    }

    /// Submits the staged free-form draft.
    ///
    /// The draft is trimmed and empty submits are rejected client-side with
    /// the row staying pending: no identity is minted and nothing is
    /// dispatched. A submitted draft clears the staged draft and selection,
    /// mirroring the legacy surface.
    pub fn submit_freeform(
        &mut self,
        thread_id: ThreadId,
        run_id: RunId,
        question_id: ObservationId,
    ) -> Option<QuestionAnswerAttempt> {
        let trimmed = self.draft.value().trim().to_owned();
        if trimmed.is_empty() {
            return None;
        }
        let attempt = self.submit_answers(thread_id, run_id, question_id, vec![trimmed])?;
        self.draft.set_value("");
        self.selected.clear();
        Some(attempt)
    }

    fn submit_answers(
        &mut self,
        thread_id: ThreadId,
        run_id: RunId,
        question_id: ObservationId,
        answers: Vec<String>,
    ) -> Option<QuestionAnswerAttempt> {
        if answers.is_empty() {
            return None;
        }
        if !self.flight.begin() {
            return None;
        }
        let request_id = match mint_answer_request_id() {
            Ok(request_id) => request_id,
            Err(_) => {
                self.flight.settle();
                return None;
            }
        };
        let action = RespondQuestionAction {
            run_id,
            question_id,
            answers,
        };
        let command = match question_command(thread_id, &action, request_id.clone()) {
            Ok(command) => command,
            Err(error) => {
                self.flight.settle();
                self.failure = Some(format!("{}{error}", AnswerKind::Question.failure_prefix()));
                return None;
            }
        };
        self.failure = None;
        self.last_request_id = Some(request_id.clone());
        Some(QuestionAnswerAttempt {
            action,
            command,
            request_id,
        })
    }

    /// Pairs one correlated question receipt onto the existing policy.
    ///
    /// No new pairing logic is introduced: this delegates to
    /// [`pair_question_answer`]. A settled attempt keeps the gate closed
    /// until the row resolves in place; any other outcome reopens the gate
    /// and surfaces the renderer-safe message.
    pub fn settle_receipt(
        &mut self,
        state: &EngineObservationState,
        command: &artisan_domain::RespondQuestion,
        receipt: &RespondQuestionReceipt,
    ) -> AnswerPairing {
        let pairing = pair_question_answer(state, command, receipt);
        if pairing.is_settled() {
            self.failure = None;
            self.draft.set_value("");
            self.selected.clear();
        } else {
            self.flight.settle();
            self.failure = pairing.settlement.message().map(str::to_owned);
        }
        pairing
    }

    /// Pairs one answer failure onto the existing policy.
    ///
    /// The retry message comes from [`pair_answer_failure`]; the gate
    /// reopens so the same answer may be retried explicitly with a freshly
    /// minted identity.
    pub fn settle_failure(
        &mut self,
        request_id: &RequestId,
        failure: &ProtocolFailure,
    ) -> AnswerSettlement {
        let settlement = pair_answer_failure(AnswerKind::Question, request_id, failure);
        self.flight.settle();
        self.failure = settlement.message().map(str::to_owned);
        settlement
    }
}

/// One answer dispatch that a drain attempt could not hand to transport.
///
/// The dispatch is re-queued in the surface outbox so nothing is silently
/// dropped. Its row stays pending (the gate remains in flight until a receipt
/// pairs through the existing settle-in-place pairing) and `message` carries
/// the existing retry/diagnostic text from [`pair_answer_failure`].
#[derive(Clone, Debug, PartialEq)]
pub struct FailedAnswerDispatch {
    /// The already-minted request identity of the unsent dispatch.
    pub request_id: RequestId,
    /// The existing retry/diagnostic message for the send failure.
    pub message: String,
}

/// Finite report of one outbox drain call.
///
/// Each taken dispatch is accounted exactly once: either sent or re-queued
/// with its retry message. A second drain over an emptied outbox reports
/// zeros without touching the send path.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnswerDrainReport {
    /// Dispatches handed to the send path in this call.
    pub sent: usize,
    /// Dispatches the send path refused, re-queued with retry messages.
    pub failed: Vec<FailedAnswerDispatch>,
}

impl AnswerDrainReport {
    /// Returns whether the drain call moved every taken dispatch.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.failed.is_empty()
    }
}

/// Maps one mirrored send-path failure onto the existing answer retry policy.
///
/// [`CommandSendError::Busy`] (bounded queue full, mirroring the composer
/// backpressure path) becomes a retryable failure so the report carries the
/// existing "retry the same answer" text; [`CommandSendError::Stopped`]
/// (service gone) becomes a terminal failure so the report carries the
/// existing diagnostic text. No new pairing logic is introduced: the message
/// always comes from [`pair_answer_failure`].
fn answer_send_failure(
    kind: AnswerKind,
    request_id: &RequestId,
    error: CommandSendError,
) -> ProtocolFailure {
    let (detail, retryable) = match error {
        CommandSendError::Busy => ("answer transport is busy", true),
        CommandSendError::Stopped => ("answer transport has stopped", false),
    };
    ProtocolFailure {
        code: ErrorCode::Internal,
        detail: ErrorDetail::parse(detail).expect("static answer send detail is valid"),
        retryable,
        request_id: Some(request_id.clone()),
    }
}

/// Maps one taken answer dispatch onto its live transport command.
///
/// The domain command moves across unchanged with its already-minted request
/// id (cloned once for the by-value transport wrapper, exactly like the
/// `StopRun` arm clones; never re-minted). The outbox carries only
/// gate-built answer commands, so any other command is unreachable.
fn answer_transport_command(dispatch: &AnswerDispatch) -> NativeTransportCommand {
    match &dispatch.command {
        Command::RespondApproval(answer) => {
            NativeTransportCommand::RespondApproval(Box::new(answer.clone()))
        }
        Command::RespondQuestion(answer) => {
            NativeTransportCommand::RespondQuestion(Box::new(answer.clone()))
        }
        _ => unreachable!("the answer outbox carries only answer commands"),
    }
}

/// Drains one taken answer queue through the transport submit path.
///
/// Every dispatch is mapped through [`answer_transport_command`] and handed
/// to `submit` at most once, in FIFO order; the queue itself is consumed,
/// never re-taken or cloned. Refused dispatches are returned for re-queue
/// with the existing retry message from [`pair_answer_failure`]; an empty
/// queue never touches `submit`. Row gates are never settled here:
/// single-flight is preserved until resolutions pair through the existing
/// receipt path.
pub fn drain_answer_queue(
    queue: Vec<AnswerDispatch>,
    submit: &mut impl FnMut(NativeTransportCommand) -> Result<(), CommandSendError>,
) -> (Vec<AnswerDispatch>, AnswerDrainReport) {
    let mut requeue = Vec::with_capacity(queue.len());
    let mut report = AnswerDrainReport::default();
    for dispatch in queue {
        match submit(answer_transport_command(&dispatch)) {
            Ok(()) => {
                report.sent = report.sent.saturating_add(1);
            }
            Err(error) => {
                let kind = match dispatch.action {
                    AnswerDispatchAction::Approval(_) => AnswerKind::Approval,
                    AnswerDispatchAction::Question(_) => AnswerKind::Question,
                };
                let failure = answer_send_failure(kind, &dispatch.request_id, error);
                let settlement = pair_answer_failure(kind, &dispatch.request_id, &failure);
                let message = settlement
                    .message()
                    .expect("send failures never settle")
                    .to_owned();
                report.failed.push(FailedAnswerDispatch {
                    request_id: dispatch.request_id.clone(),
                    message,
                });
                requeue.push(dispatch);
            }
        }
    }
    (requeue, report)
}

impl Render for ConversationSurface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ArtisanTheme::for_mode(self.theme_mode);
        let entity = cx.entity();
        // Pure read of the live reduced-motion signal: no state writes, so
        // no notification can loop out of render.
        let status_motion = effective_status_motion(self.status_motion, cx.reduce_motion());
        // End-space height lives in framework window-local state, not on the
        // entity: two windows showing one surface measure different
        // viewports, and a shared scalar could never converge for both.
        let end_space = window.use_state(cx, |_, _| TRANSCRIPT_END_SPACE_PX);
        let end_space_px = *end_space.read(cx);
        // The reader's current navigator marker is geometry-derived like
        // end space, so it lives in the same window-local state: two
        // windows sharing one surface converge independently.
        let navigator_active = window.use_state(cx, |_, _| None::<String>);
        // Reference rhythm keeps 32 px (`gap-8`) between turn groups; the
        // settled footer's absolute reveal lives inside that room instead of
        // overlapping the next turn. No per-turn pad is added, so unsettled
        // turns carry no phantom gap.
        let wheel_surface = entity.downgrade();
        let mut transcript = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(8.0))
            .pt(px(TRANSCRIPT_PAD_TOP_PX))
            .on_scroll_wheel(move |event, window, cx| {
                let _ = wheel_surface.update(cx, |surface, surface_cx| {
                    surface.handle_transcript_wheel(event, window, surface_cx);
                });
            });
        let previous_anchors = std::mem::take(&mut self.scroll_anchors);
        let mut rendered_anchors = Vec::new();
        self.sync_footer_focus(&mut *window, cx);
        {
            let mut anchors = ScrollAnchorRegistry {
                handle: &self.scroll_handle,
                previous: &previous_anchors,
                next_element_id: 0,
                rendered: &mut rendered_anchors,
            };
            for turn in self.scene.turn_scenes() {
                transcript = transcript.child(self.render_turn(
                    turn,
                    &entity,
                    &theme,
                    &mut anchors,
                    &mut *window,
                    status_motion,
                    cx,
                ));
            }
            // Base end space keeps scrollable room after the last turn so an
            // anchored turn can reach the viewport top inset. The height
            // grows from live measurement via [`end_space_height`] once the
            // viewport lane supplies it; until then the reference base holds
            // with no invented cap.
            transcript = transcript.child(
                div()
                    .w_full()
                    .h(px(end_space_px))
                    .debug_selector(|| TRANSCRIPT_END_SPACE_SELECTOR.to_owned()),
            );
        }

        let scroll_executed = self.drain_painted_scroll_targets(&rendered_anchors, window, cx);
        self.scroll_anchors = rendered_anchors;
        if scroll_executed {
            // `ScrollAnchor::scroll_to` defers the offset write to a
            // `window.on_next_frame` callback, which only runs when another
            // frame is actually drawn. This render just consumed the dirty
            // flag, so request the next frame explicitly; otherwise the
            // callback pends forever on an idle window and the viewport
            // never moves despite holding painted custody.
            cx.notify();
        }

        let paint_token = Rc::new(());
        self.scroll_anchor_paint_token = Some(paint_token.clone());
        let surface = entity.downgrade();
        let end_space_state = end_space.clone();
        let navigator_active_state = navigator_active.clone();
        // Painted custody comes from the real prepaint boundary. GPUI writes
        // each retained anchor origin during Div prepaint, and this listener
        // runs after the transcript children are prepainted. A defer marker
        // is not paint evidence and must never mint painted custody.
        transcript = transcript.on_children_prepainted(move |children_bounds, window, app| {
            let _ = surface.update(app, |surface, cx| {
                let current = surface
                    .scroll_anchor_paint_token
                    .as_ref()
                    .is_some_and(|current| Rc::ptr_eq(current, &paint_token));
                if !current {
                    return;
                }

                // End-space measurement is window-local state, written only
                // by the current frame: two windows sharing one surface
                // converge independently, and an unchanged value never
                // notifies, so neither window can loop the other.
                let viewport_height =
                    f64::from(surface.scroll_handle.bounds().size.height);
                let offset = f64::from(window.element_offset().y);
                if let Some(measured) =
                    ConversationSurface::measured_end_space_height(
                        &children_bounds,
                        viewport_height,
                        offset,
                    )
                {
                    let changed = end_space_state.update(cx, |value, _| {
                        let changed = *value != measured;
                        *value = measured;
                        changed
                    });
                    if changed {
                        cx.notify();
                    }
                }

                // Reader-position tracking rides the same prepaint boundary
                // into the same window-local state discipline: per-window
                // geometry in, change-guarded notify out.
                let active = ConversationSurface::navigator_active_for_geometry(
                    &surface.scene,
                    &surface.scroll_handle,
                    &children_bounds,
                    window,
                );
                let active_changed = navigator_active_state.update(cx, |value, _| {
                    let changed = *value != active;
                    *value = active;
                    changed
                });
                if active_changed {
                    cx.notify();
                }

                let mut newly_painted = false;
                for anchor in &mut surface.scroll_anchors {
                    if !anchor.painted {
                        anchor.painted = true;
                        newly_painted = true;
                    }
                }
                if newly_painted && !surface.pending_scroll_targets.is_empty() {
                    cx.notify();
                }
                // Turn roots are the transcript's direct children in scene
                // order, so executed turn targets resolve here exactly like
                // block and item targets resolve one level down.
                if !surface.executed_scroll_targets.is_empty() {
                    let turn_identities = surface.turn_scroll_identities();
                    surface.apply_executed_scroll_targets(
                        &turn_identities,
                        &children_bounds,
                        window,
                    );
                }
            });
        });

        // Deferred change cards intentionally have no transcript position in
        // the scene contract. Their aggregate owner supplies placement later;
        // rendering only turn blocks keeps this surface an exhaustive view of
        // the accepted ordered block tree.
        self.schedule_viewport_observation(window, cx);

        // The viewport inherits the thread-screen shell (black): no opaque fill
        // here, so the transcript never paints over its parent. Cards,
        // bubbles, panels, and popovers keep their own faces.
        let scroll_area = ScrollArea::new(self.scroll_handle.clone(), theme)
            .focus_handle(self.transcript_focus.clone())
            .debug_selector(CONVERSATION_SURFACE_SELECTOR)
            .size_full()
            .child(transcript);

        let mut root = div().relative().size_full().child(scroll_area);
        if self.jump_to_latest_visible {
            let surface = entity.downgrade();
            let button = Button::new(
                JUMP_TO_LATEST_SELECTOR,
                self.jump_to_latest_focus.clone(),
                theme,
                MotionPolicy::Reduced,
                ButtonVariant::Ghost,
                ButtonSize::Small,
                ButtonContent::text("Jump to latest"),
            )
            .expect("static jump-to-latest button configuration is valid")
            .focus_visibility(FocusVisibility::Visible)
            .debug_selector(JUMP_TO_LATEST_SELECTOR)
            .on_activate(move |_, _, app| {
                let _ = surface.update(app, |surface, cx| {
                    if surface.enqueue_action(ConversationSurfaceAction::JumpToLatestRequested) {
                        cx.notify();
                    }
                });
            });
            root = root.child(
                div()
                    .absolute()
                    .right(px(16.0))
                    .bottom(px(16.0))
                    .child(button),
            );
        }
        if let Some(rail) = self.render_turn_navigator(
            &entity,
            &theme,
            window,
            cx,
            navigator_active.read(cx).clone(),
        ) {
            // Deferred overlay at dropdown priority: the rail paints above
            // the composer dock exactly like the reference `z-30`, while its
            // layout stays in this tree so measurement and hit-testing are
            // unaffected.
            root = root.child(deferred(rail).with_priority(2));
        }
        self.flush_rich_link_requests(cx);
        root
    }
}

/// One paintable transcript detail row with its durable ordinal.
///
/// Session mode carries the single ordered `session_details` list
/// (assistant prose, activities, compactions, native facts); legacy
/// positional groups carry `items` in vec order, which is durable by
/// construction. The builder guarantees never both, so no interleave can
/// scramble chronology. Reasoning maps to nothing: it is stripped from
/// visible details unconditionally and lives only on the thinking line.
#[derive(Clone, Copy, Debug)]
enum DetailRow<'a> {
    /// Assistant prose that is not the promoted reply.
    Assistant { id: &'a SceneId, body: &'a str },
    /// Activity or tool-result summary.
    Activity {
        id: &'a SceneId,
        body: &'a str,
        /// Provider activity kind, when the source row carried one.
        kind: Option<&'a str>,
        /// Raw provider detail, when disclosed.
        detail: Option<&'a str>,
        /// Durable liveness for the chain's live/failed default-open rule.
        lifecycle: Option<ConversationLifecycle>,
    },
    /// Compaction summary folded into the session.
    Compaction { id: &'a SceneId, summary: &'a str },
    /// Native fact folded into the session.
    NativeFact { id: &'a SceneId, text: &'a str },
    /// Legacy work-session title (fixture-only in production: no shipped
    /// producer emits session titles; the variant stays matched).
    SessionTitle { id: &'a SceneId, title: &'a str },
}

impl<'a> DetailRow<'a> {
    /// Returns the stable scene identity carried for scroll anchoring.
    fn scene_id(&self) -> &SceneId {
        match self {
            Self::Assistant { id, .. }
            | Self::Activity { id, .. }
            | Self::Compaction { id, .. }
            | Self::NativeFact { id, .. }
            | Self::SessionTitle { id, .. } => id,
        }
    }
}

/// Collects one group's paintable rows in exact chronological order.
///
/// Session details arrive ordinal-keyed and sort stably; legacy items keep
/// vec order. Callers paint the returned sequence verbatim.
fn ordered_detail_rows(block: &WorkGroupBlock) -> Vec<(u64, DetailRow<'_>)> {
    if !block.session_details.is_empty() {
        let mut rows: Vec<(u64, DetailRow<'_>)> = block
            .session_details
            .iter()
            .map(|detail| match detail {
                SessionDetail::Assistant { id, body, ordinal, .. } => (
                    *ordinal,
                    DetailRow::Assistant {
                        id,
                        body: body.as_str(),
                    },
                ),
                SessionDetail::Activity {
                    id,
                    body,
                    kind,
                    detail,
                    lifecycle,
                    ordinal,
                    ..
                } => (
                    *ordinal,
                    DetailRow::Activity {
                        id,
                        body: body.as_str(),
                        kind: kind.as_deref(),
                        detail: detail.as_deref(),
                        lifecycle: *lifecycle,
                    },
                ),
                SessionDetail::Compaction {
                    id, summary, ordinal, ..
                } => (
                    *ordinal,
                    DetailRow::Compaction {
                        id,
                        summary: summary.as_str(),
                    },
                ),
                SessionDetail::NativeFact {
                    id, text, ordinal, ..
                } => (
                    *ordinal,
                    DetailRow::NativeFact {
                        id,
                        text: text.as_str(),
                    },
                ),
            })
            .collect();
        rows.sort_by_key(|(ordinal, _)| *ordinal);
        rows
    } else {
        block
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                let ordinal = u64::try_from(index).unwrap_or(u64::MAX);
                match item {
                    WorkItem::Reasoning { .. } => None,
                    WorkItem::Activity {
                        body,
                        kind,
                        detail,
                        lifecycle,
                        ..
                    } => Some((
                        ordinal,
                        DetailRow::Activity {
                            id: work_item_id(item),
                            body: body.as_str(),
                            kind: kind.as_deref(),
                            detail: detail.as_deref(),
                            lifecycle: *lifecycle,
                        },
                    )),
                    WorkItem::WorkSession { title, .. } => Some((
                        ordinal,
                        DetailRow::SessionTitle {
                            id: work_item_id(item),
                            title: title.as_str(),
                        },
                    )),
                }
            })
            .collect()
    }
}

impl ConversationSurface {
    /// Prunes navigator focus handles and paints the loaded-turn rail.
    ///
    /// Pruning runs on every render, even when the replacement scene has no
    /// markers at all, so a focused control that disappears returns focus to
    /// the transcript. The rail itself paints only for two or more loaded
    /// user-message markers. Emitting a scroll intent never touches the GPUI
    /// handle or scene directly.
    /// Paints the shared hover pill for navigator rows.
    ///
    /// Row probes measure into the picker's [`SlidingHoverState`]; the pill
    /// paints the retained flight behind the rows and settles interrupted
    /// flights from the currently displayed rectangle, exactly like the
    /// model picker surface. Reduced motion jumps to the target.
    fn render_navigator_hover_pill(&self, theme: &ArtisanTheme, reduce_motion: bool) -> AnyElement {
        let (mut rect, visible, transition) = {
            let hover = self.navigator_hover.borrow();
            (hover.visual_rect(), hover.visible(), hover.transition())
        };
        if reduce_motion {
            if let Some(transition) = transition {
                rect = transition.to;
                self.navigator_hover
                    .borrow_mut()
                    .apply_progress(transition.generation, 1.0);
            }
        }
        let pill = div()
            .id(SharedString::from(TURN_NAVIGATOR_HOVER_SELECTOR))
            .absolute()
            .left(px(rect.left))
            .top(px(rect.top))
            .w(px(rect.width))
            .h(px(rect.height))
            .rounded(RadiusTokens::value(RadiusStep::Lg))
            .bg(hover_fill_gradient(*theme))
            .opacity(if visible { 1.0 } else { 0.0 });
        if !reduce_motion {
            if let Some(transition) = transition {
                let motion = Rc::clone(&self.navigator_hover);
                let from = transition.from;
                let to = transition.to;
                let generation = transition.generation;
                let animation_id = ElementId::Name(SharedString::from(format!(
                    "{TURN_NAVIGATOR_HOVER_SELECTOR}-{generation}"
                )));
                return pill
                    .with_animation(
                        animation_id,
                        Animation::new(Duration::from_millis(250))
                            .with_easing(navigator_smooth_out),
                        move |pill, progress| {
                            let rect = from.lerp(to, progress);
                            motion.borrow_mut().apply_progress(generation, progress);
                            pill.left(px(rect.left))
                                .top(px(rect.top))
                                .w(px(rect.width))
                                .h(px(rect.height))
                        },
                    )
                    .into_any_element();
            }
        }
        pill.into_any_element()
    }

    fn render_turn_navigator(
        &mut self,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
        navigator_active: Option<String>,
    ) -> Option<AnyElement> {
        let markers = loaded_turn_navigator_markers(&self.scene);
        // Window-local rail geometry, measured live: two windows sharing one
        // surface center independently, exactly like end-space height.
        let navigator_metrics = window.use_state(cx, |_, _| TurnNavigatorMetrics::default());
        let metrics = *navigator_metrics.read(cx);
        // Prune focus handles whose targets left the scene on every render,
        // even when the replacement scene has no markers at all. A focused
        // control that disappears returns focus to the transcript.
        let live: Vec<String> = markers
            .iter()
            .map(|marker| navigator_focus_key(&marker.target))
            .collect();
        let stale: Vec<String> = self
            .navigator_focus
            .keys()
            .filter(|key| !live.contains(*key))
            .cloned()
            .collect();
        for key in stale {
            if let Some(handle) = self.navigator_focus.remove(&key)
                && handle.is_focused(window)
            {
                self.transcript_focus.focus(window, cx);
            }
        }
        if markers.is_empty() {
            self.navigator_expanded = false;
            return None;
        }
        // Handles are ensured before any focus observation below, so a
        // keyboard arrival can expand the rail into labels exactly like rail
        // hover does in the reference (`group-focus-within`).
        for marker in &markers {
            let key = navigator_focus_key(&marker.target);
            self.navigator_focus
                .entry(key)
                .or_insert_with(|| cx.focus_handle().tab_stop(true));
        }
        let focus_expanded = markers.iter().any(|marker| {
            self.navigator_focus
                .get(&navigator_focus_key(&marker.target))
                .is_some_and(|handle| handle.is_focused(window))
        });
        let expanded = self.navigator_expanded || focus_expanded;
        // --inspector-width: clamp(16rem, 25vw, 350px) (theme.css:176): the
        // expanded panel matches the inspector facing it across the
        // transcript, capped to the card actually available.
        let card_width = f64::from(self.scroll_handle.bounds().size.width);
        let mut inspector_width =
            (f64::from(window.bounds().size.width) * 0.25).clamp(256.0, 350.0);
        if card_width > 0.0 {
            inspector_width = inspector_width.min(card_width);
        }
        let inspector_width = inspector_width as f32;
        // Keyboard arrival selects the pill exactly like pointer hover
        // does (`onfocusin`); the guard selects only on arrival so a later
        // trigger-zone clear survives while the row keeps focus. Render is
        // already painting, so neither path notifies.
        if let Some(focused) = markers.iter().find(|marker| {
            self.navigator_focus
                .get(&navigator_focus_key(&marker.target))
                .is_some_and(|handle| handle.is_focused(window))
        }) {
            let slug = navigator_target_slug(&focused.target).to_owned();
            if self.navigator_focused_key.as_deref() != Some(slug.as_str()) {
                self.navigator_focused_key = Some(slug.clone());
                self.navigator_hover.borrow_mut().set_active(slug);
            }
        } else {
            self.navigator_focused_key = None;
            if !expanded && self.navigator_hover.borrow().visible() {
                self.navigator_hover.borrow_mut().hide();
            }
        }
        // The lit marker is the turn the reader is at, tracked from painted
        // geometry against the reference 96 px threshold — never the tail.
        let navigator_surface = entity.downgrade();
        // One control per marker in every state, exactly like the reference:
        // hovering reveals the labels of controls that were already there
        // rather than mounting new ones, so pointer and keyboard activation
        // can never race a remount.
        // Plain Div until the metrics listener below: `on_children_prepainted`
        // is a Div inherent, so identity, scrolling, and the width clock
        // attach at the tail. The list is the pill's positioning context.
        let mut list = div()
            .relative()
            .flex()
            .flex_col()
            .items_end()
            .gap(theme.spacing.steps(1.0))
            .p(px(8.0))
            // Hover is evaluated while painting, and a mouse move alone does
            // not schedule a frame. The list is the hit target for the gaps
            // and padding between rows, so it has to refresh from its own move
            // events; rows already refresh the same way.
            .on_mouse_move(move |_: &MouseMoveEvent, window: &mut Window, _| {
                window.refresh();
            });
        if metrics.viewport_px > 0.0 {
            list = list.max_h(px(metrics.viewport_px * 0.7));
        }
        // Shared hover pill behind the rows plus the probe capturing list
        // bounds for pill-relative coordinates, mirroring the model picker.
        list = list.child(self.render_navigator_hover_pill(theme, cx.reduce_motion()));
        let hover_surface_bounds = Rc::clone(&self.navigator_hover_surface);
        let list_bounds_surface = entity.downgrade();
        // Height-change detection is window-local: two windows share one
        // surface, and a shared last-measured value would make them notify
        // each other about each other's layout.
        let last_probe_height = window.use_state(cx, |_, _| None::<gpui::Pixels>);
        list = list.child(
            canvas(
                |_, _, _| {},
                move |bounds, (), window, app| {
                    // The metrics listener reads this probe on the next frame,
                    // so a height change here has to schedule that frame too:
                    // otherwise an expanded rail keeps its collapsed top
                    // offset until some unrelated repaint.
                    let changed = last_probe_height.update(app, |height, _| {
                        let changed = *height != Some(bounds.size.height);
                        *height = Some(bounds.size.height);
                        changed
                    });
                    *hover_surface_bounds.borrow_mut() = Some(bounds);
                    if changed {
                        let _ = list_bounds_surface.update(app, |_, cx| cx.notify());
                        window.defer(app, |window, _| window.refresh());
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        );
        for marker in &markers {
            let key = navigator_focus_key(&marker.target);
            let Some(handle) = self.navigator_focus.get(&key).cloned() else {
                continue;
            };
            let slug = navigator_target_slug(&marker.target);
            let control_selector = format!("{TURN_NAVIGATOR_CONTROL_PREFIX}-{slug}");
            let label_selector = format!("{control_selector}-label");
            let tick_selector = format!("{control_selector}-tick");
            let active = navigator_active.as_deref() == Some(slug);
            let click_surface = navigator_surface.clone();
            let click_target = marker.target.clone();
            let probe_hover = Rc::clone(&self.navigator_hover);
            let probe_surface = Rc::clone(&self.navigator_hover_surface);
            let probe_id = slug.to_owned();
            let move_hover = Rc::clone(&self.navigator_hover);
            let move_surface = Rc::clone(&self.navigator_hover_surface);
            let move_id = slug.to_owned();
            let focus_ring_color = theme.interaction.focus_ring_color.to_paint();
            let focus_ring_width = theme.interaction.focus_ring_width;
            // Labels show expanded, ticks at rest: the spans swap while the
            // row itself never remounts, so pointer and keyboard activation
            // keep one stable target.
            let mut row_content_label = None;
            if expanded {
                let mut label = div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_size(theme.typography.control_text)
                    .text_color(theme.colors.foreground.to_paint())
                    .debug_selector({
                        let selector = label_selector.clone();
                        move || selector.clone()
                    })
                    .child(marker.label.clone());
                if active {
                    label = label.font_weight(FontWeight::MEDIUM);
                }
                row_content_label = Some(label);
            }
            let mut row_content_tick = None;
            if !expanded {
                row_content_tick = Some(
                    div()
                        .h(px(1.0))
                        .w(px(if active { 24.0 } else { 16.0 }))
                        .rounded_full()
                        .debug_selector({
                            let selector = tick_selector.clone();
                            move || selector.clone()
                        })
                        .bg(if active {
                            theme.colors.foreground.to_paint()
                        } else {
                            theme.colors.muted_foreground.with_alpha(0.5).to_paint()
                        }),
                );
            }
            let mut row = div()
                .id(control_selector.clone())
                .relative()
                .track_focus(&handle)
                .tab_index(0)
                .role(gpui::Role::Button)
                .aria_label(marker.label.clone())
                .cursor_pointer()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .justify_end()
                .gap(theme.spacing.steps(3.0))
                .debug_selector(move || control_selector.clone())
                .focus(move |focused| {
                    focused.shadow(vec![BoxShadow {
                        color: focus_ring_color,
                        offset: point(px(0.0), px(0.0)),
                        blur_radius: px(0.0),
                        spread_radius: focus_ring_width,
                        inset: false,
                    }])
                })
                // Trigger zone: the rightmost 40 px strip reveals the card,
                // so the pill belongs to the label area — a pointer over the
                // strip clears it instead of highlighting a tick row of its
                // own. Keyboard focus selects through render instead.
                .on_mouse_move(move |event: &MouseMoveEvent, window, _cx| {
                    let over_strip = move_surface.borrow().as_ref().is_some_and(|surface| {
                        f64::from(event.position.x) >= f64::from(surface.right()) - 40.0
                    });
                    if over_strip {
                        move_hover.borrow_mut().hide();
                    } else {
                        move_hover.borrow_mut().set_active(move_id.clone());
                    }
                    window.refresh();
                })
                // Pointer and keyboard activation share one path: the button
                // role synthesizes click from Enter/Space, so no separate key
                // handler may double the intent.
                .on_click(move |_, _, app| {
                    let _ = click_surface.update(app, |surface, cx| {
                        surface.request_scroll(click_target.clone(), cx);
                    });
                });
            if let Some(label) = row_content_label {
                row = row.child(label);
            }
            if let Some(tick) = row_content_tick {
                row = row.child(tick);
            }
            row = row.child(
                canvas(
                    |_, _, _| {},
                    move |bounds, (), window, cx| {
                        let Some(surface) = *probe_surface.borrow() else {
                            return;
                        };
                        let rect = HoverRect {
                            left: f32::from(bounds.left() - surface.left()),
                            top: f32::from(bounds.top() - surface.top()),
                            width: f32::from(bounds.size.width),
                            height: f32::from(bounds.size.height),
                        };
                        if probe_hover.borrow_mut().measure(&probe_id, rect) {
                            window.defer(cx, |window, _| window.refresh());
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            );
            list = list.child(row);
        }
        // Rail metrics converge through the same window-local discipline as
        // end space: measured per window, notified only on change.
        let metrics_state = navigator_metrics.clone();
        let metrics_hover_surface = Rc::clone(&self.navigator_hover_surface);
        list = list.on_children_prepainted(move |_children_bounds, window, app| {
            let changed = navigator_surface
                .update(app, |surface, cx| {
                    let viewport = f64::from(surface.scroll_handle.bounds().size.height);
                    // Center the actually painted list box, read from its probe:
                    // row spans miscount once absolute overlays (pill, probe,
                    // glass) join the children.
                    let guard = metrics_hover_surface.borrow();
                    let Some(list_bounds) = guard.as_ref() else {
                        return false;
                    };
                    let height = f64::from(list_bounds.size.height);
                    // The capped list is what actually paints: clamp the measured
                    // height to the 70 % cap before centering, or a long list
                    // pins its top at zero instead of centering the cap.
                    let capped = if viewport > 0.0 {
                        height.min(viewport * 0.7)
                    } else {
                        height
                    };
                    let next = TurnNavigatorMetrics {
                        top_px: (((viewport - capped) / 2.0).max(0.0)) as f32,
                        viewport_px: viewport as f32,
                    };
                    let changed = metrics_state.update(cx, |metrics, _| {
                        let changed = metrics.top_px != next.top_px
                            || metrics.viewport_px != next.viewport_px;
                        *metrics = next;
                        changed
                    });
                    if changed {
                        cx.notify();
                    }
                    changed
                })
                .unwrap_or(false);
            if changed {
                // A notification raised inside prepaint does not itself
                // request a frame: without this the rail keeps its previous
                // top until some unrelated repaint, which is exactly what a
                // reduced-motion reveal would otherwise show.
                window.defer(app, |window, _| window.refresh());
            }
        });
        // Identity, scrolling, and width attach after the Div-phase
        // listener above: the rail owns gestures over its own rows, applying
        // the bounded immediate scroll locally (the reference plain scroller
        // has no smoothing) and containing the event so the transcript does
        // not scroll underneath. Width motion replays per hover generation,
        // never on mount: generation zero paints the static width outright
        // (open 250 ms, close 150 ms on the dropdown curve).
        let navigator_scroll_handle = self.navigator_scroll.clone();
        let list = list
            .id(SharedString::from(TURN_NAVIGATOR_LIST_SELECTOR))
            .overflow_y_scroll()
            .track_scroll(&self.navigator_scroll)
            .on_scroll_wheel(move |event, window, cx| {
                let delta = f32::from(event.delta.pixel_delta(window.line_height()).y);
                if delta.abs() <= f32::EPSILON {
                    return;
                }
                let offset = navigator_scroll_handle.offset();
                let maximum = f32::from(navigator_scroll_handle.max_offset().y).max(0.0);
                let next = (f32::from(offset.y) + delta).clamp(-maximum, 0.0);
                navigator_scroll_handle.set_offset(point(offset.x, px(next)));
                cx.stop_propagation();
            });
        // Width motion replays per hover generation, never on mount, and
        // never under reduced motion: generation zero paints the static
        // width outright. Reversals start from the retained painted width,
        // so an interrupted flight never jumps to a fixed endpoint. Open
        // runs the reference 250 ms, close 150 ms, both on the dropdown
        // curve.
        let width_target = if expanded { inspector_width } else { 40.0 };
        let list: AnyElement = if self.navigator_width_generation == 0 || cx.reduce_motion() {
            *self.navigator_width_px.borrow_mut() = width_target;
            list.w(px(width_target)).into_any_element()
        } else {
            let from_w = self.navigator_width_from;
            let width_state = Rc::clone(&self.navigator_width_px);
            let generation = self.navigator_width_generation;
            let duration_ms = if expanded { 250 } else { 150 };
            list.w(px(from_w))
                .with_animation(
                    ElementId::Name(SharedString::from(format!(
                        "{TURN_NAVIGATOR_SELECTOR}-width-{generation}"
                    ))),
                    Animation::new(Duration::from_millis(duration_ms))
                        .with_easing(navigator_smooth_out),
                    move |list, progress| {
                        let width = from_w + (width_target - from_w) * progress;
                        *width_state.borrow_mut() = width;
                        list.w(px(width))
                    },
                )
                .into_any_element()
        };
        let hover_surface = entity.downgrade();
        // Middle of the card, not the prose column: the reference anchors
        // the rail at `top-1/2 right-2` with a 70 % height cap. Centering
        // has no translate primitive here, so the top offset converges from
        // the measured list height in window-local state.
        let mut rail = div()
            .id(SharedString::from(TURN_NAVIGATOR_SELECTOR))
            .absolute()
            .right(px(8.0))
            .top(px(metrics.top_px))
            .debug_selector(|| TURN_NAVIGATOR_SELECTOR.to_owned())
            .on_hover(move |hovered: &bool, window, app| {
                let _ = hover_surface.update(app, |surface, cx| {
                    let mut changed = false;
                    if surface.navigator_expanded != *hovered {
                        surface.navigator_expanded = *hovered;
                        surface.navigator_width_from = *surface.navigator_width_px.borrow();
                        surface.navigator_width_generation =
                            surface.navigator_width_generation.wrapping_add(1);
                        changed = true;
                    }
                    // The pill belongs to the expanded area: leaving the rail
                    // hides it unless a row keeps keyboard focus, which holds
                    // the labels open on its own.
                    if !*hovered {
                        let any_focused = surface
                            .navigator_focus
                            .values()
                            .any(|handle| handle.is_focused(window));
                        if !any_focused && surface.navigator_hover.borrow().visible() {
                            surface.navigator_hover.borrow_mut().hide();
                            changed = true;
                        }
                    }
                    if changed {
                        cx.notify();
                    }
                });
            });
        // Expanded glass card behind the labels, composed from the shipped
        // picker/composer material primitives: backdrop blur, foreground
        // base, material and highlight layers, and the card shadow. The
        // collapsed tick strip stays unpainted and click-through.
        if expanded {
            let radius = RadiusTokens::value(RadiusStep::Xl);
            rail = rail
                .rounded(radius)
                .backdrop_blur(glass_blur_radius(GlassStrength::Strong))
                .bg(glass_foreground_base(*theme))
                .shadow(glass_card_shadows())
                .child(glass_material_layer(GlassStrength::Strong, radius))
                .child(glass_highlight_layer(GlassStrength::Strong, radius));
        }
        rail = rail.child(list);
        Some(rail.into_any_element())
    }
}

fn card_heading(title: impl Into<SharedString>, theme: &ArtisanTheme) -> Div {
    div()
        .w_full()
        .min_w_0()
        .text_size(theme.typography.control_text)
        .font_weight(FontWeight::MEDIUM)
        .child(title.into())
}

fn body_text(text: &str, theme: &ArtisanTheme) -> Div {
    div()
        .w_full()
        .min_w_0()
        .text_size(theme.typography.editor_text_desktop)
        .line_height(theme.spacing.steps(6.0))
        .whitespace_normal()
        .child(text.to_owned())
}

fn changed_file_row(
    card_id: &SceneId,
    index: usize,
    file: &SceneFileChange,
    theme: &ArtisanTheme,
) -> AnyElement {
    let selector = changed_file_selector(card_id, index);
    let mut row = div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(theme.spacing.steps(2.0));
    row = row.debug_selector(move || selector.clone());
    row = row
        .child(outline_badge(
            BadgeStyle::resolve(*theme),
            file_change_status_label(file.status),
        ))
        .child(body_text(&file.path, theme));
    row.into_any_element()
}

fn status_color(theme: &ArtisanTheme, narration: TurnNarration) -> gpui::Hsla {
    match narration {
        TurnNarration::Failed | TurnNarration::Interrupted | TurnNarration::Cancelled => {
            theme.colors.destructive.to_paint()
        }
        _ => theme.colors.muted_foreground.to_paint(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use artisan_domain::{ConversationLifecycle, ItemId, TurnId};
    use artisan_ui::theme::ThemeMode;
    use gpui::{
        Entity, Modifiers, ScrollDelta, ScrollWheelEvent, TestAppContext, TouchPhase,
        VisualTestContext, point, px, size,
    };

    use crate::conversation_scene::{
        AssistantPhase, ConversationScene, ItemProvenance, SceneDisclosure, SceneItem,
        SceneItemKind, SceneTurn, TurnFooterBlock, TurnNarration, TurnNarrationEntry,
    };

    fn scene_id(value: &str) -> SceneId {
        SceneId::parse(value).expect("scene id is valid")
    }

    fn turn_id(value: &str) -> TurnId {
        TurnId::parse(value).expect("turn id is valid")
    }

    fn item(
        id: &str,
        ordinal: u64,
        kind: SceneItemKind,
        disclosure: Option<SceneDisclosure>,
    ) -> SceneItem {
        SceneItem::new(scene_id(id), turn_id("turn_a"), ordinal, kind, disclosure)
            .expect("scene item is valid")
    }

    fn body() -> String {
        (0..12)
            .map(|line| format!("Transcript line {line} keeps the viewport measurable."))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn scene(items: Vec<SceneItem>) -> ConversationScene {
        ConversationScene::build(
            vec![SceneTurn::new(
                turn_id("turn_a"),
                0,
                ConversationLifecycle::Completed,
            )],
            items,
            vec![TurnNarrationEntry::new(
                turn_id("turn_a"),
                TurnNarration::Quiet,
            )],
            Vec::new(),
        )
        .expect("conversation scene is valid")
    }

    fn scroll_target_scene(disclosure: SceneDisclosure) -> ConversationScene {
        let mut items = (0..8)
            .map(|index| {
                item(
                    &format!("user-{index}"),
                    index + 1,
                    SceneItemKind::UserMessage { body: body() },
                    None,
                )
            })
            .collect::<Vec<_>>();
        items.extend([
            item(
                "work-first",
                9,
                SceneItemKind::ReasoningSummary { body: body() },
                Some(disclosure),
            ),
            item(
                "work-target",
                10,
                SceneItemKind::Activity {
                    body: body(),
                    kind: None,
                    detail: None,
                },
                Some(disclosure),
            )
            .with_provenance(ItemProvenance {
                run_id: None,
                lifecycle: Some(ConversationLifecycle::Active),
            }),
        ]);
        scene(items)
    }

    fn settle(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.run_until_parked();
    }

    fn offset(
        surface: &Entity<ConversationSurface>,
        cx: &mut VisualTestContext,
    ) -> gpui::Point<gpui::Pixels> {
        cx.update(|_, app| surface.read(app).scroll_handle().offset())
    }

    #[test]
    fn quiet_and_suppressed_narrations_paint_no_status_row() {
        assert_eq!(turn_status_copy(TurnNarration::Quiet), None);
        assert_eq!(turn_status_copy(TurnNarration::StreamingSuppression), None);
        assert!(!status_row_visible(false, TurnNarration::Quiet));
        assert!(!status_row_visible(true, TurnNarration::Quiet));
        assert!(!status_row_visible(
            false,
            TurnNarration::StreamingSuppression
        ));
        assert!(status_row_visible(false, TurnNarration::Thinking));
        assert!(status_row_visible(false, TurnNarration::Working));
        assert!(status_row_visible(
            false,
            TurnNarration::ProviderWait
        ));
    }

    #[test]
    fn terminal_duration_prefers_the_group_header() {
        let worked = TurnNarration::WorkedFor { millis: 65_000 };
        let thought = TurnNarration::ThoughtFor { millis: 5_000 };
        assert_eq!(
            turn_status_copy(worked),
            Some("Worked for 1m 5s".to_owned())
        );
        assert!(!status_row_visible(true, worked));
        assert!(!status_row_visible(true, thought));
        assert!(status_row_visible(false, worked));
        assert!(status_row_visible(false, thought));
        assert!(status_row_visible(true, TurnNarration::Failed));
        assert!(status_row_visible(true, TurnNarration::ProviderWait));
    }

    #[test]
    fn live_line_is_owned_by_the_latest_group_once() {
        assert_eq!(
            live_group_header_copy(TurnNarration::Working, Some(0), Some(65_000)),
            Some("Working for 1m 5s".to_owned())
        );
        assert_eq!(
            live_group_header_copy(TurnNarration::Thinking, None, None),
            Some("Thinking".to_owned())
        );
        assert_eq!(
            live_group_header_copy(TurnNarration::ProviderWait, Some(0), Some(5_000)),
            None
        );
        assert_eq!(
            live_group_header_copy(TurnNarration::Failed, None, None),
            None
        );
        assert!(status_row_visible(true, TurnNarration::Thinking));
        assert!(status_row_visible(true, TurnNarration::Working));
        assert!(status_row_visible(false, TurnNarration::Thinking));
        assert!(status_row_visible(false, TurnNarration::Working));
    }

    #[test]
    fn owning_group_index_selects_the_latest_group() {
        let live_scene = ConversationScene::build(
            vec![SceneTurn::new(
                turn_id("turn_a"),
                0,
                ConversationLifecycle::Active,
            )],
            vec![
                item(
                    "user-a",
                    1,
                    SceneItemKind::UserMessage {
                        body: "hi".to_owned(),
                    },
                    None,
                ),
                item(
                    "work-a",
                    2,
                    SceneItemKind::Activity {
                        body: "a".to_owned(),
                        kind: None,
                        detail: None,
                    },
                    None,
                ),
                item(
                    "assistant-a",
                    3,
                    SceneItemKind::AssistantMessage {
                        body: "hello".to_owned(),
                        phase: AssistantPhase::Final,
                    },
                    None,
                ),
                item(
                    "work-b",
                    4,
                    SceneItemKind::Activity {
                        body: "b".to_owned(),
                        kind: None,
                        detail: None,
                    },
                    None,
                ),
            ],
            vec![TurnNarrationEntry::new(
                turn_id("turn_a"),
                TurnNarration::Working,
            )],
            Vec::new(),
        )
        .expect("conversation scene is valid");
        let turn = live_scene
            .turn_scene(&turn_id("turn_a"))
            .expect("turn present");
        assert_eq!(owning_group_index(turn), Some(3));
        assert_eq!(
            ordered_block_kinds(&live_scene)[3],
            RenderedBlockKind::WorkGroup
        );
        let empty = scene(Vec::new());
        let empty_turn = empty
            .turn_scene(&turn_id("turn_a"))
            .expect("turn present");
        assert_eq!(owning_group_index(empty_turn), None);
    }

    #[test]
    fn live_status_formats_the_authoritative_elapsed_basis() {
        assert_eq!(
            live_status_copy(TurnNarration::Thinking, None, Some(1_000)),
            Some("Thinking".to_owned())
        );
        assert_eq!(
            live_status_copy(TurnNarration::Thinking, Some(0), None),
            Some("Thinking".to_owned())
        );
        assert_eq!(
            live_status_copy(TurnNarration::Thinking, Some(0), Some(65_000)),
            Some("Thinking for 1m 5s".to_owned())
        );
        assert_eq!(
            live_status_copy(TurnNarration::Working, Some(10_000), Some(3_000)),
            Some("Working for 0s".to_owned())
        );
        assert_eq!(
            live_status_copy(TurnNarration::Working, Some(0), Some(3_661_000)),
            Some("Working for 1h 1m 1s".to_owned())
        );
        assert_eq!(
            live_status_copy(TurnNarration::Failed, Some(0), Some(5_000)),
            Some("Failed".to_owned())
        );
        assert_eq!(
            live_status_copy(TurnNarration::Quiet, Some(0), Some(5_000)),
            None
        );
        assert_eq!(
            live_status_copy(TurnNarration::StreamingSuppression, Some(0), Some(5_000)),
            None
        );
        assert_eq!(
            live_status_copy(TurnNarration::ProviderWait, Some(0), Some(5_000)),
            Some("Waiting for provider to respond…".to_owned())
        );
        assert_eq!(
            live_status_copy(TurnNarration::ProviderWait, None, Some(5_000)),
            Some("Waiting for provider to respond…".to_owned())
        );
    }

    #[test]
    fn multiline_reasoning_reduces_to_one_headline() {
        assert_eq!(
            status_summary_copy(Some(
                "**First thought**\n\nSome body.\n\n**Planning playful ambiguous response**"
            )),
            Some("Planning playful ambiguous response".to_owned())
        );
        assert_eq!(
            status_summary_copy(Some("Unfinished thought without end")),
            None
        );
        assert_eq!(status_summary_copy(None), None);
    }

    #[test]
    fn provider_wait_copy_names_the_engine_when_known() {
        assert_eq!(
            provider_wait_copy(None),
            "Waiting for provider to respond…".to_owned()
        );
        assert_eq!(
            provider_wait_copy(Some("Claude")),
            "Waiting for Claude to respond…".to_owned()
        );
    }

    #[test]
    fn production_provider_wait_keeps_the_waiting_sentence() {
        // Production derives ProviderWait while no scene fact has arrived;
        // the basis may still ride along, but the row narrates the wait —
        // counting lives in the group header, never in this sentence.
        let scene = ConversationScene::build(
            vec![SceneTurn::new(
                turn_id("turn_a"),
                0,
                ConversationLifecycle::Active,
            )],
            Vec::new(),
            vec![
                TurnNarrationEntry::new(turn_id("turn_a"), TurnNarration::ProviderWait)
                    .with_active_started_at_ms(0),
            ],
            Vec::new(),
        )
        .expect("provider wait may carry the active basis");
        let (narration, basis) = scene
            .turn_scene(&turn_id("turn_a"))
            .expect("turn present")
            .blocks()
            .iter()
            .find_map(|block| match block {
                TurnBlock::TurnStatus(status) => {
                    Some((status.narration, status.active_started_at_ms))
                }
                _ => None,
            })
            .expect("status block present");
        assert_eq!(narration, TurnNarration::ProviderWait);
        assert_eq!(basis, Some(0));
        assert_eq!(
            live_status_copy(narration, basis, Some(65_000)),
            Some("Waiting for provider to respond…".to_owned())
        );
    }

    #[test]
    fn status_copy_and_paint_share_one_decision() {
        // Summary wins over narration; unfinished phases fall back through.
        assert_eq!(
            turn_status_copy_text(
                TurnNarration::Working,
                Some(0),
                Some(65_000),
                Some("**Planning it**"),
                None,
            ),
            Some("Planning it".to_owned())
        );
        assert_eq!(
            turn_status_copy_text(
                TurnNarration::Working,
                Some(0),
                Some(65_000),
                Some("no ending here"),
                None,
            ),
            Some("Working for 1m 5s".to_owned())
        );
        assert_eq!(
            turn_status_copy_text(TurnNarration::Quiet, None, None, None, None),
            None
        );
        // Identical lines paint once; distinct lines both paint.
        assert!(!turn_status_paints(
            true,
            TurnNarration::Working,
            Some("Working for 1m 5s"),
            Some("Working for 1m 5s"),
        ));
        assert!(turn_status_paints(
            true,
            TurnNarration::Working,
            Some("Planning it"),
            Some("Working for 1m 5s"),
        ));
        assert!(turn_status_paints(
            false,
            TurnNarration::Working,
            Some("Working for 1m 5s"),
            None,
        ));
        assert!(!turn_status_paints(
            true,
            TurnNarration::WorkedFor { millis: 1_000 },
            Some("Worked for 1s"),
            None,
        ));
    }

    #[test]
    fn identical_status_and_header_paint_once() {
        assert!(status_duplicates_owner(
            Some("Working for 1m 5s"),
            Some("Working for 1m 5s")
        ));
        assert!(!status_duplicates_owner(
            Some("Thinking for 1m 5s"),
            Some("Working for 1m 5s")
        ));
        assert!(!status_duplicates_owner(Some("Working"), None));
        assert!(!status_duplicates_owner(None, Some("Working for 1m 5s")));
        assert!(!status_duplicates_owner(None, None));
    }

    #[test]
    fn work_group_header_has_no_generic_fallback() {
        use crate::conversation_scene::WorkGroupLabel;
        assert_eq!(work_group_header_copy(None), None);
        assert_eq!(
            work_group_header_copy(Some(WorkGroupLabel::WorkedFor { millis: 65_000 })),
            Some("Worked for 1m 5s".to_owned())
        );
        assert_eq!(
            work_group_header_copy(Some(WorkGroupLabel::ThoughtFor { millis: 5_000 })),
            Some("Thought for 5s".to_owned())
        );
    }

    #[test]
    fn end_space_matches_the_reference_anchoring_formula() {
        assert_eq!(end_space_height(900.0, 0.0, 0.0), 884.0);
        assert_eq!(end_space_height(900.0, 100.0, 800.0), 192.0);
        assert_eq!(end_space_height(0.0, 0.0, 0.0), 192.0);
    }

    #[gpui::test]
    fn end_space_grows_short_content_to_the_top_inset(cx: &mut TestAppContext) {
        // One short turn in a tall window: the painted spacer must equal the
        // anchoring formula applied to live geometry, proving the observer
        // drives the spacer instead of the fixed base.
        let user_scene = ConversationScene::build(
            vec![SceneTurn::new(
                turn_id("turn_a"),
                0,
                ConversationLifecycle::Active,
            )],
            vec![item(
                "user-a",
                1,
                SceneItemKind::UserMessage {
                    body: "hi".to_owned(),
                },
                None,
            )],
            vec![TurnNarrationEntry::new(
                turn_id("turn_a"),
                TurnNarration::Quiet,
            )],
            Vec::new(),
        )
        .expect("conversation scene is valid");
        let (surface, cx) = cx.add_window_view(|_, surface_cx| {
            ConversationSurface::new(user_scene, ThemeMode::Dark, surface_cx)
        });
        cx.simulate_resize(size(px(720.0), px(600.0)));
        settle(cx);
        settle(cx);
        let turn_bounds = cx
            .debug_bounds("artisan-conversation-surface-turn-turn_a")
            .expect("turn must paint");
        let spacer_bounds = cx
            .debug_bounds(TRANSCRIPT_END_SPACE_SELECTOR)
            .expect("end space must paint");
        let viewport_height = cx.update(|_, app| {
            f64::from(surface.read(app).scroll_handle().bounds().size.height)
        });
        // Window and content spaces agree on differences: the element offset
        // cancels out of the formula exactly like the scroll-offset path.
        let expected = end_space_height(
            viewport_height,
            f64::from(turn_bounds.origin.y),
            f64::from(spacer_bounds.origin.y),
        );
        assert!(expected >= 192.0);
        assert!((f64::from(spacer_bounds.size.height) - expected).abs() < 1.0);
        cx.update(|_, app| {
            // Settling the viewport legitimately emits viewport observations;
            // drain them and prove nothing else is pending.
            surface.update(app, |surface, _| {
                for action in surface.take_actions() {
                    assert!(
                        matches!(
                            action,
                            ConversationSurfaceAction::ViewportObserved(_)
                        ),
                        "only legitimate viewport observations may precede assertions, got {action:?}"
                    );
                }
            });
            assert!(surface.read(app).pending_actions().is_empty());
        });
    }

    #[gpui::test]
    fn group_detail_rows_paint_in_durable_order(cx: &mut TestAppContext) {
        // Mounted order proof to go with the pure merge test: two legacy
        // activity rows must paint top-to-bottom in vec order.
        let detail_scene = ConversationScene::build(
            vec![SceneTurn::new(
                turn_id("turn_a"),
                0,
                ConversationLifecycle::Active,
            )],
            vec![
                item(
                    "work-a",
                    1,
                    SceneItemKind::Activity {
                        body: "first".to_owned(),
                        kind: None,
                        detail: None,
                    },
                    None,
                ),
                item(
                    "work-b",
                    2,
                    SceneItemKind::Activity {
                        body: "second".to_owned(),
                        kind: None,
                        detail: None,
                    },
                    None,
                ),
            ],
            vec![TurnNarrationEntry::new(
                turn_id("turn_a"),
                TurnNarration::Working,
            )],
            Vec::new(),
        )
        .expect("conversation scene is valid");
        let (surface, cx) = cx.add_window_view(|_, surface_cx| {
            ConversationSurface::new(detail_scene, ThemeMode::Dark, surface_cx)
        });
        cx.simulate_resize(size(px(720.0), px(480.0)));
        settle(cx);
        let first = cx
            .debug_bounds("artisan-conversation-surface-turn-turn_a-block-work-work-a-detail-0")
            .expect("first detail must paint");
        let second = cx
            .debug_bounds("artisan-conversation-surface-turn-turn_a-block-work-work-a-detail-1")
            .expect("second detail must paint");
        assert!(first.origin.y < second.origin.y);
        assert!(first.size.height > px(0.0));
        cx.update(|_, app| {
            surface.update(app, |surface, _| {
                for action in surface.take_actions() {
                    assert!(
                        matches!(
                            action,
                            ConversationSurfaceAction::ViewportObserved(_)
                        ),
                        "only legitimate viewport observations may precede assertions, got {action:?}"
                    );
                }
            });
            assert!(surface.read(app).pending_actions().is_empty());
        });
    }

    /// Builds one work group holding a single kinded terminal activity.
    fn activity_chain_scene(
        turn_lifecycle: ConversationLifecycle,
        narration: TurnNarration,
        disclosure: SceneDisclosure,
    ) -> ConversationScene {
        ConversationScene::build(
            vec![SceneTurn::new(turn_id("turn_a"), 0, turn_lifecycle)],
            vec![item(
                "work-a",
                1,
                SceneItemKind::Activity {
                    body: "cargo test --locked".to_owned(),
                    kind: Some("terminal_activity".to_owned()),
                    detail: Some(
                        r#""C:\Program Files\WindowsApps\pwsh.exe" -Command "cargo test --locked""#
                            .to_owned(),
                    ),
                },
                Some(disclosure),
            )],
            vec![TurnNarrationEntry::new(turn_id("turn_a"), narration)],
            Vec::new(),
        )
        .expect("activity chain scene is valid")
    }

    #[gpui::test]
    fn settled_activity_chain_starts_closed_and_toggles_open(cx: &mut TestAppContext) {
        const TRIGGER: &str =
            "artisan-conversation-surface-turn-turn_a-block-work-work-a-trace-0-trigger";
        const DETAIL: &str = "artisan-conversation-surface-turn-turn_a-block-work-work-a-detail-0";
        const RAIL: &str =
            "artisan-conversation-surface-turn-turn_a-block-work-work-a-trace-0-rail";
        let (surface, cx) = cx.add_window_view(|_, surface_cx| {
            ConversationSurface::new(
                activity_chain_scene(
                    ConversationLifecycle::Completed,
                    TurnNarration::Quiet,
                    SceneDisclosure::Open,
                ),
                ThemeMode::Dark,
                surface_cx,
            )
        });
        cx.simulate_resize(size(px(720.0), px(480.0)));
        settle(cx);
        assert!(
            cx.debug_bounds(TRIGGER).is_some(),
            "the chain header paints even when closed"
        );
        assert!(
            cx.debug_bounds(DETAIL).is_none(),
            "a settled chain starts closed"
        );

        let trigger = cx.debug_bounds(TRIGGER).expect("chain trigger paints");
        let center = point(trigger.origin.x + px(4.0), trigger.origin.y + px(4.0));
        cx.simulate_mouse_down(center, gpui::MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(center, gpui::MouseButton::Left, Modifiers::default());
        settle(cx);
        let detail = cx
            .debug_bounds(DETAIL)
            .expect("toggling the header mounts the rows");
        let rail = cx.debug_bounds(RAIL).expect("the left rail paints");
        assert!(
            rail.size.height >= detail.size.height,
            "the rail spans the mounted rows, got {rail:?} over {detail:?}"
        );
        // The toggle is surface-local: it never emits a scene disclosure
        // action for a chain identity the scene does not own.
        cx.update(|_, app| {
            let actions = surface.read(app).pending_actions().to_vec();
            assert!(
                actions
                    .iter()
                    .all(|action| matches!(action, ConversationSurfaceAction::ViewportObserved(_))),
                "the chain toggle stays surface-local, got {actions:?}"
            );
        });
    }

    #[gpui::test]
    fn live_activity_chain_paints_open_without_a_toggle(cx: &mut TestAppContext) {
        const DETAIL: &str = "artisan-conversation-surface-turn-turn_a-block-work-work-a-detail-0";
        let (_surface, cx) = cx.add_window_view(|_, surface_cx| {
            ConversationSurface::new(
                activity_chain_scene(
                    ConversationLifecycle::Active,
                    TurnNarration::Working,
                    SceneDisclosure::Open,
                ),
                ThemeMode::Dark,
                surface_cx,
            )
        });
        cx.simulate_resize(size(px(720.0), px(480.0)));
        settle(cx);
        assert!(
            cx.debug_bounds(DETAIL).is_some(),
            "a live chain opens itself"
        );
    }

    #[gpui::test]
    fn failed_activity_chain_paints_open_without_a_toggle(cx: &mut TestAppContext) {
        const DETAIL: &str = "artisan-conversation-surface-turn-turn_a-block-work-work-a-detail-0";
        let (_surface, cx) = cx.add_window_view(|_, surface_cx| {
            ConversationSurface::new(
                activity_chain_scene(
                    ConversationLifecycle::Failed,
                    TurnNarration::Quiet,
                    SceneDisclosure::Open,
                ),
                ThemeMode::Dark,
                surface_cx,
            )
        });
        cx.simulate_resize(size(px(720.0), px(480.0)));
        settle(cx);
        assert!(
            cx.debug_bounds(DETAIL).is_some(),
            "a failed chain opens itself"
        );
    }

    #[gpui::test]
    fn session_activity_chain_carries_kind_and_detail(cx: &mut TestAppContext) {
        const DETAIL: &str = "artisan-conversation-surface-turn-turn_a-block-work-turn_a-detail-1";
        let run = artisan_domain::RunId::parse("run-a").expect("fixture run id is valid");
        let session_scene = ConversationScene::build(
            vec![SceneTurn::new(
                turn_id("turn_a"),
                0,
                ConversationLifecycle::Active,
            )],
            vec![
                item(
                    "work-a",
                    1,
                    SceneItemKind::Activity {
                        body: "cargo test --locked".to_owned(),
                        kind: Some("terminal_activity".to_owned()),
                        detail: Some(
                            r#""C:\Program Files\WindowsApps\pwsh.exe" -Command "cargo test --locked""#
                                .to_owned(),
                        ),
                    },
                    None,
                )
                .with_provenance(ItemProvenance {
                    run_id: Some(run),
                    lifecycle: Some(ConversationLifecycle::Active),
                }),
            ],
            vec![TurnNarrationEntry::new(
                turn_id("turn_a"),
                TurnNarration::Working,
            )],
            Vec::new(),
        )
        .expect("session activity scene is valid");
        let (_surface, cx) = cx.add_window_view(|_, surface_cx| {
            ConversationSurface::new(session_scene, ThemeMode::Dark, surface_cx)
        });
        cx.simulate_resize(size(px(720.0), px(480.0)));
        settle(cx);
        assert!(
            cx.debug_bounds(DETAIL).is_some(),
            "a live session chain mounts its kinded row"
        );
    }

    #[gpui::test]
    fn unresolved_assistant_links_queue_one_bounded_resolve_request(cx: &mut TestAppContext) {
        let link_scene = ConversationScene::build(
            vec![SceneTurn::new(
                turn_id("turn_a"),
                0,
                ConversationLifecycle::Completed,
            )],
            vec![item(
                "reply",
                1,
                SceneItemKind::AssistantMessage {
                    body: "See [the docs](https://Example.com/page#section) and [again](https://example.com/page)."
                        .to_owned(),
                    phase: AssistantPhase::Final,
                },
                None,
            )],
            vec![TurnNarrationEntry::new(
                turn_id("turn_a"),
                TurnNarration::Quiet,
            )],
            Vec::new(),
        )
        .expect("conversation scene is valid");
        let (surface, cx) = cx.add_window_view(|_, surface_cx| {
            ConversationSurface::new(link_scene, ThemeMode::Dark, surface_cx)
        });
        cx.simulate_resize(size(px(720.0), px(480.0)));
        settle(cx);

        // One render pass observes both links; the fragment variant and the
        // exact duplicate collapse into one canonical resolve URL.
        cx.update(|_, app| {
            surface.update(app, |surface, cx| {
                let urls = surface
                    .take_actions()
                    .into_iter()
                    .filter_map(|action| match action {
                        ConversationSurfaceAction::ResolveRichLinks { urls } => Some(urls),
                        _ => None,
                    })
                    .flatten()
                    .collect::<Vec<_>>();
                assert_eq!(urls, vec!["https://example.com/page".to_owned()]);

                // A resolved title repaints without another resolve request.
                surface.set_rich_link_title(
                    "https://example.com/page",
                    &SharedString::from("Resolved Docs"),
                    4_000_000_000_000,
                    cx,
                );
            });
        });
        settle(cx);
        cx.update(|_, app| {
            surface.update(app, |surface, _| {
                assert_eq!(
                    surface.rich_link_titles.lookup("https://example.com/page#section"),
                    Some(SharedString::from("Resolved Docs"))
                );
                assert!(
                    !surface.take_actions().iter().any(|action| matches!(
                        action,
                        ConversationSurfaceAction::ResolveRichLinks { .. }
                    )),
                    "a fresh title must not queue another resolve"
                );
            });
        });

        // A failed resolution keeps the authored label and never retries.
        cx.update(|_, app| {
            surface.update(app, |surface, cx| {
                surface.set_rich_link_failure("https://example.com/page", cx);
            });
        });
        settle(cx);
        cx.update(|_, app| {
            surface.update(app, |surface, _| {
                assert!(
                    surface.rich_link_titles.lookup("https://example.com/page").is_none(),
                    "a failed resolution keeps the authored label"
                );
                assert!(
                    !surface.take_actions().iter().any(|action| matches!(
                        action,
                        ConversationSurfaceAction::ResolveRichLinks { .. }
                    )),
                    "a failed resolution must not be retried"
                );
            });
        });
    }

    #[gpui::test]
    fn group_disclosure_toggle_emits_typed_action(cx: &mut TestAppContext) {
        // Clicking the group's disclosure trigger must emit exactly one
        // typed toggle request; the scene stays authoritative afterwards.
        let open_scene = ConversationScene::build(
            vec![SceneTurn::new(
                turn_id("turn_a"),
                0,
                ConversationLifecycle::Active,
            )],
            vec![item(
                "work-a",
                1,
                SceneItemKind::Activity {
                    body: "first".to_owned(),
                    kind: None,
                    detail: None,
                },
                Some(SceneDisclosure::Open),
            )],
            vec![TurnNarrationEntry::new(
                turn_id("turn_a"),
                TurnNarration::Working,
            )],
            Vec::new(),
        )
        .expect("conversation scene is valid");
        let (surface, cx) = cx.add_window_view(|_, surface_cx| {
            ConversationSurface::new(open_scene, ThemeMode::Dark, surface_cx)
        });
        cx.simulate_resize(size(px(720.0), px(480.0)));
        settle(cx);
        // Trigger bounds key follows the disclosure `-trigger` suffix
        // convention on the group disclosure selector.
        let trigger = cx
            .debug_bounds(
                "artisan-conversation-surface-turn-turn_a-block-work-work-a-disclosure-trigger",
            )
            .expect("disclosure trigger must paint");
        let center = point(
            trigger.origin.x + px(4.0),
            trigger.origin.y + px(4.0),
        );
        cx.simulate_mouse_down(center, gpui::MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(center, gpui::MouseButton::Left, Modifiers::default());
        settle(cx);
        cx.update(|_, app| {
            let actions = surface.read(app).pending_actions().to_vec();
            for action in &actions {
                assert!(
                    matches!(
                        action,
                        ConversationSurfaceAction::ViewportObserved(_)
                            | ConversationSurfaceAction::DisclosureToggleRequested { .. }
                    ),
                    "only viewport observations and the toggle may be pending, got {action:?}"
                );
            }
            let toggles: Vec<(_, _)> = actions
                .iter()
                .filter_map(|action| match action {
                    ConversationSurfaceAction::DisclosureToggleRequested {
                        id,
                        requested_open,
                    } => Some((id.clone(), *requested_open)),
                    _ => None,
                })
                .collect();
            assert_eq!(toggles.len(), 1, "exactly one toggle, got {actions:?}");
            let (id, requested_open) = toggles.into_iter().next().expect("one toggle");
            assert_eq!(id.as_str(), "work-a");
            assert!(!requested_open, "an open group toggles closed");
        });
    }

    /// Builds one live work-group scene whose disclosure is `disclosure`.
    fn disclosure_flight_scene(disclosure: SceneDisclosure) -> ConversationScene {
        ConversationScene::build(
            vec![SceneTurn::new(
                turn_id("turn_a"),
                0,
                ConversationLifecycle::Active,
            )],
            vec![item(
                "work-a",
                1,
                SceneItemKind::Activity {
                    body: body(),
                    kind: None,
                    detail: None,
                },
                Some(disclosure),
            )],
            vec![TurnNarrationEntry::new(
                turn_id("turn_a"),
                TurnNarration::Working,
            )],
            Vec::new(),
        )
        .expect("disclosure flight scene is valid")
    }

    /// Waits out real motion time, then delivers the pending animation frame.
    ///
    /// GPUI's element animation clock is the wall clock, which the test
    /// harness cannot advance through its dispatcher. A real wait guarantees
    /// at least that much elapsed motion before the next frame samples it.
    fn pump_animation_frame_after(cx: &mut VisualTestContext, wait: Duration) {
        std::thread::sleep(wait);
        cx.update(|window, app| {
            window.simulate_next_frame(app);
        });
        settle(cx);
    }

    #[gpui::test]
    fn open_flight_paints_a_partial_height_before_settling_full(cx: &mut TestAppContext) {
        const PANEL: &str =
            "artisan-conversation-surface-turn-turn_a-block-work-work-a-disclosure-panel";
        const CONTENT: &str =
            "artisan-conversation-surface-turn-turn_a-block-work-work-a-disclosure-content";
        let (surface, cx) = cx.add_window_view(|_, surface_cx| {
            ConversationSurface::new(
                disclosure_flight_scene(SceneDisclosure::Closed),
                ThemeMode::Dark,
                surface_cx,
            )
        });
        cx.simulate_resize(size(px(720.0), px(480.0)));
        settle(cx);
        // Settled closed history keeps its rows unmounted (the reference
        // `details_mounted` policy) and its panel measured at zero.
        assert!(
            cx.debug_bounds(CONTENT).is_none(),
            "closed details stay unmounted"
        );
        assert_eq!(
            cx.debug_bounds(PANEL).expect("panel paints").size.height,
            px(0.0)
        );

        cx.update(|_, app| {
            surface.update(app, |surface, surface_cx| {
                surface.replace_scene(
                    disclosure_flight_scene(SceneDisclosure::Open),
                    surface_cx,
                );
            });
        });
        settle(cx);
        // The flip frame mounts the rows at the flight's zero-height start and
        // measures the full target during prepaint.
        let flip = cx.debug_bounds(PANEL).expect("panel paints on the flip frame");
        let full = cx
            .debug_bounds(CONTENT)
            .expect("the open flight mounts its rows");
        assert!(
            f32::from(flip.size.height) < f32::from(full.size.height),
            "the flip frame starts the flight at zero, got {flip:?}"
        );

        let full_height = f32::from(full.size.height);
        pump_animation_frame_after(cx, Duration::from_millis(60));
        let mid_height =
            f32::from(cx.debug_bounds(PANEL).expect("panel paints mid-flight").size.height);
        assert!(
            mid_height > 0.0 && mid_height < full_height,
            "the open flight must pass through a partial height, got {mid_height} of {full_height}"
        );

        pump_animation_frame_after(cx, Duration::from_millis(300));
        let settled = f32::from(cx.debug_bounds(PANEL).expect("panel paints settled").size.height);
        assert!(
            (settled - full_height).abs() < 0.5,
            "the open flight settles at the measured content height, got {settled} of {full_height}"
        );
    }

    #[gpui::test]
    fn collapse_flight_animates_then_settles_unmounted(cx: &mut TestAppContext) {
        const PANEL: &str =
            "artisan-conversation-surface-turn-turn_a-block-work-work-a-disclosure-panel";
        const CONTENT: &str =
            "artisan-conversation-surface-turn-turn_a-block-work-work-a-disclosure-content";
        let (surface, cx) = cx.add_window_view(|_, surface_cx| {
            ConversationSurface::new(
                disclosure_flight_scene(SceneDisclosure::Open),
                ThemeMode::Dark,
                surface_cx,
            )
        });
        cx.simulate_resize(size(px(720.0), px(480.0)));
        settle(cx);
        let full_height = f32::from(
            cx.debug_bounds(CONTENT)
                .expect("an open group mounts its rows")
                .size
                .height,
        );
        let opened = f32::from(cx.debug_bounds(PANEL).expect("panel paints").size.height);
        assert!(
            (opened - full_height).abs() < 0.5,
            "a mounted open group rests at its content height"
        );

        cx.update(|_, app| {
            surface.update(app, |surface, surface_cx| {
                surface.replace_scene(
                    disclosure_flight_scene(SceneDisclosure::Closed),
                    surface_cx,
                );
            });
        });
        settle(cx);
        // The collapse starts from the displayed height rather than snapping.
        let start = f32::from(
            cx.debug_bounds(PANEL)
                .expect("panel paints at the collapse start")
                .size
                .height,
        );
        assert!(
            start > full_height * 0.9 && start <= full_height + 0.5,
            "the collapse flight starts at the displayed height, got {start} of {full_height}"
        );

        pump_animation_frame_after(cx, Duration::from_millis(60));
        let mid_height =
            f32::from(cx.debug_bounds(PANEL).expect("panel paints mid-collapse").size.height);
        assert!(
            mid_height > 0.0 && mid_height < full_height,
            "the collapse flight must pass through a partial height, got {mid_height} of {full_height}"
        );

        pump_animation_frame_after(cx, Duration::from_millis(300));
        assert_eq!(
            cx.debug_bounds(PANEL).expect("panel paints settled").size.height,
            px(0.0)
        );
        // The settled collapse disarms the flight and unmounts the rows again.
        pump_animation_frame_after(cx, Duration::from_millis(20));
        assert!(
            cx.debug_bounds(CONTENT).is_none(),
            "a settled collapse unmounts its rows"
        );
    }

    #[gpui::test]
    fn reduced_motion_jumps_the_disclosure(cx: &mut TestAppContext) {
        const PANEL: &str =
            "artisan-conversation-surface-turn-turn_a-block-work-work-a-disclosure-panel";
        const CONTENT: &str =
            "artisan-conversation-surface-turn-turn_a-block-work-work-a-disclosure-content";
        let (surface, cx) = cx.add_window_view(|_, surface_cx| {
            ConversationSurface::new(
                disclosure_flight_scene(SceneDisclosure::Closed),
                ThemeMode::Dark,
                surface_cx,
            )
        });
        cx.simulate_resize(size(px(720.0), px(480.0)));
        settle(cx);
        cx.update(|_, app| {
            app.set_reduce_motion(true);
        });

        // The system signal: an open flip lands at full height on its frame.
        cx.update(|_, app| {
            surface.update(app, |surface, surface_cx| {
                surface.replace_scene(
                    disclosure_flight_scene(SceneDisclosure::Open),
                    surface_cx,
                );
            });
        });
        settle(cx);
        let full_height = f32::from(
            cx.debug_bounds(CONTENT)
                .expect("reduced motion mounts the rows")
                .size
                .height,
        );
        let opened = f32::from(cx.debug_bounds(PANEL).expect("panel paints").size.height);
        assert!(
            (opened - full_height).abs() < 0.5,
            "reduced motion jumps the open flip, got {opened} of {full_height}"
        );
        cx.update(|_, app| {
            surface.update(app, |surface, surface_cx| {
                surface.replace_scene(
                    disclosure_flight_scene(SceneDisclosure::Closed),
                    surface_cx,
                );
            });
        });
        settle(cx);
        assert_eq!(
            cx.debug_bounds(PANEL).expect("panel paints").size.height,
            px(0.0),
            "reduced motion jumps the close flip"
        );
        assert!(
            cx.debug_bounds(CONTENT).is_none(),
            "reduced motion never mounts settled rows"
        );
        cx.update(|_, app| {
            app.set_reduce_motion(false);
        });

        // The stored explicit override wins over the live system signal.
        cx.update(|_, app| {
            surface.update(app, |surface, surface_cx| {
                surface.set_status_motion(MotionPolicy::Reduced, surface_cx);
                surface.replace_scene(
                    disclosure_flight_scene(SceneDisclosure::Open),
                    surface_cx,
                );
            });
        });
        settle(cx);
        let opened = f32::from(cx.debug_bounds(PANEL).expect("panel paints").size.height);
        assert!(
            (opened - full_height).abs() < 0.5,
            "an explicit reduced preference jumps the open flip, got {opened} of {full_height}"
        );

        // A policy switch midway through a flight settles at the final state
        // and drops the clock: the close must never replay once motion returns.
        cx.update(|_, app| {
            surface.update(app, |surface, surface_cx| {
                surface.set_status_motion(MotionPolicy::Full, surface_cx);
                surface.replace_scene(
                    disclosure_flight_scene(SceneDisclosure::Closed),
                    surface_cx,
                );
            });
        });
        settle(cx);
        cx.update(|_, app| {
            app.set_reduce_motion(true);
        });
        settle(cx);
        assert_eq!(
            cx.debug_bounds(PANEL).expect("panel paints").size.height,
            px(0.0),
            "a reduced-motion switch settles the in-flight close"
        );
        cx.update(|_, app| {
            app.set_reduce_motion(false);
        });
        pump_animation_frame_after(cx, Duration::from_millis(60));
        assert_eq!(
            cx.debug_bounds(PANEL).expect("panel paints").size.height,
            px(0.0),
            "a settled reduced-motion close never replays"
        );
    }

    #[test]
    fn legacy_group_details_render_in_durable_order() {
        // Legacy positional groups (no provenance) keep vec order with
        // reasoning stripped but its positional slot retained, so surviving
        // rows keep their original ordinals; this holds under both scene
        // generations because session mode never carries these inputs.
        let scene = ConversationScene::build(
            vec![SceneTurn::new(
                turn_id("turn_a"),
                0,
                ConversationLifecycle::Active,
            )],
            vec![
                item(
                    "work-a",
                    1,
                    SceneItemKind::Activity {
                        body: "first".to_owned(),
                        kind: None,
                        detail: None,
                    },
                    None,
                ),
                item(
                    "work-r",
                    2,
                    SceneItemKind::ReasoningSummary {
                        body: "hidden thought.".to_owned(),
                    },
                    None,
                ),
                item(
                    "work-b",
                    3,
                    SceneItemKind::Activity {
                        body: "second".to_owned(),
                        kind: None,
                        detail: None,
                    },
                    None,
                ),
            ],
            vec![TurnNarrationEntry::new(
                turn_id("turn_a"),
                TurnNarration::Working,
            )],
            Vec::new(),
        )
        .expect("conversation scene is valid");
        let turn = scene.turn_scene(&turn_id("turn_a")).expect("turn present");
        let group = turn
            .blocks()
            .iter()
            .find_map(|block| match block {
                TurnBlock::WorkGroup(group) => Some(group),
                _ => None,
            })
            .expect("work group present");
        let rows = ordered_detail_rows(group);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, 0);
        assert!(matches!(rows[0].1, DetailRow::Activity { .. }));
        assert_eq!(rows[1].0, 2);
        assert!(matches!(rows[1].1, DetailRow::Activity { .. }));
    }

    #[test]
    fn session_details_sort_by_stable_ordinal() {
        use crate::conversation_scene::{ProgressPhase, SessionDetail};
        let group = WorkGroupBlock {
            items: Vec::new(),
            label: None,
            disclosure: None,
            session: Some(scene_id("session-turn_a")),
            session_run: None,
            superseded: false,
            reasoning_summary: None,
            progress: ProgressPhase::Work,
            transition: None,
            session_details: vec![
                SessionDetail::Activity {
                    id: scene_id("act-2"),
                    body: "second".to_owned(),
                    kind: None,
                    detail: None,
                    lifecycle: None,
                    ordinal: 4,
                    disclosure: None,
                },
                SessionDetail::Assistant {
                    id: scene_id("asst-1"),
                    body: "first".to_owned(),
                    phase: AssistantPhase::Commentary,
                    ordinal: 2,
                    provenance: None,
                    disclosure: None,
                },
                SessionDetail::NativeFact {
                    id: scene_id("fact-3"),
                    text: "third".to_owned(),
                    ordinal: 6,
                    disclosure: None,
                },
            ],
        };
        let rows = ordered_detail_rows(&group);
        assert_eq!(
            rows.iter().map(|(ordinal, _)| *ordinal).collect::<Vec<_>>(),
            vec![2, 4, 6]
        );
        assert!(matches!(rows[0].1, DetailRow::Assistant { .. }));
        assert!(matches!(rows[1].1, DetailRow::Activity { .. }));
        assert!(matches!(rows[2].1, DetailRow::NativeFact { .. }));
    }

    #[test]
    fn footer_paints_only_with_settlement() {
        let without = TurnFooterBlock {
            turn_id: turn_id("turn_a"),
            settlement: None,
        };
        assert!(!footer_has_content(&without));
        assert!(footer_settlement(&without).is_none());
    }

    #[test]
    fn footer_keys_are_stable_per_turn() {
        assert_eq!(footer_key(&turn_id("turn_a")), "turn-footer:turn_a");
        assert_ne!(footer_key(&turn_id("turn_a")), footer_key(&turn_id("turn_b")));
    }

    #[test]
    fn footer_hover_reveal_survives_the_trip_from_the_turn() {
        let mut revealed = None;
        // The pointer arrives over the footer after the turn group's own hover
        // has already dropped.
        assert!(footer_hover_transition(
            &mut revealed,
            "turn-footer:turn_a",
            true
        ));
        assert_eq!(revealed.as_deref(), Some("turn-footer:turn_a"));
        // A repeat observation is not a change.
        assert!(!footer_hover_transition(
            &mut revealed,
            "turn-footer:turn_a",
            true
        ));
        // Another turn's footer leaving must not clear the retained key.
        assert!(!footer_hover_transition(
            &mut revealed,
            "turn-footer:turn_b",
            false
        ));
        assert_eq!(revealed.as_deref(), Some("turn-footer:turn_a"));
        // Leaving the footer releases it.
        assert!(footer_hover_transition(
            &mut revealed,
            "turn-footer:turn_a",
            false
        ));
        assert!(revealed.is_none());
    }

    #[test]
    fn scene_block_order_is_preserved_with_conditional_paint() {
        let scene = ConversationScene::build(
            vec![SceneTurn::new(
                turn_id("turn_a"),
                0,
                ConversationLifecycle::Completed,
            )],
            vec![
                item(
                    "user-a",
                    1,
                    SceneItemKind::UserMessage {
                        body: "hi".to_owned(),
                    },
                    None,
                ),
                item(
                    "assistant-a",
                    2,
                    SceneItemKind::AssistantMessage {
                        body: "hello".to_owned(),
                        phase: AssistantPhase::Final,
                    },
                    None,
                ),
            ],
            vec![TurnNarrationEntry::new(
                turn_id("turn_a"),
                TurnNarration::Working,
            )],
            Vec::new(),
        )
        .expect("conversation scene is valid");
        assert_eq!(
            ordered_block_kinds(&scene),
            vec![
                RenderedBlockKind::UserMessage,
                RenderedBlockKind::AssistantMessage,
                RenderedBlockKind::TurnStatus,
                RenderedBlockKind::TurnFooter,
            ]
        );
    }

    #[gpui::test]
    fn status_motion_defaults_to_full_with_reduced_override(
        cx: &mut TestAppContext,
    ) {
        let (surface, cx) = cx.add_window_view(|_, surface_cx| {
            ConversationSurface::new(scene(Vec::new()), ThemeMode::Dark, surface_cx)
        });
        cx.update(|_, app| {
            assert_eq!(surface.read(app).status_motion(), MotionPolicy::Full);
        });
        cx.update(|_, app| {
            surface.update(app, |surface, surface_cx| {
                surface.set_status_motion(MotionPolicy::Reduced, surface_cx);
            });
        });
        cx.update(|_, app| {
            assert_eq!(
                surface.read(app).status_motion(),
                MotionPolicy::Reduced
            );
        });
    }

    #[test]
    fn effective_status_motion_matrix() {
        use artisan_ui::shimmer_text::{ShimmerMotionPlan, ShimmerText};
        use artisan_ui::theme::ArtisanTheme;
        let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
        // Stored Full follows the live system signal.
        assert_eq!(
            effective_status_motion(MotionPolicy::Full, false),
            MotionPolicy::Full
        );
        assert_eq!(
            effective_status_motion(MotionPolicy::Full, true),
            MotionPolicy::Reduced
        );
        // An explicit Reduced override always wins.
        assert_eq!(
            effective_status_motion(MotionPolicy::Reduced, false),
            MotionPolicy::Reduced
        );
        assert_eq!(
            effective_status_motion(MotionPolicy::Reduced, true),
            MotionPolicy::Reduced
        );
        // Live rows animate only under effective Full; settled rows and
        // reduced motion always resolve to the immediate static path.
        let animate = ShimmerText::new("Thinking", theme, MotionPolicy::Full)
            .active(true)
            .motion_plan();
        assert!(matches!(animate, ShimmerMotionPlan::Animate(_)));
        let still = ShimmerText::new("Thinking", theme, MotionPolicy::Reduced)
            .active(true)
            .motion_plan();
        assert!(matches!(still, ShimmerMotionPlan::Immediate));
        let settled = ShimmerText::new("Worked for 1m 5s", theme, MotionPolicy::Full)
            .active(false)
            .motion_plan();
        assert!(matches!(settled, ShimmerMotionPlan::Immediate));
    }

    #[test]
    fn present_shell_command_recovers_the_wrapped_command() {
        // WindowsApps pwsh with `-Command` and a quoted inner script.
        assert_eq!(
            present_shell_command(
                r#""C:\Program Files\WindowsApps\Microsoft.PowerShell\pwsh.exe" -Command "cargo test --locked""#,
            ),
            "cargo test --locked"
        );
        // PowerShell's short flag, single-quoted body.
        assert_eq!(
            present_shell_command(r#"pwsh -c 'git status --short'"#),
            "git status --short"
        );
        // bash -lc.
        assert_eq!(
            present_shell_command(r#"bash -lc "cargo check -p artisan-frontend""#),
            "cargo check -p artisan-frontend"
        );
        // cmd /c and /k read case-insensitively with `.exe` stripped.
        assert_eq!(present_shell_command(r#"cmd.exe /C "dir /b""#), "dir /b");
        assert_eq!(
            present_shell_command(r#"C:\Windows\System32\cmd.exe /K ver"#),
            "ver"
        );
    }

    #[test]
    fn present_shell_command_keeps_what_it_cannot_recognise() {
        // Unrecognised input is returned collapsed, never guessed at.
        assert_eq!(
            present_shell_command("cargo   test\n--locked"),
            "cargo test --locked"
        );
        assert_eq!(
            present_shell_command(r#""C:\tools\runner.exe" --work"#),
            r#""C:\tools\runner.exe" --work"#
        );
        // A recognised wrapper with a different flag stays whole.
        assert_eq!(
            present_shell_command("bash --login -c script.sh"),
            "bash --login -c script.sh"
        );
        // An empty quoted body falls back to the collapsed invocation.
        assert_eq!(
            present_shell_command(r#"pwsh -Command """#),
            r#"pwsh -Command """#
        );
        // An unterminated quote still yields the wrapper's body, exactly
        // like the reference unquote (which only drops a matching pair).
        assert_eq!(
            present_shell_command(r#"pwsh -Command "unterminated"#),
            "\"unterminated"
        );
    }

    #[test]
    fn activity_chain_clauses_describe_composition() {
        assert_eq!(activity_chain_clause(["bash"].into_iter()), "Ran a command");
        assert_eq!(
            activity_chain_clause(["read", "read"].into_iter()),
            "Read 2 files"
        );
        assert_eq!(
            activity_chain_clause(["bash", "read", "read"].into_iter()),
            "Ran a command, read 2 files"
        );
        // A kind-less legacy row counts as generic tool work.
        assert_eq!(activity_chain_clause(["tool"].into_iter()), "Used a tool");
        assert_eq!(activity_chain_clause(std::iter::empty()), "");
    }

    #[test]
    fn activity_chain_icons_follow_the_reference_category_map() {
        assert_eq!(
            activity_chain_icon(["bash"].into_iter()),
            AssetId::TABLER_TERMINAL_2
        );
        assert_eq!(
            activity_chain_icon(["read"].into_iter()),
            AssetId::TABLER_FILE_TEXT
        );
        assert_eq!(
            activity_chain_icon(["edit"].into_iter()),
            AssetId::TABLER_FILE_PENCIL
        );
        assert_eq!(
            activity_chain_icon(["file.delete"].into_iter()),
            AssetId::TABLER_FILE_X
        );
        assert_eq!(
            activity_chain_icon(["grep"].into_iter()),
            AssetId::TABLER_FILE_SEARCH
        );
        assert_eq!(
            activity_chain_icon(["search"].into_iter()),
            AssetId::TABLER_WORLD_SEARCH
        );
        assert_eq!(
            activity_chain_icon(["mcp"].into_iter()),
            AssetId::TABLER_TOOL
        );
        assert_eq!(
            activity_chain_icon(["bash", "read"].into_iter()),
            AssetId::TABLER_LIST_DETAILS
        );
        // Two distinct kinds sharing one category stay homogeneous.
        assert_eq!(
            activity_chain_icon(["read", "file"].into_iter()),
            AssetId::TABLER_FILE_TEXT
        );
    }

    #[test]
    fn activity_detail_text_normalizes_terminal_and_falls_back_to_labels() {
        assert_eq!(
            activity_detail_text(
                "terminal_activity",
                Some(r#""C:\Program Files\WindowsApps\pwsh.exe" -Command "cargo test""#),
                None,
            ),
            "cargo test"
        );
        assert_eq!(
            activity_detail_text(
                "read",
                Some("src/main.rs"),
                Some(ConversationLifecycle::Completed)
            ),
            "src/main.rs"
        );
        assert_eq!(
            activity_detail_text("read", None, Some(ConversationLifecycle::Completed)),
            "Read a file"
        );
        assert_eq!(
            activity_detail_text("read", None, Some(ConversationLifecycle::Active)),
            "Reading a file"
        );
        assert_eq!(
            activity_detail_text(
                "terminal_activity",
                None,
                Some(ConversationLifecycle::Failed)
            ),
            "Command failed"
        );
        // A terminal detail that normalizes to nothing falls back.
        assert_eq!(
            activity_detail_text(
                "terminal_activity",
                Some("   "),
                Some(ConversationLifecycle::Completed),
            ),
            "Ran a command"
        );
    }

    #[gpui::test]
    fn status_shimmer_tracks_system_reduced_motion(cx: &mut TestAppContext) {
        let thinking = ConversationScene::build(
            vec![SceneTurn::new(
                turn_id("turn_a"),
                0,
                ConversationLifecycle::Active,
            )],
            Vec::new(),
            vec![TurnNarrationEntry::new(
                turn_id("turn_a"),
                TurnNarration::Thinking,
            )],
            Vec::new(),
        )
        .expect("conversation scene is valid");
        let (surface, cx) = cx.add_window_view(|_, surface_cx| {
            ConversationSurface::new(thinking, ThemeMode::Dark, surface_cx)
        });
        settle(cx);
        cx.update(|_, app| {
            app.set_reduce_motion(true);
        });
        settle(cx);
        cx.update(|_, app| {
            // The system signal never mutates the stored preference and
            // emits no surface actions on its own: render only reads it.
            assert_eq!(surface.read(app).status_motion(), MotionPolicy::Full);
            assert!(surface.read(app).pending_actions().is_empty());
        });
        cx.update(|_, app| {
            app.set_reduce_motion(false);
        });
        settle(cx);
        cx.update(|_, app| {
            assert_eq!(surface.read(app).status_motion(), MotionPolicy::Full);
            assert!(surface.read(app).pending_actions().is_empty());
        });
    }

    #[gpui::test]
    fn user_body_drag_selects_and_copies_exact_bytes(cx: &mut TestAppContext) {
        // Real pointer drag across the painted user body, then the platform
        // copy keystroke: the clipboard must carry the exact body bytes and
        // no observation may cross the surface action boundary.
        const BODY: &str = "selectable proof body";
        let user_scene = ConversationScene::build(
            vec![SceneTurn::new(
                turn_id("turn_a"),
                0,
                ConversationLifecycle::Active,
            )],
            vec![item(
                "user-a",
                1,
                SceneItemKind::UserMessage {
                    body: BODY.to_owned(),
                },
                None,
            )],
            vec![TurnNarrationEntry::new(
                turn_id("turn_a"),
                TurnNarration::Quiet,
            )],
            Vec::new(),
        )
        .expect("conversation scene is valid");
        let (surface, cx) = cx.add_window_view(|_, surface_cx| {
            ConversationSurface::new(user_scene, ThemeMode::Dark, surface_cx)
        });
        cx.simulate_resize(size(px(720.0), px(240.0)));
        settle(cx);
        // Static literal: debug_bounds takes &'static str. Verified against
        // the selector contract: turn_selector appends "-turn-turn_a" to the
        // surface root, the user arm appends "-block-user-user-a", and the
        // body container appends "-body".
        let bounds = cx
            .debug_bounds("artisan-conversation-surface-turn-turn_a-block-user-user-a-body")
            .expect("user body must paint");
        let left = point(bounds.origin.x + px(1.0), bounds.origin.y + px(10.0));
        // The head lands past the text end on the LAST line inside the
        // bubble padding, so the layout clamps it to the exact whole body:
        // an endpoint on a last glyph resolves to that char start and drops
        // the final character, and a single-line head would miss wrapped
        // lines below it entirely.
        let right = point(
            bounds.origin.x + bounds.size.width + px(8.0),
            bounds.origin.y + bounds.size.height - px(10.0),
        );
        cx.simulate_mouse_down(left, gpui::MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(right, gpui::MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(right, gpui::MouseButton::Left, Modifiers::default());
        cx.simulate_keystrokes("ctrl-c");
        settle(cx);

        let copied = cx.update(|_, app| {
            app.read_from_clipboard()
                .as_ref()
                .and_then(gpui::ClipboardItem::text)
        });
        assert_eq!(copied, Some(BODY.to_owned()));
        cx.update(|_, app| {
            surface.update(app, |surface, _| {
                for action in surface.take_actions() {
                    assert!(
                        matches!(
                            action,
                            ConversationSurfaceAction::ViewportObserved(_)
                        ),
                        "only legitimate viewport observations may precede assertions, got {action:?}"
                    );
                }
            });
            assert!(surface.read(app).pending_actions().is_empty());
        });
    }

    #[gpui::test]
    fn scene_scroll_target_executes_against_rendered_group_root(cx: &mut TestAppContext) {
        let (surface, cx) = cx.add_window_view(|_, surface_cx| {
            ConversationSurface::new(
                scroll_target_scene(SceneDisclosure::Open),
                ThemeMode::Dark,
                surface_cx,
            )
        });
        cx.simulate_resize(size(px(720.0), px(240.0)));
        settle(cx);
        let before = offset(&surface, cx);

        cx.update(|_, app| {
            surface.update(app, |surface, surface_cx| {
                assert!(surface.schedule_scroll_target(
                    ConversationSurfaceTarget::Scene(scene_id("work-first")),
                    surface_cx,
                ));
            });
        });
        settle(cx);

        let after = offset(&surface, cx);
        assert!(
            after.y < before.y,
            "the rendered work group must be reached"
        );
        cx.update(|_, app| assert!(surface.read(app).pending_scroll_targets.is_empty()));
    }

    #[gpui::test]
    fn item_scroll_target_executes_against_rendered_work_item_root(cx: &mut TestAppContext) {
        let (surface, cx) = cx.add_window_view(|_, surface_cx| {
            ConversationSurface::new(
                scroll_target_scene(SceneDisclosure::Open),
                ThemeMode::Dark,
                surface_cx,
            )
        });
        cx.simulate_resize(size(px(720.0), px(240.0)));
        settle(cx);
        let before = offset(&surface, cx);

        cx.update(|_, app| {
            surface.update(app, |surface, surface_cx| {
                assert!(surface.schedule_scroll_target(
                    ConversationSurfaceTarget::Item(
                        ItemId::parse("work-target").expect("item id is valid"),
                    ),
                    surface_cx,
                ));
            });
        });
        settle(cx);

        let after = offset(&surface, cx);
        assert!(after.y < before.y, "the rendered work item must be reached");
        cx.update(|_, app| assert!(surface.read(app).pending_scroll_targets.is_empty()));
    }

    #[gpui::test]
    fn work_group_header_row_wraps_the_disclosure_trigger(cx: &mut TestAppContext) {
        const HEADER: &str =
            "artisan-conversation-surface-turn-turn_a-block-work-work-first-header";
        const TRIGGER: &str =
            "artisan-conversation-surface-turn-turn_a-block-work-work-first-disclosure-trigger";
        let (_surface, cx) = cx.add_window_view(|_, surface_cx| {
            ConversationSurface::new(
                scroll_target_scene(SceneDisclosure::Open),
                ThemeMode::Dark,
                surface_cx,
            )
        });
        cx.simulate_resize(size(px(720.0), px(240.0)));
        settle(cx);
        let header = cx
            .debug_bounds(HEADER)
            .expect("controlled header row must paint");
        let trigger = cx
            .debug_bounds(TRIGGER)
            .expect("disclosure trigger must paint");
        assert!(
            trigger.origin.y >= header.origin.y
                && trigger.origin.y + trigger.size.height <= header.origin.y + header.size.height,
            "the trigger lives inside the shared header row"
        );
    }

    #[gpui::test]
    fn work_group_header_row_paints_without_a_disclosure_wrapper(cx: &mut TestAppContext) {
        const HEADER: &str =
            "artisan-conversation-surface-turn-turn_a-block-work-plain-first-header";
        let (_surface, cx) = cx.add_window_view(|_, surface_cx| {
            ConversationSurface::new(
                scene(vec![
                    item(
                        "plain-first",
                        1,
                        SceneItemKind::Activity {
                            body: body(),
                            kind: None,
                            detail: None,
                        },
                        None,
                    ),
                    item(
                        "plain-target",
                        2,
                        SceneItemKind::Activity {
                            body: body(),
                            kind: None,
                            detail: None,
                        },
                        None,
                    ),
                ]),
                ThemeMode::Dark,
                surface_cx,
            )
        });
        cx.simulate_resize(size(px(720.0), px(240.0)));
        settle(cx);
        assert!(
            cx.debug_bounds(HEADER).is_some(),
            "the plain header row paints without a disclosure wrapper"
        );
    }

    fn tall_navigator_scene() -> ConversationScene {
        // Turn ordinals (0, 1) share the global ordinal namespace with
        // items, so item ordinals continue after them.
        let long_body = (0..40)
            .map(|line| format!("Navigator line {line} fills the viewport for tracking."))
            .collect::<Vec<_>>()
            .join("\n");
        ConversationScene::build(
            vec![
                SceneTurn::new(turn_id("turn_a"), 0, ConversationLifecycle::Completed),
                SceneTurn::new(turn_id("turn_b"), 1, ConversationLifecycle::Completed),
            ],
            vec![
                SceneItem::new(
                    scene_id("nav-first"),
                    turn_id("turn_a"),
                    2,
                    SceneItemKind::UserMessage {
                        body: long_body.clone(),
                    },
                    None,
                )
                .expect("first navigator item is valid"),
                SceneItem::new(
                    scene_id("nav-first-reply"),
                    turn_id("turn_a"),
                    3,
                    SceneItemKind::AssistantMessage {
                        body: "first reply".to_owned(),
                        phase: AssistantPhase::Final,
                    },
                    None,
                )
                .expect("first reply is valid"),
                SceneItem::new(
                    scene_id("nav-second"),
                    turn_id("turn_b"),
                    4,
                    SceneItemKind::UserMessage { body: long_body },
                    None,
                )
                .expect("second navigator item is valid"),
                SceneItem::new(
                    scene_id("nav-second-reply"),
                    turn_id("turn_b"),
                    5,
                    SceneItemKind::AssistantMessage {
                        body: "second reply".to_owned(),
                        phase: AssistantPhase::Final,
                    },
                    None,
                )
                .expect("second reply is valid"),
            ],
            vec![
                TurnNarrationEntry::new(turn_id("turn_a"), TurnNarration::Quiet),
                TurnNarrationEntry::new(turn_id("turn_b"), TurnNarration::Quiet),
            ],
            Vec::new(),
        )
        .expect("tall navigator scene is valid")
    }

    #[gpui::test]
    fn reasoning_only_group_has_no_working_disclosure(cx: &mut TestAppContext) {
        // Disclosure is registered but the group holds no visible trace
        // content: reasoning is stripped from visible details. The disabled
        // wrapper keeps stable ancestry (its selector still paints), but no
        // chevron control exists and neither click nor Enter may emit a
        // toggle — this is the no-empty-collapse requirement, evidenced by
        // behavior rather than selector absence.
        const TRIGGER: &str =
            "artisan-conversation-surface-turn-turn_a-block-work-thinking-only-disclosure-trigger";
        let (surface, cx) = cx.add_window_view(|_, surface_cx| {
            ConversationSurface::new(
                scene(vec![item(
                    "thinking-only",
                    1,
                    SceneItemKind::ReasoningSummary {
                        body: "stripped".to_owned(),
                    },
                    Some(SceneDisclosure::Open),
                )]),
                ThemeMode::Dark,
                surface_cx,
            )
        });
        cx.simulate_resize(size(px(720.0), px(240.0)));
        settle(cx);
        let trigger = cx
            .debug_bounds(TRIGGER)
            .expect("disabled wrapper keeps stable ancestry");
        cx.simulate_click(trigger.center(), Modifiers::none());
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.update(|_, app| {
            let toggles: Vec<ConversationSurfaceAction> =
                surface.update(app, |surface, _| surface.take_actions());
            assert!(
                toggles.iter().all(|action| !matches!(
                    action,
                    ConversationSurfaceAction::DisclosureToggleRequested { .. }
                )),
                "an empty group must never emit a disclosure toggle, got {toggles:?}"
            );
        });
    }

    #[gpui::test]
    fn navigator_active_tracks_the_reader_not_the_tail(cx: &mut TestAppContext) {
        const FIRST_TICK: &str =
            "artisan-conversation-surface-turn-navigator-control-nav-first-tick";
        const SECOND_TICK: &str =
            "artisan-conversation-surface-turn-navigator-control-nav-second-tick";
        let (surface, cx) = cx.add_window_view(|_, surface_cx| {
            ConversationSurface::new(tall_navigator_scene(), ThemeMode::Dark, surface_cx)
        });
        cx.simulate_resize(size(px(720.0), px(480.0)));
        settle(cx);
        // At the top the first turn owns the position, not the tail.
        let first_tick = cx.debug_bounds(FIRST_TICK).expect("first tick paints");
        let second_tick = cx.debug_bounds(SECOND_TICK).expect("second tick paints");
        assert_eq!(first_tick.size.width, px(24.0));
        assert_eq!(second_tick.size.width, px(16.0));
        // Scrolled to the end the second turn takes over.
        let handle = cx.update(|_, app| surface.read(app).scroll_handle().clone());
        let maximum = cx.update(|_, app| surface.read(app).scroll_handle().max_offset().y);
        assert!(maximum > px(0.0), "the tall fixture must scroll");
        handle.set_offset(point(px(0.0), -maximum));
        // `set_offset` writes the shared scroll state without scheduling a
        // frame; production scroll paths notify, so request the repaint that
        // the geometry-derived active marker rides on.
        cx.update(|_, app| surface.update(app, |_, cx| cx.notify()));
        settle(cx);
        let first_tick = cx.debug_bounds(FIRST_TICK).expect("first tick paints");
        let second_tick = cx.debug_bounds(SECOND_TICK).expect("second tick paints");
        assert_eq!(first_tick.size.width, px(16.0));
        assert_eq!(second_tick.size.width, px(24.0));
    }

    #[gpui::test]
    fn transcript_wheel_queues_bounded_target_then_settles(cx: &mut TestAppContext) {
        // Manual frame pump, stated honestly: the test harness never runs
        // `on_next_frame` callbacks on dirty draws, so parked frames alone
        // cannot advance the smoothing clock. Each pumped frame calls the
        // same `advance_transcript_scroll` the production callback runs.
        let body = (0..40)
            .map(|line| format!("Wheel line {line} makes the transcript scrollable."))
            .collect::<Vec<_>>()
            .join("\n");
        let (surface, cx) = cx.add_window_view(|_, surface_cx| {
            ConversationSurface::new(
                scene(vec![item(
                    "tall-user",
                    1,
                    SceneItemKind::UserMessage { body },
                    None,
                )]),
                ThemeMode::Dark,
                surface_cx,
            )
        });
        cx.simulate_resize(size(px(720.0), px(240.0)));
        settle(cx);
        let maximum = cx.update(|_, app| surface.read(app).scroll_handle().max_offset().y);
        assert!(maximum > px(0.0), "the tall fixture must scroll");
        let before = offset(&surface, cx);
        let viewport = cx
            .debug_bounds(CONVERSATION_VIEWPORT_SELECTOR)
            .expect("viewport paints");
        cx.simulate_event(ScrollWheelEvent {
            position: viewport.center(),
            delta: ScrollDelta::Lines(point(0.0f32, -3.0f32)),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::default(),
        });
        // No synchronous jump: smoothing queues a bounded target with
        // frames still pending.
        assert_eq!(offset(&surface, cx), before);
        let target = cx.update(|_, app| {
            let surface_ref = surface.read(app);
            assert!(
                surface_ref.transcript_scroll.active(),
                "a coarse wheel tick must leave an interpolation target outstanding"
            );
            surface_ref.transcript_scroll.target()
        });
        // Pump bounded frames with draws between steps: the first frame
        // lands strictly between start and target, and the run settles
        // exactly onto the queued target before retiring it.
        let pump_frame = |cx: &mut VisualTestContext| {
            cx.update(|window, app| {
                surface.update(app, |surface, surface_cx| {
                    surface.advance_transcript_scroll(window, surface_cx);
                });
            });
            cx.run_until_parked();
        };
        pump_frame(cx);
        let mid = offset(&surface, cx);
        assert!(
            mid != before && f32::from(mid.y) != target,
            "the first pumped frame must sit between start and target"
        );
        for _ in 0..64 {
            let settled_now = cx.update(|_, app| !surface.read(app).transcript_scroll.active());
            if settled_now {
                break;
            }
            pump_frame(cx);
        }
        let settled = offset(&surface, cx);
        assert_eq!(
            f32::from(settled.y),
            target,
            "smoothing must settle exactly onto the queued target"
        );
        cx.update(|_, app| {
            assert!(
                !surface.read(app).transcript_scroll.active(),
                "settling must retire the interpolation target"
            );
        });
    }

    #[gpui::test]
    fn provider_wait_paints_without_group_or_disclosure(cx: &mut TestAppContext) {
        // Pre-response state: user plus wait narration, no assistant and no
        // work group. The status row still narrates the wait, and with no
        // group there is no header, trigger, or fabricated detail anywhere.
        const STATUS: &str = "artisan-conversation-surface-turn-turn_a-status";
        const NO_GROUP_TRIGGER: &str =
            "artisan-conversation-surface-turn-turn_a-block-work-turn_a-disclosure-trigger";
        let scene = ConversationScene::build(
            vec![SceneTurn::new(
                turn_id("turn_a"),
                0,
                ConversationLifecycle::Active,
            )],
            vec![item(
                "user-a",
                1,
                SceneItemKind::UserMessage {
                    body: "hi".to_owned(),
                },
                None,
            )],
            vec![TurnNarrationEntry::new(
                turn_id("turn_a"),
                TurnNarration::ProviderWait,
            )],
            Vec::new(),
        )
        .expect("waiting scene is valid");
        let (surface, cx) = cx.add_window_view(|_, surface_cx| {
            ConversationSurface::new(scene, ThemeMode::Dark, surface_cx)
        });
        cx.simulate_resize(size(px(720.0), px(240.0)));
        settle(cx);
        assert!(
            cx.debug_bounds(STATUS).is_some(),
            "the wait line narrates before any work exists"
        );
        assert!(
            cx.debug_bounds(NO_GROUP_TRIGGER).is_none(),
            "no disclosure control without a work group"
        );
        cx.update(|_, app| {
            assert!(
                !ordered_block_kinds(surface.read(app).scene())
                    .contains(&RenderedBlockKind::WorkGroup),
                "no fabricated work detail"
            );
        });
    }

    #[test]
    fn navigator_turn_top_accounts_for_host_header_offset() {
        // Standalone tests see a zero viewport origin and would miss the
        // parent offset, so the rebasing math carries an explicit case: a
        // turn 300 px down the content, scrolled 100 px, under a host
        // header offsetting the viewport 50 px, sits 150 px past the
        // viewport top.
        assert_eq!(
            ConversationSurface::navigator_turn_top_viewport(300.0, 100.0, 50.0),
            150.0
        );
        assert_eq!(
            ConversationSurface::navigator_turn_top_viewport(300.0, 100.0, 0.0),
            200.0
        );
    }

    #[gpui::test]
    fn long_navigator_list_centers_the_cap_not_the_content(cx: &mut TestAppContext) {
        // Thirty turns overflow the 70 % cap once expanded: the rail must
        // center the capped box instead of pinning a full-content top at zero.
        let turns: Vec<SceneTurn> = (0..30)
            .map(|index| {
                SceneTurn::new(
                    turn_id(&format!("turn_{index:02}")),
                    index as u64,
                    ConversationLifecycle::Completed,
                )
            })
            .collect();
        let mut items = Vec::new();
        for (index, turn) in turns.iter().enumerate() {
            items.push(
                SceneItem::new(
                    scene_id(&format!("nq{index:02}")),
                    turn.turn_id.clone(),
                    (turns.len() + index + 1) as u64,
                    SceneItemKind::UserMessage {
                        body: format!("question {index}"),
                    },
                    None,
                )
                .expect("navigator item is valid"),
            );
        }
        let narrations = turns
            .iter()
            .map(|turn| TurnNarrationEntry::new(turn.turn_id.clone(), TurnNarration::Quiet))
            .collect();
        let scene = ConversationScene::build(turns, items, narrations, Vec::new())
            .expect("long navigator scene is valid");
        let (surface, cx) = cx.add_window_view(|_, surface_cx| {
            ConversationSurface::new(scene, ThemeMode::Dark, surface_cx)
        });
        cx.simulate_resize(size(px(720.0), px(480.0)));
        // The harness does not always repaint just because the window resized;
        // notify once so the rail metrics measure the 480 px viewport before
        // the pointer targets the rail. Without this the rail still carries
        // its creation-frame position at the maximized window height.
        cx.update(|_, app| surface.update(app, |_, cx| cx.notify()));
        settle(cx);
        let rail = cx
            .debug_bounds(TURN_NAVIGATOR_SELECTOR)
            .expect("rail paints");
        cx.simulate_mouse_move(rail.center(), None::<gpui::MouseButton>, Modifiers::none());
        // Two geometry passes follow the reveal: the probe reports the capped
        // list height, then the metrics listener re-centers the rail from it.
        settle(cx);
        let expanded = cx
            .debug_bounds(TURN_NAVIGATOR_SELECTOR)
            .expect("expanded rail paints");
        let viewport = cx
            .debug_bounds(CONVERSATION_VIEWPORT_SELECTOR)
            .expect("viewport paints");
        let height = f64::from(expanded.size.height);
        let top = f64::from(expanded.origin.y) - f64::from(viewport.origin.y);
        assert!(
            (height - 336.0).abs() <= 4.0,
            "the expanded list caps at 70 % of the 480 px viewport, got {height}"
        );
        assert!(
            (top - 72.0).abs() <= 4.0,
            "the capped rail centers instead of pinning top at zero, got {top}"
        );
    }

    #[gpui::test]
    fn transcript_turns_keep_the_centered_prose_column(cx: &mut TestAppContext) {
        // The screen mounts the host full-bleed; prose rhythm lives on the
        // turn roots themselves, so a wide standalone surface centers the
        // same 768 px column the composer card keeps.
        const TURN_A: &str = "artisan-conversation-surface-turn-turn_a";
        let (_surface, cx) = cx.add_window_view(|_, surface_cx| {
            ConversationSurface::new(tall_navigator_scene(), ThemeMode::Dark, surface_cx)
        });
        cx.simulate_resize(size(px(1200.0), px(800.0)));
        settle(cx);
        let root = cx
            .debug_bounds(CONVERSATION_SURFACE_SELECTOR)
            .expect("surface root lays out");
        let turn = cx.debug_bounds(TURN_A).expect("turn lays out");
        assert!(
            f64::from(turn.size.width) <= 769.0,
            "turn keeps the prose max width"
        );
        let turn_center = f64::from(turn.origin.x) + f64::from(turn.size.width) / 2.0;
        let root_center = f64::from(root.origin.x) + f64::from(root.size.width) / 2.0;
        assert!(
            (turn_center - root_center).abs() <= 1.0,
            "turn centers in the surface"
        );
    }

    #[gpui::test]
    fn stale_or_unmounted_scroll_targets_are_benign_no_ops(cx: &mut TestAppContext) {
        let (surface, cx) = cx.add_window_view(|_, surface_cx| {
            ConversationSurface::new(
                scroll_target_scene(SceneDisclosure::Closed),
                ThemeMode::Dark,
                surface_cx,
            )
        });
        cx.simulate_resize(size(px(720.0), px(240.0)));
        settle(cx);
        let before = offset(&surface, cx);

        cx.update(|_, app| {
            surface.update(app, |surface, surface_cx| {
                assert!(surface.schedule_scroll_target(
                    ConversationSurfaceTarget::Scene(scene_id("not-rendered")),
                    surface_cx,
                ));
                assert!(surface.schedule_scroll_target(
                    ConversationSurfaceTarget::Item(
                        ItemId::parse("work-target").expect("item id is valid"),
                    ),
                    surface_cx,
                ));
            });
        });
        settle(cx);

        assert_eq!(offset(&surface, cx), before);
        cx.update(|_, app| assert!(surface.read(app).pending_scroll_targets.is_empty()));
    }

    #[gpui::test]
    fn scroll_target_queue_retains_fifo_head_at_bounded_capacity(cx: &mut TestAppContext) {
        let (surface, cx) = cx.add_window_view(|_, surface_cx| {
            ConversationSurface::new(scene(Vec::new()), ThemeMode::Dark, surface_cx)
        });
        let first = ConversationSurfaceTarget::Scene(scene_id("queued-0"));
        cx.update(|_, app| {
            surface.update(app, |surface, surface_cx| {
                for index in 0..CONVERSATION_SURFACE_MAX_SCROLL_TARGETS {
                    assert!(surface.schedule_scroll_target(
                        ConversationSurfaceTarget::Scene(scene_id(&format!("queued-{index}"))),
                        surface_cx,
                    ));
                }
                assert!(!surface.schedule_scroll_target(
                    ConversationSurfaceTarget::Scene(scene_id("refused")),
                    surface_cx,
                ));
                assert_eq!(
                    surface.pending_scroll_targets.len(),
                    CONVERSATION_SURFACE_MAX_SCROLL_TARGETS
                );
                assert_eq!(surface.pending_scroll_targets.first(), Some(&first));
            });
        });
    }

    /// Engine answer submit wiring: explicit gestures, fresh identities,
    /// single-flight suppression, client-side empty rejection, and
    /// settle-in-place resolution through the existing pairing policy.
    ///
    /// These are plain `#[test]` functions against the surface's submit gates
    /// with no display dependencies.
    mod approve_submit {
        use artisan_domain::{
            ApprovalObservation, ApprovalRequest, Command, EngineObservationEvent,
            OBSERVATION_ANSWER_MAX_BYTES, Observation, ObservationId, ObservationSequence,
            QuestionInput, QuestionObservation, QuestionOption, ReceiptDisposition, RunId,
            ThreadId,
        };
        use artisan_protocol::{
            ErrorCode, ErrorDetail, ProtocolFailure, RespondApprovalReceipt,
            RespondQuestionReceipt, RunInteractionOutcome,
        };

        use super::super::{
            AnswerDispatch, AnswerDispatchAction, ApprovalAnswerGate, ConversationSurface,
            QuestionAnswerGate, QuestionChoiceCache, drain_answer_queue,
        };
        use super::{item, scene};
        use crate::conversation_scene::SceneItemKind;
        use crate::engine_approve_ui::AnswerSettlement;
        use crate::engine_observation_state::EngineObservationState;
        use crate::native_transport_service::{CommandSendError, NativeTransportCommand};
        use artisan_ui::theme::ThemeMode;
        use gpui::{Entity, KeyDownEvent, Keystroke, TestAppContext, VisualTestContext};

        fn observation_id(value: &str) -> ObservationId {
            ObservationId::parse(value).expect("fixture observation id is valid")
        }

        fn sequence(value: u64) -> ObservationSequence {
            ObservationSequence::new(value).expect("fixture sequence is valid")
        }

        fn thread_id() -> ThreadId {
            ThreadId::parse("thread-approve").expect("fixture thread id is valid")
        }

        fn run_id() -> RunId {
            RunId::parse("run-approve").expect("fixture run id is valid")
        }

        fn event(observation: Observation) -> EngineObservationEvent {
            EngineObservationEvent {
                thread_id: thread_id(),
                observation,
                // Integrated domain carries the optional engine attribution
                // row on every observation event; fixtures leave it absent.
                attribution: None,
            }
        }

        fn approval_requested(approval: &str) -> Observation {
            Observation::Approval(
                ApprovalObservation::requested(
                    observation_id(&format!("obs-{approval}-requested")),
                    sequence(9),
                    observation_id(approval),
                    String::from("Run the test suite?"),
                    ApprovalRequest::command(
                        String::from("cargo test"),
                        Some(String::from("C:/repos/demo")),
                        Some(String::from("verify before landing")),
                    )
                    .expect("fixture approval request is valid"),
                )
                .expect("fixture requested approval is valid"),
            )
        }

        fn approval_resolved(approval: &str, approved: bool) -> Observation {
            Observation::Approval(
                ApprovalObservation::resolved(
                    observation_id(&format!("obs-{approval}-resolved")),
                    sequence(10),
                    observation_id(approval),
                    String::from("Run the test suite?"),
                    ApprovalRequest::command(
                        String::from("cargo test"),
                        Some(String::from("C:/repos/demo")),
                        Some(String::from("verify before landing")),
                    )
                    .expect("fixture approval request is valid"),
                    approved,
                )
                .expect("fixture resolved approval is valid"),
            )
        }

        fn first_options() -> Vec<QuestionOption> {
            vec![
                QuestionOption::new(String::from("tokio"), None).expect("fixture option is valid"),
                QuestionOption::new(
                    String::from("async-std"),
                    Some(String::from("alternative runtime")),
                )
                .expect("fixture option is valid"),
            ]
        }

        fn question_input(question: &str, multi_select: bool) -> QuestionInput {
            QuestionInput {
                question_id: observation_id(question),
                text: String::from("Which runtime?"),
                header: Some(String::from("Runtime")),
                multi_select,
                options: Some(first_options()),
            }
        }

        fn question_requested(question: &str, multi_select: bool) -> Observation {
            Observation::Question(
                QuestionObservation::requested(
                    observation_id(&format!("obs-{question}-requested")),
                    sequence(11),
                    question_input(question, multi_select),
                )
                .expect("fixture requested question is valid"),
            )
        }

        fn approval_command_inner(command: &Command) -> &artisan_domain::RespondApproval {
            match command {
                Command::RespondApproval(command) => command,
                _ => panic!("approval gesture must build an approval command"),
            }
        }

        fn question_command_inner(command: &Command) -> &artisan_domain::RespondQuestion {
            match command {
                Command::RespondQuestion(command) => command,
                _ => panic!("question gesture must build a question command"),
            }
        }

        fn approval_dispatch() -> AnswerDispatch {
            let mut gate = ApprovalAnswerGate::new();
            let attempt = gate
                .begin(thread_id(), run_id(), observation_id("approval-1"), true)
                .expect("approve gesture admits an attempt");
            AnswerDispatch {
                action: AnswerDispatchAction::Approval(attempt.action),
                command: attempt.command,
                request_id: attempt.request_id,
            }
        }

        fn question_dispatch() -> AnswerDispatch {
            let mut gate = QuestionAnswerGate::new();
            let attempt = gate
                .submit_single(
                    thread_id(),
                    run_id(),
                    observation_id("question-1"),
                    String::from("tokio"),
                )
                .expect("single-select choice submits immediately");
            AnswerDispatch {
                action: AnswerDispatchAction::Question(attempt.action),
                command: attempt.command,
                request_id: attempt.request_id,
            }
        }

        #[test]
        fn approve_submit_dispatches_with_fresh_ids() {
            let mut first_row = ApprovalAnswerGate::new();
            let mut second_row = ApprovalAnswerGate::new();
            let first = first_row
                .begin(thread_id(), run_id(), observation_id("approval-1"), true)
                .expect("approve gesture admits an attempt");
            let second = second_row
                .begin(thread_id(), run_id(), observation_id("approval-2"), true)
                .expect("second row admits its own attempt");
            assert_ne!(first.request_id, second.request_id);
            assert!(first.action.approved);
            assert_eq!(first.action.approval_id.as_str(), "approval-1");
            assert_eq!(first.action.run_id, run_id());
            let command = approval_command_inner(&first.command);
            assert_eq!(command.request_id(), &first.request_id);
            assert_eq!(command.thread_id(), &thread_id());
            assert!(command.approved());
            assert!(first_row.is_in_flight());
            assert_eq!(first_row.pending_decision(), Some(true));
        }

        #[test]
        fn deny_submit_carries_an_explicit_denial() {
            let mut gate = ApprovalAnswerGate::new();
            let attempt = gate
                .begin(thread_id(), run_id(), observation_id("approval-1"), false)
                .expect("deny gesture admits an attempt");
            assert!(!attempt.action.approved);
            assert!(!approval_command_inner(&attempt.command).approved());
            assert_eq!(gate.pending_decision(), Some(false));
        }

        #[test]
        fn question_choice_submit_carries_selected_options() {
            let mut gate = QuestionAnswerGate::new();
            assert!(
                QuestionChoiceCache {
                    multi_select: true,
                    options: vec![
                        (String::from("tokio"), None),
                        (
                            String::from("async-std"),
                            Some(String::from("alternative runtime")),
                        ),
                    ],
                }
                .is_choice()
            );
            gate.toggle_option(String::from("tokio"), true);
            gate.toggle_option(String::from("async-std"), true);
            assert_eq!(
                gate.selected(),
                &[String::from("tokio"), String::from("async-std")]
            );
            let attempt = gate
                .submit_selected(thread_id(), run_id(), observation_id("question-1"))
                .expect("staged choices submit");
            assert_eq!(
                attempt.action.answers,
                vec![String::from("tokio"), String::from("async-std")]
            );
            assert_eq!(
                question_command_inner(&attempt.command).answers(),
                &attempt.action.answers
            );
            assert!(gate.is_in_flight());
        }

        #[test]
        fn freeform_submit_carries_typed_text() {
            let mut gate = QuestionAnswerGate::new();
            gate.set_draft(String::from("  typed answer  "));
            let attempt = gate
                .submit_freeform(thread_id(), run_id(), observation_id("question-free"))
                .expect("typed draft submits");
            assert_eq!(attempt.action.answers, vec![String::from("typed answer")]);
            assert_eq!(gate.draft(), "");
            assert!(gate.is_in_flight());
        }

        #[test]
        fn empty_freeform_rejected_without_dispatch() {
            let mut gate = QuestionAnswerGate::new();
            gate.set_draft(String::from("   "));
            assert!(
                gate.submit_freeform(thread_id(), run_id(), observation_id("question-free"))
                    .is_none()
            );
            assert!(!gate.is_in_flight());
            assert!(gate.last_request_id().is_none());
            assert!(gate.failure_message().is_none());
            gate.set_draft(String::from("typed"));
            assert!(
                gate.submit_freeform(thread_id(), run_id(), observation_id("question-free"))
                    .is_some(),
                "the row stays pending so a later typed submit still works"
            );
        }

        #[test]
        fn double_submit_suppressed_while_flight_outstanding() {
            let mut gate = ApprovalAnswerGate::new();
            let first = gate
                .begin(thread_id(), run_id(), observation_id("approval-1"), true)
                .expect("first gesture admits an attempt");
            assert!(
                gate.begin(thread_id(), run_id(), observation_id("approval-1"), true,)
                    .is_none(),
                "a second gesture mints nothing while the first is outstanding"
            );
            assert_eq!(gate.last_request_id(), Some(&first.request_id));
            let failure = ProtocolFailure {
                code: ErrorCode::Internal,
                detail: ErrorDetail::parse("fixture unavailable").expect("fixture detail is valid"),
                retryable: true,
                request_id: Some(first.request_id.clone()),
            };
            let settlement = gate.settle_failure(&first.request_id, &failure);
            match &settlement {
                AnswerSettlement::RetryableFailure { message } => {
                    assert!(message.contains("retry the same answer"));
                }
                _ => panic!("unavailable work must stay retryable"),
            }
            assert!(!gate.is_in_flight());
            let retry = gate
                .begin(thread_id(), run_id(), observation_id("approval-1"), true)
                .expect("an explicit retry mints a fresh identity");
            assert_ne!(retry.request_id, first.request_id);
        }

        #[test]
        fn resolution_settles_the_row_via_existing_pairing() {
            let mut state = EngineObservationState::new(thread_id());
            state.apply(1, &event(approval_requested("approval-1")));
            state.apply(2, &event(question_requested("question-1", false)));

            let mut gate = ApprovalAnswerGate::new();
            let attempt = gate
                .begin(thread_id(), run_id(), observation_id("approval-1"), true)
                .expect("approve gesture admits an attempt");
            let receipt = RespondApprovalReceipt {
                request_id: attempt.request_id.clone(),
                thread_id: thread_id(),
                run_id: run_id(),
                approval_id: observation_id("approval-1"),
                approved: true,
                outcome: RunInteractionOutcome::Applied,
                disposition: ReceiptDisposition::Accepted,
            };
            let pairing =
                gate.settle_receipt(&state, approval_command_inner(&attempt.command), &receipt);
            assert_eq!(
                pairing.settlement,
                AnswerSettlement::SettledInPlace { duplicate: false }
            );
            assert!(pairing.is_settled());

            state.apply(3, &event(approval_resolved("approval-1", true)));
            assert_eq!(
                state.approval("approval-1").expect("row pairs").approved(),
                Some(true)
            );
            assert_eq!(state.approvals_in_order().len(), 1);

            let mut question_gate = QuestionAnswerGate::new();
            let question_attempt = question_gate
                .submit_single(
                    thread_id(),
                    run_id(),
                    observation_id("question-1"),
                    String::from("tokio"),
                )
                .expect("single-select choice submits immediately");
            let question_receipt = RespondQuestionReceipt {
                request_id: question_attempt.request_id.clone(),
                thread_id: thread_id(),
                run_id: run_id(),
                question_id: observation_id("question-1"),
                answers: vec![String::from("tokio")],
                outcome: RunInteractionOutcome::Applied,
                disposition: ReceiptDisposition::Accepted,
            };
            let question_pairing = question_gate.settle_receipt(
                &state,
                question_command_inner(&question_attempt.command),
                &question_receipt,
            );
            assert!(question_pairing.is_settled());
        }

        /// Scripted transport submit harness mirroring the application
        /// `test_command_sink`: records every submitted transport command and
        /// replays scripted admission outcomes in order, defaulting to
        /// admitted once the script runs out.
        struct ScriptedSubmit {
            commands: Vec<NativeTransportCommand>,
            outcomes: std::collections::VecDeque<Result<(), CommandSendError>>,
        }

        impl ScriptedSubmit {
            fn new(outcomes: Vec<Result<(), CommandSendError>>) -> Self {
                Self {
                    commands: Vec::new(),
                    outcomes: outcomes.into(),
                }
            }

            fn submit(&mut self, command: NativeTransportCommand) -> Result<(), CommandSendError> {
                self.commands.push(command);
                self.outcomes.pop_front().unwrap_or(Ok(()))
            }
        }

        fn approval_answer(command: &NativeTransportCommand) -> &artisan_domain::RespondApproval {
            if let NativeTransportCommand::RespondApproval(answer) = command {
                answer
            } else {
                panic!("dispatch must submit an approval answer")
            }
        }

        fn question_answer(command: &NativeTransportCommand) -> &artisan_domain::RespondQuestion {
            if let NativeTransportCommand::RespondQuestion(answer) = command {
                answer
            } else {
                panic!("dispatch must submit a question answer")
            }
        }

        #[test]
        fn drain_sends_taken_queue_preserving_minted_ids() {
            let approval = approval_dispatch();
            let question = question_dispatch();
            let expected_approval = approval.request_id.clone();
            let expected_question = question.request_id.clone();
            let mut submit = ScriptedSubmit::new(Vec::new());
            let (requeue, report) = drain_answer_queue(vec![approval, question], &mut |command| {
                submit.submit(command)
            });
            assert_eq!(report.sent, 2);
            assert!(report.is_clean());
            assert!(requeue.is_empty());
            assert_eq!(submit.commands.len(), 2);
            let submitted = approval_answer(&submit.commands[0]);
            assert_eq!(submitted.request_id(), &expected_approval);
            assert_eq!(submitted.thread_id(), &thread_id());
            assert_eq!(submitted.run_id(), &run_id());
            assert_eq!(submitted.approval_id().as_str(), "approval-1");
            assert!(submitted.approved);
            let submitted = question_answer(&submit.commands[1]);
            assert_eq!(submitted.request_id(), &expected_question);
            assert_eq!(submitted.thread_id(), &thread_id());
            assert_eq!(submitted.run_id(), &run_id());
            assert_eq!(submitted.question_id().as_str(), "question-1");
            assert_eq!(submitted.answers(), &vec![String::from("tokio")]);
        }

        #[test]
        fn drain_failure_requeues_with_retry_message() {
            let dispatch = approval_dispatch();
            let expected = dispatch.request_id.clone();
            let mut submit = ScriptedSubmit::new(vec![Err(CommandSendError::Busy)]);
            let (requeue, report) =
                drain_answer_queue(vec![dispatch], &mut |command| submit.submit(command));
            assert_eq!(submit.commands.len(), 1, "one drain attempt per dispatch");
            assert_eq!(report.sent, 0);
            assert!(!report.is_clean());
            assert_eq!(report.failed.len(), 1);
            assert_eq!(report.failed[0].request_id, expected);
            assert!(
                report.failed[0].message.contains("retry the same answer"),
                "the existing retry message is surfaced"
            );
            assert_eq!(requeue.len(), 1);
            assert_eq!(requeue[0].request_id, expected);

            let mut submit = ScriptedSubmit::new(Vec::new());
            let (requeue, report) =
                drain_answer_queue(requeue, &mut |command| submit.submit(command));
            assert_eq!(report.sent, 1);
            assert!(report.is_clean());
            assert!(requeue.is_empty());
            assert_eq!(submit.commands.len(), 1);
            assert_eq!(approval_answer(&submit.commands[0]).request_id, expected);
        }

        #[test]
        fn drain_stopped_reports_a_diagnostic() {
            let dispatch = question_dispatch();
            let expected = dispatch.request_id.clone();
            let mut submit = ScriptedSubmit::new(vec![Err(CommandSendError::Stopped)]);
            let (requeue, report) =
                drain_answer_queue(vec![dispatch], &mut |command| submit.submit(command));
            assert_eq!(report.sent, 0);
            assert_eq!(report.failed.len(), 1);
            assert_eq!(report.failed[0].request_id, expected);
            assert!(
                report.failed[0].message.contains("nothing was recorded"),
                "a stopped service degrades to the existing diagnostic text"
            );
            assert_eq!(requeue.len(), 1, "nothing is silently dropped");
        }

        #[test]
        fn drain_empty_outbox_is_a_no_op() {
            let mut submit = ScriptedSubmit::new(vec![Err(CommandSendError::Busy)]);
            let (requeue, report) =
                drain_answer_queue(Vec::new(), &mut |command| submit.submit(command));
            assert_eq!(report.sent, 0);
            assert!(report.is_clean());
            assert!(requeue.is_empty());
            assert!(
                submit.commands.is_empty(),
                "an empty queue never touches submit"
            );
        }

        #[test]
        fn double_drain_sends_once() {
            let dispatch = approval_dispatch();
            let expected = dispatch.request_id.clone();
            let mut submit = ScriptedSubmit::new(Vec::new());
            let (requeue, first) =
                drain_answer_queue(vec![dispatch], &mut |command| submit.submit(command));
            let (requeue, second) =
                drain_answer_queue(requeue, &mut |command| submit.submit(command));
            assert_eq!((first.sent, second.sent), (1, 0));
            assert!(second.is_clean());
            assert!(requeue.is_empty());
            assert_eq!(submit.commands.len(), 1);
            assert_eq!(approval_answer(&submit.commands[0]).request_id, expected);
        }

        #[gpui::test]
        fn surface_drain_takes_sends_and_empties(cx: &mut TestAppContext) {
            let (surface, cx) = cx.add_window_view(|_, surface_cx| {
                ConversationSurface::new(scene(Vec::new()), ThemeMode::Dark, surface_cx)
            });
            cx.update(|_, app| {
                surface.update(app, |surface, cx| {
                    surface.set_answer_context(thread_id(), run_id(), cx);
                    assert!(surface.submit_approval_gesture(
                        "approval-1",
                        &observation_id("approval-1"),
                        true,
                        cx,
                    ));
                    assert_eq!(surface.pending_answer_dispatches().len(), 1);
                });
            });
            let mut submit = ScriptedSubmit::new(Vec::new());
            cx.update(|_, app| {
                surface.update(app, |surface, _| {
                    let report = surface
                        .drain_pending_answer_dispatches(&mut |command| submit.submit(command));
                    assert_eq!(report.sent, 1);
                    assert!(report.is_clean());
                    assert!(surface.pending_answer_dispatches().is_empty());
                    let replay = surface
                        .drain_pending_answer_dispatches(&mut |command| submit.submit(command));
                    assert_eq!(replay.sent, 0);
                    assert!(surface.pending_answer_dispatches().is_empty());
                });
            });
            assert_eq!(submit.commands.len(), 1);
            let submitted = approval_answer(&submit.commands[0]);
            assert_eq!(submitted.thread_id(), &thread_id());
            assert_eq!(submitted.run_id(), &run_id());
            assert_eq!(submitted.approval_id().as_str(), "approval-1");
            assert!(submitted.approved);
        }

        #[test]
        fn keystroke_text_accumulates_with_canonical_intake() {
            let mut gate = QuestionAnswerGate::new();
            assert!(gate.insert_text("h"));
            assert!(gate.insert_text("i"));
            assert_eq!(gate.draft(), "hi");
            assert!(gate.insert_text("a\r\nb"));
            assert_eq!(gate.draft(), "hiab");
            assert!(gate.insert_text("x\u{200B}y"));
            assert_eq!(gate.draft(), "hiabxy");
            assert!(!gate.insert_text(""));
        }

        #[test]
        fn keystroke_intake_clamped_to_answer_bound() {
            let mut gate = QuestionAnswerGate::new();
            assert!(gate.insert_text(&"y".repeat(OBSERVATION_ANSWER_MAX_BYTES)));
            assert_eq!(gate.draft().len(), OBSERVATION_ANSWER_MAX_BYTES);
            assert!(!gate.insert_text("z"));
            assert_eq!(gate.draft().len(), OBSERVATION_ANSWER_MAX_BYTES);
        }

        #[test]
        fn delete_backward_removes_last_char() {
            let mut gate = QuestionAnswerGate::new();
            assert!(!gate.delete_backward());
            gate.set_draft(String::from("hi"));
            assert!(gate.delete_backward());
            assert_eq!(gate.draft(), "h");
            assert!(gate.delete_backward());
            assert_eq!(gate.draft(), "");
            assert!(!gate.delete_backward());
        }

        #[test]
        fn settled_row_drops_its_draft() {
            let mut state = EngineObservationState::new(thread_id());
            state.apply(1, &event(question_requested("question-1", false)));
            let mut gate = QuestionAnswerGate::new();
            gate.set_draft(String::from("typed"));
            let attempt = gate
                .submit_freeform(thread_id(), run_id(), observation_id("question-1"))
                .expect("typed draft submits");
            gate.set_draft(String::from("typed during flight"));
            let receipt = RespondQuestionReceipt {
                request_id: attempt.request_id.clone(),
                thread_id: thread_id(),
                run_id: run_id(),
                question_id: observation_id("question-1"),
                answers: vec![String::from("typed")],
                outcome: RunInteractionOutcome::Applied,
                disposition: ReceiptDisposition::Accepted,
            };
            let pairing =
                gate.settle_receipt(&state, question_command_inner(&attempt.command), &receipt);
            assert!(pairing.is_settled());
            assert_eq!(gate.draft(), "");
        }

        #[test]
        fn failed_row_keeps_its_draft_for_retry() {
            let mut gate = QuestionAnswerGate::new();
            gate.set_draft(String::from("typed"));
            let attempt = gate
                .submit_freeform(thread_id(), run_id(), observation_id("question-1"))
                .expect("typed draft submits");
            gate.set_draft(String::from("retry text"));
            let failure = ProtocolFailure {
                code: ErrorCode::Internal,
                detail: ErrorDetail::parse("fixture unavailable").expect("fixture detail is valid"),
                retryable: true,
                request_id: Some(attempt.request_id.clone()),
            };
            let settlement = gate.settle_failure(&attempt.request_id, &failure);
            assert!(matches!(
                settlement,
                AnswerSettlement::RetryableFailure { .. }
            ));
            assert_eq!(gate.draft(), "retry text");
            assert!(!gate.is_in_flight());
        }

        fn freeform_scene() -> crate::conversation_scene::ConversationScene {
            scene(vec![
                item(
                    "question-a",
                    1,
                    SceneItemKind::Question {
                        prompt: String::from("Which runtime?"),
                    },
                    None,
                ),
                item(
                    "question-b",
                    2,
                    SceneItemKind::Question {
                        prompt: String::from("Which region?"),
                    },
                    None,
                ),
            ])
        }

        fn press_key(cx: &mut VisualTestContext, key: &str) {
            cx.simulate_event(KeyDownEvent {
                keystroke: Keystroke::parse(key).expect("known test key"),
                is_held: false,
                prefer_character_input: false,
            });
        }

        fn focus_row(
            cx: &mut VisualTestContext,
            surface: &Entity<ConversationSurface>,
            block: &str,
        ) {
            cx.update(|window, app| {
                window.focus(
                    &surface
                        .read(app)
                        .question_focus_handle(block)
                        .expect("row focus handle"),
                    app,
                );
            });
            cx.run_until_parked();
        }

        fn row_draft(
            cx: &mut VisualTestContext,
            surface: &Entity<ConversationSurface>,
            block: &str,
        ) -> String {
            cx.update(|_, app| {
                surface
                    .read(app)
                    .question_gates
                    .get(block)
                    .map_or("", QuestionAnswerGate::draft)
                    .to_owned()
            })
        }

        #[gpui::test]
        fn freeform_typing_accumulates_per_row(cx: &mut TestAppContext) {
            let (surface, cx) = cx.add_window_view(|_, surface_cx| {
                ConversationSurface::new(freeform_scene(), ThemeMode::Dark, surface_cx)
            });
            cx.run_until_parked();
            focus_row(cx, &surface, "question-a");
            press_key(cx, "h");
            press_key(cx, "i");
            press_key(cx, "space");
            press_key(cx, "h");
            press_key(cx, "o");
            cx.run_until_parked();
            assert_eq!(row_draft(cx, &surface, "question-a"), "hi ho");
            focus_row(cx, &surface, "question-b");
            press_key(cx, "x");
            cx.run_until_parked();
            assert_eq!(row_draft(cx, &surface, "question-b"), "x");
            assert_eq!(
                row_draft(cx, &surface, "question-a"),
                "hi ho",
                "rows keep isolated drafts"
            );
        }

        #[gpui::test]
        fn freeform_backspace_deletes(cx: &mut TestAppContext) {
            let (surface, cx) = cx.add_window_view(|_, surface_cx| {
                ConversationSurface::new(freeform_scene(), ThemeMode::Dark, surface_cx)
            });
            cx.run_until_parked();
            focus_row(cx, &surface, "question-a");
            press_key(cx, "h");
            press_key(cx, "i");
            press_key(cx, "backspace");
            cx.run_until_parked();
            assert_eq!(row_draft(cx, &surface, "question-a"), "h");
        }

        #[gpui::test]
        fn freeform_enter_submits_staged_text(cx: &mut TestAppContext) {
            let (surface, cx) = cx.add_window_view(|_, surface_cx| {
                ConversationSurface::new(freeform_scene(), ThemeMode::Dark, surface_cx)
            });
            cx.run_until_parked();
            cx.update(|_, app| {
                surface.update(app, |surface, cx| {
                    surface.set_answer_context(thread_id(), run_id(), cx);
                });
            });
            focus_row(cx, &surface, "question-a");
            for key in ["t", "o", "k", "i", "o"] {
                press_key(cx, key);
            }
            press_key(cx, "enter");
            cx.run_until_parked();
            let answers = cx.update(|_, app| {
                let surface = surface.read(app);
                assert_eq!(surface.pending_answer_dispatches().len(), 1);
                match &surface.pending_answer_dispatches()[0].command {
                    Command::RespondQuestion(command) => command.answers().clone(),
                    _ => panic!("free-form enter must dispatch an answer"),
                }
            });
            assert_eq!(answers, vec![String::from("tokio")]);
            assert_eq!(row_draft(cx, &surface, "question-a"), "");
        }

        #[gpui::test]
        fn freeform_empty_enter_rejected(cx: &mut TestAppContext) {
            let (surface, cx) = cx.add_window_view(|_, surface_cx| {
                ConversationSurface::new(freeform_scene(), ThemeMode::Dark, surface_cx)
            });
            cx.run_until_parked();
            cx.update(|_, app| {
                surface.update(app, |surface, cx| {
                    surface.set_answer_context(thread_id(), run_id(), cx);
                });
            });
            focus_row(cx, &surface, "question-b");
            press_key(cx, "enter");
            cx.run_until_parked();
            cx.update(|_, app| {
                assert!(surface.read(app).pending_answer_dispatches().is_empty());
            });
            assert_eq!(row_draft(cx, &surface, "question-b"), "");
        }

        #[gpui::test]
        fn freeform_escape_clears_focus(cx: &mut TestAppContext) {
            let (surface, cx) = cx.add_window_view(|_, surface_cx| {
                ConversationSurface::new(freeform_scene(), ThemeMode::Dark, surface_cx)
            });
            cx.run_until_parked();
            focus_row(cx, &surface, "question-a");
            press_key(cx, "x");
            press_key(cx, "escape");
            cx.run_until_parked();
            cx.update(|window, app| {
                let view = surface.read(app);
                assert!(
                    !view
                        .question_focus_handle("question-a")
                        .expect("row focus handle")
                        .is_focused(window),
                    "escape must clear row focus without submitting"
                );
                assert!(
                    view.transcript_focus_handle().is_focused(window),
                    "escape returns focus to the transcript"
                );
            });
            assert_eq!(row_draft(cx, &surface, "question-a"), "x");
            cx.update(|_, app| {
                assert!(surface.read(app).pending_answer_dispatches().is_empty());
            });
        }
    }
}
