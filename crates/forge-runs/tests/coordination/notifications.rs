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
    fixture.hub.stop(Duration::from_secs(2)).await;
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
    fixture.hub.stop(Duration::from_secs(2)).await;
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
    tokio::time::timeout(Duration::from_secs(10), async {
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
            tokio::time::sleep(Duration::from_millis(20)).await;
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
    restarted.stop(Duration::from_secs(2)).await;
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
    fixture.hub.stop(Duration::from_secs(2)).await;
}
