use super::*;
use crate::ui::media::source::MediaFailure;
use std::sync::mpsc;
use std::time::Duration;
use tokio::sync::oneshot;

type Reply = oneshot::Sender<Result<Arc<[u8]>, MediaFailure>>;

const LIMIT: Duration = Duration::from_secs(10);

struct Fixture {
    assets: AudioAssets,
    fetcher: MediaFetcher,
    calls: mpsc::Receiver<Reply>,
    wake: mpsc::Receiver<()>,
}

impl Fixture {
    fn new() -> Self {
        let (calls, requests) = mpsc::channel();
        let fetcher = MediaFetcher::new("forge", move |_| {
            let (reply, receive) = oneshot::channel();
            calls.send(reply).expect("the test listens for fetches");
            Box::pin(async move { receive.await.unwrap_or(Err(MediaFailure::Unreadable)) })
        })
        .owned_by(&[7], 1);
        let assets = AudioAssets::new();
        let (wake, incoming) = mpsc::channel();
        assets
            .loads
            .wake_mailbox()
            .set_waker(Arc::new(move || match wake.send(()) {
                Ok(()) | Err(mpsc::SendError(())) => {}
            }));
        Self {
            assets,
            fetcher,
            calls: requests,
            wake: incoming,
        }
    }

    fn request(&self, src: &str) -> Option<Result<Arc<[u8]>, AudioLoadError>> {
        self.assets.request(
            Some(&self.fetcher),
            &MediaSource::from(src.to_owned()),
            "audio",
            1,
            Arc::new(|| {}),
        )
    }

    fn reply(&self, bytes: Result<Arc<[u8]>, MediaFailure>) {
        self.calls
            .recv_timeout(LIMIT)
            .expect("the track was fetched")
            .send(bytes)
            .expect("the fetch still waits");
        self.wake
            .recv_timeout(LIMIT)
            .expect("the reply woke the view");
        assert!(self.assets.drain());
    }
}

#[test]
fn a_missing_track_says_the_file_does_not_exist() {
    let fixture = Fixture::new();
    assert!(fixture.request("tracks/intro.ogg").is_none());
    fixture.reply(Err(MediaFailure::Missing));
    let error = fixture
        .request("tracks/intro.ogg")
        .expect("the failure is cached")
        .expect_err("a missing file does not play");
    assert_eq!(
        error,
        AudioLoadError::Load(MediaLoadError::Fetch(MediaFailure::Missing))
    );
    assert_eq!(error.to_string(), "the file does not exist");
}

#[test]
fn an_unreachable_host_is_told_apart_from_a_missing_file() {
    let fixture = Fixture::new();
    assert!(fixture.request("tracks/outro.ogg").is_none());
    fixture.reply(Err(MediaFailure::ForgeUnreachable));
    assert_eq!(
        fixture.request("tracks/outro.ogg"),
        Some(Err(AudioLoadError::Load(MediaLoadError::Fetch(
            MediaFailure::ForgeUnreachable
        ))))
    );
}

#[test]
fn a_remote_track_is_refused_without_fetching() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture.request("https://example.com/intro.ogg"),
        Some(Err(AudioLoadError::Remote))
    );
    fixture.calls.try_recv().unwrap_err();
}

#[test]
fn an_embedded_track_at_the_size_limit_stays_cached_once_loaded() {
    use base64::Engine as _;
    let fixture = Fixture::new();
    let payload = vec![7_u8; 32 * 1024 * 1024];
    let source = MediaSource::from(format!(
        "data:audio/ogg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&payload)
    ));
    let request = || {
        fixture
            .assets
            .request(Some(&fixture.fetcher), &source, "audio", 1, Arc::new(|| {}))
    };
    assert!(request().is_none());
    fixture
        .wake
        .recv_timeout(LIMIT)
        .expect("the embedded track decoded");
    assert!(fixture.assets.drain());
    let first = request()
        .expect("the track is cached")
        .expect("the track plays");
    let second = request()
        .expect("the track is still cached")
        .expect("the track plays");
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(first.len(), payload.len());
}
