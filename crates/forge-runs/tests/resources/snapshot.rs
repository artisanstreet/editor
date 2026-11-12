use super::*;
use crate::resources::{ResourceError, Written};

async fn kept(fixture: &Fixture, item: &ItemId, written: &Written) -> Vec<u8> {
    let thread = fixture
        .hub
        .store()
        .thread(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let snapshot = fixture.hub.store().resource_snapshot_target(
                &fixture.thread_id,
                item,
                &written.path,
                written.from_home,
            );
            if snapshot.is_some_and(|path| path.is_file()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the reply's image is kept");
    fixture
        .hub
        .read_resource(&thread, Some(item), written)
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_image_a_reply_shows_outlives_its_file_and_later_versions() {
    let fixture = fixture(Arc::new(|turn, io| {
        Box::pin(async move {
            let key = format!("reply-{}", turn.input.text);
            let text = "```artisan-image\n{ \"src\": \"shots/latest.png\" }\n```";
            io.emit(reply(&key, text)).await;
            Outcome::done()
        })
    }))
    .await;
    let shots = fixture.dir.path().join("shots");
    std::fs::create_dir_all(&shots).unwrap();
    let written = Written {
        path: "shots/latest.png".to_owned(),
        from_home: false,
    };

    std::fs::write(shots.join("latest.png"), b"first render").unwrap();
    fixture
        .hub
        .send_message(&fixture.thread_id, message("one"))
        .await
        .unwrap();
    fixture.settled().await;
    let first = fixture.by_native("reply-one").await.id;
    assert_eq!(kept(&fixture, &first, &written).await, b"first render");

    std::fs::write(shots.join("latest.png"), b"second render").unwrap();
    fixture
        .hub
        .send_message(&fixture.thread_id, message("two"))
        .await
        .unwrap();
    fixture.settled().await;
    let second = fixture.by_native("reply-two").await.id;
    assert_eq!(kept(&fixture, &second, &written).await, b"second render");

    std::fs::remove_dir_all(&shots).unwrap();
    assert_eq!(kept(&fixture, &first, &written).await, b"first render");
    assert_eq!(kept(&fixture, &second, &written).await, b"second render");
    let thread = fixture
        .hub
        .store()
        .thread(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        fixture.hub.read_resource(&thread, None, &written).await,
        Err(ResourceError::Missing)
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_first_seen_when_viewed_is_kept_from_then_on() {
    let fixture = fixture(Arc::new(|_, _| Box::pin(async { Outcome::done() }))).await;
    let thread = fixture
        .hub
        .store()
        .thread(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap();
    let file = fixture.dir.path().join("plot.png");
    let written = Written {
        path: "plot.png".to_owned(),
        from_home: false,
    };
    let item = ItemId::new();
    assert!(matches!(
        fixture
            .hub
            .read_resource(&thread, Some(&item), &written)
            .await,
        Err(ResourceError::Missing)
    ));
    std::fs::write(&file, b"plot").unwrap();
    assert_eq!(
        fixture
            .hub
            .read_resource(&thread, Some(&item), &written)
            .await
            .unwrap(),
        b"plot"
    );
    std::fs::remove_file(&file).unwrap();
    assert_eq!(
        fixture
            .hub
            .read_resource(&thread, Some(&item), &written)
            .await
            .unwrap(),
        b"plot"
    );
    assert!(matches!(
        fixture
            .hub
            .read_resource(&thread, Some(&ItemId::new()), &written)
            .await,
        Err(ResourceError::Missing)
    ));
}
