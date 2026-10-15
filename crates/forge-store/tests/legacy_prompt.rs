#![cfg(feature = "legacy-import")]
use forge_store::{Store, legacy};
use rusqlite::{Connection, params};
#[tokio::test]
async fn old_prompt_revisions_stream_atomically_and_repeat_import_is_idle() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.sqlite3");
    let mut source = Connection::open(&path).unwrap();
    source.execute_batch("CREATE TABLE global_prompt_versions(revision INTEGER PRIMARY KEY,text TEXT,saved_at_ms INTEGER);").unwrap();
    let tx = source.transaction().unwrap();
    for n in 1..=603 {
        tx.execute(
            "INSERT INTO global_prompt_versions VALUES(?1,?2,?1)",
            params![n, format!("revision {n}")],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    drop(source);
    let store = Store::open(&dir.path().join("new")).unwrap();
    let first = legacy::import(&path, dir.path(), &store).await.unwrap();
    assert_eq!(first.prompt_versions, 603);
    assert_eq!(store.prompt_version().await.unwrap(), 603);
    let head = store.head();
    let again = legacy::import(&path, dir.path(), &store).await.unwrap();
    assert_eq!(again.prompt_versions, 0);
    assert_eq!(store.head(), head);
}
#[tokio::test]
async fn oversized_old_prompt_rolls_back_all_prompt_rows_before_materializing_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.sqlite3");
    let source = Connection::open(&path).unwrap();
    source.execute_batch("CREATE TABLE global_prompt_versions(revision INTEGER PRIMARY KEY,text TEXT,saved_at_ms INTEGER); INSERT INTO global_prompt_versions VALUES(1,'first',1); INSERT INTO global_prompt_versions VALUES(2,zeroblob(33554433),2);").unwrap();
    drop(source);
    let store = Store::open(&dir.path().join("new")).unwrap();
    let head = store.head();
    let error = legacy::import(&path, dir.path(), &store).await.unwrap_err();
    assert!(format!("{error:#}").contains("exceeds 32 MiB"));
    assert_eq!(store.prompt_version().await.unwrap(), 0);
    assert_eq!(store.head(), head);
}
