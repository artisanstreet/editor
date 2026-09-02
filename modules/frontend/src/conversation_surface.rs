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

use artisan_domain::{ItemId, TurnId};
use artisan_ui::alert::{Alert, AlertVariant};
use artisan_ui::badge::{BadgeStyle, outline_badge};
use artisan_ui::button::{Button, ButtonContent, ButtonSize, ButtonVariant, FocusVisibility};
use artisan_ui::card::{CardStyle, compact_card, compact_card_content};
use artisan_ui::collapsible::Collapsible;
use artisan_ui::markdown_renderer::MarkdownRenderer;
use artisan_ui::motion::MotionPolicy;
use artisan_ui::scroll_area::ScrollArea;
use artisan_ui::separator::{SeparatorAxis, separator};
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use gpui::{
    AnyElement, Context, Div, ElementId, Entity, FocusHandle, FontWeight, IntoElement, Render,
    ScrollAnchor, ScrollHandle, SharedString, Stateful, Window, div,
    prelude::{
        InteractiveElement as _, ParentElement as _, StatefulInteractiveElement as _, Styled as _,
    },
    px,
};
use std::collections::HashMap;
use std::rc::Rc;

use crate::conversation_scene::{
    ChangeSetBlock, CompactionBlock, ConversationScene, ErrorBlock, FileChangeStatus,
    ModelTransitionBlock, NativeFactBlock, PlanBlock, QuestionBlock, SceneDisclosure,
    SceneFileChange, SceneId, SteeringBlock, TurnBlock, TurnFooterBlock, TurnNarration, TurnScene,
    UsageInterruptionBlock, UserMessageBlock, WorkGroupBlock, WorkItem,
};
use crate::conversation_scroll_position::conversation_is_following;
use crate::conversation_turn_navigator::{
    ConversationSnapshotInput, ConversationTurnInput, LoadedConversationItemInput,
    conversation_turn_markers,
};

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

/// Alias for callers that use the shorter root-selector vocabulary.
pub const ROOT_SELECTOR: &str = CONVERSATION_SURFACE_SELECTOR;

/// Alias for callers that use the shorter viewport-selector vocabulary.
pub const VIEWPORT_SELECTOR: &str = CONVERSATION_VIEWPORT_SELECTOR;

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
/// `StreamingSuppression` intentionally returns `None`, so a renderer can
/// guarantee that no thinking/working row is painted while a streaming
/// assistant message owns the visible progress state.
#[must_use]
pub fn turn_status_copy(narration: TurnNarration) -> Option<String> {
    match narration {
        TurnNarration::Quiet => Some("Quiet".to_owned()),
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
    theme_mode: ThemeMode,
    markdown_renderer: MarkdownRenderer,
    scroll_handle: ScrollHandle,
    transcript_focus: FocusHandle,
    disclosure_focus: FocusHandle,
    jump_to_latest_focus: FocusHandle,
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

/// One loaded user-message control in the turn navigator rail.
struct NavigatorMarker {
    /// Visible policy label; never crosses the action boundary.
    label: String,
    /// Exact scroll target; identity only, never body text.
    target: ConversationSurfaceTarget,
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
        TurnBlock::WorkGroup(block) => (work_group_anchor_id(turn_id, block), None),
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

impl ConversationSurface {
    /// Creates a surface with keyboard-focusable transcript and disclosure
    /// handles. The surface starts with the supplied scene and no actions.
    #[must_use]
    pub fn new(scene: ConversationScene, theme_mode: ThemeMode, cx: &mut Context<Self>) -> Self {
        Self {
            scene,
            theme_mode,
            markdown_renderer: MarkdownRenderer::new(),
            scroll_handle: ScrollHandle::new(),
            transcript_focus: cx.focus_handle().tab_index(0).tab_stop(true),
            disclosure_focus: cx.focus_handle().tab_index(1).tab_stop(true),
            jump_to_latest_focus: cx.focus_handle().tab_index(2).tab_stop(true),
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
        }
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
        cx.notify();
    }

    /// Sets the shared theme mode and repaints the surface when it changes.
    pub fn set_theme_mode(&mut self, theme_mode: ThemeMode, cx: &mut Context<Self>) {
        if self.theme_mode != theme_mode {
            self.theme_mode = theme_mode;
            cx.notify();
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
        offset.x = offset.x.clamp(-max_offset.width, px(0.0));
        offset.y = offset.y.clamp(-max_offset.height, px(0.0));
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
        let scroll_height = viewport_height + f64::from(max_offset.height);
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
    ) -> AnyElement {
        let selector = turn_selector(&turn.turn_id);
        let turn_element = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(4.0));
        // Identities mirror the children pushed below in block order. A
        // suppressed status row paints no child, so it contributes no slot.
        let child_identities: Vec<(Option<SceneId>, Option<ItemId>)> = turn
            .blocks()
            .iter()
            .filter_map(|block| {
                if let TurnBlock::TurnStatus(status) = block
                    && turn_status_copy(status.narration).is_none()
                {
                    return None;
                }
                Some(block_scroll_identity(&turn.turn_id, block))
            })
            .collect();
        let surface = entity.downgrade();
        let turn_element = turn_element.on_children_prepainted(
            move |children_bounds, window, app| {
                let _ = surface.update(app, |surface, _| {
                    surface.apply_executed_scroll_targets(
                        &child_identities,
                        &children_bounds,
                        window,
                    );
                });
            },
        );
        let turn_element = anchors.attach(
            turn_element,
            SceneId::parse(turn.turn_id.as_str()).ok().as_ref(),
            None,
        );
        let mut turn_element = turn_element.debug_selector(move || selector.clone());

        for block in turn.blocks() {
            if let Some(element) =
                self.render_block(&turn.turn_id, block, entity, theme, anchors)
            {
                turn_element = turn_element.child(element);
            }
        }

        turn_element.into_any_element()
    }

    fn render_block(
        &self,
        turn_id: &TurnId,
        block: &TurnBlock,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> Option<AnyElement> {
        let selector = block_selector(turn_id, block);
        match block {
            TurnBlock::UserMessage(block) => {
                Some(self.render_user_message(block, selector, entity, theme, anchors))
            }
            TurnBlock::AssistantMessage(block) => {
                Some(self.render_assistant_message(block, selector, entity, theme, anchors))
            }
            TurnBlock::WorkGroup(block) => {
                Some(self.render_work_group(turn_id, block, selector, entity, theme, anchors))
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
            TurnBlock::TurnStatus(block) => Self::render_status(block, selector, theme),
            TurnBlock::TurnFooter(block) => Some(Self::render_footer(block, selector, theme)),
        }
    }

    fn render_user_message(
        &self,
        block: &UserMessageBlock,
        selector: String,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        self.render_markdown_text_block(
            TextBlockRender {
                id: &block.id,
                disclosure: block.disclosure,
                selector,
                title: "User message",
                body: &block.body,
            },
            entity,
            theme,
            anchors,
        )
    }

    fn render_assistant_message(
        &self,
        block: &crate::conversation_scene::AssistantMessageBlock,
        selector: String,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        self.render_markdown_text_block(
            TextBlockRender {
                id: &block.id,
                disclosure: block.disclosure,
                selector,
                title: "Assistant message",
                body: &block.body,
            },
            entity,
            theme,
            anchors,
        )
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

    fn render_markdown_text_block(
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
        let rendered_body = self
            .markdown_renderer
            .render_source(body, *theme, selector.clone());
        self.render_controlled_card(
            ControlledCardOptions {
                id: id.clone(),
                item_id: item_id_for_scene_id(id),
                disclosure,
                selector,
                style,
            },
            compact_card_content(style).child(card_heading(title, theme)),
            compact_card_content(style).child(rendered_body),
            entity,
            anchors,
        )
    }

    fn render_work_group(
        &self,
        turn_id: &TurnId,
        block: &WorkGroupBlock,
        selector: String,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        let style = CardStyle::resolve(*theme);
        let group_id = work_group_anchor_id(turn_id, block);
        let title = block
            .label
            .map_or_else(|| "Work".to_owned(), format_work_group_label);

        let mut items = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(3.0));
        let items_mounted = !matches!(block.disclosure, Some(SceneDisclosure::Closed));
        for item in &block.items {
            items = items.child(Self::render_work_item(
                item,
                &selector,
                theme,
                anchors,
                items_mounted,
            ));
        }
        let item_identities: Vec<(Option<SceneId>, Option<ItemId>)> = block
            .items
            .iter()
            .map(|item| (None, item_id_for_scene_id(work_item_id(item))))
            .collect();
        let surface = entity.downgrade();
        items = items.on_children_prepainted(move |children_bounds, window, app| {
            let _ = surface.update(app, |surface, _| {
                surface.apply_executed_scroll_targets(
                    &item_identities,
                    &children_bounds,
                    window,
                );
            });
        });

        let Some(group_id) = group_id else {
            let fallback_selector = selector.clone();
            let mut card = anchors.attach(compact_card(style).w_full(), None, None);
            card = card.debug_selector(move || fallback_selector.clone());
            return card
                .child(compact_card_content(style).child(card_heading(title, theme)))
                .child(compact_card_content(style).child(items))
                .into_any_element();
        };

        self.render_controlled_card(
            ControlledCardOptions {
                id: group_id,
                item_id: None,
                disclosure: block.disclosure,
                selector,
                style,
            },
            compact_card_content(style).child(card_heading(title, theme)),
            compact_card_content(style).child(items),
            entity,
            anchors,
        )
    }

    fn render_work_item(
        item: &WorkItem,
        group_selector: &str,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
        mounted: bool,
    ) -> AnyElement {
        let (id, title, text) = match item {
            WorkItem::Reasoning { id, body, .. } => (id, "Reasoning", body.as_str()),
            WorkItem::Activity { id, body, .. } => (id, "Activity", body.as_str()),
            WorkItem::WorkSession { id, title, .. } => (id, "Work session", title.as_str()),
        };
        let selector = format!("{group_selector}-item-{}", id.as_str());
        let item_element = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(1.0));
        if mounted {
            let mut item_element =
                anchors.attach(item_element, None, item_id_for_scene_id(id).as_ref());
            item_element = item_element.debug_selector(move || selector.clone());
            item_element = item_element
                .child(card_heading(title, theme))
                .child(body_text(text, theme));
            item_element.into_any_element()
        } else {
            let mut item_element = item_element.debug_selector(move || selector.clone());
            item_element = item_element
                .child(card_heading(title, theme))
                .child(body_text(text, theme));
            item_element.into_any_element()
        }
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
        self.render_text_block(
            TextBlockRender {
                id: &block.id,
                disclosure: block.disclosure,
                selector,
                title: "Approval requested",
                body: &block.prompt,
            },
            entity,
            theme,
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
        self.render_text_block(
            TextBlockRender {
                id: &block.id,
                disclosure: block.disclosure,
                selector,
                title: "Question",
                body: &block.prompt,
            },
            entity,
            theme,
            anchors,
        )
    }

    fn render_error(
        &self,
        block: &ErrorBlock,
        selector: String,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        anchors: &mut ScrollAnchorRegistry<'_>,
    ) -> AnyElement {
        let style = CardStyle::resolve(*theme);
        let alert = Alert::from_theme(*theme, AlertVariant::Destructive)
            .title("Error")
            .description(block.message.clone())
            .debug_selector(format!("{selector}-alert"));
        self.render_controlled_card(
            ControlledCardOptions {
                id: block.id.clone(),
                item_id: item_id_for_scene_id(&block.id),
                disclosure: block.disclosure,
                selector,
                style,
            },
            compact_card_content(style).child(card_heading("Error", theme)),
            alert,
            entity,
            anchors,
        )
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
        block: &crate::conversation_scene::TurnStatusBlock,
        selector: String,
        theme: &ArtisanTheme,
    ) -> Option<AnyElement> {
        let copy = turn_status_copy(block.narration)?;
        let mut status = div()
            .w_full()
            .min_w_0()
            .text_size(theme.typography.label_text)
            .text_color(status_color(theme, block.narration));
        status = status.child(copy);
        status = status.debug_selector(move || selector.clone());
        Some(status.into_any_element())
    }

    fn render_footer(
        _block: &TurnFooterBlock,
        selector: String,
        theme: &ArtisanTheme,
    ) -> AnyElement {
        let mut footer = div()
            .w_full()
            .text_size(theme.typography.label_text)
            .text_color(theme.colors.muted_foreground.to_paint())
            .child("Turn footer");
        footer = footer.debug_selector(move || selector.clone());
        footer.into_any_element()
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

impl Render for ConversationSurface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ArtisanTheme::for_mode(self.theme_mode);
        let entity = cx.entity();
        let mut transcript = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(4.0));
        let previous_anchors = std::mem::take(&mut self.scroll_anchors);
        let mut rendered_anchors = Vec::new();
        {
            let mut anchors = ScrollAnchorRegistry {
                handle: &self.scroll_handle,
                previous: &previous_anchors,
                next_element_id: 0,
                rendered: &mut rendered_anchors,
            };
            for turn in self.scene.turn_scenes() {
                transcript =
                    transcript.child(self.render_turn(turn, &entity, &theme, &mut anchors));
            }
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

        let scroll_area = ScrollArea::new(self.scroll_handle.clone(), theme)
            .focus_handle(self.transcript_focus.clone())
            .debug_selector(CONVERSATION_SURFACE_SELECTOR)
            .size_full()
            .bg(theme.colors.background.to_paint())
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
        if let Some(rail) = self.render_turn_navigator(&entity, &theme, window, cx) {
            root = root.child(rail);
        }
        root
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
    fn render_turn_navigator(
        &mut self,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let markers = loaded_turn_navigator_markers(&self.scene);
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
                self.transcript_focus.focus(window);
            }
        }
        if markers.is_empty() {
            return None;
        }
        let navigator_surface = entity.downgrade();
        let mut rail = div()
            .absolute()
            .right(px(16.0))
            .top(px(64.0))
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(1.0))
            .debug_selector(|| TURN_NAVIGATOR_SELECTOR.to_owned());
        for marker in &markers {
            let key = navigator_focus_key(&marker.target);
            let handle = self
                .navigator_focus
                .entry(key)
                .or_insert_with(|| cx.focus_handle().tab_stop(true))
                .clone();
            let target = marker.target.clone();
            let control_selector = format!(
                "{TURN_NAVIGATOR_CONTROL_PREFIX}-{}",
                navigator_target_slug(&marker.target)
            );
            let surface_handle = navigator_surface.clone();
            let button = Button::new(
                SharedString::from(control_selector.clone()),
                handle,
                *theme,
                MotionPolicy::Reduced,
                ButtonVariant::Ghost,
                ButtonSize::Small,
                ButtonContent::text(marker.label.clone()),
            )
            .expect("turn-navigator button configuration is valid")
            .focus_visibility(FocusVisibility::Visible)
            .debug_selector(control_selector)
            .on_activate(move |_, _, app| {
                let _ = surface_handle.update(app, |surface, cx| {
                    surface.request_scroll(target.clone(), cx);
                });
            });
            rail = rail.child(button);
        }
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
    use gpui::{Entity, TestAppContext, VisualTestContext, px, size};

    use crate::conversation_scene::{
        ConversationScene, SceneDisclosure, SceneItem, SceneItemKind, SceneTurn, TurnNarration,
        TurnNarrationEntry,
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
                SceneItemKind::Activity { body: body() },
                Some(disclosure),
            ),
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
}
