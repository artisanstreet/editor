use super::*;
use std::{
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

#[test]
fn retry_refetches_the_failed_file_and_releases_the_cached_image() {
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(2, 1)
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
    let bytes: Arc<[u8]> = encoded.into_inner().into();
    let exists = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&exists);
    let fetcher = MediaFetcher::new("test", move |_| {
        let result = if flag.load(Ordering::SeqCst) {
            Ok(Arc::clone(&bytes))
        } else {
            Err(crate::ui::media::source::MediaFailure::Missing)
        };
        Box::pin(async move { result })
    })
    .owned_by(&[1], 1);
    let block: ImageBlock = serde_json::from_str(r#"{"src":"image.png"}"#).unwrap();
    let loader = Images::new();
    let assets = aqua::images::ImageAssets::new(aqua::images::ImageAssetBudget::editor());
    let mut state = ImageState::new(&block, fetcher.clone(), loader.clone());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        loader.drain(&assets);
        state.refresh();
        if matches!(state.loaded[0], Loaded::Failed(_)) {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    exists.store(true, Ordering::SeqCst);
    assert!(state.retry(0));
    loop {
        loader.drain(&assets);
        state.refresh();
        if matches!(state.loaded[0], Loaded::Ready(_)) {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(!state.retry(0));
    drop(state);
    fetcher.retain_resources(&[]);
    loader.retain_owners(&[]);
    assert_eq!(assets.stats().images, 0);
}

#[test]
fn a_new_item_revision_fetches_fresh_image_pixels() {
    use std::sync::atomic::AtomicUsize;
    let mut first = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(2, 1)
        .write_to(&mut first, image::ImageFormat::Png)
        .unwrap();
    let mut second = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(3, 1)
        .write_to(&mut second, image::ImageFormat::Png)
        .unwrap();
    let frames: [Arc<[u8]>; 2] = [first.into_inner().into(), second.into_inner().into()];
    let calls = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&calls);
    let fetcher = MediaFetcher::new("test", move |_| {
        let index = count.fetch_add(1, Ordering::SeqCst).min(1);
        let bytes = Arc::clone(&frames[index]);
        Box::pin(async move { Ok(bytes) })
    });
    let block: ImageBlock = serde_json::from_str(r#"{"src":"image.png"}"#).unwrap();
    let loader = Images::new();
    let assets = aqua::images::ImageAssets::default();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut first = ImageState::new(&block, fetcher.clone().owned_by(&[42], 1), loader.clone());
    loop {
        loader.drain(&assets);
        first.refresh();
        if matches!(&first.loaded[0], Loaded::Ready(image) if matches!(image.status(), ImageStatus::Ready { width: 2, .. }))
        {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    let mut second = ImageState::new(&block, fetcher.clone().owned_by(&[42], 2), loader.clone());
    loop {
        loader.drain(&assets);
        second.refresh();
        if matches!(&second.loaded[0], Loaded::Ready(image) if matches!(image.status(), ImageStatus::Ready { width: 3, .. }))
        {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
