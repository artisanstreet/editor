use super::{HostCompletion, HostData, HostMessage};
use aqua::images::{ImageAssets, ImageHandle, ImageStatus, WeakImageHandle};
use artisan_api::BlobHash;
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    rc::{Rc, Weak},
    sync::{Arc, Weak as ArcWeak},
};
use tokio_util::task::AbortOnDropHandle;

const DECODE_FAILURE: &str = "Image is invalid or exceeds preview limits (32 MiB source, 64 MiB decoded, 8192px edges, 120 animation frames)";

#[derive(Debug, Clone)]
pub enum BlobImage {
    Loading,
    Ready(Arc<ImageHandle>),
    Failed(Arc<str>),
}

enum Retained {
    Ready(WeakImageHandle),
    Failed(ArcWeak<str>),
}
impl Retained {
    fn upgrade(&self) -> Option<BlobImage> {
        match self {
            Self::Ready(image) => image
                .upgrade()
                .map(|image| BlobImage::Ready(Arc::new(image))),
            Self::Failed(error) => error.upgrade().map(BlobImage::Failed),
        }
    }
    fn alive(&self) -> bool {
        self.upgrade().is_some()
    }
}

#[derive(Default)]
struct ImageValues {
    kept: HashMap<BlobHash, BlobImage>,
    retained: HashMap<BlobHash, Retained>,
}

#[derive(Debug, Clone)]
pub struct BlobImages {
    values: Weak<RefCell<ImageValues>>,
}
impl BlobImages {
    pub fn lookup(&self, hash: &BlobHash) -> Option<BlobImage> {
        self.values.upgrade()?.borrow().lookup(hash)
    }
}
impl ImageValues {
    fn lookup(&self, hash: &BlobHash) -> Option<BlobImage> {
        self.kept
            .get(hash)
            .cloned()
            .or_else(|| self.retained.get(hash).and_then(Retained::upgrade))
    }
}

#[derive(Default)]
pub(super) struct ImageCache {
    values: Rc<RefCell<ImageValues>>,
    loads: HashMap<BlobHash, (u64, AbortOnDropHandle<()>)>,
    decoding: HashMap<BlobHash, (u64, AbortOnDropHandle<()>)>,
    order: VecDeque<BlobHash>,
    generation: u64,
    assets: Option<ImageAssets>,
}

impl HostData {
    pub(crate) fn set_image_assets(&mut self, assets: ImageAssets) {
        self.images.assets = Some(assets);
    }
    pub fn image(&self, hash: &BlobHash) -> Option<BlobImage> {
        self.images.values.borrow().lookup(hash)
    }
    pub fn blob_images(&self) -> BlobImages {
        BlobImages {
            values: Rc::downgrade(&self.images.values),
        }
    }
    pub fn load_image(&mut self, hash: BlobHash, retry: bool) {
        let mut values = self.images.values.borrow_mut();
        values.retained.retain(|_, result| result.alive());
        if retry && matches!(values.lookup(&hash), Some(BlobImage::Failed(_))) {
            values.retained.remove(&hash);
            values.kept.remove(&hash);
            self.images.order.retain(|old| old != &hash);
        }
        if matches!(values.kept.get(&hash), Some(BlobImage::Ready(_))) {
            self.images.order.retain(|old| old != &hash);
            self.images.order.push_back(hash);
            return;
        }
        if values.kept.contains_key(&hash)
            || values
                .retained
                .get(&hash)
                .and_then(Retained::upgrade)
                .is_some()
            || self.images.loads.len() + self.images.decoding.len() >= 4
        {
            return;
        }
        values.kept.insert(hash.clone(), BlobImage::Loading);
        drop(values);
        self.images.generation = self.images.generation.wrapping_add(1);
        let generation = self.images.generation;
        let client = self.client.clone();
        let sender = self.delivery.sender().map(HostMessage::completion);
        let key = hash.clone();
        let task_hash = hash.clone();
        let task = crate::core::runtime::runtime().spawn(async move {
            let result = client.blob(&task_hash).await;
            sender
                .offer(HostCompletion::Image(key, generation, result))
                .await;
        });
        self.images
            .loads
            .insert(hash, (generation, AbortOnDropHandle::new(task)));
    }
    pub(super) fn finish_image_download(
        &mut self,
        hash: BlobHash,
        generation: u64,
        result: Result<Vec<u8>, crate::core::client::CallError>,
    ) -> bool {
        if self
            .images
            .loads
            .get(&hash)
            .is_none_or(|(current, _)| *current != generation)
        {
            return false;
        }
        self.images.loads.remove(&hash);
        match (result, self.images.assets.as_ref()) {
            (Ok(bytes), Some(_)) if bytes.len() > 32 * 1024 * 1024 => {
                self.finish_image(hash, Err(DECODE_FAILURE.into()));
                true
            }
            (Ok(bytes), Some(assets)) => match assets.decode_preview(Arc::<[u8]>::from(bytes)) {
                Ok(handle) => {
                    let assets = assets.clone();
                    self.watch_decode(hash, generation, &assets, handle);
                    false
                }
                Err(error) => {
                    tracing::debug!(host = %self.id, image = %hash, %error, "couldn't start decoding an image");
                    self.finish_image(hash, Err(DECODE_FAILURE.into()));
                    true
                }
            },
            (Ok(_), None) => {
                self.finish_image(hash, Err("This Editor can't decode images.".into()));
                true
            }
            (Err(error), _) => {
                self.finish_image(hash, Err(::diagnostics::chain(&error)));
                true
            }
        }
    }
    fn watch_decode(
        &mut self,
        hash: BlobHash,
        generation: u64,
        assets: &ImageAssets,
        handle: ImageHandle,
    ) {
        let notify = Arc::new(tokio::sync::Notify::new());
        let signal = Arc::clone(&notify);
        let subscription = assets.subscribe(Arc::new(move || signal.notify_one()));
        let sender = self.delivery.sender().map(HostMessage::completion);
        let task_hash = hash.clone();
        let task = crate::core::runtime::runtime().spawn(async move {
            let _subscription = subscription;
            let result = loop {
                match handle.status() {
                    ImageStatus::Pending => notify.notified().await,
                    ImageStatus::Ready { .. } => break Ok(Arc::new(handle)),
                    ImageStatus::Failed(error) => {
                        tracing::debug!(image = %task_hash, %error, "couldn't decode an image");
                        break Err(DECODE_FAILURE.to_owned());
                    }
                    ImageStatus::Cancelled => break Err(DECODE_FAILURE.to_owned()),
                }
            };
            sender
                .offer(HostCompletion::DecodedImage(task_hash, generation, result))
                .await;
        });
        self.images
            .decoding
            .insert(hash, (generation, AbortOnDropHandle::new(task)));
    }
    pub(super) fn finish_decoded_image(
        &mut self,
        hash: BlobHash,
        generation: u64,
        result: Result<Arc<ImageHandle>, String>,
    ) -> bool {
        if self
            .images
            .decoding
            .get(&hash)
            .is_none_or(|(current, _)| *current != generation)
        {
            return false;
        }
        self.images.decoding.remove(&hash);
        self.finish_image(hash, result);
        true
    }
    fn finish_image(&mut self, hash: BlobHash, result: Result<Arc<ImageHandle>, String>) {
        let mut values = self.images.values.borrow_mut();
        values.retained.remove(&hash);
        values.kept.insert(
            hash.clone(),
            match result {
                Ok(image) => BlobImage::Ready(image),
                Err(error) => BlobImage::Failed(error.into()),
            },
        );
        self.images.order.push_back(hash);
        while values.kept.len() > 32
            || values
                .kept
                .values()
                .filter_map(|image| match image {
                    BlobImage::Ready(image) => image.decoded_bytes(),
                    BlobImage::Loading | BlobImage::Failed(_) => None,
                })
                .sum::<u64>()
                > 16 * 1024 * 1024
        {
            let Some(oldest) = self.images.order.pop_front() else {
                break;
            };
            match values.kept.remove(&oldest) {
                Some(BlobImage::Ready(image)) => {
                    values
                        .retained
                        .insert(oldest, Retained::Ready(image.downgrade()));
                }
                Some(BlobImage::Failed(error)) => {
                    values
                        .retained
                        .insert(oldest, Retained::Failed(Arc::downgrade(&error)));
                }
                Some(BlobImage::Loading) | None => {}
            }
        }
        values.retained.retain(|_, result| result.alive());
    }
}
