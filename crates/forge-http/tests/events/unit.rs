use super::*;

fn live(text: &str, revision: u64) -> Envelope {
    Envelope {
        seq: None,
        live_revision: revision,
        event: Event::Live {
            thread_id: ThreadId("thread".into()),
            target: LiveTarget::Text {
                item_id: artisan_api::ItemId("item".into()),
            },
            text: text.into(),
            append: true,
            coverage: artisan_api::LiveCoverage {
                start: 0,
                total: text.len() as u64,
            },
        },
    }
}

#[tokio::test]
#[expect(
    clippy::significant_drop_tightening,
    reason = "the test holds its queue and frames until it ends"
)]
async fn byte_budget_blocks_even_when_the_entry_queue_has_space() {
    let envelope = live("abc", 1);
    let encoded = serde_json::to_string(&envelope).unwrap().len();
    let (sender, mut receiver) = mpsc::channel(64);
    let queue = Queue {
        sender,
        bytes: Arc::new(Semaphore::new(encoded)),
    };
    queue.send(envelope.clone()).await.unwrap();
    tokio::time::timeout(Duration::from_millis(10), queue.send(envelope.clone()))
        .await
        .unwrap_err();
    drop(receiver.recv().await.unwrap());
    queue.send(envelope).await.unwrap();
}

#[tokio::test]
#[expect(
    clippy::significant_drop_tightening,
    reason = "the test holds its queue and frames until it ends"
)]
async fn oversized_events_send_a_reset_without_retaining_the_large_body() {
    let (sender, mut receiver) = mpsc::channel(64);
    let queue = Queue {
        sender,
        bytes: Arc::new(Semaphore::new(QUEUE_BYTES)),
    };
    assert!(queue.send(live(&"x".repeat(QUEUE_BYTES), 1)).await.is_err());
    let frame = receiver.recv().await.unwrap();
    assert!(matches!(
        serde_json::from_str::<Envelope>(&frame.data).unwrap().event,
        Event::Reset
    ));
    assert_eq!(frame.seq, None);
    assert!(frame.data.len() < 128);
}

#[tokio::test]
async fn queued_live_deltas_covered_by_snapshot_are_not_replayed() {
    let directory = tempfile::tempdir().unwrap();
    let store = forge_store::Store::open(directory.path()).unwrap();
    let mut receiver = store.subscribe();
    store.publish_live(live("before", 0).event);
    let boundary = store.live_revision();
    let snapshot = "before";
    store.publish_live(live("after", 0).event);
    let mut rendered = snapshot.to_owned();
    let mut cursor = Cursor::after_snapshot(0, boundary, &[]);
    for _ in 0..2 {
        let envelope = receiver.recv().await.unwrap();
        if !cursor.accept(&envelope) {
            continue;
        }
        if let Event::Live { text, append, .. } = envelope.event {
            if append {
                rendered.push_str(&text);
            } else {
                rendered = text;
            }
        }
    }
    assert_eq!(rendered, "beforeafter");
}

#[tokio::test]
async fn online_revocation_closes_the_authorization_watcher_immediately() {
    let directory = tempfile::tempdir().unwrap();
    let store = forge_store::Store::open(directory.path()).unwrap();
    let device = store
        .add_device(
            "client".into(),
            forge_store::DeviceCertificate::from_der(b"certificate"),
        )
        .await
        .unwrap();
    let revocations = store.subscribe_revocations();
    let changes = store.subscribe_device_changes();
    store.revoke_device(&device).await.unwrap();
    tokio::time::timeout(
        Duration::from_millis(100),
        watch_authorization(
            &store,
            &device,
            &forge_store::DeviceCertificate::from_der(b"certificate"),
            revocations,
            changes,
        ),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn revocation_by_another_process_closes_a_passive_watcher() {
    let directory = tempfile::tempdir().unwrap();
    let store = forge_store::Store::open(directory.path()).unwrap();
    let device = store
        .add_device(
            "client".into(),
            forge_store::DeviceCertificate::from_der(b"certificate"),
        )
        .await
        .unwrap();
    let watcher = tokio::spawn({
        let store = store.clone();
        let device = device.clone();
        let revocations = store.subscribe_revocations();
        let changes = store.subscribe_device_changes();
        async move {
            watch_authorization(
                &store,
                &device,
                &forge_store::DeviceCertificate::from_der(b"certificate"),
                revocations,
                changes,
            )
            .await;
        }
    });
    store
        .add_device(
            "another client".into(),
            forge_store::DeviceCertificate::from_der(b"another certificate"),
        )
        .await
        .unwrap();
    tokio::task::yield_now().await;
    assert!(
        !watcher.is_finished(),
        "a paired device's stream stays open"
    );
    forge_store::admin::Admin::open(directory.path())
        .unwrap()
        .revoke_device(device.as_str())
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), watcher)
        .await
        .expect("a device revoked by `ae forge devices revoke` kept receiving events")
        .unwrap();
}
