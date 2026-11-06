use editor_aqua::ui::media::resource_cache::{Load, ResourceCache, ResourceLoader};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

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

fn wake(counter: &Arc<AtomicUsize>) -> aqua::mailbox::Wake {
    let counter = Arc::clone(counter);
    Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    })
}

#[test]
fn shared_completion_notifies_waiters_and_lasts_until_last_owner() {
    let cache = ResourceCache::new();
    let first: Arc<[u64]> = Arc::from([10, 1]);
    let second: Arc<[u64]> = Arc::from([20, 2]);
    let key = "same".to_owned();
    let first_wake = Arc::new(AtomicUsize::new(0));
    let second_wake = Arc::new(AtomicUsize::new(0));
    let released = Arc::new(AtomicUsize::new(0));
    let Load::Start(ticket) = cache.load::<Image>(&first, &key, 4, 1, wake(&first_wake)) else {
        panic!()
    };
    assert!(matches!(
        cache.load::<Image>(&second, &key, 4, 2, wake(&second_wake)),
        Load::Pending
    ));
    cache.retain_owners(&[vec![20]]);
    let payload: Arc<[u8]> = Arc::from([1, 2, 3]);
    let count = Arc::clone(&released);
    assert!(ticket.publish(Arc::clone(&payload), 3, move |_| {
        count.fetch_add(1, Ordering::SeqCst);
    }));
    assert_eq!(first_wake.load(Ordering::SeqCst), 1);
    assert_eq!(second_wake.load(Ordering::SeqCst), 1);
    let Load::Ready(actual) = cache.load::<Image>(&second, &key, 4, 2, wake(&second_wake)) else {
        panic!()
    };
    assert!(Arc::ptr_eq(&payload, &actual));
    assert!(matches!(
        cache.load::<Audio>(&second, &key, 4, 2, wake(&second_wake)),
        Load::Start(_)
    ));
    cache.retain_owners(&[]);
    assert_eq!(released.load(Ordering::SeqCst), 1);
    cache.clear();
    assert_eq!(released.load(Ordering::SeqCst), 1);
}

#[test]
fn pending_limit_and_last_owner_abort_and_stale_completion() {
    let cache = ResourceCache::new();
    let owner: Arc<[u64]> = Arc::from([7]);
    let counter = Arc::new(AtomicUsize::new(0));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let mut tickets = Vec::new();
    for index in 0..8 {
        let Load::Start(ticket) =
            cache.load::<Image>(&owner, &index.to_string(), 1, index, wake(&counter))
        else {
            panic!()
        };
        tickets.push(ticket);
    }
    assert!(matches!(
        cache.load::<Image>(&owner, &"overflow".into(), 1, 9, wake(&counter)),
        Load::Pending
    ));
    let task = runtime.spawn(std::future::pending::<()>());
    assert!(tickets[0].attach(task.abort_handle()));
    cache.retain_owners(&[]);
    assert!(runtime.block_on(task).unwrap_err().is_cancelled());
    let released = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&released);
    assert!(!tickets.remove(0).publish(Arc::from([1]), 1, move |_| {
        count.fetch_add(1, Ordering::SeqCst);
    }));
    assert_eq!(released.load(Ordering::SeqCst), 1);
    assert!(matches!(
        cache.load::<Image>(&owner, &"overflow".into(), 1, 9, wake(&counter)),
        Load::Start(_)
    ));
}

#[test]
fn forgotten_generation_cannot_overwrite_retried_result() {
    let cache = ResourceCache::new();
    let owner: Arc<[u64]> = Arc::from([1]);
    let count = Arc::new(AtomicUsize::new(0));
    let key = "file".to_owned();
    let Load::Start(old) = cache.load::<Image>(&owner, &key, 4, 1, wake(&count)) else {
        panic!()
    };
    cache.forget::<Image>(&key);
    let Load::Start(current) = cache.load::<Image>(&owner, &key, 4, 1, wake(&count)) else {
        panic!()
    };
    let released = Arc::new(AtomicUsize::new(0));
    let old_release = Arc::clone(&released);
    assert!(!old.publish(Arc::from([1]), 1, move |_| {
        old_release.fetch_add(1, Ordering::SeqCst);
    }));
    assert!(current.publish(Arc::from([2]), 1, |_| {}));
    let Load::Ready(value) = cache.load::<Image>(&owner, &key, 4, 1, wake(&count)) else {
        panic!()
    };
    assert_eq!(&*value, &[2]);
    assert_eq!(released.load(Ordering::SeqCst), 1);
}

#[test]
fn ticket_cancel_aborts_only_its_pending_generation() {
    let cache = ResourceCache::new();
    let owner: Arc<[u64]> = Arc::from([1]);
    let count = Arc::new(AtomicUsize::new(0));
    let key = "file".to_owned();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let Load::Start(old) = cache.load::<Image>(&owner, &key, 4, 1, wake(&count)) else {
        panic!()
    };
    assert!(old.current());
    cache.forget::<Image>(&key);
    assert!(!old.current());
    let Load::Start(current) = cache.load::<Image>(&owner, &key, 4, 1, wake(&count)) else {
        panic!()
    };
    let task = runtime.spawn(std::future::pending::<()>());
    assert!(current.attach(task.abort_handle()));
    assert!(!old.cancel());
    assert!(current.current());
    assert!(current.cancel());
    assert!(!current.current());
    assert!(!current.cancel());
    assert!(runtime.block_on(task).unwrap_err().is_cancelled());
    let Load::Start(retry) = cache.load::<Image>(&owner, &key, 4, 1, wake(&count)) else {
        panic!()
    };
    assert!(!current.cancel());
    assert!(retry.publish(Arc::from([2]), 1, |_| {}));
    assert!(matches!(
        cache.load::<Image>(&owner, &key, 4, 1, wake(&count)),
        Load::Ready(_)
    ));
}

#[test]
fn lru_and_byte_budget_release_evicted_results_once() {
    let cache = ResourceCache::new();
    let owner: Arc<[u64]> = Arc::from([3]);
    let count = Arc::new(AtomicUsize::new(0));
    let released = Arc::new(AtomicUsize::new(0));
    for index in 0..32 {
        let Load::Start(ticket) =
            cache.load::<Image>(&owner, &index.to_string(), 1, index, wake(&count))
        else {
            panic!()
        };
        let released = Arc::clone(&released);
        assert!(ticket.publish(Arc::from([0]), 1, move |_| {
            released.fetch_add(1, Ordering::SeqCst);
        }));
    }
    assert!(matches!(
        cache.load::<Image>(&owner, &"0".into(), 1, 0, wake(&count)),
        Load::Ready(_)
    ));
    let Load::Start(ticket) = cache.load::<Image>(&owner, &"new".into(), 1, 50, wake(&count))
    else {
        panic!()
    };
    assert_eq!(released.load(Ordering::SeqCst), 1);
    assert!(ticket.publish(Arc::from([0]), 64 * 1024 * 1024, |_| {}));
    assert!(matches!(
        cache.load::<Image>(&owner, &"huge".into(), 64 * 1024 * 1024, 51, wake(&count)),
        Load::Pending
    ));
    cache.clear();
    assert_eq!(released.load(Ordering::SeqCst), 32);
}

#[test]
fn fetcher_retention_and_clear_operate_on_the_shared_thread_cache() {
    use editor_aqua::ui::media::source::{MediaFailure, MediaFetcher};
    let fetcher = MediaFetcher::new("thread", |_| Box::pin(async { Err(MediaFailure::Missing) }));
    let first = fetcher.clone().owned_by(&[1, 2], 1);
    let second = fetcher.clone().owned_by(&[3], 1);
    let key = "shared".to_owned();
    let count = Arc::new(AtomicUsize::new(0));
    let released = Arc::new(AtomicUsize::new(0));
    let Load::Start(ticket) =
        first
            .resources()
            .load::<Image>(first.owner(), &key, 6, 1, wake(&count))
    else {
        panic!()
    };
    assert!(matches!(
        second
            .resources()
            .load::<Image>(second.owner(), &key, 6, 2, wake(&count)),
        Load::Pending
    ));
    let release = Arc::clone(&released);
    assert!(ticket.publish(Arc::from([9]), 1, move |_| {
        release.fetch_add(1, Ordering::SeqCst);
    }));
    fetcher.retain_resources(&[vec![3]]);
    assert_eq!(released.load(Ordering::SeqCst), 0);
    fetcher.clear_resources();
    assert_eq!(released.load(Ordering::SeqCst), 1);
    assert!(matches!(
        second
            .resources()
            .load::<Image>(second.owner(), &key, 6, 2, wake(&count)),
        Load::Pending
    ));
}
