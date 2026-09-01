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
    AnyElement, Context, Div, Entity, FocusHandle, FontWeight, IntoElement, Render, ScrollHandle,
    SharedString, Window, div,
    prelude::{InteractiveElement as _, ParentElement as _, Styled as _},
    px,
};

use crate::conversation_scene::{
    ChangeSetBlock, CompactionBlock, ConversationScene, ErrorBlock, FileChangeStatus,
    ModelTransitionBlock, NativeFactBlock, PlanBlock, QuestionBlock, SceneDisclosure,
    SceneFileChange, SceneId, SteeringBlock, TurnBlock, TurnFooterBlock, TurnNarration, TurnScene,
    UsageInterruptionBlock, UserMessageBlock, WorkGroupBlock, WorkItem,
};
use crate::conversation_scroll_position::conversation_is_following;

/// Stable debug selector for the conversation surface root.
pub const CONVERSATION_SURFACE_SELECTOR: &str = "artisan-conversation-surface";

/// Stable debug selector derived by [`ScrollArea`] for the transcript viewport.
pub const CONVERSATION_VIEWPORT_SELECTOR: &str = "artisan-conversation-surface-viewport";

/// Stable debug selector for the detached-reader jump control.
pub const JUMP_TO_LATEST_SELECTOR: &str = "artisan-conversation-surface-jump-to-latest";

/// Alias for callers that use the shorter root-selector vocabulary.
pub const ROOT_SELECTOR: &str = CONVERSATION_SURFACE_SELECTOR;

/// Alias for callers that use the shorter viewport-selector vocabulary.
pub const VIEWPORT_SELECTOR: &str = CONVERSATION_VIEWPORT_SELECTOR;

/// Maximum number of typed observations retained before new observations are
/// refused. Dropping the newest observation keeps the outbox bounded and
/// preserves the order of observations already accepted by the controller.
pub const CONVERSATION_SURFACE_MAX_ACTIONS: usize = 256;

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
    actions: Vec<ConversationSurfaceAction>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ViewportGeometry {
    scroll_top: f64,
    viewport_height: f64,
    scroll_height: f64,
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
            actions: Vec::new(),
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

    fn schedule_viewport_observation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.viewport_observation_scheduled {
            return;
        }
        self.viewport_observation_scheduled = true;

        let entity = cx.entity().downgrade();
        window.on_next_frame(move |_, app| {
            let _ = entity.update(app, |surface, cx| {
                surface.viewport_observation_scheduled = false;
                let offset = surface.scroll_handle.offset();
                let bounds = surface.scroll_handle.bounds();
                let max_offset = surface.scroll_handle.max_offset();
                let scroll_top = -f64::from(offset.y);
                let viewport_height = f64::from(bounds.size.height);
                let scroll_height = viewport_height + f64::from(max_offset.height);
                let geometry = ViewportGeometry {
                    scroll_top,
                    viewport_height,
                    scroll_height,
                };
                let geometry_changed = surface.last_viewport_geometry != Some(geometry);
                surface.last_viewport_geometry = Some(geometry);
                if !geometry_changed {
                    return;
                }
                let observation = ViewportObservation {
                    first_visible: None,
                    last_visible: None,
                    at_bottom: conversation_is_following(
                        scroll_top,
                        scroll_height,
                        viewport_height,
                    ),
                };
                let _ = surface.observe_viewport(observation, cx);
            });
        });
    }

    fn render_turn(
        &self,
        turn: &TurnScene,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
    ) -> AnyElement {
        let selector = turn_selector(&turn.turn_id);
        let mut turn_element = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(4.0));
        turn_element = turn_element.debug_selector(move || selector.clone());

        for block in turn.blocks() {
            if let Some(element) = self.render_block(&turn.turn_id, block, entity.clone(), theme) {
                turn_element = turn_element.child(element);
            }
        }

        turn_element.into_any_element()
    }

    fn render_block(
        &self,
        turn_id: &TurnId,
        block: &TurnBlock,
        entity: Entity<Self>,
        theme: &ArtisanTheme,
    ) -> Option<AnyElement> {
        let selector = block_selector(turn_id, block);
        match block {
            TurnBlock::UserMessage(block) => {
                Some(self.render_user_message(block, selector, entity, theme))
            }
            TurnBlock::AssistantMessage(block) => {
                Some(self.render_assistant_message(block, selector, entity, theme))
            }
            TurnBlock::WorkGroup(block) => {
                Some(self.render_work_group(turn_id, block, selector, entity, theme))
            }
            TurnBlock::Compaction(block) => {
                Some(self.render_compaction(block, selector, entity, theme))
            }
            TurnBlock::ChangeSet(block) => {
                Some(self.render_change_set(block, selector, entity, theme))
            }
            TurnBlock::Plan(block) => Some(self.render_plan(block, selector, entity, theme)),
            TurnBlock::Approval(block) => {
                Some(self.render_approval(block, selector, entity, theme))
            }
            TurnBlock::Question(block) => {
                Some(self.render_question(block, selector, entity, theme))
            }
            TurnBlock::Error(block) => Some(self.render_error(block, selector, entity, theme)),
            TurnBlock::UsageInterruption(block) => {
                Some(self.render_usage_interruption(block, selector, entity, theme))
            }
            TurnBlock::ModelTransition(block) => {
                Some(self.render_model_transition(block, selector, entity, theme))
            }
            TurnBlock::NativeFact(block) => {
                Some(self.render_native_fact(block, selector, entity, theme))
            }
            TurnBlock::SteeringLabel(block) => Some(Self::render_steering(block, selector, theme)),
            TurnBlock::TurnStatus(block) => Self::render_status(block, selector, theme),
            TurnBlock::TurnFooter(block) => Some(Self::render_footer(block, selector, theme)),
        }
    }

    fn render_user_message(
        &self,
        block: &UserMessageBlock,
        selector: String,
        entity: Entity<Self>,
        theme: &ArtisanTheme,
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
        )
    }

    fn render_assistant_message(
        &self,
        block: &crate::conversation_scene::AssistantMessageBlock,
        selector: String,
        entity: Entity<Self>,
        theme: &ArtisanTheme,
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
        )
    }

    fn render_text_block(
        &self,
        params: TextBlockRender<'_>,
        entity: Entity<Self>,
        theme: &ArtisanTheme,
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
                disclosure,
                selector,
                style,
            },
            compact_card_content(style).child(card_heading(title, theme)),
            compact_card_content(style).child(body_text(body, theme)),
            entity,
        )
    }

    fn render_markdown_text_block(
        &self,
        params: TextBlockRender<'_>,
        entity: Entity<Self>,
        theme: &ArtisanTheme,
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
                disclosure,
                selector,
                style,
            },
            compact_card_content(style).child(card_heading(title, theme)),
            compact_card_content(style).child(rendered_body),
            entity,
        )
    }

    fn render_work_group(
        &self,
        turn_id: &TurnId,
        block: &WorkGroupBlock,
        selector: String,
        entity: Entity<Self>,
        theme: &ArtisanTheme,
    ) -> AnyElement {
        let style = CardStyle::resolve(*theme);
        let group_id = block
            .items
            .first()
            .map(work_item_id)
            .cloned()
            .or_else(|| SceneId::parse(turn_id.as_str()).ok());
        let title = block
            .label
            .map_or_else(|| "Work".to_owned(), format_work_group_label);

        let mut items = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(3.0));
        for item in &block.items {
            items = items.child(Self::render_work_item(item, &selector, theme));
        }

        let Some(group_id) = group_id else {
            let fallback_selector = selector.clone();
            let mut card = compact_card(style).w_full();
            card = card.debug_selector(move || fallback_selector.clone());
            return card
                .child(compact_card_content(style).child(card_heading(title, theme)))
                .child(compact_card_content(style).child(items))
                .into_any_element();
        };

        self.render_controlled_card(
            ControlledCardOptions {
                id: group_id,
                disclosure: block.disclosure,
                selector,
                style,
            },
            compact_card_content(style).child(card_heading(title, theme)),
            compact_card_content(style).child(items),
            entity,
        )
    }

    fn render_work_item(item: &WorkItem, group_selector: &str, theme: &ArtisanTheme) -> AnyElement {
        let (id, title, text) = match item {
            WorkItem::Reasoning { id, body, .. } => (id, "Reasoning", body.as_str()),
            WorkItem::Activity { id, body, .. } => (id, "Activity", body.as_str()),
            WorkItem::WorkSession { id, title, .. } => (id, "Work session", title.as_str()),
        };
        let selector = format!("{group_selector}-item-{}", id.as_str());
        let mut item_element = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(1.0));
        item_element = item_element.debug_selector(move || selector.clone());
        item_element = item_element
            .child(card_heading(title, theme))
            .child(body_text(text, theme));
        item_element.into_any_element()
    }

    fn render_compaction(
        &self,
        block: &CompactionBlock,
        selector: String,
        entity: Entity<Self>,
        theme: &ArtisanTheme,
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
        )
    }

    fn render_change_set(
        &self,
        block: &ChangeSetBlock,
        selector: String,
        entity: Entity<Self>,
        theme: &ArtisanTheme,
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
                disclosure: block.disclosure,
                selector,
                style,
            },
            compact_card_content(style).child(header),
            compact_card_content(style).child(rows),
            entity,
        )
    }

    fn render_plan(
        &self,
        block: &PlanBlock,
        selector: String,
        entity: Entity<Self>,
        theme: &ArtisanTheme,
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
                disclosure: block.disclosure,
                selector,
                style,
            },
            compact_card_content(style).child(card_heading("Plan", theme)),
            compact_card_content(style).child(content),
            entity,
        )
    }

    fn render_approval(
        &self,
        block: &crate::conversation_scene::ApprovalBlock,
        selector: String,
        entity: Entity<Self>,
        theme: &ArtisanTheme,
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
        )
    }

    fn render_question(
        &self,
        block: &QuestionBlock,
        selector: String,
        entity: Entity<Self>,
        theme: &ArtisanTheme,
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
        )
    }

    fn render_error(
        &self,
        block: &ErrorBlock,
        selector: String,
        entity: Entity<Self>,
        theme: &ArtisanTheme,
    ) -> AnyElement {
        let style = CardStyle::resolve(*theme);
        let alert = Alert::from_theme(*theme, AlertVariant::Destructive)
            .title("Error")
            .description(block.message.clone())
            .debug_selector(format!("{selector}-alert"));
        self.render_controlled_card(
            ControlledCardOptions {
                id: block.id.clone(),
                disclosure: block.disclosure,
                selector,
                style,
            },
            compact_card_content(style).child(card_heading("Error", theme)),
            alert,
            entity,
        )
    }

    fn render_usage_interruption(
        &self,
        block: &UsageInterruptionBlock,
        selector: String,
        entity: Entity<Self>,
        theme: &ArtisanTheme,
    ) -> AnyElement {
        let style = CardStyle::resolve(*theme);
        let alert = Alert::from_theme(*theme, AlertVariant::Default)
            .title("Usage interruption")
            .description(block.detail.clone())
            .debug_selector(format!("{selector}-alert"));
        self.render_controlled_card(
            ControlledCardOptions {
                id: block.id.clone(),
                disclosure: block.disclosure,
                selector,
                style,
            },
            compact_card_content(style).child(card_heading("Usage interruption", theme)),
            alert,
            entity,
        )
    }

    fn render_model_transition(
        &self,
        block: &ModelTransitionBlock,
        selector: String,
        entity: Entity<Self>,
        theme: &ArtisanTheme,
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
        )
    }

    fn render_native_fact(
        &self,
        block: &NativeFactBlock,
        selector: String,
        entity: Entity<Self>,
        theme: &ArtisanTheme,
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
        )
    }

    fn render_steering(
        block: &SteeringBlock,
        selector: String,
        theme: &ArtisanTheme,
    ) -> AnyElement {
        let mut label = div()
            .w_full()
            .min_w_0()
            .text_size(theme.typography.label_text)
            .text_color(theme.colors.muted_foreground.to_paint())
            .whitespace_normal()
            .child(block.label.clone());
        label = label.debug_selector(move || selector.clone());
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
        entity: Entity<Self>,
    ) -> AnyElement {
        let ControlledCardOptions {
            id,
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
            let action_id = id;
            collapsible = collapsible.on_change(move |requested_open, _, _, app| {
                let action = ConversationSurfaceAction::DisclosureToggleRequested {
                    id: action_id.clone(),
                    requested_open,
                };
                entity.update(app, |surface, cx| {
                    if surface.enqueue_action(action) {
                        cx.notify();
                    }
                });
            });
        }

        let mut card = compact_card(style).w_full();
        card = card.debug_selector(move || selector.clone());
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
        for turn in self.scene.turn_scenes() {
            transcript = transcript.child(self.render_turn(turn, &entity, &theme));
        }

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
        root
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
