#[test]
fn row_cache_evicts_least_recently_used_without_revoking_mounted_lease() {
    use editor_aqua::conversation::fold::RowKey;
    use editor_aqua::conversation::view::row_cache::RowCache;
    use std::rc::Rc;

    let key = |index| RowKey::Item(artisan_api::ItemId(format!("row-{index}")));
    let mut cache = RowCache::default();
    let mounted = cache.candidate(&key(0));
    cache.commit(key(0), mounted.clone());
    for index in 1..128 {
        let owner = cache.candidate(&key(index));
        cache.commit(key(index), owner);
    }
    let reused = cache.candidate(&key(1));
    cache.commit(key(1), reused.clone());
    let owner = cache.candidate(&key(128));
    cache.commit(key(128), owner);
    assert!(cache.get(&key(0)).is_none());
    assert!(Rc::ptr_eq(&cache.get(&key(1)).unwrap(), &reused));
    let replacement = cache.candidate(&key(0));
    assert!(!Rc::ptr_eq(&mounted, &replacement));
    let mounted_weak = Rc::downgrade(&mounted);
    cache.commit(key(0), replacement.clone());
    assert!(mounted_weak.upgrade().is_some());
    drop(mounted);
    assert!(mounted_weak.upgrade().is_none());
    assert!(Rc::ptr_eq(&cache.get(&key(0)).unwrap(), &replacement));
    cache.retain_rows(&[key(0)].into_iter().collect());
    assert!(cache.get(&key(1)).is_none());
    assert!(Rc::ptr_eq(&cache.get(&key(0)).unwrap(), &replacement));
}
