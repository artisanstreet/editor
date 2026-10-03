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
//! force-built so scrolling to a far turn still resolves. Each built turn is
//! its own cached child view (see [`turn_row`](self::turn_row)): an animation
//! or change in one turn re-renders that row, and clean rows replay their
//! previous frame.

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
use artisan_ui::copy_feedback::{COPY_FEEDBACK_WINDOW, copy_feedback_icon, copy_feedback_progress};
use artisan_ui::gradient::{hover_fill_gradient, vertical_gradient};
use artisan_ui::inline_code_text::inline_runs;
use artisan_ui::input_state::TextInputState;
use artisan_ui::markdown_cache::MarkdownParseReport;
use artisan_ui::markdown_renderer::{MarkdownBodyTone, MarkdownRenderer, RichLinkTitleSource};
use artisan_ui::markdown_reveal::RevealFade;
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
    ScrollHandle, ScrollWheelEvent, SharedString, Stateful, Window, anchored, canvas, deferred,
    div, point,
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
    AnswerSettlement, RespondApprovalAction, RespondQuestionAction, approval_command,
    mint_answer_request_id, pair_answer_failure, pair_approval_answer, pair_question_answer,
    pending_approval_label, question_command,
};
use crate::engine_observation_state::EngineObservationState;
use crate::native_model_selector::{HoverRect, PickerScrollState, SlidingHoverState};
use crate::native_transport_service::{CommandSendError, NativeTransportCommand};
use crate::rich_link_titles::RichLinkTitleTable;
use artisan_ui::glass::{
    GlassStrength, glass_blur_radius, glass_card_shadows, glass_foreground_base,
    glass_highlight_layer, glass_material_layer,
};
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
#[path = "conversation_surface/reply_reveal.rs"]
mod reply_reveal;
#[path = "conversation_surface/work_motion.rs"]
mod work_motion;

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
#[path = "conversation_surface/turn_row.rs"]
mod turn_row;

pub use transcript_window::{TranscriptShapeLedger, TranscriptWindowReport};

use disclosure::{disclosure_flight_panel, disclosure_frame};
pub(crate) use pending_rows::PendingMessageRow;
use pending_rows::SendEntrance;
use render_blocks::user_message_element;
use scroll_anchor::{RenderedScrollAnchor, ScrollAnchorRegistry, ViewportGeometry};
use transcript_window::{TranscriptShaper, TranscriptWindowState};
use turn_row::{TurnRowHandle, TurnRowServices, TurnRowSource, TurnRowView};

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
    /// The budgeted Markdown shaper, its parse cache, and the rich-link
    /// title table, shared with every turn row.
    ///
    /// Rows probe the table while their links flatten and record misses
    /// here; the surface queues each new destination exactly once before
    /// notifying the host.
    shaper: Rc<TranscriptShaper>,
    /// Bumped whenever the rich-link table changes, so rows showing a title
    /// or favicon repaint instead of replaying a stale label.
    rich_link_generation: u64,
    /// Retained turn rows keyed by turn id, pruned on scene replacement.
    ///
    /// Each turn renders from its own cached child view (see
    /// [`turn_row`](self::turn_row)); a row survives window changes so a
    /// turn scrolled out and back keeps its entity and retained state.
    turn_rows: HashMap<String, TurnRowHandle>,
    /// Bumped by every scene replacement, so an unchanged scene never
    /// compares turn contents.
    scene_generation: u64,
    /// When the oldest scene replacement not yet painted arrived. A window
    /// that is minimized or hidden stops painting while scenes keep
    /// arriving; its next paint catches up instead of animating them all.
    unpainted_since: Option<Instant>,
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
    /// The transcript tail (last Forge outbox row, else last turn) the
    /// reader was last brought to by an automatic follow.
    ///
    /// A follow that finds a different tail always brings it into view: the
    /// end space reserves exactly the room that aligns a new turn at the top
    /// of the viewport, which is where a sent message must land. A follow
    /// for the same tail is growth of that turn, which the reserved space
    /// absorbs until it is exhausted; only then does the follow scroll.
    followed_tail: Option<String>,
    /// A new tail was just followed and its end space has not been measured
    /// yet: the follow repeats once after that measurement lands.
    ///
    /// The end space is measured in prepaint and sized into the next frame,
    /// so the scroll that follows a new tail lands one measurement short of
    /// where the reserve finally puts the transcript end. One more follow,
    /// past the reserve gate, settles the tail exactly.
    tail_follow_settling: bool,
    last_viewport_geometry: Option<ViewportGeometry>,
    /// The oldest loaded turn the surface last asked for older turns above.
    ///
    /// The request repeats only once that turn changes (a page arrived) or
    /// the reader leaves the start and returns, so one page in flight is
    /// never asked for twice.
    earlier_turns_wanted_for: Option<TurnId>,
    /// Whether the thread has turns before the loaded ones. The host says
    /// so from the canonical window; a surface that holds a thread's first
    /// turn never asks for older ones.
    earlier_turns_available: bool,
    /// User messages of the turns before the loaded ones, oldest first: the
    /// turn navigator lists them in front of the loaded markers.
    earlier_turn_markers: Vec<(ItemId, String)>,
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
    /// The anchor registries of this frame's built rows, in scene order.
    ///
    /// Each row owns its registry and rebuilds it when it renders; a cached
    /// row keeps the one its last render produced, which is exactly what it
    /// still paints.
    scroll_anchors: Vec<Rc<RefCell<Vec<RenderedScrollAnchor>>>>,
    /// The pending head target found no anchor in the latest render and is
    /// waiting one frame for the rows to render their current anchors.
    scroll_targets_await_rows: bool,
    /// Anchor scrolls executed whose next-frame GPUI callback has not run.
    anchor_scrolls_in_flight: usize,
    scroll_anchor_paint_token: Option<Rc<()>>,
    /// Focus handles for loaded-turn navigator controls, keyed by stable
    /// target identity (`item:<id>` or `scene:<id>`).
    ///
    /// Handles persist while their target remains in the scene and are
    /// pruned on scene replacement; a focused control that disappears
    /// returns focus to the transcript.
    navigator_focus: HashMap<String, FocusHandle>,
    /// Whether the pointer is over the turn navigator's tick rail.
    ///
    /// The reference hides labels until rail hover or tick focus; at rest
    /// only the tick column paints, so full message texts never float beside
    /// the transcript. The floating menu is open while the pointer is over
    /// the rail or the menu, or while a tick holds keyboard focus.
    navigator_rail_hovered: bool,
    /// Whether the pointer is over the navigator's floating menu, including
    /// the gap between the menu and the rail.
    navigator_menu_hovered: bool,
    /// Dedicated scroll handle for the navigator menu's capped label list.
    ///
    /// Long threads scroll the menu independently of the transcript through
    /// the same bounded tracking the thread screen uses for viewports.
    navigator_scroll: ScrollHandle,
    /// Shared hover pill for navigator menu rows, reused from the model
    /// picker.
    ///
    /// Row probes measure into this state and the pill paints the retained
    /// flight; hovering a row selects, leaving the navigator hides.
    /// Reference pill behavior without a second motion invention.
    navigator_hover: Rc<RefCell<SlidingHoverState>>,
    /// Measured navigator list bounds backing pill-relative coordinates.
    ///
    /// Window-space bounds like the picker's surface bounds, so row origins
    /// subtract to list-relative pill rects with plain pixel arithmetic.
    navigator_hover_surface: Rc<RefCell<Option<gpui::Bounds<gpui::Pixels>>>>,
    /// Focused navigator tick identity; selecting the pill only on arrival
    /// keeps a later pointer selection from being overridden every render
    /// while the tick retains keyboard focus.
    navigator_focused_key: Option<String>,
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
    trace_groups_open: Rc<RefCell<HashMap<String, bool>>>,
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
    /// The thread that owns every engine approval/question row here.
    ///
    /// The host supplies it through [`Self::set_answer_thread`] when it
    /// mounts the surface; nothing ambient is read. The owning run and the
    /// engine's interaction identity ride on each block from its durable
    /// provenance, so a gesture submits exactly the identities the engine
    /// issued. Submit affordances stay disabled until the thread is known.
    answer_thread: Option<ThreadId>,
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
            // Nothing to travel (already at the end, or nothing overflows):
            // the jump is complete now. An armed flag with no frames to
            // clear it would report the next wheel tick as an interrupted
            // jump and detach a reader who never left the end.
            self.smooth_bottom_active = self.transcript_scroll.active();
            if self.smooth_bottom_active {
                self.schedule_transcript_scroll_frame(window, cx);
            } else {
                self.report_reached_end(cx);
            }
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
            // A new tail (a sent row, a delivered turn) is always brought
            // into view: the reserved end space aligns it at the top. For
            // the same tail, like Electron's ResizeObserver, let that
            // reserved space absorb growth before handing over to tail
            // following.
            let floor = TRANSCRIPT_END_SPACE_PX.max(
                self.composer_clearance
                    .get(&window.window_handle().window_id())
                    .copied()
                    .unwrap_or(0.0),
            );
            let tail = self.tail_identity();
            let new_tail = tail != self.followed_tail;
            if !self.transcript_scroll.active()
                && (new_tail || self.tail_follow_settling || end_space_px <= floor)
            {
                // The measurement that lands after a new tail's first layout
                // re-renders this surface; the follow stays armed for that
                // frame so the tail settles where its reserve puts it.
                self.follow_bottom_pending = new_tail;
                self.tail_follow_settling = new_tail;
                self.followed_tail = tail;
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
        // Plan the build window before anything renders: only visible rows
        // plus bounded overscan build real subtrees, and footer focus handles
        // are ensured for exactly those rows.
        let built = self.plan_transcript_build();
        self.sync_footer_focus(&built, cx);
        let rows = self.render_turn_rows(&built, status_motion, cx);
        transcript = transcript.children(rows);
        {
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
                        .child(user_message_element(
                            &block,
                            selector,
                            &theme,
                            self.message_images.as_ref(),
                            self.pending_row_entrance(index, cx.reduce_motion()),
                            cx,
                        )),
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

        let scroll_executed = self.drain_painted_scroll_targets(window, cx);
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
            // Deferred overlay at dropdown priority: the rail and its
            // floating menu paint above the composer dock exactly like the
            // reference `z-30`. The rail's layout stays in this tree; the
            // menu is anchored in window coordinates.
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
    Assistant {
        id: &'a SceneId,
        body: &'a str,
        /// Whether the prose is still arriving, so it streams in.
        streaming: bool,
    },
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
/// vec order. Callers paint the returned sequence verbatim. An assistant
/// message with no text yet is not a row: the Forge opens the item ahead of
/// its first text, and an empty row would paint a gap and split the tool
/// chain around it.
#[expect(
    clippy::too_many_lines,
    reason = "one mapping covers every legacy and session detail kind in paint order; splitting it would separate the two orderings it keeps identical"
)]
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
            .filter(|detail| {
                !matches!(detail, SessionDetail::Assistant { body, .. } if body.trim().is_empty())
            })
            .map(|detail| match detail {
                SessionDetail::Assistant {
                    id,
                    body,
                    ordinal,
                    provenance,
                    ..
                } => (
                    *ordinal,
                    DetailRow::Assistant {
                        id,
                        body: body.as_str(),
                        streaming: provenance_is_live(provenance.as_ref()),
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
        // Prose followed by any later row is finished, whatever its message
        // lifecycle says: a message can stay live until its turn ends, and a
        // body still counted as streaming keeps its last word held back.
        let last = rows.len().saturating_sub(1);
        for (_, row) in &mut rows[..last] {
            if let DetailRow::Assistant { streaming, .. } = row {
                *streaming = false;
            }
        }
        rows
    }
}

#[cfg(test)]
#[path = "conversation_surface/tests.rs"]
mod tests;

/// Whether a durable lifecycle means the item is still arriving.
pub(super) fn lifecycle_is_live(lifecycle: ConversationLifecycle) -> bool {
    matches!(
        lifecycle,
        ConversationLifecycle::Pending
            | ConversationLifecycle::Streaming
            | ConversationLifecycle::Active
            | ConversationLifecycle::Waiting
    )
}

/// Whether an item's durable attribution says it is still arriving.
fn provenance_is_live(provenance: Option<&crate::conversation_scene::ItemProvenance>) -> bool {
    provenance
        .and_then(|provenance| provenance.lifecycle)
        .is_some_and(lifecycle_is_live)
}
