use super::*;
use forge_store::coordination::{Content, Intent, MessageState, Update, WatchState, WorkState};

async fn other_thread(fixture: &Fixture) -> ThreadId {
    let project = fixture
        .hub
        .store()
        .thread(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap()
        .project_id;
    let id = ThreadId::new();
    fixture
        .hub
        .store()
        .create_thread(CreateThread {
            id: id.clone(),
            project_id: project,
            config: EngineConfig::new(EngineId::Claude),
            title: Some("Dependency".into()),
            parent_thread_id: None,
            spawned_by: None,
        })
        .await
        .unwrap();
    id
}

fn work(state: WorkState) -> Content {
    Content::Work {
        objective: "Update shared API".into(),
        worktree: "/repo".into(),
        state,
        resources: vec![],
        dependencies: vec![],
        summary: "Constructors and callers updated".into(),
        evidence: vec!["cargo check passed".into()],
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn request_retries_do_not_duplicate_delivery_and_acknowledgments_notify_the_sender() {
    let fixture = fixture(Arc::new(|_, _| Box::pin(async { Outcome::done() }))).await;
    let recipient = other_thread(&fixture).await;
    let record = fixture
        .hub
        .store()
        .coordination_put(
            fixture.thread_id.clone(),
            None,
            Content::Message {
                recipient: recipient.clone(),
                intent: Intent::Boundary,
                text: "Leave the parser module to me".into(),
                state: MessageState::Prepared,
                reply: None,
                delivery_item: None,
                receipt_revision: 0,
            },
            None,
        )
        .await
        .unwrap();
    let first = fixture
        .hub
        .deliver_coordination_message(&fixture.thread_id, record.id.clone())
        .await
        .unwrap();
    let second = fixture
        .hub
        .deliver_coordination_message(&fixture.thread_id, record.id.clone())
        .await
        .unwrap();
    assert_eq!(first, second);
    let messages = fixture
        .hub
        .store()
        .items_page(
            &recipient,
            ItemsQuery {
                limit: Some(100),
                before: None,
                after: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        messages
            .items
            .iter()
            .filter(|item| matches!(item.kind, ItemKind::UserMessage { .. }))
            .count(),
        1
    );
    fixture
        .hub
        .acknowledge_coordination(
            &recipient,
            record.id.clone(),
            first.revision,
            MessageState::Acknowledged,
            Some("Agreed; I will edit only the caller".into()),
        )
        .await
        .unwrap();
    fixture.wait_for(|items|items.iter().any(|item|matches!(&item.kind,ItemKind::UserMessage { text,.. } if text.inline().is_some_and(|text|text.contains("Coordination receipt:"))))).await;
    fixture.hub.stop(test_support::wait::LIVENESS).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn dependency_watches_survive_controller_restart_and_preserve_one_notification() {
    let fixture = fixture(Arc::new(|_, _| Box::pin(async { Outcome::done() }))).await;
    let target = other_thread(&fixture).await;
    let task = fixture
        .hub
        .store()
        .coordination_put(target.clone(), None, work(WorkState::Running), None)
        .await
        .unwrap();
    let watch = fixture
        .hub
        .store()
        .coordination_put(
            fixture.thread_id.clone(),
            None,
            Content::Watch {
                target: target.clone(),
                state: WatchState::Pending,
                after_revision: task.revision,
                outcome: None,
            },
            None,
        )
        .await
        .unwrap();
    fixture.hub.stop(test_support::wait::LIVENESS).await;
    fixture
        .hub
        .store()
        .coordination_put(
            target,
            None,
            work(WorkState::Completed),
            Some(task.revision),
        )
        .await
        .unwrap();
    let harness: Arc<dyn Harness> = Arc::new(Scripted(Arc::new(|_, _| {
        Box::pin(async { Outcome::done() })
    })));
    let restarted = Hub::with_harness(
        fixture.hub.store().clone(),
        Engines::new(fixture.dir.path()),
        fixture.dir.path().to_path_buf(),
        Arc::new(move |_| Arc::clone(&harness)),
    );
    let mut changes = restarted.store().watch_coordination();
    tokio::time::timeout(test_support::wait::LIVENESS, async {
        loop {
            let current = restarted
                .store()
                .coordination_get(watch.id.clone())
                .await
                .unwrap()
                .unwrap();
            if matches!(
                current.content,
                Content::Watch {
                    state: WatchState::Delivered,
                    ..
                }
            ) {
                break;
            }
            changes.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let items = restarted
        .store()
        .items_page(
            &fixture.thread_id,
            ItemsQuery {
                limit: Some(100),
                before: None,
                after: None,
            },
        )
        .await
        .unwrap()
        .items;
    assert_eq!(items.iter().filter(|item|matches!(&item.kind,ItemKind::UserMessage { text,.. } if text.inline().is_some_and(|text|text.contains("Work dependency")))).count(),1);
    restarted.stop(test_support::wait::LIVENESS).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelled_watches_never_start_a_notification_turn() {
    let fixture = fixture(Arc::new(|_, _| Box::pin(async { Outcome::done() }))).await;
    let target = other_thread(&fixture).await;
    let task = fixture
        .hub
        .store()
        .coordination_put(target.clone(), None, work(WorkState::Running), None)
        .await
        .unwrap();
    let watch = fixture
        .hub
        .store()
        .coordination_put(
            fixture.thread_id.clone(),
            None,
            Content::Watch {
                target: target.clone(),
                state: WatchState::Pending,
                after_revision: task.revision,
                outcome: None,
            },
            None,
        )
        .await
        .unwrap();
    fixture
        .hub
        .store()
        .coordination_update(
            fixture.thread_id.clone(),
            watch.id,
            watch.revision,
            Update::FinishWatch {
                state: WatchState::Cancelled,
                outcome: "Independent path chosen".into(),
            },
        )
        .await
        .unwrap();
    fixture
        .hub
        .store()
        .coordination_put(
            target,
            None,
            work(WorkState::Completed),
            Some(task.revision),
        )
        .await
        .unwrap();
    assert!(
        fixture
            .hub
            .store()
            .coordination_watches()
            .await
            .unwrap()
            .is_empty()
    );
    assert!(fixture.items().await.is_empty());
    fixture.hub.stop(test_support::wait::LIVENESS).await;
}

async fn acknowledged_request(
    fixture: &Fixture,
    recipient: &ThreadId,
    text: &str,
) -> (String, u64) {
    let record = fixture
        .hub
        .store()
        .coordination_put(
            fixture.thread_id.clone(),
            None,
            Content::Message {
                recipient: recipient.clone(),
                intent: Intent::Boundary,
                text: text.into(),
                state: MessageState::Prepared,
                reply: None,
                delivery_item: None,
                receipt_revision: 0,
            },
            None,
        )
        .await
        .unwrap();
    let delivered = fixture
        .hub
        .deliver_coordination_message(&fixture.thread_id, record.id.clone())
        .await
        .unwrap();
    (record.id, delivered.revision)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_receipt_that_cannot_be_delivered_does_not_hold_up_the_others() {
    let fixture = fixture(Arc::new(|_, _| Box::pin(async { Outcome::done() }))).await;
    let recipient = other_thread(&fixture).await;
    let (stuck, stuck_revision) =
        acknowledged_request(&fixture, &recipient, "Leave the parser to me").await;
    let (fine, fine_revision) =
        acknowledged_request(&fixture, &recipient, "Leave the lexer to me").await;
    let mut taken = message("an unrelated message");
    taken.id = ItemId::from(format!(
        "coordination-receipt:{stuck}:{}",
        stuck_revision + 1
    ));
    fixture.hub.send_message(&recipient, taken).await.unwrap();

    for (id, revision) in [(&stuck, stuck_revision), (&fine, fine_revision)] {
        let acknowledged = fixture
            .hub
            .acknowledge_coordination(
                &recipient,
                id.clone(),
                revision,
                MessageState::Acknowledged,
                None,
            )
            .await
            .unwrap();
        assert_eq!(acknowledged.revision, revision + 1);
    }

    let expected = format!("request {fine} as Acknowledged");
    fixture
        .wait_for(|items| {
            items.iter().any(|item| {
                matches!(&item.kind, ItemKind::UserMessage { text, .. }
                    if text.inline().is_some_and(|text| text.contains(&expected)))
            })
        })
        .await;
    let undeliverable = format!("request {stuck} as");
    assert!(!fixture.items().await.iter().any(|item| matches!(
        &item.kind,
        ItemKind::UserMessage { text, .. }
            if text.inline().is_some_and(|text| text.contains(&undeliverable))
    )));
    fixture.hub.stop(test_support::wait::LIVENESS).await;
}

#[tokio::test(start_paused = true)]
async fn a_quiet_forge_does_no_periodic_store_work() {
    let fixture = fixture(Arc::new(|_, _| Box::pin(async { Outcome::done() }))).await;
    let store = fixture.hub.store().clone();
    let activity = || (store.reads(), store.commits());
    let mut settled = activity();
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_mins(1)).await;
        let now = activity();
        if now == settled {
            break;
        }
        settled = now;
    }
    tokio::time::sleep(Duration::from_hours(1)).await;
    assert_eq!(
        activity(),
        settled,
        "an idle Forge read or wrote its store with nothing to do"
    );
    fixture.hub.stop(test_support::wait::LIVENESS).await;
}
