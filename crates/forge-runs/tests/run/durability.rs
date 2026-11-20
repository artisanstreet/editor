use super::*;

pub(super) struct DiskFull(rusqlite::Connection);

impl DiskFull {
    pub(super) fn arming(fixture: &Fixture, table: &str) -> Self {
        let conn =
            rusqlite::Connection::open(fixture.dir.path().join("forge").join("forge.sqlite3"))
                .unwrap();
        conn.busy_timeout(test_support::wait::LIVENESS).unwrap();
        conn.execute_batch(&include_str!("arm_disk_full.sql").replace("__TABLE__", table))
            .unwrap();
        Self(conn)
    }

    pub(super) fn strike(&self) {
        self.0.execute_batch(include_str!("commit.sql")).unwrap();
    }

    pub(super) fn clear(self) {
        self.0
            .execute_batch(include_str!("drop_disk_full.sql"))
            .unwrap();
    }
}

async fn working(fixture: &Fixture, live: bool) {
    test_support::wait::within(
        "the thread's live state",
        test_support::wait::LIVENESS,
        async {
            while fixture.hub.is_live(&fixture.thread_id) != live {
                tokio::task::yield_now().await;
            }
        },
    )
    .await;
}

async fn stopped_turn(fixture: &Fixture) {
    fixture
        .hub
        .send_message(&fixture.thread_id, message("refactor the parser"))
        .await
        .unwrap();
    fixture
        .wait_for(|items| {
            items
                .iter()
                .any(|item| matches!(item.kind, ItemKind::Tool { .. }))
        })
        .await;
    fixture.hub.cancel(&fixture.thread_id).await.unwrap();
    assert_eq!(fixture.settled().await[0].status(), RunStatus::Cancelled);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_continue_whose_run_could_not_be_saved_is_still_waiting() {
    let asked = Asked::default();
    let fixture = fixture(stoppable(&asked)).await;
    stopped_turn(&fixture).await;
    fixture
        .hub
        .store()
        .request_continue(fixture.thread_id.clone())
        .await
        .unwrap();
    let disk = DiskFull::arming(&fixture, "runs");

    fixture.hub.resume_pending();
    working(&fixture, true).await;
    disk.strike();
    working(&fixture, false).await;

    assert!(
        fixture
            .hub
            .store()
            .continue_pending(&fixture.thread_id)
            .await
            .unwrap(),
        "the continue survives a run that could not start"
    );
    disk.clear();
    fixture.hub.resume_pending();
    let mut runs = fixture.settled().await;
    runs.sort_by_key(|run| run.started_at);
    let continued = runs
        .last()
        .expect("the continue ran once storage recovered");
    assert_eq!(
        continued.continuation,
        Some(artisan_api::Continuation::Requested),
        "{runs:#?}"
    );
    assert_eq!(continued.status(), RunStatus::Done, "{runs:#?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_whose_end_could_not_be_saved_settles_once_storage_recovers() {
    let release = Arc::new(Notify::new());
    let script: Script = Arc::new({
        let release = Arc::clone(&release);
        move |turn, io| {
            let release = Arc::clone(&release);
            Box::pin(async move {
                io.emit(EngineEvent::InputAccepted {
                    id: turn.input.id.clone(),
                })
                .await;
                release.notified().await;
                Outcome::failed("the working directory is gone")
            })
        }
    });
    let harness: Arc<dyn Harness> = Arc::new(Unscripted(script));
    let fixture = fixture_with_harness(Arc::new(move |_| Arc::clone(&harness))).await;
    fixture
        .hub
        .send_message(&fixture.thread_id, message("build it"))
        .await
        .unwrap();
    fixture
        .wait_for(|items| {
            items.iter().any(|item| {
                matches!(item.kind, ItemKind::UserMessage { .. }) && item.status == Status::Done
            })
        })
        .await;
    let disk = DiskFull::arming(&fixture, "runs");
    disk.strike();

    release.notify_one();
    working(&fixture, false).await;
    let stuck = fixture
        .hub
        .store()
        .latest_run(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stuck.status(), RunStatus::Running);
    assert!(
        fixture
            .items()
            .await
            .iter()
            .all(|item| !matches!(item.kind, ItemKind::Error { .. })),
        "the reason and the run's end are saved together"
    );

    disk.clear();
    test_support::wait::eventually(
        "the run to settle after storage recovers",
        test_support::wait::LIVENESS,
        || async {
            fixture
                .hub
                .store()
                .latest_run(&fixture.thread_id)
                .await
                .unwrap()
                .is_some_and(|run| run.status() == RunStatus::Failed)
        },
    )
    .await;
    assert!(fixture.items().await.iter().any(|item| matches!(
        &item.kind,
        ItemKind::Error { message, .. } if message.inline() == Some("the working directory is gone")
    )));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_that_cannot_start_fails_its_message_where_the_user_sees_it() {
    let fixture = fixture(Arc::new(|_, _| Box::pin(async { Outcome::done() }))).await;
    rusqlite::Connection::open(fixture.dir.path().join("forge").join("forge.sqlite3"))
        .unwrap()
        .execute_batch(include_str!("corrupt_transport.sql"))
        .unwrap();
    let sent = message("hello");
    fixture
        .hub
        .send_message(&fixture.thread_id, sent.clone())
        .await
        .unwrap();

    fixture
        .wait_for(|items| {
            items
                .iter()
                .any(|item| item.id == sent.id && item.status == Status::Failed)
                && items
                    .iter()
                    .any(|item| matches!(item.kind, ItemKind::Error { .. }))
        })
        .await;
    let run = fixture
        .hub
        .store()
        .latest_run(&fixture.thread_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.status(), RunStatus::Failed);
    working(&fixture, false).await;
}

async fn deferred_wait(fixture: &Fixture) -> Duration {
    let give_up = std::time::Instant::now() + test_support::wait::LIVENESS;
    loop {
        let now = tokio::time::Instant::now();
        let due = fixture.hub.lock().next_due(now);
        if let Some(due) = due {
            return due - now;
        }
        assert!(
            std::time::Instant::now() < give_up,
            "the failed start to wait before its retry"
        );
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn a_thread_whose_run_cannot_start_is_retried_less_and_less_often() {
    let fixture = fixture(Arc::new(|_, _| Box::pin(async { Outcome::done() }))).await;
    let disk = DiskFull::arming(&fixture, "runs");
    disk.strike();
    fixture
        .hub
        .send_message(&fixture.thread_id, message("build it"))
        .await
        .unwrap();

    let mut waits = Vec::new();
    for _ in 0..5 {
        let wait = deferred_wait(&fixture).await;
        waits.push(wait.as_secs());
        tokio::time::advance(wait).await;
    }

    assert_eq!(waits, [1, 2, 4, 8, 16]);
    disk.clear();
}
