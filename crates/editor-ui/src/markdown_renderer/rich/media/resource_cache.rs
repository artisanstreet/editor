//! A thread-owned resource cache. GPUI only owns pending worker loads;
//! completed results transfer here before their generic asset key is removed.
use futures::FutureExt as _;
use gpui::{App, AppContext as _, Asset, EntityId, Task, Window};
use std::any::{Any, TypeId};
use std::collections::{HashSet, VecDeque};
use std::sync::{
    Arc, Mutex, MutexGuard,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

const MAX_ENTRIES: usize = 32;
// Fits one 64 MiB decoded image plus its 32 MiB encoded source.
const MAX_BYTES: usize = 96 * 1024 * 1024;
const MAX_PENDING: usize = 8;
static NEXT_CACHE: AtomicU64 = AtomicU64::new(1);
type Release = Box<dyn Fn(&mut App) + Send>;
struct Entry {
    loader: TypeId,
    owners: Vec<Arc<[u64]>>,
    waiters: Vec<EntityId>,
    source: Box<dyn Any + Send>,
    output: Option<Box<dyn Any + Send>>,
    bytes: usize,
    task: Option<Task<()>>,
    release: Release,
}
impl Entry {
    fn resident_bytes(&self) -> usize {
        self.bytes
            .saturating_add(self.owners.capacity() * std::mem::size_of::<Arc<[u64]>>())
            .saturating_add(self.waiters.capacity() * std::mem::size_of::<EntityId>())
            .saturating_add(
                self.owners
                    .iter()
                    .map(|owner| {
                        owner.len() * std::mem::size_of::<u64>() + 2 * std::mem::size_of::<usize>()
                    })
                    .sum::<usize>(),
            )
    }
}

pub(super) struct ResourceCache {
    id: u64,
    closed: AtomicBool,
    entries: Mutex<VecDeque<Entry>>,
}
impl Default for ResourceCache {
    fn default() -> Self {
        Self {
            id: NEXT_CACHE.fetch_add(1, Ordering::Relaxed),
            closed: AtomicBool::new(false),
            entries: Mutex::new(VecDeque::new()),
        }
    }
}
impl ResourceCache {
    pub(super) const fn id(&self) -> u64 {
        self.id
    }
    #[expect(
        clippy::expect_used,
        reason = "nothing panics while holding the lock, so a poisoned cache is a bug"
    )]
    fn entries(&self) -> MutexGuard<'_, VecDeque<Entry>> {
        self.entries.lock().expect("resource cache lock")
    }
    pub(super) fn clear(&self, cx: &mut App) {
        self.closed.store(true, Ordering::Release);
        let entries = std::mem::take(&mut *self.entries());
        for entry in entries {
            (entry.release)(cx);
        }
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "one resource load: its key, owner, source, budget and callbacks"
    )]
    #[expect(
        clippy::too_many_lines,
        reason = "one sequential pass; splitting it would only thread its locals through helpers"
    )]
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the entry borrowed from the guard is used to the end"
    )]
    pub(super) fn load<A: Asset>(
        self: &Arc<Self>,
        owner: &Arc<[u64]>,
        source: &A::Source,
        window: &Window,
        cx: &mut App,
        source_bytes: usize,
        bytes: impl Fn(&A::Output) -> usize + Send + 'static,
        release: impl Fn(&A::Output, &mut App) + Send + 'static,
    ) -> Option<A::Output>
    where
        A::Source: PartialEq + 'static,
        A::Output: 'static,
    {
        if self.closed.load(Ordering::Acquire) {
            return None;
        }
        let loader = TypeId::of::<A>();
        let mut entries = self.entries();
        let index = entries.iter().position(|entry| {
            entry.loader == loader && entry.source.downcast_ref::<A::Source>() == Some(source)
        });
        if let Some(mut entry) = index.and_then(|index| entries.remove(index)) {
            if !entry.owners.iter().any(|known| known == owner) {
                entry.owners.push(Arc::clone(owner));
            }
            if entry.output.is_none() && !entry.waiters.contains(&window.current_view()) {
                entry.waiters.push(window.current_view());
            }
            let output = entry
                .output
                .as_ref()
                .and_then(|output| output.downcast_ref::<A::Output>())
                .cloned();
            entries.push_back(entry);
            let mut removed = Vec::new();
            while entries.iter().map(Entry::resident_bytes).sum::<usize>() > MAX_BYTES {
                let Some(entry) = entries.pop_front() else {
                    break;
                };
                removed.push(entry);
            }
            let retained = entries.iter().any(|entry| {
                entry.loader == loader && entry.source.downcast_ref::<A::Source>() == Some(source)
            });
            drop(entries);
            for entry in removed {
                (entry.release)(cx);
            }
            if !retained {
                return None;
            }
            if output.is_some() {
                return output;
            }
            entries = self.entries();
        } else {
            if entries
                .iter()
                .filter(|entry| entry.output.is_none())
                .count()
                >= MAX_PENDING
            {
                return None;
            }
            let membership_bytes = std::mem::size_of::<Arc<[u64]>>()
                + owner.len() * std::mem::size_of::<u64>()
                + 2 * std::mem::size_of::<usize>()
                + std::mem::size_of::<EntityId>();
            if source_bytes.saturating_add(membership_bytes) > MAX_BYTES {
                return None;
            }
            let mut removed: Vec<Entry> = Vec::new();
            while entries.len() >= MAX_ENTRIES
                || entries.iter().map(Entry::resident_bytes).sum::<usize>()
                    + source_bytes
                    + membership_bytes
                    > MAX_BYTES
            {
                let Some(ready) = entries
                    .iter()
                    .position(|entry| entry.output.is_some())
                    .and_then(|index| entries.remove(index))
                else {
                    drop(entries);
                    for entry in removed {
                        (entry.release)(cx);
                    }
                    return None;
                };
                removed.push(ready);
            }
            drop(entries);
            for entry in removed {
                (entry.release)(cx);
            }
            entries = self.entries();
            let asset_source = source.clone();
            entries.push_back(Entry {
                loader,
                owners: vec![Arc::clone(owner)],
                waiters: vec![window.current_view()],
                source: Box::new(source.clone()),
                output: None,
                bytes: source_bytes,
                task: None,
                release: Box::new(move |cx| cx.remove_asset::<A>(&asset_source)),
            });
        }
        drop(entries);
        let (task, _) = cx.fetch_asset::<A>(source);
        let Some(output) = task.clone().now_or_never() else {
            let mut entries = self.entries();
            let entry = entries.iter_mut().find(|entry| {
                entry.loader == loader && entry.source.downcast_ref::<A::Source>() == Some(source)
            })?;
            if entry.task.is_none() {
                let cache = Arc::downgrade(self);
                let source = source.clone();
                entry.task = Some(cx.spawn(async move |cx| {
                    let output = task.await;
                    cx.update(|cx| {
                        // Publish even when the loading block has left the viewport.
                        // This task never keeps the thread cache alive by itself.
                        if let Some(cache) = cache.upgrade() {
                            let cost = bytes(&output).saturating_add(source_bytes);
                            cache.publish::<A>(&source, &output, cost, release, cx);
                        } else {
                            cx.remove_asset::<A>(&source);
                            release(&output, cx);
                        }
                    });
                }));
            }
            return None;
        };
        let cost = bytes(&output).saturating_add(source_bytes);
        self.publish::<A>(source, &output, cost, release, cx);
        Some(output)
    }

    fn publish<A: Asset>(
        &self,
        source: &A::Source,
        output: &A::Output,
        cost: usize,
        release: impl Fn(&A::Output, &mut App) + Send + 'static,
        cx: &mut App,
    ) where
        A::Source: PartialEq + 'static,
        A::Output: 'static,
    {
        let loader = TypeId::of::<A>();
        let owned = output.clone();
        let mut entries = self.entries();
        let index = entries.iter().position(|entry| {
            entry.loader == loader && entry.source.downcast_ref::<A::Source>() == Some(source)
        });
        let Some(index) = index else {
            drop(entries);
            cx.remove_asset::<A>(source);
            release(output, cx);
            return;
        };
        let entry = &mut entries[index];
        // Publish ownership before removing the completed shared GPUI task.
        entry.output = Some(Box::new(output.clone()));
        entry.task.take();
        let waiters = std::mem::take(&mut entry.waiters);
        entry.bytes = cost;
        entry.release = Box::new(move |cx| release(&owned, cx));
        drop(entries);
        cx.remove_asset::<A>(source);
        let mut entries = self.entries();
        let mut removed: Vec<Entry> = Vec::new();
        while entries.len() > MAX_ENTRIES
            || entries.iter().map(Entry::resident_bytes).sum::<usize>() > MAX_BYTES
        {
            if let Some(entry) = entries.pop_front() {
                removed.push(entry);
            } else {
                break;
            }
        }
        drop(entries);
        for entry in removed {
            (entry.release)(cx);
        }
        for entity in waiters {
            cx.notify(entity);
        }
    }

    /// Release Item resources immediately when their semantic owner is unloaded.
    /// Native rows can have deeper prefixes, so membership uses the Item prefix.
    pub(super) fn retain_owners(&self, owners: &[Vec<u64>], cx: &mut App) {
        let owners: HashSet<&[u64]> = owners.iter().map(Vec::as_slice).collect();
        let mut entries = self.entries();
        let mut removed = Vec::new();
        let mut retained = VecDeque::new();
        for mut entry in entries.drain(..) {
            entry
                .owners
                .retain(|owner| (0..=owner.len()).any(|end| owners.contains(&owner[..end])));
            if entry.owners.is_empty() {
                removed.push(entry);
            } else {
                retained.push_back(entry);
            }
        }
        *entries = retained;
        drop(entries);
        for entry in removed {
            (entry.release)(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::channel::oneshot;
    use gpui::{Context, IntoElement, Render, TestAppContext, div};
    use std::hash::{Hash, Hasher};
    use std::sync::atomic::AtomicUsize;

    #[derive(Clone)]
    pub(super) struct GatedSource {
        pub(super) id: u64,
        pub(super) gate: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
    }
    impl PartialEq for GatedSource {
        fn eq(&self, other: &Self) -> bool {
            self.id == other.id
        }
    }
    impl Eq for GatedSource {}
    impl Hash for GatedSource {
        fn hash<H: Hasher>(&self, state: &mut H) {
            self.id.hash(state);
        }
    }
    pub(super) enum GatedLoader {}
    impl Asset for GatedLoader {
        type Source = GatedSource;
        type Output = Arc<[u8]>;
        fn load(
            source: Self::Source,
            _: &mut App,
        ) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
            let gate = source.gate.lock().unwrap().take().unwrap();
            async move {
                gate.await.unwrap();
                Arc::<[u8]>::from([7u8; 32])
            }
        }
    }
    pub(super) struct Fixture {
        pub(super) cache: Arc<ResourceCache>,
        pub(super) source: GatedSource,
        pub(super) showing: bool,
        pub(super) released: Arc<AtomicUsize>,
    }
    impl Render for Fixture {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            if self.showing {
                let released = Arc::clone(&self.released);
                self.cache.load::<GatedLoader>(
                    &Arc::from([42u64, 9]),
                    &self.source,
                    window,
                    cx,
                    16,
                    |output| output.len(),
                    move |_, _| {
                        released.fetch_add(1, Ordering::SeqCst);
                    },
                );
            }
            div()
        }
    }

    use super::super::source::{MediaFetcher, Owned};

    enum SharedLoader {}
    impl Asset for SharedLoader {
        type Source = Owned<GatedSource>;
        type Output = Arc<[u8]>;
        fn load(
            source: Self::Source,
            cx: &mut App,
        ) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
            GatedLoader::load(source.source, cx)
        }
    }

    struct Consumer {
        fetcher: MediaFetcher,
        source: Owned<GatedSource>,
        showing: bool,
        output: Option<Arc<[u8]>>,
        released: Arc<AtomicUsize>,
    }
    impl Render for Consumer {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            if self.showing {
                let released = Arc::clone(&self.released);
                self.output = self.fetcher.resources.load::<SharedLoader>(
                    &self.fetcher.owner,
                    &self.source,
                    window,
                    cx,
                    16,
                    |output| output.len(),
                    move |_, _| {
                        released.fetch_add(1, Ordering::SeqCst);
                    },
                );
            }
            div()
        }
    }
    struct SharedFixture {
        first: gpui::Entity<Consumer>,
        second: gpui::Entity<Consumer>,
    }
    impl Render for SharedFixture {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            use gpui::ParentElement as _;
            div().child(self.first.clone()).child(self.second.clone())
        }
    }

    #[gpui::test]
    fn repeated_immutable_references_share_completion_and_release_after_last_item(
        cx: &mut TestAppContext,
    ) {
        let (send, receive) = oneshot::channel();
        let provider = MediaFetcher::new("forge/thread", |_| Box::pin(async { None }));
        let cache = Arc::clone(&provider.resources);
        let source = GatedSource {
            id: 3,
            gate: Arc::new(Mutex::new(Some(receive))),
        };
        let released = Arc::new(AtomicUsize::new(0));
        let (view, cx) = cx.add_window_view(|_, cx| {
            let first_fetcher = provider.clone().owned_by(&[42], 1);
            let second_fetcher = provider.clone().owned_by(&[43], 2);
            let first_source = Owned::new(
                &first_fetcher.asset_owner(&"first".into(), true),
                source.clone(),
            );
            let second_source = Owned::new(
                &second_fetcher.asset_owner(&"second".into(), true),
                source.clone(),
            );
            SharedFixture {
                first: cx.new(|_| Consumer {
                    fetcher: first_fetcher,
                    source: first_source,
                    showing: true,
                    output: None,
                    released: Arc::clone(&released),
                }),
                second: cx.new(|_| Consumer {
                    fetcher: second_fetcher,
                    source: second_source,
                    showing: true,
                    output: None,
                    released: Arc::clone(&released),
                }),
            }
        });
        cx.run_until_parked();
        let (first, second) =
            view.read_with(cx, |view, _| (view.first.clone(), view.second.clone()));
        {
            let entries = cache.entries.lock().unwrap();
            assert_eq!(
                entries.len(),
                1,
                "two references must load one immutable source"
            );
            assert_eq!(entries[0].owners.len(), 2);
            assert_eq!(
                entries[0].waiters.len(),
                2,
                "both native views need completion notification"
            );
        }
        first.update(cx, |consumer, cx| {
            consumer.showing = false;
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|_, cx| cache.retain_owners(&[vec![43]], cx));
        assert!(
            !send.is_canceled(),
            "one remaining owner keeps its worker alive"
        );
        send.send(()).unwrap();
        cx.run_until_parked();
        let second_output = second
            .read_with(cx, |consumer, _| consumer.output.clone())
            .expect("remaining consumer receives completion without the first view rendering");
        assert_eq!(released.load(Ordering::SeqCst), 0);
        assert_eq!(cache.entries.lock().unwrap().len(), 1);
        first.update(cx, |consumer, cx| {
            consumer.showing = true;
            cx.notify();
        });
        cx.run_until_parked();
        let first_output = first
            .read_with(cx, |consumer, _| consumer.output.clone())
            .unwrap();
        assert!(
            Arc::ptr_eq(&first_output, &second_output),
            "repeated reference reuses exact completed allocation"
        );
        cx.update(|_, cx| cache.retain_owners(&[vec![42]], cx));
        assert_eq!(
            released.load(Ordering::SeqCst),
            0,
            "removing one membership must not drop the shared resource"
        );
        cx.update(|_, cx| cache.retain_owners(&[], cx));
        assert!(cache.entries.lock().unwrap().is_empty());
        assert_eq!(released.load(Ordering::SeqCst), 1);
        cx.update(|_, cx| cache.clear(cx));
        assert_eq!(released.load(Ordering::SeqCst), 1);
    }

    #[gpui::test]
    fn last_pending_membership_cancels_shared_worker(cx: &mut TestAppContext) {
        let (send, receive) = oneshot::channel();
        let provider = MediaFetcher::new("forge/thread", |_| Box::pin(async { None }));
        let cache = Arc::clone(&provider.resources);
        let source = GatedSource {
            id: 4,
            gate: Arc::new(Mutex::new(Some(receive))),
        };
        let released = Arc::new(AtomicUsize::new(0));
        let (view, cx) = cx.add_window_view(|_, cx| {
            let make = |id, cx: &mut Context<SharedFixture>| {
                let fetcher = provider.clone().owned_by(&[id], id);
                let source =
                    Owned::new(&fetcher.asset_owner(&"inline".into(), true), source.clone());
                cx.new(|_| Consumer {
                    fetcher,
                    source,
                    showing: true,
                    output: None,
                    released: Arc::clone(&released),
                })
            };
            SharedFixture {
                first: make(42, cx),
                second: make(43, cx),
            }
        });
        cx.run_until_parked();
        cx.update(|_, cx| cache.retain_owners(&[vec![43]], cx));
        assert!(!send.is_canceled());
        let (first, second) =
            view.read_with(cx, |view, _| (view.first.clone(), view.second.clone()));
        first.update(cx, |consumer, cx| {
            consumer.showing = false;
            cx.notify();
        });
        second.update(cx, |consumer, cx| {
            consumer.showing = false;
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|_, cx| cache.retain_owners(&[], cx));
        cx.run_until_parked();
        assert!(
            send.send(()).is_err(),
            "the last owner releases the actual pending receiver"
        );
        assert!(cache.entries.lock().unwrap().is_empty());
        assert_eq!(released.load(Ordering::SeqCst), 0);
    }

    #[gpui::test]
    fn hidden_completion_transfers_asset_and_item_eviction_releases_it(cx: &mut TestAppContext) {
        let (send, receive) = oneshot::channel();
        let cache = Arc::new(ResourceCache::default());
        let released = Arc::new(AtomicUsize::new(0));
        let (view, cx) = cx.add_window_view(|_, _| Fixture {
            cache: Arc::clone(&cache),
            source: GatedSource {
                id: 1,
                gate: Arc::new(Mutex::new(Some(receive))),
            },
            showing: true,
            released: Arc::clone(&released),
        });
        cx.run_until_parked();
        assert_eq!(cache.entries.lock().unwrap().len(), 1);
        let source = view.read_with(cx, |view, _| view.source.clone());
        assert!(cx.update(|_, cx| cx.has_asset::<GatedLoader>(&source)));
        view.update(cx, |view, cx| {
            view.showing = false;
            cx.notify();
        });
        cx.run_until_parked();
        send.send(()).unwrap();
        cx.run_until_parked();
        {
            let entries = cache.entries.lock().unwrap();
            assert!(
                entries[0].output.is_some(),
                "completion owns output without another resource render"
            );
            assert!(entries[0].task.is_none());
            assert_eq!(entries[0].bytes, 48);
        }
        assert!(
            !cx.update(|_, cx| cx.has_asset::<GatedLoader>(&source)),
            "completed GPUI task must be removed without another resource render"
        );
        cx.update(|_, cx| cache.retain_owners(&[vec![42]], cx));
        assert_eq!(
            cache.entries.lock().unwrap().len(),
            1,
            "Item prefix retains nested native rows"
        );
        cx.update(|_, cx| cache.retain_owners(&[vec![43]], cx));
        assert!(cache.entries.lock().unwrap().is_empty());
        assert_eq!(released.load(Ordering::SeqCst), 1);
        cx.update(|_, cx| cache.clear(cx));
        assert_eq!(
            released.load(Ordering::SeqCst),
            1,
            "thread close must not release an evicted result twice"
        );
    }
}

#[cfg(test)]
mod pending_tests {
    use super::tests::{Fixture, GatedLoader, GatedSource};
    use super::*;
    use futures::channel::oneshot;
    use std::sync::atomic::AtomicUsize;
    #[gpui::test]
    fn unloading_an_item_cancels_its_pending_asset(cx: &mut gpui::TestAppContext) {
        let (send, receive) = oneshot::channel();
        let cache = Arc::new(ResourceCache::default());
        let released = Arc::new(AtomicUsize::new(0));
        let (view, cx) = cx.add_window_view(|_, _| Fixture {
            cache: Arc::clone(&cache),
            source: GatedSource {
                id: 2,
                gate: Arc::new(Mutex::new(Some(receive))),
            },
            showing: true,
            released: Arc::clone(&released),
        });
        cx.run_until_parked();
        let source = view.read_with(cx, |view, _| view.source.clone());
        view.update(cx, |view, cx| {
            view.showing = false;
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|_, cx| cache.retain_owners(&[], cx));
        cx.run_until_parked();
        assert!(cache.entries.lock().unwrap().is_empty());
        assert!(!cx.update(|_, cx| cx.has_asset::<GatedLoader>(&source)));
        assert!(
            send.send(()).is_err(),
            "pending worker receiver must be dropped"
        );
        assert_eq!(released.load(Ordering::SeqCst), 0);
    }
}
