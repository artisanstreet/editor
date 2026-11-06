use super::*;
use std::sync::{Arc, mpsc};
use std::time::Duration;
use tokio::sync::oneshot;

type Reply = oneshot::Sender<Result<Arc<[u8]>, MediaFailure>>;

const OBJ: &[u8] = b"v 0 0 0\nv 2 0 0\nv 0 1 0\nf 1 2 3\n";
const LIMIT: Duration = Duration::from_secs(10);

struct Fixture {
    assets: ModelAssets,
    fetcher: MediaFetcher,
    calls: mpsc::Receiver<Reply>,
    wake: mpsc::Receiver<()>,
}

impl Fixture {
    fn new() -> Self {
        let (calls, requests) = mpsc::channel();
        let fetcher = MediaFetcher::new("forge", move |_| {
            let (reply, receive) = oneshot::channel();
            calls.send(reply).unwrap();
            Box::pin(async move { receive.await.unwrap_or(Err(MediaFailure::Unreadable)) })
        })
        .owned_by(&[7], 1);
        let assets = ModelAssets::new();
        let (wake, incoming) = mpsc::channel();
        assets.mailbox.borrow().set_waker(Arc::new(move || {
            let _ = wake.send(());
        }));
        Self {
            assets,
            fetcher,
            calls: requests,
            wake: incoming,
        }
    }
    fn request(
        &self,
        fetcher: &MediaFetcher,
        renderer: RendererCapability,
        format: Option<Format>,
        up: Option<Up>,
        waiter: u64,
    ) -> Option<Result<Arc<LoadedModel>, String>> {
        self.assets.request(
            Some(fetcher),
            &MediaSource::from("shape.obj".to_owned()),
            "model",
            format,
            up,
            renderer,
            waiter,
            Arc::new(|| {}),
        )
    }
    fn reply(&self, bytes: Result<Arc<[u8]>, MediaFailure>) {
        self.calls.recv_timeout(LIMIT).unwrap().send(bytes).unwrap();
        self.wake.recv_timeout(LIMIT).unwrap();
        assert!(self.assets.drain());
    }
}

#[test]
fn concurrent_waiters_share_one_real_obj_parse() {
    let fixture = Fixture::new();
    let mode = RendererCapability::Software;
    assert!(
        fixture
            .request(&fixture.fetcher, mode, None, None, 1)
            .is_none()
    );
    assert!(
        fixture
            .request(&fixture.fetcher, mode, None, None, 2)
            .is_none()
    );
    fixture.reply(Ok(Arc::from(OBJ)));
    let first = fixture
        .request(&fixture.fetcher, mode, None, None, 1)
        .unwrap()
        .unwrap();
    let second = fixture
        .request(&fixture.fetcher, mode, None, None, 2)
        .unwrap()
        .unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(first.triangles, 1);
    assert_eq!(first.format, Format::Obj);
    let Geometry::Software(mesh) = &first.geometry else {
        panic!("expected software geometry")
    };
    assert_eq!(mesh.positions.len(), 3);
    assert!(fixture.calls.try_recv().is_err());
}

#[test]
fn capability_format_and_up_select_distinct_cached_results() {
    let fixture = Fixture::new();
    let soft = RendererCapability::Software;
    assert!(
        fixture
            .request(&fixture.fetcher, soft, None, None, 1)
            .is_none()
    );
    fixture.reply(Ok(Arc::from(OBJ)));
    let original = fixture
        .request(&fixture.fetcher, soft, None, None, 1)
        .unwrap()
        .unwrap();
    assert!(
        fixture
            .request(&fixture.fetcher, RendererCapability::Mesh, None, None, 2)
            .is_none()
    );
    fixture.reply(Ok(Arc::from(OBJ)));
    let gpu = fixture
        .request(&fixture.fetcher, RendererCapability::Mesh, None, None, 2)
        .unwrap()
        .unwrap();
    let Geometry::Mesh(mesh) = &gpu.geometry else {
        panic!("expected packed mesh")
    };
    assert_eq!(mesh.vertices().len(), 3);
    assert!(
        fixture
            .request(&fixture.fetcher, soft, Some(Format::Obj), Some(Up::Z), 3)
            .is_none()
    );
    fixture.reply(Ok(Arc::from(OBJ)));
    let z = fixture
        .request(&fixture.fetcher, soft, Some(Format::Obj), Some(Up::Z), 3)
        .unwrap()
        .unwrap();
    let (Geometry::Software(a), Geometry::Software(b)) = (&original.geometry, &z.geometry) else {
        panic!("expected software geometry")
    };
    assert_ne!(a.positions, b.positions);
    assert!(
        fixture
            .request(&fixture.fetcher, soft, Some(Format::Stl), None, 4)
            .is_none()
    );
    fixture.reply(Ok(Arc::from(OBJ)));
    assert!(
        fixture
            .request(&fixture.fetcher, soft, Some(Format::Stl), None, 4)
            .unwrap()
            .is_err()
    );
    assert!(fixture.calls.try_recv().is_err());
}

#[test]
fn failure_is_cached_and_a_new_revision_fetches_fresh_data() {
    let fixture = Fixture::new();
    let mode = RendererCapability::Software;
    assert!(
        fixture
            .request(&fixture.fetcher, mode, None, None, 1)
            .is_none()
    );
    fixture.reply(Err(MediaFailure::Missing));
    assert_eq!(
        fixture
            .request(&fixture.fetcher, mode, None, None, 1)
            .unwrap()
            .err()
            .as_deref(),
        Some("the file does not exist")
    );
    assert_eq!(
        fixture
            .request(&fixture.fetcher, mode, None, None, 2)
            .unwrap()
            .err()
            .as_deref(),
        Some("the file does not exist")
    );
    let next = fixture.fetcher.clone().owned_by(&[7], 2);
    assert!(fixture.request(&next, mode, None, None, 3).is_none());
    fixture.reply(Ok(Arc::from(OBJ)));
    assert_eq!(
        fixture
            .request(&next, mode, None, None, 3)
            .unwrap()
            .unwrap()
            .triangles,
        1
    );
    assert!(fixture.calls.try_recv().is_err());
}

#[test]
fn removing_owner_cancels_pending_fetch_and_recreation_loads_again() {
    let fixture = Fixture::new();
    let mode = RendererCapability::Software;
    assert!(
        fixture
            .request(&fixture.fetcher, mode, None, None, 1)
            .is_none()
    );
    let mut abandoned = fixture.calls.recv_timeout(LIMIT).unwrap();
    let (cancelled, observed) = mpsc::channel();
    runtime::runtime().spawn(async move {
        abandoned.closed().await;
        let _ = cancelled.send(());
    });
    fixture.fetcher.retain_resources(&[]);
    observed.recv_timeout(LIMIT).unwrap();
    assert!(
        fixture
            .request(&fixture.fetcher, mode, None, None, 2)
            .is_none()
    );
    fixture.reply(Ok(Arc::from(OBJ)));
    assert_eq!(
        fixture
            .request(&fixture.fetcher, mode, None, None, 2)
            .unwrap()
            .unwrap()
            .triangles,
        1
    );
}
