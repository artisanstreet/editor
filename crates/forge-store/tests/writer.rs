use std::time::Duration;

const MIB: usize = 1024 * 1024;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stalled_writer_queues_at_most_its_byte_budget() {
    let directory = tempfile::tempdir().unwrap();
    let store = forge_store::Store::open(directory.path()).unwrap();
    let blocker = rusqlite::Connection::open(directory.path().join("forge.sqlite3")).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let writes: Vec<_> = (0..128)
        .map(|n| {
            let store = store.clone();
            tokio::spawn(async move {
                store
                    .set_setting(
                        format!("large-{n}"),
                        serde_json::Value::String("v".repeat(MIB)),
                    )
                    .await
            })
        })
        .collect();
    tokio::time::timeout(Duration::from_secs(4), async {
        while store.queued_write_bytes() < 60 * MIB {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the stalled writer fills its byte budget");
    for _ in 0..1000 {
        tokio::task::yield_now().await;
    }
    assert!(
        store.queued_write_bytes() <= 64 * MIB,
        "{} bytes queued behind a stalled writer",
        store.queued_write_bytes()
    );
    blocker.execute_batch("ROLLBACK").unwrap();
    let mut saved = 0;
    for write in writes {
        let result = tokio::time::timeout(Duration::from_secs(30), write)
            .await
            .expect("every queued write runs once the writer is free")
            .unwrap();
        saved += usize::from(result.is_ok());
    }
    assert!(saved > 0, "the settings fit in their own budget");
    assert_eq!(store.queued_write_bytes(), 0);
}
