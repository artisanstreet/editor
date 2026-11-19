use crate::core::{delivery::Delivery, media_http, runtime};
use crate::ui::media::source::{self, MediaBytes, MediaLoadError, MediaSource};
use aqua::images::{ImageAssets, ImageHandle, ImageStatus, ImageSubscription};
use aqua::mailbox::Mailbox;
use std::{
    num::NonZeroUsize,
    sync::Arc,
    time::{Duration, Instant},
};
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
        error: AvatarError,
        at: Instant,
    },
}

const RELOAD_AFTER: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum AvatarError {
    #[error("fetch the avatar")]
    Fetch(#[source] MediaLoadError),
    #[error("decode the avatar")]
    Decode(#[source] aqua::images::ImageAssetError),
    #[error("the avatar decode was cancelled")]
    DecodeCancelled,
}

impl Phase {
    fn url(&self) -> Option<&str> {
        match self {
            Self::Absent => None,
            Self::Fetching { url, .. }
            | Self::Decoding { url, .. }
            | Self::Ready { url, .. }
            | Self::Failed { url, .. } => Some(url),
        }
    }
}

pub struct Avatar {
    phase: Phase,
    delivery: Delivery<Fetched>,
}

impl std::fmt::Debug for Avatar {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Avatar").finish_non_exhaustive()
    }
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
    pub(crate) fn mailbox(&self) -> Option<&Mailbox<()>> {
        self.delivery.wake_mailbox()
    }

    #[must_use]
    pub fn image(&self) -> Option<Arc<ImageHandle>> {
        match &self.phase {
            Phase::Ready { image, .. } => Some(Arc::clone(image)),
            Phase::Absent
            | Phase::Fetching { .. }
            | Phase::Decoding { .. }
            | Phase::Failed { .. } => None,
        }
    }

    #[must_use]
    pub const fn failure(&self) -> Option<&AvatarError> {
        match &self.phase {
            Phase::Failed { error, .. } => Some(error),
            Phase::Absent
            | Phase::Fetching { .. }
            | Phase::Decoding { .. }
            | Phase::Ready { .. } => None,
        }
    }

    pub fn want(&mut self, url: Option<&str>, now: Instant) {
        let reload = matches!(
            &self.phase,
            Phase::Failed { at, .. } if now.saturating_duration_since(*at) >= RELOAD_AFTER
        );
        if self.phase.url() == url && !reload {
            return;
        }
        self.phase = url.map_or(Phase::Absent, |url| {
            let sender = self.delivery.sender();
            let source = MediaSource::from(url.to_owned());
            let owned = url.to_owned();
            let task = runtime::runtime().spawn(async move {
                let result = source::load_image_bytes(&source, None, &media_http::client()).await;
                if sender.send(Fetched { url: owned, result }).await.is_err() {
                    tracing::debug!("the avatar was replaced before its image arrived");
                }
            });
            Phase::Fetching {
                url: url.to_owned(),
                _task: AbortOnDropHandle::new(task),
            }
        });
    }

    pub fn drain(&mut self, assets: &ImageAssets, now: Instant) -> bool {
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
                Ok(Err(error)) => Self::failed(fetched.url, AvatarError::Decode(error), now),
                Err(error) => Self::failed(fetched.url, AvatarError::Fetch(error), now),
            };
            changed = true;
        }
        let status = match &self.phase {
            Phase::Decoding { image, .. } => Some(image.status()),
            Phase::Absent | Phase::Fetching { .. } | Phase::Ready { .. } | Phase::Failed { .. } => {
                None
            }
        };
        let next = match (status, std::mem::replace(&mut self.phase, Phase::Absent)) {
            (Some(ImageStatus::Ready { .. }), Phase::Decoding { url, image, .. }) => {
                changed = true;
                Phase::Ready { url, image }
            }
            (Some(ImageStatus::Failed(error)), Phase::Decoding { url, .. }) => {
                changed = true;
                Self::failed(url, AvatarError::Decode(error), now)
            }
            (Some(ImageStatus::Cancelled), Phase::Decoding { url, .. }) => {
                changed = true;
                Self::failed(url, AvatarError::DecodeCancelled, now)
            }
            (_, phase) => phase,
        };
        self.phase = next;
        changed
    }

    fn failed(url: String, error: AvatarError, at: Instant) -> Phase {
        tracing::warn!(
            error = %::diagnostics::chain(&error),
            "the account avatar failed to load"
        );
        Phase::Failed { url, error, at }
    }
}
