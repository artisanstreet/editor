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

#[derive(Clone)]
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

#[derive(Clone)]
pub struct BlobImages {
    values: Weak<RefCell<ImageValues>>,
}
impl BlobImages {
    pub fn lookup(&self, hash: &BlobHash) -> Option<BlobImage> {
        let values = self.values.upgrade()?;
        let result = values.borrow().lookup(hash);
        result
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
    #[cfg(feature = "test-support")]
    pub fn test_begin_image(&mut self, hash: BlobHash) -> u64 {
        self.images.generation += 1;
        let generation = self.images.generation;
        self.images
            .values
            .borrow_mut()
            .kept
            .insert(hash.clone(), BlobImage::Loading);
        let task = crate::core::runtime::runtime().spawn(std::future::pending());
        self.images
            .decoding
            .insert(hash, (generation, AbortOnDropHandle::new(task)));
        generation
    }
    pub fn set_image_assets(&mut self, assets: ImageAssets) {
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
        let sender = self.delivery.sender().map(HostMessage::Completion);
        let key = hash.clone();
        let task_hash = hash.clone();
        let task = crate::core::runtime::runtime().spawn(async move {
            let result = client.blob(&task_hash).await;
            let _ = sender
                .send(HostCompletion::Image(key, generation, result))
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
        result: anyhow::Result<Vec<u8>>,
    ) -> bool {
        if !self
            .images
            .loads
            .get(&hash)
            .is_some_and(|(current, _)| *current == generation)
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
                    let notify = Arc::new(tokio::sync::Notify::new());
                    let signal = Arc::clone(&notify);
                    let subscription = assets.subscribe(Arc::new(move || signal.notify_one()));
                    let sender = self.delivery.sender().map(HostMessage::Completion);
                    let task_hash = hash.clone();
                    let task = crate::core::runtime::runtime().spawn(async move {
                        let _subscription = subscription;
                        loop {
                            let result = match handle.status() {
                                ImageStatus::Pending => {
                                    notify.notified().await;
                                    continue;
                                }
                                ImageStatus::Ready { .. } => Ok(Arc::new(handle)),
                                ImageStatus::Failed(_) | ImageStatus::Cancelled => {
                                    Err(DECODE_FAILURE.to_owned())
                                }
                            };
                            let _ = sender
                                .send(HostCompletion::DecodedImage(task_hash, generation, result))
                                .await;
                            break;
                        }
                    });
                    self.images
                        .decoding
                        .insert(hash, (generation, AbortOnDropHandle::new(task)));
                    false
                }
                Err(_) => {
                    self.finish_image(hash, Err(DECODE_FAILURE.into()));
                    true
                }
            },
            (Ok(_), None) => {
                self.finish_image(hash, Err("This Editor can't decode images.".into()));
                true
            }
            (Err(error), _) => {
                self.finish_image(hash, Err(format!("{error:#}")));
                true
            }
        }
    }
    pub(super) fn finish_decoded_image(
        &mut self,
        hash: BlobHash,
        generation: u64,
        result: Result<Arc<ImageHandle>, String>,
    ) -> bool {
        if !self
            .images
            .decoding
            .get(&hash)
            .is_some_and(|(current, _)| *current == generation)
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
                    _ => None,
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
                _ => {}
            }
        }
        values.retained.retain(|_, result| result.alive());
    }
}
