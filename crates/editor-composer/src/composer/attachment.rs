use artisan_api::BlobHash;
enum Input {
    Encoded,
    Uploaded(BlobHash),
    Failed(String),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Encoding,
    Uploading,
    Ready(BlobHash),
    Failed(String),
}
impl State {
    pub(crate) const fn hash(&self) -> Option<&BlobHash> {
        match self {
            Self::Ready(hash) => Some(hash),
            Self::Encoding | Self::Uploading | Self::Failed(_) => None,
        }
    }
    pub(crate) fn failure(&self) -> Option<&str> {
        match self {
            Self::Failed(reason) => Some(reason),
            Self::Encoding | Self::Uploading | Self::Ready(_) => None,
        }
    }
    fn accept(&mut self, input: &Input) -> bool {
        match (&self, input) {
            (Self::Encoding, Input::Encoded) => {
                *self = Self::Uploading;
                true
            }
            (Self::Uploading, Input::Uploaded(hash)) => {
                *self = Self::Ready(hash.clone());
                false
            }
            (Self::Encoding | Self::Uploading, Input::Failed(reason)) => {
                *self = Self::Failed(reason.clone());
                false
            }
            (Self::Encoding, Input::Uploaded(_))
            | (Self::Uploading, Input::Encoded)
            | (
                Self::Ready(_) | Self::Failed(_),
                Input::Encoded | Input::Uploaded(_) | Input::Failed(_),
            ) => false,
        }
    }
}

use aqua::images::{ImageAssets, ImageHandle, ImageStatus};
use editor_core::{ForgeClient, delivery::Delivery, runtime::runtime};
use std::{
    hash::{Hash, Hasher},
    num::NonZeroUsize,
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::Instant,
};

#[derive(Debug)]
pub enum Source {
    Bytes(Vec<u8>),
    Path(PathBuf),
    Clipboard(aqua_ui::input::PasteImage),
}

#[derive(Debug, thiserror::Error)]
pub enum ReadImageError {
    #[error("Images must be at most 32 MiB")]
    TooLarge,
    #[error("the pasted {width}×{height} image is too large to encode")]
    ClipboardExtent {
        width: usize,
        height: usize,
        #[source]
        source: std::num::TryFromIntError,
    },
    #[error("encode the pasted image")]
    Clipboard(#[source] image::ImageError),
    #[error("read {}", path.display())]
    File {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum PrepareImageError {
    #[error(transparent)]
    Read(#[from] ReadImageError),
    #[error(transparent)]
    Encode(#[from] super::encode::EncodeError),
}

#[derive(Debug, thiserror::Error)]
pub enum UploadImageError {
    #[error("the image preparation task stopped")]
    Task(#[source] tokio::task::JoinError),
    #[error("prepare the image")]
    Prepare(#[source] PrepareImageError),
    #[error("upload the image to Forge")]
    Put(#[source] editor_core::client::CallError),
}

impl Source {
    fn read(&self) -> Result<Vec<u8>, ReadImageError> {
        use std::io::Read as _;
        let bytes = match self {
            Self::Bytes(bytes) => bytes.clone(),
            Self::Clipboard(image) => {
                use image::ImageEncoder as _;
                let extent = |source| ReadImageError::ClipboardExtent {
                    width: image.width(),
                    height: image.height(),
                    source,
                };
                let mut bytes = Vec::new();
                image::codecs::png::PngEncoder::new(&mut bytes)
                    .write_image(
                        image.rgba(),
                        u32::try_from(image.width()).map_err(extent)?,
                        u32::try_from(image.height()).map_err(extent)?,
                        image::ExtendedColorType::Rgba8,
                    )
                    .map_err(ReadImageError::Clipboard)?;
                bytes
            }
            Self::Path(path) => {
                let failed = |source| ReadImageError::File {
                    path: path.clone(),
                    source,
                };
                let file = std::fs::File::open(path).map_err(failed)?;
                if file.metadata().map_err(failed)?.len() > super::encode::MAX_INPUT_BYTES as u64 {
                    return Err(ReadImageError::TooLarge);
                }
                let mut bytes = Vec::new();
                file.take((super::encode::MAX_INPUT_BYTES + 1) as u64)
                    .read_to_end(&mut bytes)
                    .map_err(failed)?;
                bytes
            }
        };
        if bytes.len() > super::encode::MAX_INPUT_BYTES {
            return Err(ReadImageError::TooLarge);
        }
        Ok(bytes)
    }
}

#[derive(Debug)]
pub struct Attachment {
    pub key: u64,
    pub(crate) name: String,
    pub(crate) state: State,
    pub preview: Option<ImageHandle>,
    pending_preview: Option<ImageHandle>,
    pub(crate) preview_error: Option<String>,
    pub(crate) revealed: Option<Instant>,
    pub(crate) shown: bool,
    source: Option<(u64, usize)>,
    origin: Option<Arc<Source>>,
    encoded: Option<Vec<u8>>,
    task: Option<tokio::task::AbortHandle>,
}

#[derive(Clone, Debug)]
pub struct PendingUpload {
    origin: Arc<Source>,
}

impl PendingUpload {
    pub(crate) fn kept(bytes: Vec<u8>) -> Self {
        Self {
            origin: Arc::new(Source::Bytes(bytes)),
        }
    }

    pub(crate) fn bytes(&self) -> Result<Vec<u8>, ReadImageError> {
        self.origin.read()
    }

    pub(crate) async fn upload(self, client: ForgeClient) -> Result<BlobHash, UploadImageError> {
        let origin = self.origin;
        let webp = tokio::task::spawn_blocking(move || {
            let bytes = origin.read()?;
            Ok::<_, PrepareImageError>(super::encode::to_webp(&bytes)?)
        })
        .await
        .map_err(UploadImageError::Task)?
        .map_err(UploadImageError::Prepare)?;
        client
            .put_blob(webp)
            .await
            .map(|blob| blob.hash)
            .map_err(UploadImageError::Put)
    }
}

impl Drop for Attachment {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

enum Completion {
    Read(u64, Result<(Vec<u8>, Option<image::RgbaImage>), String>),
    Converted(u64, Result<Vec<u8>, String>),
    Uploaded(u64, Result<BlobHash, String>),
    Restored(u64, Result<Vec<u8>, String>),
}

#[derive(Debug)]
pub struct Attachments {
    pub items: Vec<Attachment>,
    pub(crate) shake: Option<(u64, Instant)>,
    pub(crate) notice: Option<String>,
    next: u64,
    client: ForgeClient,
    delivery: Delivery<Completion>,
}

impl Attachments {
    pub fn new(client: ForgeClient) -> Self {
        Self {
            items: Vec::new(),
            shake: None,
            notice: None,
            next: 0,
            client,
            delivery: Delivery::new(
                NonZeroUsize::new(32)
                    .expect("invariant: the attachment delivery capacity is non-zero"),
            ),
        }
    }
    pub fn wake_mailbox(&self) -> Option<&aqua::mailbox::Mailbox<()>> {
        self.delivery.wake_mailbox()
    }
    pub(crate) fn ready_hashes(&self) -> Option<Vec<BlobHash>> {
        self.items
            .iter()
            .map(|item| item.state.hash().cloned())
            .collect()
    }
    pub fn add(&mut self, source: Source, name: String) -> bool {
        if self.items.len() >= 8 {
            self.notice = Some("A message can contain at most 8 images.".into());
            return false;
        }
        if matches!(&source, Source::Bytes(bytes) if bytes.len() > super::encode::MAX_INPUT_BYTES) {
            self.notice = Some("Images must be at most 32 MiB.".into());
            return false;
        }
        let source = Arc::new(source);
        let key = self.next;
        self.next += 1;
        self.items.push(Attachment {
            key,
            name,
            state: State::Encoding,
            preview: None,
            pending_preview: None,
            preview_error: None,
            revealed: None,
            shown: false,
            source: None,
            origin: Some(Arc::clone(&source)),
            encoded: None,
            task: None,
        });
        let sender = self.delivery.sender();
        let task = runtime().spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                source.read().map(|bytes| {
                    let placeholder = match super::encode::placeholder(&bytes) {
                        Ok(placeholder) => Some(placeholder),
                        Err(error) => {
                            tracing::debug!(
                                key,
                                error = %diagnostics::chain(&error),
                                "no quick preview for the attachment; it shows once converted"
                            );
                            None
                        }
                    };
                    (bytes, placeholder)
                })
            })
            .await
            .map_err(|error| error.to_string())
            .and_then(|result| result.map_err(|error| diagnostics::chain(&error)));
            sender.offer(Completion::Read(key, result)).await;
        });
        if let Some(item) = self.items.last_mut() {
            item.task = Some(task.abort_handle());
        }
        true
    }
    pub(crate) fn restore(&mut self, hash: BlobHash) {
        if self.items.len() >= 8 {
            self.notice = Some("This draft exceeds the 8 image limit.".into());
            return;
        }
        let key = self.next;
        self.next += 1;
        self.items.push(Attachment {
            key,
            name: "Image".into(),
            state: State::Ready(hash.clone()),
            preview: None,
            pending_preview: None,
            preview_error: None,
            revealed: None,
            shown: true,
            source: None,
            origin: None,
            encoded: None,
            task: None,
        });
        self.load_preview(key, hash);
    }
    fn load_preview(&self, key: u64, hash: BlobHash) {
        let client = self.client.clone();
        let sender = self.delivery.sender();
        runtime().spawn(async move {
            let result = client
                .blob(&hash)
                .await
                .map_err(|error| diagnostics::chain(&error));
            sender.offer(Completion::Restored(key, result)).await;
        });
    }
    pub(crate) fn retry_preview(&mut self, key: u64) {
        let Some(item) = self.items.iter_mut().find(|item| item.key == key) else {
            return;
        };
        let Some(hash) = item.state.hash().cloned() else {
            return;
        };
        item.preview_error = None;
        self.load_preview(key, hash);
    }
    pub(crate) fn remove(&mut self, key: u64) {
        self.items.retain(|item| item.key != key);
    }
    pub(crate) fn clear(&mut self) {
        self.items.clear();
    }
    pub(crate) fn contains(&self, hash: &BlobHash) -> bool {
        self.items
            .iter()
            .any(|item| item.state.hash() == Some(hash))
    }
    pub fn take_unfinished(&mut self) -> Vec<PendingUpload> {
        self.items
            .iter_mut()
            .filter(|item| matches!(item.state, State::Encoding | State::Uploading))
            .filter_map(|item| item.origin.take())
            .map(|origin| PendingUpload { origin })
            .collect()
    }
    #[expect(
        clippy::too_many_lines,
        reason = "each attachment completion kind updates the same item list and notice in one drain pass"
    )]
    pub(crate) fn drain(&mut self, assets: &ImageAssets) -> bool {
        let mut changed = false;
        for completion in self.delivery.drain() {
            let key = match &completion {
                Completion::Read(key, _)
                | Completion::Converted(key, _)
                | Completion::Uploaded(key, _)
                | Completion::Restored(key, _) => *key,
            };
            if !self.items.iter().any(|item| item.key == key) {
                continue;
            }
            changed = true;
            match completion {
                Completion::Read(key, result) => match result {
                    Err(error) => {
                        if let Some(item) = self.items.iter_mut().find(|item| item.key == key) {
                            item.shown = true;
                            fail(item, error);
                        }
                    }
                    Ok((bytes, placeholder)) => {
                        let mut hasher = std::hash::DefaultHasher::new();
                        bytes.hash(&mut hasher);
                        let source = (hasher.finish(), bytes.len());
                        if let Some(existing) = self
                            .items
                            .iter()
                            .find(|item| item.key != key && item.source == Some(source))
                            .map(|item| item.key)
                        {
                            self.remove(key);
                            self.shake = Some((existing, Instant::now()));
                            continue;
                        }
                        let Some(item) = self.items.iter_mut().find(|item| item.key == key) else {
                            continue;
                        };
                        item.source = Some(source);
                        item.preview = placeholder.and_then(|image| {
                            let (width, height) = image.dimensions();
                            let inserted =
                                aqua::images::DecodedImage::from_rgba(width, height, image.into_raw())
                                    .map_err(|error| error.to_string())
                                    .and_then(|image| {
                                        assets.insert(image).map_err(|error| error.to_string())
                                    });
                            match inserted {
                                Ok(handle) => Some(handle),
                                Err(error) => {
                                    tracing::debug!(key, %error, "couldn't show the attachment's quick preview");
                                    None
                                }
                            }
                        });
                        item.shown = true;
                        let sender = self.delivery.sender();
                        let task = runtime().spawn(async move {
                            static CONVERSION: OnceLock<Arc<tokio::sync::Semaphore>> =
                                OnceLock::new();
                            let permit = Arc::clone(
                                CONVERSION.get_or_init(|| Arc::new(tokio::sync::Semaphore::new(1))),
                            )
                            .acquire_owned()
                            .await;
                            let result = match permit {
                                Ok(permit) => tokio::task::spawn_blocking(move || {
                                    let _permit = permit;
                                    super::encode::to_webp(&bytes)
                                })
                                .await
                                .map_err(|error| error.to_string())
                                .and_then(|result| {
                                    result.map_err(|error| diagnostics::chain(&error))
                                }),
                                Err(error) => Err(error.to_string()),
                            };
                            sender.offer(Completion::Converted(key, result)).await;
                        });
                        item.task = Some(task.abort_handle());
                    }
                },
                Completion::Converted(key, result) => {
                    if let Some(item) = self.items.iter_mut().find(|item| item.key == key) {
                        match result {
                            Ok(bytes) => match assets.decode(bytes.clone()) {
                                Ok(preview) => {
                                    item.pending_preview = Some(preview);
                                    item.encoded = Some(bytes);
                                }
                                Err(error) => {
                                    fail(item, error.to_string());
                                }
                            },
                            Err(error) => {
                                fail(item, error);
                            }
                        }
                    }
                }
                Completion::Uploaded(key, result) => {
                    if let Some(item) = self.items.iter_mut().find(|item| item.key == key) {
                        match result {
                            Ok(hash) => {
                                item.origin = None;
                                item.state.accept(&Input::Uploaded(hash));
                                item.revealed = Some(Instant::now());
                            }
                            Err(error) => {
                                fail(item, format!("Upload failed: {error}"));
                            }
                        }
                    }
                }
                Completion::Restored(key, result) => {
                    if let Some(item) = self.items.iter_mut().find(|item| item.key == key) {
                        match result {
                            Ok(bytes) => match assets.decode(bytes) {
                                Ok(preview) => item.pending_preview = Some(preview),
                                Err(error) => item.preview_error = Some(error.to_string()),
                            },
                            Err(error) => item.preview_error = Some(error),
                        }
                    }
                }
            }
        }
        for item in &mut self.items {
            let Some(preview) = &item.pending_preview else {
                continue;
            };
            match preview.status() {
                ImageStatus::Ready { .. }
                    if item.encoded.is_some() && matches!(item.state, State::Encoding) =>
                {
                    item.preview = item.pending_preview.take();
                    let bytes = item
                        .encoded
                        .take()
                        .expect("invariant: the guard checked the encoded bytes are present");
                    if item.state.accept(&Input::Encoded) {
                        let key = item.key;
                        let client = self.client.clone();
                        let sender = self.delivery.sender();
                        let task = runtime().spawn(async move {
                            let result = client
                                .put_blob(bytes)
                                .await
                                .map(|blob| blob.hash)
                                .map_err(|error| diagnostics::chain(&error));
                            sender.offer(Completion::Uploaded(key, result)).await;
                        });
                        item.task = Some(task.abort_handle());
                    }
                    changed = true;
                }
                ImageStatus::Ready { .. }
                    if item.state.hash().is_some() && item.revealed.is_none() =>
                {
                    item.preview = item.pending_preview.take();
                    item.revealed = Some(Instant::now());
                    changed = true;
                }
                ImageStatus::Failed(error) => {
                    abandon_preview(item, error.to_string());
                    changed = true;
                }
                ImageStatus::Cancelled => {
                    abandon_preview(item, "The preview was cancelled.".to_owned());
                    changed = true;
                }
                ImageStatus::Pending | ImageStatus::Ready { .. } => {}
            }
        }
        changed
    }
}

fn abandon_preview(item: &mut Attachment, reason: String) {
    if matches!(item.state, State::Encoding) {
        fail(item, reason);
    } else {
        item.preview_error = Some(reason);
    }
    item.pending_preview = None;
    item.encoded = None;
}

fn fail(item: &mut Attachment, reason: String) {
    tracing::warn!(key = item.key, name = %item.name, %reason, "pasted image failed");
    item.state.accept(&Input::Failed(reason));
}

#[cfg(test)]
#[path = "../../tests/composer/attachment.rs"]
mod tests;
