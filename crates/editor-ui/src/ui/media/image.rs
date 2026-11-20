use std::{cell::RefCell, collections::BTreeMap, num::NonZeroUsize, rc::Rc, sync::Arc};

use aqua::images::{ImageAssets, ImageHandle, ImageStatus};
use tokio::task::AbortHandle;

use super::{
    resource_cache::{Load, LoadTicket, ResourceLoader},
    source::{self, MediaBytes, MediaFetcher, MediaLoadError, MediaSource, Owned, Parsed},
};
use editor_core::{delivery::Delivery, media_http, runtime};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageError {
    NeedsConsent,
    Load(MediaLoadError),
    Decode,
}

impl ImageError {
    #[must_use]
    pub(crate) const fn summary(self) -> &'static str {
        match self {
            Self::NeedsConsent => "Remote image",
            Self::Load(MediaLoadError::Invalid) => "Not a valid image path",
            Self::Load(MediaLoadError::TooLarge) => "Too large to preview",
            Self::Load(MediaLoadError::Fetch(source::MediaFailure::Missing)) => "File not found",
            Self::Load(MediaLoadError::Fetch(source::MediaFailure::ForgeUnreachable)) => {
                "Forge didn't answer"
            }
            Self::Load(MediaLoadError::Fetch(source::MediaFailure::ServerUnreachable)) => {
                "The image's server didn't answer"
            }
            Self::Load(MediaLoadError::Fetch(source::MediaFailure::Unreadable)) => {
                "Couldn't read the file"
            }
            Self::Decode => "Not a supported image",
        }
    }

    #[must_use]
    pub(crate) const fn retryable(self) -> bool {
        matches!(self, Self::Load(MediaLoadError::Fetch(_)) | Self::Decode)
    }
}

#[derive(Debug)]
pub struct ImageLoader;

impl ResourceLoader for ImageLoader {
    type Source = Owned<Parsed>;
    type Output = Result<ImageHandle, ImageError>;
}

pub(crate) fn cache_key(fetcher: &MediaFetcher, source: &MediaSource, part: &str) -> Owned<Parsed> {
    let immutable = matches!(source.parsed(), Parsed::Data(_));
    Owned::new(
        fetcher.asset_owner(part, immutable),
        source.parsed().clone(),
    )
}

struct Completion {
    id: u64,
    result: Result<MediaBytes, MediaLoadError>,
}

enum Phase {
    Fetching,
    Decoding(ImageHandle),
}

struct Job {
    owners: Vec<Arc<[u64]>>,
    cache_id: u64,
    key: Owned<Parsed>,
    ticket: LoadTicket<ImageLoader>,
    abort: AbortHandle,
    phase: Phase,
}

struct State {
    next: u64,
    jobs: BTreeMap<u64, Job>,
    delivery: Delivery<Completion>,
}

impl Drop for State {
    fn drop(&mut self) {
        for job in self.jobs.values() {
            job.ticket.cancel();
            job.abort.abort();
        }
    }
}

#[derive(Clone)]
pub struct Images {
    state: Rc<RefCell<State>>,
    mailbox: Rc<aqua::mailbox::Mailbox<()>>,
}

impl std::fmt::Debug for Images {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Images").finish_non_exhaustive()
    }
}

impl Default for Images {
    fn default() -> Self {
        Self::new()
    }
}

impl Images {
    pub fn new() -> Self {
        let mailbox = Rc::new(aqua::mailbox::Mailbox::new(NonZeroUsize::MIN));
        Self {
            state: Rc::new(RefCell::new(State {
                next: 0,
                jobs: BTreeMap::new(),
                delivery: Delivery::with_wake(
                    const {
                        NonZeroUsize::new(16)
                            .expect("invariant: the delivery capacity 16 is nonzero")
                    },
                    mailbox.sender(),
                ),
            })),
            mailbox,
        }
    }

    pub fn wake_mailbox(&self) -> &aqua::mailbox::Mailbox<()> {
        &self.mailbox
    }

    pub fn request(
        &self,
        fetcher: &MediaFetcher,
        source: &MediaSource,
        part: &str,
        waiter: u64,
        wake: aqua::mailbox::Wake,
    ) -> Option<Result<ImageHandle, ImageError>> {
        self.discard_stale();
        if fetcher.waits_for_consent(source) {
            return Some(Err(ImageError::NeedsConsent));
        }
        let key = cache_key(fetcher, source, part);
        let source_bytes = match source.parsed() {
            Parsed::Data(payload) => payload.bytes.len(),
            Parsed::Url(_) | Parsed::File { .. } | Parsed::Invalid => source.raw().len(),
        };
        match fetcher.resources().load::<ImageLoader>(
            fetcher.owner(),
            &key,
            source_bytes,
            waiter,
            wake,
        ) {
            Load::Ready(result) => Some(result),
            Load::TooLarge => Some(Err(ImageError::Load(MediaLoadError::TooLarge))),
            Load::Pending => {
                let mut state = self.state.borrow_mut();
                if let Some(job) = state
                    .jobs
                    .values_mut()
                    .find(|job| job.cache_id == fetcher.resources().id() && job.key == key)
                    && !job.owners.iter().any(|owner| owner == fetcher.owner())
                {
                    job.owners.push(Arc::clone(fetcher.owner()));
                }
                None
            }
            Load::Start(ticket) => {
                let mut state = self.state.borrow_mut();
                let id = state.next;
                state.next = state.next.wrapping_add(1);
                let sender = state.delivery.sender();
                let source = source.clone();
                let provider = fetcher.provider();
                let task = runtime::runtime().spawn(async move {
                    let scope = provider.scope().to_owned();
                    let transport =
                        MediaFetcher::new(scope, move |request| provider.fetch(request));
                    let result =
                        source::load_image_bytes(&source, Some(&transport), &media_http::client())
                            .await;
                    sender.offer(Completion { id, result }).await;
                });
                let abort = task.abort_handle();
                if ticket.attach(abort.clone()) {
                    state.jobs.insert(
                        id,
                        Job {
                            owners: vec![Arc::clone(fetcher.owner())],
                            cache_id: fetcher.resources().id(),
                            key,
                            ticket,
                            abort,
                            phase: Phase::Fetching,
                        },
                    );
                }
                None
            }
        }
    }

    pub fn drain(&self, assets: &ImageAssets) -> bool {
        let completions = self.state.borrow_mut().delivery.drain();
        let mut changed = false;
        for completion in completions {
            let Some(mut job) = self.state.borrow_mut().jobs.remove(&completion.id) else {
                continue;
            };
            if !job.ticket.current() {
                job.abort.abort();
                continue;
            }
            match completion.result {
                Ok(bytes) => match bytes.decode(assets) {
                    Ok(image) => {
                        job.phase = Phase::Decoding(image);
                        self.state.borrow_mut().jobs.insert(completion.id, job);
                    }
                    Err(_) => {
                        job.ticket.publish(Err(ImageError::Decode), 0, drop);
                    }
                },
                Err(error) => {
                    job.ticket.publish(Err(ImageError::Load(error)), 0, drop);
                }
            }
            changed = true;
        }
        self.poll_assets() || changed
    }

    pub fn poll_assets(&self) -> bool {
        self.discard_stale();
        let jobs = {
            let mut state = self.state.borrow_mut();
            let (ready, waiting): (BTreeMap<u64, Job>, BTreeMap<u64, Job>) =
                std::mem::take(&mut state.jobs)
                    .into_iter()
                    .partition(|(_, job)| match &job.phase {
                        Phase::Decoding(image) => !matches!(image.status(), ImageStatus::Pending),
                        Phase::Fetching => false,
                    });
            state.jobs = waiting;
            ready.into_values().collect::<Vec<_>>()
        };
        let changed = !jobs.is_empty();
        for job in jobs {
            if let Phase::Decoding(image) = job.phase {
                if matches!(image.status(), ImageStatus::Ready { .. }) {
                    let bytes = image
                        .decoded_bytes()
                        .and_then(|bytes| usize::try_from(bytes).ok())
                        .unwrap_or(usize::MAX);
                    job.ticket.publish(Ok(image), bytes, drop);
                } else {
                    job.ticket.publish(Err(ImageError::Decode), 0, drop);
                }
            }
        }
        changed
    }

    fn discard_stale(&self) {
        self.state.borrow_mut().jobs.retain(|_, job| {
            let current = job.ticket.current();
            if !current {
                job.abort.abort();
            }
            current
        });
    }

    pub fn clear(&self) {
        let mut state = self.state.borrow_mut();
        for job in std::mem::take(&mut state.jobs).into_values() {
            job.ticket.cancel();
            job.abort.abort();
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/ui/media/image.rs"]
mod tests;
