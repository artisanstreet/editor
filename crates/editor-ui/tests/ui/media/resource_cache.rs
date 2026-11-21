use super::*;
use std::sync::atomic::AtomicUsize;

const MIB: usize = 1024 * 1024;

struct Image;
impl ResourceLoader for Image {
    type Source = String;
    type Output = Arc<[u8]>;
}
struct Audio;
impl ResourceLoader for Audio {
    type Source = String;
    type Output = Arc<[u8]>;
}

#[derive(Clone, Default)]
struct Count(Arc<AtomicUsize>);

impl Count {
    fn wake(&self) -> aqua::mailbox::Wake {
        let count = Arc::clone(&self.0);
        Arc::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
        })
    }
    fn release(&self) -> impl FnOnce(Arc<[u8]>) + Send + 'static {
        let count = Arc::clone(&self.0);
        move |_| {
            count.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn get(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn start<L: ResourceLoader<Source = String>>(
    cache: &Arc<ResourceCache>,
    owner: &Arc<[u64]>,
    key: &str,
    bytes: usize,
    waiter: u64,
    wake: &Count,
) -> LoadTicket<L> {
    match cache.load::<L>(owner, &key.to_owned(), bytes, waiter, wake.wake()) {
        Load::Start(ticket) => ticket,
        Load::Ready(_) | Load::Pending | Load::TooLarge => panic!("{key} should start loading"),
    }
}

fn live(cache: &ResourceCache) -> (usize, usize) {
    let state = cache.state();
    (state.entries.len(), state.pending())
}

#[test]
fn shared_completion_notifies_waiters_and_lasts_until_last_owner() {
    let cache = ResourceCache::new();
    let first: Arc<[u64]> = Arc::from([10, 1]);
    let second: Arc<[u64]> = Arc::from([20, 2]);
    let first_wake = Count::default();
    let second_wake = Count::default();
    let released = Count::default();
    let ticket = start::<Image>(&cache, &first, "same", 4, 1, &first_wake);
    assert!(matches!(
        cache.load::<Image>(&second, &"same".into(), 4, 2, second_wake.wake()),
        Load::Pending
    ));
    cache.retain_owners(&[vec![20]]);
    let payload: Arc<[u8]> = Arc::from([1, 2, 3]);
    assert!(ticket.publish(Arc::clone(&payload), 3, released.release()));
    assert_eq!((first_wake.get(), second_wake.get()), (1, 1));
    let Load::Ready(actual) =
        cache.load::<Image>(&second, &"same".into(), 4, 2, second_wake.wake())
    else {
        panic!("the shared result is ready")
    };
    assert!(Arc::ptr_eq(&payload, &actual));
    drop(actual);
    assert!(matches!(
        cache.load::<Audio>(&second, &"same".into(), 4, 2, second_wake.wake()),
        Load::Start(_)
    ));
    cache.retain_owners(&[]);
    assert_eq!(released.get(), 1);
    assert_eq!(live(&cache), (0, 0));
    assert_eq!(Arc::strong_count(&payload), 1);
    cache.close();
    assert_eq!(released.get(), 1);
}

#[test]
fn a_ninth_load_waits_for_admission_and_is_woken_when_a_slot_frees() {
    let cache = ResourceCache::new();
    let owner: Arc<[u64]> = Arc::from([7]);
    let runtime = runtime();
    let started = Count::default();
    let mut tickets: Vec<_> = (0..8)
        .map(|index| start::<Image>(&cache, &owner, &index.to_string(), 1, index, &started))
        .collect();
    let overflow = Count::default();
    assert!(matches!(
        cache.load::<Image>(&owner, &"overflow".into(), 1, 9, overflow.wake()),
        Load::Pending
    ));
    assert!(matches!(
        cache.load::<Image>(&owner, &"overflow".into(), 1, 9, overflow.wake()),
        Load::Pending
    ));
    assert_eq!(live(&cache), (8, 8));
    let task = runtime.spawn(std::future::pending::<()>());
    assert!(tickets[0].attach(task.abort_handle()));
    assert!(tickets.remove(1).publish(Arc::from([1]), 1, drop));
    assert_eq!(
        overflow.get(),
        1,
        "the waiting load hears about the free slot once"
    );
    assert!(matches!(
        cache.load::<Image>(&owner, &"overflow".into(), 1, 9, overflow.wake()),
        Load::Start(_)
    ));
    cache.retain_owners(&[]);
    assert!(runtime.block_on(task).unwrap_err().is_cancelled());
    let released = Count::default();
    assert!(
        !tickets
            .remove(0)
            .publish(Arc::from([1]), 1, released.release())
    );
    assert_eq!(released.get(), 1);
    assert_eq!(live(&cache), (0, 0));
}

#[test]
fn removing_an_unfinished_load_wakes_its_waiters_so_they_can_retry() {
    let cache = ResourceCache::new();
    let owner: Arc<[u64]> = Arc::from([1]);
    let first = Count::default();
    let second = Count::default();
    let ticket = start::<Image>(&cache, &owner, "file", 4, 1, &first);
    assert!(matches!(
        cache.load::<Image>(&owner, &"file".into(), 4, 2, second.wake()),
        Load::Pending
    ));
    cache.forget::<Image>(&"file".into());
    assert_eq!((first.get(), second.get()), (1, 1));
    assert!(!ticket.current());
    assert!(matches!(
        cache.load::<Image>(&owner, &"file".into(), 4, 2, second.wake()),
        Load::Start(_)
    ));
}

#[test]
fn forgotten_generation_cannot_overwrite_retried_result() {
    let cache = ResourceCache::new();
    let owner: Arc<[u64]> = Arc::from([1]);
    let count = Count::default();
    let old = start::<Image>(&cache, &owner, "file", 4, 1, &count);
    cache.forget::<Image>(&"file".into());
    let current = start::<Image>(&cache, &owner, "file", 4, 1, &count);
    let released = Count::default();
    assert!(!old.publish(Arc::from([1]), 1, released.release()));
    assert!(current.publish(Arc::from([2]), 1, drop));
    let Load::Ready(value) = cache.load::<Image>(&owner, &"file".into(), 4, 1, count.wake()) else {
        panic!("the retried result is ready")
    };
    assert_eq!(&*value, &[2]);
    assert_eq!(released.get(), 1);
}

#[test]
fn ticket_cancel_aborts_only_its_pending_generation() {
    let cache = ResourceCache::new();
    let owner: Arc<[u64]> = Arc::from([1]);
    let count = Count::default();
    let runtime = runtime();
    let old = start::<Image>(&cache, &owner, "file", 4, 1, &count);
    assert!(old.current());
    cache.forget::<Image>(&"file".into());
    assert!(!old.current());
    let current = start::<Image>(&cache, &owner, "file", 4, 1, &count);
    let task = runtime.spawn(std::future::pending::<()>());
    assert!(current.attach(task.abort_handle()));
    assert!(!old.cancel());
    assert!(current.current());
    assert!(current.cancel());
    assert!(!current.current());
    assert!(!current.cancel());
    assert!(runtime.block_on(task).unwrap_err().is_cancelled());
    let retry = start::<Image>(&cache, &owner, "file", 4, 1, &count);
    assert!(!current.cancel());
    assert!(retry.publish(Arc::from([2]), 1, drop));
    assert!(matches!(
        cache.load::<Image>(&owner, &"file".into(), 4, 1, count.wake()),
        Load::Ready(_)
    ));
}

#[test]
fn the_least_recent_result_is_evicted_and_released_once() {
    let cache = ResourceCache::new();
    let owner: Arc<[u64]> = Arc::from([3]);
    let count = Count::default();
    let released = Count::default();
    let payload: Arc<[u8]> = Arc::from([0]);
    for index in 0..32 {
        let ticket = start::<Image>(&cache, &owner, &index.to_string(), 1, index, &count);
        assert!(ticket.publish(Arc::clone(&payload), 1, released.release()));
    }
    assert_eq!(Arc::strong_count(&payload), 33);
    assert!(matches!(
        cache.load::<Image>(&owner, &"0".into(), 1, 0, count.wake()),
        Load::Ready(_)
    ));
    let ticket = start::<Image>(&cache, &owner, "new", 1, 50, &count);
    assert_eq!(released.get(), 1);
    assert_eq!(Arc::strong_count(&payload), 32);
    assert!(matches!(
        cache.load::<Image>(&owner, &"1".into(), 1, 1, count.wake()),
        Load::Start(_)
    ));
    assert!(matches!(
        cache.load::<Image>(&owner, &"0".into(), 1, 0, count.wake()),
        Load::Ready(_)
    ));
    drop(ticket);
    cache.close();
    assert_eq!(released.get(), 32);
    assert_eq!(Arc::strong_count(&payload), 1);
}

#[test]
fn a_result_larger_than_the_budget_fails_stably_instead_of_reloading() {
    let cache = ResourceCache::new();
    let owner: Arc<[u64]> = Arc::from([3]);
    let waiter = Count::default();
    let released = Count::default();
    let output: Arc<[u8]> = Arc::from([0]);
    let ticket = start::<Image>(&cache, &owner, "huge", 1, 1, &waiter);
    assert!(ticket.publish(Arc::clone(&output), 64 * MIB, released.release()));
    assert_eq!(waiter.get(), 1);
    assert_eq!(released.get(), 1, "the oversized output is not kept");
    assert_eq!(Arc::strong_count(&output), 1);
    for _ in 0..3 {
        assert!(matches!(
            cache.load::<Image>(&owner, &"huge".into(), 1, 1, waiter.wake()),
            Load::TooLarge
        ));
    }
    assert!(matches!(
        cache.load::<Image>(&owner, &"source".into(), 64 * MIB, 2, waiter.wake()),
        Load::TooLarge
    ));
    assert_eq!(live(&cache), (1, 0));
}

#[test]
fn publishing_keeps_unfinished_loads_and_its_own_result() {
    let cache = ResourceCache::new();
    let owner: Arc<[u64]> = Arc::from([3]);
    let count = Count::default();
    let runtime = runtime();
    let older = start::<Image>(&cache, &owner, "older", 1, 1, &count);
    assert!(older.publish(Arc::from([1]), MIB, drop));
    let unfinished = start::<Image>(&cache, &owner, "unfinished", 8 * MIB, 2, &count);
    let task = runtime.spawn(std::future::pending::<()>());
    assert!(unfinished.attach(task.abort_handle()));
    let large = start::<Image>(&cache, &owner, "large", 1, 3, &count);
    let output: Arc<[u8]> = Arc::from([2]);
    assert!(large.publish(Arc::clone(&output), 60 * MIB, drop));
    assert!(unfinished.current(), "the unfinished load keeps running");
    assert!(!task.is_finished());
    let Load::Ready(kept) = cache.load::<Image>(&owner, &"large".into(), 1, 3, count.wake()) else {
        panic!("the published result is kept")
    };
    assert!(Arc::ptr_eq(&kept, &output));
    assert!(matches!(
        cache.load::<Image>(&owner, &"older".into(), 1, 1, count.wake()),
        Load::Start(_)
    ));
    assert!(unfinished.publish(Arc::from([3]), 1, drop));
    assert!(matches!(
        cache.load::<Image>(&owner, &"unfinished".into(), 8 * MIB, 2, count.wake()),
        Load::Ready(_)
    ));
}

#[test]
fn closing_wakes_everyone_waiting_and_releases_every_result() {
    let cache = ResourceCache::new();
    let owner: Arc<[u64]> = Arc::from([3]);
    let pending = Count::default();
    let admission = Count::default();
    let released = Count::default();
    let payload: Arc<[u8]> = Arc::from([0]);
    let ready = start::<Image>(&cache, &owner, "ready", 1, 0, &Count::default());
    assert!(ready.publish(Arc::clone(&payload), 1, released.release()));
    let tickets: Vec<_> = (1..=8)
        .map(|index| start::<Image>(&cache, &owner, &index.to_string(), 1, index, &pending))
        .collect();
    assert!(matches!(
        cache.load::<Image>(&owner, &"later".into(), 1, 9, admission.wake()),
        Load::Pending
    ));
    cache.close();
    assert_eq!((pending.get(), admission.get(), released.get()), (8, 1, 1));
    assert_eq!(Arc::strong_count(&payload), 1);
    assert_eq!(live(&cache), (0, 0));
    assert!(tickets.iter().all(|ticket| !ticket.current()));
}

#[test]
fn fetcher_retention_and_clear_operate_on_the_shared_thread_cache() {
    use crate::ui::media::source::{MediaFailure, MediaFetcher};
    let fetcher = MediaFetcher::new("thread", |_| {
        Box::pin(async {
            Err(crate::ui::media::source::MediaLoadError::Fetch(
                MediaFailure::Missing,
            ))
        })
    });
    let first = fetcher.clone().owned_by(&[1, 2], 1);
    let second = fetcher.clone().owned_by(&[3], 1);
    let key = "shared".to_owned();
    let count = Count::default();
    let released = Count::default();
    let Load::Start(ticket) =
        first
            .resources()
            .load::<Image>(first.owner(), &key, 6, 1, count.wake())
    else {
        panic!("the first view starts the load")
    };
    assert!(matches!(
        second
            .resources()
            .load::<Image>(second.owner(), &key, 6, 2, count.wake()),
        Load::Pending
    ));
    assert!(ticket.publish(Arc::from([9]), 1, released.release()));
    fetcher.retain_resources(&[vec![3]]);
    assert_eq!(released.get(), 0);
    fetcher.close_resources();
    assert_eq!(released.get(), 1);
    assert!(matches!(
        second
            .resources()
            .load::<Image>(second.owner(), &key, 6, 2, count.wake()),
        Load::Pending
    ));
}
