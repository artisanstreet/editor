use super::{
    resource_cache::{LoadTicket, ResourceCache, ResourceLoader},
    source::MediaFetcher,
};
use editor_core::{delivery::Delivery, runtime};
use std::{cell::RefCell, collections::BTreeMap, num::NonZeroUsize, rc::Rc, sync::Arc};
use tokio::task::AbortHandle;

struct Completion<T> {
    id: u64,
    value: T,
}
struct Job<L: ResourceLoader> {
    ticket: LoadTicket<L>,
    abort: AbortHandle,
}
struct State<L: ResourceLoader, T> {
    next: u64,
    jobs: BTreeMap<u64, Job<L>>,
    delivery: Delivery<Completion<T>>,
}
impl<L: ResourceLoader, T> Drop for State<L, T> {
    fn drop(&mut self) {
        for job in self.jobs.values() {
            job.ticket.cancel();
            job.abort.abort();
        }
    }
}

pub(crate) struct Loads<L: ResourceLoader, T> {
    state: Rc<RefCell<State<L, T>>>,
    mailbox: Rc<RefCell<aqua::mailbox::Mailbox<()>>>,
    fallback: Arc<ResourceCache>,
}
impl<L: ResourceLoader, T> Clone for Loads<L, T> {
    fn clone(&self) -> Self {
        Self {
            state: Rc::clone(&self.state),
            mailbox: Rc::clone(&self.mailbox),
            fallback: Arc::clone(&self.fallback),
        }
    }
}
impl<L: ResourceLoader, T: Send + 'static> Loads<L, T> {
    pub(crate) fn new() -> Self {
        let mailbox = Rc::new(RefCell::new(aqua::mailbox::Mailbox::new(NonZeroUsize::MIN)));
        let sender = mailbox.borrow().sender();
        Self {
            state: Rc::new(RefCell::new(State {
                next: 0,
                jobs: BTreeMap::new(),
                delivery: Delivery::with_wake(
                    const {
                        NonZeroUsize::new(16)
                            .expect("invariant: the delivery capacity 16 is nonzero")
                    },
                    sender,
                ),
            })),
            mailbox,
            fallback: ResourceCache::new(),
        }
    }
    pub(crate) fn bind(&self, ctx: &aqua::AppCtx<'_>, window: aqua::prelude::AquaWindowId) {
        ctx.bind_mailbox(window, &self.wake_mailbox());
    }
    pub(crate) fn wake_mailbox(&self) -> std::cell::Ref<'_, aqua::mailbox::Mailbox<()>> {
        self.mailbox.borrow()
    }
    pub(crate) fn cache<'a>(&'a self, fetcher: Option<&'a MediaFetcher>) -> &'a Arc<ResourceCache> {
        fetcher.map_or(&self.fallback, MediaFetcher::resources)
    }
    pub(crate) fn start(
        &self,
        ticket: LoadTicket<L>,
        load: impl Future<Output = T> + Send + 'static,
    ) {
        let mut state = self.state.borrow_mut();
        let id = state.next;
        state.next = state.next.wrapping_add(1);
        let sender = state.delivery.sender();
        let task = runtime::runtime().spawn(async move {
            let value = load.await;
            sender.offer(Completion { id, value }).await;
        });
        let abort = task.abort_handle();
        if ticket.attach(abort.clone()) {
            state.jobs.insert(id, Job { ticket, abort });
        } else {
            abort.abort();
        }
    }
    pub(crate) fn drain(&self, mut finish: impl FnMut(T) -> (L::Output, usize)) -> bool {
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
            let (output, size) = finish(completion.value);
            job.ticket.publish(output, size, drop);
            changed = true;
        }
        self.discard_stale();
        changed
    }
    pub(crate) fn discard_stale(&self) {
        self.state.borrow_mut().jobs.retain(|_, job| {
            let current = job.ticket.current();
            if !current {
                job.abort.abort();
            }
            current
        });
    }
}
