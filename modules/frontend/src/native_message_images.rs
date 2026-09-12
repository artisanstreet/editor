//! Bounded native presentation of persisted message images.
//!
//! Conversation scenes carry only [`artisan_domain::ImageAttachmentRef`]
//! metadata. This entity turns a visible reference into one authenticated
//! `RequestImage` observation, retains a small encoded LRU, and prepares a
//! 256-pixel GPUI thumbnail. The parent owns transport and calls
//! [`NativeMessageImages::accept_image`] or [`NativeMessageImages::fail_image`]
//! with the matching response.
//!
//! The entity is intentionally a presentation boundary. It does not read
//! files, construct URLs, send commands, fabricate pixels, persist messages,
//! or decide whether a run is complete. Full-size decoding is a temporary
//! viewer lease and is fenced by both the exact reference and the current
//! thread scope.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::{
    collections::{HashMap, VecDeque},
    io::Cursor,
    sync::Arc,
};

use artisan_assets::AssetId;
use artisan_domain::{ImageAttachment, ImageAttachmentRef, ImageMimeType, ThreadId};
use artisan_ui::{
    asset_seam::asset_glyph,
    theme::{ArtisanTheme, RadiusStep, RadiusTokens, ThemeMode},
};
use gpui::ColorExt;
use gpui::prelude::{
    InteractiveElement as _, IntoElement, ParentElement as _, StatefulInteractiveElement as _,
    Styled as _, StyledImage as _,
};
use gpui::{
    Bounds, Context, ElementId, FocusHandle, Image, ImageFormat, ImageSource, KeyDownEvent,
    ObjectFit, Pixels, Render, RenderImage, SvgRenderer, Task, Window, div, img, px,
};
use image::{DynamicImage, ImageDecoder, ImageFormat as EncodedImageFormat, ImageReader, Limits};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::native_transport_service::ServiceFailure;

/// The largest encoded source that this renderer will custody.
pub const MAX_SOURCE_BYTES: usize = 5 * 1024 * 1024;
/// The encoded-byte ceiling for the client image LRU.
pub const MAX_ENCODED_CACHE_BYTES: usize = 24 * 1024 * 1024;
/// The maximum number of cache records, including loading/error records.
pub const MAX_THUMBNAIL_COUNT: usize = 32;
/// The maximum number of transport requests in flight at one time.
pub const MAX_OUTSTANDING_REQUESTS: usize = 4;
/// The maximum number of visible requests retained across one paint cycle.
pub const MAX_PENDING_VISIBLE_REQUESTS: usize = 32;
/// The largest decoded image area accepted before image allocation.
pub const MAX_DECODED_IMAGE_PIXELS: u64 = 16_000_000;
/// The longest edge of an encoded thumbnail.
pub const THUMBNAIL_EDGE: u32 = 256;
/// The on-screen edge of a transcript thumbnail tile.
pub const THUMBNAIL_TILE_EDGE: Pixels = px(128.0);

/// Stable selector for the entity's optional overlay root.
pub const NATIVE_MESSAGE_IMAGES_ROOT_SELECTOR: &str = "artisan-native-message-images";
/// Stable selector for one image thumbnail tile.
pub const NATIVE_MESSAGE_IMAGE_THUMBNAIL_SELECTOR: &str = "artisan-native-message-image-thumbnail";
/// Stable selector for the full preview backdrop.
pub const NATIVE_MESSAGE_IMAGE_PREVIEW_BACKDROP_SELECTOR: &str =
    "artisan-native-message-image-preview-backdrop";
/// Stable selector for the full preview content.
pub const NATIVE_MESSAGE_IMAGE_PREVIEW_CONTENT_SELECTOR: &str =
    "artisan-native-message-image-preview-content";
/// Stable selector for the full preview close action.
pub const NATIVE_MESSAGE_IMAGE_PREVIEW_CLOSE_SELECTOR: &str =
    "artisan-native-message-image-preview-close";

/// The only observation emitted by [`NativeMessageImages`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeMessageImagesEvent {
    /// Ask the authenticated parent service to read exactly this reference.
    RequestImage(ImageAttachmentRef),
}

/// The externally visible state of one thumbnail tile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageThumbnailStatus {
    /// No visible element has admitted the reference yet.
    Unrequested,
    /// The reference is retained in the bounded visible-request queue.
    Queued,
    /// The parent has received a request and has not answered it yet.
    Loading,
    /// A bounded thumbnail is being decoded or rendered.
    Decoding,
    /// A GPUI thumbnail is ready for display.
    Ready,
    /// The request or decode failed; an explicit retry is available.
    Failed,
}

/// The outcome of a parent response handed to this entity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageResponseDisposition {
    /// The response was consumed and state changed.
    Applied,
    /// There was no live pending request with this exact reference.
    IgnoredNotPending,
    /// The exact request existed, but its bytes or local bounded state was
    /// rejected before any image decoder was entered.
    Rejected(ImageResponseRejection),
}

/// Redacted reasons for refusing a response.
///
/// This type deliberately carries no peer bytes, filenames, paths, or error
/// text. The filename in a reference is used only as already-validated UI
/// metadata and never enters this diagnostic surface.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ImageResponseRejection {
    /// No exact pending reference remained for the response.
    #[error("image response has no exact pending reference")]
    NoPendingReference,
    /// The response contained no bytes.
    #[error("image response contains no bytes")]
    EmptyBytes,
    /// The response exceeded the client source bound.
    #[error("image response exceeds the encoded-byte bound")]
    SourceTooLarge,
    /// The response MIME type differed from the reference.
    #[error("image response MIME type differs from its reference")]
    MimeTypeMismatch,
    /// The response display name differed from the reference.
    #[error("image response name differs from its reference")]
    NameMismatch,
    /// The response byte length differed from the reference.
    #[error("image response size differs from its reference")]
    SizeMismatch,
    /// The response digest differed from the reference.
    #[error("image response digest differs from its reference")]
    DigestMismatch,
    /// The bounded cache could not admit the response.
    #[error("image response could not fit the bounded cache")]
    CacheCapacity,
    /// A checked generation counter reached its finite limit.
    #[error("image-generation capacity is exhausted")]
    GenerationExhausted,
}

/// Failure returned by the controlled current-thread scope setter.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ImageStateError {
    /// A checked generation counter cannot safely wrap.
    #[error("image state generation capacity is exhausted")]
    GenerationExhausted,
}

/// The outcome of opening the lazy full-size viewer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewOpenDisposition {
    /// A viewer lease was opened and a background decode was admitted.
    Opened,
    /// The requested reference is already the open viewer.
    AlreadyOpen,
    /// The reference has no encoded or thumbnail data to inspect.
    NotReady,
    /// The reference is outside the controlled current-thread scope.
    WrongThread,
    /// A checked generation counter cannot safely wrap.
    GenerationExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingRequestState {
    Queued,
    InFlight,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingRequest {
    state: PendingRequestState,
    scope_generation: u64,
}

struct CachedImage {
    encoded: Option<Arc<Vec<u8>>>,
    thumbnail: Option<Arc<RenderImage>>,
    status: ImageThumbnailStatus,
    generation: u64,
    scope_generation: u64,
}

struct ThumbnailCache {
    entries: HashMap<ImageAttachmentRef, CachedImage>,
    lru: VecDeque<ImageAttachmentRef>,
    encoded_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CacheInsertError {
    Capacity,
}

impl ThumbnailCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            lru: VecDeque::new(),
            encoded_bytes: 0,
        }
    }

    fn status(&self, reference: &ImageAttachmentRef) -> ImageThumbnailStatus {
        self.entries
            .get(reference)
            .map_or(ImageThumbnailStatus::Unrequested, |entry| entry.status)
    }

    fn thumbnail(&self, reference: &ImageAttachmentRef) -> Option<Arc<RenderImage>> {
        self.entries
            .get(reference)
            .and_then(|entry| entry.thumbnail.clone())
    }

    fn is_failed(&self, reference: &ImageAttachmentRef) -> bool {
        self.status(reference) == ImageThumbnailStatus::Failed
    }

    fn get(&self, reference: &ImageAttachmentRef) -> Option<&CachedImage> {
        self.entries.get(reference)
    }

    #[cfg(test)]
    fn contains(&self, reference: &ImageAttachmentRef) -> bool {
        self.entries.contains_key(reference)
    }

    fn insert(
        &mut self,
        reference: ImageAttachmentRef,
        entry: CachedImage,
        protected: Option<&ImageAttachmentRef>,
    ) -> Result<Vec<ImageAttachmentRef>, CacheInsertError> {
        let incoming_bytes = entry.encoded.as_ref().map_or(0, |bytes| bytes.len());
        if incoming_bytes > MAX_ENCODED_CACHE_BYTES {
            return Err(CacheInsertError::Capacity);
        }

        let existing_bytes = self
            .entries
            .get(&reference)
            .and_then(|existing| existing.encoded.as_ref())
            .map_or(0, |bytes| bytes.len());
        let adds_record = usize::from(!self.entries.contains_key(&reference));
        let mut evicted = Vec::new();

        while self.entries.len().saturating_add(adds_record) > MAX_THUMBNAIL_COUNT
            || self
                .encoded_bytes
                .saturating_sub(existing_bytes)
                .saturating_add(incoming_bytes)
                > MAX_ENCODED_CACHE_BYTES
        {
            let candidate = self
                .lru
                .iter()
                .find(|candidate| {
                    *candidate != &reference
                        && protected.is_none_or(|protected| *candidate != protected)
                })
                .cloned();
            let Some(candidate) = candidate else {
                return Err(CacheInsertError::Capacity);
            };
            if self.remove(&candidate).is_some() {
                evicted.push(candidate);
            }
        }

        let _ = self.remove(&reference);
        self.encoded_bytes = self.encoded_bytes.saturating_add(incoming_bytes);
        self.entries.insert(reference.clone(), entry);
        self.lru.push_back(reference);
        Ok(evicted)
    }

    fn remove(&mut self, reference: &ImageAttachmentRef) -> Option<CachedImage> {
        self.lru.retain(|candidate| candidate != reference);
        let removed = self.entries.remove(reference);
        if let Some(entry) = &removed {
            self.encoded_bytes = self
                .encoded_bytes
                .saturating_sub(entry.encoded.as_ref().map_or(0, |bytes| bytes.len()));
        }
        removed
    }

    fn mark_loading(&mut self, reference: &ImageAttachmentRef) {
        if let Some(entry) = self.entries.get_mut(reference) {
            entry.status = ImageThumbnailStatus::Loading;
        }
    }

    fn mark_ready(
        &mut self,
        reference: &ImageAttachmentRef,
        generation: u64,
        scope_generation: u64,
        thumbnail: Arc<RenderImage>,
    ) -> bool {
        let Some(entry) = self.entries.get_mut(reference) else {
            return false;
        };
        if entry.generation != generation
            || entry.scope_generation != scope_generation
            || entry.status != ImageThumbnailStatus::Decoding
        {
            return false;
        }
        entry.thumbnail = Some(thumbnail);
        entry.status = ImageThumbnailStatus::Ready;
        true
    }

    fn mark_failed(
        &mut self,
        reference: &ImageAttachmentRef,
        generation: u64,
        scope_generation: u64,
    ) -> bool {
        let Some(entry) = self.entries.get_mut(reference) else {
            return false;
        };
        if entry.generation != generation || entry.scope_generation != scope_generation {
            return false;
        }
        entry.thumbnail = None;
        entry.status = ImageThumbnailStatus::Failed;
        true
    }

    fn touch(&mut self, reference: &ImageAttachmentRef) {
        if !self.entries.contains_key(reference) {
            return;
        }
        self.lru.retain(|candidate| candidate != reference);
        self.lru.push_back(reference.clone());
    }

    fn clear_other_threads(&mut self, thread_id: Option<&ThreadId>) -> Vec<ImageAttachmentRef> {
        let stale = self
            .entries
            .keys()
            .filter(|reference| thread_id.is_none_or(|thread_id| &reference.thread_id != thread_id))
            .cloned()
            .collect::<Vec<_>>();
        for reference in &stale {
            let _ = self.remove(reference);
        }
        stale
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    fn encoded_bytes(&self) -> usize {
        self.encoded_bytes
    }
}

struct ImageState {
    current_thread: Option<ThreadId>,
    scope_generation: u64,
    generation: u64,
    pending: HashMap<ImageAttachmentRef, PendingRequest>,
    visible_queue: VecDeque<ImageAttachmentRef>,
    in_flight: usize,
    cache: ThumbnailCache,
}

impl ImageState {
    fn new() -> Self {
        Self {
            current_thread: None,
            scope_generation: 0,
            generation: 0,
            pending: HashMap::new(),
            visible_queue: VecDeque::new(),
            in_flight: 0,
            cache: ThumbnailCache::new(),
        }
    }

    fn next_generation(&mut self) -> Result<u64, ImageStateError> {
        let next = self
            .generation
            .checked_add(1)
            .ok_or(ImageStateError::GenerationExhausted)?;
        self.generation = next;
        Ok(next)
    }

    fn status(&self, reference: &ImageAttachmentRef) -> ImageThumbnailStatus {
        if let Some(pending) = self.pending.get(reference) {
            return match pending.state {
                PendingRequestState::Queued => ImageThumbnailStatus::Queued,
                PendingRequestState::InFlight => ImageThumbnailStatus::Loading,
            };
        }
        self.cache.status(reference)
    }

    fn enqueue_visible(
        &mut self,
        reference: ImageAttachmentRef,
        force: bool,
        protected: Option<&ImageAttachmentRef>,
    ) -> Result<(bool, Vec<ImageAttachmentRef>), CacheInsertError> {
        if self.current_thread.as_ref() != Some(&reference.thread_id)
            || self.pending.contains_key(&reference)
        {
            return Ok((false, Vec::new()));
        }
        if !force && self.cache.status(&reference) != ImageThumbnailStatus::Unrequested {
            return Ok((false, Vec::new()));
        }
        if self.pending.len() >= MAX_PENDING_VISIBLE_REQUESTS {
            return Ok((false, Vec::new()));
        }

        let entry = CachedImage {
            encoded: None,
            thumbnail: None,
            status: ImageThumbnailStatus::Queued,
            generation: self.generation,
            scope_generation: self.scope_generation,
        };
        let evicted = self.cache.insert(reference.clone(), entry, protected)?;
        self.pending.insert(
            reference.clone(),
            PendingRequest {
                state: PendingRequestState::Queued,
                scope_generation: self.scope_generation,
            },
        );
        self.visible_queue.push_back(reference);
        Ok((true, evicted))
    }

    fn settle_pending(&mut self, reference: &ImageAttachmentRef) -> Option<PendingRequest> {
        let settled = self.pending.remove(reference);
        if let Some(pending) = settled {
            if pending.state == PendingRequestState::InFlight {
                self.in_flight = self.in_flight.saturating_sub(1);
            }
            self.visible_queue
                .retain(|candidate| candidate != reference);
            Some(pending)
        } else {
            None
        }
    }

    fn take_request_batch(&mut self) -> Vec<ImageAttachmentRef> {
        let mut requests = Vec::with_capacity(MAX_OUTSTANDING_REQUESTS);
        while self.in_flight < MAX_OUTSTANDING_REQUESTS {
            let Some(reference) = self.visible_queue.pop_front() else {
                break;
            };
            let Some(pending) = self.pending.get_mut(&reference) else {
                continue;
            };
            if pending.state != PendingRequestState::Queued {
                continue;
            }
            pending.state = PendingRequestState::InFlight;
            self.in_flight = self.in_flight.saturating_add(1);
            self.cache.mark_loading(&reference);
            requests.push(reference);
        }
        requests
    }

    fn record_failure(
        &mut self,
        reference: ImageAttachmentRef,
        protected: Option<&ImageAttachmentRef>,
    ) -> Result<Vec<ImageAttachmentRef>, CacheInsertError> {
        let entry = CachedImage {
            encoded: None,
            thumbnail: None,
            status: ImageThumbnailStatus::Failed,
            generation: self.generation,
            scope_generation: self.scope_generation,
        };
        self.cache.insert(reference, entry, protected)
    }

    fn insert_decoding(
        &mut self,
        reference: ImageAttachmentRef,
        encoded: Arc<Vec<u8>>,
        generation: u64,
        protected: Option<&ImageAttachmentRef>,
    ) -> Result<Vec<ImageAttachmentRef>, CacheInsertError> {
        let entry = CachedImage {
            encoded: Some(encoded),
            thumbnail: None,
            status: ImageThumbnailStatus::Decoding,
            generation,
            scope_generation: self.scope_generation,
        };
        self.cache.insert(reference, entry, protected)
    }

    fn clear_pending_other_threads(&mut self, thread_id: Option<&ThreadId>) {
        let stale = self
            .pending
            .keys()
            .filter(|reference| thread_id.is_none_or(|thread_id| &reference.thread_id != thread_id))
            .cloned()
            .collect::<Vec<_>>();
        for reference in stale {
            let _ = self.settle_pending(&reference);
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PreviewFence {
    reference: ImageAttachmentRef,
    generation: u64,
    scope_generation: u64,
}

struct PreviewState {
    fence: PreviewFence,
    thumbnail: Option<Arc<RenderImage>>,
    preview: Option<Arc<RenderImage>>,
    error: bool,
    task: Option<Task<()>>,
}

/// Native message-image presentation entity.
pub struct NativeMessageImages {
    state: ImageState,
    thumbnail_tasks: HashMap<ImageAttachmentRef, Task<()>>,
    viewer: Option<PreviewState>,
    focus_handle: FocusHandle,
    theme: ArtisanTheme,
}

impl NativeMessageImages {
    /// Creates an image entity with the neutral dark Spline theme.
    ///
    /// The application may call [`Self::set_theme`] when its controlled theme
    /// changes. No transport or image work starts during construction.
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            state: ImageState::new(),
            thumbnail_tasks: HashMap::new(),
            viewer: None,
            focus_handle: cx.focus_handle(),
            theme: ArtisanTheme::for_mode(ThemeMode::Dark),
        }
    }

    /// Replaces the paint theme used by the mounted overlay.
    pub fn set_theme(&mut self, theme: ArtisanTheme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    /// Sets the only thread whose visible image references may be admitted.
    ///
    /// Changing scope drops old pending admissions and all old-thread cache
    /// records, releases any full preview task, and fences late work. The
    /// caller should perform this before rendering a replacement conversation
    /// scene.
    ///
    /// # Errors
    ///
    /// Returns [`ImageStateError::GenerationExhausted`] without changing the
    /// current scope when the checked scope-generation counter cannot advance.
    pub fn set_current_thread(
        &mut self,
        thread_id: Option<&ThreadId>,
        cx: &mut Context<Self>,
    ) -> Result<(), ImageStateError> {
        if self.state.current_thread.as_ref() == thread_id {
            return Ok(());
        }
        let scope_generation = self.state.next_generation()?;
        self.state.scope_generation = scope_generation;
        self.state.current_thread = thread_id.cloned();
        self.state.clear_pending_other_threads(thread_id);
        let evicted = self.state.cache.clear_other_threads(thread_id);
        self.drop_thumbnail_tasks(evicted);
        self.thumbnail_tasks.retain(|reference, _| {
            thread_id.is_some_and(|thread_id| &reference.thread_id == thread_id)
        });
        self.viewer = None;
        cx.notify();
        Ok(())
    }

    /// Returns the currently controlled thread, if one is selected.
    #[must_use]
    pub fn current_thread(&self) -> Option<&ThreadId> {
        self.state.current_thread.as_ref()
    }

    /// Returns the bounded presentation status for one reference.
    #[must_use]
    pub fn status(&self, reference: &ImageAttachmentRef) -> ImageThumbnailStatus {
        self.state.status(reference)
    }

    /// Returns whether the lazy full preview is currently open.
    #[must_use]
    pub fn preview_is_open(&self) -> bool {
        self.viewer.is_some()
    }

    /// Builds one transcript thumbnail tile.
    ///
    /// The only automatic admission point is the tile's GPUI prepaint
    /// callback. It compares the actual element bounds with the current
    /// content mask, so rendering a long historical scene does not queue the
    /// entire scene.
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI tile builder composes the prepaint admission gate, status overlays, and keyboard/pointer wiring"
    )]
    pub fn render_thumbnail(
        &mut self,
        reference: &ImageAttachmentRef,
        theme: ArtisanTheme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let status = self.state.status(reference);
        let thumbnail = self.state.cache.thumbnail(reference);
        let selector = thumbnail_selector(reference);
        let selector_for_debug = selector.clone();
        let request_entity = cx.entity();
        let request_reference = reference.clone();
        let click_entity = cx.entity();
        let click_reference = reference.clone();
        let key_entity = cx.entity();
        let key_reference = reference.clone();
        let name = reference.name.clone();

        let mut media = div()
            .id(ElementId::Name(selector.clone().into()))
            .relative()
            .size(THUMBNAIL_TILE_EDGE)
            .flex_none()
            .overflow_hidden()
            .rounded(RadiusTokens::value(RadiusStep::Md))
            .border_1()
            .border_color(theme.colors.border.to_paint())
            .bg(theme.colors.card.to_paint())
            .cursor_pointer()
            .tab_index(0)
            .role(gpui::Role::Button)
            .aria_label(format!("View image {name}"))
            .debug_selector(move || selector_for_debug.clone())
            .on_prepaint(move |prepaint, window, app| {
                if is_visible_in_content_mask(prepaint.bounds, &window.content_mask().bounds) {
                    let () = request_entity.update(app, |images, images_cx| {
                        images.request_visible(request_reference.clone(), images_cx);
                    });
                }
            })
            .on_click(move |_, window, app| {
                let () = click_entity.update(app, |images, images_cx| {
                    let _ = images.open_preview(click_reference.clone(), window, images_cx);
                });
            })
            .on_key_down(move |event, window, app| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    let () = key_entity.update(app, |images, images_cx| {
                        let _ = images.open_preview(key_reference.clone(), window, images_cx);
                    });
                }
            });

        match (status, thumbnail) {
            (ImageThumbnailStatus::Ready, Some(thumbnail)) => {
                media = media.child(
                    img(ImageSource::Render(thumbnail))
                        .size_full()
                        .object_fit(ObjectFit::Cover),
                );
            }
            (ImageThumbnailStatus::Failed, _) => {
                media = media.child(
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(4.0))
                        .px(px(8.0))
                        .text_color(theme.colors.muted_foreground.to_paint())
                        .text_size(theme.typography.label_text)
                        .child("Image unavailable"),
                );
            }
            (ImageThumbnailStatus::Queued, _) => {
                media = media.child(image_status_label(&theme, "Queued"));
            }
            (ImageThumbnailStatus::Loading, _) => {
                media = media.child(image_status_label(&theme, "Loading image…"));
            }
            (ImageThumbnailStatus::Decoding, _) => {
                media = media.child(image_status_label(&theme, "Preparing image…"));
            }
            (ImageThumbnailStatus::Unrequested, _) | (ImageThumbnailStatus::Ready, None) => {
                media = media.child(image_status_label(&theme, "Image"));
            }
        }

        let mut footer = div()
            .w(THUMBNAIL_TILE_EDGE)
            .flex()
            .items_center()
            .gap(px(6.0))
            .text_size(theme.typography.label_text)
            .text_color(theme.colors.muted_foreground.to_paint())
            .child(name);

        if status == ImageThumbnailStatus::Failed {
            let retry_entity = cx.entity();
            let retry_reference = reference.clone();
            let retry_key_entity = cx.entity();
            let retry_key_reference = reference.clone();
            let retry = div()
                .id(ElementId::Name(retry_selector(reference).into()))
                .flex_none()
                .cursor_pointer()
                .tab_index(0)
                .role(gpui::Role::Button)
                .aria_label("Retry loading image")
                .text_color(theme.colors.accent.to_paint())
                .on_click(move |_, _, app| {
                    app.stop_propagation();
                    let () = retry_entity.update(app, |images, images_cx| {
                        let _ = images.retry_image(&retry_reference, images_cx);
                    });
                })
                .on_key_down(move |event, _, app| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        app.stop_propagation();
                        let () = retry_key_entity.update(app, |images, images_cx| {
                            let _ = images.retry_image(&retry_key_reference, images_cx);
                        });
                    }
                })
                .child("Retry");
            footer = footer.child(retry);
        }

        div()
            .id(ElementId::Name(format!("{selector}-cell").into()))
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(media)
            .child(footer)
    }

    /// Accepts one loaded image from the authenticated parent.
    ///
    /// The exact pending reference is checked before any byte is copied or
    /// decoder is entered. MIME, name, size, source bound, and SHA-256 are
    /// checked again against the byte-bearing value. A valid response retains
    /// one bounded encoded source and schedules thumbnail work in GPUI's
    /// background executor.
    pub fn accept_image(
        &mut self,
        reference: &ImageAttachmentRef,
        image: &ImageAttachment,
        cx: &mut Context<Self>,
    ) -> ImageResponseDisposition {
        if let Err(rejection) = validate_pending_response(&self.state, reference, image) {
            if rejection == ImageResponseRejection::NoPendingReference {
                return ImageResponseDisposition::IgnoredNotPending;
            }
            let _ = self.state.settle_pending(reference);
            let protected = self.viewer_reference();
            if let Ok(evicted) = self
                .state
                .record_failure(reference.clone(), protected.as_ref())
            {
                self.drop_thumbnail_tasks(evicted);
            }
            self.pump_requests(cx);
            cx.notify();
            return ImageResponseDisposition::Rejected(rejection);
        }

        let Ok(generation) = self.state.next_generation() else {
            let _ = self.state.settle_pending(reference);
            let protected = self.viewer_reference();
            if let Ok(evicted) = self
                .state
                .record_failure(reference.clone(), protected.as_ref())
            {
                self.drop_thumbnail_tasks(evicted);
            }
            self.pump_requests(cx);
            cx.notify();
            return ImageResponseDisposition::Rejected(ImageResponseRejection::GenerationExhausted);
        };

        let scope_generation = self.state.scope_generation;
        let encoded = Arc::new(image.bytes().to_vec());
        let _ = self.state.settle_pending(reference);
        let protected = self.viewer_reference();
        let Ok(evicted) = self.state.insert_decoding(
            reference.clone(),
            encoded.clone(),
            generation,
            protected.as_ref(),
        ) else {
            if let Ok(evicted) = self
                .state
                .record_failure(reference.clone(), protected.as_ref())
            {
                self.drop_thumbnail_tasks(evicted);
            }
            self.pump_requests(cx);
            cx.notify();
            return ImageResponseDisposition::Rejected(ImageResponseRejection::CacheCapacity);
        };
        self.drop_thumbnail_tasks(evicted);

        let task_reference = reference.clone();
        let mime_type = reference.mime_type;
        let task = cx.spawn(async move |this, async_cx| {
            let result = async_cx
                .background_executor()
                .spawn(async move { render_thumbnail_image(&encoded, mime_type) })
                .await;
            let _ = this.update(async_cx, |images, images_cx| {
                images.finish_thumbnail(
                    &task_reference,
                    generation,
                    scope_generation,
                    result,
                    images_cx,
                );
            });
        });
        self.thumbnail_tasks.insert(reference.clone(), task);
        self.pump_requests(cx);
        cx.notify();
        ImageResponseDisposition::Applied
    }

    /// Records a typed parent failure for one exact pending response.
    ///
    /// The transport diagnostic is consumed without retaining any service
    /// error payload in the entity.
    pub fn fail_image(
        &mut self,
        reference: ImageAttachmentRef,
        _failure: ServiceFailure,
        cx: &mut Context<Self>,
    ) -> ImageResponseDisposition {
        if validate_pending_reference(&self.state, &reference).is_err() {
            return ImageResponseDisposition::IgnoredNotPending;
        }
        let _ = self.state.settle_pending(&reference);
        let protected = self.viewer_reference();
        if let Ok(evicted) = self.state.record_failure(reference, protected.as_ref()) {
            self.drop_thumbnail_tasks(evicted);
        }
        self.pump_requests(cx);
        cx.notify();
        ImageResponseDisposition::Applied
    }

    /// Retries a failed visible reference after explicit user activation.
    ///
    /// Retry is an interaction event, not render-time work. It is admitted
    /// only for the current thread and only while the bounded pending budget
    /// has room.
    pub fn retry_image(&mut self, reference: &ImageAttachmentRef, cx: &mut Context<Self>) -> bool {
        if self.state.current_thread.as_ref() != Some(&reference.thread_id)
            || !self.state.cache.is_failed(reference)
            || self.state.pending.contains_key(reference)
            || self.state.pending.len() >= MAX_PENDING_VISIBLE_REQUESTS
        {
            return false;
        }
        let _ = self.state.cache.remove(reference);
        self.thumbnail_tasks.remove(reference);
        let protected = self.viewer_reference();
        let Ok((admitted, evicted)) =
            self.state
                .enqueue_visible(reference.clone(), true, protected.as_ref())
        else {
            return false;
        };
        if !admitted {
            return false;
        }
        self.drop_thumbnail_tasks(evicted);
        self.pump_requests(cx);
        cx.notify();
        true
    }

    /// Opens the full preview for one retained reference.
    ///
    /// Only this one viewer may hold a full decoded `RenderImage`; replacing
    /// or closing it drops the previous task and artifact.
    pub fn open_preview(
        &mut self,
        reference: ImageAttachmentRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PreviewOpenDisposition {
        if self.state.current_thread.as_ref() != Some(&reference.thread_id) {
            return PreviewOpenDisposition::WrongThread;
        }
        if self
            .viewer
            .as_ref()
            .is_some_and(|viewer| viewer.fence.reference == reference)
        {
            return PreviewOpenDisposition::AlreadyOpen;
        }
        let Some(record) = self.state.cache.get(&reference) else {
            return PreviewOpenDisposition::NotReady;
        };
        if !matches!(
            record.status,
            ImageThumbnailStatus::Decoding | ImageThumbnailStatus::Ready
        ) {
            return PreviewOpenDisposition::NotReady;
        }
        let Some(encoded) = record.encoded.clone() else {
            return PreviewOpenDisposition::NotReady;
        };
        let thumbnail = record.thumbnail.clone();
        self.state.cache.touch(&reference);
        let scope_generation = self.state.scope_generation;
        let Ok(generation) = self.state.next_generation() else {
            return PreviewOpenDisposition::GenerationExhausted;
        };
        let task_reference = reference.clone();
        let mime_type = reference.mime_type;
        let task = cx.spawn(async move |this, async_cx| {
            let result = async_cx
                .background_executor()
                .spawn(async move { render_full_preview(&encoded, mime_type) })
                .await;
            let _ = this.update(async_cx, |images, images_cx| {
                images.finish_preview(
                    &task_reference,
                    generation,
                    scope_generation,
                    result,
                    images_cx,
                );
            });
        });
        self.viewer = Some(PreviewState {
            fence: PreviewFence {
                reference,
                generation,
                scope_generation,
            },
            thumbnail,
            preview: None,
            error: false,
            task: Some(task),
        });
        self.focus_handle.focus(window, cx);
        cx.notify();
        PreviewOpenDisposition::Opened
    }

    /// Closes and releases the current full preview, if any.
    pub fn close_preview(&mut self, cx: &mut Context<Self>) -> bool {
        if self.viewer.take().is_some() {
            cx.notify();
            true
        } else {
            false
        }
    }

    /// Renders the optional lazy full preview overlay.
    ///
    /// The returned element is empty when no viewer is open, so the root may
    /// mount this entity continuously without an invisible hitbox covering
    /// the conversation.
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI overlay builder composes the backdrop, viewer chrome, and dismissal wiring"
    )]
    pub fn render_preview(
        &mut self,
        theme: ArtisanTheme,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut root = div()
            .id(ElementId::Name(NATIVE_MESSAGE_IMAGES_ROOT_SELECTOR.into()))
            .debug_selector(|| NATIVE_MESSAGE_IMAGES_ROOT_SELECTOR.to_owned());
        let Some(viewer) = self.viewer.as_ref() else {
            return root;
        };

        let reference = viewer.fence.reference.clone();
        let preview = viewer.preview.clone().or_else(|| viewer.thumbnail.clone());
        let full_ready = viewer.preview.is_some();
        let preview_error = viewer.error;
        let name = reference.name.clone();
        let dismiss_entity = cx.entity();
        let close_entity = cx.entity();
        let close_key_entity = cx.entity();

        let dismiss = div()
            .id(ElementId::Name(
                NATIVE_MESSAGE_IMAGE_PREVIEW_BACKDROP_SELECTOR.into(),
            ))
            .absolute()
            .left(Pixels::ZERO)
            .top(Pixels::ZERO)
            .right(Pixels::ZERO)
            .bottom(Pixels::ZERO)
            .debug_selector(|| NATIVE_MESSAGE_IMAGE_PREVIEW_BACKDROP_SELECTOR.to_owned())
            .on_click(move |_, _, app| {
                let () = dismiss_entity.update(app, |images, images_cx| {
                    let _ = images.close_preview(images_cx);
                });
            });

        let mut content = div()
            .id(ElementId::Name(
                NATIVE_MESSAGE_IMAGE_PREVIEW_CONTENT_SELECTOR.into(),
            ))
            .relative()
            .max_w(px(960.0))
            .max_h(px(720.0))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(8.0))
            .on_click(|_, _, app| app.stop_propagation());

        if let Some(preview) = preview {
            content = content.child(
                img(ImageSource::Render(preview))
                    .max_w(px(960.0))
                    .max_h(px(720.0))
                    .object_fit(ObjectFit::Contain),
            );
        }
        content = content.child(
            div()
                .text_size(theme.typography.label_text)
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(if preview_error {
                    "Preview unavailable".to_owned()
                } else if full_ready {
                    name
                } else {
                    "Preparing full preview…".to_owned()
                }),
        );

        let close = div()
            .id(ElementId::Name(
                NATIVE_MESSAGE_IMAGE_PREVIEW_CLOSE_SELECTOR.into(),
            ))
            .absolute()
            .top(px(12.0))
            .right(px(12.0))
            .size(px(32.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(theme.colors.card.to_paint().opacity(0.94))
            .text_color(theme.colors.muted_foreground.to_paint())
            .cursor_pointer()
            .tab_index(0)
            .role(gpui::Role::Button)
            .aria_label("Close image preview")
            .debug_selector(|| NATIVE_MESSAGE_IMAGE_PREVIEW_CLOSE_SELECTOR.to_owned())
            .on_click(move |_, _, app| {
                let () = close_entity.update(app, |images, images_cx| {
                    let _ = images.close_preview(images_cx);
                });
            })
            .on_key_down(move |event, _, app| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    let () = close_key_entity.update(app, |images, images_cx| {
                        let _ = images.close_preview(images_cx);
                    });
                }
            })
            .child(asset_glyph(AssetId::TABLER_X).size(px(16.0)));

        root = root
            .absolute()
            .left(Pixels::ZERO)
            .top(Pixels::ZERO)
            .right(Pixels::ZERO)
            .bottom(Pixels::ZERO)
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.colors.background.to_paint().opacity(0.97))
            .occlude()
            .track_focus(&self.focus_handle)
            .tab_group()
            .on_key_down(cx.listener(Self::handle_preview_key_down))
            .child(dismiss)
            .child(content)
            .child(close);
        root
    }

    fn request_visible(&mut self, reference: ImageAttachmentRef, cx: &mut Context<Self>) {
        let protected = self.viewer_reference();
        let Ok((admitted, evicted)) =
            self.state
                .enqueue_visible(reference, false, protected.as_ref())
        else {
            return;
        };
        if !admitted {
            return;
        }
        self.drop_thumbnail_tasks(evicted);
        self.pump_requests(cx);
        cx.notify();
    }

    fn pump_requests(&mut self, cx: &mut Context<Self>) {
        for reference in self.state.take_request_batch() {
            cx.emit(NativeMessageImagesEvent::RequestImage(reference));
        }
    }

    fn finish_thumbnail(
        &mut self,
        reference: &ImageAttachmentRef,
        generation: u64,
        scope_generation: u64,
        result: Result<Arc<RenderImage>, DecodeFailure>,
        cx: &mut Context<Self>,
    ) {
        if self.state.scope_generation != scope_generation
            || self.state.current_thread.as_ref() != Some(&reference.thread_id)
        {
            return;
        }
        self.thumbnail_tasks.remove(reference);
        let changed = match result {
            Ok(thumbnail) => {
                self.state
                    .cache
                    .mark_ready(reference, generation, scope_generation, thumbnail)
            }
            Err(_) => self
                .state
                .cache
                .mark_failed(reference, generation, scope_generation),
        };
        if changed {
            self.state.cache.touch(reference);
            cx.notify();
        }
    }

    fn finish_preview(
        &mut self,
        reference: &ImageAttachmentRef,
        generation: u64,
        scope_generation: u64,
        result: Result<Arc<RenderImage>, DecodeFailure>,
        cx: &mut Context<Self>,
    ) {
        let is_current = self.viewer.as_ref().is_some_and(|viewer| {
            preview_fence_is_current(Some(&viewer.fence), reference, generation, scope_generation)
                && self.state.scope_generation == scope_generation
                && self.state.current_thread.as_ref() == Some(&reference.thread_id)
        });
        if !is_current {
            return;
        }
        let Some(viewer) = self.viewer.as_mut() else {
            return;
        };
        viewer.task = None;
        if let Ok(preview) = result {
            viewer.preview = Some(preview);
            viewer.error = false;
        } else {
            viewer.preview = None;
            viewer.error = true;
        }
        cx.notify();
    }

    fn handle_preview_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key.as_str() == "escape" {
            let _ = self.close_preview(cx);
        }
    }

    fn viewer_reference(&self) -> Option<ImageAttachmentRef> {
        self.viewer
            .as_ref()
            .map(|viewer| viewer.fence.reference.clone())
    }

    fn drop_thumbnail_tasks(&mut self, references: Vec<ImageAttachmentRef>) {
        for reference in references {
            self.thumbnail_tasks.remove(&reference);
        }
    }
}

impl gpui::EventEmitter<NativeMessageImagesEvent> for NativeMessageImages {}

impl Render for NativeMessageImages {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_preview(self.theme, window, cx)
    }
}

fn validate_pending_reference(
    state: &ImageState,
    reference: &ImageAttachmentRef,
) -> Result<PendingRequest, ImageResponseRejection> {
    let Some(pending) = state.pending.get(reference).copied() else {
        return Err(ImageResponseRejection::NoPendingReference);
    };
    if pending.scope_generation != state.scope_generation
        || state.current_thread.as_ref() != Some(&reference.thread_id)
    {
        return Err(ImageResponseRejection::NoPendingReference);
    }
    Ok(pending)
}

fn validate_pending_response(
    state: &ImageState,
    reference: &ImageAttachmentRef,
    image: &ImageAttachment,
) -> Result<(), ImageResponseRejection> {
    let _ = validate_pending_reference(state, reference)?;
    validate_image_response(reference, image)
}

fn validate_image_response(
    reference: &ImageAttachmentRef,
    image: &ImageAttachment,
) -> Result<(), ImageResponseRejection> {
    if image.bytes().is_empty() {
        return Err(ImageResponseRejection::EmptyBytes);
    }
    if image.byte_len() > MAX_SOURCE_BYTES {
        return Err(ImageResponseRejection::SourceTooLarge);
    }
    if image.mime_type() != reference.mime_type {
        return Err(ImageResponseRejection::MimeTypeMismatch);
    }
    if image.name() != reference.name {
        return Err(ImageResponseRejection::NameMismatch);
    }
    if image.byte_len() != reference.size_bytes as usize {
        return Err(ImageResponseRejection::SizeMismatch);
    }
    let digest = Sha256::digest(image.bytes());
    if digest.as_slice() != reference.digest.as_slice() {
        return Err(ImageResponseRejection::DigestMismatch);
    }
    Ok(())
}

fn preview_fence_is_current(
    fence: Option<&PreviewFence>,
    reference: &ImageAttachmentRef,
    generation: u64,
    scope_generation: u64,
) -> bool {
    fence.is_some_and(|fence| {
        fence.reference == *reference
            && fence.generation == generation
            && fence.scope_generation == scope_generation
    })
}

fn is_visible_in_content_mask(bounds: Bounds<Pixels>, content_mask: &Bounds<Pixels>) -> bool {
    !bounds.intersect(content_mask).is_empty()
}

fn thumbnail_selector(reference: &ImageAttachmentRef) -> String {
    format!(
        "{NATIVE_MESSAGE_IMAGE_THUMBNAIL_SELECTOR}-{}-{}-{}",
        reference.thread_id.as_str(),
        reference.message_id.as_str(),
        reference.index
    )
}

fn retry_selector(reference: &ImageAttachmentRef) -> String {
    format!(
        "{NATIVE_MESSAGE_IMAGE_THUMBNAIL_SELECTOR}-retry-{}-{}-{}",
        reference.thread_id.as_str(),
        reference.message_id.as_str(),
        reference.index
    )
}

fn image_status_label(theme: &ArtisanTheme, label: &'static str) -> impl IntoElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .px(px(8.0))
        .text_color(theme.colors.muted_foreground.to_paint())
        .text_size(theme.typography.label_text)
        .child(label)
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
enum DecodeFailure {
    #[error("image source is invalid")]
    InvalidImage,
    #[error("image dimensions exceed the renderer bound")]
    DecodedImageTooLarge,
    #[error("image could not be rendered")]
    RenderFailed,
}

fn render_thumbnail_image(
    bytes: &[u8],
    mime_type: ImageMimeType,
) -> Result<Arc<RenderImage>, DecodeFailure> {
    let decoded = decode_bounded(bytes, mime_type)?;
    let thumbnail = decoded.thumbnail(THUMBNAIL_EDGE, THUMBNAIL_EDGE);
    let encoded = encode_png(&thumbnail)?;
    render_png(encoded)
}

fn render_full_preview(
    bytes: &[u8],
    mime_type: ImageMimeType,
) -> Result<Arc<RenderImage>, DecodeFailure> {
    let decoded = decode_bounded(bytes, mime_type)?;
    let encoded = encode_png(&decoded)?;
    render_png(encoded)
}

fn decode_bounded(bytes: &[u8], mime_type: ImageMimeType) -> Result<DynamicImage, DecodeFailure> {
    if bytes.is_empty() {
        return Err(DecodeFailure::InvalidImage);
    }
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err(DecodeFailure::InvalidImage);
    }
    let mut reader = ImageReader::new(Cursor::new(bytes));
    reader.set_format(encoded_format(mime_type));
    reader.limits(decoding_limits());
    let decoder = reader
        .into_decoder()
        .map_err(|_| DecodeFailure::InvalidImage)?;
    let (width, height) = decoder.dimensions();
    if !decoded_dimensions_are_bounded(width, height) {
        return Err(DecodeFailure::DecodedImageTooLarge);
    }
    DynamicImage::from_decoder(decoder).map_err(|_| DecodeFailure::InvalidImage)
}

fn decoding_limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(u32::try_from(MAX_DECODED_IMAGE_PIXELS).unwrap_or(u32::MAX));
    limits.max_image_height = Some(u32::try_from(MAX_DECODED_IMAGE_PIXELS).unwrap_or(u32::MAX));
    limits.max_alloc = MAX_DECODED_IMAGE_PIXELS
        .checked_mul(4)
        .and_then(|bytes| bytes.checked_add(16 * 1024 * 1024));
    limits
}

fn decoded_dimensions_are_bounded(width: u32, height: u32) -> bool {
    width != 0
        && height != 0
        && u64::from(width)
            .checked_mul(u64::from(height))
            .is_some_and(|pixels| pixels <= MAX_DECODED_IMAGE_PIXELS)
}

fn encode_png(image: &DynamicImage) -> Result<Vec<u8>, DecodeFailure> {
    let mut output = Cursor::new(Vec::new());
    image
        .write_to(&mut output, EncodedImageFormat::Png)
        .map_err(|_| DecodeFailure::InvalidImage)?;
    Ok(output.into_inner())
}

fn render_png(bytes: Vec<u8>) -> Result<Arc<RenderImage>, DecodeFailure> {
    Image::from_bytes(ImageFormat::Png, bytes)
        .to_image_data(SvgRenderer::new(Arc::new(())))
        .map_err(|_| DecodeFailure::RenderFailed)
}

fn encoded_format(mime_type: ImageMimeType) -> EncodedImageFormat {
    match mime_type {
        ImageMimeType::Gif => EncodedImageFormat::Gif,
        ImageMimeType::Jpeg => EncodedImageFormat::Jpeg,
        ImageMimeType::Png => EncodedImageFormat::Png,
        ImageMimeType::Webp => EncodedImageFormat::WebP,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use artisan_domain::{MessageId, ThreadId};
    use gpui::{Bounds, point, px, size};

    fn reference(thread: &str, message: &str, index: u32, bytes: &[u8]) -> ImageAttachmentRef {
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        ImageAttachmentRef::new(
            MessageId::parse(message).expect("test message id is valid"),
            ThreadId::parse(thread).expect("test thread id is valid"),
            index,
            "image/png",
            format!("capture-{index}.png"),
            u32::try_from(bytes.len()).expect("test bytes fit"),
            digest,
        )
        .expect("test reference is valid")
    }

    fn attachment(name: &str, bytes: &[u8]) -> ImageAttachment {
        ImageAttachment::new("image/png", bytes.to_vec(), name).expect("test attachment is valid")
    }

    fn state_for(thread: &str) -> ImageState {
        let mut state = ImageState::new();
        state.current_thread = Some(ThreadId::parse(thread).expect("test thread is valid"));
        state.scope_generation = 1;
        state
    }

    fn cached_entry(
        bytes: Option<Arc<Vec<u8>>>,
        status: ImageThumbnailStatus,
        generation: u64,
    ) -> CachedImage {
        CachedImage {
            encoded: bytes,
            thumbnail: None,
            status,
            generation,
            scope_generation: 1,
        }
    }

    fn bounds(origin_x: f32, origin_y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(origin_x), px(origin_y)),
            size: size(px(width), px(height)),
        }
    }

    #[test]
    fn exact_reference_is_required_before_bytes_are_validated() {
        let bytes = [1, 2, 3];
        let exact = reference("thread-a", "message-a", 0, &bytes);
        let other_thread = reference("thread-b", "message-a", 0, &bytes);
        let mut state = state_for("thread-a");
        state
            .enqueue_visible(exact.clone(), false, None)
            .expect("queue admission succeeds");

        assert!(
            validate_pending_response(&state, &exact, &attachment("capture-0.png", &bytes)).is_ok()
        );
        assert_eq!(
            validate_pending_response(&state, &other_thread, &attachment("capture-0.png", &bytes)),
            Err(ImageResponseRejection::NoPendingReference)
        );
    }

    #[test]
    fn changed_digest_size_type_and_name_are_rejected_before_decode() {
        let expected_bytes = [1, 2, 3];
        let exact = reference("thread-a", "message-a", 0, &expected_bytes);
        let mut state = state_for("thread-a");
        state
            .enqueue_visible(exact.clone(), false, None)
            .expect("queue admission succeeds");

        assert_eq!(
            validate_pending_response(&state, &exact, &attachment("capture-0.png", &[4, 5, 6])),
            Err(ImageResponseRejection::DigestMismatch)
        );
        assert_eq!(
            validate_pending_response(&state, &exact, &attachment("capture-0.png", &[1, 2])),
            Err(ImageResponseRejection::SizeMismatch)
        );
        assert_eq!(
            validate_pending_response(
                &state,
                &exact,
                &ImageAttachment::new("image/jpeg", expected_bytes.to_vec(), "capture-0.png")
                    .expect("test attachment is valid")
            ),
            Err(ImageResponseRejection::MimeTypeMismatch)
        );
        assert_eq!(
            validate_pending_response(&state, &exact, &attachment("other.png", &expected_bytes)),
            Err(ImageResponseRejection::NameMismatch)
        );
    }

    #[test]
    fn preview_fence_discards_close_and_thread_switch_results() {
        let bytes = [1, 2, 3];
        let exact = reference("thread-a", "message-a", 0, &bytes);
        let fence = PreviewFence {
            reference: exact.clone(),
            generation: 7,
            scope_generation: 3,
        };

        assert!(preview_fence_is_current(Some(&fence), &exact, 7, 3));
        assert!(!preview_fence_is_current(None, &exact, 7, 3));
        assert!(!preview_fence_is_current(Some(&fence), &exact, 7, 4));
        assert!(!preview_fence_is_current(Some(&fence), &exact, 8, 3));
        assert!(!preview_fence_is_current(
            Some(&fence),
            &reference("thread-b", "message-a", 0, &bytes),
            7,
            3
        ));
    }

    #[test]
    fn cache_keeps_lru_count_and_encoded_byte_budgets() {
        let mut cache = ThumbnailCache::new();
        let small = Arc::new(vec![0_u8; 1]);
        let mut references = Vec::new();
        for index in 0..MAX_THUMBNAIL_COUNT {
            let reference = reference(
                "thread-a",
                &format!("message-{index}"),
                u32::try_from(index).expect("bounded index"),
                &[1],
            );
            cache
                .insert(
                    reference.clone(),
                    cached_entry(
                        Some(small.clone()),
                        ImageThumbnailStatus::Ready,
                        index as u64,
                    ),
                    None,
                )
                .expect("small entry fits");
            references.push(reference);
        }
        cache.touch(&references[0]);
        let newest = reference("thread-a", "message-new", 99, &[1]);
        cache
            .insert(
                newest,
                cached_entry(Some(small.clone()), ImageThumbnailStatus::Ready, 99),
                None,
            )
            .expect("one LRU entry can be evicted");
        assert_eq!(cache.len(), MAX_THUMBNAIL_COUNT);
        assert!(cache.contains(&references[0]));
        assert!(!cache.contains(&references[1]));

        let mut bytes = Vec::new();
        for index in 0..8 {
            bytes.push(reference(
                "thread-a",
                &format!("large-{index}"),
                index,
                &[1],
            ));
        }
        for (index, reference) in bytes.into_iter().enumerate() {
            cache
                .insert(
                    reference,
                    cached_entry(
                        Some(Arc::new(vec![0_u8; 5 * 1024 * 1024])),
                        ImageThumbnailStatus::Ready,
                        index as u64,
                    ),
                    None,
                )
                .expect("the bounded LRU evicts old entries");
        }
        assert!(cache.encoded_bytes() <= MAX_ENCODED_CACHE_BYTES);
        assert!(cache.len() <= MAX_THUMBNAIL_COUNT);
    }

    #[test]
    fn request_admission_deduplicates_caps_and_retries_errors() {
        let mut state = state_for("thread-a");
        let references = (0..MAX_PENDING_VISIBLE_REQUESTS)
            .map(|index| {
                reference(
                    "thread-a",
                    &format!("message-{index}"),
                    u32::try_from(index).expect("bounded index"),
                    &[1],
                )
            })
            .collect::<Vec<_>>();

        assert!(
            state
                .enqueue_visible(references[0].clone(), false, None)
                .expect("first request fits")
                .0
        );
        assert!(
            !state
                .enqueue_visible(references[0].clone(), false, None)
                .expect("duplicate is a no-op")
                .0
        );
        for reference in references.iter().skip(1) {
            assert!(
                state
                    .enqueue_visible(reference.clone(), false, None)
                    .expect("request fits")
                    .0
            );
        }
        let overflow = reference("thread-a", "message-overflow", 100, &[1]);
        assert!(
            !state
                .enqueue_visible(overflow, false, None)
                .expect("capacity refusal is typed")
                .0
        );
        assert_eq!(state.take_request_batch().len(), MAX_OUTSTANDING_REQUESTS);

        let failed = references[0].clone();
        let _ = state.settle_pending(&failed);
        state
            .record_failure(failed.clone(), None)
            .expect("failure record fits");
        assert_eq!(state.status(&failed), ImageThumbnailStatus::Failed);
        let _ = state.cache.remove(&failed);
        assert!(
            state
                .enqueue_visible(failed, true, None)
                .expect("explicit retry fits")
                .0
        );
    }

    #[test]
    fn hidden_thumbnail_bounds_never_admit_a_request() {
        let hidden = bounds(900.0, 900.0, 128.0, 128.0);
        let viewport = bounds(0.0, 0.0, 640.0, 480.0);
        assert!(!is_visible_in_content_mask(hidden, &viewport));
        assert!(is_visible_in_content_mask(
            bounds(8.0, 16.0, 128.0, 128.0),
            &viewport
        ));

        let mut state = state_for("thread-a");
        let hidden_reference = reference("thread-a", "hidden", 0, &[1]);
        if is_visible_in_content_mask(hidden, &viewport) {
            let _ = state.enqueue_visible(hidden_reference, false, None);
        }
        assert!(state.pending.is_empty());
    }

    #[test]
    fn generation_overflow_is_a_typed_failure() {
        let mut state = ImageState::new();
        state.generation = u64::MAX;
        assert_eq!(
            state.next_generation(),
            Err(ImageStateError::GenerationExhausted)
        );
        assert_eq!(state.generation, u64::MAX);
    }
}
