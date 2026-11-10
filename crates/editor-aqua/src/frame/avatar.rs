use crate::core::{delivery::Delivery, media_http, runtime};
use crate::ui::media::source::{self, MediaBytes, MediaLoadError, MediaSource};
use aqua::images::{ImageAssets, ImageHandle, ImageStatus, ImageSubscription};
use aqua::mailbox::Mailbox;
use std::{num::NonZeroUsize, sync::Arc};
use tokio_util::task::AbortOnDropHandle;

struct Fetched {
    url: String,
    result: Result<MediaBytes, MediaLoadError>,
}

enum Phase {
    Absent,
    Fetching {
        url: String,
        _task: AbortOnDropHandle<()>,
    },
    Decoding {
        url: String,
        image: Arc<ImageHandle>,
        _subscription: ImageSubscription,
    },
    Ready {
        url: String,
        image: Arc<ImageHandle>,
    },
    Failed {
        url: String,
    },
}

impl Phase {
    fn url(&self) -> Option<&str> {
        match self {
            Self::Absent => None,
            Self::Fetching { url, .. }
            | Self::Decoding { url, .. }
            | Self::Ready { url, .. }
            | Self::Failed { url } => Some(url),
        }
    }
}

pub struct Avatar {
    phase: Phase,
    delivery: Delivery<Fetched>,
}

impl Default for Avatar {
    fn default() -> Self {
        Self {
            phase: Phase::Absent,
            delivery: Delivery::new(NonZeroUsize::MIN),
        }
    }
}

impl Avatar {
    #[must_use]
    pub fn mailbox(&self) -> Option<&Mailbox<()>> {
        self.delivery.wake_mailbox()
    }

    #[must_use]
    pub fn image(&self) -> Option<Arc<ImageHandle>> {
        match &self.phase {
            Phase::Ready { image, .. } => Some(Arc::clone(image)),
            _ => None,
        }
    }

    pub fn want(&mut self, url: Option<&str>) {
        if self.phase.url() == url {
            return;
        }
        self.phase = url.map_or(Phase::Absent, |url| {
            let sender = self.delivery.sender();
            let source = MediaSource::from(url.to_owned());
            let owned = url.to_owned();
            let task = runtime::runtime().spawn(async move {
                let result = source::load_image_bytes(&source, None, &media_http::client()).await;
                let _ = sender.send(Fetched { url: owned, result }).await;
            });
            Phase::Fetching {
                url: url.to_owned(),
                _task: AbortOnDropHandle::new(task),
            }
        });
    }

    pub fn drain(&mut self, assets: &ImageAssets) -> bool {
        let mut changed = false;
        for fetched in self.delivery.drain() {
            if self.phase.url() != Some(fetched.url.as_str()) {
                continue;
            }
            self.phase = match fetched.result.map(|bytes| bytes.decode(assets)) {
                Ok(Ok(image)) => Phase::Decoding {
                    url: fetched.url,
                    _subscription: assets.subscribe(self.delivery.waker()),
                    image: Arc::new(image),
                },
                Ok(Err(_)) | Err(_) => Phase::Failed { url: fetched.url },
            };
            changed = true;
        }
        let status = match &self.phase {
            Phase::Decoding { image, .. } => Some(image.status()),
            _ => None,
        };
        let next = match (status, std::mem::replace(&mut self.phase, Phase::Absent)) {
            (Some(ImageStatus::Ready { .. }), Phase::Decoding { url, image, .. }) => {
                changed = true;
                Phase::Ready { url, image }
            }
            (
                Some(ImageStatus::Failed(_) | ImageStatus::Cancelled),
                Phase::Decoding { url, .. },
            ) => {
                changed = true;
                Phase::Failed { url }
            }
            (_, phase) => phase,
        };
        self.phase = next;
        changed
    }
}
