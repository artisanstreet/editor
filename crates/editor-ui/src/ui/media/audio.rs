use super::{
    resource_cache::{Load, LoadTicket, ResourceCache, ResourceLoader},
    source::{self, MediaBytes, MediaFetcher, MediaSource, Owned, Parsed},
};
use editor_core::{delivery::Delivery, media_http, runtime};
use std::{cell::RefCell, collections::HashMap, num::NonZeroUsize, rc::Rc, sync::Arc};
use tokio::task::AbortHandle;

struct Loader;
impl ResourceLoader for Loader {
    type Source = Owned<Parsed>;
    type Output = Option<Arc<[u8]>>;
}
struct Completion {
    id: u64,
    bytes: Option<MediaBytes>,
}
struct Job {
    ticket: LoadTicket<Loader>,
    abort: AbortHandle,
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
pub struct AudioAssets {
    state: Rc<RefCell<State>>,
    mailbox: Rc<RefCell<aqua::mailbox::Mailbox<()>>>,
    fallback: Arc<ResourceCache>,
}
impl Default for AudioAssets {
    fn default() -> Self {
        Self::new()
    }
}
impl AudioAssets {
    pub fn new() -> Self {
        let mailbox = Rc::new(RefCell::new(aqua::mailbox::Mailbox::new(NonZeroUsize::MIN)));
        let sender = mailbox.borrow().sender();
        Self {
            state: Rc::new(RefCell::new(State {
                next: 0,
                jobs: HashMap::new(),
                delivery: Delivery::with_wake(NonZeroUsize::new(16).unwrap(), sender),
            })),
            mailbox,
            fallback: ResourceCache::new(),
        }
    }
    pub fn bind(&self, ctx: &aqua::AppCtx<'_>, window: aqua::prelude::AquaWindowId) {
        ctx.bind_mailbox(window, &self.mailbox.borrow());
    }
    #[cfg(feature = "test-support")]
    pub fn set_test_waker(&self, wake: aqua::mailbox::Wake) {
        self.mailbox.borrow().set_waker(wake);
    }
    pub fn request(
        &self,
        fetcher: Option<&MediaFetcher>,
        source: &MediaSource,
        part: &str,
        waiter: u64,
        wake: aqua::mailbox::Wake,
    ) -> Option<Option<Arc<[u8]>>> {
        self.discard_stale();
        if matches!(source.parsed, Parsed::Invalid | Parsed::Url(_)) {
            return Some(None);
        }
        if matches!(source.parsed, Parsed::File { .. }) && fetcher.is_none() {
            return Some(None);
        }
        let immutable = matches!(source.parsed, Parsed::Data(_));
        let owner = fetcher.map_or_else(|| part.to_owned(), |f| f.asset_owner(part, immutable));
        let key = Owned::new(owner, source.parsed.clone());
        let cache = fetcher.map_or(&self.fallback, MediaFetcher::resources);
        let membership = fetcher.map_or_else(|| Arc::<[u64]>::from([]), |f| Arc::clone(f.owner()));
        let source_bytes = match &source.parsed {
            Parsed::Data(payload) => payload.bytes.len(),
            _ => source.raw.len(),
        };
        match cache.load::<Loader>(&membership, &key, source_bytes, waiter, wake) {
            Load::Ready(result) => Some(result),
            Load::Pending => None,
            Load::Start(ticket) => {
                let mut state = self.state.borrow_mut();
                let id = state.next;
                state.next = state.next.wrapping_add(1);
                let sender = state.delivery.sender();
                let source = source.clone();
                let transport = fetcher.map(MediaFetcher::provider);
                let task = runtime::runtime().spawn(async move {
                    let fetcher = transport.map(|provider| {
                        let scope = provider.scope().to_owned();
                        MediaFetcher::new(scope, move |request| provider.fetch(request))
                    });
                    let bytes =
                        source::load_image_bytes(&source, fetcher.as_ref(), &media_http::client())
                            .await
                            .ok();
                    let _ = sender.send(Completion { id, bytes }).await;
                });
                let abort = task.abort_handle();
                if ticket.attach(abort.clone()) {
                    state.jobs.insert(id, Job { ticket, abort });
                } else {
                    abort.abort();
                }
                None
            }
        }
    }
    pub fn drain(&self) -> bool {
        self.mailbox.borrow_mut().drain();
        let completions = self.state.borrow_mut().delivery.drain();
        let mut changed = false;
        for completion in completions {
            let Some(job) = self.state.borrow_mut().jobs.remove(&completion.id) else {
                continue;
            };
            if !job.ticket.current() {
                job.abort.abort();
                continue;
            }
            let bytes = completion.bytes.map(|loaded| loaded.bytes);
            let size = bytes.as_ref().map_or(0, |bytes| bytes.len());
            job.ticket.publish(bytes, size, drop);
            changed = true;
        }
        self.discard_stale();
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
}
