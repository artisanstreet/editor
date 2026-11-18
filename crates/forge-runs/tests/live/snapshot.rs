use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn reconnecting_clients_receive_live_previews_in_the_same_order() {
    let started = Arc::new(Notify::new());
    let engine_started = Arc::clone(&started);
    let fixture = fixture(Arc::new(move |_, mut io| {
        let started = Arc::clone(&engine_started);
        Box::pin(async move {
            started.notify_one();
            tokio::select! {
                _ = io.inbox.recv() => Outcome::done(),
                () = io.cancel.cancelled() => Outcome::cancelled(),
            }
        })
    }))
    .await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("start"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), started.notified())
        .await
        .expect("the engine must start before publishing its live previews");
    for _ in 0..32 {
        fixture.hub.publish_live(
            &fixture.thread_id,
            LiveTarget::Shell {
                item_id: ItemId::new(),
            },
            "output".into(),
            false,
        );
    }
    let targets: Vec<LiveTarget> = fixture
        .hub
        .live_snapshot()
        .1
        .into_iter()
        .filter_map(|event| {
            if let Event::Live { target, .. } = event {
                Some(target)
            } else {
                None
            }
        })
        .collect();
    let mut ordered = targets.clone();
    ordered.sort();
    assert_eq!(targets.len(), 32);
    assert_eq!(
        targets, ordered,
        "the same live state must reach every reconnecting client in one order"
    );
    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    fixture.settled().await;
}
