//! Synchronous gesture classification for the composer boundary.
//!
//! This is the dependency-free Rust counterpart of
//! `modules/frontend/src/lib/composer/gesture-intake.ts`. It captures only
//! the data that must be read while a browser gesture is dispatching. Queue
//! scheduling, worker execution, browser `File` values, DOM events, and
//! cleanup belong to the caller and are deliberately not represented here.
//!
//! A transfer is optional because browser drag and clipboard events may have
//! no `DataTransfer`. MIME filtering is exact: accepted files are those whose
//! type starts with the case-sensitive `image/` prefix, and a drag is
//! accepted only when its type list contains the case-sensitive `Files`
//! entry.

/// The file metadata needed by the gesture policy.
///
/// The native policy does not need a browser `File` object or its contents;
/// it only needs the MIME type in order to retain image files. The owning
/// attachment layer can associate this metadata with its eventual file bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComposerFileMetadata {
    /// The browser-reported MIME type, preserved exactly.
    pub mime_type: String,
}

impl ComposerFileMetadata {
    /// Creates metadata with the supplied MIME type, without normalizing it.
    #[must_use]
    pub fn new(mime_type: impl Into<String>) -> Self {
        Self {
            mime_type: mime_type.into(),
        }
    }

    /// Returns whether this file has the exact image MIME prefix accepted by
    /// the legacy composer.
    #[must_use]
    pub fn is_image(&self) -> bool {
        self.mime_type.starts_with("image/")
    }
}

/// The synchronous subset of a browser `DataTransfer` used by the composer.
///
/// `types` is kept separately from `files` because drag acceptance uses the
/// browser's advertised type list, while drop and paste use the file list.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ComposerFileTransfer {
    /// Files exposed by the transfer, in browser order.
    pub files: Vec<ComposerFileMetadata>,
    /// Advertised transfer types, in browser order.
    pub types: Vec<String>,
}

impl ComposerFileTransfer {
    /// Creates a transfer snapshot without changing either input sequence.
    #[must_use]
    pub fn new(files: Vec<ComposerFileMetadata>, types: Vec<String>) -> Self {
        Self { files, types }
    }
}

/// Coordinates captured from the client point at which a file was dropped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ComposerDropPoint {
    /// Client-space horizontal coordinate.
    pub x: f64,
    /// Client-space vertical coordinate.
    pub y: f64,
}

impl ComposerDropPoint {
    /// Creates a drop point, preserving coordinates exactly.
    #[must_use]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// Keyboard fields needed to classify one composer submit gesture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComposerSubmitKeyInput<'a> {
    /// Whether the browser is currently composing IME text.
    pub is_composing: bool,
    /// The browser-reported key value, compared exactly with `Enter`.
    pub key: &'a str,
    /// Whether Shift is held, which requests a newline instead of submit.
    pub shift_key: bool,
}

impl<'a> ComposerSubmitKeyInput<'a> {
    /// Creates keyboard input without normalizing the key value.
    #[must_use]
    pub const fn new(is_composing: bool, key: &'a str, shift_key: bool) -> Self {
        Self {
            is_composing,
            key,
            shift_key,
        }
    }
}

/// One gesture detached from its browser event.
#[derive(Clone, Debug, PartialEq)]
pub enum ComposerGesture {
    /// A plain Enter submit request.
    Submit,
    /// Image files from a drop or paste, optionally with a drop point.
    Images {
        /// Accepted files in their original transfer order.
        files: Vec<ComposerFileMetadata>,
        /// `Some` for a drop and `None` for a paste.
        point: Option<ComposerDropPoint>,
    },
}

/// The synchronous result of classifying one composer input.
///
/// `gesture` is `None` when the input is ignored or when a submit is a
/// browser auto-repeat already represented by the pending submit latch.
/// `prevent_default` remains true for that repeated plain Enter, matching the
/// browser contract: the newline is suppressed even though no second submit
/// is queued.
#[derive(Clone, Debug, PartialEq)]
pub struct ComposerGestureDecision {
    /// The detached gesture for the worker, if one should be retained.
    pub gesture: Option<ComposerGesture>,
    /// Whether the caller must prevent the browser's default action now.
    pub prevent_default: bool,
}

impl ComposerGestureDecision {
    /// Creates an ignored decision that leaves the browser default intact.
    #[must_use]
    pub const fn ignored() -> Self {
        Self {
            gesture: None,
            prevent_default: false,
        }
    }

    /// Creates a retained gesture that suppresses the browser default.
    #[must_use]
    fn retained(gesture: ComposerGesture) -> Self {
        Self {
            gesture: Some(gesture),
            prevent_default: true,
        }
    }

    /// Creates a prevention-only decision, used for accepted drags and
    /// repeated plain Enter events.
    #[must_use]
    fn prevent_only() -> Self {
        Self {
            gesture: None,
            prevent_default: true,
        }
    }
}

/// Returns the drag-over decision for an optional transfer.
///
/// The browser delivers a later drop only when the drag-over default is
/// prevented for a transfer whose type list contains exact `Files`. This
/// decision never retains a gesture or reads the file list.
#[must_use]
pub fn accept_file_drag(transfer: Option<&ComposerFileTransfer>) -> ComposerGestureDecision {
    let accepts_files = transfer.is_some_and(|transfer| {
        transfer
            .types
            .iter()
            .any(|transfer_type| transfer_type == "Files")
    });

    if accepts_files {
        ComposerGestureDecision::prevent_only()
    } else {
        ComposerGestureDecision::ignored()
    }
}

/// Returns the drop decision for an optional transfer and its client point.
///
/// Only image files are retained. An absent transfer, empty file list, or
/// transfer without accepted images is a no-op that does not prevent the
/// browser default.
#[must_use]
pub fn classify_drop(
    transfer: Option<&ComposerFileTransfer>,
    point: ComposerDropPoint,
) -> ComposerGestureDecision {
    let files = image_files(transfer);
    if files.is_empty() {
        return ComposerGestureDecision::ignored();
    }

    ComposerGestureDecision::retained(ComposerGesture::Images {
        files,
        point: Some(point),
    })
}

/// Returns the paste decision for an optional transfer.
///
/// Pasted image files retain their transfer order and never carry a drop
/// point. An absent transfer, empty file list, or transfer without accepted
/// images is a no-op that does not prevent the browser default.
#[must_use]
pub fn classify_paste(transfer: Option<&ComposerFileTransfer>) -> ComposerGestureDecision {
    let files = image_files(transfer);
    if files.is_empty() {
        return ComposerGestureDecision::ignored();
    }

    ComposerGestureDecision::retained(ComposerGesture::Images { files, point: None })
}

fn image_files(transfer: Option<&ComposerFileTransfer>) -> Vec<ComposerFileMetadata> {
    transfer
        .map(|transfer| {
            transfer
                .files
                .iter()
                .filter(|file| file.is_image())
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// Synchronous submit de-duplication state.
///
/// At most one submit remains pending until the worker takes it. The latch is
/// released before the worker runs, so a failed worker operation does not
/// permanently suppress a later plain Enter. Image gestures do not interact
/// with this state.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ComposerGestureState {
    submit_queued: bool,
}

impl ComposerGestureState {
    /// Creates an idle state with no pending submit.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            submit_queued: false,
        }
    }

    /// Returns whether a submit is currently waiting for the worker to take
    /// it.
    #[must_use]
    pub const fn is_submit_queued(&self) -> bool {
        self.submit_queued
    }

    /// Classifies keyboard input and applies submit de-duplication.
    ///
    /// Composing input, non-`Enter` keys, and Shift+Enter are ignored without
    /// preventing the browser default. A plain Enter always prevents the
    /// default; if a submit is already queued, it produces no second
    /// [`ComposerGesture::Submit`].
    #[must_use]
    pub fn submit_key(&mut self, input: ComposerSubmitKeyInput<'_>) -> ComposerGestureDecision {
        if input.is_composing || input.key != "Enter" || input.shift_key {
            return ComposerGestureDecision::ignored();
        }

        if self.submit_queued {
            return ComposerGestureDecision::prevent_only();
        }

        self.submit_queued = true;
        ComposerGestureDecision::retained(ComposerGesture::Submit)
    }

    /// Marks a detached gesture as taken by the worker.
    ///
    /// This must happen before the caller runs the effectful work. Taking a
    /// submit releases the de-duplication latch even if that work later
    /// fails; taking an image gesture leaves the latch unchanged.
    pub fn mark_taken(&mut self, gesture: &ComposerGesture) {
        if matches!(gesture, ComposerGesture::Submit) {
            self.submit_queued = false;
        }
    }
}
