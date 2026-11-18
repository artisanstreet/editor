use super::*;

struct DiskFull(rusqlite::Connection);

impl DiskFull {
    fn arming(fixture: &Fixture, table: &str) -> Self {
        let conn =
            rusqlite::Connection::open(fixture.dir.path().join("forge").join("forge.sqlite3"))
                .unwrap();
        conn.busy_timeout(Duration::from_secs(5)).unwrap();
        conn.execute_batch(&format!(
            "BEGIN IMMEDIATE;
             CREATE TRIGGER disk_full_insert BEFORE INSERT ON {table}
                 BEGIN SELECT RAISE(ABORT, 'database or disk is full'); END;
             CREATE TRIGGER disk_full_update BEFORE UPDATE ON {table}
                 BEGIN SELECT RAISE(ABORT, 'database or disk is full'); END;"
        ))
        .unwrap();
        Self(conn)
    }

    fn strike(&self) {
        self.0.execute_batch("COMMIT").unwrap();
    }

    fn clear(self) {
        self.0
            .execute_batch("DROP TRIGGER disk_full_insert; DROP TRIGGER disk_full_update;")
            .unwrap();
    }
}

async fn working(fixture: &Fixture, live: bool) {
    test_support::wait::within("the thread's live state", Duration::from_secs(10), async {
        while fixture.hub.is_live(&fixture.thread_id) != live {
            tokio::task::yield_now().await;
        }
    })
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
    assert_eq!(fixture.settled().await[0].status, RunStatus::Cancelled);
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
    let continued = fixture.settled().await.remove(0);
    assert_eq!(
        continued.continuation,
        Some(artisan_api::Continuation::Requested)
    );
    assert_eq!(continued.status, RunStatus::Done);
}
