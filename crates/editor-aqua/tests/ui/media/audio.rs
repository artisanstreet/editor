use editor_aqua::ui::media::{
    audio::AudioAssets,
    source::{MediaFailure, MediaFetcher, MediaSource},
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::time::Duration;

#[test]
fn fetched_audio_deduplicates_success_and_failure_until_revision_changes() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let fetcher = MediaFetcher::new("forge", move |request| {
        let counter = counter.clone();
        Box::pin(async move {
            counter.fetch_add(1, Ordering::SeqCst);
            if request.path == "missing.wav" {
                Err(MediaFailure::Missing)
            } else {
                Ok(Arc::<[u8]>::from(&b"audio"[..]))
            }
        })
    })
    .owned_by(&[7], 1);
    let assets = AudioAssets::new();
    let (sender, receiver) = mpsc::channel();
    assets.set_test_waker(Arc::new(move || {
        let _ = sender.send(());
    }));
    let noop = Arc::new(|| {});
    let source = MediaSource::from("recording.wav".to_owned());
    assert!(
        assets
            .request(Some(&fetcher), &source, "row", 1, noop.clone())
            .is_none()
    );
    assert!(
        assets
            .request(Some(&fetcher), &source, "row", 2, noop.clone())
            .is_none()
    );
    receiver.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(assets.drain());
    assert_eq!(
        assets
            .request(Some(&fetcher), &source, "row", 1, noop.clone())
            .unwrap()
            .unwrap()
            .as_ref(),
        b"audio"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let missing = MediaSource::from("missing.wav".to_owned());
    assert!(
        assets
            .request(Some(&fetcher), &missing, "row", 3, noop.clone())
            .is_none()
    );
    receiver.recv_timeout(Duration::from_secs(3)).unwrap();
    assets.drain();
    assert!(
        assets
            .request(Some(&fetcher), &missing, "row", 3, noop.clone())
            .unwrap()
            .is_none()
    );
    assert!(
        assets
            .request(Some(&fetcher), &missing, "row", 4, noop.clone())
            .unwrap()
            .is_none()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let next = fetcher.clone().owned_by(&[7], 2);
    assert!(
        assets
            .request(Some(&next), &source, "row", 5, noop.clone())
            .is_none()
    );
    receiver.recv_timeout(Duration::from_secs(3)).unwrap();
    assets.drain();
    assert!(
        assets
            .request(Some(&next), &source, "row", 5, noop)
            .unwrap()
            .is_some()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}
