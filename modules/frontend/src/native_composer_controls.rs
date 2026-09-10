//! Native GPUI run controls and composer feedback surfaces.
//!
//! This module is a controlled view over parent-owned composer state. It does
//! not start, stop, retry, withdraw, or otherwise mutate an async operation.
//! Pointer and keyboard activation both emit one of the bounded
//! [`NativeComposerControlsEvent`] values; the parent applies the event and
//! sends a fresh [`NativeComposerControlsSnapshot`] back through
//! [`NativeComposerControls::set_snapshot`]. In particular, a queued steering
//! row remains visible until the parent confirms its withdrawal or projection.
//!
//! The bottom row accepts a model-picker element from the parent. The parent
//! can also mount [`NativeComposerControls::render_lip`] and
//! [`NativeComposerControls::render_failure`] above its editor, preserving the
//! existing floating-card boundary instead of creating a second composer
//! panel. The optional context gauge is rendered as a sibling of that picker
//! and delegates its actual projection to [`crate::native_context_usage`].

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use artisan_assets::AssetId;
use artisan_ui::{
    button::{AccessibleLabel, Button, ButtonContent, ButtonSize, ButtonVariant, FocusVisibility},
    motion::MotionPolicy,
    theme::{ArtisanTheme, DesktopTheme, ThemeMode},
};
use gpui::prelude::{InteractiveElement as _, ParentElement as _, Styled as _};
use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Context, Div, ElementId, FocusHandle,
    Focusable, IntoElement, Render, Stateful, Task, Window, div, px,
};

use crate::composer_action_failure::ComposerActionFailure;
use crate::native_composer_visuals::{
    COMPOSER_LIP_MOTION_MS, QueuedSteerRow, START_NEW_THREAD_PROMPT_LABEL, SendButtonStill,
    composer_smooth_out,
};
use crate::native_context_usage::NativeContextUsage;

/// Stable selector for the controls component root.
pub const NATIVE_COMPOSER_CONTROLS_SELECTOR: &str = "artisan-native-composer-controls";
/// Stable selector for the bottom control row.
pub const NATIVE_COMPOSER_CONTROL_ROW_SELECTOR: &str = "artisan-native-composer-control-row";
/// Stable selector for the primary send/stop action.
pub const NATIVE_COMPOSER_PRIMARY_SELECTOR: &str = "artisan-native-composer-primary";
/// Stable selector for the run-time new-thread escape action.
pub const NATIVE_COMPOSER_NEW_THREAD_SELECTOR: &str = "artisan-native-composer-new-thread";
/// Stable selector for the transcript jump action.
pub const NATIVE_COMPOSER_JUMP_TO_LATEST_SELECTOR: &str = "artisan-native-composer-jump-to-latest";
/// Stable selector for the feedback banner.
pub const NATIVE_COMPOSER_FAILURE_SELECTOR: &str = "artisan-native-composer-action-failure";
/// Stable selector prefix for one failed-dispatch new-chat action.
pub const NATIVE_COMPOSER_FAILED_NEW_THREAD_SELECTOR: &str =
    "artisan-native-composer-failed-new-thread";

const ROW_SELECTOR_PREFIX: &str = "artisan-native-composer-steering-row";
const ROW_EDIT_SELECTOR_SUFFIX: &str = "edit";
const ROW_DISCARD_SELECTOR_SUFFIX: &str = "discard";
const FAILURE_RETRY_SELECTOR: &str = "artisan-native-composer-failure-retry";
const FAILURE_DISMISS_SELECTOR: &str = "artisan-native-composer-failure-dismiss";
const JUMP_TO_LATEST_LABEL: &str = "Jump to latest";

/// Immutable identity of one queued steering submission.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct QueuedSteeringIdentity {
    /// Command id minted for this exact queued submission.
    pub command_id: String,
    /// Generation allocated for this command id.
    pub generation: u64,
}

impl QueuedSteeringIdentity {
    /// Creates an identity without normalizing either component.
    #[must_use]
    pub fn new(command_id: impl Into<String>, generation: u64) -> Self {
        Self {
            command_id: command_id.into(),
            generation,
        }
    }
}

/// One parent-projected queued steering lip row.
///
/// The text is retained exactly as supplied. Rendering uses the existing
/// `QueuedSteerRow` policy for the trimmed/fallback label, while this type
/// keeps the command id and generation available to every emitted event.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PendingSteeringRow {
    /// Exact command/generation identity that fences edit and discard intents.
    pub identity: QueuedSteeringIdentity,
    /// Text originally submitted for the steer.
    pub text: String,
    /// Whether the parent still exposes a withdrawal route for this row.
    pub editable: bool,
}

impl PendingSteeringRow {
    /// Creates a queued row retaining the exact identity and text.
    #[must_use]
    pub fn new(
        command_id: impl Into<String>,
        generation: u64,
        text: impl Into<String>,
        editable: bool,
    ) -> Self {
        Self {
            identity: QueuedSteeringIdentity::new(command_id, generation),
            text: text.into(),
            editable,
        }
    }

    /// Returns the immutable command id.
    #[must_use]
    pub fn command_id(&self) -> &str {
        &self.identity.command_id
    }

    /// Returns the immutable generation.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.identity.generation
    }
}

/// One parent-projected terminally failed dispatch.
///
/// The identity binds the recovery action to the exact failed message and
/// the generation that projected it: never to current composer text or a
/// run id. The reason is the verbatim dispatcher diagnostic. There is no
/// retry affordance: a terminal failure will never send on this thread, so
/// the only action is the explicit new chat.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FailedDispatchRow {
    /// Exact command/generation identity that fences the new-chat intent.
    pub identity: QueuedSteeringIdentity,
    /// Prompt text of the failed send, empty for image-only failures.
    pub text: String,
    /// Whether the failed prompt carries image attachments.
    pub has_attachments: bool,
    /// Verbatim dispatcher diagnostic for the terminal failure.
    pub reason: String,
}

impl FailedDispatchRow {
    /// Creates a failed row retaining the exact identity, text, and reason.
    #[must_use]
    pub fn new(
        command_id: impl Into<String>,
        generation: u64,
        text: impl Into<String>,
        has_attachments: bool,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            identity: QueuedSteeringIdentity::new(command_id, generation),
            text: text.into(),
            has_attachments,
            reason: reason.into(),
        }
    }

    /// Returns the immutable command id.
    #[must_use]
    pub fn command_id(&self) -> &str {
        &self.identity.command_id
    }

    /// Returns the immutable generation.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.identity.generation
    }
}

/// An actionable failure owned by the parent.
///
/// `failure_id` fences a delayed click against a newer failure notice. The
/// retry action is shown only when `retryable` is true; the parent remains the
/// owner of what retry means and when the notice is cleared.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeComposerFailure {
    /// Monotonic or otherwise unique parent-owned notice identity.
    pub failure_id: u64,
    /// Existing title/description presentation value.
    pub failure: ComposerActionFailure,
    /// Whether the failed operation can be retried from this surface.
    pub retryable: bool,
}

impl NativeComposerFailure {
    /// Creates an actionable failure notice.
    #[must_use]
    pub fn new(
        failure_id: u64,
        title: impl Into<String>,
        description: impl Into<String>,
        retryable: bool,
    ) -> Self {
        Self {
            failure_id,
            failure: ComposerActionFailure::new(title, description),
            retryable,
        }
    }
}

/// Parent-owned snapshot rendered by this module.
///
/// `run_id` is the current authoritative run owner for both stop/new-thread
/// intents and context-usage attribution. It may remain populated after a run
/// becomes idle when the parent still wants to show that run's final usage;
/// it must be `None` when there is no current usage owner. Readiness booleans
/// are already projected by the parent and are never inferred from draft text
/// here.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NativeComposerControlsSnapshot {
    /// Current run identity, when a run or its final usage still owns the view.
    pub run_id: Option<String>,
    /// Whether the primary action currently represents an active run.
    pub run_active: bool,
    /// Whether an abort request is already in flight.
    pub cancelling: bool,
    /// Whether the parent has an abort operation for the current run.
    pub abort_available: bool,
    /// Whether a new send can be admitted by the parent.
    pub send_ready: bool,
    /// Optional explanation for a disabled idle send action.
    pub send_blocked_reason: Option<String>,
    /// Whether the parent can carry the draft into a new thread while running.
    pub new_thread_ready: bool,
    /// Owner-supplied transcript state for the jump action.
    pub show_jump_to_latest: bool,
    /// Pending steering rows. Rows leave only when this projection changes.
    pub pending_steering: Vec<PendingSteeringRow>,
    /// Terminally failed dispatches, newest first. Rows leave only when this
    /// projection changes. Each row offers the explicit new chat only; a
    /// terminal failure is never retried on its thread.
    pub failed_dispatches: Vec<FailedDispatchRow>,
    /// Whether the new-chat recovery is currently admissible. The parent
    /// projects composer emptiness here so a conflicting draft disables the
    /// action with a clear state instead of stranding either prompt.
    pub failed_new_chat_ready: bool,
    pub queue_status: Option<String>,
    pub queue_retry: Option<String>,
    /// Last actionable failure, if any.
    pub failure: Option<NativeComposerFailure>,
    /// Actual usage reported by a run, if one is available.
    pub context_usage: Option<NativeContextUsage>,
    /// Controlled open value for the context details popover.
    pub context_usage_open: bool,
    /// Global surface disablement. It suppresses all run-scoped controls.
    pub disabled: bool,
}

impl NativeComposerControlsSnapshot {
    /// Returns an idle snapshot with no fabricated readiness or telemetry.
    #[must_use]
    pub fn idle() -> Self {
        Self::default()
    }

    /// Returns whether the snapshot still contains one exact steering identity.
    #[must_use]
    pub fn contains_steering(&self, identity: &QueuedSteeringIdentity) -> bool {
        self.pending_steering
            .iter()
            .any(|row| &row.identity == identity)
    }
}

/// One bounded intent emitted by pointer or keyboard activation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeComposerControlsEvent {
    RetryQueue,
    /// Request a new message submission. There is no run identity yet; the
    /// parent owns the draft/thread identity and mints the next run id.
    SendRequested,
    /// Request cancellation of one exact active run.
    StopRequested { run_id: String },
    /// Carry the ready draft into a new thread while one exact run is active.
    StartNewThreadWithPrompt { run_id: String },
    /// Carry one exact terminally failed prompt into a new thread as an
    /// unsent draft. The parent resolves the identity against its
    /// generation-fenced failed projection; nothing is ever read from the
    /// current composer text or an unrelated run.
    StartNewThreadWithFailedPrompt {
        /// Original queue command id of the failed row that was clicked.
        command_id: String,
        /// Generation of the row that was clicked.
        generation: u64,
    },
    /// Ask the transcript owner to scroll to its latest content.
    JumpToLatest,
    /// Recall one exact queued steer into the editor.
    EditQueuedSteer {
        /// Command id of the row that was clicked.
        command_id: String,
        /// Generation of the row that was clicked.
        generation: u64,
    },
    /// Ask the parent to withdraw one exact queued steer.
    DiscardQueuedSteer {
        /// Command id of the row that was clicked.
        command_id: String,
        /// Generation of the row that was clicked.
        generation: u64,
    },
    /// Dismiss one exact failure notice.
    DismissFailure { failure_id: u64 },
    /// Retry one exact failure notice.
    RetryFailure { failure_id: u64 },
    /// Request a controlled context-details open value.
    ContextUsageToggled { run_id: String, open: bool },
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum SteeringAction {
    Edit,
    Discard,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct SteeringFocusKey {
    identity: QueuedSteeringIdentity,
    action: SteeringAction,
}

/// Native GPUI owner of the controlled controls seam.
pub struct NativeComposerControls {
    snapshot: NativeComposerControlsSnapshot,
    primary_focus: FocusHandle,
    new_thread_focus: FocusHandle,
    jump_focus: FocusHandle,
    context_focus: FocusHandle,
    failure_retry_focus: FocusHandle,
    queue_retry_focus: FocusHandle,
    failure_dismiss_focus: FocusHandle,
    steering_focus: HashMap<SteeringFocusKey, FocusHandle>,
    failed_focus: HashMap<QueuedSteeringIdentity, FocusHandle>,
    /// Presentation-only lip motion state.
    ///
    /// `lip_row_nonces` gives each newly arrived steer a fresh entrance
    /// animation identity; `closing_lip_rows` keeps the last rows mounted
    /// for the collapse fade until the generation-fenced settle timer
    /// unmounts them. No queue identity or withdrawal route lives here.
    lip_row_nonces: HashMap<QueuedSteeringIdentity, u64>,
    lip_nonce_counter: u64,
    closing_lip_rows: Vec<PendingSteeringRow>,
    closing_lip_generation: u64,
    lip_settle_task: Option<Task<()>>,
}

impl gpui::EventEmitter<NativeComposerControlsEvent> for NativeComposerControls {}

impl Focusable for NativeComposerControls {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.primary_focus.clone()
    }
}

impl NativeComposerControls {
    /// Creates a controls entity over the supplied parent snapshot.
    #[must_use]
    pub fn new(snapshot: NativeComposerControlsSnapshot, cx: &mut Context<Self>) -> Self {
        Self {
            snapshot,
            primary_focus: cx.focus_handle().tab_index(50).tab_stop(true),
            new_thread_focus: cx.focus_handle().tab_index(49).tab_stop(false),
            jump_focus: cx.focus_handle().tab_index(0).tab_stop(false),
            context_focus: cx.focus_handle().tab_index(48).tab_stop(false),
            failure_retry_focus: cx.focus_handle().tab_index(10).tab_stop(false),
            queue_retry_focus: cx.focus_handle().tab_index(9).tab_stop(true),
            failure_dismiss_focus: cx.focus_handle().tab_index(11).tab_stop(false),
            steering_focus: HashMap::new(),
            failed_focus: HashMap::new(),
            lip_row_nonces: HashMap::new(),
            lip_nonce_counter: 0,
            closing_lip_rows: Vec::new(),
            closing_lip_generation: 0,
            lip_settle_task: None,
        }
    }

    /// Returns the current immutable snapshot.
    #[must_use]
    pub const fn snapshot(&self) -> &NativeComposerControlsSnapshot {
        &self.snapshot
    }

    /// Replaces the parent projection and retains no local operation state.
    ///
    /// A pending lip row is therefore removed only when the parent sends a
    /// snapshot without its identity, after the owner has confirmed the
    /// withdrawal/projection.
    pub fn set_snapshot(
        &mut self,
        snapshot: NativeComposerControlsSnapshot,
        cx: &mut Context<Self>,
    ) {
        if self.snapshot == snapshot {
            return;
        }
        let previous_rows = self.snapshot.pending_steering.clone();
        self.update_lip_motion(&previous_rows, &snapshot, cx);
        self.snapshot = snapshot;
        let pending = self.snapshot.pending_steering.clone();
        self.steering_focus.retain(|key, _| {
            pending
                .iter()
                .any(|row| row.identity == key.identity && row.editable)
        });
        let failed = self
            .snapshot
            .failed_dispatches
            .iter()
            .map(|row| row.identity.clone())
            .collect::<Vec<_>>();
        self.failed_focus
            .retain(|identity, _| failed.contains(identity));
        cx.notify();
    }

    /// Advances the presentation-only lip motion from the previous rows to
    /// the incoming snapshot.
    ///
    /// Newly arrived steers receive fresh entrance nonces so their mount
    /// fade replays; a lip that empties keeps its last rows for the
    /// collapse fade until a generation-fenced settle timer unmounts them.
    /// The timer mirrors the polished picker settle pattern and never
    /// touches queue identity: emission stays fenced on the snapshot.
    fn update_lip_motion(
        &mut self,
        previous_rows: &[PendingSteeringRow],
        snapshot: &NativeComposerControlsSnapshot,
        cx: &mut Context<Self>,
    ) {
        let was_open = !previous_rows.is_empty();
        let is_open = !snapshot.pending_steering.is_empty();
        if is_open {
            self.closing_lip_rows.clear();
            self.lip_settle_task = None;
            let present = snapshot
                .pending_steering
                .iter()
                .map(|row| &row.identity)
                .collect::<HashSet<_>>();
            self.lip_row_nonces
                .retain(|identity, _| present.contains(identity));
            for row in &snapshot.pending_steering {
                if !self.lip_row_nonces.contains_key(&row.identity) {
                    self.lip_nonce_counter = self.lip_nonce_counter.wrapping_add(1);
                    self.lip_row_nonces
                        .insert(row.identity.clone(), self.lip_nonce_counter);
                }
            }
        } else if was_open {
            self.closing_lip_rows = previous_rows.to_vec();
            self.closing_lip_generation = self.closing_lip_generation.wrapping_add(1);
            self.lip_row_nonces.clear();
            let generation = self.closing_lip_generation;
            self.lip_settle_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(COMPOSER_LIP_MOTION_MS))
                    .await;
                let _ = this.update(cx, |controls, controls_cx| {
                    if controls.closing_lip_generation == generation {
                        controls.closing_lip_rows.clear();
                        controls.lip_settle_task = None;
                        controls_cx.notify();
                    }
                });
            }));
        } else {
            self.closing_lip_rows.clear();
            self.lip_settle_task = None;
        }
    }

    /// Computes the current primary event without emitting it.
    #[must_use]
    pub fn primary_event(&self) -> Option<NativeComposerControlsEvent> {
        if self.snapshot.disabled {
            return None;
        }

        if self.snapshot.run_active {
            if self.snapshot.cancelling || !self.snapshot.abort_available {
                return None;
            }
            return self
                .snapshot
                .run_id
                .clone()
                .map(|run_id| NativeComposerControlsEvent::StopRequested { run_id });
        }

        self.snapshot
            .send_ready
            .then_some(NativeComposerControlsEvent::SendRequested)
    }

    /// Computes the run-time new-thread escape event without emitting it.
    #[must_use]
    pub fn new_thread_event(&self) -> Option<NativeComposerControlsEvent> {
        if self.snapshot.disabled || !self.snapshot.run_active || !self.snapshot.new_thread_ready {
            return None;
        }

        self.snapshot
            .run_id
            .clone()
            .map(|run_id| NativeComposerControlsEvent::StartNewThreadWithPrompt { run_id })
    }

    /// Builds one static lip row without actions or motion.
    ///
    /// Reference (`steering-lip.svelte:28-32`): `flex items-center gap-3
    /// py-2 pr-2 pl-5 text-base`, single-line truncated label. Both the live
    /// rows and the inert collapse-fade rows share this geometry. The `id`
    /// makes the row `Stateful`; animation converts to `AnyElement` later.
    fn lip_row_base(
        row_element_id: ElementId,
        row_selector: String,
        label: String,
        desktop_theme: DesktopTheme,
    ) -> Stateful<Div> {
        div()
            .id(row_element_id)
            .debug_selector(move || row_selector.clone())
            .w_full()
            .min_w(px(0.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.0))
            .pl(px(20.0))
            .pr(px(8.0))
            .py(px(8.0))
            .text_size(px(16.0))
            .line_height(px(24.0))
            .text_color(desktop_theme.secondary)
            .bg(desktop_theme.sidebar)
            .child(
                div()
                    .min_w(px(0.0))
                    .flex_1()
                    .truncate()
                    .child(label),
            )
    }

    /// Renders the pending steering lip above the parent-owned editor.
    #[must_use]
    pub fn render_lip(
        &mut self,
        theme: ArtisanTheme,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        if self.snapshot.pending_steering.is_empty()
            && self.snapshot.queue_status.is_none()
            && self.closing_lip_rows.is_empty()
        {
            return None;
        }

        let entity = cx.entity();
        let desktop_theme = DesktopTheme::neutral_dark();
        let mut lip = div()
            .id(ElementId::Name(
                "artisan-native-composer-steering-lip".into(),
            ))
            .debug_selector(|| "artisan-native-composer-steering-lip".to_owned())
            .w_full()
            .flex()
            .flex_col();

        for (index, row) in self
            .snapshot
            .pending_steering
            .clone()
            .into_iter()
            .enumerate()
        {
            let still = QueuedSteerRow::new(row.generation(), &row.text, row.editable);
            let identity = row.identity.clone();
            let row_selector = row_selector(&identity);
            let row_element_id = ElementId::Name(row_selector.clone().into());
            let mut row_view =
                Self::lip_row_base(row_element_id, row_selector, still.label, desktop_theme);

            if row.editable {
                let edit_key = SteeringFocusKey {
                    identity: identity.clone(),
                    action: SteeringAction::Edit,
                };
                let discard_key = SteeringFocusKey {
                    identity: identity.clone(),
                    action: SteeringAction::Discard,
                };
                let row_index = isize::try_from(index).unwrap_or(isize::MAX);
                let edit_tab_index = 20isize.saturating_add(row_index.saturating_mul(2));
                let discard_tab_index = edit_tab_index.saturating_add(1);
                let edit_focus = self.steering_focus(&edit_key, edit_tab_index, cx);
                let discard_focus = self.steering_focus(&discard_key, discard_tab_index, cx);
                let edit_selector = format!("{row_selector}-{ROW_EDIT_SELECTOR_SUFFIX}");
                let discard_selector = format!("{row_selector}-{ROW_DISCARD_SELECTOR_SUFFIX}");
                let edit_entity = entity.clone();
                let discard_entity = entity.clone();
                let edit_identity = identity.clone();
                let discard_identity = identity.clone();

                let edit = Button::new(
                    ElementId::Name(edit_selector.clone().into()),
                    edit_focus,
                    theme,
                    MotionPolicy::Reduced,
                    ButtonVariant::Ghost,
                    ButtonSize::IconSmall,
                    ButtonContent::icon_only(
                        QueuedSteerRow::edit_icon(),
                        AccessibleLabel::new(QueuedSteerRow::edit_label())
                            .expect("the edit label is nonempty"),
                    ),
                )
                .expect("the queued-steer edit button is valid")
                .focus_visibility(FocusVisibility::Visible)
                .disabled(self.snapshot.disabled)
                .debug_selector(edit_selector)
                .on_activate(move |_, _, app| {
                    edit_entity.update(app, |controls, controls_cx| {
                        controls.emit_if_allowed(
                            NativeComposerControlsEvent::EditQueuedSteer {
                                command_id: edit_identity.command_id.clone(),
                                generation: edit_identity.generation,
                            },
                            controls_cx,
                        );
                    });
                });

                let discard = Button::new(
                    ElementId::Name(discard_selector.clone().into()),
                    discard_focus,
                    theme,
                    MotionPolicy::Reduced,
                    ButtonVariant::Ghost,
                    ButtonSize::IconSmall,
                    ButtonContent::icon_only(
                        QueuedSteerRow::discard_icon(),
                        AccessibleLabel::new(QueuedSteerRow::discard_label())
                            .expect("the discard label is nonempty"),
                    ),
                )
                .expect("the queued-steer discard button is valid")
                .focus_visibility(FocusVisibility::Visible)
                .disabled(self.snapshot.disabled)
                .debug_selector(discard_selector)
                .on_activate(move |_, _, app| {
                    discard_entity.update(app, |controls, controls_cx| {
                        controls.emit_if_allowed(
                            NativeComposerControlsEvent::DiscardQueuedSteer {
                                command_id: discard_identity.command_id.clone(),
                                generation: discard_identity.generation,
                            },
                            controls_cx,
                        );
                    });
                });

                row_view = row_view.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(4.0))
                        .flex_shrink_0()
                        .child(edit)
                        .child(discard),
                );
            }

            // Reference mount motion (`utilities.css:522-531`,
            // `lip-row-grow` on `--acc-expand`): the row grows its grid
            // track so the lip height tweens. GPUI has no grid-track or
            // auto-height interpolation (see the lane report), so only the
            // opacity half is reproduced, on the exact 250ms clock. Each
            // newly arrived steer carries a fresh nonce, so its entrance
            // replays without replaying the rows already on screen. Styling
            // finishes first; the animated and plain branches converge to
            // `AnyElement` without dropping the animation.
            let nonce = self.lip_row_nonces.get(&identity).copied().unwrap_or(0);
            let row_view: AnyElement = if cx.reduce_motion() {
                row_view.into_any_element()
            } else {
                row_view
                    .opacity(0.0)
                    .with_animation(
                        ElementId::Name(
                            format!(
                                "artisan-native-composer-steering-row-entrance-{}-{}-{nonce}",
                                identity.command_id, identity.generation
                            )
                            .into(),
                        ),
                        Animation::new(Duration::from_millis(COMPOSER_LIP_MOTION_MS))
                            .with_easing(composer_smooth_out),
                        move |row, progress| row.opacity(progress.clamp(0.0, 1.0)),
                    )
                    .into_any_element()
            };

            lip = lip.child(row_view);
        }

        // A lip that just emptied keeps its last rows for the collapse fade.
        // The retained rows carry no actions: without a snapshot identity the
        // emission fence already refuses them, and no focus handle is
        // installed, so the fade-out is pointer- and keyboard-inert.
        if !self.closing_lip_rows.is_empty() {
            let mut closing_body = div().w_full().flex().flex_col();
            for row in self.closing_lip_rows.clone() {
                let still = QueuedSteerRow::new(row.generation(), &row.text, false);
                let selector = row_selector(&row.identity);
                let element_id = ElementId::Name(selector.clone().into());
                closing_body = closing_body.child(Self::lip_row_base(
                    element_id,
                    selector,
                    still.label,
                    desktop_theme,
                ));
            }
            let closing: AnyElement = if cx.reduce_motion() {
                closing_body.into_any_element()
            } else {
                let generation = self.closing_lip_generation;
                closing_body
                    .opacity(1.0)
                    .with_animation(
                        ElementId::Name(
                            format!("artisan-native-composer-steering-lip-close-{generation}")
                                .into(),
                        ),
                        Animation::new(Duration::from_millis(COMPOSER_LIP_MOTION_MS))
                            .with_easing(composer_smooth_out),
                        move |lip, progress| lip.opacity((1.0 - progress).clamp(0.0, 1.0)),
                    )
                    .into_any_element()
            };
            lip = lip.child(closing);
        }

        if let Some(status) = self.snapshot.queue_status.clone() {
            let mut row = div().flex().items_center().gap(px(8.0)).px(px(12.0)).py(px(4.0))
                .text_size(px(12.0)).text_color(theme.colors.muted_foreground.to_paint())
                .child(div().flex_1().child(status));
            if let Some(label) = self.snapshot.queue_retry.clone() {
                let entity = cx.entity();
                let retry = Button::new("artisan-composer-queue-retry", self.queue_retry_focus.clone(), theme,
                    MotionPolicy::Reduced, ButtonVariant::Ghost, ButtonSize::Small, ButtonContent::text(label))
                    .expect("queue retry label is valid").disabled(self.snapshot.disabled)
                    .on_activate(move |_, _, app| entity.update(app, |controls, cx| controls.emit_if_allowed(NativeComposerControlsEvent::RetryQueue, cx)));
                row = row.child(retry);
            }
            lip = lip.child(row);
        }
        Some(lip)
    }

    /// Alias emphasizing the lip's composer placement.
    #[must_use]
    pub fn render_steering_lip(
        &mut self,
        theme: ArtisanTheme,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        self.render_lip(theme, cx)
    }

    /// Renders the typed action-failure banner above the editor.
    #[must_use]
    pub fn render_failure(
        &mut self,
        theme: ArtisanTheme,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let failure = self.snapshot.failure.as_ref()?.clone();
        let entity = cx.entity();
        let desktop_theme = DesktopTheme::neutral_dark();
        let failure_id = failure.failure_id;

        self.failure_dismiss_focus = self.failure_dismiss_focus.clone().tab_stop(true);
        self.failure_retry_focus = self
            .failure_retry_focus
            .clone()
            .tab_stop(!self.snapshot.disabled && failure.retryable);

        let dismiss_entity = entity.clone();
        let dismiss = Button::new(
            ElementId::Name(FAILURE_DISMISS_SELECTOR.into()),
            self.failure_dismiss_focus.clone(),
            theme,
            MotionPolicy::Reduced,
            ButtonVariant::Ghost,
            ButtonSize::Small,
            ButtonContent::text("Dismiss"),
        )
        .expect("the failure dismiss button is valid")
        .focus_visibility(FocusVisibility::Visible)
        // Dismissal remains available even when the parent has disabled the
        // run-scoped controls, so an error cannot trap keyboard focus.
        .disabled(false)
        .debug_selector(FAILURE_DISMISS_SELECTOR)
        .on_activate(move |_, _, app| {
            dismiss_entity.update(app, |controls, controls_cx| {
                controls.emit_if_allowed(
                    NativeComposerControlsEvent::DismissFailure { failure_id },
                    controls_cx,
                );
            });
        });

        let mut actions = div().flex().flex_row().items_center().gap(px(4.0));
        if failure.retryable {
            let retry_entity = entity.clone();
            let retry = Button::new(
                ElementId::Name(FAILURE_RETRY_SELECTOR.into()),
                self.failure_retry_focus.clone(),
                theme,
                MotionPolicy::Reduced,
                ButtonVariant::Ghost,
                ButtonSize::Small,
                ButtonContent::text("Retry"),
            )
            .expect("the failure retry button is valid")
            .focus_visibility(FocusVisibility::Visible)
            .disabled(self.snapshot.disabled)
            .debug_selector(FAILURE_RETRY_SELECTOR)
            .on_activate(move |_, _, app| {
                retry_entity.update(app, |controls, controls_cx| {
                    controls.emit_if_allowed(
                        NativeComposerControlsEvent::RetryFailure { failure_id },
                        controls_cx,
                    );
                });
            });
            actions = actions.child(retry);
        }
        actions = actions.child(dismiss);

        Some(
            div()
                .id(ElementId::Name(NATIVE_COMPOSER_FAILURE_SELECTOR.into()))
                .debug_selector(|| NATIVE_COMPOSER_FAILURE_SELECTOR.to_owned())
                .w_full()
                .flex()
                .flex_row()
                .items_start()
                .gap(px(12.0))
                // Reference (`action-failure.svelte:31`): `rounded-xl`
                // (14px), `border-destructive/40`, `px-4 py-3`, title and
                // description both `text-sm` (14px).
                .rounded(px(14.0))
                .border_1()
                .border_color(theme.colors.destructive.with_alpha(0.4).to_paint())
                .bg(desktop_theme.field)
                .px(px(16.0))
                .py(px(12.0))
                .child(
                    div()
                        .min_w(px(0.0))
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .text_size(theme.typography.control_text)
                                .text_color(desktop_theme.foreground)
                                .child(failure.failure.title),
                        )
                        .child(
                            div()
                                .text_size(theme.typography.control_text)
                                .text_color(desktop_theme.secondary)
                                .child(failure.failure.description),
                        ),
                )
                .child(actions),
        )
    }

    /// Alias naming the banner's complete feedback role.
    #[must_use]
    pub fn render_failure_banner(
        &mut self,
        theme: ArtisanTheme,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        self.render_failure(theme, cx)
    }

    /// Renders one terminal-failure card per failed dispatch, newest first.
    ///
    /// Each card states the verbatim dispatcher reason and offers the single
    /// honest action: `Start new chat` carries the exact failed prompt into a
    /// new thread as an unsent draft. There is deliberately no retry: a
    /// terminal failure will never send on its thread.
    #[must_use]
    pub fn render_failed_dispatches(
        &mut self,
        theme: ArtisanTheme,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let rows = self.snapshot.failed_dispatches.clone();
        if rows.is_empty() {
            return None;
        }
        let entity = cx.entity();
        let desktop_theme = DesktopTheme::neutral_dark();
        let mut cards = div()
            .id(ElementId::Name(
                "artisan-native-composer-failed-dispatches".into(),
            ))
            .debug_selector(|| "artisan-native-composer-failed-dispatches".to_owned())
            .w_full()
            .flex()
            .flex_col()
            .gap(px(8.0));
        for (index, row) in rows.into_iter().enumerate() {
            let row_index = isize::try_from(index).unwrap_or(isize::MAX);
            let tab_index = 30isize.saturating_add(row_index);
            let focus = self.failed_focus(&row.identity, tab_index, cx);
            let selector =
                format!("{NATIVE_COMPOSER_FAILED_NEW_THREAD_SELECTOR}-{}", row.command_id());
            let action_entity = entity.clone();
            let command_id = row.command_id().to_owned();
            let generation = row.generation();
            let action = Button::new(
                ElementId::Name(selector.clone().into()),
                focus,
                theme,
                MotionPolicy::Reduced,
                ButtonVariant::Ghost,
                ButtonSize::Small,
                ButtonContent::text("Start new chat"),
            )
            .expect("the failed-dispatch new-chat button is valid")
            .focus_visibility(FocusVisibility::Visible)
            .disabled(self.snapshot.disabled || !self.snapshot.failed_new_chat_ready)
            .debug_selector(selector.clone())
            .on_activate(move |_, _, app| {
                action_entity.update(app, |controls, controls_cx| {
                    controls.emit_if_allowed(
                        NativeComposerControlsEvent::StartNewThreadWithFailedPrompt {
                            command_id: command_id.clone(),
                            generation,
                        },
                        controls_cx,
                    );
                });
            });
            let mut body = div()
                .min_w(px(0.0))
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(theme.typography.control_text)
                        .text_color(desktop_theme.foreground)
                        .child("Send failed"),
                )
                .child(
                    div()
                        .text_size(theme.typography.control_text)
                        .text_color(desktop_theme.secondary)
                        .child(row.reason.clone()),
                );
            if !row.text.is_empty() {
                body = body.child(
                    div()
                        .text_size(theme.typography.control_text)
                        .text_color(desktop_theme.secondary)
                        .child(row.text.clone()),
                );
            }
            if row.has_attachments {
                body = body.child(
                    div()
                        .text_size(theme.typography.label_text)
                        .text_color(desktop_theme.secondary)
                        .child("Images move with the prompt."),
                );
            }
            if !self.snapshot.failed_new_chat_ready {
                body = body.child(
                    div()
                        .text_size(theme.typography.label_text)
                        .text_color(desktop_theme.secondary)
                        .child("Start a new chat once the composer is empty and idle."),
                );
            }
            cards = cards.child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(px(12.0))
                    .rounded(px(14.0))
                    .border_1()
                    .border_color(theme.colors.destructive.with_alpha(0.4).to_paint())
                    .bg(desktop_theme.field)
                    .px(px(16.0))
                    .py(px(12.0))
                    .child(body)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(4.0))
                            .child(action),
                    ),
            );
        }
        Some(cards)
    }

    /// Renders the owner-gated transcript escape action above the feedback
    /// surfaces. It has no element at all when the transcript owner says the
    /// latest-content affordance is unnecessary.
    #[must_use]
    pub fn render_jump_to_latest(
        &mut self,
        theme: ArtisanTheme,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        if !self.snapshot.show_jump_to_latest {
            self.jump_focus = self.jump_focus.clone().tab_stop(false);
            return None;
        }

        self.jump_focus = self.jump_focus.clone().tab_stop(!self.snapshot.disabled);
        let jump_entity = cx.entity();
        let jump = Button::new(
            ElementId::Name(NATIVE_COMPOSER_JUMP_TO_LATEST_SELECTOR.into()),
            self.jump_focus.clone(),
            theme,
            MotionPolicy::Reduced,
            ButtonVariant::Ghost,
            ButtonSize::IconSmall,
            ButtonContent::icon_only(
                AssetId::TABLER_CHEVRON_DOWN,
                AccessibleLabel::new(JUMP_TO_LATEST_LABEL)
                    .expect("the jump-to-latest label is nonempty"),
            ),
        )
        .expect("the jump-to-latest button is valid")
        .focus_visibility(FocusVisibility::Visible)
        .disabled(self.snapshot.disabled)
        .debug_selector(NATIVE_COMPOSER_JUMP_TO_LATEST_SELECTOR)
        .on_activate(move |_, _, app| {
            jump_entity.update(app, |controls, controls_cx| {
                controls.emit_if_allowed(NativeComposerControlsEvent::JumpToLatest, controls_cx);
            });
        });

        Some(
            div()
                .id(ElementId::Name("artisan-native-composer-jump-row".into()))
                .w_full()
                .flex()
                .justify_center()
                .child(jump),
        )
    }

    /// Renders the bottom row, accepting a parent-supplied model picker slot.
    #[must_use]
    pub fn render_control_row<E: IntoElement>(
        &mut self,
        theme: ArtisanTheme,
        model_picker: E,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let entity = cx.entity();
        let primary_event = self.primary_event();
        let new_thread_event = self.new_thread_event();
        let desktop_theme = DesktopTheme::neutral_dark();

        self.primary_focus = self.primary_focus.clone().tab_stop(primary_event.is_some());
        self.new_thread_focus = self
            .new_thread_focus
            .clone()
            .tab_stop(new_thread_event.is_some());

        let mut left = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(2.0))
            .min_w(px(0.0))
            .child(model_picker);

        if let Some(context_usage) = self.snapshot.context_usage.as_ref() {
            let context_valid = context_usage
                .presentation(self.snapshot.run_id.as_deref())
                .is_some();
            self.context_focus = self
                .context_focus
                .clone()
                .tab_stop(context_valid && !self.snapshot.disabled);
            if context_valid {
                let context_entity = entity.clone();
                let context_run_id = context_usage.reporting_run_id.clone();
                if let Some(popover) = context_usage.render_popover(
                    self.snapshot.run_id.as_deref(),
                    theme,
                    self.context_focus.clone(),
                    self.snapshot.context_usage_open,
                    self.snapshot.disabled,
                    move |open, _reason, _window, app| {
                        context_entity.update(app, |controls, controls_cx| {
                            controls.emit_if_allowed(
                                NativeComposerControlsEvent::ContextUsageToggled {
                                    run_id: context_run_id.clone(),
                                    open,
                                },
                                controls_cx,
                            );
                        });
                    },
                ) {
                    left = left.child(popover);
                }
            }
        } else {
            self.context_focus = self.context_focus.clone().tab_stop(false);
        }

        // Reference (`controls.svelte:116-143`): the right cluster holds
        // only the escape action and the send button; a 4px separation
        // (`mr-1` on the escape action) with zero gap otherwise.
        let mut right = div().flex().flex_row().items_center().gap(px(4.0));

        if let Some(event) = new_thread_event {
            let new_thread_entity = entity.clone();
            let focus = self.new_thread_focus.clone();
            let new_thread = Button::new(
                ElementId::Name(NATIVE_COMPOSER_NEW_THREAD_SELECTOR.into()),
                focus,
                theme,
                MotionPolicy::Reduced,
                ButtonVariant::Ghost,
                ButtonSize::Small,
                ButtonContent::icon_text(
                    AssetId::TABLER_MESSAGE_PLUS,
                    START_NEW_THREAD_PROMPT_LABEL,
                ),
            )
            .expect("the new-thread button is valid")
            .focus_visibility(FocusVisibility::Visible)
            .corner_radius(px(10.0))
            .disabled(self.snapshot.disabled)
            .debug_selector(NATIVE_COMPOSER_NEW_THREAD_SELECTOR)
            .on_activate(move |_, _, app| {
                new_thread_entity.update(app, |controls, controls_cx| {
                    controls.emit_if_allowed(event.clone(), controls_cx);
                });
            });
            right = right.child(new_thread);
        }

        let (icon, label) = (
            SendButtonStill::control_icon(self.snapshot.run_active),
            SendButtonStill::control_label(self.snapshot.run_active),
        );
        let primary_disabled = primary_event.is_none();
        let mut primary = Button::new(
            ElementId::Name(NATIVE_COMPOSER_PRIMARY_SELECTOR.into()),
            self.primary_focus.clone(),
            theme,
            MotionPolicy::Reduced,
            ButtonVariant::Ghost,
            ButtonSize::IconSmall,
            ButtonContent::icon_only(
                icon,
                AccessibleLabel::new(label).expect("the primary action label is nonempty"),
            ),
        )
        .expect("the primary composer control is valid")
        .focus_visibility(FocusVisibility::Visible)
        .corner_radius(px(10.0))
        .disabled(primary_disabled)
        .debug_selector(NATIVE_COMPOSER_PRIMARY_SELECTOR);
        if let Some(event) = primary_event {
            let primary_entity = entity.clone();
            primary = primary.on_activate(move |_, _, app| {
                primary_entity.update(app, |controls, controls_cx| {
                    controls.emit_if_allowed(event.clone(), controls_cx);
                });
            });
        }
        right = right.child(primary);

        div()
            .id(ElementId::Name(NATIVE_COMPOSER_CONTROL_ROW_SELECTOR.into()))
            .debug_selector(|| NATIVE_COMPOSER_CONTROL_ROW_SELECTOR.to_owned())
            .w_full()
            .h(px(32.0))
            .flex_shrink_0()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .text_color(desktop_theme.secondary)
            .child(left)
            .child(right)
    }

    /// Alias used by parents that call the row the control strip.
    #[must_use]
    pub fn render_bottom_row<E: IntoElement>(
        &mut self,
        theme: ArtisanTheme,
        model_picker: E,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        self.render_control_row(theme, model_picker, cx)
    }

    /// Renders the whole focused component with an empty picker slot.
    ///
    /// Native composer integration normally calls the focused methods above so
    /// the actual model selector can occupy the left slot. This implementation
    /// exists for standalone component-gallery and behavior tests.
    fn render_standalone(&mut self, theme: ArtisanTheme, cx: &mut Context<Self>) -> Stateful<Div> {
        let mut root = div()
            .id(ElementId::Name(NATIVE_COMPOSER_CONTROLS_SELECTOR.into()))
            .debug_selector(|| NATIVE_COMPOSER_CONTROLS_SELECTOR.to_owned())
            .w_full()
            .flex()
            .flex_col()
            .gap(px(8.0));
        if let Some(lip) = self.render_lip(theme, cx) {
            root = root.child(lip);
        }
        if let Some(jump) = self.render_jump_to_latest(theme, cx) {
            root = root.child(jump);
        }
        if let Some(failure) = self.render_failure(theme, cx) {
            root = root.child(failure);
        }
        if let Some(failed) = self.render_failed_dispatches(theme, cx) {
            root = root.child(failed);
        }
        root.child(self.render_control_row(theme, div(), cx))
    }

    fn steering_focus(
        &mut self,
        key: &SteeringFocusKey,
        tab_index: isize,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        if let Some(focus) = self.steering_focus.get(key) {
            let focus = focus
                .clone()
                .tab_index(tab_index)
                .tab_stop(!self.snapshot.disabled);
            self.steering_focus.insert(key.clone(), focus.clone());
            return focus;
        }
        let focus = cx
            .focus_handle()
            .tab_index(tab_index)
            .tab_stop(!self.snapshot.disabled);
        self.steering_focus.insert(key.clone(), focus.clone());
        focus
    }

    fn failed_focus(
        &mut self,
        identity: &QueuedSteeringIdentity,
        tab_index: isize,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        if let Some(focus) = self.failed_focus.get(identity) {
            let focus = focus
                .clone()
                .tab_index(tab_index)
                .tab_stop(!self.snapshot.disabled);
            self.failed_focus
                .insert(identity.clone(), focus.clone());
            return focus;
        }
        let focus = cx
            .focus_handle()
            .tab_index(tab_index)
            .tab_stop(!self.snapshot.disabled);
        self.failed_focus
            .insert(identity.clone(), focus.clone());
        focus
    }

    fn emit_if_allowed(&self, event: NativeComposerControlsEvent, cx: &mut Context<Self>) {
        if native_composer_controls_event_is_allowed(&self.snapshot, &event) {
            cx.emit(event);
        }
    }
}

/// Purely checks whether an intent still matches the current parent snapshot.
///
/// The renderer calls this fence immediately before emitting an activation.
/// A delayed click therefore cannot stop a newer run, edit a recycled queued
/// row, retry a replaced failure, or open details for stale telemetry. The
/// function is public so parent tests can exercise the same identity policy
/// without constructing a GPUI window.
#[must_use]
pub fn native_composer_controls_event_is_allowed(
    snapshot: &NativeComposerControlsSnapshot,
    event: &NativeComposerControlsEvent,
) -> bool {
    if snapshot.disabled {
        return matches!(event, NativeComposerControlsEvent::DismissFailure { .. })
            && failure_matches(snapshot, event);
    }

    match event {
        NativeComposerControlsEvent::RetryQueue => snapshot.queue_retry.is_some(),
        NativeComposerControlsEvent::SendRequested => !snapshot.run_active && snapshot.send_ready,
        NativeComposerControlsEvent::StopRequested { run_id } => {
            snapshot.run_active
                && !snapshot.cancelling
                && snapshot.abort_available
                && snapshot.run_id.as_deref() == Some(run_id)
        }
        NativeComposerControlsEvent::StartNewThreadWithPrompt { run_id } => {
            snapshot.run_active
                && snapshot.new_thread_ready
                && snapshot.run_id.as_deref() == Some(run_id)
        }
        NativeComposerControlsEvent::StartNewThreadWithFailedPrompt {
            command_id,
            generation,
        } => {
            snapshot.failed_new_chat_ready
                && snapshot.failed_dispatches.iter().any(|row| {
                    row.identity.command_id == *command_id
                        && row.identity.generation == *generation
                })
        }
        NativeComposerControlsEvent::JumpToLatest => snapshot.show_jump_to_latest,
        NativeComposerControlsEvent::EditQueuedSteer {
            command_id,
            generation,
        }
        | NativeComposerControlsEvent::DiscardQueuedSteer {
            command_id,
            generation,
        } => snapshot.pending_steering.iter().any(|row| {
            row.editable
                && row.identity.command_id == *command_id
                && row.identity.generation == *generation
        }),
        NativeComposerControlsEvent::DismissFailure { .. }
        | NativeComposerControlsEvent::RetryFailure { .. } => failure_matches(snapshot, event),
        NativeComposerControlsEvent::ContextUsageToggled { run_id, .. } => {
            snapshot.context_usage.as_ref().is_some_and(|usage| {
                usage.reporting_run_id == *run_id
                    && usage.presentation(snapshot.run_id.as_deref()).is_some()
            })
        }
    }
}

fn failure_matches(
    snapshot: &NativeComposerControlsSnapshot,
    event: &NativeComposerControlsEvent,
) -> bool {
    let Some(failure) = snapshot.failure.as_ref() else {
        return false;
    };
    match event {
        NativeComposerControlsEvent::DismissFailure { failure_id }
        | NativeComposerControlsEvent::RetryFailure { failure_id } => {
            failure.failure_id == *failure_id
                && (!matches!(event, NativeComposerControlsEvent::RetryFailure { .. })
                    || failure.retryable)
        }
        _ => false,
    }
}

impl Render for NativeComposerControls {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_standalone(ArtisanTheme::for_mode(ThemeMode::Dark), cx)
    }
}

fn row_selector(identity: &QueuedSteeringIdentity) -> String {
    format!(
        "{ROW_SELECTOR_PREFIX}-{}-{}",
        identity.command_id, identity.generation
    )
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use super::{
        NATIVE_COMPOSER_FAILED_NEW_THREAD_SELECTOR, NATIVE_COMPOSER_PRIMARY_SELECTOR,
        FailedDispatchRow, NativeComposerControls, NativeComposerControlsEvent,
        NativeComposerControlsSnapshot, PendingSteeringRow,
        native_composer_controls_event_is_allowed,
    };
    use gpui::{
        Entity, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, Subscription, TestAppContext,
        VisualTestContext,
    };

    fn observe_events(
        cx: &mut VisualTestContext,
        view: &Entity<NativeComposerControls>,
    ) -> (Rc<RefCell<Vec<NativeComposerControlsEvent>>>, Subscription) {
        let events = Rc::new(RefCell::new(Vec::new()));
        let observed = events.clone();
        let subscription = cx.update(|_, app| {
            app.subscribe(view, move |_, event: &NativeComposerControlsEvent, _| {
                observed.borrow_mut().push(event.clone());
            })
        });
        cx.run_until_parked();
        (events, subscription)
    }

    fn snapshot_idle() -> NativeComposerControlsSnapshot {
        NativeComposerControlsSnapshot {
            run_id: Some("thread-run".to_owned()),
            send_ready: true,
            ..NativeComposerControlsSnapshot::default()
        }
    }

    #[gpui::test]
    fn pointer_and_keyboard_share_the_typed_primary_intent(cx: &mut TestAppContext) {
        let (view, cx) =
            cx.add_window_view(|_, cx| NativeComposerControls::new(snapshot_idle(), cx));
        let (events, _subscription) = observe_events(cx, &view);
        let bounds = cx
            .debug_bounds(NATIVE_COMPOSER_PRIMARY_SELECTOR)
            .expect("the primary control paints");

        cx.simulate_click(bounds.center(), Modifiers::none());
        for key in ["enter", "space"] {
            cx.simulate_event(KeyDownEvent {
                keystroke: Keystroke::parse(key).expect("known activation key"),
                is_held: false,
                prefer_character_input: false,
            });
            cx.simulate_event(KeyUpEvent {
                keystroke: Keystroke::parse(key).expect("known activation key"),
            });
        }

        assert_eq!(
            events.borrow().as_slice(),
            [
                NativeComposerControlsEvent::SendRequested,
                NativeComposerControlsEvent::SendRequested,
                NativeComposerControlsEvent::SendRequested,
            ]
        );
    }

    #[gpui::test]
    fn cancelling_stop_is_disabled_for_pointer_and_keyboard(cx: &mut TestAppContext) {
        let snapshot = NativeComposerControlsSnapshot {
            run_id: Some("run-1".to_owned()),
            run_active: true,
            cancelling: false,
            abort_available: true,
            ..NativeComposerControlsSnapshot::default()
        };
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposerControls::new(snapshot, cx));
        let (events, _subscription) = observe_events(cx, &view);
        let bounds = cx
            .debug_bounds(NATIVE_COMPOSER_PRIMARY_SELECTOR)
            .expect("the stop control paints");
        cx.simulate_click(bounds.center(), Modifiers::none());
        assert_eq!(
            events.borrow().as_slice(),
            [NativeComposerControlsEvent::StopRequested {
                run_id: "run-1".to_owned()
            }]
        );

        cx.update(|_, app| {
            view.update(app, |controls, controls_cx| {
                let mut next = controls.snapshot().clone();
                next.cancelling = true;
                controls.set_snapshot(next, controls_cx);
            });
        });
        cx.run_until_parked();
        for key in ["enter", "space"] {
            cx.simulate_event(KeyDownEvent {
                keystroke: Keystroke::parse(key).expect("known activation key"),
                is_held: false,
                prefer_character_input: false,
            });
            cx.simulate_event(KeyUpEvent {
                keystroke: Keystroke::parse(key).expect("known activation key"),
            });
        }
        assert_eq!(events.borrow().len(), 1);
    }

    #[test]
    fn queued_rows_keep_exact_identity_until_parent_projection_withdraws_them() {
        let mut snapshot = NativeComposerControlsSnapshot::default();
        let row = PendingSteeringRow::new("command-7", 3, "keep this", true);
        let identity = row.identity.clone();
        snapshot.pending_steering.push(row);
        assert!(snapshot.contains_steering(&identity));

        snapshot.pending_steering.clear();
        assert!(!snapshot.contains_steering(&identity));
    }

    #[test]
    fn action_identity_is_fenced_against_stale_generation() {
        let mut controls_snapshot = snapshot_idle();
        controls_snapshot
            .pending_steering
            .push(PendingSteeringRow::new("command-1", 4, "queued", true));
        let stale = NativeComposerControlsEvent::DiscardQueuedSteer {
            command_id: "command-1".to_owned(),
            generation: 3,
        };
        let current = NativeComposerControlsEvent::DiscardQueuedSteer {
            command_id: "command-1".to_owned(),
            generation: 4,
        };
        assert_eq!(controls_snapshot.pending_steering[0].generation(), 4);
        assert_ne!(stale, current);
        assert!(!native_composer_controls_event_is_allowed(
            &controls_snapshot,
            &stale
        ));
        assert!(native_composer_controls_event_is_allowed(
            &controls_snapshot,
            &current
        ));
    }

    #[test]
    fn run_scoped_intents_cannot_cross_run_and_cancelling_blocks_only_stop() {
        let snapshot = NativeComposerControlsSnapshot {
            run_id: Some("run-2".to_owned()),
            run_active: true,
            abort_available: true,
            new_thread_ready: true,
            ..NativeComposerControlsSnapshot::default()
        };
        assert!(!native_composer_controls_event_is_allowed(
            &snapshot,
            &NativeComposerControlsEvent::StopRequested {
                run_id: "run-1".to_owned()
            }
        ));
        assert!(native_composer_controls_event_is_allowed(
            &snapshot,
            &NativeComposerControlsEvent::StartNewThreadWithPrompt {
                run_id: "run-2".to_owned()
            }
        ));

        let mut cancelling = snapshot;
        cancelling.cancelling = true;
        assert!(!native_composer_controls_event_is_allowed(
            &cancelling,
            &NativeComposerControlsEvent::StopRequested {
                run_id: "run-2".to_owned()
            }
        ));
        assert!(native_composer_controls_event_is_allowed(
            &cancelling,
            &NativeComposerControlsEvent::StartNewThreadWithPrompt {
                run_id: "run-2".to_owned()
            }
        ));
    }

    fn failed_snapshot(ready: bool) -> NativeComposerControlsSnapshot {
        let mut snapshot = NativeComposerControlsSnapshot::default();
        snapshot.failed_dispatches.push(FailedDispatchRow::new(
            "command-9",
            4,
            "hello",
            false,
            "provider continuation unavailable: the prior run was interrupted with unknown outcome; start a new chat to continue",
        ));
        snapshot.failed_new_chat_ready = ready;
        snapshot
    }

    fn failed_event() -> NativeComposerControlsEvent {
        NativeComposerControlsEvent::StartNewThreadWithFailedPrompt {
            command_id: "command-9".to_owned(),
            generation: 4,
        }
    }

    #[test]
    fn failed_new_chat_fences_exact_identity_and_generation() {
        let snapshot = failed_snapshot(true);
        assert!(native_composer_controls_event_is_allowed(
            &snapshot,
            &failed_event()
        ));
        assert!(!native_composer_controls_event_is_allowed(
            &snapshot,
            &NativeComposerControlsEvent::StartNewThreadWithFailedPrompt {
                command_id: "command-9".to_owned(),
                generation: 5,
            }
        ));
        assert!(!native_composer_controls_event_is_allowed(
            &snapshot,
            &NativeComposerControlsEvent::StartNewThreadWithFailedPrompt {
                command_id: "command-other".to_owned(),
                generation: 4,
            }
        ));
        assert!(!native_composer_controls_event_is_allowed(
            &failed_snapshot(false),
            &failed_event()
        ));
    }

    #[gpui::test]
    fn failed_new_chat_click_emits_exact_failed_identity(cx: &mut TestAppContext) {
        let (view, cx) =
            cx.add_window_view(|_, cx| NativeComposerControls::new(failed_snapshot(true), cx));
        let (events, _subscription) = observe_events(cx, &view);
        let selector = "artisan-native-composer-failed-new-thread-command-9";
        let bounds = cx
            .debug_bounds(selector)
            .expect("the failed new-chat control paints");
        cx.simulate_click(bounds.center(), Modifiers::none());
        assert_eq!(events.borrow().as_slice(), [failed_event()]);
    }

    #[gpui::test]
    fn failed_new_chat_click_stays_silent_without_ready_snapshot(cx: &mut TestAppContext) {
        let (view, cx) =
            cx.add_window_view(|_, cx| NativeComposerControls::new(failed_snapshot(false), cx));
        let (events, _subscription) = observe_events(cx, &view);
        let selector = "artisan-native-composer-failed-new-thread-command-9";
        let bounds = cx
            .debug_bounds(selector)
            .expect("the failed new-chat control paints");
        cx.simulate_click(bounds.center(), Modifiers::none());
        assert!(events.borrow().is_empty());
    }
}
