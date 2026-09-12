//! Editable GPUI composer for the native first-message workflow.
//!
//! [`ComposerState`] remains the only draft and submission authority. This
//! entity owns the toolkit state needed to bridge GPUI 0.2.2 text services:
//! focus, UTF-8 selection, marked text, and the current text layout. It never
//! normalizes, trims, or otherwise rewrites authored text.

#![forbid(unsafe_code)]

use std::{collections::HashSet, ops::Range, panic, sync::Arc};

use artisan_assets::AssetId;
use artisan_ui::{
    asset_seam::asset_glyph,
    button::{AccessibleLabel, Button, ButtonContent, ButtonSize, ButtonVariant, FocusVisibility},
    motion::MotionPolicy,
    theme::{ArtisanTheme, DesktopTheme, ProseTypography, ThemeMode},
};
use gpui::ColorExt;
use gpui::StyledImage;
use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Bounds, ClipboardItem, Context, DispatchPhase,
    Div, Element, ElementId, ElementInputHandler, Entity, EventEmitter, ExternalPaths, FocusHandle,
    Focusable, GlobalElementId, HighlightStyle, ImageFormat, ImageSource, InspectorElementId,
    IntoElement, KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ObjectFit, Pixels, Point, Render, RenderImage, SharedString, Stateful, StyledText,
    Subscription, Task, UTF16Selection, Window, actions, div, img, point,
    prelude::{
        InteractiveElement as _, ParentElement as _, StatefulInteractiveElement as _, Styled as _,
    },
    px, size,
};
use std::time::Duration;

use crate::composer::{ComposerState, DraftDisposition, SubmissionBlocked, SubmissionToken};
use crate::composer_draft_session_policy::{
    ComposerDraftDocument, ComposerDraftSession, ComposerDraftToken, InMemoryComposerDraftStore,
};
use crate::native_composer_controls::NativeComposerControls;
use crate::native_composer_material::{
    GlassStrength, card_shadows, glass_blur_radius, glass_card_shadows, glass_foreground_base,
    glass_highlight_layer, glass_material_layer,
};
use crate::native_composer_visuals::{
    COMPOSER_TRAY_MOTION_MS, composer_placeholder_phrase, composer_smooth_out,
};
use crate::native_model_selector::NativeModelSelector;

#[path = "native_composer_attachments.rs"]
mod native_composer_attachments;
use self::attachments::{AttachmentPreviewState, AttachmentWork, PasteScope};
use self::native_composer_attachments::{
    AttachmentPayloadError, AttachmentPreparationError, AttachmentPreparationOutcome,
    ClipboardImageCandidate, ClipboardInput, ComposerAttachment, MAXIMUM_ATTACHMENT_COUNT,
    MAXIMUM_ATTACHMENT_TOTAL_BYTES, MAXIMUM_RAW_ATTACHMENT_BYTES,
    MAXIMUM_RAW_ATTACHMENT_TOTAL_BYTES, NativeComposerAttachmentSnapshot,
    PreparedComposerAttachment, RecalledAttachmentInput, RestoredAttachmentInput,
    display_file_name, prepare_clipboard_batch, prepare_file_batch, prepare_recalled_batch,
    prepare_restored_batch, render_full_preview,
};

actions!(
    native_composer,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Paste,
        Copy,
        Cut,
        Home,
        End,
        Up,
        Down,
        SelectHome,
        SelectEnd,
        SelectUp,
        SelectDown,
        DocumentHome,
        DocumentEnd,
        SelectDocumentHome,
        SelectDocumentEnd,
        RequestSend,
        InsertNewline,
        Undo,
        Redo,
    ]
);

const NATIVE_COMPOSER_KEY_CONTEXT: &str = "artisan-native-composer";
const NATIVE_COMPOSER_PLACEHOLDER_SELECTOR: &str = "artisan-native-composer-placeholder";
pub(crate) const NATIVE_COMPOSER_EDITOR_SELECTOR: &str = "artisan-native-composer-editor";
const NATIVE_COMPOSER_SEND_SELECTOR: &str = "artisan-native-composer-send";
pub(crate) const NATIVE_COMPOSER_ATTACHMENT_TRAY_SELECTOR: &str =
    "artisan-native-composer-attachment-tray";
pub(crate) const NATIVE_COMPOSER_ATTACHMENT_VIEWER_SELECTOR: &str =
    "artisan-native-composer-attachment-viewer";
pub(crate) const NATIVE_COMPOSER_ATTACHMENT_BLOCKED_SELECTOR: &str =
    "artisan-native-composer-attachment-send-blocked";
const NATIVE_COMPOSER_ATTACHMENT_SIZE: f32 = 72.0;

/// One bounded application event emitted by the send control.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NativeComposerEvent {
    /// The user invoked the send control; the application owns admission.
    SendRequested,
    ConfigureModel,
}

/// Exact identity of an empty composer that is eligible to receive one
/// withdrawn queued payload.
///
/// Fields remain private intentionally: the owner may pass this value back to
/// [`NativeComposer::restore_recalled_payload`], but cannot manufacture or
/// partially compare a recall target outside this component.
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct ComposerRecallTarget {
    thread: String,
    draft_generation: u64,
    draft_revision: u64,
    selection_revision: u64,
    selection: Range<usize>,
    selection_reversed: bool,
}

/// Native GPUI owner of the exact composer draft and its text-service state.
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent composer admission bits (blocked, authored-text presence, delivery, drag) are read separately across the submit and render paths"
)]
pub(crate) struct NativeComposer {
    state: ComposerState,
    focus_handle: FocusHandle,
    send_focus_handle: FocusHandle,
    model_focus_handle: FocusHandle,
    controls: Option<Entity<NativeComposerControls>>,
    model_selector: Option<Entity<NativeModelSelector>>,
    controls_observation: Option<Subscription>,
    model_selector_observation: Option<Subscription>,
    model_label: String,
    send_blocked: bool,
    /// Whether the authored text field is present in the typed payload.
    ///
    /// Ordinary editing uses `Some(text)`, including an authored empty string
    /// alongside images. Recall additionally preserves an image-only payload
    /// whose wire text was absent (`None`), since the editor's visible draft
    /// string alone cannot represent that distinction.
    authored_text_present: bool,
    attachment_delivery_enabled: bool,
    attachments: Vec<ComposerAttachment>,
    viewed_attachment: Option<String>,
    attachment_preview: Option<AttachmentPreviewState>,
    attachment_preview_error: Option<String>,
    attachment_preview_task: Option<Task<()>>,
    attachment_preview_request: u64,
    attachment_error: Option<String>,
    draft_generation: u64,
    draft_revision: u64,
    selection_revision: u64,
    draft_tokens: Vec<ComposerDraftToken>,
    active_attachment_submission: Option<NativeComposerAttachmentSnapshot>,
    active_submission_draft_revision: Option<u64>,
    attachment_tasks: Vec<Task<()>>,
    next_attachment_id: u64,
    draft_store: InMemoryComposerDraftStore,
    draft_thread: Option<String>,
    undo: Vec<(String, Range<usize>)>,
    redo: Vec<(String, Range<usize>)>,
    selection: Range<usize>,
    selection_reversed: bool,
    selection_anchor: Option<usize>,
    vertical_goal_x: Option<Pixels>,
    vertical_goal_column: Option<usize>,
    selection_dragging: bool,
    marked_range: Option<Range<usize>>,
    layout: Option<gpui::TextLayout>,
    painted_bounds: Option<Bounds<Pixels>>,
    /// Presentation-only placeholder reveal state.
    ///
    /// This mirrors `composer-placeholder.ts:21-25` without touching draft
    /// content: `placeholder_generation` walks the reference vocabulary and
    /// `placeholder_was_visible` detects each fresh reveal in render.
    placeholder_generation: u64,
    placeholder_was_visible: bool,
    /// Presentation-only tray entrance state. The generation gives each
    /// hidden-to-shown mount a fresh animation identity; close unmounts
    /// immediately (see the tray motion note in `render`).
    tray_entrance_generation: u64,
    tray_was_open: bool,
}

impl EventEmitter<NativeComposerEvent> for NativeComposer {}

impl Focusable for NativeComposer {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl NativeComposer {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        Self {
            state: ComposerState::new(),
            focus_handle: cx.focus_handle().tab_index(0).tab_stop(true),
            send_focus_handle: cx.focus_handle().tab_index(2).tab_stop(true),
            model_focus_handle: cx.focus_handle().tab_index(1).tab_stop(true),
            controls: None,
            model_selector: None,
            controls_observation: None,
            model_selector_observation: None,
            model_label: "Select model".into(),
            send_blocked: false,
            authored_text_present: true,
            attachment_delivery_enabled: false,
            attachments: Vec::new(),
            viewed_attachment: None,
            attachment_preview: None,
            attachment_preview_error: None,
            attachment_preview_task: None,
            attachment_preview_request: 0,
            attachment_error: None,
            draft_generation: 0,
            draft_revision: 0,
            selection_revision: 0,
            draft_tokens: Vec::new(),
            active_attachment_submission: None,
            active_submission_draft_revision: None,
            attachment_tasks: Vec::new(),
            next_attachment_id: 0,
            draft_store: InMemoryComposerDraftStore::new(),
            draft_thread: None,
            undo: Vec::new(),
            redo: Vec::new(),
            selection: 0..0,
            selection_reversed: false,
            selection_anchor: None,
            vertical_goal_x: None,
            vertical_goal_column: None,
            selection_dragging: false,
            marked_range: None,
            layout: None,
            painted_bounds: None,
            placeholder_generation: 0,
            placeholder_was_visible: true,
            tray_entrance_generation: 0,
            tray_was_open: false,
        }
    }

    /// Attaches the parent-owned controls and model selector entities.
    ///
    /// Until this setter is called with both entities, the composer keeps its
    /// legacy model/send row so isolated composer tests and constructors stay
    /// self-contained. The composer observes both children only to invalidate
    /// its own layout; event routing and snapshot updates remain parent-owned.
    pub(crate) fn set_components(
        &mut self,
        controls: &Entity<NativeComposerControls>,
        model_selector: &Entity<NativeModelSelector>,
        cx: &mut Context<Self>,
    ) {
        self.controls = Some(controls.clone());
        self.model_selector = Some(model_selector.clone());
        self.controls_observation = Some(cx.observe(controls, |_, _, cx| {
            Self::invalidate_component_render(cx);
        }));
        self.model_selector_observation = Some(cx.observe(model_selector, |_, _, cx| {
            Self::invalidate_component_render(cx);
        }));
        cx.notify();
    }

    fn invalidate_component_render(cx: &mut Context<Self>) {
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn draft(&self) -> &str {
        self.state.draft()
    }

    #[cfg(test)]
    /// Returns whether the authored draft is byte-identical to `body`.
    ///
    /// This read-only seam keeps draft comparison out of the submission
    /// lifecycle: it does not expose text, allocate, mint a token, mutate
    /// editor state, or notify observers.
    pub(crate) fn draft_matches_body(&self, body: &artisan_domain::MessageBody) -> bool {
        self.state.draft().as_bytes() == body.as_str().as_bytes()
    }

    #[cfg(test)]
    pub(crate) fn set_draft(&mut self, draft: impl Into<String>) {
        let draft = draft.into();
        if self.state.draft() != draft {
            self.draft_revision = self.draft_revision.saturating_add(1);
        }
        self.state.set_draft(draft);
        self.authored_text_present = true;
        self.layout = None;
        self.painted_bounds = None;
        let end = self.state.draft().len();
        self.selection = end..end;
        self.selection_reversed = false;
        self.selection_anchor = None;
        self.clear_vertical_goal();
        self.selection_dragging = false;
        self.marked_range = None;
        self.selection_revision = self.selection_revision.saturating_add(1);
    }

    fn clear_vertical_goal(&mut self) {
        self.vertical_goal_x = None;
        self.vertical_goal_column = None;
    }

    fn prune_attachment_tasks(&mut self) {
        self.attachment_tasks.retain(|task| !task.is_ready());
    }

    fn advance_selection_revision(&mut self) {
        self.selection_revision = self.selection_revision.saturating_add(1);
    }

    fn paste_scope_is_current(&self, scope: &PasteScope) -> bool {
        self.draft_thread == scope.thread
            && self.draft_generation == scope.draft_generation
            && self.selection_revision == scope.selection_revision
    }

    /// Captures the exact empty-composer scope that may receive one queued
    /// message after the owning UI withdraws it.
    ///
    /// The target is deliberately unavailable while the surface is disabled,
    /// composing, submitting, or already holding authored input. A caller may
    /// keep the queued payload and retry the restore only with the returned
    /// target; a later edit or route transition invalidates it.
    pub(crate) fn capture_recall_target(&self) -> Option<ComposerRecallTarget> {
        let thread = self
            .draft_thread
            .as_ref()
            .filter(|thread| !thread.is_empty())?
            .clone();
        if self.send_blocked
            || self.state.is_disabled()
            || self.state.is_submitting()
            || !self.state.draft().is_empty()
            || !self.attachments.is_empty()
            || self.marked_range.is_some()
        {
            return None;
        }

        Some(ComposerRecallTarget {
            thread,
            draft_generation: self.draft_generation,
            draft_revision: self.draft_revision,
            selection_revision: self.selection_revision,
            selection: self.selection.clone(),
            selection_reversed: self.selection_reversed,
        })
    }

    fn recall_target_is_current(&self, target: &ComposerRecallTarget) -> bool {
        self.draft_thread.as_deref() == Some(target.thread.as_str())
            && self.draft_generation == target.draft_generation
            && self.draft_revision == target.draft_revision
            && self.selection_revision == target.selection_revision
            && self.selection == target.selection
            && self.selection_reversed == target.selection_reversed
            && self.state.draft().is_empty()
            && self.attachments.is_empty()
            && self.marked_range.is_none()
            && !self.state.is_submitting()
            && !self.state.is_disabled()
            && !self.send_blocked
    }

    /// Restores one withdrawn, already validated queue payload into an empty
    /// composer.
    ///
    /// A failed precondition returns the same owned payload so the caller can
    /// keep recovery available after a typing, thread, or lifecycle race. On
    /// success the text/`None` distinction is retained for the next typed
    /// submission, while image decoding and thumbnail work stays bounded and
    /// off the UI thread.
    pub(crate) fn restore_recalled_payload(
        &mut self,
        target: &ComposerRecallTarget,
        payload: artisan_domain::QueueMessagePayload,
        cx: &mut Context<Self>,
    ) -> Result<(), artisan_domain::QueueMessagePayload> {
        if !self.recall_target_is_current(target)
            || payload.attachments().len() > MAXIMUM_ATTACHMENT_COUNT
            || payload.total_attachment_bytes() > MAXIMUM_ATTACHMENT_TOTAL_BYTES
            || payload.attachments().iter().any(|attachment| {
                attachment.byte_len() > native_composer_attachments::MAXIMUM_ATTACHMENT_BYTES
            })
        {
            return Err(payload);
        }

        let text_present = payload.text().is_some();
        let text = payload
            .text()
            .map_or_else(String::new, |text| text.as_str().to_owned());
        let formats = payload
            .attachments()
            .iter()
            .map(|attachment| ImageFormat::from_mime_type(attachment.mime_type_str()))
            .collect::<Option<Vec<_>>>();
        let Some(formats) = formats else {
            return Err(payload);
        };
        let mut reserved_ids = HashSet::with_capacity(payload.attachments().len());
        let mut ids = Vec::with_capacity(payload.attachments().len());
        for _ in 0..payload.attachments().len() {
            let Some(id) = self.next_unique_attachment_id(&mut reserved_ids) else {
                return Err(payload);
            };
            ids.push(id);
        }
        let mut pending_attachments = Vec::with_capacity(payload.attachments().len());
        let mut work = Vec::with_capacity(payload.attachments().len());
        for ((attachment, format), id) in payload.attachments().iter().zip(formats).zip(ids) {
            let bytes = attachment.bytes().to_vec();
            pending_attachments.push(ComposerAttachment::pending(
                id.clone(),
                attachment.name().to_owned(),
                Some(format),
                attachment.mime_type_str(),
                bytes.len(),
            ));
            work.push(RecalledAttachmentInput {
                id,
                name: attachment.name().to_owned(),
                mime_type: attachment.mime_type_str().to_owned(),
                bytes,
            });
        }

        // The target proves that no current attachment work can be useful to
        // this restore, but dropping the handles also cancels any clipboard
        // read that was still pending while the composer was empty.
        self.attachment_tasks.clear();
        self.draft_generation = self.draft_generation.saturating_add(1);
        self.draft_revision = self.draft_revision.saturating_add(1);
        self.selection_revision = self.selection_revision.saturating_add(1);
        self.state.set_draft(text);
        self.authored_text_present = text_present;
        self.attachments = pending_attachments;
        self.viewed_attachment = None;
        self.clear_attachment_preview();
        self.attachment_error = None;
        self.draft_tokens.clear();
        self.undo.clear();
        self.redo.clear();
        self.active_attachment_submission = None;
        self.active_submission_draft_revision = None;
        self.selection = self.state.draft().len()..self.state.draft().len();
        self.selection_reversed = false;
        self.selection_anchor = None;
        self.clear_vertical_goal();
        self.selection_dragging = false;
        self.marked_range = None;
        self.layout = None;
        self.painted_bounds = None;
        self.persist_current_draft();
        if !work.is_empty() {
            self.spawn_attachment_work(AttachmentWork::Recalled { items: work }, cx);
        }
        cx.notify();
        Ok(())
    }

    pub(crate) fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        if self.state.is_disabled() == disabled && self.send_blocked == disabled {
            return;
        }
        self.send_blocked = disabled;
        self.state.set_disabled(disabled);
        cx.notify();
    }

    pub(crate) fn switch_thread(
        &mut self,
        thread: &str,
        carry_draft: bool,
        cx: &mut Context<Self>,
    ) {
        if self.draft_thread.as_deref() == Some(thread) || self.state.is_submitting() {
            return;
        }

        self.persist_current_draft();
        self.draft_generation = self.draft_generation.saturating_add(1);
        self.draft_thread = Some(thread.to_owned());
        self.attachment_tasks.clear();
        self.selection_dragging = false;
        self.selection_anchor = None;
        self.clear_vertical_goal();
        self.active_attachment_submission = None;
        self.active_submission_draft_revision = None;
        self.viewed_attachment = None;
        self.clear_attachment_preview();

        if carry_draft {
            // A pending source belongs to the old generation. It is not safe
            // to let its completion enter the newly selected thread.
            self.attachments.retain(ComposerAttachment::is_ready);
            self.attachment_error = None;
            self.persist_current_draft();
        } else {
            self.attachments.clear();
            self.viewed_attachment = None;
            self.attachment_error = None;

            let mut session = ComposerDraftSession::for_key(thread.to_owned());
            let restoration = session.restore(&mut self.draft_store, true).restored;
            let (text, tokens, restored_attachments) = restoration.map_or_else(
                || (String::new(), Vec::new(), Vec::new()),
                |restoration| {
                    (
                        restoration.document.text,
                        restoration.document.tokens,
                        restoration.attachments,
                    )
                },
            );
            self.state.set_draft(text);
            self.authored_text_present = true;
            self.draft_revision = self.draft_revision.saturating_add(1);
            self.advance_selection_revision();
            self.draft_tokens = tokens;
            self.selection = self.state.draft().len()..self.state.draft().len();
            self.selection_reversed = false;
            self.selection_anchor = None;
            self.clear_vertical_goal();
            self.selection_dragging = false;
            self.marked_range = None;
            self.undo.clear();
            self.redo.clear();
            self.layout = None;
            self.painted_bounds = None;

            let mut inputs = Vec::new();
            for attachment in restored_attachments {
                let id = attachment.id.clone();
                let format = ImageFormat::from_mime_type(&attachment.mime_type);
                self.attachments.push(ComposerAttachment::pending(
                    id.clone(),
                    attachment.name.clone(),
                    format,
                    attachment.mime_type.clone(),
                    attachment.size_bytes,
                ));
                inputs.push(RestoredAttachmentInput {
                    id,
                    name: attachment.name,
                    mime_type: attachment.mime_type,
                    content_base64: attachment.content_base64,
                    size_bytes: attachment.size_bytes,
                    source_digest: attachment.source_digest,
                    source_size_bytes: attachment.source_size_bytes,
                });
            }
            if !inputs.is_empty() {
                self.spawn_attachment_work(AttachmentWork::Restored { items: inputs }, cx);
            }
        }
        cx.notify();
    }

    fn current_draft_document(&self) -> ComposerDraftDocument {
        ComposerDraftDocument::new(self.state.draft().to_owned(), self.draft_tokens.clone())
    }

    fn current_draft_attachments(
        &self,
    ) -> Vec<crate::composer_draft_session_policy::ComposerImageAttachment> {
        self.attachments
            .iter()
            .map(ComposerAttachment::draft_value)
            .collect()
    }

    fn persist_current_draft(&mut self) {
        let Some(thread) = self.draft_thread.clone() else {
            return;
        };
        let session = ComposerDraftSession::for_key(thread);
        let document = self.current_draft_document();
        let attachments = self.current_draft_attachments();
        let _ = session.persist(&mut self.draft_store, &document, &attachments);
    }

    pub(crate) fn set_surface(
        &mut self,
        blocked: bool,
        model_label: String,
        cx: &mut Context<Self>,
    ) {
        if self.send_blocked != blocked || self.model_label != model_label {
            self.send_blocked = blocked;
            self.model_label = model_label;
            cx.notify();
        }
    }

    pub(crate) fn send_ready(&self) -> bool {
        if self.send_blocked
            || self.state.is_disabled()
            || self.state.is_submitting()
            || self.marked_range.is_some()
        {
            return false;
        }
        if self.attachments.is_empty() {
            return self.state.send_ready();
        }
        self.attachment_delivery_enabled
            && self.attachment_error.is_none()
            && self.attachments.iter().all(ComposerAttachment::is_ready)
    }

    pub(crate) fn is_submitting(&self) -> bool {
        self.state.is_submitting()
    }

    /// Returns the caret quad for the focused, collapsed selection.
    ///
    /// `None` when blurred, when a non-empty selection owns the highlight,
    /// or without laid-out text geometry. The origin reuses the stored text
    /// layout exactly like the IME `bounds_for_range` seam, falling back to
    /// the layout origin so the caret shows on an empty field before any
    /// typing. The offset itself comes from [`caret_offset_for_paint`].
    pub(crate) fn caret_quad(&self, window: &Window) -> Option<gpui::PaintQuad> {
        if !self.focus_handle.is_focused(window) {
            return None;
        }
        let offset = caret_offset_for_paint(true, &self.selection, self.state.draft())?;
        let layout = self.layout.as_ref()?;
        let line_height = layout.line_height();
        let origin = layout
            .position_for_index(offset)
            .unwrap_or_else(|| layout.bounds().origin);
        if !valid_point(origin) || line_height <= Pixels::ZERO {
            return None;
        }
        Some(gpui::fill(
            Bounds::new(origin, size(px(2.0), line_height)),
            DesktopTheme::neutral_dark().foreground,
        ))
    }

    #[cfg(test)]
    pub(crate) fn begin_submission(
        &mut self,
    ) -> Result<(artisan_domain::MessageBody, SubmissionToken), SubmissionBlocked> {
        // The existing native transport only accepts MessageBody. Keeping
        // this path text-only prevents an attachment from being silently
        // serialized into text or claimed as delivered.
        if self.send_blocked || !self.attachments.is_empty() {
            return Err(SubmissionBlocked::Disabled);
        }
        let submission = self.state.begin_submission()?;
        self.active_attachment_submission = Some(NativeComposerAttachmentSnapshot {
            draft_generation: self.draft_generation,
            attachments: Vec::new(),
        });
        self.active_submission_draft_revision = Some(self.draft_revision);
        Ok(submission)
    }

    /// Builds the exact typed queue payload owned by the current draft.
    ///
    /// The current draft text is parsed without trimming and each ready tray
    /// item contributes one owned image in tray order. Image-only messages
    /// preserve whether authored text was absent or explicitly empty; no
    /// marker text is ever inserted. The payload is returned to the
    /// application transport, while this entity retains a byte-identical
    /// snapshot for accepted cleanup and retry matching.
    pub(crate) fn begin_payload_submission(
        &mut self,
    ) -> Result<(artisan_domain::QueueMessagePayload, SubmissionToken), SubmissionBlocked> {
        if self.send_blocked
            || self.state.is_disabled()
            || self.marked_range.is_some()
            || !self.attachment_delivery_enabled
            || self.attachment_error.is_some()
        {
            return Err(SubmissionBlocked::Disabled);
        }

        let snapshot = self
            .snapshot_ordered_ready_attachments()
            .map_err(|_| SubmissionBlocked::Disabled)?;
        let active_snapshot = snapshot.clone();
        let text = if self.authored_text_present {
            Some(
                artisan_domain::AuthoredText::parse(self.state.draft().to_owned()).map_err(
                    |error| match error {
                        artisan_domain::AuthoredTextError::TooLong { length, maximum } => {
                            SubmissionBlocked::InvalidBody(
                                artisan_domain::MessageBodyError::TooLong { length, maximum },
                            )
                        }
                    },
                )?,
            )
        } else {
            None
        };
        let images = snapshot
            .attachments
            .into_iter()
            .map(|attachment| {
                artisan_domain::ImageAttachment::new(
                    attachment.media_type,
                    attachment.bytes,
                    attachment.name,
                )
                .map_err(|_| SubmissionBlocked::Disabled)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let payload = artisan_domain::QueueMessagePayload::new(text, images)
            .map_err(|_| SubmissionBlocked::Disabled)?;
        let token = self.state.begin_payload_submission(&payload)?;
        self.active_attachment_submission = Some(active_snapshot);
        self.active_submission_draft_revision = Some(self.draft_revision);
        Ok((payload, token))
    }

    /// Compares a typed payload with the current authored text and complete
    /// ordered attachment bytes without beginning a submission or allocating
    /// a replacement snapshot.
    pub(crate) fn draft_matches_payload(
        &self,
        payload: &artisan_domain::QueueMessagePayload,
    ) -> bool {
        let text_matches = match (payload.text(), self.authored_text_present) {
            (Some(text), true) => text.as_str() == self.state.draft(),
            (None, false) => self.state.draft().is_empty(),
            _ => false,
        };
        text_matches
            && payload.attachments().len() == self.attachments.len()
            && payload
                .attachments()
                .iter()
                .zip(self.attachments.iter())
                .all(|(expected, current)| {
                    current.is_ready()
                        && expected.mime_type_str() == current.mime_type
                        && expected.name() == current.name
                        && current
                            .bytes
                            .as_deref()
                            .is_some_and(|bytes| expected.bytes() == bytes.as_slice())
                })
    }

    pub(crate) fn finish_submission(
        &mut self,
        token: SubmissionToken,
        disposition: DraftDisposition,
        cx: &mut Context<Self>,
    ) {
        let was_submitting = self.state.is_submitting();
        let active_snapshot = self.active_attachment_submission.clone();
        let clean_text = self
            .active_submission_draft_revision
            .is_some_and(|revision| revision == self.draft_revision);
        self.state.finish_submission(token, disposition);
        if was_submitting && !self.state.is_submitting() {
            if disposition == DraftDisposition::Accepted
                && clean_text
                && let Some(snapshot) = active_snapshot.as_ref()
            {
                self.clear_submitted_attachment_values(snapshot);
            }
            self.active_attachment_submission = None;
            self.active_submission_draft_revision = None;
            self.persist_current_draft();
            self.layout = None;
            self.painted_bounds = None;
            let end = self.selection.end.min(self.state.draft().len());
            self.selection = end..end;
            self.selection_reversed = false;
            self.authored_text_present = true;
            self.selection_anchor = None;
            self.clear_vertical_goal();
            self.selection_dragging = false;
            self.marked_range = None;
            self.advance_selection_revision();
            cx.notify();
        }
    }
}

fn valid_pixels(value: Pixels) -> bool {
    f32::from(value).is_finite()
}

fn valid_bounds(bounds: &Bounds<Pixels>) -> bool {
    !bounds.is_empty()
        && valid_pixels(bounds.origin.x)
        && valid_pixels(bounds.origin.y)
        && valid_pixels(bounds.size.width)
        && valid_pixels(bounds.size.height)
        && valid_pixels(bounds.right())
        && valid_pixels(bounds.bottom())
}

fn valid_point(point: Point<Pixels>) -> bool {
    valid_pixels(point.x) && valid_pixels(point.y)
}

/// Returns the draft byte offset where the native caret paints, or `None`
/// when no caret may paint.
///
/// The caret is a focused, collapsed-selection affordance: blur hides it, a
/// non-empty selection hides it in favor of the highlight, and a stale or
/// non-boundary offset hides it rather than misplacing it. The returned
/// offset is a UTF-8 byte index into the draft, matching
/// `TextLayout::position_for_index` exactly (the IME seam converts to UTF-16
/// separately), so multibyte text positions the caret after whole characters
/// and movement/typing follows the stored selection.
fn caret_offset_for_paint(focused: bool, selection: &Range<usize>, draft: &str) -> Option<usize> {
    if !focused || !selection.is_empty() {
        return None;
    }
    let offset = selection.end;
    if offset > draft.len() || !draft.is_char_boundary(offset) {
        return None;
    }
    Some(offset)
}

#[cfg(test)]
fn localize_painted_point(
    painted_bounds: &Bounds<Pixels>,
    global_point: Point<Pixels>,
) -> Option<Point<Pixels>> {
    valid_bounds(painted_bounds)
        .then_some(())
        .and_then(|()| valid_point(global_point).then_some(()))?;
    painted_bounds.localize(&global_point)
}

#[cfg(test)]
fn offset_layout_bounds(
    element_bounds: &Bounds<Pixels>,
    local_start: Point<Pixels>,
    local_end: Point<Pixels>,
    line_height: Pixels,
) -> Option<Bounds<Pixels>> {
    if !valid_bounds(element_bounds)
        || !valid_point(local_start)
        || !valid_point(local_end)
        || !valid_pixels(line_height)
        || line_height <= Pixels::ZERO
    {
        return None;
    }

    let width = (local_end.x - local_start.x).max(px(1.0));
    let bounds = Bounds::new(
        point(
            element_bounds.left() + local_start.x,
            element_bounds.top() + local_start.y,
        ),
        size(width, line_height),
    );
    valid_bounds(&bounds).then_some(bounds)
}

fn replace_text_preserving_raw(
    draft: &str,
    range: Range<usize>,
    replacement: &str,
) -> Option<String> {
    if range.start > range.end
        || range.end > draft.len()
        || !draft.is_char_boundary(range.start)
        || !draft.is_char_boundary(range.end)
    {
        return None;
    }
    let mut next = draft.to_owned();
    next.replace_range(range, replacement);
    Some(next)
}

fn utf16_range_to_utf8(text: &str, range: Range<usize>) -> Option<Range<usize>> {
    if range.start > range.end {
        return None;
    }
    Some(utf16_offset_to_utf8(text, range.start)?..utf16_offset_to_utf8(text, range.end)?)
}

fn utf16_offset_to_utf8(text: &str, target: usize) -> Option<usize> {
    let mut utf16_offset = 0;
    if target == 0 {
        return Some(0);
    }
    for (byte_offset, character) in text.char_indices() {
        if utf16_offset == target {
            return Some(byte_offset);
        }
        utf16_offset = utf16_offset.checked_add(character.len_utf16())?;
        if utf16_offset == target {
            return Some(byte_offset + character.len_utf8());
        }
        if utf16_offset > target {
            return None;
        }
    }
    (utf16_offset == target).then_some(text.len())
}

fn utf8_range_to_utf16(text: &str, range: Range<usize>) -> Option<Range<usize>> {
    if range.start > range.end
        || range.end > text.len()
        || !text.is_char_boundary(range.start)
        || !text.is_char_boundary(range.end)
    {
        return None;
    }
    Some(utf8_offset_to_utf16(text, range.start)?..utf8_offset_to_utf16(text, range.end)?)
}

fn utf8_offset_to_utf16(text: &str, byte_offset: usize) -> Option<usize> {
    if byte_offset > text.len() || !text.is_char_boundary(byte_offset) {
        return None;
    }
    text.get(..byte_offset)?
        .chars()
        .try_fold(0usize, |offset, character| {
            offset.checked_add(character.len_utf16())
        })
}

fn previous_char_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn previous_character_boundary(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn next_character_boundary(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    text[offset..]
        .chars()
        .next()
        .map_or(text.len(), |character| offset + character.len_utf8())
}

fn logical_line_start(text: &str, offset: usize) -> usize {
    let offset = previous_char_boundary(text, offset);
    text[..offset].rfind('\n').map_or(0, |index| index + 1)
}

fn logical_line_end(text: &str, offset: usize) -> usize {
    let offset = previous_char_boundary(text, offset);
    text[offset..]
        .find('\n')
        .map_or(text.len(), |index| offset + index)
}

fn logical_vertical_target(text: &str, cursor: usize, direction: i32, goal_column: usize) -> usize {
    let cursor = previous_char_boundary(text, cursor);
    let line_start = logical_line_start(text, cursor);
    let line_end = logical_line_end(text, cursor);

    let (target_start, target_end) = if direction < 0 {
        if line_start == 0 {
            return cursor;
        }
        let previous_end = line_start - 1;
        let previous_start = logical_line_start(text, previous_end);
        (previous_start, previous_end)
    } else {
        if line_end >= text.len() {
            return cursor;
        }
        let next_start = line_end + 1;
        (next_start, logical_line_end(text, next_start))
    };

    let target_line = &text[target_start..target_end];
    let mut target_column =
        goal_column.min(utf8_offset_to_utf16(target_line, target_line.len()).unwrap_or_default());
    while target_column > 0 && utf16_offset_to_utf8(target_line, target_column).is_none() {
        target_column -= 1;
    }
    let target_offset = utf16_offset_to_utf8(target_line, target_column).unwrap_or_default();
    target_start + target_offset
}

#[cfg(test)]
#[path = "native_composer/tests.rs"]
mod tests;

#[path = "native_composer/attachments.rs"]
mod attachments;

#[path = "native_composer/editing.rs"]
mod editing;

#[path = "native_composer/render.rs"]
mod render;
