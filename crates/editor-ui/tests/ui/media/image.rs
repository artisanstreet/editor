use super::*;
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

fn fetcher() -> MediaFetcher {
    MediaFetcher::new("test", |_| {
        Box::pin(async { Err(source::MediaFailure::Missing) })
    })
}

fn image_source() -> MediaSource {
    use base64::Engine as _;
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(2, 1)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes.into_inner())
    )
    .into()
}

#[test]
fn retained_loader_publishes_after_consumer_drops_and_releases_cache_lease() {
    let loader = Images::new();
    let consumer = loader.clone();
    let fetcher = fetcher().owned_by(&[1], 1);
    let source = image_source();
    let assets = ImageAssets::new(aqua::images::ImageAssetBudget::editor());
    let wakes = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&wakes);
    let wake: aqua::mailbox::Wake = Arc::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
    });
    assert!(
        consumer
            .request(&fetcher, &source, "image", 1, Arc::clone(&wake))
            .is_none()
    );
    drop(consumer);
    let deadline = Instant::now() + Duration::from_secs(5);
    let image = loop {
        loader.drain(&assets);
        if let Some(result) = loader.request(&fetcher, &source, "image", 2, Arc::clone(&wake)) {
            break result.unwrap();
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    };
    assert!(matches!(
        image.status(),
        ImageStatus::Ready {
            width: 2,
            height: 1
        }
    ));
    assert!(wakes.load(Ordering::SeqCst) > 0);
    drop(image);
    fetcher.retain_resources(&[]);
    assert_eq!(assets.stats().images, 0);
}

#[test]
fn same_source_shares_pending_work_across_owners_and_prunes_only_last_owner() {
    let loader = Images::new();
    let fetcher = fetcher();
    let first = fetcher.clone().owned_by(&[1], 1);
    let second = fetcher.clone().owned_by(&[2], 1);
    let source = image_source();
    let wake: aqua::mailbox::Wake = Arc::new(|| {});
    assert!(
        loader
            .request(&first, &source, "before", 1, Arc::clone(&wake))
            .is_none()
    );
    assert!(
        loader
            .request(&second, &source, "after", 2, Arc::clone(&wake))
            .is_none()
    );
    assert_eq!(loader.state.borrow().jobs.len(), 1);
    fetcher.retain_resources(&[vec![2]]);
    loader.retain_owners(&[vec![2]]);
    assert_eq!(loader.state.borrow().jobs.len(), 1);
    fetcher.retain_resources(&[]);
    loader.retain_owners(&[]);
    assert!(loader.state.borrow().jobs.is_empty());
}

#[test]
fn discarded_generation_cannot_replace_new_cached_result() {
    let fetcher = fetcher().owned_by(&[1], 1);
    let source = image_source();
    let key = Owned::new(fetcher.asset_owner("image", true), source.parsed);
    let wake: aqua::mailbox::Wake = Arc::new(|| {});
    let Load::Start(old) =
        fetcher
            .resources()
            .load::<ImageLoader>(fetcher.owner(), &key, 1, 1, Arc::clone(&wake))
    else {
        panic!("first ticket")
    };
    fetcher.retain_resources(&[]);
    let Load::Start(new) =
        fetcher
            .resources()
            .load::<ImageLoader>(fetcher.owner(), &key, 1, 2, Arc::clone(&wake))
    else {
        panic!("new ticket")
    };
    assert!(!old.publish(Err(ImageError::Decode), 0, drop));
    assert!(new.publish(Err(ImageError::Load(MediaLoadError::Invalid)), 0, drop));
    assert!(matches!(
        fetcher
            .resources()
            .load::<ImageLoader>(fetcher.owner(), &key, 1, 3, wake),
        Load::Ready(Err(ImageError::Load(MediaLoadError::Invalid)))
    ));
}

#[test]
fn dropping_last_loader_cancels_its_pending_entry() {
    let fetcher = fetcher().owned_by(&[1], 1);
    let source = image_source();
    let wake: aqua::mailbox::Wake = Arc::new(|| {});
    let loader = Images::new();
    assert!(
        loader
            .request(&fetcher, &source, "image", 1, Arc::clone(&wake))
            .is_none()
    );
    drop(loader);
    let key = Owned::new(fetcher.asset_owner("image", true), source.parsed);
    assert!(matches!(
        fetcher
            .resources()
            .load::<ImageLoader>(fetcher.owner(), &key, 1, 2, wake),
        Load::Start(_)
    ));
}

#[test]
fn stale_job_cleanup_does_not_cancel_replacement_generation() {
    let fetcher = fetcher().owned_by(&[1], 1);
    let source = image_source();
    let wake: aqua::mailbox::Wake = Arc::new(|| {});
    let old = Images::new();
    let replacement = Images::new();
    assert!(
        old.request(&fetcher, &source, "image", 1, Arc::clone(&wake))
            .is_none()
    );
    fetcher.retain_resources(&[]);
    assert!(
        replacement
            .request(&fetcher, &source, "image", 2, Arc::clone(&wake))
            .is_none()
    );
    old.clear();
    let key = Owned::new(fetcher.asset_owner("image", true), source.parsed);
    assert!(matches!(
        fetcher
            .resources()
            .load::<ImageLoader>(fetcher.owner(), &key, 1, 3, wake),
        Load::Pending
    ));
    assert_eq!(replacement.state.borrow().jobs.len(), 1);
}

#[test]
fn url_cache_entries_are_scoped_to_item_revision() {
    let base = fetcher();
    let first = base.clone().owned_by(&[42], 1);
    let second = base.clone().owned_by(&[42], 2);
    let source: MediaSource = "https://example.invalid/image.png".to_owned().into();
    let first_key = cache_key(&first, &source, "image");
    let second_key = cache_key(&second, &source, "image");
    let wake: aqua::mailbox::Wake = Arc::new(|| {});
    let Load::Start(first_ticket) =
        base.resources()
            .load::<ImageLoader>(first.owner(), &first_key, 1, 1, Arc::clone(&wake))
    else {
        panic!("first revision")
    };
    assert!(first_ticket.publish(Err(ImageError::Load(MediaLoadError::Invalid)), 0, drop));
    let Load::Start(second_ticket) =
        base.resources()
            .load::<ImageLoader>(second.owner(), &second_key, 1, 2, Arc::clone(&wake))
    else {
        panic!("second revision must load")
    };
    assert!(second_ticket.publish(Err(ImageError::Decode), 0, drop));
    assert!(matches!(
        base.resources()
            .load::<ImageLoader>(first.owner(), &first_key, 1, 3, Arc::clone(&wake)),
        Load::Ready(Err(ImageError::Load(MediaLoadError::Invalid)))
    ));
    assert!(matches!(
        base.resources()
            .load::<ImageLoader>(second.owner(), &second_key, 1, 4, wake),
        Load::Ready(Err(ImageError::Decode))
    ));
}
