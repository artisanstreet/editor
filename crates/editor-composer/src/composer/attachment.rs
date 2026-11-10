use artisan_api::BlobHash;
pub enum Input {
    Encoded,
    Uploaded(BlobHash),
    Failed(String),
}
#[derive(Clone, PartialEq, Eq)]
pub enum State {
    Encoding,
    Uploading,
    Ready(BlobHash),
    Failed(String),
}
impl State {
    pub const fn hash(&self) -> Option<&BlobHash> {
        match self {
            Self::Ready(hash) => Some(hash),
            Self::Encoding | Self::Uploading | Self::Failed(_) => None,
        }
    }
    pub fn failure(&self) -> Option<&str> {
        match self {
            Self::Failed(reason) => Some(reason),
            Self::Encoding | Self::Uploading | Self::Ready(_) => None,
        }
    }
    pub fn accept(&mut self, input: &Input) -> bool {
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
            _ => false,
        }
    }
}

use editor_core::{ForgeClient, delivery::Delivery, runtime::runtime};
use aqua::images::{ImageAssets, ImageHandle, ImageStatus};
use std::{
    hash::{Hash, Hasher},
    num::NonZeroUsize,
    path::PathBuf,
    sync::OnceLock,
    time::Instant,
};

pub enum Source {
    Bytes(Vec<u8>),
    Path(PathBuf),
    Clipboard(aqua_ui::input::PasteImage),
}

impl Source {
    fn read(self) -> anyhow::Result<Vec<u8>> {
        use anyhow::Context as _;
        use std::io::Read as _;
        let bytes = match self {
            Self::Bytes(bytes) => bytes,
            Self::Clipboard(image) => {
                use image::ImageEncoder as _;
                let mut bytes = Vec::new();
                image::codecs::png::PngEncoder::new(&mut bytes).write_image(
                    image.rgba(),
                    u32::try_from(image.width())?,
                    u32::try_from(image.height())?,
                    image::ExtendedColorType::Rgba8,
                )?;
                bytes
            }
            Self::Path(path) => {
                let file = std::fs::File::open(&path)
                    .with_context(|| format!("read {}", path.display()))?;
                anyhow::ensure!(
                    file.metadata()?.len() <= super::encode::MAX_INPUT_BYTES as u64,
                    "Images must be at most 32 MiB"
                );
                let mut bytes = Vec::new();
                file.take((super::encode::MAX_INPUT_BYTES + 1) as u64)
                    .read_to_end(&mut bytes)?;
                bytes
            }
        };
        anyhow::ensure!(
            bytes.len() <= super::encode::MAX_INPUT_BYTES,
            "Images must be at most 32 MiB"
        );
        Ok(bytes)
    }
}

pub struct Attachment {
    pub key: u64,
    pub name: String,
    pub state: State,
    pub preview: Option<ImageHandle>,
    pending_preview: Option<ImageHandle>,
    pub preview_error: Option<String>,
    pub revealed: Option<Instant>,
    pub shown: bool,
    source: Option<(u64, usize)>,
    encoded: Option<Vec<u8>>,
    task: Option<tokio::task::AbortHandle>,
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

pub struct Attachments {
    pub items: Vec<Attachment>,
    pub shake: Option<(u64, Instant)>,
    pub notice: Option<String>,
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
            delivery: Delivery::new(NonZeroUsize::new(32).unwrap()),
        }
    }
    pub fn wake_mailbox(&self) -> Option<&aqua::mailbox::Mailbox<()>> {
        self.delivery.wake_mailbox()
    }
    pub fn ready_hashes(&self) -> Option<Vec<BlobHash>> {
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
            encoded: None,
            task: None,
        });
        let sender = self.delivery.sender();
        let task = runtime().spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                source.read().map(|bytes| {
                    let placeholder = super::encode::placeholder(&bytes);
                    (bytes, placeholder)
                })
            })
            .await
            .map_err(|error| error.to_string())
            .and_then(|result| result.map_err(|error| format!("{error:#}")));
            let _ = sender.send(Completion::Read(key, result)).await;
        });
        if let Some(item) = self.items.last_mut() {
            item.task = Some(task.abort_handle());
        }
        true
    }
    pub fn restore(&mut self, hash: BlobHash) {
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
                .map_err(|error| format!("{error:#}"));
            let _ = sender.send(Completion::Restored(key, result)).await;
        });
    }
    pub fn retry_preview(&mut self, key: u64) {
        let Some(item) = self.items.iter_mut().find(|item| item.key == key) else {
            return;
        };
        let Some(hash) = item.state.hash().cloned() else {
            return;
        };
        item.preview_error = None;
        self.load_preview(key, hash);
    }
    pub fn remove(&mut self, key: u64) {
        self.items.retain(|item| item.key != key);
    }
    pub fn clear(&mut self) {
        self.items.clear();
    }
    pub fn drain(&mut self, assets: &ImageAssets) -> bool {
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
                            aqua::images::DecodedImage::from_rgba(
                                width,
                                height,
                                image.into_raw(),
                            )
                            .ok()
                            .and_then(|image| assets.insert(image).ok())
                        });
                        item.shown = true;
                        let sender = self.delivery.sender();
                        let task = runtime().spawn(async move {
                            static CONVERSION: OnceLock<std::sync::Arc<tokio::sync::Semaphore>> =
                                OnceLock::new();
                            let permit = CONVERSION
                                .get_or_init(|| std::sync::Arc::new(tokio::sync::Semaphore::new(1)))
                                .clone()
                                .acquire_owned()
                                .await;
                            let result = match permit {
                                Ok(permit) => tokio::task::spawn_blocking(move || {
                                    let _permit = permit;
                                    super::encode::to_webp(&bytes)
                                })
                                .await
                                .map_err(|error| error.to_string())
                                .and_then(|result| result.map_err(|error| format!("{error:#}"))),
                                Err(error) => Err(error.to_string()),
                            };
                            let _ = sender.send(Completion::Converted(key, result)).await;
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
                    let bytes = item.encoded.take().unwrap();
                    if item.state.accept(&Input::Encoded) {
                        let key = item.key;
                        let client = self.client.clone();
                        let sender = self.delivery.sender();
                        let task = runtime().spawn(async move {
                            let result = client
                                .put_blob(bytes)
                                .await
                                .map(|blob| blob.hash)
                                .map_err(|error| format!("{error:#}"));
                            let _ = sender.send(Completion::Uploaded(key, result)).await;
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
                    if matches!(item.state, State::Encoding) {
                        fail(item, error.to_string());
                    } else {
                        item.preview_error = Some(error.to_string());
                    }
                    item.pending_preview = None;
                    changed = true;
                }
                _ => {}
            }
        }
        changed
    }
}

fn fail(item: &mut Attachment, reason: String) {
    tracing::warn!(key = item.key, name = %item.name, %reason, "pasted image failed");
    item.state.accept(&Input::Failed(reason));
}
