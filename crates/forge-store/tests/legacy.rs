use artisan_api::{ItemKind, ItemsQuery, Permissions, Phase, RunStatus, ThreadId};
use forge_store::{Store, legacy};
use rusqlite::{Connection, params};
use std::io::Cursor;
use std::path::Path;

const PROJECT: &str = "935ec1e66614653a701ef1538d77c15c";
const THREAD: &str = "25b31fa1131f0a58783a42216a0bcb49";
const SESSION: &str = "6ff23a05-91db-4bb5-8c21-67cb013d16a8";

/// The old tables the import reads, as the old migrations left them.
fn old_database(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        r"
        CREATE TABLE attached_projects (project_id varchar PRIMARY KEY, root_path TEXT NOT NULL,
            display_name varchar, attached_at_ms INTEGER NOT NULL);
        CREATE TABLE threads (thread_id varchar PRIMARY KEY, project_id varchar NOT NULL,
            title TEXT, created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL,
            engine_run_config_version INTEGER, engine_run_config_revision INTEGER,
            engine_run_config BLOB);
        CREATE TABLE thread_reads (thread_id TEXT PRIMARY KEY, read_at_ms INTEGER NOT NULL);
        CREATE TABLE messages (message_id varchar PRIMARY KEY, thread_id varchar NOT NULL,
            ordinal INTEGER NOT NULL, body TEXT NOT NULL, accepted_at_ms INTEGER NOT NULL);
        CREATE TABLE message_image_attachments (message_id TEXT NOT NULL, position INTEGER NOT NULL,
            mime_type TEXT NOT NULL, name TEXT, size_bytes INTEGER NOT NULL, bytes BLOB NOT NULL);
        CREATE TABLE conversation_items (item_id varchar PRIMARY KEY, thread_id varchar NOT NULL,
            turn_id varchar NOT NULL, ordinal INTEGER NOT NULL, kind varchar NOT NULL,
            revision INTEGER NOT NULL, lifecycle varchar NOT NULL, item_kind varchar,
            source_message_id varchar, run_id varchar, native_item_key TEXT, phase varchar,
            body TEXT, created_at_ms INTEGER NOT NULL, updated_at_ms INTEGER NOT NULL,
            text_after_sequence INTEGER);
        CREATE TABLE assistant_runs (run_id varchar PRIMARY KEY, thread_id varchar NOT NULL,
            lifecycle varchar NOT NULL, provider_binding BLOB, error_code varchar,
            error_message TEXT, created_at_ms INTEGER NOT NULL, terminal_at_ms INTEGER,
            engine_run_config BLOB);
        CREATE TABLE run_usage (run_id TEXT NOT NULL, thread_id TEXT NOT NULL,
            source_sequence INTEGER NOT NULL, model_id TEXT, input_tokens INTEGER,
            cached_input_tokens INTEGER, output_tokens INTEGER, context_tokens INTEGER,
            context_window_tokens INTEGER);
        CREATE TABLE global_prompt (state_id INTEGER NOT NULL PRIMARY KEY,
            revision INTEGER NOT NULL DEFAULT 0, established INTEGER NOT NULL DEFAULT 0,
            text TEXT NOT NULL DEFAULT '', updated_at_ms INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE global_prompt_versions (revision INTEGER NOT NULL PRIMARY KEY,
            text TEXT NOT NULL, saved_at_ms INTEGER NOT NULL);
        ",
    )
    .unwrap();
    let config = br#"{"version":2,"engine":"claude","permission":{"approval":"never","filesystem":"host"},
        "details":{"model_id":"claude-opus-5-5","effort":"medium","permission_mode":"bypassPermissions"}}"#;
    conn.execute(
        "INSERT INTO attached_projects VALUES (?1, '/home/sander/projects/varde', 'varde', 1)",
        [PROJECT],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO threads VALUES (?1, ?2, 'Fix the header', 10, 50, 2, 1, ?3)",
        params![THREAD, PROJECT, &config[..]],
    )
    .unwrap();
    conn.execute("INSERT INTO thread_reads VALUES (?1, 60)", [THREAD])
        .unwrap();
    conn.execute(
        "INSERT INTO messages VALUES ('m1', ?1, 1, 'Why is the header blurry?', 20)",
        [THREAD],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO message_image_attachments VALUES ('m1', 0, 'image/png', 'Pasted image.png', 0, ?1)",
        [png()],
    )
    .unwrap();
    let items = [
        (
            "u1",
            1,
            "completed",
            "user_message",
            Some("m1"),
            None,
            None,
            "Why is the header blurry?",
        ),
        (
            "a1",
            2,
            "completed",
            "assistant_message",
            None,
            Some("r1"),
            Some("commentary"),
            "Looking at the CSS.",
        ),
        (
            "a2",
            3,
            "completed",
            "assistant_message",
            None,
            Some("r1"),
            Some("final"),
            "It was scaled by 1.01; fixed.",
        ),
    ];
    for (id, ordinal, lifecycle, kind, message, run, phase, body) in items {
        conn.execute(
            "INSERT INTO conversation_items VALUES
                (?1, ?2, 't1', ?3, 'item', 0, ?4, ?5, ?6, ?7, NULL, ?8, ?9, ?10, ?10, NULL)",
            params![
                id,
                THREAD,
                ordinal,
                lifecycle,
                kind,
                message,
                run,
                phase,
                body,
                20 + ordinal
            ],
        )
        .unwrap();
    }
    let binding = format!(r#"{{"engine":"claude","format":1,"session_id":"{SESSION}"}}"#);
    conn.execute(
        "INSERT INTO assistant_runs VALUES ('r0', ?1, 'failed', NULL, 'provider_failed', NULL, 5, 6, NULL)",
        [THREAD],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO assistant_runs VALUES ('r1', ?1, 'completed', ?2, NULL, NULL, 20, 40, ?3)",
        params![THREAD, binding.as_bytes(), &config[..]],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO run_usage VALUES ('r1', ?1, 7, 'claude-opus-5-5', 2, 1000, 30, 1032, 400000)",
        [THREAD],
    )
    .unwrap();
    conn.execute_batch(
        "INSERT INTO global_prompt_versions VALUES (1, 'Answer in English.', 60);
         INSERT INTO global_prompt_versions VALUES (2, 'Answer in British English.', 70);
         INSERT INTO global_prompt VALUES (1, 2, 1, 'Answer in British English.', 70);",
    )
    .unwrap();
}

fn png() -> Vec<u8> {
    let image =
        image::RgbImage::from_fn(32, 16, |x, y| image::Rgb([x as u8 * 8, y as u8 * 16, 90]));
    let mut bytes = Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}

#[tokio::test(flavor = "multi_thread")]
async fn old_thread_reads_the_same_after_importing_twice() {
    let dir = tempfile::tempdir().unwrap();
    let old_db = dir.path().join("old.sqlite3");
    old_database(&old_db);
    let store = Store::open_in_memory(&dir.path().join("blobs")).unwrap();

    let first = legacy::import(&old_db, dir.path(), &store).await.unwrap();
    assert_eq!(
        (
            first.projects,
            first.threads,
            first.items,
            first.runs,
            first.images,
            first.prompt_versions
        ),
        (1, 1, 3, 1, 1, 2),
        "{first:?}"
    );
    let second = legacy::import(&old_db, dir.path(), &store).await.unwrap();
    assert_eq!(
        (
            second.projects,
            second.threads,
            second.items,
            second.runs,
            second.images,
            second.prompt_versions
        ),
        (0, 0, 0, 0, 0, 0),
        "{second:?}"
    );

    let state = store.state().await.unwrap();
    assert_eq!(state.projects.len(), 1);
    assert_eq!(state.projects[0].path, "/home/sander/projects/varde");
    assert_eq!(state.projects[0].created_at, 1, "attached time kept");
    let [thread] = state.threads.as_slice() else {
        panic!("one thread expected: {:?}", state.threads)
    };
    assert_eq!(thread.id, ThreadId::from(THREAD));
    assert_eq!(thread.title.as_deref(), Some("Fix the header"));
    assert_eq!(thread.config.model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(
        thread.config.permissions,
        Permissions::Full,
        "a thread that never asked keeps running without asking"
    );
    assert!(!thread.is_unread(), "the thread was read in the old Editor");
    assert_eq!(
        (thread.created_at, thread.updated_at),
        (10, 50),
        "old thread times kept"
    );
    assert_eq!(
        state.settings["global_prompt"], "Answer in British English.",
        "Settings shows the prompt the old Forge gave the engines"
    );
    assert_eq!(
        store.prompt_version().await.unwrap(),
        2,
        "the next save follows the old history"
    );

    let page = store
        .items_page(
            &thread.id,
            ItemsQuery {
                before: None,
                after: None,
                limit: Some(50),
            },
        )
        .await
        .unwrap();
    let shown: Vec<_> = page
        .items
        .iter()
        .map(|item| (item.id.as_str(), &item.kind))
        .collect();
    let [
        ("u1", ItemKind::UserMessage { text, images, .. }),
        (
            "a1",
            ItemKind::AssistantMessage {
                phase: Phase::Commentary,
                ..
            },
        ),
        (
            "a2",
            ItemKind::AssistantMessage {
                text: reply,
                phase: Phase::Final,
            },
        ),
    ] = shown.as_slice()
    else {
        panic!("items out of shape: {shown:#?}")
    };
    let times: Vec<_> = page
        .items
        .iter()
        .map(|item| (item.created_at, item.updated_at))
        .collect();
    assert_eq!(times, [(21, 21), (22, 22), (23, 23)], "message times kept");
    assert_eq!(text, "Why is the header blurry?");
    assert_eq!(reply, "It was scaled by 1.01; fixed.");
    let [image] = images.as_slice() else {
        panic!("one image expected")
    };
    let blob = std::fs::read(store.blob_path(image).expect("image blob stored")).unwrap();
    assert_eq!((&blob[..4], &blob[8..12]), (&b"RIFF"[..], &b"WEBP"[..]));
    assert_eq!(image::load_from_memory(&blob).unwrap().width(), 32);

    let run = store
        .latest_run(&thread.id)
        .await
        .unwrap()
        .expect("latest run imported");
    assert_eq!(run.session_id.as_deref(), Some(SESSION));
    assert_eq!(run.status, RunStatus::Done);
    assert_eq!(run.usage.cache_read, 1000);
}
