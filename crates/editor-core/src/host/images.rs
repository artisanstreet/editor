//! Decoded blob images of one Forge, bounded by count and bytes. The cache
//! owns its downloads; evicted pixels stay reachable while a mounted view
//! still holds them, so scrolling back never decodes them twice.
use super::{HostData, HostEvent};
use crate::client::ForgeClient;
use artisan_api::BlobHash;
use gpui::{App, Context, Global, RenderImage, SvgRenderer, Task};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Weak};

/// At most this many decoded images are kept…
const KEPT: usize = 32;
/// …and at most this many pixel bytes, about one full-screen screenshot.
/// Shown images stay alive through their rows whatever this says, and the
/// GPU holds a second copy of every kept one, so this only bounds what
/// scrolling back can show without decoding again.
const KEPT_BYTES: usize = 16 * 1024 * 1024;
/// Downloads and decodes at once.
const LOADS: usize = 4;

#[derive(Clone)]
pub enum BlobImage {
    Loading,
    Ready(Arc<RenderImage>),
    Failed(Arc<str>),
}

/// Turns blob bytes into pixels. The application installs it (decoding
/// lives with the UI); without one, images fail to load.
#[derive(Clone)]
pub struct ImageDecoder(pub Arc<Decode>);
/// Bytes and the SVG renderer in, pixels or why not out.
pub type Decode = dyn Fn(&[u8], SvgRenderer) -> Result<Arc<RenderImage>, String> + Send + Sync;
impl Global for ImageDecoder {}

/// A lookup never retains result bytes. Mounted rows can keep an evicted
/// result alive; reusing it avoids another download or a retry loop.
pub(super) enum Retained {
    Ready(Weak<RenderImage>),
    Failed(Weak<str>),
}
impl Retained {
    fn upgrade(&self) -> Option<BlobImage> {
        match self {
            Self::Ready(image) => image.upgrade().map(BlobImage::Ready),
            Self::Failed(error) => error.upgrade().map(BlobImage::Failed),
        }
    }
    fn alive(&self) -> bool {
        match self {
            Self::Ready(image) => image.strong_count() > 0,
            Self::Failed(error) => error.strong_count() > 0,
        }
    }
}

#[derive(Default)]
pub(super) struct ImageCache {
    pub(super) kept: HashMap<BlobHash, BlobImage>,
    pub(super) loads: HashMap<BlobHash, Task<()>>,
    pub(super) retained: HashMap<BlobHash, Retained>,
    /// Least recently used first.
    pub(super) order: VecDeque<BlobHash>,
}

/// CPU pixel bytes an image keeps.
pub(super) fn resident_bytes(image: &RenderImage) -> usize {
    (0..image.frame_count())
        .map(|frame| image.as_bytes(frame).map_or(0, <[u8]>::len))
        .sum()
}

impl ImageCache {
    pub(super) fn get(&self, hash: &BlobHash) -> Option<BlobImage> {
        self.kept
            .get(hash)
            .cloned()
            .or_else(|| self.retained.get(hash).and_then(Retained::upgrade))
    }

    /// Hands the GPU copies of every kept image back as the host goes.
    pub(super) fn release(&mut self, cx: &mut App) {
        for (_, image) in self.kept.drain() {
            if let BlobImage::Ready(image) = image {
                cx.defer(move |cx| {
                    cx.drop_image(image, None);
                    // Other owners can still show these pixels: invalidate
                    // cached scenes before they reuse a tile.
                    cx.refresh_windows();
                });
            }
        }
    }

    /// Starts loading `hash` unless it is kept, loading or still shown.
    /// Only `retry` loads a failed image again.
    pub(super) fn load(
        &mut self,
        hash: BlobHash,
        retry: bool,
        client: &ForgeClient,
        cx: &Context<HostData>,
    ) {
        self.retained.retain(|_, result| result.alive());
        if retry && matches!(self.get(&hash), Some(BlobImage::Failed(_))) {
            self.retained.remove(&hash);
            self.kept.remove(&hash);
            self.order.retain(|old| old != &hash);
        }
        if matches!(self.kept.get(&hash), Some(BlobImage::Ready(_))) {
            self.order.retain(|old| old != &hash);
            self.order.push_back(hash);
            return;
        }
        if self.kept.contains_key(&hash)
            || self
                .retained
                .get(&hash)
                .and_then(Retained::upgrade)
                .is_some()
            || self.loads.len() >= LOADS
        {
            return;
        }
        self.kept.insert(hash.clone(), BlobImage::Loading);
        let client = client.clone();
        let renderer = cx.svg_renderer();
        let decoder = cx.try_global::<ImageDecoder>().cloned();
        let key = hash.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = match (client.blob(&hash).await, decoder) {
                (Ok(bytes), Some(ImageDecoder(decode))) => {
                    cx.background_executor()
                        .spawn(async move { decode(&bytes, renderer) })
                        .await
                }
                (Ok(_), None) => Err("This Editor can't decode images.".to_owned()),
                (Err(error), _) => Err(format!("{error:#}")),
            };
            this.update(cx, |host, cx| {
                host.images.finish(hash, result, cx);
                cx.emit(HostEvent::ImagesChanged);
                cx.notify();
            })
            .ok();
        });
        self.loads.insert(key, task);
    }

    fn finish(
        &mut self,
        hash: BlobHash,
        result: Result<Arc<RenderImage>, String>,
        cx: &mut Context<HostData>,
    ) {
        self.loads.remove(&hash);
        self.retained.remove(&hash);
        self.kept.insert(
            hash.clone(),
            match result {
                Ok(image) => BlobImage::Ready(image),
                Err(error) => BlobImage::Failed(error.into()),
            },
        );
        self.order.push_back(hash);
        while self.kept.len() > KEPT || self.kept_bytes() > KEPT_BYTES {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            match self.kept.remove(&oldest) {
                Some(BlobImage::Ready(image)) => {
                    self.retained
                        .insert(oldest, Retained::Ready(Arc::downgrade(&image)));
                    cx.defer(move |cx| {
                        cx.drop_image(image, None);
                        // Retained scenes need fresh GPU tiles for these shared pixels.
                        cx.refresh_windows();
                    });
                }
                Some(BlobImage::Failed(error)) => {
                    self.retained
                        .insert(oldest, Retained::Failed(Arc::downgrade(&error)));
                }
                _ => {}
            }
        }
        self.retained.retain(|_, result| result.alive());
    }

    pub(super) fn kept_bytes(&self) -> usize {
        self.kept
            .values()
            .filter_map(|image| match image {
                BlobImage::Ready(image) => Some(resident_bytes(image)),
                BlobImage::Loading | BlobImage::Failed(_) => None,
            })
            .sum()
    }
}
