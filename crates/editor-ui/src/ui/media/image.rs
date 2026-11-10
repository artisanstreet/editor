use std::{cell::RefCell, collections::HashMap, num::NonZeroUsize, rc::Rc, sync::Arc};

use aqua::images::{ImageAssets, ImageHandle, ImageStatus};
use tokio::task::AbortHandle;

use super::{
    resource_cache::{Load, LoadTicket, ResourceLoader},
    source::{self, MediaBytes, MediaFetcher, MediaLoadError, MediaSource, Owned, Parsed},
};
use editor_core::{delivery::Delivery, media_http, runtime};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageError {
    Load(MediaLoadError),
    Decode,
}

pub struct ImageLoader;

impl ResourceLoader for ImageLoader {
    type Source = Owned<Parsed>;
    type Output = Result<ImageHandle, ImageError>;
}

pub(crate) fn cache_key(fetcher: &MediaFetcher, source: &MediaSource, part: &str) -> Owned<Parsed> {
    let immutable = matches!(source.parsed, Parsed::Data(_));
    Owned::new(fetcher.asset_owner(part, immutable), source.parsed.clone())
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
    jobs: HashMap<u64, Job>,
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
                jobs: HashMap::new(),
                delivery: Delivery::with_wake(NonZeroUsize::new(16).unwrap(), mailbox.sender()),
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
        let key = cache_key(fetcher, source, part);
        let source_bytes = match &source.parsed {
            Parsed::Data(payload) => payload.bytes.len(),
            _ => source.raw.len(),
        };
        match fetcher.resources().load::<ImageLoader>(
            fetcher.owner(),
            &key,
            source_bytes,
            waiter,
            wake,
        ) {
            Load::Ready(result) => Some(result),
            Load::Pending => {
                let mut state = self.state.borrow_mut();
                if let Some(job) = state
                    .jobs
                    .values_mut()
                    .find(|job| job.cache_id == fetcher.resources().id() && job.key == key)
                {
                    if !job.owners.iter().any(|owner| owner == fetcher.owner()) {
                        job.owners.push(Arc::clone(fetcher.owner()));
                    }
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
                    let _ = sender.send(Completion { id, result }).await;
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
            let ready: Vec<_> = state
                .jobs
                .iter()
                .filter_map(|(id, job)| match &job.phase {
                    Phase::Decoding(image) if !matches!(image.status(), ImageStatus::Pending) => {
                        Some(*id)
                    }
                    _ => None,
                })
                .collect();
            ready
                .into_iter()
                .filter_map(|id| state.jobs.remove(&id))
                .collect::<Vec<_>>()
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

    pub fn retain_owners(&self, owners: &[Vec<u64>]) {
        self.state.borrow_mut().jobs.retain(|_, job| {
            job.owners
                .retain(|known| owners.iter().any(|owner| known.starts_with(owner)));
            let keep = !job.owners.is_empty();
            if !keep {
                job.ticket.cancel();
                job.abort.abort();
            }
            keep
        });
    }

    pub fn clear(&self) {
        let mut state = self.state.borrow_mut();
        for (_, job) in state.jobs.drain() {
            job.ticket.cancel();
            job.abort.abort();
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/ui/media/image.rs"]
mod tests;
