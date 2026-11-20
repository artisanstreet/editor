use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn a_parked_turn_resumes_on_time_even_when_its_due_time_could_not_be_read() {
    let asked = Asked::default();
    let fixture = fixture(failing_then_done(&asked, vec![at_capacity()])).await;
    let conn =
        rusqlite::Connection::open(fixture.dir.path().join("forge").join("forge.sqlite3")).unwrap();
    conn.busy_timeout(test_support::wait::LIVENESS).unwrap();
    conn.execute_batch(include_str!("unreadable_due.sql"))
        .unwrap();
    fixture
        .hub
        .send_message(&fixture.thread_id, message("refactor the parser"))
        .await
        .unwrap();
    let due = fixture.parked_after(None).await;
    assert!(
        fixture.hub.store().next_parked().await.is_err(),
        "the parked turn's due time can't be read"
    );

    fixture
        .clock
        .advance((due + 1 - fixture.hub.store().now()).max(0));

    fixture.latest_run_once(RunStatus::Done).await;
    let statuses: Vec<_> = fixture
        .runs()
        .await
        .iter()
        .map(artisan_api::Run::status)
        .collect();
    assert_eq!(statuses, [RunStatus::Failed, RunStatus::Done]);
}
