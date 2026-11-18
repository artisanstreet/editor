use super::{MIGRATIONS, Rollback, backup};
use artisan_api::{ItemKind, ItemsQuery, RunStatus, ThreadId, ToolKind};
use rusqlite::Connection;
use std::io::Read as _;
use std::path::Path;
use std::time::Duration;

const RELEASED_SCHEMA: usize = 26;
const RELEASED: &[u8] = include_bytes!("../fixtures/released/schema-26.sqlite3.gz");
const CLOSED_WITHIN: Duration = Duration::from_secs(20);

fn unpack(database: &Path) {
    let mut bytes = Vec::new();
    flate2::read::GzDecoder::new(RELEASED)
        .read_to_end(&mut bytes)
        .expect("unpack the released database");
    std::fs::write(database, bytes).expect("write the released database");
}

fn version(database: &Path) -> usize {
    Connection::open(database)
        .expect("open the database")
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("read the schema version")
}

fn titles(database: &Path) -> Vec<String> {
    let conn = Connection::open(database).expect("open the database");
    let mut select = conn
        .prepare(include_str!("released/thread_titles.sql"))
        .expect("prepare");
    select
        .query_map([], |row| row.get(0))
        .expect("query")
        .collect::<rusqlite::Result<_>>()
        .expect("read thread titles")
}

fn text(kind: &ItemKind) -> Option<String> {
    match kind {
        ItemKind::UserMessage { text, .. } => text.inline().map(|text| format!("you: {text}")),
        ItemKind::AssistantMessage { text, .. } => {
            text.inline().map(|text| format!("reply: {text}"))
        }
        ItemKind::Tool {
            tool: ToolKind::Shell { command, .. },
            output,
            ..
        } => Some(format!(
            "shell: {} -> {}",
            command.inline().unwrap_or("[stored]"),
            output
                .as_ref()
                .and_then(|output| output.text.inline())
                .unwrap_or("[no output]")
                .trim_end()
                .replace('\n', " ")
        )),
        ItemKind::Tool { .. }
        | ItemKind::Agent { .. }
        | ItemKind::Question { .. }
        | ItemKind::Plan { .. }
        | ItemKind::Image { .. }
        | ItemKind::Compaction { .. }
        | ItemKind::Notice { .. }
        | ItemKind::Error { .. } => None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_last_released_database_opens_with_its_threads_and_can_go_back_to_the_release() {
    let dir = tempfile::tempdir().expect("create the data folder");
    let database = dir.path().join("forge.sqlite3");
    unpack(&database);
    assert_eq!(version(&database), RELEASED_SCHEMA);

    let store = crate::Store::open(dir.path()).expect("today's Forge opens the released database");
    let state = store.state().await.expect("read the state");
    let thread = state
        .threads
        .iter()
        .find(|thread| thread.title.as_deref() == Some("Released thread"))
        .unwrap_or_else(|| panic!("the released thread is listed: {:#?}", state.threads));
    let project = state
        .projects
        .iter()
        .find(|project| project.id == thread.project_id)
        .expect("the released thread's project is listed");
    assert_eq!(project.name, "Released");
    assert_eq!(
        state.settings.get("suggestions"),
        Some(&serde_json::json!(false)),
        "settings saved by the release still apply"
    );

    let page = store
        .transcript_page(
            &thread.id,
            None,
            ItemsQuery {
                before: None,
                after: None,
                limit: Some(100),
            },
        )
        .await
        .expect("read the released transcript");
    let runs: Vec<RunStatus> = page.runs.iter().map(artisan_api::Run::status).collect();
    assert_eq!(runs, [RunStatus::Done, RunStatus::Done]);
    let shown: Vec<String> = page
        .items
        .iter()
        .filter_map(|item| text(&item.kind))
        .collect();
    assert_eq!(
        shown,
        [
            "you: List the files",
            "shell: ls -> Cargo.toml src",
            "reply: Answered: List the files",
            "you: Summarise the crate",
            "shell: ls -> Cargo.toml src",
            "reply: Answered: Summarise the crate",
        ]
    );
    let draft = store
        .draft(thread.id.as_str())
        .await
        .expect("read the released draft");
    assert_eq!(draft.body, serde_json::json!({"text": "Unsent words"}));
    let thread_id: ThreadId = thread.id.clone();
    drop(store);

    test_support::wait::wait_until("today's Forge to close the database", CLOSED_WITHIN, || {
        !dir.path().join("forge.sqlite3-wal").exists()
    });
    assert_eq!(version(&database), MIGRATIONS.len());
    let kept = dir
        .path()
        .join(format!("forge.sqlite3.schema-{RELEASED_SCHEMA}"));
    assert_eq!(
        version(&kept),
        RELEASED_SCHEMA,
        "the upgrade kept the released database"
    );

    match backup::roll_back(&database, RELEASED_SCHEMA).expect("go back to the release") {
        Rollback::Restored {
            found,
            restored,
            aside,
        } => {
            assert_eq!((found, restored), (MIGRATIONS.len(), RELEASED_SCHEMA));
            assert_eq!(version(&aside), MIGRATIONS.len());
        }
        Rollback::Current => panic!("the upgraded database was left in place for the release"),
    }
    assert_eq!(version(&database), RELEASED_SCHEMA);
    assert_eq!(
        titles(&database),
        ["Released thread"],
        "the release finds its thread {thread_id} again"
    );
}
