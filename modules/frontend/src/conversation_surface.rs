//! Native GPUI conversation transcript surface.
//!
//! [`ConversationSurface`] is a deliberately thin renderer over the accepted
//! [`ConversationScene`](crate::conversation_scene::ConversationScene). The
//! scene owns ordering, grouping, disclosure values, narration, and bounded
//! display text; this module only paints those already-decided values and
//! reports typed interaction observations back to its controller.
//!
//! No durable state, domain records, or network work belongs here. Clipboard
//! confirmation uses a local presentation clock.
//! Message-body Markdown parsing is delegated to the shared renderer, and
//! local disclosure state never becomes a second source of truth. A
//! replacement scene is the only source of truth after a disclosure request
//! has been emitted.
//!
//! Per-frame cost is bounded by construction: the render loop builds only the
//! visible turn rows plus a bounded overscan (see
//! [`render_budget`](self::render_budget) and
//! [`transcript_window`](self::transcript_window)), and off-window turns paint
//! from remembered heights as placeholders. Rows are measured on the frame
//! they enter the window, and the FIFO head of the scroll-target queue is
//! force-built so scrolling to a far turn still resolves.

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
use artisan_ui::button::{
    AccessibleLabel, Button, ButtonContent, ButtonSize, ButtonVariant, FocusVisibility,
};
use artisan_ui::card::{CardStyle, compact_card, compact_card_content};
use artisan_ui::collapsible::Collapsible;
use artisan_ui::gradient::{hover_fill_gradient, vertical_gradient};
use artisan_ui::inline_code_text::{inline_runs, summary_line};
use artisan_ui::input_state::TextInputState;
use artisan_ui::markdown_cache::MarkdownParseReport;
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
use std::time::{Duration, Instant};

use crate::approval_presentation::ApprovalKind as PresentationApprovalKind;
use crate::conversation_scene::{
    ActivityCategory, ChangeSetBlock, CompactionBlock, ConversationScene, ErrorBlock,
    FileChangeStatus, ModelTransitionBlock, NativeFactBlock, PlanBlock, QuestionBlock,
    SceneDisclosure, SceneFileChange, SceneId, SessionDetail, SteeringBlock, TurnBlock,
    TurnFooterBlock, TurnFooterSettlement, TurnNarration, TurnScene, UsageInterruptionBlock,
    UserMessageBlock, WorkGroupBlock, WorkItem, activity_category, activity_presentation_label,
};
use crate::conversation_scroll_position::conversation_is_following;
use crate::conversation_turn_footer_policy::{COPY_RESPONSE_LABEL, TURN_ACTIONS_LABEL};
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
use crate::engine_observation_state::EngineObservationState;
use crate::native_composer_material::{
    GlassStrength, glass_blur_radius, glass_card_shadows, glass_foreground_base,
    glass_highlight_layer, glass_material_layer,
};
use crate::native_model_selector::{HoverRect, PickerScrollState, SlidingHoverState};
use crate::native_transport_service::{CommandSendError, NativeTransportCommand};
use crate::rich_link_titles::RichLinkTitleTable;
// Phase-1 split submodules (see conversation_surface/).

#[path = "conversation_surface/answer_state.rs"]
mod answer_state;
#[path = "conversation_surface/contract.rs"]
mod contract;
#[path = "conversation_surface/transcript_helpers.rs"]
mod transcript_helpers;

pub use answer_state::*;
pub use contract::*;
pub use transcript_helpers::*;

// Phase-2 split submodules (see conversation_surface/).

#[path = "conversation_surface/interactions.rs"]
mod interactions;
#[path = "conversation_surface/pending_rows.rs"]
mod pending_rows;
#[path = "conversation_surface/render_blocks.rs"]
mod render_blocks;
#[path = "conversation_surface/render_navigator.rs"]
mod render_navigator;
#[path = "conversation_surface/render_sections.rs"]
mod render_sections;

// Answer outcome settlement (see conversation_surface/).

#[path = "conversation_surface/answer_settlement.rs"]
mod answer_settlement;

// Phase-3 split submodules (see conversation_surface/).

#[path = "conversation_surface/disclosure.rs"]
mod disclosure;
#[path = "conversation_surface/scroll_anchor.rs"]
mod scroll_anchor;

// Windowed transcript rendering (see conversation_surface/).

#[path = "conversation_surface/render_budget.rs"]
mod render_budget;
#[path = "conversation_surface/transcript_window.rs"]
mod transcript_window;

pub use transcript_window::{TranscriptShapeLedger, TranscriptWindowReport};

use disclosure::{disclosure_flight_panel, disclosure_frame};
pub(crate) use pending_rows::PendingMessageRow;
use pending_rows::SendEntrance;
use scroll_anchor::{RenderedScrollAnchor, ScrollAnchorRegistry, ViewportGeometry};
use transcript_window::TranscriptWindowState;

/// Native GPUI transcript surface over one immutable replacement scene.
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent viewport/disclosure flags are tracked separately by the render loop; packing them would conflate distinct fence states"
)]
pub struct ConversationSurface {
    composer_clearance: HashMap<gpui::WindowId, f32>,
    pending_messages: Vec<PendingMessageRow>,
    send_entrance: Option<SendEntrance>,
    scene: ConversationScene,
    /// Loaded user-message markers, rebuilt only when the scene changes.
    ///
    /// Render and the per-frame prepaint geometry both read this cache, so a
    /// frame never re-walks the transcript to rebuild navigator labels.
    navigator_markers: Rc<Vec<NavigatorMarker>>,
    /// Windowed transcript build state: remembered heights, cached offsets,
    /// and the latest frame diagnostics.
    ///
    /// Render-local and never authoritative: the accepted scene remains the
    /// only source of truth, and the height table is a bounded estimate used
    /// to place placeholders for off-window turns.
    transcript_window: RefCell<TranscriptWindowState>,
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
    smooth_bottom_pending: bool,
    smooth_bottom_active: bool,
    last_viewport_observation: Option<ViewportObservation>,
    pending_viewport_observation: Option<ViewportObservation>,
    pending_viewport_extent_change: bool,
    follow_bottom_pending: bool,
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
    /// chain starts closed even while live or failed. A user toggle or
    /// explicit navigation to a tool row pins its value. The work-session
    /// disclosure stays scene-owned; this map is presentation-only and never mutates the
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
    fn favicon(&self, destination: &str) -> Option<std::sync::Arc<gpui::RenderImage>> {
        self.titles.icon(destination)
    }

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

impl Render for ConversationSurface {
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI render builder assembles the transcript, navigator, disclosures, and measurement probes that share reactive state"
    )]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ArtisanTheme::for_mode(self.theme_mode);
        let entity = cx.entity();
        // Pure read of the live reduced-motion signal: no state writes, so
        // no notification can loop out of render.
        let status_motion = effective_status_motion(self.status_motion, cx.reduce_motion());
        // End-space height lives in framework window-local state, not on the
        // entity: two windows showing one surface measure different
        // viewports, and a shared scalar could never converge for both.
        if self.smooth_bottom_pending {
            self.smooth_bottom_pending = false;
            let current = f32::from(self.scroll_handle.offset().y);
            let maximum = f32::from(self.scroll_handle.max_offset().y).max(0.0);
            self.transcript_scroll.cancel_to(current, maximum);
            self.transcript_scroll
                .push(current, -maximum - current, maximum);
            self.smooth_bottom_active = true;
            self.schedule_transcript_scroll_frame(window, cx);
        }
        let end_space = window.use_state(cx, |_, _| 0.0_f32);
        let end_space_px = (*end_space.read(cx)).max(
            self.composer_clearance
                .get(&window.window_handle().window_id())
                .copied()
                .unwrap_or(0.0),
        );
        if self.follow_bottom_pending {
            self.follow_bottom_pending = false;
            // Like Electron's ResizeObserver, let the sent turn's reserved
            // space absorb growth before handing over to tail following.
            let floor = TRANSCRIPT_END_SPACE_PX.max(
                self.composer_clearance
                    .get(&window.window_handle().window_id())
                    .copied()
                    .unwrap_or(0.0),
            );
            if end_space_px <= floor && !self.transcript_scroll.active() {
                self.scroll_handle.scroll_to_bottom();
            }
        }
        // The reader's current navigator marker is geometry-derived like
        // end space, so it lives in the same window-local state: two
        // windows sharing one surface converge independently.
        let navigator_active = window.use_state(cx, |_, _| None::<String>);
        // Keep 32 px between turns. Settled footer controls occupy their own
        // flow row inside each turn's padded reading column.
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
        // Plan the build window before anything renders: only visible rows
        // plus bounded overscan build real subtrees, and footer focus handles
        // are ensured for exactly those rows.
        let built = self.plan_transcript_build();
        self.sync_footer_focus(&built, cx);
        {
            let mut anchors = ScrollAnchorRegistry {
                handle: &self.scroll_handle,
                previous: &previous_anchors,
                next_element_id: 0,
                rendered: &mut rendered_anchors,
            };
            for (index, turn) in self.scene.turn_scenes().iter().enumerate() {
                if built.contains(index) {
                    transcript = transcript.child(self.render_turn(
                        turn,
                        &entity,
                        &theme,
                        &mut anchors,
                        &mut *window,
                        status_motion,
                        cx,
                    ));
                } else {
                    // One placeholder per off-window turn keeps the child
                    // count and order stable for every prepaint listener.
                    transcript = transcript.child(self.render_turn_placeholder(index));
                }
            }
            self.finish_transcript_window(&built);
            for (index, row) in self.pending_messages.iter().enumerate() {
                let selector = format!("pending-send-{index}");
                let block = UserMessageBlock {
                    id: SceneId::parse(selector.clone()).expect("bounded pending row identity"),
                    body: row.text.clone(),
                    attachments: row.attachments.clone(),
                    disclosure: None,
                };
                // The Forge row paints with no status label beneath it:
                // neither its queued state nor a waiting reason reserves
                // label space under the bubble.
                transcript = transcript.child(
                    div()
                        .w_full()
                        .max_w(px(TRANSCRIPT_PROSE_WIDTH_PX))
                        .mx_auto()
                        .px(px(TRANSCRIPT_GUTTER_PX))
                        .flex()
                        .flex_col()
                        .items_end()
                        .gap(px(8.0))
                        .child(self.render_user_message(&block, selector, &theme, cx)),
                );
            }
            // Long transcripts reserve anchoring room. Short conversations
            // keep zero end space. Cancel the inter-turn gap before this
            // spacer so a zero-height spacer cannot create overflow.
            transcript = transcript.child(
                div()
                    .w_full()
                    .h(px(end_space_px))
                    .mt(-theme.spacing.steps(8.0))
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
        // is not paint evidence and must never mint painted custody. The
        // handler also records measured heights for the built rows, feeding
        // the next frame's window plan.
        transcript = transcript.on_children_prepainted(move |children_bounds, window, app| {
            let _ = surface.update(app, |surface, cx| {
                surface.handle_transcript_prepaint(
                    &children_bounds,
                    &paint_token,
                    &end_space_state,
                    &navigator_active_state,
                    window,
                    cx,
                );
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
            if let Ok(button) = Button::new(
                JUMP_TO_LATEST_SELECTOR,
                self.jump_to_latest_focus.clone(),
                theme,
                MotionPolicy::Reduced,
                ButtonVariant::Ghost,
                ButtonSize::IconSmall,
                ButtonContent::icon_only(
                    AssetId::TABLER_CHEVRON_DOWN,
                    AccessibleLabel::new("Jump to latest").expect("nonempty label"),
                ),
            ) {
                let button = button
                    .corner_radius(px(16.0))
                    .focus_visibility(FocusVisibility::Visible)
                    .debug_selector(JUMP_TO_LATEST_SELECTOR)
                    .on_activate(move |_, _, app| {
                        let _ = surface.update(app, |surface, cx| {
                            if surface
                                .enqueue_action(ConversationSurfaceAction::JumpToLatestRequested)
                            {
                                cx.notify();
                            }
                        });
                    });
                root = root.child(
                    div()
                        .absolute()
                        .left(px(0.0))
                        .right(px(0.0))
                        .bottom(px((self
                            .composer_clearance
                            .get(&window.window_handle().window_id())
                            .copied()
                            .unwrap_or(24.0)
                            - 16.0)
                            .max(8.0)))
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .relative()
                                .size(px(32.0))
                                .rounded_full()
                                .overflow_hidden()
                                .backdrop_blur(glass_blur_radius(GlassStrength::Quiet))
                                .bg(glass_foreground_base(&theme))
                                .shadow(glass_card_shadows())
                                .child(glass_material_layer(GlassStrength::Quiet, px(16.0)))
                                .child(glass_highlight_layer(GlassStrength::Quiet, px(16.0)))
                                .child(button),
                        ),
                );
            }
        }
        let active_navigator = navigator_active.read(cx).clone();
        if let Some(rail) =
            self.render_turn_navigator(&entity, &theme, window, cx, active_navigator.as_deref())
        {
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

impl DetailRow<'_> {
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
    if block.session_details.is_empty() {
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
    } else {
        let mut rows: Vec<(u64, DetailRow<'_>)> = block
            .session_details
            .iter()
            .map(|detail| match detail {
                SessionDetail::Assistant {
                    id, body, ordinal, ..
                } => (
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
                    id,
                    summary,
                    ordinal,
                    ..
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
    }
}

#[cfg(test)]
#[path = "conversation_surface/tests.rs"]
mod tests;
