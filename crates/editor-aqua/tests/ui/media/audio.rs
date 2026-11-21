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
    let counter = Arc::clone(&calls);
    let fetcher = MediaFetcher::new("forge", move |request| {
        let counter = Arc::clone(&counter);
        Box::pin(async move {
            counter.fetch_add(1, Ordering::SeqCst);
            if request.path == "missing.wav" {
                Err(editor_aqua::ui::media::source::MediaLoadError::Fetch(
                    MediaFailure::Missing,
                ))
            } else {
                Ok(Arc::<[u8]>::from(&b"audio"[..]))
            }
        })
    })
    .owned_by(&[7], 1);
    let assets = AudioAssets::new();
    let (sender, receiver) = mpsc::channel();
    assets
        .wake_mailbox()
        .set_waker(Arc::new(move || match sender.send(()) {
            Ok(()) | Err(mpsc::SendError(())) => {}
        }));
    let noop: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
    let source = MediaSource::from("recording.wav".to_owned());
    assert!(
        assets
            .request(Some(&fetcher), &source, "row", 1, Arc::clone(&noop))
            .is_none()
    );
    assert!(
        assets
            .request(Some(&fetcher), &source, "row", 2, Arc::clone(&noop))
            .is_none()
    );
    receiver.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(assets.drain());
    assert_eq!(
        assets
            .request(Some(&fetcher), &source, "row", 1, Arc::clone(&noop))
            .unwrap()
            .unwrap()
            .as_ref(),
        b"audio"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let missing = MediaSource::from("missing.wav".to_owned());
    assert!(
        assets
            .request(Some(&fetcher), &missing, "row", 3, Arc::clone(&noop))
            .is_none()
    );
    receiver.recv_timeout(Duration::from_secs(3)).unwrap();
    assets.drain();
    assets
        .request(Some(&fetcher), &missing, "row", 3, Arc::clone(&noop))
        .unwrap()
        .unwrap_err();
    assets
        .request(Some(&fetcher), &missing, "row", 4, Arc::clone(&noop))
        .unwrap()
        .unwrap_err();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let next = fetcher.owned_by(&[7], 2);
    assert!(
        assets
            .request(Some(&next), &source, "row", 5, Arc::clone(&noop))
            .is_none()
    );
    receiver.recv_timeout(Duration::from_secs(3)).unwrap();
    assets.drain();
    assets
        .request(Some(&next), &source, "row", 5, noop)
        .unwrap()
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}
