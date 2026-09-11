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
    Div, Element, ElementId, ElementInputHandler, Entity, EventEmitter, ExternalPaths,
    FocusHandle, Focusable, GlobalElementId, HighlightStyle, ImageFormat, ImageSource,
    InspectorElementId, IntoElement, KeyBinding, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ObjectFit, Pixels, Point, Render, RenderImage, SharedString,
    Stateful, StyledText, Subscription, Task, UTF16Selection, Window, actions, div, img, point,
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

enum AttachmentWork {
    Clipboard {
        items: Vec<(String, ClipboardImageCandidate)>,
    },
    Files {
        items: Vec<(String, std::path::PathBuf)>,
    },
    Restored {
        items: Vec<RestoredAttachmentInput>,
    },
    Recalled {
        items: Vec<RecalledAttachmentInput>,
    },
}

#[derive(Clone)]
struct PasteScope {
    thread: Option<String>,
    draft_generation: u64,
    selection_revision: u64,
    replacement_range: Option<Range<usize>>,
}

struct AttachmentPreviewState {
    attachment_id: String,
    draft_generation: u64,
    request: u64,
    image: Arc<RenderImage>,
}

struct PreviewWork {
    format: ImageFormat,
    bytes: Arc<Vec<u8>>,
}

impl PreviewWork {
    fn run(self) -> Result<Arc<RenderImage>, AttachmentPreparationError> {
        render_full_preview(self.format, self.bytes.as_ref())
    }
}

impl AttachmentWork {
    fn run(self) -> Vec<AttachmentPreparationOutcome> {
        match self {
            Self::Clipboard { items } => prepare_clipboard_batch(items, None),
            Self::Files { items } => prepare_file_batch(items, None),
            Self::Restored { items } => prepare_restored_batch(items, None),
            Self::Recalled { items } => prepare_recalled_batch(items),
        }
    }
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
        controls: Entity<NativeComposerControls>,
        model_selector: Entity<NativeModelSelector>,
        cx: &mut Context<Self>,
    ) {
        self.controls = Some(controls.clone());
        self.model_selector = Some(model_selector.clone());
        self.controls_observation = Some(cx.observe(&controls, |composer, _, cx| {
            composer.invalidate_component_render(cx);
        }));
        self.model_selector_observation = Some(cx.observe(&model_selector, |composer, _, cx| {
            composer.invalidate_component_render(cx);
        }));
        cx.notify();
    }

    fn invalidate_component_render(&mut self, cx: &mut Context<Self>) {
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn draft(&self) -> &str {
        self.state.draft()
    }

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

    /// Returns the generation of the currently selected draft scope.
    ///
    /// A transport owner must carry this value with any attachment payload and
    /// reject a completion or retry whose generation no longer matches the
    /// mounted composer scope.
    pub(crate) const fn draft_generation(&self) -> u64 {
        self.draft_generation
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

    fn next_unique_attachment_id(&mut self, reserved: &mut HashSet<String>) -> Option<String> {
        for _ in 0..=MAXIMUM_ATTACHMENT_COUNT {
            let id = self.next_attachment_id();
            if !self
                .attachments
                .iter()
                .any(|attachment| attachment.id == id)
                && reserved.insert(id.clone())
            {
                return Some(id);
            }
        }
        None
    }

    /// Returns the number of live attachment slots, including pending reads.
    pub(crate) fn attachment_count(&self) -> usize {
        self.attachments.len()
    }

    /// Returns the current visible attachment failure, if any.
    pub(crate) fn attachment_error(&self) -> Option<&str> {
        self.attachment_error.as_deref()
    }

    /// Snapshots ready images in the exact tray order for a future typed
    /// message-content command.
    ///
    /// No text marker or placeholder is created. A pending image is an
    /// explicit refusal until its encoded bytes and tray thumbnail are ready.
    pub(crate) fn snapshot_ordered_ready_attachments(
        &self,
    ) -> Result<NativeComposerAttachmentSnapshot, AttachmentPayloadError> {
        let attachments = self
            .attachments
            .iter()
            .enumerate()
            .map(|(position, attachment)| attachment.payload(position))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(NativeComposerAttachmentSnapshot {
            draft_generation: self.draft_generation,
            attachments,
        })
    }

    /// Returns a fresh payload for a retry only when the complete ordered
    /// attachment set still matches the original generation and bytes.
    pub(crate) fn match_retry_attachment_payload(
        &self,
        snapshot: &NativeComposerAttachmentSnapshot,
    ) -> Option<NativeComposerAttachmentSnapshot> {
        let current = self.snapshot_ordered_ready_attachments().ok()?;
        (current == *snapshot).then_some(current)
    }

    /// Enables the future typed attachment submission path. The current
    /// `QueueFirstMessage` caller deliberately does not invoke this seam.
    pub(crate) fn set_attachment_delivery_enabled(
        &mut self,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        if self.attachment_delivery_enabled != enabled {
            self.attachment_delivery_enabled = enabled;
            cx.notify();
        }
    }

    fn clear_attachment_preview(&mut self) {
        self.attachment_preview_request = self.attachment_preview_request.saturating_add(1);
        self.attachment_preview = None;
        self.attachment_preview_error = None;
        self.attachment_preview_task = None;
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
        thread: String,
        carry_draft: bool,
        cx: &mut Context<Self>,
    ) {
        if self.draft_thread.as_ref() == Some(&thread) || self.state.is_submitting() {
            return;
        }

        self.persist_current_draft();
        self.draft_generation = self.draft_generation.saturating_add(1);
        self.draft_thread = Some(thread.clone());
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

            let mut session = ComposerDraftSession::for_key(thread.clone());
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

    fn next_attachment_id(&mut self) -> String {
        let id = format!("attachment:{}", self.next_attachment_id);
        self.next_attachment_id = self.next_attachment_id.saturating_add(1);
        id
    }

    fn current_pending_input_total(&self) -> usize {
        self.attachments
            .iter()
            .filter(|attachment| !attachment.is_ready())
            .fold(0usize, |total, attachment| {
                total.saturating_add(attachment.source_bytes_len())
            })
    }

    fn enqueue_clipboard_images(
        &mut self,
        candidates: Vec<ClipboardImageCandidate>,
        cx: &mut Context<Self>,
    ) {
        self.attachment_error = None;
        let mut pending_input_total = self.current_pending_input_total();
        let mut work = Vec::new();
        for candidate in candidates {
            if !matches!(
                candidate.format,
                ImageFormat::Gif | ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::Webp
            ) {
                self.attachment_error =
                    Some(native_composer_attachments::ATTACHMENT_UNSUPPORTED_FORMAT_MESSAGE.into());
                continue;
            }
            if candidate.bytes.len() > MAXIMUM_RAW_ATTACHMENT_BYTES {
                self.attachment_error =
                    Some(native_composer_attachments::ATTACHMENT_TOO_LARGE_MESSAGE.into());
                continue;
            }
            if self.attachments.len() >= MAXIMUM_ATTACHMENT_COUNT {
                self.attachment_error =
                    Some(native_composer_attachments::ATTACHMENT_COUNT_LIMIT_MESSAGE.into());
                break;
            }
            let incoming_size = candidate.bytes.len();
            if pending_input_total.saturating_add(incoming_size)
                > MAXIMUM_RAW_ATTACHMENT_TOTAL_BYTES
            {
                self.attachment_error =
                    Some(native_composer_attachments::ATTACHMENT_TOTAL_LIMIT_MESSAGE.into());
                continue;
            }

            let id = self.next_attachment_id();
            let name = candidate.name.clone();
            self.attachments.push(ComposerAttachment::pending(
                id.clone(),
                name,
                Some(candidate.format),
                candidate.format.mime_type(),
                incoming_size,
            ));
            pending_input_total = pending_input_total.saturating_add(incoming_size);
            work.push((id, candidate));
        }

        if !work.is_empty() {
            self.spawn_attachment_work(AttachmentWork::Clipboard { items: work }, cx);
            self.persist_current_draft();
            cx.notify();
        }
    }

    fn enqueue_file_drop(&mut self, paths: Vec<std::path::PathBuf>, cx: &mut Context<Self>) {
        self.attachment_error = None;
        if paths.is_empty() {
            return;
        }
        if self.attachments.len().saturating_add(paths.len()) > MAXIMUM_ATTACHMENT_COUNT {
            self.attachment_error =
                Some(native_composer_attachments::ATTACHMENT_COUNT_LIMIT_MESSAGE.into());
            cx.notify();
            return;
        }

        let mut work = Vec::with_capacity(paths.len());
        for path in paths {
            let id = self.next_attachment_id();
            let name = display_file_name(&path);
            self.attachments
                .push(ComposerAttachment::pending(id.clone(), name, None, "", 0));
            work.push((id, path));
        }
        self.spawn_attachment_work(AttachmentWork::Files { items: work }, cx);
        self.persist_current_draft();
        cx.notify();
    }

    fn spawn_attachment_work(&mut self, work: AttachmentWork, cx: &mut Context<Self>) {
        // Task handles cancel their futures when dropped. Completed handles do
        // not need to remain in the composer, so retire them before every new
        // piece of attachment work. The draft generation and attachment IDs
        // below remain the authoritative completion fences for work that is
        // still running.
        self.prune_attachment_tasks();
        let generation = self.draft_generation;
        let thread = self.draft_thread.clone();
        let task = cx.spawn(async move |this, cx| {
            let outcomes = cx
                .background_executor()
                .spawn(async move { work.run() })
                .await;
            this.update(cx, |composer, composer_cx| {
                composer.apply_attachment_outcomes(thread, generation, outcomes, composer_cx);
            })
            .ok();
        });
        self.attachment_tasks.push(task);
    }

    fn apply_attachment_outcomes(
        &mut self,
        thread: Option<String>,
        generation: u64,
        outcomes: Vec<AttachmentPreparationOutcome>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.draft_generation || thread != self.draft_thread {
            self.prune_attachment_tasks();
            return;
        }

        let mut changed = false;
        for outcome in outcomes {
            let Some(index) = self
                .attachments
                .iter()
                .position(|attachment| attachment.id == outcome.id)
            else {
                continue;
            };
            changed = true;
            match outcome.result {
                Ok(prepared) => {
                    if self.attachment_is_duplicate(index, &prepared) {
                        self.attachments.remove(index);
                        self.attachment_error = Some("That image is already attached.".into());
                        continue;
                    }
                    let other_total = self
                        .attachments
                        .iter()
                        .enumerate()
                        .filter(|(other_index, attachment)| {
                            *other_index != index && attachment.is_ready()
                        })
                        .fold(0usize, |total, (_, attachment)| {
                            total.saturating_add(attachment.size_bytes)
                        });
                    if other_total.saturating_add(prepared.size_bytes)
                        > MAXIMUM_ATTACHMENT_TOTAL_BYTES
                    {
                        self.attachments.remove(index);
                        self.attachment_error = Some(
                            native_composer_attachments::ATTACHMENT_TOTAL_LIMIT_MESSAGE.into(),
                        );
                        continue;
                    }
                    self.attachments[index] = ComposerAttachment::from_prepared(prepared);
                }
                Err(error) => {
                    self.attachments.remove(index);
                    self.attachment_error = Some(format!("{}: {error}", outcome.name));
                }
            }
        }
        if changed {
            self.viewed_attachment = self.viewed_attachment.take().filter(|id| {
                self.attachments
                    .iter()
                    .any(|attachment| &attachment.id == id)
            });
            self.persist_current_draft();
            cx.notify();
        }
        self.prune_attachment_tasks();
    }

    fn attachment_is_duplicate(&self, index: usize, prepared: &PreparedComposerAttachment) -> bool {
        self.attachments
            .iter()
            .enumerate()
            .filter(|(other_index, _)| *other_index != index)
            .any(|(_, attachment)| {
                (attachment.is_ready()
                    && attachment.source_digest == prepared.source_digest
                    && attachment.source_size_bytes == prepared.source_size_bytes)
                    || (attachment.is_ready()
                        && attachment.encoded_digest == prepared.encoded_digest
                        && attachment.size_bytes == prepared.size_bytes)
            })
    }

    fn remove_attachment(&mut self, attachment_id: &str, cx: &mut Context<Self>) {
        self.prune_attachment_tasks();
        let Some(index) = self
            .attachments
            .iter()
            .position(|attachment| attachment.id == attachment_id)
        else {
            return;
        };
        self.attachments.remove(index);
        if self.viewed_attachment.as_deref() == Some(attachment_id) {
            self.viewed_attachment = None;
            self.clear_attachment_preview();
        }
        self.attachment_error = None;
        self.persist_current_draft();
        cx.notify();
    }

    fn view_attachment(&mut self, attachment_id: &str, cx: &mut Context<Self>) {
        let Some(attachment) = self
            .attachments
            .iter()
            .find(|attachment| attachment.id == attachment_id && attachment.is_ready())
        else {
            return;
        };
        let Some(format) = attachment.format else {
            return;
        };
        let Some(bytes) = attachment.bytes.clone() else {
            return;
        };

        self.viewed_attachment = Some(attachment_id.to_owned());
        self.clear_attachment_preview();
        let request = self.attachment_preview_request;
        let generation = self.draft_generation;
        let id = attachment_id.to_owned();
        let task = cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { PreviewWork { format, bytes }.run() })
                .await;
            this.update(cx, |composer, composer_cx| {
                if composer.draft_generation != generation
                    || composer.attachment_preview_request != request
                    || composer.viewed_attachment.as_deref() != Some(id.as_str())
                {
                    return;
                }
                composer.attachment_preview_task = None;
                match result {
                    Ok(image) => {
                        composer.attachment_preview = Some(AttachmentPreviewState {
                            attachment_id: id,
                            draft_generation: generation,
                            request,
                            image,
                        });
                    }
                    Err(error) => {
                        composer.attachment_preview_error = Some(error.to_string());
                    }
                }
                composer_cx.notify();
            })
            .ok();
        });
        self.attachment_preview_task = Some(task);
        cx.notify();
    }

    fn close_attachment_viewer(&mut self, cx: &mut Context<Self>) {
        if self.viewed_attachment.take().is_some() {
            self.clear_attachment_preview();
            cx.notify();
        }
    }

    fn clear_submitted_attachment_values(
        &mut self,
        snapshot: &NativeComposerAttachmentSnapshot,
    ) -> bool {
        if !self.attachment_snapshot_matches_current(snapshot) {
            return false;
        }
        let ids = snapshot
            .attachments
            .iter()
            .map(|attachment| attachment.client_token.as_str())
            .collect::<std::collections::HashSet<_>>();
        if ids.is_empty() {
            return true;
        }
        if self
            .viewed_attachment
            .as_deref()
            .is_some_and(|id| ids.contains(id))
        {
            self.viewed_attachment = None;
            self.clear_attachment_preview();
        }
        self.attachments
            .retain(|attachment| !ids.contains(attachment.id.as_str()));
        self.persist_current_draft();
        true
    }

    fn attachment_snapshot_matches_current(
        &self,
        snapshot: &NativeComposerAttachmentSnapshot,
    ) -> bool {
        if snapshot.draft_generation != self.draft_generation {
            return false;
        }
        snapshot.attachments.iter().all(|expected| {
            let Some((position, current)) = self
                .attachments
                .iter()
                .enumerate()
                .find(|(_, attachment)| attachment.id == expected.client_token)
            else {
                return false;
            };
            position == expected.position
                && current
                    .payload(position)
                    .is_ok_and(|payload| payload == expected.clone())
        })
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

    /// Compatibility shim for callers that still require the legacy text
    /// body. Typed message delivery must use [`Self::begin_payload_submission`].
    pub(crate) fn begin_submission_with_attachments(
        &mut self,
    ) -> Result<
        (
            artisan_domain::MessageBody,
            SubmissionToken,
            NativeComposerAttachmentSnapshot,
        ),
        SubmissionBlocked,
    > {
        if self.send_blocked || !self.attachments.is_empty() {
            return Err(SubmissionBlocked::Disabled);
        }
        let submission = self.state.begin_submission()?;
        self.active_attachment_submission = Some(NativeComposerAttachmentSnapshot {
            draft_generation: self.draft_generation,
            attachments: Vec::new(),
        });
        self.active_submission_draft_revision = Some(self.draft_revision);
        Ok((
            submission.0,
            submission.1,
            NativeComposerAttachmentSnapshot {
                draft_generation: self.draft_generation,
                attachments: Vec::new(),
            },
        ))
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

    /// Completes cleanup for an accepted typed attachment payload.
    ///
    /// The snapshot must still belong to the current draft generation and
    /// every submitted payload must remain byte-identical. New attachments
    /// appended after submission are retained.
    pub(crate) fn complete_accepted_attachment_cleanup(
        &mut self,
        snapshot: &NativeComposerAttachmentSnapshot,
        cx: &mut Context<Self>,
    ) -> bool {
        let cleaned = self.clear_submitted_attachment_values(snapshot);
        if cleaned {
            cx.notify();
        }
        cleaned
    }

    fn undo_action(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_disabled() || self.state.is_submitting() || self.marked_range.is_some() {
            return;
        }
        if let Some((draft, selection)) = self.undo.pop() {
            self.redo
                .push((self.state.draft().to_owned(), self.selection.clone()));
            self.state.set_draft(draft);
            self.draft_revision = self.draft_revision.saturating_add(1);
            self.selection = selection;
            self.selection_reversed = false;
            self.selection_anchor = None;
            self.clear_vertical_goal();
            self.selection_dragging = false;
            self.advance_selection_revision();
            self.layout = None;
            self.persist_current_draft();
            cx.notify();
        }
    }

    fn redo_action(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_disabled() || self.state.is_submitting() || self.marked_range.is_some() {
            return;
        }
        if let Some((draft, selection)) = self.redo.pop() {
            self.undo
                .push((self.state.draft().to_owned(), self.selection.clone()));
            self.state.set_draft(draft);
            self.draft_revision = self.draft_revision.saturating_add(1);
            self.selection = selection;
            self.selection_reversed = false;
            self.selection_anchor = None;
            self.clear_vertical_goal();
            self.selection_dragging = false;
            self.advance_selection_revision();
            self.layout = None;
            self.persist_current_draft();
            cx.notify();
        }
    }

    fn request_send(&mut self, cx: &mut Context<Self>) {
        if self.send_ready() {
            cx.emit(NativeComposerEvent::SendRequested);
        }
    }

    fn request_send_action(&mut self, _: &RequestSend, _: &mut Window, cx: &mut Context<Self>) {
        self.request_send(cx);
    }

    fn replace_range(
        &mut self,
        range: Range<usize>,
        replacement: &str,
        marked_selection: Option<Range<usize>>,
        cx: &mut Context<Self>,
    ) {
        if self.state.is_disabled() {
            return;
        }
        let Some(next) =
            replace_text_preserving_raw(self.state.draft(), range.clone(), replacement)
        else {
            return;
        };
        let selected_offsets = match marked_selection {
            Some(selected_range) if selected_range.start <= selected_range.end => Some((
                match utf16_offset_to_utf8(replacement, selected_range.start) {
                    Some(offset) => offset,
                    None => return,
                },
                match utf16_offset_to_utf8(replacement, selected_range.end) {
                    Some(offset) => offset,
                    None => return,
                },
            )),
            Some(_) => return,
            None => None,
        };
        let changed = next != self.state.draft();
        if changed && self.marked_range.is_none() {
            if self.undo.len() >= 64 {
                self.undo.remove(0);
            }
            self.undo
                .push((self.state.draft().to_owned(), self.selection.clone()));
            self.redo.clear();
        }
        self.state.set_draft(next);
        if !replacement.is_empty() || !range.is_empty() {
            self.authored_text_present = true;
        }
        if changed {
            self.draft_revision = self.draft_revision.saturating_add(1);
        }
        self.advance_selection_revision();
        self.layout = None;
        self.painted_bounds = None;
        self.selection_anchor = None;
        self.clear_vertical_goal();
        self.selection_dragging = false;
        let replacement_end = range.start.saturating_add(replacement.len());
        if let Some((start, end)) = selected_offsets {
            self.selection = range.start + start..range.start + end;
            self.selection_reversed = false;
            self.marked_range = Some(range.start..replacement_end);
        } else {
            self.selection = replacement_end..replacement_end;
            self.selection_reversed = false;
            self.marked_range = None;
        }
        if changed {
            self.persist_current_draft();
        }
        cx.notify();
    }

    fn replacement_range(&self, range: Option<Range<usize>>) -> Option<Range<usize>> {
        let draft = self.state.draft();
        let range = match range {
            Some(range) => utf16_range_to_utf8(draft, range),
            None => self
                .marked_range
                .clone()
                .or_else(|| Some(self.selection.clone())),
        }?;
        (range.start <= range.end
            && range.end <= draft.len()
            && draft.is_char_boundary(range.start)
            && draft.is_char_boundary(range.end))
        .then_some(range)
    }

    fn current_selection(&self) -> Range<usize> {
        self.selection.clone()
    }

    fn begin_selection_drag(&mut self, point: Point<Pixels>, extend: bool, cx: &mut Context<Self>) {
        let Some(byte_index) = self.byte_index_for_global_point(point) else {
            return;
        };
        if extend {
            self.select_to(byte_index);
        } else {
            self.selection = byte_index..byte_index;
            self.selection_reversed = false;
            self.selection_anchor = None;
            self.advance_selection_revision();
        }
        self.marked_range = None;
        self.clear_vertical_goal();
        self.selection_dragging = true;
        cx.notify();
    }

    fn update_selection_drag(&mut self, point: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.selection_dragging {
            return;
        }
        let Some(byte_index) = self.byte_index_for_drag_point(point) else {
            return;
        };
        let old_selection = self.selection.clone();
        let old_reversed = self.selection_reversed;
        self.select_to(byte_index);
        self.marked_range = None;
        self.clear_vertical_goal();
        if old_selection != self.selection || old_reversed != self.selection_reversed {
            cx.notify();
        }
    }

    fn end_selection_drag(&mut self) {
        self.selection_dragging = false;
    }

    pub(crate) fn bind_actions(cx: &mut App) {
        cx.bind_keys([
            KeyBinding::new("ctrl-z", Undo, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("cmd-z", Undo, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("ctrl-shift-z", Redo, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("cmd-shift-z", Redo, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("ctrl-y", Redo, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("backspace", Backspace, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("delete", Delete, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("left", Left, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("right", Right, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("shift-left", SelectLeft, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new(
                "shift-right",
                SelectRight,
                Some(NATIVE_COMPOSER_KEY_CONTEXT),
            ),
            KeyBinding::new("cmd-a", SelectAll, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("ctrl-a", SelectAll, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("cmd-v", Paste, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("ctrl-v", Paste, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("cmd-c", Copy, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("ctrl-c", Copy, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("cmd-x", Cut, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("ctrl-x", Cut, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("home", Home, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("end", End, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("up", Up, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("down", Down, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("shift-home", SelectHome, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("shift-end", SelectEnd, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("shift-up", SelectUp, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("shift-down", SelectDown, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("ctrl-home", DocumentHome, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("ctrl-end", DocumentEnd, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new(
                "ctrl-shift-home",
                SelectDocumentHome,
                Some(NATIVE_COMPOSER_KEY_CONTEXT),
            ),
            KeyBinding::new(
                "ctrl-shift-end",
                SelectDocumentEnd,
                Some(NATIVE_COMPOSER_KEY_CONTEXT),
            ),
            KeyBinding::new("cmd-home", DocumentHome, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("cmd-end", DocumentEnd, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new(
                "cmd-shift-home",
                SelectDocumentHome,
                Some(NATIVE_COMPOSER_KEY_CONTEXT),
            ),
            KeyBinding::new(
                "cmd-shift-end",
                SelectDocumentEnd,
                Some(NATIVE_COMPOSER_KEY_CONTEXT),
            ),
            KeyBinding::new("cmd-up", DocumentHome, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new("cmd-down", DocumentEnd, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new(
                "cmd-shift-up",
                SelectDocumentHome,
                Some(NATIVE_COMPOSER_KEY_CONTEXT),
            ),
            KeyBinding::new(
                "cmd-shift-down",
                SelectDocumentEnd,
                Some(NATIVE_COMPOSER_KEY_CONTEXT),
            ),
            KeyBinding::new("enter", RequestSend, Some(NATIVE_COMPOSER_KEY_CONTEXT)),
            KeyBinding::new(
                "shift-enter",
                InsertNewline,
                Some(NATIVE_COMPOSER_KEY_CONTEXT),
            ),
        ]);
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selection.start
        } else {
            self.selection.end
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selection = offset..offset;
        self.selection_reversed = false;
        self.selection_anchor = None;
        self.clear_vertical_goal();
        self.marked_range = None;
        self.selection_dragging = false;
        self.advance_selection_revision();
        cx.notify();
    }

    fn select_to(&mut self, offset: usize) {
        let anchor = match self.selection_anchor {
            Some(anchor) => anchor,
            None => {
                let anchor = if self.selection.is_empty() {
                    self.cursor_offset()
                } else if self.selection_reversed {
                    self.selection.end
                } else {
                    self.selection.start
                };
                self.selection_anchor = Some(anchor);
                anchor
            }
        };
        if offset < anchor {
            self.selection = offset..anchor;
            self.selection_reversed = true;
        } else {
            self.selection = anchor..offset;
            self.selection_reversed = false;
        }
        self.advance_selection_revision();
    }

    fn move_left(&mut self, extend: bool, cx: &mut Context<Self>) {
        let target = if !extend && !self.selection.is_empty() {
            self.selection.start
        } else {
            previous_character_boundary(self.state.draft(), self.cursor_offset())
        };
        if extend {
            self.select_to(target);
            self.marked_range = None;
            self.clear_vertical_goal();
            self.selection_dragging = false;
            cx.notify();
        } else {
            self.move_to(target, cx);
        }
    }

    fn move_right(&mut self, extend: bool, cx: &mut Context<Self>) {
        let target = if !extend && !self.selection.is_empty() {
            self.selection.end
        } else {
            next_character_boundary(self.state.draft(), self.cursor_offset())
        };
        if extend {
            self.select_to(target);
            self.marked_range = None;
            self.clear_vertical_goal();
            self.selection_dragging = false;
            cx.notify();
        } else {
            self.move_to(target, cx);
        }
    }

    fn delete_backward(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_disabled() {
            return;
        }
        let range = self
            .marked_range
            .clone()
            .filter(|range| !range.is_empty())
            .or_else(|| (!self.selection.is_empty()).then_some(self.selection.clone()))
            .or_else(|| {
                let cursor = self.cursor_offset();
                (cursor > 0)
                    .then(|| previous_character_boundary(self.state.draft(), cursor)..cursor)
            });
        if let Some(range) = range {
            self.replace_range(range, "", None, cx);
        }
    }

    fn delete_forward(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_disabled() {
            return;
        }
        let range = self
            .marked_range
            .clone()
            .filter(|range| !range.is_empty())
            .or_else(|| (!self.selection.is_empty()).then_some(self.selection.clone()))
            .or_else(|| {
                let cursor = self.cursor_offset();
                (cursor < self.state.draft().len())
                    .then(|| cursor..next_character_boundary(self.state.draft(), cursor))
            });
        if let Some(range) = range {
            self.replace_range(range, "", None, cx);
        }
    }

    fn move_left_action(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if !self.state.is_disabled() {
            self.move_left(false, cx);
        }
    }

    fn move_right_action(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if !self.state.is_disabled() {
            self.move_right(false, cx);
        }
    }

    fn select_left_action(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        if !self.state.is_disabled() {
            self.move_left(true, cx);
        }
    }

    fn select_right_action(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        if !self.state.is_disabled() {
            self.move_right(true, cx);
        }
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        if !self.state.is_disabled() {
            self.selection = 0..self.state.draft().len();
            self.selection_reversed = false;
            self.selection_anchor = None;
            self.clear_vertical_goal();
            self.selection_dragging = false;
            self.marked_range = None;
            self.advance_selection_revision();
            cx.notify();
        }
    }

    fn move_home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        if !self.state.is_disabled() {
            let cursor = self.cursor_offset();
            let line_start = logical_line_start(self.state.draft(), cursor);
            self.move_to(line_start, cx);
        }
    }

    fn move_end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        if !self.state.is_disabled() {
            let cursor = self.cursor_offset();
            let line_end = logical_line_end(self.state.draft(), cursor);
            self.move_to(line_end, cx);
        }
    }

    fn move_up_action(&mut self, _: &Up, window: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(-1, window, false, cx);
    }

    fn move_down_action(&mut self, _: &Down, window: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(1, window, false, cx);
    }

    fn select_home_action(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_disabled() {
            return;
        }
        let target = logical_line_start(self.state.draft(), self.cursor_offset());
        self.select_to(target);
        self.marked_range = None;
        self.clear_vertical_goal();
        self.selection_dragging = false;
        cx.notify();
    }

    fn select_end_action(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_disabled() {
            return;
        }
        let target = logical_line_end(self.state.draft(), self.cursor_offset());
        self.select_to(target);
        self.marked_range = None;
        self.clear_vertical_goal();
        self.selection_dragging = false;
        cx.notify();
    }

    fn move_document_home_action(
        &mut self,
        _: &DocumentHome,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.state.is_disabled() {
            self.move_to(0, cx);
        }
    }

    fn move_document_end_action(
        &mut self,
        _: &DocumentEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.state.is_disabled() {
            self.move_to(self.state.draft().len(), cx);
        }
    }

    fn select_document_home_action(
        &mut self,
        _: &SelectDocumentHome,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.is_disabled() {
            return;
        }
        self.select_to(0);
        self.marked_range = None;
        self.clear_vertical_goal();
        self.selection_dragging = false;
        cx.notify();
    }

    fn select_document_end_action(
        &mut self,
        _: &SelectDocumentEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.is_disabled() {
            return;
        }
        self.select_to(self.state.draft().len());
        self.marked_range = None;
        self.clear_vertical_goal();
        self.selection_dragging = false;
        cx.notify();
    }

    fn select_up_action(&mut self, _: &SelectUp, window: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(-1, window, true, cx);
    }

    fn select_down_action(&mut self, _: &SelectDown, window: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(1, window, true, cx);
    }

    fn move_vertical(
        &mut self,
        direction: i32,
        _window: &mut Window,
        extend: bool,
        cx: &mut Context<Self>,
    ) {
        if self.state.is_disabled() {
            return;
        }

        let cursor = self.cursor_offset();
        let target = self.vertical_target(cursor, direction);
        if extend {
            self.select_to(target);
            self.marked_range = None;
            self.selection_dragging = false;
            cx.notify();
        } else {
            // A vertical move keeps its x/column goal across repeated Up/Down
            // presses, even when an intermediate line is shorter.
            self.selection = target..target;
            self.selection_reversed = false;
            self.selection_anchor = None;
            self.marked_range = None;
            self.selection_dragging = false;
            self.advance_selection_revision();
            cx.notify();
        }
    }

    fn vertical_target(&mut self, cursor: usize, direction: i32) -> usize {
        let draft = self.state.draft().to_owned();
        let goal_column = self.vertical_goal_column.unwrap_or_else(|| {
            let line_start = logical_line_start(&draft, cursor);
            utf8_offset_to_utf16(&draft, cursor)
                .unwrap_or_default()
                .saturating_sub(utf8_offset_to_utf16(&draft, line_start).unwrap_or_default())
        });
        self.vertical_goal_column = Some(goal_column);

        if self.painted_bounds.is_some()
            && let Some(layout) = self.layout.clone()
            && let Some(position) = layout.position_for_index(cursor)
        {
            let goal_x = self.vertical_goal_x.unwrap_or(position.x);
            self.vertical_goal_x = Some(goal_x);
            // `TextLayout::index_for_position` treats the exact bottom edge of
            // a row as belonging to that row. Aim at the center of the target
            // row so Down/Up cannot accidentally resolve back to the source
            // row at the shared line boundary.
            let target_y =
                position.y + layout.line_height() * direction as f32 + layout.line_height() / 2.0;
            let layout_bounds = layout.bounds();
            if target_y < layout_bounds.top() || target_y >= layout_bounds.bottom() {
                return cursor;
            }
            let target = match layout.index_for_position(point(goal_x, target_y)) {
                Ok(index) | Err(index) => index,
            };
            return previous_char_boundary(&draft, target.min(draft.len()));
        }

        logical_vertical_target(&draft, cursor, direction, goal_column)
    }

    fn insert_newline(&mut self, _: &InsertNewline, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_disabled() {
            return;
        }
        let Some(range) = self.replacement_range(None) else {
            return;
        };
        self.replace_range(range, "\n", None, cx);
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_disabled() {
            return;
        }
        self.prune_attachment_tasks();
        let scope = PasteScope {
            thread: self.draft_thread.clone(),
            draft_generation: self.draft_generation,
            selection_revision: self.selection_revision,
            replacement_range: self.replacement_range(None),
        };
        let clipboard = cx.read_from_clipboard_async();
        let task = cx.spawn(async move |this, cx| {
            let Ok(Some(item)) = clipboard.await else {
                return;
            };
            match native_composer_attachments::classify_clipboard(item) {
                ClipboardInput::Images(candidates) => {
                    this.update(cx, |composer, composer_cx| {
                        if !composer.paste_scope_is_current(&scope) {
                            return;
                        }
                        composer.enqueue_clipboard_images(candidates, composer_cx);
                    })
                    .ok();
                }
                ClipboardInput::Files(paths) => {
                    this.update(cx, |composer, composer_cx| {
                        if !composer.paste_scope_is_current(&scope) {
                            return;
                        }
                        composer.enqueue_file_drop(paths, composer_cx);
                    })
                    .ok();
                }
                ClipboardInput::Text(text) => {
                    let Some(range) = scope.replacement_range.clone() else {
                        return;
                    };
                    this.update(cx, |composer, composer_cx| {
                        if !composer.paste_scope_is_current(&scope) {
                            return;
                        }
                        composer.replace_range(range, &text, None, composer_cx);
                    })
                    .ok();
                }
                ClipboardInput::Empty => {}
            }
        });
        self.attachment_tasks.push(task);
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_disabled() || self.selection.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.state.draft()[self.selection.clone()].to_owned(),
        ));
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if self.state.is_disabled() || self.selection.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.state.draft()[self.selection.clone()].to_owned(),
        ));
        let range = self.selection.clone();
        self.replace_range(range, "", None, cx);
    }

    fn attachment_tray(
        &self,
        entity: Entity<Self>,
        theme: ArtisanTheme,
        desktop_theme: DesktopTheme,
    ) -> Stateful<Div> {
        // Reference (`attachment-tray.svelte:28-30`): the open row carries
        // `px-1 pt-1 pb-2`. The tray only mounts while attachments exist,
        // which is exactly the reference open state.
        let mut row = div()
            .id("artisan-native-composer-attachment-tray-row")
            .w_full()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(4.0))
            .pt(px(4.0))
            .pb(px(8.0))
            .overflow_x_scroll();

        for (position, attachment) in self.attachments.iter().enumerate() {
            let attachment_id = attachment.id.clone();
            let name = attachment.name.clone();
            let view_entity = entity.clone();
            // Reference (`attachment-tray.svelte:32`): `card relative size-18
            // overflow-hidden rounded-xl`. The tile is a regular card, not a
            // card-glass surface.
            let mut tile = div()
                .id(format!("artisan-native-composer-attachment-{position}"))
                .relative()
                .size(px(NATIVE_COMPOSER_ATTACHMENT_SIZE))
                .flex_none()
                .overflow_hidden()
                .rounded(px(14.0))
                .shadow(card_shadows(theme))
                .bg(desktop_theme.field)
                .cursor_pointer()
                .role(gpui::Role::Button)
                .aria_label(format!("View {name}"))
                .debug_selector(|| "artisan-native-composer-attachment".to_owned())
                .on_click(move |_, _, cx| {
                    view_entity.update(cx, |composer, composer_cx| {
                        composer.view_attachment(&attachment_id, composer_cx);
                    });
                });

            if let Some(thumbnail) = attachment.thumbnail.clone() {
                tile = tile.child(
                    img(ImageSource::Render(thumbnail))
                        .size_full()
                        .object_fit(ObjectFit::Cover),
                );
            } else {
                tile = tile.child(
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .px(px(5.0))
                        .text_color(desktop_theme.secondary)
                        .text_size(px(11.0))
                        .child("Preparing…"),
                );
            }

            let remove_click_id = attachment.id.clone();
            let remove_key_id = attachment.id.clone();
            let remove_click_entity = entity.clone();
            let remove_key_entity = entity.clone();
            let remove_label = format!("Remove {name}");
            // Reference (`attachment-tray.svelte:41-49`): `absolute
            // top/right 0.2rem`, `size-5.5`, secondary icon button, `X
            // size-3.5`.
            let remove = div()
                .id(format!(
                    "artisan-native-composer-attachment-remove-{position}"
                ))
                .absolute()
                .top(px(3.2))
                .right(px(3.2))
                .size(px(22.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(desktop_theme.chrome.opacity(0.9))
                .text_color(desktop_theme.foreground)
                .cursor_pointer()
                .tab_index(0)
                .role(gpui::Role::Button)
                .aria_label(remove_label)
                .debug_selector(|| "artisan-native-composer-attachment-remove".to_owned())
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    remove_click_entity.update(cx, |composer, composer_cx| {
                        composer.remove_attachment(&remove_click_id, composer_cx);
                    });
                })
                .on_key_down(move |event, _, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        cx.stop_propagation();
                        remove_key_entity.update(cx, |composer, composer_cx| {
                            composer.remove_attachment(&remove_key_id, composer_cx);
                        });
                    }
                })
                .child(asset_glyph(AssetId::TABLER_X).size(px(14.0)));
            tile = tile.child(remove);
            row = row.child(tile);
        }

        let mut tray = div()
            .id(NATIVE_COMPOSER_ATTACHMENT_TRAY_SELECTOR)
            .w_full()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .debug_selector(|| NATIVE_COMPOSER_ATTACHMENT_TRAY_SELECTOR.to_owned())
            .aria_label("Attachments")
            .child(row);
        if let Some(error) = self.attachment_error.clone() {
            tray = tray.child(
                div()
                    .text_color(desktop_theme.secondary)
                    .text_size(px(12.0))
                    .child(error),
            );
        }
        if !self.attachment_delivery_enabled {
            tray = tray.child(
                div()
                    .id(NATIVE_COMPOSER_ATTACHMENT_BLOCKED_SELECTOR)
                    .text_color(desktop_theme.secondary)
                    .text_size(px(12.0))
                    .debug_selector(|| NATIVE_COMPOSER_ATTACHMENT_BLOCKED_SELECTOR.to_owned())
                    .child("Images stay attached until image delivery is available."),
            );
        }
        tray
    }

    /// Fades a newly mounted tray in on the reference open clock.
    ///
    /// Each hidden-to-shown mount carries a fresh animation identity from
    /// `tray_entrance_generation`, so the entrance replays every time the
    /// tray opens. Reduced motion paints the settled tray immediately.
    /// Styling finishes first as `Stateful<Div>`; the animated and plain
    /// branches converge here to `AnyElement` for the card boundary.
    fn animate_tray_entrance(
        &self,
        tray: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if cx.reduce_motion() {
            return tray.into_any_element();
        }
        let generation = self.tray_entrance_generation;
        tray.opacity(0.0)
            .with_animation(
                ElementId::Name(
                    format!("artisan-native-composer-tray-entrance-{generation}").into(),
                ),
                Animation::new(Duration::from_millis(COMPOSER_TRAY_MOTION_MS))
                    .with_easing(composer_smooth_out),
                move |tray, progress| tray.opacity(progress.clamp(0.0, 1.0)),
            )
            .into_any_element()
    }

    fn attachment_viewer(
        &self,
        entity: Entity<Self>,
        theme: DesktopTheme,
    ) -> Option<impl IntoElement> {
        let viewed_id = self.viewed_attachment.as_ref()?;
        let preview = self
            .attachment_preview
            .as_ref()
            .filter(|preview| {
                preview.attachment_id == *viewed_id
                    && preview.draft_generation == self.draft_generation
                    && preview.request == self.attachment_preview_request
            })
            .map(|preview| preview.image.clone());
        let preview_error = self.attachment_preview_error.clone();

        let dismiss_entity = entity.clone();
        let dismiss = div()
            .id("artisan-native-composer-attachment-viewer-dismiss")
            .absolute()
            .left(Pixels::ZERO)
            .top(Pixels::ZERO)
            .right(Pixels::ZERO)
            .bottom(Pixels::ZERO)
            .on_click(move |_, _, cx| {
                dismiss_entity.update(cx, |composer, composer_cx| {
                    composer.close_attachment_viewer(composer_cx);
                });
            });

        let close_entity = entity.clone();
        let close = div()
            .id("artisan-native-composer-attachment-viewer-close")
            .absolute()
            .top(px(8.0))
            .right(px(8.0))
            .size(px(28.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(theme.chrome.opacity(0.9))
            .text_color(theme.foreground)
            .cursor_pointer()
            .tab_index(0)
            .role(gpui::Role::Button)
            .aria_label("Close image preview")
            .debug_selector(|| "artisan-native-composer-attachment-viewer-close".to_owned())
            .on_click(move |_, _, cx| {
                close_entity.update(cx, |composer, composer_cx| {
                    composer.close_attachment_viewer(composer_cx);
                });
            })
            .child(asset_glyph(AssetId::TABLER_X).size(px(16.0)));

        let mut content = div()
            .id("artisan-native-composer-attachment-viewer-content")
            .relative()
            .max_w(px(960.0))
            .max_h(px(720.0))
            .flex()
            .items_center()
            .justify_center()
            .on_click(|_, _, cx| cx.stop_propagation());
        if let Some(preview) = preview {
            content = content.child(
                img(ImageSource::Render(preview))
                    .max_w(px(960.0))
                    .max_h(px(720.0))
                    .object_fit(ObjectFit::Contain),
            );
        } else if let Some(error) = preview_error {
            content = content
                .px(px(16.0))
                .py(px(12.0))
                .text_color(theme.secondary)
                .child(format!("Preview unavailable: {error}"));
        } else {
            content = content
                .px(px(16.0))
                .py(px(12.0))
                .text_color(theme.secondary)
                .child("Preparing preview…");
        }

        Some(
            div()
                .id(NATIVE_COMPOSER_ATTACHMENT_VIEWER_SELECTOR)
                .absolute()
                .left(Pixels::ZERO)
                .top(Pixels::ZERO)
                .right(Pixels::ZERO)
                .bottom(Pixels::ZERO)
                .flex()
                .items_center()
                .justify_center()
                .bg(theme.chrome.opacity(0.97))
                .occlude()
                .debug_selector(|| NATIVE_COMPOSER_ATTACHMENT_VIEWER_SELECTOR.to_owned())
                .child(dismiss)
                .child(content)
                .child(close),
        )
    }

    fn byte_index_for_global_point(&self, point: Point<Pixels>) -> Option<usize> {
        let bounds = self.painted_bounds.as_ref()?;
        // TextLayout positions are already relative to the window in GPUI's
        // prepaint pass. Keep the editor hit-test guard, but do not localize
        // the point a second time before asking the layout for its index.
        if !valid_bounds(bounds) || !valid_point(point) || !bounds.contains(&point) {
            return None;
        }
        let layout = self.layout.as_ref()?;
        let layout_bounds = layout.bounds();
        let index = if point.y < layout_bounds.top() {
            0
        } else if point.y >= layout_bounds.bottom() {
            self.state.draft().len()
        } else {
            match layout.index_for_position(point) {
                Ok(index) | Err(index) => index,
            }
        };
        (index <= self.state.draft().len())
            .then(|| previous_char_boundary(self.state.draft(), index))
    }

    fn byte_index_for_drag_point(&self, global_point: Point<Pixels>) -> Option<usize> {
        let bounds = self.painted_bounds.as_ref()?;
        if !valid_bounds(bounds) || !valid_point(global_point) {
            return None;
        }

        // Keep the endpoint inside the layout viewport while the pointer is
        // outside the field. This gives ordinary editor behavior at the top
        // and bottom edges without manufacturing an offset outside the text.
        let left = bounds.left();
        let top = bounds.top();
        let right = bounds.right();
        let bottom = bounds.bottom();
        let x = if global_point.x < left {
            left
        } else if global_point.x >= right {
            if right > left { right - px(0.1) } else { left }
        } else {
            global_point.x
        };
        let y = if global_point.y < top {
            top
        } else if global_point.y >= bottom {
            if bottom > top { bottom - px(0.1) } else { top }
        } else {
            global_point.y
        };
        let point = point(x, y);
        let layout = self.layout.as_ref()?;
        let layout_bounds = layout.bounds();
        let index = if point.y < layout_bounds.top() {
            0
        } else if point.y >= layout_bounds.bottom() {
            self.state.draft().len()
        } else {
            match layout.index_for_position(point) {
                Ok(index) | Err(index) => index,
            }
        };
        (index <= self.state.draft().len())
            .then(|| previous_char_boundary(self.state.draft(), index))
    }
}

impl Render for NativeComposer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.prune_attachment_tasks();
        let entity = cx.entity();
        let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
        let desktop_theme = DesktopTheme::neutral_dark();
        let draft = self.state.draft().to_owned();
        let styled_text = if self.selection.is_empty() {
            StyledText::new(SharedString::from(draft))
        } else {
            StyledText::new(SharedString::from(draft)).with_highlights([(
                self.selection.clone(),
                HighlightStyle {
                    color: Some(desktop_theme.foreground),
                    background_color: Some(desktop_theme.selected),
                    ..Default::default()
                },
            )])
        };
        self.painted_bounds = None;
        self.layout = Some(styled_text.layout().clone());

        let focus = self.focus_handle.clone();
        // Reference (`thread-composer.svelte:571-587`): `min-h-16 px-3 py-2
        // text-base`, uncapped with no internal scroll. Growth pushes the
        // absolute overlay taller while the transcript end space preserves
        // scroll-to-bottom (surface lane); no pixel cap lives here.
        let mut editor = div()
            .id("artisan-native-composer-editor")
            .debug_selector(|| NATIVE_COMPOSER_EDITOR_SELECTOR.to_string())
            .key_context(NATIVE_COMPOSER_KEY_CONTEXT)
            .w_full()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(64.0))
            .px(px(12.0))
            .py(px(8.0))
            .text_color(desktop_theme.foreground)
            .text_size(px(16.0))
            .line_height(px(24.0))
            .font_weight(ProseTypography::BODY_WEIGHT)
            .letter_spacing(px(ProseTypography::body_tracking_px(16.0)))
            .whitespace_normal()
            .track_focus(&focus)
            .child(styled_text);

        // Reference visibility (`thread-composer.svelte:180-187,559`): the
        // placeholder shows only while the composed value is empty, where an
        // attachment counts as content. Each fresh reveal walks the reference
        // vocabulary (`composer-placeholder.ts:54-67`). The per-character
        // `placeholder-reveal-in` keyframes are dead in the reference CSS (no
        // rule applies them), so the phrase paints statically.
        let placeholder_visible =
            self.state.draft().is_empty() && self.attachments.is_empty();
        if placeholder_visible && !self.placeholder_was_visible {
            self.placeholder_generation = self.placeholder_generation.wrapping_add(1);
        }
        self.placeholder_was_visible = placeholder_visible;
        if placeholder_visible {
            let phrase = composer_placeholder_phrase(self.placeholder_generation);
            editor = editor.child(
                div()
                    .absolute()
                    .top(px(8.0))
                    .left(px(12.0))
                    .text_color(desktop_theme.secondary)
                    .text_size(px(16.0))
                    .line_height(px(24.0))
                    .whitespace_normal()
                    .debug_selector(|| NATIVE_COMPOSER_PLACEHOLDER_SELECTOR.to_string())
                    .child(phrase),
            );
        }

        editor = editor
            .on_action(cx.listener(Self::undo_action))
            .on_action(cx.listener(Self::redo_action))
            .on_action(cx.listener(Self::delete_backward))
            .on_action(cx.listener(Self::delete_forward))
            .on_action(cx.listener(Self::move_left_action))
            .on_action(cx.listener(Self::move_right_action))
            .on_action(cx.listener(Self::select_left_action))
            .on_action(cx.listener(Self::select_right_action))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::move_home))
            .on_action(cx.listener(Self::move_end))
            .on_action(cx.listener(Self::move_up_action))
            .on_action(cx.listener(Self::move_down_action))
            .on_action(cx.listener(Self::select_home_action))
            .on_action(cx.listener(Self::select_end_action))
            .on_action(cx.listener(Self::select_up_action))
            .on_action(cx.listener(Self::select_down_action))
            .on_action(cx.listener(Self::move_document_home_action))
            .on_action(cx.listener(Self::move_document_end_action))
            .on_action(cx.listener(Self::select_document_home_action))
            .on_action(cx.listener(Self::select_document_end_action))
            .on_action(cx.listener(Self::request_send_action))
            .on_action(cx.listener(Self::insert_newline))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut));

        let editor =
            NativeComposerInputElement::new(editor.into_any_element(), entity.clone(), focus);
        let mounted_controls = self.controls.clone();
        let mounted_model_selector = self.model_selector.clone();
        let (controls_lip, controls_failure, controls_failed, controls_row, jump_to_latest) = if let Some((
            controls,
            model_selector,
        )) =
            mounted_controls.zip(mounted_model_selector)
        {
            let controls_lip = controls.update(cx, |controls, controls_cx| {
                controls.render_lip(theme, controls_cx)
            });
            let controls_failure = controls.update(cx, |controls, controls_cx| {
                controls.render_failure(theme, controls_cx)
            });
            let controls_failed = controls.update(cx, |controls, controls_cx| {
                controls.render_failed_dispatches(theme, controls_cx)
            });
            let controls_row = controls.update(cx, |controls, controls_cx| {
                controls.render_control_row(theme, model_selector.clone(), controls_cx)
            });
            let jump_to_latest = controls.update(cx, |controls, controls_cx| {
                controls.render_jump_to_latest(theme, controls_cx)
            });
            (
                controls_lip,
                controls_failure,
                controls_failed,
                Some(controls_row),
                jump_to_latest,
            )
        } else {
            (None, None, None, None, None)
        };

        let legacy_toolbar = if controls_row.is_none() {
            let send_ready = self.send_ready();
            self.send_focus_handle = self.send_focus_handle.clone().tab_stop(send_ready);
            let send_entity = entity.clone();
            let send = Button::new(
                NATIVE_COMPOSER_SEND_SELECTOR,
                self.send_focus_handle.clone(),
                theme,
                MotionPolicy::Reduced,
                ButtonVariant::Default,
                ButtonSize::IconSmall,
                ButtonContent::icon_only(
                    AssetId::TABLER_ARROW_UP,
                    AccessibleLabel::new("Send message").expect("nonempty send label"),
                ),
            )
            .expect("the native composer send button configuration is valid")
            .focus_visibility(FocusVisibility::Visible)
            .corner_radius(px(10.0))
            .disabled(!send_ready)
            .debug_selector(NATIVE_COMPOSER_SEND_SELECTOR)
            .on_activate(move |_, _, cx| {
                send_entity.update(cx, NativeComposer::request_send);
            });

            let model = div()
                .id("artisan-composer-model")
                .track_focus(&self.model_focus_handle)
                .tab_index(0)
                .h(px(32.0))
                .min_w(px(0.0))
                .px(px(8.0))
                .flex()
                .items_center()
                .gap(px(6.0))
                .rounded(px(10.0))
                .cursor_pointer()
                .hover(move |style| style.bg(desktop_theme.selected))
                .text_color(desktop_theme.secondary)
                .text_size(px(13.0))
                .debug_selector(|| "artisan-composer-model".to_owned())
                .child(div().truncate().child(self.model_label.clone()))
                .child(asset_glyph(AssetId::TABLER_CHEVRON_DOWN).size(px(14.0)))
                .on_click(cx.listener(|_, _, _, cx| cx.emit(NativeComposerEvent::ConfigureModel)))
                .on_key_down(cx.listener(|_, event: &gpui::KeyDownEvent, _, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        cx.stop_propagation();
                        cx.emit(NativeComposerEvent::ConfigureModel);
                    }
                }));

            Some(
                div()
                    .w_full()
                    .flex()
                    .h(px(32.0))
                    .flex_shrink_0()
                    .items_center()
                    .justify_between()
                    .child(model)
                    .child(send),
            )
        } else {
            None
        };

        let drop_entity = entity.clone();
        // Reference (`thread-composer.svelte:553`): `flex min-h-32 flex-col
        // p-2`. No gaps: the tray carries its own open padding, the editor
        // its own py, and the control row sits directly below.
        let mut root = div()
            .id("artisan-native-composer")
            .debug_selector(|| "artisan-native-composer".to_owned())
            .w_full()
            .flex()
            .flex_col()
            .min_h(px(128.0))
            .p(px(8.0))
            .rounded(px(18.0))
            .backdrop_blur(glass_blur_radius(GlassStrength::Quiet))
            .bg(glass_foreground_base(theme))
            .shadow(glass_card_shadows())
            .relative()
            .child(glass_material_layer(GlassStrength::Quiet, px(18.0)))
            .child(glass_highlight_layer(GlassStrength::Quiet, px(18.0)))
            .on_drop::<ExternalPaths>(move |paths, _, cx| {
                let paths = paths.paths().to_vec();
                drop_entity.update(cx, |composer, composer_cx| {
                    composer.enqueue_file_drop(paths, composer_cx);
                });
            });

        if !self.attachments.is_empty() {
            // Reference open motion (`attachment-tray.svelte:25`): the tray
            // fades in on `--composer-resize-dur` (300ms). The grid-track
            // height tween has no GPUI primitive (see the lane report), so
            // only the opacity half is reproduced. Close unmounts
            // immediately; a fade-out would need a retained tile snapshot
            // plus a settle timer for zero visual gain on a surface the user
            // just dismissed.
            if !self.tray_was_open {
                self.tray_entrance_generation = self.tray_entrance_generation.wrapping_add(1);
            }
            self.tray_was_open = true;
            let tray = self.attachment_tray(entity.clone(), theme, desktop_theme);
            root = root.child(self.animate_tray_entrance(tray, cx));
        } else {
            self.tray_was_open = false;
            if let Some(error) = self.attachment_error.clone() {
                root = root.child(
                    div()
                        .id(NATIVE_COMPOSER_ATTACHMENT_BLOCKED_SELECTOR)
                        .text_color(desktop_theme.secondary)
                        .text_size(px(12.0))
                        .debug_selector(|| NATIVE_COMPOSER_ATTACHMENT_BLOCKED_SELECTOR.to_owned())
                        .child(error),
                );
            }
        }
        root = root.child(editor);
        if let Some(controls_row) = controls_row {
            root = root.child(controls_row);
        } else if let Some(legacy_toolbar) = legacy_toolbar {
            root = root.child(legacy_toolbar);
        }
        if let Some(viewer) = self.attachment_viewer(entity, desktop_theme) {
            root = root.child(viewer);
        }
        // Reference (`thread-composer.svelte:526-543`): jump, failure, and
        // the queued lip are siblings above the composer card, spaced by the
        // frame's `gap-2`. The card holds only tray, editor, and controls.
        let mut shell = div().w_full().flex().flex_col().gap(px(8.0));
        if let Some(jump_to_latest) = jump_to_latest {
            shell = shell.child(jump_to_latest);
        }
        if let Some(failure) = controls_failure {
            shell = shell.child(failure);
        }
        if let Some(failed) = controls_failed {
            shell = shell.child(failed);
        }
        if let Some(lip) = controls_lip {
            shell = shell.child(lip);
        }
        shell.child(root)
    }
}

/// An element wrapper that registers the entity input handler in paint.
struct NativeComposerInputElement {
    child: AnyElement,
    view: Entity<NativeComposer>,
    focus_handle: FocusHandle,
}

impl NativeComposerInputElement {
    fn new(child: AnyElement, view: Entity<NativeComposer>, focus_handle: FocusHandle) -> Self {
        Self {
            child,
            view,
            focus_handle,
        }
    }
}

impl Element for NativeComposerInputElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.handle_input(
            &self.focus_handle,
            ElementInputHandler::new(bounds, self.view.clone()),
            cx,
        );

        window.on_mouse_event({
            let view = self.view.clone();
            let focus_handle = self.focus_handle.clone();
            let bounds = bounds.clone();
            move |event: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble
                    || event.button != MouseButton::Left
                    || !bounds.contains(&event.position)
                {
                    return;
                }

                cx.stop_propagation();
                window.focus(&focus_handle, cx);
                view.update(cx, |composer, composer_cx| {
                    composer.begin_selection_drag(
                        event.position,
                        event.modifiers.shift,
                        composer_cx,
                    );
                });
            }
        });
        window.on_mouse_event({
            let view = self.view.clone();
            move |event: &MouseMoveEvent, phase, _window, cx| {
                if phase != DispatchPhase::Bubble || !event.dragging() {
                    return;
                }

                let is_dragging = view.read(cx).selection_dragging;
                if is_dragging {
                    view.update(cx, |composer, composer_cx| {
                        composer.update_selection_drag(event.position, composer_cx);
                    });
                }
            }
        });
        window.on_mouse_event({
            let view = self.view.clone();
            move |event: &MouseUpEvent, phase, _window, cx| {
                if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                    return;
                }

                if view.read(cx).selection_dragging {
                    view.update(cx, |composer, _| composer.end_selection_drag());
                }
            }
        });
        self.child.paint(window, cx);
        if let Some(caret) = self.view.read(cx).caret_quad(window) {
            window.paint_quad(caret);
        }
        self.view.update(cx, |composer, _| {
            composer.painted_bounds = valid_bounds(&bounds).then_some(bounds);
        });
    }
}

impl IntoElement for NativeComposerInputElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl gpui::EntityInputHandler for NativeComposer {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let utf8_range = utf16_range_to_utf8(self.state.draft(), range.clone())?;
        *adjusted_range = Some(range);
        Some(self.state.draft()[utf8_range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        if self.state.is_disabled() && !ignore_disabled_input {
            return None;
        }
        let draft = self.state.draft();
        let selection = self.current_selection();
        Some(UTF16Selection {
            range: utf8_range_to_utf16(draft, selection)?,
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .clone()
            .and_then(|range| utf8_range_to_utf16(self.state.draft(), range))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.marked_range.take().is_some() {
            self.advance_selection_revision();
            cx.notify();
        }
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(range) = self.replacement_range(range) else {
            return;
        };
        self.replace_range(range, text, None, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(range) = self.replacement_range(range) else {
            return;
        };
        self.replace_range(range, new_text, new_selected_range, cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let painted_bounds = self.painted_bounds.as_ref()?;
        if painted_bounds != &element_bounds || !valid_bounds(painted_bounds) {
            return None;
        }
        let draft = self.state.draft();
        let range = utf16_range_to_utf8(draft, range_utf16)?;
        let layout = self.layout.as_ref()?;
        let start = layout.position_for_index(range.start)?;
        let end = layout.position_for_index(range.end)?;
        let width = (end.x - start.x).max(px(1.0));
        let bounds = Bounds::new(start, size(width, layout.line_height()));
        valid_bounds(&bounds).then_some(bounds)
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let draft = self.state.draft();
        let byte_index = self.byte_index_for_global_point(point)?;
        utf8_offset_to_utf16(draft, byte_index)
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
fn caret_offset_for_paint(
    focused: bool,
    selection: &Range<usize>,
    draft: &str,
) -> Option<usize> {
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
mod tests {
    use std::{cell::Cell, rc::Rc};

    use super::native_composer_attachments::ComposerAttachment;
    use super::{
        DocumentEnd, DocumentHome, NATIVE_COMPOSER_ATTACHMENT_BLOCKED_SELECTOR,
        NATIVE_COMPOSER_ATTACHMENT_TRAY_SELECTOR, NATIVE_COMPOSER_EDITOR_SELECTOR,
        NATIVE_COMPOSER_PLACEHOLDER_SELECTOR,
        NATIVE_COMPOSER_SEND_SELECTOR, NativeComposer, NativeComposerEvent, SelectDocumentEnd,
        SelectDocumentHome, SelectEnd, SelectHome, localize_painted_point, logical_vertical_target,
        offset_layout_bounds, replace_text_preserving_raw, utf8_offset_to_utf16,
        utf16_offset_to_utf8, utf16_range_to_utf8, caret_offset_for_paint,
    };
    use crate::native_composer_visuals::composer_placeholder_phrase;
    use crate::composer::DraftDisposition;
    use crate::image_policy::{ImageDimensions, ImageMediaType};
    use artisan_domain::{AuthoredText, ImageAttachment, QueueMessagePayload};
    use artisan_ui::button::{Button, ButtonContent, ButtonSize, ButtonVariant, FocusVisibility};
    use artisan_ui::motion::MotionPolicy;
    use artisan_ui::theme::{ArtisanTheme, ThemeMode};
    use base64::Engine as _;
    use gpui::{
        Bounds, Entity, EntityInputHandler as _, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers,
        Subscription, Task, TestAppContext, VisualTestContext, point, px, size,
    };
    use std::sync::Arc;

    fn ready_attachment(id: &str, bytes: &[u8]) -> ComposerAttachment {
        let bytes = Arc::new(bytes.to_vec());
        ComposerAttachment {
            id: id.to_owned(),
            name: format!("{id}.png"),
            format: Some(gpui::ImageFormat::Png),
            mime_type: ImageMediaType::Png.as_mime_type().to_owned(),
            bytes: Some(bytes.clone()),
            content_base64: base64::engine::general_purpose::STANDARD.encode(bytes.as_ref()),
            thumbnail: Some(Arc::new(gpui::RenderImage::new(Vec::<image::Frame>::new()))),
            dimensions: Some(ImageDimensions {
                width: 1.0,
                height: 1.0,
            }),
            recommended_media_type: Some(ImageMediaType::Png),
            rescale_target: None,
            source_digest: "source-digest".to_owned(),
            encoded_digest: "encoded-digest".to_owned(),
            source_size_bytes: bytes.len(),
            size_bytes: bytes.len(),
        }
    }

    const RECALL_PRIMARY_PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8,
        0xcf, 0xc0, 0xf0, 0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x72, 0x9c, 0x52, 0x67, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    const RECALL_SECONDARY_PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

    fn recalled_image_payload(text: Option<AuthoredText>) -> QueueMessagePayload {
        let secondary = base64::engine::general_purpose::STANDARD
            .decode(RECALL_SECONDARY_PNG_BASE64)
            .expect("secondary PNG fixture");
        QueueMessagePayload::new(
            text,
            vec![
                ImageAttachment::new("image/png", RECALL_PRIMARY_PNG.to_vec(), "first.png")
                    .expect("primary image"),
                ImageAttachment::new("image/png", secondary, "second.png")
                    .expect("secondary image"),
            ],
        )
        .expect("recall payload")
    }

    fn set_draft(cx: &mut VisualTestContext, view: &Entity<NativeComposer>, draft: &str) {
        let draft = draft.to_owned();
        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.set_draft(draft);
                composer_cx.notify();
            });
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn draft_body_match_is_exact_and_does_not_change_editor_state(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        let draft = "  exact\n😀  ";
        set_draft(cx, &view, draft);

        cx.update(|_, app| {
            view.update(app, |composer, _| {
                composer.selection = 2..7;
                composer.selection_reversed = true;
                composer.marked_range = Some(2..7);
                let selection = composer.selection.clone();
                let selection_reversed = composer.selection_reversed;
                let marked_range = composer.marked_range.clone();
                let layout_present = composer.layout.is_some();
                let painted_bounds_present = composer.painted_bounds.is_some();
                let submitting = composer.is_submitting();
                let body =
                    artisan_domain::MessageBody::parse(draft.to_owned()).expect("draft body");
                let changed = artisan_domain::MessageBody::parse("  exact\n😀  !".to_owned())
                    .expect("changed body");

                assert!(composer.draft_matches_body(&body));
                assert!(!composer.draft_matches_body(&changed));
                assert_eq!(composer.selection, selection);
                assert_eq!(composer.selection_reversed, selection_reversed);
                assert_eq!(composer.marked_range, marked_range);
                assert_eq!(composer.layout.is_some(), layout_present);
                assert_eq!(composer.painted_bounds.is_some(), painted_bounds_present);
                assert_eq!(composer.is_submitting(), submitting);
            });
        });
    }

    #[test]
    fn caret_paints_focused_collapsed_only_at_valid_boundaries() {
        // Empty field, focused: visible at the origin.
        assert_eq!(caret_offset_for_paint(true, &(0..0), ""), Some(0));
        // Collapsed selection follows movement through multibyte text.
        let draft = "a😀b";
        assert_eq!(caret_offset_for_paint(true, &(1..1), draft), Some(1));
        assert_eq!(caret_offset_for_paint(true, &(5..5), draft), Some(5));
        assert_eq!(caret_offset_for_paint(true, &(6..6), draft), Some(6));
        // Mid-character offsets never place the caret inside a character.
        assert_eq!(caret_offset_for_paint(true, &(2..2), draft), None);
        // A selection hides the caret in favor of the highlight.
        assert_eq!(caret_offset_for_paint(true, &(0..1), draft), None);
        assert_eq!(caret_offset_for_paint(true, &(1..5), draft), None);
        // Blur hides the caret even for a collapsed selection.
        assert_eq!(caret_offset_for_paint(false, &(0..0), ""), None);
        assert_eq!(caret_offset_for_paint(false, &(1..1), draft), None);
        // Stale offsets past the draft hide rather than misplace.
        assert_eq!(caret_offset_for_paint(true, &(7..7), draft), None);
    }

    fn bind_actions(cx: &mut VisualTestContext) {
        cx.update(|_, app| NativeComposer::bind_actions(app));
    }

    #[gpui::test]
    fn mounted_caret_paints_focused_geometry_and_hides_on_blur_or_selection(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        cx.simulate_resize(size(px(900.0), px(600.0)));
        focus_editor(cx, &view);
        // Focused empty field: the caret paints at the text origin. Its
        // height is the reference `text-base` leading (24px), matching the
        // editor's explicit line height.
        set_draft(cx, &view, "");
        let empty = cx.update(|window, app| {
            view.read(app)
                .caret_quad(window)
                .expect("focused empty caret")
        });
        assert_eq!(empty.bounds.size, size(px(2.0), px(24.0)));
        let editor = cx
            .debug_bounds(NATIVE_COMPOSER_EDITOR_SELECTOR)
            .expect("editor bounds");
        assert!(editor.contains(&empty.bounds.origin));
        // A collapsed multibyte selection follows typing.
        set_draft(cx, &view, "a😀b");
        let end = cx.update(|window, app| {
            view.read(app)
                .caret_quad(window)
                .expect("focused end caret")
        });
        assert_eq!(end.bounds.size, size(px(2.0), px(24.0)));
        assert!(
            end.bounds.origin.x > empty.bounds.origin.x,
            "caret must advance past typed text"
        );
        assert!(editor.contains(&end.bounds.origin));
        // A selection hides the caret in favor of the highlight.
        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.selection = 0..1;
                composer.selection_reversed = false;
                composer_cx.notify();
            });
        });
        cx.run_until_parked();
        assert!(
            cx.update(|window, app| view.read(app).caret_quad(window))
                .is_none()
        );
        // Blur hides the caret even for a collapsed selection.
        cx.update(|window, app| {
            view.update(app, |composer, composer_cx| {
                composer.selection = 6..6;
                composer.selection_reversed = false;
                composer_cx.notify();
            });
            let send = view.read(app).send_focus_handle.clone();
            window.focus(&send, app);
        });
        cx.run_until_parked();
        assert!(
            cx.update(|window, app| view.read(app).caret_quad(window))
                .is_none()
        );
    }

    fn focus_editor(cx: &mut VisualTestContext, view: &Entity<NativeComposer>) {
        cx.update(|window, app| {
            let focus = view.read(app).focus_handle.clone();
            window.focus(&focus, app);
        });
        cx.run_until_parked();
    }

    fn observe_send_requests(
        cx: &mut VisualTestContext,
        view: &Entity<NativeComposer>,
    ) -> (Rc<Cell<usize>>, Subscription) {
        let requests = Rc::new(Cell::new(0));
        let observed_requests = requests.clone();
        let subscription = cx.update(|_, app| {
            app.subscribe(view, move |_, event: &NativeComposerEvent, _| {
                if *event == NativeComposerEvent::SendRequested {
                    observed_requests.set(observed_requests.get() + 1);
                }
            })
        });
        cx.run_until_parked();
        (requests, subscription)
    }

    fn send_button(focus: gpui::FocusHandle) -> Button {
        Button::new(
            NATIVE_COMPOSER_SEND_SELECTOR,
            focus,
            ArtisanTheme::for_mode(ThemeMode::Dark),
            MotionPolicy::Reduced,
            ButtonVariant::Ghost,
            ButtonSize::Small,
            ButtonContent::text("Send"),
        )
        .expect("the native composer send button configuration is valid")
        .focus_visibility(FocusVisibility::Visible)
    }

    #[gpui::test]
    fn plain_enter_requests_send_and_shift_enter_inserts_a_newline(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        set_draft(cx, &view, "draft");
        bind_actions(cx);
        focus_editor(cx, &view);
        let (requests, _subscription) = observe_send_requests(cx, &view);

        cx.simulate_keystrokes("enter");
        cx.update(|_, app| assert_eq!(view.read(app).draft(), "draft"));
        assert_eq!(requests.get(), 1);

        cx.simulate_keystrokes("shift-enter");
        cx.update(|_, app| assert_eq!(view.read(app).draft(), "draft\n"));
        assert_eq!(requests.get(), 1);
    }

    #[gpui::test]
    fn modified_and_unready_enter_requests_are_refused(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        bind_actions(cx);
        focus_editor(cx, &view);
        let (requests, _subscription) = observe_send_requests(cx, &view);

        set_draft(cx, &view, "draft");
        cx.simulate_keystrokes("ctrl-enter alt-enter cmd-enter");
        cx.update(|_, app| assert_eq!(view.read(app).draft(), "draft"));
        assert_eq!(requests.get(), 0);

        for draft in ["", " \t\n"] {
            set_draft(cx, &view, draft);
            cx.simulate_keystrokes("enter");
            assert_eq!(requests.get(), 0);
        }

        set_draft(cx, &view, "disabled");
        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.set_disabled(true, composer_cx);
            });
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        assert_eq!(requests.get(), 0);

        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.set_disabled(false, composer_cx);
                composer.set_draft("in flight");
                assert!(composer.begin_submission().is_ok());
                composer_cx.notify();
            });
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("enter");
        assert_eq!(requests.get(), 0);
        cx.update(|_, app| assert!(view.read(app).is_submitting()));
    }

    #[gpui::test]
    fn marked_ime_text_refuses_enter_until_composition_is_unmarked(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        bind_actions(cx);
        focus_editor(cx, &view);
        let (requests, _subscription) = observe_send_requests(cx, &view);

        cx.update(|window, app| {
            view.update(app, |composer, composer_cx| {
                composer.replace_and_mark_text_in_range(
                    Some(0..0),
                    "preedit",
                    Some(0..7),
                    window,
                    composer_cx,
                );
            });
        });
        cx.run_until_parked();
        cx.update(|_, app| {
            let composer = view.read(app);
            assert_eq!(composer.draft(), "preedit");
            assert!(composer.marked_range.is_some());
            assert!(!composer.send_ready());
        });

        cx.simulate_keystrokes("enter");
        assert_eq!(requests.get(), 0);

        cx.update(|window, app| {
            view.update(app, |composer, composer_cx| {
                composer.unmark_text(window, composer_cx);
            });
        });
        cx.run_until_parked();
        cx.update(|_, app| assert!(view.read(app).send_ready()));

        cx.simulate_keystrokes("enter");
        assert_eq!(requests.get(), 1);
    }

    #[gpui::test]
    fn empty_ime_mark_is_still_a_composition_and_cannot_submit(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        bind_actions(cx);
        focus_editor(cx, &view);
        set_draft(cx, &view, "draft");
        let (requests, _subscription) = observe_send_requests(cx, &view);

        cx.update(|window, app| {
            view.update(app, |composer, composer_cx| {
                composer.replace_and_mark_text_in_range(
                    Some(0..0),
                    "",
                    Some(0..0),
                    window,
                    composer_cx,
                );
                assert!(
                    composer
                        .marked_range
                        .as_ref()
                        .is_some_and(|range| range.is_empty())
                );
                assert!(!composer.send_ready());
            });
        });
        cx.simulate_keystrokes("enter");
        assert_eq!(requests.get(), 0);
    }

    #[gpui::test]
    fn multiline_selection_actions_keep_an_anchor_when_crossing_zero(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        cx.update(|window, app| {
            view.update(app, |composer, composer_cx| {
                composer.set_draft("one\ntwo");
                composer.move_to(5, composer_cx);
                composer.select_home_action(&SelectHome, window, composer_cx);
                assert_eq!(composer.selection, 4..5);
                assert!(composer.selection_reversed);

                composer.select_end_action(&SelectEnd, window, composer_cx);
                assert_eq!(composer.selection, 5..7);
                assert!(!composer.selection_reversed);

                composer.move_to(2, composer_cx);
                composer.move_right(true, composer_cx);
                composer.move_left(true, composer_cx);
                composer.move_left(true, composer_cx);
                assert_eq!(composer.selection, 1..2);
                assert!(composer.selection_reversed);
            });
        });
    }

    #[gpui::test]
    fn document_selection_actions_cover_ctrl_home_end_targets(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        cx.update(|window, app| {
            view.update(app, |composer, composer_cx| {
                composer.set_draft("one\ntwo");
                composer.move_to(5, composer_cx);
                composer.select_document_home_action(&SelectDocumentHome, window, composer_cx);
                assert_eq!(composer.selection, 0..5);
                assert!(composer.selection_reversed);

                composer.select_document_end_action(&SelectDocumentEnd, window, composer_cx);
                assert_eq!(composer.selection, 5..7);
                assert!(!composer.selection_reversed);

                composer.move_document_home_action(&DocumentHome, window, composer_cx);
                assert_eq!(composer.selection, 0..0);
                composer.move_document_end_action(&DocumentEnd, window, composer_cx);
                assert_eq!(composer.selection, 7..7);
            });
        });
    }

    #[gpui::test]
    fn multiline_key_bindings_drive_navigation_and_selection(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        set_draft(cx, &view, "one\ntwo");
        bind_actions(cx);
        focus_editor(cx, &view);

        cx.simulate_keystrokes("ctrl-home");
        cx.update(|_, app| assert_eq!(view.read(app).selection, 0..0));

        cx.simulate_keystrokes("down");
        cx.update(|_, app| assert_eq!(view.read(app).selection, 4..4));

        cx.simulate_keystrokes("shift-end");
        cx.update(|_, app| {
            let composer = view.read(app);
            assert_eq!(composer.selection, 4..7);
            assert!(!composer.selection_reversed);
        });

        cx.simulate_keystrokes("ctrl-shift-home");
        cx.update(|_, app| {
            let composer = view.read(app);
            assert_eq!(composer.selection, 0..4);
            assert!(composer.selection_reversed);
        });

        cx.simulate_keystrokes("ctrl-end");
        cx.update(|_, app| assert_eq!(view.read(app).selection, 7..7));
    }

    #[test]
    fn logical_vertical_navigation_preserves_column_across_short_lines_and_unicode() {
        let text = "abcd\nx\nab😀d";
        assert_eq!(logical_vertical_target(text, 4, 1, 4), 6);
        assert_eq!(logical_vertical_target(text, 6, 1, 4), 13);
        assert_eq!(logical_vertical_target(text, 13, -1, 4), 6);

        let unicode = "x\n😀";
        // UTF-16 column 1 falls inside the surrogate pair and is clamped to
        // the preceding valid boundary rather than splitting the character.
        assert_eq!(logical_vertical_target(unicode, 1, 1, 1), 2);
    }

    #[gpui::test]
    fn completed_attachment_task_handles_are_pruned_without_touching_live_work(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        let live_task = cx.spawn(|_| std::future::pending::<()>());
        cx.update(|_, app| {
            view.update(app, |composer, _| {
                composer.attachment_tasks.push(live_task);
                composer.attachment_tasks.push(Task::ready(()));
                composer.attachment_tasks.push(Task::ready(()));
                composer.prune_attachment_tasks();
                assert_eq!(composer.attachment_tasks.len(), 1);
                assert!(!composer.attachment_tasks[0].is_ready());
            });
        });
    }

    #[gpui::test]
    fn thread_drafts_restore_without_cross_thread_undo(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        cx.update(|_, app| {
            view.update(app, |composer, cx| {
                composer.switch_thread("one".into(), false, cx);
                composer.replace_range(0..0, "first draft", None, cx);
                composer.switch_thread("two".into(), false, cx);
                assert_eq!(composer.state.draft(), "");
                assert!(composer.undo.is_empty());
                composer.replace_range(0..0, "second draft", None, cx);
                composer.switch_thread("one".into(), false, cx);
                assert_eq!(composer.state.draft(), "first draft");
                composer.switch_thread("two".into(), false, cx);
                assert_eq!(composer.state.draft(), "second draft");
            });
        });
    }

    #[gpui::test]
    fn attachment_snapshot_preserves_order_and_rejects_a_new_thread_generation(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        let snapshot = cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.switch_thread("one".into(), false, composer_cx);
                composer
                    .attachments
                    .push(ready_attachment("first", &[1, 2]));
                composer
                    .attachments
                    .push(ready_attachment("second", &[3, 4]));
                composer_cx.notify();
                let snapshot = composer
                    .snapshot_ordered_ready_attachments()
                    .expect("ready attachment snapshot");
                assert_eq!(snapshot.attachments[0].position, 0);
                assert_eq!(snapshot.attachments[1].position, 1);
                assert_eq!(
                    composer.match_retry_attachment_payload(&snapshot),
                    Some(snapshot.clone())
                );
                composer.switch_thread("two".into(), false, composer_cx);
                assert!(composer.match_retry_attachment_payload(&snapshot).is_none());
                snapshot
            })
        });
        assert_eq!(snapshot.attachments.len(), 2);
    }

    #[gpui::test]
    fn recalled_text_restores_into_the_exact_empty_thread(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        let payload = QueueMessagePayload::text_only("queued\nmessage").expect("text payload");
        let target = cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.switch_thread("recall-text".into(), false, composer_cx);
                composer
                    .capture_recall_target()
                    .expect("real empty thread is recallable")
            })
        });

        let result = cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.restore_recalled_payload(&target, payload.clone(), composer_cx)
            })
        });
        assert_eq!(result, Ok(()));
        cx.update(|_, app| {
            let composer = view.read(app);
            assert_eq!(composer.draft(), "queued\nmessage");
            assert_eq!(composer.attachment_count(), 0);
            assert!(composer.draft_matches_payload(&payload));
        });
    }

    #[gpui::test]
    fn recall_race_returns_the_full_payload_after_typing_without_overwrite(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        let payload = QueueMessagePayload::text_only("queued message").expect("text payload");
        let target = cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.switch_thread("recall-typing".into(), false, composer_cx);
                composer
                    .capture_recall_target()
                    .expect("real empty thread is recallable")
            })
        });

        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.replace_range(0..0, "user started typing", None, composer_cx);
                assert!(composer.capture_recall_target().is_none());
            });
        });
        let result = cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.restore_recalled_payload(&target, payload.clone(), composer_cx)
            })
        });
        assert_eq!(result, Err(payload));
        cx.update(|_, app| {
            let composer = view.read(app);
            assert_eq!(composer.draft(), "user started typing");
            assert_eq!(composer.attachment_count(), 0);
        });
    }

    #[gpui::test]
    fn recall_race_returns_the_full_payload_after_thread_change(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        let payload = QueueMessagePayload::text_only("queued message").expect("text payload");
        let target = cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.switch_thread("recall-old-thread".into(), false, composer_cx);
                composer
                    .capture_recall_target()
                    .expect("real empty thread is recallable")
            })
        });
        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.switch_thread("recall-new-thread".into(), false, composer_cx);
            });
        });

        let result = cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.restore_recalled_payload(&target, payload.clone(), composer_cx)
            })
        });
        assert_eq!(result, Err(payload));
        cx.update(|_, app| {
            let composer = view.read(app);
            assert_eq!(composer.draft(), "");
            assert_eq!(composer.attachment_count(), 0);
        });
    }

    #[gpui::test]
    fn existing_draft_never_offers_a_recall_target(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.switch_thread("recall-existing".into(), false, composer_cx);
                composer.replace_range(0..0, "keep this draft", None, composer_cx);
                assert!(composer.capture_recall_target().is_none());
                assert_eq!(composer.draft(), "keep this draft");
            });
        });
    }

    #[gpui::test]
    fn image_only_recall_preserves_absent_text_order_and_exact_bytes(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        let payload = recalled_image_payload(None);
        let expected = payload
            .attachments()
            .iter()
            .map(|attachment| (attachment.name().to_owned(), attachment.bytes().to_vec()))
            .collect::<Vec<_>>();
        let target = cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.switch_thread("recall-images".into(), false, composer_cx);
                composer
                    .capture_recall_target()
                    .expect("real empty thread is recallable")
            })
        });
        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer
                    .restore_recalled_payload(&target, payload, composer_cx)
                    .expect("valid queued image payload restores");
            });
        });
        cx.run_until_parked();

        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.set_attachment_delivery_enabled(true, composer_cx);
                assert_eq!(composer.attachments.len(), expected.len());
                assert!(
                    composer
                        .attachments
                        .iter()
                        .all(ComposerAttachment::is_ready)
                );
                let (restored, token) = composer
                    .begin_payload_submission()
                    .expect("prepared recalled images submit");
                assert!(restored.text().is_none());
                assert_eq!(
                    restored
                        .attachments()
                        .iter()
                        .map(|attachment| {
                            (attachment.name().to_owned(), attachment.bytes().to_vec())
                        })
                        .collect::<Vec<_>>(),
                    expected
                );
                composer.finish_submission(token, DraftDisposition::Retained, composer_cx);
            });
        });
    }

    #[gpui::test]
    fn image_only_recall_preserves_present_empty_text(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        let payload = recalled_image_payload(Some(AuthoredText::empty()));
        let target = cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.switch_thread("recall-empty-text".into(), false, composer_cx);
                composer
                    .capture_recall_target()
                    .expect("real empty thread is recallable")
            })
        });
        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer
                    .restore_recalled_payload(&target, payload, composer_cx)
                    .expect("valid queued image payload restores");
            });
        });
        cx.run_until_parked();

        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.set_attachment_delivery_enabled(true, composer_cx);
                let (restored, token) = composer
                    .begin_payload_submission()
                    .expect("prepared recalled images submit");
                assert_eq!(restored.text().map(AuthoredText::as_str), Some(""));
                composer.finish_submission(token, DraftDisposition::Retained, composer_cx);
            });
        });
    }

    #[gpui::test]
    fn recalled_attachment_work_is_fenced_after_thread_change(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        let payload = recalled_image_payload(None);
        let target = cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.switch_thread("recall-pending".into(), false, composer_cx);
                composer
                    .capture_recall_target()
                    .expect("real empty thread is recallable")
            })
        });
        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer
                    .restore_recalled_payload(&target, payload.clone(), composer_cx)
                    .expect("valid queued image payload restores");
            });
        });
        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.switch_thread("recall-after-switch".into(), false, composer_cx);
            });
        });
        cx.run_until_parked();

        let result = cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                assert_eq!(composer.draft(), "");
                assert_eq!(composer.attachment_count(), 0);
                composer.restore_recalled_payload(&target, payload, composer_cx)
            })
        });
        assert!(result.is_err());
    }

    #[gpui::test]
    fn image_only_typed_payload_has_no_placeholder_and_cleans_after_acceptance(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        let token = cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer
                    .attachments
                    .push(ready_attachment("image", &[1, 2, 3]));
                composer.set_attachment_delivery_enabled(true, composer_cx);
                let (payload, token) = composer
                    .begin_payload_submission()
                    .expect("typed image-only payload begins");
                assert_eq!(payload.text().expect("authored text").as_str(), "");
                assert_eq!(payload.attachments().len(), 1);
                assert!(composer.draft_matches_payload(&payload));
                token
            })
        });

        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.finish_submission(token, DraftDisposition::Accepted, composer_cx);
                assert!(composer.attachments.is_empty());
                assert_eq!(composer.draft(), "");
            });
        });
    }

    #[gpui::test]
    fn attachment_tray_remove_action_keeps_the_text_transport_refusal_visible(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.attachments.push(ComposerAttachment::pending(
                    "pending",
                    "capture.png",
                    Some(gpui::ImageFormat::Png),
                    "image/png",
                    0,
                ));
                composer_cx.notify();
            });
        });
        cx.run_until_parked();

        assert!(
            cx.debug_bounds(NATIVE_COMPOSER_ATTACHMENT_TRAY_SELECTOR)
                .is_some()
        );
        assert!(
            cx.debug_bounds(NATIVE_COMPOSER_ATTACHMENT_BLOCKED_SELECTOR)
                .is_some()
        );
        let remove = cx
            .debug_bounds("artisan-native-composer-attachment-remove")
            .expect("pending attachment remove action");
        cx.simulate_click(remove.center(), Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, app| {
            assert_eq!(view.read(app).attachment_count(), 0);
            assert!(!view.read(app).send_ready());
        });
    }

    #[gpui::test]
    fn offline_drafting_and_undo_preserve_text_without_admitting_send(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        cx.update(|window, app| {
            view.update(app, |composer, cx| {
                composer.set_surface(true, "Select model".into(), cx);
                composer.replace_range(0..0, "hello 🌍", None, cx);
                assert_eq!(composer.state.draft(), "hello 🌍");
                assert!(!composer.send_ready());
                assert!(matches!(
                    composer.begin_submission(),
                    Err(super::SubmissionBlocked::Disabled)
                ));
                composer.undo_action(&super::Undo, window, cx);
                assert_eq!(composer.state.draft(), "");
                composer.redo_action(&super::Redo, window, cx);
                assert_eq!(composer.state.draft(), "hello 🌍");
            });
        });
    }

    #[gpui::test]
    fn send_has_stable_identity_focusability_visible_focus_and_local_tab_order(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        set_draft(cx, &view, "draft");

        assert!(
            cx.debug_bounds(NATIVE_COMPOSER_SEND_SELECTOR).is_some(),
            "the native Send button must retain its stable selector"
        );
        cx.update(|_, app| {
            let composer = view.read(app);
            assert_eq!(composer.focus_handle.tab_index, 0);
            assert_eq!(composer.send_focus_handle.tab_index, 2);
            assert!(composer.focus_handle.tab_stop);
            assert!(composer.send_focus_handle.tab_stop);
        });

        let ring_visible = cx.update(|window, app| {
            let focus = view.read(app).send_focus_handle.clone();
            window.focus(&focus, app);
            send_button(focus).focus_ring_visible(window)
        });
        assert!(ring_visible);

        cx.update(|window, app| {
            let composer = view.read(app);
            let editor_focus = composer.focus_handle.clone();
            let send_focus = composer.send_focus_handle.clone();
            let model_focus = composer.model_focus_handle.clone();
            window.focus(&editor_focus, app);
            window.focus_next(app);
            assert!(model_focus.is_focused(window));
            window.focus_next(app);
            assert!(send_focus.is_focused(window));
            window.focus_prev(app);
            assert!(model_focus.is_focused(window));
            window.focus_prev(app);
            assert!(editor_focus.is_focused(window));
        });
    }

    #[gpui::test]
    fn pointer_enter_and_space_activate_through_the_same_send_path(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        set_draft(cx, &view, "draft");
        let (requests, _subscription) = observe_send_requests(cx, &view);
        let send_bounds = cx
            .debug_bounds(NATIVE_COMPOSER_SEND_SELECTOR)
            .expect("the native Send button must paint");

        cx.simulate_click(send_bounds.center(), Modifiers::none());
        cx.update(|window, app| {
            assert!(
                view.read(app).send_focus_handle.is_focused(window),
                "a pointer activation must focus the shared Send button"
            );
        });
        // The fork only synthesizes a keyboard click for a key-up preceded
        // by a matching key-down on the same focus generation, so each
        // activation simulates the full press a real user produces.
        for key in ["enter", "space"] {
            cx.simulate_event(KeyDownEvent {
                keystroke: Keystroke::parse(key).expect("known keyboard activation key"),
                is_held: false,
                prefer_character_input: false,
            });
            cx.simulate_event(KeyUpEvent {
                keystroke: Keystroke::parse(key).expect("known keyboard activation key"),
            });
        }

        assert_eq!(requests.get(), 3);
    }

    #[gpui::test]
    fn repeated_activation_is_blocked_by_the_composer_single_flight(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        set_draft(cx, &view, "draft");
        let (requests, _subscription) = observe_send_requests(cx, &view);
        let send_bounds = cx
            .debug_bounds(NATIVE_COMPOSER_SEND_SELECTOR)
            .expect("the native Send button must paint");

        cx.simulate_click(send_bounds.center(), Modifiers::none());
        assert_eq!(requests.get(), 1);

        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                assert!(composer.begin_submission().is_ok());
                composer_cx.notify();
            });
        });
        cx.run_until_parked();
        cx.update(|_, app| assert!(!view.read(app).send_focus_handle.tab_stop));

        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.request_send(composer_cx);
            });
        });
        cx.simulate_click(send_bounds.center(), Modifiers::none());
        cx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse("enter").expect("known keyboard activation key"),
        });
        cx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse("space").expect("known keyboard activation key"),
        });

        assert_eq!(requests.get(), 1);
        cx.update(|_, app| assert!(view.read(app).is_submitting()));
    }

    #[test]
    fn painted_geometry_translates_global_points_and_layout_bounds() {
        let painted_bounds = Bounds::new(point(px(120.0), px(48.0)), size(px(300.0), px(96.0)));
        assert_eq!(
            localize_painted_point(&painted_bounds, point(px(137.0), px(79.0))),
            Some(point(px(17.0), px(31.0)))
        );
        assert!(localize_painted_point(&painted_bounds, point(px(119.0), px(79.0))).is_none());
        assert!(localize_painted_point(&painted_bounds, point(px(137.0), px(145.0))).is_none());

        let text_bounds = offset_layout_bounds(
            &painted_bounds,
            point(px(8.0), px(14.0)),
            point(px(88.0), px(14.0)),
            px(18.0),
        )
        .expect("valid painted geometry");
        assert_eq!(text_bounds.origin, point(px(128.0), px(62.0)));
        assert_eq!(text_bounds.size, size(px(80.0), px(18.0)));
    }

    #[test]
    fn invalid_painted_geometry_fails_closed() {
        let empty_bounds = Bounds::new(point(px(120.0), px(48.0)), size(px(0.0), px(96.0)));
        assert!(localize_painted_point(&empty_bounds, point(px(120.0), px(48.0))).is_none());
        assert!(
            offset_layout_bounds(
                &empty_bounds,
                point(px(0.0), px(0.0)),
                point(px(4.0), px(0.0)),
                px(18.0),
            )
            .is_none()
        );
    }

    #[test]
    fn raw_editing_preserves_whitespace_newlines_and_unicode() {
        let original = "\u{200b}  café\r\n\t第二行  ";
        let range = utf16_range_to_utf8(original, 3..7).expect("valid range");
        let edited = replace_text_preserving_raw(original, range, "é").expect("edit");
        assert_eq!(edited, "\u{200b}  é\r\n\t第二行  ");
        assert_eq!(edited.as_bytes(), "\u{200b}  é\r\n\t第二行  ".as_bytes());
    }

    #[test]
    fn utf16_and_utf8_offsets_reject_surrogate_splits_and_round_trip() {
        let text = "a😀b";
        assert_eq!(utf16_offset_to_utf8(text, 1), Some(1));
        assert_eq!(utf16_offset_to_utf8(text, 2), None);
        assert_eq!(utf16_offset_to_utf8(text, 3), Some(5));
        assert_eq!(utf8_offset_to_utf16(text, 5), Some(3));
        assert!(utf16_range_to_utf8(text, 2..3).is_none());
        assert!(replace_text_preserving_raw(text, 2..2, "x").is_none());
    }

    #[gpui::test]
    fn empty_composer_paints_exact_placeholder_selector_and_phrase(cx: &mut TestAppContext) {
        assert_eq!(composer_placeholder_phrase(0), "Do anything");
        assert_eq!(
            NATIVE_COMPOSER_PLACEHOLDER_SELECTOR,
            "artisan-native-composer-placeholder"
        );

        let (_view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        assert!(
            cx.debug_bounds(NATIVE_COMPOSER_PLACEHOLDER_SELECTOR)
                .is_some()
        );
    }

    #[gpui::test]
    fn nonempty_and_whitespace_drafts_hide_placeholder_without_rewriting(cx: &mut TestAppContext) {
        for draft in ["message", " \t\n"] {
            let (view, window_cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));

            window_cx.update(|_, app| {
                view.update(app, |composer, composer_cx| {
                    composer.set_draft(draft);
                    composer_cx.notify();
                });
            });
            window_cx.run_until_parked();

            assert!(
                window_cx
                    .debug_bounds(NATIVE_COMPOSER_PLACEHOLDER_SELECTOR)
                    .is_none()
            );
            window_cx.update(|_, app| assert_eq!(view.read(app).draft(), draft));
        }
    }

    #[gpui::test]
    fn clearing_a_nonempty_draft_restores_placeholder(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));

        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.set_draft("message");
                composer_cx.notify();
            });
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds(NATIVE_COMPOSER_PLACEHOLDER_SELECTOR)
                .is_none()
        );

        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.set_draft("");
                composer_cx.notify();
            });
        });
        cx.run_until_parked();

        cx.update(|_, app| assert_eq!(view.read(app).draft(), ""));
        assert!(
            cx.debug_bounds(NATIVE_COMPOSER_PLACEHOLDER_SELECTOR)
                .is_some()
        );
    }

    #[gpui::test]
    fn accepted_submission_clear_restores_placeholder(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));

        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.set_draft("accepted");
                composer_cx.notify();
            });
        });
        cx.run_until_parked();

        let token = cx.update(|_, app| {
            view.update(app, |composer, _| {
                composer
                    .begin_submission()
                    .expect("nonempty draft begins a submission")
                    .1
            })
        });
        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.finish_submission(token, DraftDisposition::Accepted, composer_cx);
            });
        });
        cx.run_until_parked();

        cx.update(|_, app| {
            let composer = view.read(app);
            assert_eq!(composer.draft(), "");
            assert!(!composer.is_submitting());
        });
        assert!(
            cx.debug_bounds(NATIVE_COMPOSER_PLACEHOLDER_SELECTOR)
                .is_some()
        );
    }

    #[gpui::test]
    fn disabled_and_submitting_empty_states_keep_one_unchanged_placeholder(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));

        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.set_disabled(true, composer_cx);
            });
        });
        cx.run_until_parked();
        cx.update(|_, app| {
            let composer = view.read(app);
            assert_eq!(composer.draft(), "");
            assert!(!composer.is_submitting());
        });
        assert!(
            cx.debug_bounds(NATIVE_COMPOSER_PLACEHOLDER_SELECTOR)
                .is_some()
        );

        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.set_disabled(false, composer_cx);
                composer.set_draft("in flight");
                composer_cx.notify();
            });
        });
        cx.run_until_parked();
        let token = cx.update(|_, app| {
            view.update(app, |composer, _| {
                composer
                    .begin_submission()
                    .expect("nonempty draft begins a submission")
                    .1
            })
        });

        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.set_draft("");
                composer_cx.notify();
            });
        });
        cx.run_until_parked();
        cx.update(|_, app| {
            let composer = view.read(app);
            assert_eq!(composer.draft(), "");
            assert!(composer.is_submitting());
        });
        assert!(
            cx.debug_bounds(NATIVE_COMPOSER_PLACEHOLDER_SELECTOR)
                .is_some()
        );

        cx.update(|_, app| {
            view.update(app, |composer, composer_cx| {
                composer.finish_submission(token, DraftDisposition::Accepted, composer_cx);
            });
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds(NATIVE_COMPOSER_PLACEHOLDER_SELECTOR)
                .is_some()
        );
    }

    #[gpui::test]
    fn clicking_painted_placeholder_uses_editor_selection_surface(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| NativeComposer::new(cx));
        let placeholder = cx
            .debug_bounds(NATIVE_COMPOSER_PLACEHOLDER_SELECTOR)
            .expect("empty composer paints the placeholder");

        cx.update(|window, app| {
            let focus = view.read(app).focus_handle.clone();
            window.focus(&focus, app);
            view.update(app, |composer, _| {
                composer.selection_reversed = true;
            });
        });

        cx.simulate_click(placeholder.center(), Modifiers::none());
        cx.run_until_parked();
        cx.update(|window, app| {
            let composer = view.read(app);
            assert!(composer.focus_handle.is_focused(window));
            assert_eq!(composer.selection, 0..0);
            assert!(!composer.selection_reversed);
        });
    }
}
