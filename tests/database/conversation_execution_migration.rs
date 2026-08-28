//! External behavior tests for the immutable conversation execution
//! migration (third native schema step).

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use artisan_database::{SqliteConfig, connect};
use artisan_migrations::{Migrator, migrate_to_current};
use sea_orm_migration::MigratorTrait;
use sea_orm_migration::sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};

const INITIAL_MIGRATION: &str = "m20260824_000001_initial_native_schema";
const RECEIPTS_MIGRATION: &str = "m20260824_000002_global_command_receipts";
const EXECUTION_MIGRATION: &str = "m20260824_000003_conversation_execution";

const BASE_TABLES: [&str; 5] = [
    "attached_projects",
    "threads",
    "messages",
    "message_dispatches",
    "command_receipts",
];

const EXECUTION_TABLES: [&str; 8] = [
    "conversation_state",
    "conversation_ordinals",
    "conversation_turns",
    "assistant_runs",
    "conversation_items",
    "conversation_patches",
    "run_checkpoints",
    "run_batch_receipts",
];

/// Every explicitly created execution index: uniqueness and composite
/// identity only. The schema deliberately declares no speculative
/// non-unique indexes.
const EXECUTION_INDEXES: [&str; 12] = [
    "uq_messages_message_id_thread_id",
    "uq_conversation_ordinals_entity_id",
    "uq_conversation_ordinals_thread_id_ordinal_entity_id_kind",
    "uq_conversation_turns_turn_id_thread_id",
    "uq_assistant_runs_run_start_key",
    "uq_assistant_runs_origin_message_id",
    "uq_assistant_runs_origin_turn_id",
    "uq_assistant_runs_run_id_thread_id",
    "uq_conversation_items_source_message_id",
    "uq_conversation_items_run_id_native_item_key",
    "uq_conversation_items_item_id_thread_id",
    "uq_conversation_patches_thread_id_sequence",
];

/// Every approved foreign key as child-column chains rendered into
/// `child->parent.parent_column` entries joined by `|`, one entry per
/// composite key, in `pragma_foreign_key_list` column order.
const APPROVED_FOREIGN_KEYS: [(&str, &[&str]); 8] = [
    ("conversation_state", &["thread_id->threads.thread_id"]),
    ("conversation_ordinals", &["thread_id->threads.thread_id"]),
    ("conversation_turns", &[]),
    (
        "assistant_runs",
        &[
            "origin_message_id->messages.message_id|thread_id->messages.thread_id",
            "origin_turn_id->conversation_turns.turn_id|thread_id->conversation_turns.thread_id",
        ],
    ),
    ("conversation_items", &[]),
    (
        "conversation_patches",
        &[
            "turn_id->conversation_turns.turn_id|thread_id->conversation_turns.thread_id",
            "item_id->conversation_items.item_id|thread_id->conversation_items.thread_id",
            "run_id->assistant_runs.run_id|thread_id->assistant_runs.thread_id",
        ],
    ),
    ("run_checkpoints", &["run_id->assistant_runs.run_id"]),
    ("run_batch_receipts", &["run_id->assistant_runs.run_id"]),
];

fn approved_checks(table: &str) -> &'static [&'static str] {
    match table {
        "conversation_state" => &[
            "ck_conversation_state_next_renderer_ordinal_nonnegative",
            "ck_conversation_state_last_patch_sequence_nonnegative",
        ],
        "conversation_ordinals" => &[
            "ck_conversation_ordinals_ordinal_nonnegative",
            "ck_conversation_ordinals_kind",
        ],
        "conversation_turns" => &[
            "ck_conversation_turns_kind_fixed",
            "ck_conversation_turns_revision_nonnegative",
            "ck_conversation_turns_lifecycle",
            "ck_conversation_turns_updated_not_before_created",
        ],
        "assistant_runs" => &[
            "ck_assistant_runs_generation_nonnegative",
            "ck_assistant_runs_lifecycle",
            "ck_assistant_runs_run_start_key_exact_bytes",
            "ck_assistant_runs_fence_keys_exact_bytes",
            "ck_assistant_runs_provider_binding_tuple",
            "ck_assistant_runs_error_pair_shape",
            "ck_assistant_runs_error_lifecycle_pairing",
            "ck_assistant_runs_terminal_at",
            "ck_assistant_runs_state_shape",
            "ck_assistant_runs_updated_not_before_created",
        ],
        "conversation_items" => &[
            "ck_conversation_items_kind_fixed",
            "ck_conversation_items_revision_nonnegative",
            "ck_conversation_items_lifecycle",
            "ck_conversation_items_item_kind",
            "ck_conversation_items_phase",
            "ck_conversation_items_body_bytes",
            "ck_conversation_items_shape",
            "ck_conversation_items_updated_not_before_created",
        ],
        "conversation_patches" => &[
            "ck_conversation_patches_sequence_positive",
            "ck_conversation_patches_revision_nonnegative",
            "ck_conversation_patches_kind",
            "ck_conversation_patches_phase_enum",
            "ck_conversation_patches_ordinal_nonnegative",
            "ck_conversation_patches_lifecycle_enum",
            "ck_conversation_patches_item_kind_enum",
            "ck_conversation_patches_body_bytes",
            "ck_conversation_patches_fragment_bytes",
            "ck_conversation_patches_entity_times_ordered",
            "ck_conversation_patches_payload",
        ],
        "run_checkpoints" => &[
            "ck_run_checkpoints_generation_nonnegative",
            "ck_run_checkpoints_last_batch_sequence_nonnegative",
            "ck_run_checkpoints_engine_checkpoint_tuple",
        ],
        "run_batch_receipts" => &[
            "ck_run_batch_receipts_batch_sequence_positive",
            "ck_run_batch_receipts_generation_nonnegative",
            "ck_run_batch_receipts_digest_exact_bytes",
            "ck_run_batch_receipts_committed_flag",
        ],
        _ => unreachable!("unapproved table probed for checks: {table}"),
    }
}

const NULL_LITERAL: &str = "NULL";

struct TempDatabase {
    directory: PathBuf,
    database: PathBuf,
}

impl TempDatabase {
    fn new(label: &str) -> Result<Self, Box<dyn Error>> {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "artisan-editor-execution-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&directory)?;
        let database = directory.join("forge.sqlite3");
        Ok(Self {
            directory,
            database,
        })
    }

    fn database(&self) -> &Path {
        &self.database
    }
}

impl Drop for TempDatabase {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.directory);
    }
}

async fn connect_memory() -> Result<DatabaseConnection, Box<dyn Error>> {
    Ok(connect(SqliteConfig::in_memory().sqlx_logging(false)).await?)
}

async fn scalar_i64(database: &DatabaseConnection, sql: &str) -> Result<i64, Box<dyn Error>> {
    let row = database
        .query_one_raw(Statement::from_string(DbBackend::Sqlite, sql))
        .await?
        .ok_or_else(|| std::io::Error::other("scalar query returned no row"))?;
    Ok(row.try_get_by_index(0)?)
}

async fn text_rows(
    database: &DatabaseConnection,
    sql: &str,
) -> Result<Vec<String>, Box<dyn Error>> {
    let rows = database
        .query_all_raw(Statement::from_string(DbBackend::Sqlite, sql))
        .await?;
    let mut values = Vec::with_capacity(rows.len());
    for row in rows {
        values.push(row.try_get_by_index::<String>(0)?);
    }
    Ok(values)
}

async fn single_text(database: &DatabaseConnection, sql: &str) -> Result<String, Box<dyn Error>> {
    Ok(text_rows(database, sql)
        .await?
        .pop()
        .ok_or_else(|| std::io::Error::other("expected one row"))?)
}

async fn expect_rejected(label: &str, database: &DatabaseConnection, sql: &str) {
    let outcome = database.execute_unprepared(sql).await;
    assert!(outcome.is_err(), "{label} should have been rejected");
}

fn quoted(value: &str) -> String {
    format!("'{value}'")
}

fn quoted_list(names: &[&str]) -> String {
    names
        .iter()
        .map(|name| quoted(name))
        .collect::<Vec<_>>()
        .join(", ")
}

fn blob_literal(bytes: &[u8]) -> String {
    let hex = bytes.iter().fold(
        String::with_capacity(bytes.len() * 2),
        |mut output, byte| {
            write!(output, "{byte:02x}").expect("writing to a String cannot fail");
            output
        },
    );
    format!("x'{hex}'")
}

fn filled_blob(length: usize, byte: u8) -> String {
    blob_literal(&vec![byte; length])
}

fn repeated_text(unit: &str, copies: usize) -> String {
    quoted(unit.repeat(copies).as_str())
}

/// Derives a deterministic per-run start key so seeded runs never collide
/// on the globally unique `run_start_key`.
fn start_key_for(run_id: &str) -> String {
    let mut bytes = [0u8; 32];
    for (slot, byte) in bytes.iter_mut().zip(run_id.as_bytes().iter().cycle()) {
        *slot = *byte;
    }
    blob_literal(&bytes)
}

fn opt_str(value: Option<&str>) -> String {
    value.map_or_else(|| NULL_LITERAL.to_string(), quoted)
}

fn opt_int(value: Option<i64>) -> String {
    value.map_or_else(|| NULL_LITERAL.to_string(), |number| number.to_string())
}

fn insert_sql(table: &str, columns: &[&str], values: &[String]) -> String {
    assert_eq!(columns.len(), values.len());
    format!(
        "INSERT INTO {table} ({}) VALUES ({})",
        columns.join(", "),
        values.join(", ")
    )
}

async fn named_table_count(
    database: &DatabaseConnection,
    names: &[&str],
) -> Result<i64, Box<dyn Error>> {
    scalar_i64(
        database,
        &format!(
            "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name IN ({})",
            quoted_list(names)
        ),
    )
    .await
}

async fn migration_versions(database: &DatabaseConnection) -> Result<Vec<String>, Box<dyn Error>> {
    let rows = database
        .query_all_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT version FROM seaql_migrations ORDER BY applied_at ASC, version ASC",
        ))
        .await?;
    let mut versions = Vec::with_capacity(rows.len());
    for row in rows {
        versions.push(row.try_get_by_index::<String>(0)?);
    }
    Ok(versions)
}

async fn seed_catalog(database: &DatabaseConnection) -> Result<(), Box<dyn Error>> {
    database
        .execute_unprepared(
            "INSERT INTO attached_projects (project_id, root_path, display_name, attached_at_ms) \
             VALUES ('p1', 'C:/work/p1', 'Project', 1)",
        )
        .await?;
    for (thread, title, moment) in [("t1", "First thread", 2), ("t2", "Second thread", 3)] {
        database
            .execute_unprepared(&insert_sql(
                "threads",
                &[
                    "thread_id",
                    "project_id",
                    "title",
                    "created_at_ms",
                    "updated_at_ms",
                ],
                &[
                    quoted(thread),
                    quoted("p1"),
                    quoted(title),
                    moment.to_string(),
                    moment.to_string(),
                ],
            ))
            .await?;
    }
    for (message, thread, ordinal) in [("m1", "t1", 0), ("m2", "t1", 1), ("m3", "t2", 0)] {
        database
            .execute_unprepared(&insert_sql(
                "messages",
                &[
                    "message_id",
                    "thread_id",
                    "ordinal",
                    "body",
                    "accepted_at_ms",
                ],
                &[
                    quoted(message),
                    quoted(thread),
                    ordinal.to_string(),
                    quoted("payload"),
                    "4".to_string(),
                ],
            ))
            .await?;
    }
    Ok(())
}

async fn seed_ledger_slot(
    database: &DatabaseConnection,
    thread: &str,
    ordinal: i64,
    kind: &str,
    entity_id: &str,
) -> Result<(), Box<dyn Error>> {
    database
        .execute_unprepared(&insert_sql(
            "conversation_ordinals",
            &["thread_id", "ordinal", "kind", "entity_id"],
            &[
                quoted(thread),
                ordinal.to_string(),
                quoted(kind),
                quoted(entity_id),
            ],
        ))
        .await?;
    Ok(())
}

fn turn_insert_sql(turn_id: &str, thread: &str, ordinal: i64) -> String {
    insert_sql(
        "conversation_turns",
        &[
            "turn_id",
            "thread_id",
            "ordinal",
            "kind",
            "revision",
            "lifecycle",
            "created_at_ms",
            "updated_at_ms",
        ],
        &[
            quoted(turn_id),
            quoted(thread),
            ordinal.to_string(),
            quoted("turn"),
            "0".to_string(),
            quoted("pending"),
            "20".to_string(),
            "20".to_string(),
        ],
    )
}

async fn seed_turn(
    database: &DatabaseConnection,
    turn_id: &str,
    thread: &str,
    ordinal: i64,
) -> Result<(), Box<dyn Error>> {
    seed_ledger_slot(database, thread, ordinal, "turn", turn_id).await?;
    database
        .execute_unprepared(&turn_insert_sql(turn_id, thread, ordinal))
        .await?;
    Ok(())
}

/// Seeds both threads with one turn and one queued run each: thread `t1`
/// owns turn `turn_a` plus run `r1`, thread `t2` owns turn `turn_b` plus
/// run `r2`. Runs originate from messages `m1` and `m3` respectively.
async fn seed_execution_graph(database: &DatabaseConnection) -> Result<(), Box<dyn Error>> {
    seed_catalog(database).await?;
    seed_turn(database, "turn_a", "t1", 0).await?;
    seed_turn(database, "turn_b", "t2", 0).await?;
    queued_run("r1").insert_on(database).await?;
    queued_run("r2")
        .with_thread("t2")
        .with_origin("m3", "turn_b")
        .insert_on(database)
        .await?;
    Ok(())
}

/// Adds an unused origin message plus turn on thread `t1` so each extra run
/// can satisfy the global origin uniqueness guarantees.
async fn seed_run_origin(
    database: &DatabaseConnection,
    suffix: u32,
) -> Result<(String, String), Box<dyn Error>> {
    let message = format!("om{suffix}");
    let turn = format!("ot{suffix}");
    database
        .execute_unprepared(&insert_sql(
            "messages",
            &[
                "message_id",
                "thread_id",
                "ordinal",
                "body",
                "accepted_at_ms",
            ],
            &[
                quoted(&message),
                quoted("t1"),
                (100 + i64::from(suffix)).to_string(),
                quoted("origin"),
                "9".to_string(),
            ],
        ))
        .await?;
    seed_turn(database, &turn, "t1", 500 + i64::from(suffix)).await?;
    Ok((message, turn))
}

async fn accept_run(
    database: &DatabaseConnection,
    suffix: u32,
    spec: RunSpec,
) -> Result<(), Box<dyn Error>> {
    let (message, turn) = seed_run_origin(database, suffix).await?;
    spec.with_origin(&message, &turn)
        .insert_on(database)
        .await?;
    Ok(())
}

async fn reject_run(
    database: &DatabaseConnection,
    label: &str,
    suffix: u32,
    spec: RunSpec,
) -> Result<(), Box<dyn Error>> {
    let (message, turn) = seed_run_origin(database, suffix).await?;
    expect_rejected(label, database, &spec.with_origin(&message, &turn).sql()).await;
    Ok(())
}

const RUN_COLUMNS: [&str; 18] = [
    "run_id",
    "thread_id",
    "run_start_key",
    "origin_message_id",
    "origin_turn_id",
    "lifecycle",
    "generation",
    "owner",
    "lease",
    "claim_token",
    "provider_binding_version",
    "provider_binding",
    "provider_bound_at_ms",
    "error_code",
    "error_message",
    "created_at_ms",
    "updated_at_ms",
    "terminal_at_ms",
];

struct RunSpec {
    run_id: String,
    thread_id: String,
    start_key: String,
    origin_message: String,
    origin_turn: String,
    lifecycle: String,
    generation: String,
    owner: String,
    lease: String,
    token: String,
    binding_version: String,
    binding: String,
    bound_at: String,
    error_code: String,
    error_message: String,
    terminal_at: String,
    created_at: i64,
    updated_at: i64,
}

fn queued_run(run_id: &str) -> RunSpec {
    RunSpec {
        run_id: run_id.to_string(),
        thread_id: "t1".to_string(),
        start_key: start_key_for(run_id),
        origin_message: "m1".to_string(),
        origin_turn: "turn_a".to_string(),
        lifecycle: "queued".to_string(),
        generation: "0".to_string(),
        owner: NULL_LITERAL.to_string(),
        lease: NULL_LITERAL.to_string(),
        token: NULL_LITERAL.to_string(),
        binding_version: NULL_LITERAL.to_string(),
        binding: NULL_LITERAL.to_string(),
        bound_at: NULL_LITERAL.to_string(),
        error_code: NULL_LITERAL.to_string(),
        error_message: NULL_LITERAL.to_string(),
        terminal_at: NULL_LITERAL.to_string(),
        created_at: 10,
        updated_at: 10,
    }
}

impl RunSpec {
    fn with(mut self, mutate: impl FnOnce(&mut Self)) -> Self {
        mutate(&mut self);
        self
    }

    fn with_thread(mut self, thread: &str) -> Self {
        self.thread_id = thread.to_string();
        self
    }

    fn with_origin(mut self, message: &str, turn: &str) -> Self {
        self.origin_message = message.to_string();
        self.origin_turn = turn.to_string();
        self
    }

    fn into_launching(mut self) -> Self {
        self.lifecycle = "launching".to_string();
        self.generation = "1".to_string();
        self.owner = filled_blob(32, 0x11);
        self.lease = filled_blob(32, 0x22);
        self.token = filled_blob(32, 0x33);
        self.updated_at = 11;
        self
    }

    fn into_running(mut self) -> Self {
        self = self.into_launching();
        self.lifecycle = "running".to_string();
        self.token = NULL_LITERAL.to_string();
        self.binding_version = "1".to_string();
        self.binding = filled_blob(16, 0xee);
        self.bound_at = "300".to_string();
        self.updated_at = 12;
        self
    }

    fn cleared_for_exit(mut self) -> Self {
        self.owner = NULL_LITERAL.to_string();
        self.lease = NULL_LITERAL.to_string();
        self.token = NULL_LITERAL.to_string();
        self
    }

    fn sql(&self) -> String {
        insert_sql(
            "assistant_runs",
            &RUN_COLUMNS,
            &[
                quoted(&self.run_id),
                quoted(&self.thread_id),
                self.start_key.clone(),
                quoted(&self.origin_message),
                quoted(&self.origin_turn),
                quoted(&self.lifecycle),
                self.generation.clone(),
                self.owner.clone(),
                self.lease.clone(),
                self.token.clone(),
                self.binding_version.clone(),
                self.binding.clone(),
                self.bound_at.clone(),
                self.error_code.clone(),
                self.error_message.clone(),
                self.created_at.to_string(),
                self.updated_at.to_string(),
                self.terminal_at.clone(),
            ],
        )
    }

    async fn insert_on(&self, database: &DatabaseConnection) -> Result<(), Box<dyn Error>> {
        database.execute_unprepared(&self.sql()).await?;
        Ok(())
    }
}

async fn reject_invalid_queued_and_launching_runs(
    database: &DatabaseConnection,
) -> Result<(), Box<dyn Error>> {
    let cases = [
        (
            "queued row holding an owner",
            1,
            queued_run("bad").with(|row| row.owner = filled_blob(32, 0x11)),
        ),
        (
            "queued row above generation zero",
            2,
            queued_run("bad").with(|row| row.generation = "1".to_string()),
        ),
        (
            "queued row holding a binding",
            3,
            queued_run("bad").with(|row| {
                row.binding_version = "1".to_string();
                row.binding = filled_blob(4, 0xee);
                row.bound_at = "300".to_string();
            }),
        ),
        (
            "launching at generation zero",
            4,
            queued_run("bad")
                .into_launching()
                .with(|row| row.generation = "0".to_string()),
        ),
        (
            "launching without its claim token",
            5,
            queued_run("bad")
                .into_launching()
                .with(|row| row.token = NULL_LITERAL.to_string()),
        ),
        (
            "launching without its lease",
            6,
            queued_run("bad")
                .into_launching()
                .with(|row| row.lease = NULL_LITERAL.to_string()),
        ),
        (
            "launching while already bound",
            7,
            queued_run("bad").into_launching().with(|row| {
                row.binding_version = "1".to_string();
                row.binding = filled_blob(4, 0xee);
                row.bound_at = "300".to_string();
            }),
        ),
    ];

    for (label, ordinal, run) in cases {
        reject_run(database, label, ordinal, run).await?;
    }
    Ok(())
}

async fn reject_invalid_running_and_error_runs(
    database: &DatabaseConnection,
) -> Result<(), Box<dyn Error>> {
    let cases = [
        (
            "running without its binding",
            8,
            queued_run("bad").into_running().with(|row| {
                row.binding_version = NULL_LITERAL.to_string();
                row.binding = NULL_LITERAL.to_string();
                row.bound_at = NULL_LITERAL.to_string();
            }),
        ),
        (
            "running while holding the claim token",
            9,
            queued_run("bad")
                .into_running()
                .with(|row| row.token = filled_blob(32, 0x33)),
        ),
        (
            "fence key shorter than 32 bytes",
            10,
            queued_run("bad")
                .into_launching()
                .with(|row| row.owner = filled_blob(31, 0x11)),
        ),
        (
            "interrupted without its error pair",
            11,
            queued_run("bad")
                .into_running()
                .cleared_for_exit()
                .with(|row| {
                    row.lifecycle = "interrupted".to_string();
                    row.updated_at = 14;
                }),
        ),
        (
            "failed with half an error pair",
            12,
            queued_run("bad")
                .into_running()
                .cleared_for_exit()
                .with(|row| {
                    row.lifecycle = "failed".to_string();
                    row.error_code = quoted("provider_5xx");
                    row.terminal_at = "15".to_string();
                    row.updated_at = 15;
                }),
        ),
        (
            "error code beyond 128 bytes",
            13,
            queued_run("bad")
                .into_running()
                .cleared_for_exit()
                .with(|row| {
                    row.lifecycle = "interrupted".to_string();
                    row.error_code = repeated_text("e", 129);
                    row.error_message = quoted("late failure");
                    row.updated_at = 14;
                }),
        ),
        (
            "error message beyond 1024 bytes",
            14,
            queued_run("bad")
                .into_running()
                .cleared_for_exit()
                .with(|row| {
                    row.lifecycle = "failed".to_string();
                    row.error_code = quoted("provider_5xx");
                    row.error_message = repeated_text("\u{e9}", 513);
                    row.terminal_at = "15".to_string();
                    row.updated_at = 15;
                }),
        ),
    ];

    for (label, ordinal, run) in cases {
        reject_run(database, label, ordinal, run).await?;
    }
    Ok(())
}

async fn reject_invalid_terminal_and_binding_runs(
    database: &DatabaseConnection,
) -> Result<(), Box<dyn Error>> {
    let cases = [
        (
            "cancelled carrying an error pair",
            15,
            queued_run("bad").with(|row| {
                row.lifecycle = "cancelled".to_string();
                row.error_code = quoted("late");
                row.error_message = quoted("cancel after completion");
                row.terminal_at = "15".to_string();
                row.updated_at = 15;
            }),
        ),
        (
            "completed without its terminal timestamp",
            16,
            queued_run("bad")
                .into_running()
                .cleared_for_exit()
                .with(|row| {
                    row.lifecycle = "completed".to_string();
                    row.updated_at = 16;
                }),
        ),
        (
            "non-terminal state carrying a terminal timestamp",
            17,
            queued_run("bad")
                .into_running()
                .with(|row| row.terminal_at = "12".to_string()),
        ),
        (
            "terminal timestamp before creation",
            18,
            queued_run("bad")
                .into_running()
                .cleared_for_exit()
                .with(|row| {
                    row.lifecycle = "failed".to_string();
                    row.error_code = quoted("provider_5xx");
                    row.error_message = quoted("boom");
                    row.terminal_at = "9".to_string();
                    row.updated_at = 15;
                }),
        ),
        (
            "terminal timestamp after the last update",
            19,
            queued_run("bad")
                .into_running()
                .cleared_for_exit()
                .with(|row| {
                    row.lifecycle = "cancelled".to_string();
                    row.terminal_at = "20".to_string();
                }),
        ),
        (
            "binding blob larger than 262144 bytes",
            20,
            queued_run("bad")
                .into_running()
                .with(|row| row.binding = filled_blob(262_145, 0xee)),
        ),
        (
            "binding version zero",
            21,
            queued_run("bad")
                .into_running()
                .with(|row| row.binding_version = "0".to_string()),
        ),
    ];

    for (label, ordinal, run) in cases {
        reject_run(database, label, ordinal, run).await?;
    }
    Ok(())
}

const ITEM_COLUMNS: [&str; 15] = [
    "item_id",
    "thread_id",
    "turn_id",
    "ordinal",
    "kind",
    "revision",
    "lifecycle",
    "item_kind",
    "source_message_id",
    "run_id",
    "native_item_key",
    "phase",
    "body",
    "created_at_ms",
    "updated_at_ms",
];

struct ItemSpec {
    item_id: String,
    thread: String,
    turn: String,
    ordinal: i64,
    item_kind: String,
    lifecycle: String,
    source: Option<String>,
    run: Option<String>,
    native_key: Option<String>,
    phase: Option<String>,
    body: String,
    revision: i64,
    created_at: i64,
    updated_at: i64,
}

fn user_item(item_id: &str) -> ItemSpec {
    ItemSpec {
        item_id: item_id.to_string(),
        thread: "t1".to_string(),
        turn: "turn_a".to_string(),
        ordinal: 1,
        item_kind: "user_message".to_string(),
        lifecycle: "completed".to_string(),
        source: Some("m1".to_string()),
        run: None,
        native_key: None,
        phase: None,
        body: "hello back".to_string(),
        revision: 0,
        created_at: 40,
        updated_at: 45,
    }
}

fn assistant_item(item_id: &str) -> ItemSpec {
    ItemSpec {
        item_id: item_id.to_string(),
        thread: "t1".to_string(),
        turn: "turn_a".to_string(),
        ordinal: 2,
        item_kind: "assistant_message".to_string(),
        lifecycle: "streaming".to_string(),
        source: None,
        run: Some("r1".to_string()),
        native_key: None,
        phase: Some("final".to_string()),
        body: "reply".to_string(),
        revision: 0,
        created_at: 46,
        updated_at: 48,
    }
}

impl ItemSpec {
    fn with(mut self, mutate: impl FnOnce(&mut Self)) -> Self {
        mutate(&mut self);
        self
    }

    fn sql(&self) -> String {
        insert_sql(
            "conversation_items",
            &ITEM_COLUMNS,
            &[
                quoted(&self.item_id),
                quoted(&self.thread),
                quoted(&self.turn),
                self.ordinal.to_string(),
                quoted("item"),
                self.revision.to_string(),
                quoted(&self.lifecycle),
                quoted(&self.item_kind),
                opt_str(self.source.as_deref()),
                opt_str(self.run.as_deref()),
                opt_str(self.native_key.as_deref()),
                opt_str(self.phase.as_deref()),
                quoted(&self.body),
                self.created_at.to_string(),
                self.updated_at.to_string(),
            ],
        )
    }

    async fn insert_on(&self, database: &DatabaseConnection) -> Result<(), Box<dyn Error>> {
        database.execute_unprepared(&self.sql()).await?;
        Ok(())
    }
}

/// Reserves the exact `(thread, ordinal, 'item', item_id)` ledger slot an
/// item must reference, then inserts the valid item against that slot.
async fn insert_valid_item(
    database: &DatabaseConnection,
    spec: &ItemSpec,
) -> Result<(), Box<dyn Error>> {
    seed_ledger_slot(database, &spec.thread, spec.ordinal, "item", &spec.item_id).await?;
    spec.insert_on(database).await
}

/// Assigns a caller-supplied unique ordinal, reserves that exact
/// `(thread, ordinal, 'item', item_id)` ledger row so rejection comes from
/// the intended CHECK or UNIQUE rule rather than a missing slot, then
/// asserts the invalid item insert is rejected.
async fn reject_invalid_item(
    database: &DatabaseConnection,
    label: &str,
    ordinal: i64,
    spec: ItemSpec,
) -> Result<(), Box<dyn Error>> {
    let spec = spec.with(|item| item.ordinal = ordinal);
    seed_ledger_slot(database, &spec.thread, ordinal, "item", &spec.item_id).await?;
    expect_rejected(label, database, &spec.sql()).await;
    Ok(())
}

async fn reject_invalid_item_shapes_and_duplicates(
    database: &DatabaseConnection,
) -> Result<(), Box<dyn Error>> {
    let cases = [
        (
            "user item without its source message",
            11,
            user_item("iu_no_source").with(|item| item.source = None),
        ),
        (
            "user item carrying a run",
            12,
            user_item("iu_run").with(|item| item.run = Some("r1".to_string())),
        ),
        (
            "user item carrying a phase",
            13,
            user_item("iu_phase").with(|item| item.phase = Some("final".to_string())),
        ),
        (
            "user item carrying a native key",
            14,
            user_item("iu_native").with(|item| item.native_key = Some("k".to_string())),
        ),
        (
            "assistant item without its run",
            15,
            assistant_item("ia_no_run").with(|item| item.run = None),
        ),
        (
            "assistant item without its phase",
            16,
            assistant_item("ia_no_phase").with(|item| item.phase = None),
        ),
        (
            "assistant item sourcing a user message",
            17,
            assistant_item("ia_source").with(|item| item.source = Some("m1".to_string())),
        ),
        (
            "assistant item with an empty native key",
            18,
            assistant_item("ia_empty_native").with(|item| item.native_key = Some(String::new())),
        ),
        (
            "unknown phase",
            19,
            assistant_item("ia_phase_bad")
                .with(|item| item.phase = Some("speculative".to_string())),
        ),
        ("duplicate source message", 20, user_item("iu_dup_source")),
        (
            "duplicate run-native identity",
            21,
            assistant_item("ia_dup_native")
                .with(|item| item.native_key = Some("block".to_string())),
        ),
        (
            "update time before creation time",
            22,
            user_item("iu_times").with(|item| {
                item.created_at = 50;
                item.updated_at = 49;
            }),
        ),
    ];

    for (label, ordinal, item) in cases {
        reject_invalid_item(database, label, ordinal, item).await?;
    }
    Ok(())
}

const PATCH_COLUMNS: [&str; 17] = [
    "patch_id",
    "thread_id",
    "sequence",
    "kind",
    "revision",
    "recorded_at_ms",
    "turn_id",
    "item_id",
    "ordinal",
    "lifecycle",
    "item_kind",
    "run_id",
    "phase",
    "body",
    "fragment",
    "entity_created_at_ms",
    "entity_updated_at_ms",
];

struct PatchSpec {
    patch_id: String,
    thread: String,
    sequence: i64,
    kind: String,
    revision: Option<i64>,
    recorded_at: Option<i64>,
    turn: Option<String>,
    item: Option<String>,
    ordinal: Option<i64>,
    lifecycle: Option<String>,
    item_kind: Option<String>,
    run: Option<String>,
    phase: Option<String>,
    body: Option<String>,
    fragment: Option<String>,
    entity_created_at: Option<i64>,
    entity_updated_at: Option<i64>,
}

fn turn_upsert_patch(patch_id: &str, sequence: i64) -> PatchSpec {
    PatchSpec {
        patch_id: patch_id.to_string(),
        thread: "t1".to_string(),
        sequence,
        kind: "turn_upsert".to_string(),
        revision: Some(0),
        recorded_at: Some(90),
        turn: Some("turn_a".to_string()),
        item: None,
        ordinal: Some(0),
        lifecycle: Some("active".to_string()),
        item_kind: None,
        run: None,
        phase: None,
        body: None,
        fragment: None,
        entity_created_at: Some(20),
        entity_updated_at: Some(25),
    }
}

fn item_upsert_patch(patch_id: &str, sequence: i64, item_kind: &str) -> PatchSpec {
    let user = item_kind == "user_message";
    PatchSpec {
        patch_id: patch_id.to_string(),
        thread: "t1".to_string(),
        sequence,
        kind: "item_upsert".to_string(),
        revision: Some(1),
        recorded_at: Some(91),
        turn: Some("turn_a".to_string()),
        item: Some(if user { "iu_patched" } else { "ia_patched" }.to_string()),
        ordinal: Some(10),
        lifecycle: Some("completed".to_string()),
        item_kind: Some(item_kind.to_string()),
        run: (!user).then(|| "r1".to_string()),
        phase: (!user).then(|| "final".to_string()),
        body: Some("patched body".to_string()),
        fragment: None,
        entity_created_at: Some(40),
        entity_updated_at: Some(44),
    }
}

fn item_append_patch(patch_id: &str, sequence: i64, fragment: &str) -> PatchSpec {
    PatchSpec {
        patch_id: patch_id.to_string(),
        thread: "t1".to_string(),
        sequence,
        kind: "item_append".to_string(),
        revision: Some(2),
        recorded_at: Some(92),
        turn: None,
        item: Some("ia_patched".to_string()),
        ordinal: None,
        lifecycle: None,
        item_kind: None,
        run: None,
        phase: None,
        body: None,
        fragment: Some(fragment.to_string()),
        entity_created_at: None,
        entity_updated_at: None,
    }
}

fn lifecycle_patch(patch_id: &str, sequence: i64, kind: &str) -> PatchSpec {
    let targets_turn = kind == "turn_lifecycle";
    PatchSpec {
        patch_id: patch_id.to_string(),
        thread: "t1".to_string(),
        sequence,
        kind: kind.to_string(),
        revision: Some(3),
        recorded_at: Some(93),
        turn: targets_turn.then(|| "turn_a".to_string()),
        item: (!targets_turn).then(|| "ia_patched".to_string()),
        ordinal: None,
        lifecycle: Some("completed".to_string()),
        item_kind: None,
        run: None,
        phase: None,
        body: None,
        fragment: None,
        entity_created_at: None,
        entity_updated_at: None,
    }
}

impl PatchSpec {
    fn with(mut self, mutate: impl FnOnce(&mut Self)) -> Self {
        mutate(&mut self);
        self
    }

    fn sql(&self) -> String {
        insert_sql(
            "conversation_patches",
            &PATCH_COLUMNS,
            &[
                quoted(&self.patch_id),
                quoted(&self.thread),
                self.sequence.to_string(),
                quoted(&self.kind),
                opt_int(self.revision),
                opt_int(self.recorded_at),
                opt_str(self.turn.as_deref()),
                opt_str(self.item.as_deref()),
                opt_int(self.ordinal),
                opt_str(self.lifecycle.as_deref()),
                opt_str(self.item_kind.as_deref()),
                opt_str(self.run.as_deref()),
                opt_str(self.phase.as_deref()),
                opt_str(self.body.as_deref()),
                opt_str(self.fragment.as_deref()),
                opt_int(self.entity_created_at),
                opt_int(self.entity_updated_at),
            ],
        )
    }

    async fn insert_on(&self, database: &DatabaseConnection) -> Result<(), Box<dyn Error>> {
        database.execute_unprepared(&self.sql()).await?;
        Ok(())
    }
}

#[tokio::test]
async fn fresh_migrate_is_idempotent_and_registers_the_third_version() -> Result<(), Box<dyn Error>>
{
    let temp = TempDatabase::new("fresh")?;
    let all_tables = BASE_TABLES
        .iter()
        .copied()
        .chain(EXECUTION_TABLES)
        .collect::<Vec<_>>();
    let database = connect(SqliteConfig::file(temp.database()).sqlx_logging(false)).await?;

    migrate_to_current(&database).await?;
    migrate_to_current(&database).await?;
    assert_eq!(named_table_count(&database, &all_tables).await?, 13);
    assert_eq!(
        scalar_i64(
            &database,
            &format!(
                "SELECT count(*) FROM seaql_migrations WHERE version = '{EXECUTION_MIGRATION}'"
            ),
        )
        .await?,
        1
    );

    seed_catalog(&database).await?;
    database
        .execute_unprepared(
            "INSERT INTO conversation_state (thread_id, updated_at_ms) VALUES ('t1', 7)",
        )
        .await?;
    assert_eq!(
        scalar_i64(
            &database,
            "SELECT next_renderer_ordinal FROM conversation_state WHERE thread_id = 't1'",
        )
        .await?,
        0
    );
    assert_eq!(
        scalar_i64(
            &database,
            "SELECT last_patch_sequence FROM conversation_state WHERE thread_id = 't1'",
        )
        .await?,
        0
    );

    database.close().await?;
    let reopened = connect(SqliteConfig::file(temp.database()).sqlx_logging(false)).await?;
    migrate_to_current(&reopened).await?;
    assert_eq!(
        migration_versions(&reopened).await?,
        [
            INITIAL_MIGRATION.to_string(),
            RECEIPTS_MIGRATION.to_string(),
            EXECUTION_MIGRATION.to_string(),
        ]
    );
    reopened.close().await?;
    Ok(())
}

#[tokio::test]
async fn execution_upgrade_preserves_receipts_era_data() -> Result<(), Box<dyn Error>> {
    let database = connect_memory().await?;
    Migrator::up(&database, Some(2)).await?;
    seed_catalog(&database).await?;
    database
        .execute_unprepared(
            "INSERT INTO message_dispatches (message_id, correlation_id, state, attempt_count, \
             queued_at_ms, available_at_ms, updated_at_ms) \
             VALUES ('m1', 'c1', 'queued', 0, 4, 4, 4)",
        )
        .await?;

    migrate_to_current(&database).await?;

    assert_eq!(migration_versions(&database).await?.len(), 3);
    let dispatch = database
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT correlation_id, state FROM message_dispatches WHERE message_id = 'm1'",
        ))
        .await?
        .ok_or_else(|| std::io::Error::other("dispatch row lost during upgrade"))?;
    assert_eq!(dispatch.try_get_by_index::<String>(0)?, "c1");
    assert_eq!(dispatch.try_get_by_index::<String>(1)?, "queued");
    let body = single_text(
        &database,
        "SELECT body FROM messages WHERE message_id = 'm1'",
    )
    .await?;
    assert_eq!(body, "payload");
    for table in EXECUTION_TABLES {
        assert_eq!(
            scalar_i64(&database, &format!("SELECT count(*) FROM {table}")).await?,
            0,
            "{table} should start empty"
        );
    }
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn down_reverses_only_execution_objects_then_reapply_restores_them()
-> Result<(), Box<dyn Error>> {
    let all_tables = BASE_TABLES
        .iter()
        .copied()
        .chain(EXECUTION_TABLES)
        .collect::<Vec<_>>();
    let execution_index = "SELECT count(*) FROM sqlite_schema WHERE type = 'index' \
                           AND name = 'uq_messages_message_id_thread_id'";
    let database = connect_memory().await?;
    Migrator::up(&database, Some(3)).await?;
    seed_catalog(&database).await?;

    Migrator::down(&database, Some(1)).await?;

    assert_eq!(migration_versions(&database).await?.len(), 2);
    assert_eq!(named_table_count(&database, &BASE_TABLES).await?, 5);
    assert_eq!(named_table_count(&database, &EXECUTION_TABLES).await?, 0);
    assert_eq!(scalar_i64(&database, execution_index).await?, 0);

    Migrator::up(&database, None).await?;

    assert_eq!(migration_versions(&database).await?.len(), 3);
    assert_eq!(named_table_count(&database, &all_tables).await?, 13);
    assert_eq!(scalar_i64(&database, execution_index).await?, 1);
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn schema_inventory_matches_the_approved_contract() -> Result<(), Box<dyn Error>> {
    let database = connect_memory().await?;
    migrate_to_current(&database).await?;

    let mut tables = text_rows(
        &database,
        "SELECT name FROM sqlite_schema WHERE type = 'table' \
         AND name NOT LIKE 'sqlite_%' AND name != 'seaql_migrations'",
    )
    .await?;
    tables.sort();
    let mut expected_tables = BASE_TABLES
        .iter()
        .copied()
        .chain(EXECUTION_TABLES)
        .map(str::to_string)
        .collect::<Vec<_>>();
    expected_tables.sort();
    assert_eq!(tables, expected_tables);

    let mut indexes = text_rows(
        &database,
        &format!(
            "SELECT name FROM sqlite_schema WHERE type = 'index' AND sql IS NOT NULL \
             AND tbl_name IN ({})",
            quoted_list(
                &EXECUTION_TABLES
                    .iter()
                    .copied()
                    .chain(["messages"])
                    .collect::<Vec<_>>()
            )
        ),
    )
    .await?;
    indexes.sort();
    let mut expected_indexes = EXECUTION_INDEXES
        .iter()
        .map(ToString::to_string)
        // The receipts-era schema already owns these `messages` indexes;
        // they fall inside this inventory but stay out of the execution-
        // owned EXECUTION_INDEXES contract.
        .chain([
            "idx_messages_thread_id".to_string(),
            "uq_messages_thread_id_ordinal".to_string(),
        ])
        .collect::<Vec<_>>();
    expected_indexes.sort();
    assert_eq!(indexes, expected_indexes);

    for table in EXECUTION_TABLES {
        let ddl = single_text(
            &database,
            &format!("SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = '{table}'"),
        )
        .await?;
        let approved = approved_checks(table);
        for check in approved {
            assert!(ddl.contains(check), "{table} misses check {check}");
        }
        assert_eq!(
            ddl.match_indices("ck_").count(),
            approved.len(),
            "{table} declares unexpected checks"
        );
    }
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn foreign_keys_bind_every_child_to_same_thread_parents() -> Result<(), Box<dyn Error>> {
    let database = connect_memory().await?;
    migrate_to_current(&database).await?;

    // The four-column ledger bindings are stated separately below so the
    // drift assertion stays exhaustive without burying the composite keys.
    let mut expected = Vec::from(APPROVED_FOREIGN_KEYS);
    expected[2].1 = &[
        "thread_id->conversation_ordinals.thread_id|ordinal->conversation_ordinals.ordinal|\
         turn_id->conversation_ordinals.entity_id|kind->conversation_ordinals.kind",
    ];
    expected[4].1 = &[
        "thread_id->conversation_ordinals.thread_id|ordinal->conversation_ordinals.ordinal|\
         item_id->conversation_ordinals.entity_id|kind->conversation_ordinals.kind",
        "source_message_id->messages.message_id|thread_id->messages.thread_id",
        "turn_id->conversation_turns.turn_id|thread_id->conversation_turns.thread_id",
        "run_id->assistant_runs.run_id|thread_id->assistant_runs.thread_id",
    ];

    for (table, approved) in expected {
        let rows = database
            .query_all_raw(Statement::from_string(
                DbBackend::Sqlite,
                format!("PRAGMA foreign_key_list({table})"),
            ))
            .await?;
        let mut composites: BTreeMap<i64, BTreeMap<i64, String>> = BTreeMap::new();
        for row in rows {
            let id: i64 = row.try_get_by_index(0)?;
            let seq: i64 = row.try_get_by_index(1)?;
            let parent: String = row.try_get_by_index(2)?;
            let child_column: String = row.try_get_by_index(3)?;
            let parent_column: String = row.try_get_by_index(4)?;
            composites
                .entry(id)
                .or_default()
                .insert(seq, format!("{child_column}->{parent}.{parent_column}"));
        }
        let mut actual = composites
            .into_values()
            .map(|columns| columns.into_values().collect::<Vec<_>>().join("|"))
            .collect::<Vec<_>>();
        actual.sort();
        let mut expected_sorted = approved.iter().map(ToString::to_string).collect::<Vec<_>>();
        expected_sorted.sort();
        assert_eq!(actual, expected_sorted, "{table} foreign keys drifted");
    }
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn ordinal_ledger_shares_slots_and_rejects_collisions_and_aliasing()
-> Result<(), Box<dyn Error>> {
    let database = connect_memory().await?;
    migrate_to_current(&database).await?;
    seed_catalog(&database).await?;

    seed_ledger_slot(&database, "t1", 0, "turn", "turn_a").await?;
    seed_ledger_slot(&database, "t1", 1, "item", "item_b").await?;

    let collisions = [
        (
            "cross-kind collision against a turn slot",
            "('t1', 0, 'item', 'item_x')",
        ),
        (
            "cross-kind collision against an item slot",
            "('t1', 1, 'turn', 'turn_y')",
        ),
        (
            "entity id aliased into another slot",
            "('t1', 2, 'turn', 'turn_a')",
        ),
        (
            "entity id aliased across threads",
            "('t2', 0, 'turn', 'turn_a')",
        ),
        ("identical ledger row replay", "('t1', 0, 'turn', 'turn_a')"),
        ("negative ordinal", "('t1', -1, 'item', 'item_neg')"),
        ("unknown slot kind", "('t1', 3, 'vertex', 'vertex_1')"),
        (
            "ledger row for a missing thread",
            "('ghost', 0, 'turn', 'ghost_turn')",
        ),
    ];
    for (label, values) in collisions {
        expect_rejected(
            label,
            &database,
            &format!(
                "INSERT INTO conversation_ordinals \
                 (thread_id, ordinal, kind, entity_id) VALUES {values}"
            ),
        )
        .await;
    }

    assert_eq!(
        scalar_i64(&database, "SELECT count(*) FROM conversation_ordinals").await?,
        2
    );
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn turns_and_items_require_same_thread_parents() -> Result<(), Box<dyn Error>> {
    let database = connect_memory().await?;
    migrate_to_current(&database).await?;
    seed_execution_graph(&database).await?;
    seed_ledger_slot(&database, "t1", 10, "item", "slot_user").await?;
    seed_ledger_slot(&database, "t1", 11, "item", "slot_assistant").await?;
    seed_ledger_slot(&database, "t1", 12, "item", "iu_mismatch").await?;
    seed_ledger_slot(&database, "t1", 13, "item", "ia_bad_run").await?;

    expect_rejected(
        "turn claiming another thread's ordinal slot",
        &database,
        &turn_insert_sql("turn_z", "t2", 0),
    )
    .await;

    let mismatched_source = user_item("iu_mismatch")
        .with(|item| item.ordinal = 12)
        .with(|item| item.source = Some("m3".to_string()));
    expect_rejected(
        "user item sourcing another thread's message",
        &database,
        &mismatched_source.sql(),
    )
    .await;

    let mismatched_run = assistant_item("ia_bad_run")
        .with(|item| item.ordinal = 13)
        .with(|item| item.run = Some("r2".to_string()));
    expect_rejected(
        "item bound to another thread's run",
        &database,
        &mismatched_run.sql(),
    )
    .await;

    // Honest rows land on their own reserved slots.
    user_item("slot_user")
        .with(|item| item.ordinal = 10)
        .insert_on(&database)
        .await?;
    assistant_item("slot_assistant")
        .with(|item| item.ordinal = 11)
        .insert_on(&database)
        .await?;

    // Neither entity may borrow a slot reserved for the other kind.
    expect_rejected(
        "item borrowing a turn-reserved slot",
        &database,
        &user_item("iu_thief")
            .with(|item| {
                item.item_id = "item_thief".to_string();
                item.ordinal = 0;
            })
            .sql(),
    )
    .await;
    expect_rejected(
        "turn borrowing an item-reserved slot",
        &database,
        &turn_insert_sql("turn_thief", "t1", 10),
    )
    .await;

    assert_eq!(
        scalar_i64(&database, "SELECT count(*) FROM conversation_turns").await?,
        2
    );
    assert_eq!(
        scalar_i64(&database, "SELECT count(*) FROM conversation_items").await?,
        2
    );
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn items_enforce_subshapes_and_identity_uniqueness() -> Result<(), Box<dyn Error>> {
    let database = connect_memory().await?;
    migrate_to_current(&database).await?;
    seed_execution_graph(&database).await?;

    insert_valid_item(&database, &user_item("iu_base")).await?;
    // The native key makes the later duplicate run/native rejection a
    // genuine `uq_conversation_items_run_id_native_item_key` collision.
    insert_valid_item(
        &database,
        &assistant_item("ia_base").with(|item| item.native_key = Some("block".to_string())),
    )
    .await?;

    reject_invalid_item_shapes_and_duplicates(&database).await?;

    assert_eq!(
        scalar_i64(&database, "SELECT count(*) FROM conversation_items").await?,
        2
    );
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn multibyte_payloads_hold_exact_byte_bounds() -> Result<(), Box<dyn Error>> {
    let database = connect_memory().await?;
    migrate_to_current(&database).await?;
    seed_execution_graph(&database).await?;

    // U+00E9 encodes as two UTF-8 bytes, so 32_768 copies fill exactly
    // 65_536 bytes and one more copy crosses the bound.
    insert_valid_item(
        &database,
        &user_item("iu_max_body").with(|item| item.body = "\u{e9}".repeat(32_768)),
    )
    .await?;
    reject_invalid_item(
        &database,
        "item body past 65536 utf-8 bytes",
        4,
        user_item("iu_over_body").with(|item| item.body = "\u{e9}".repeat(32_769)),
    )
    .await?;

    insert_valid_item(
        &database,
        &assistant_item("ia_stream").with(|item| item.lifecycle = "streaming".to_string()),
    )
    .await?;
    // The append patches below default to item `ia_patched`, so that exact
    // assistant item must exist for the accepted fragment to land.
    insert_valid_item(
        &database,
        &assistant_item("ia_patched").with(|item| item.ordinal = 3),
    )
    .await?;
    item_append_patch("pa_max_fragment", 1, &"\u{e9}".repeat(2_048))
        .insert_on(&database)
        .await?;
    expect_rejected(
        "patch fragment past 4096 utf-8 bytes",
        &database,
        &item_append_patch("pa_over_fragment", 2, &"\u{e9}".repeat(2_049)).sql(),
    )
    .await;

    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn assistant_runs_follow_the_launch_fence() -> Result<(), Box<dyn Error>> {
    let database = connect_memory().await?;
    migrate_to_current(&database).await?;
    seed_execution_graph(&database).await?;

    accept_run(&database, 1, queued_run("x1").into_launching()).await?;
    accept_run(&database, 2, queued_run("x2").into_running()).await?;
    accept_run(
        &database,
        3,
        queued_run("x3").into_running().with(|row| {
            row.lifecycle = "waiting".to_string();
            row.updated_at = 13;
        }),
    )
    .await?;
    accept_run(
        &database,
        4,
        queued_run("x4").into_running().with(|row| {
            row.lifecycle = "cancel_requested".to_string();
            row.updated_at = 13;
        }),
    )
    .await?;
    accept_run(
        &database,
        5,
        queued_run("x5")
            .into_running()
            .cleared_for_exit()
            .with(|row| {
                row.lifecycle = "interrupted".to_string();
                row.error_code = quoted("timeout");
                row.error_message = quoted("engine stopped responding");
                row.updated_at = 14;
            }),
    )
    .await?;
    accept_run(
        &database,
        6,
        queued_run("x6")
            .into_running()
            .cleared_for_exit()
            .with(|row| {
                row.lifecycle = "failed".to_string();
                row.error_code = quoted("provider_5xx");
                row.error_message = quoted("upstream rejected");
                row.terminal_at = "15".to_string();
                row.updated_at = 15;
            }),
    )
    .await?;
    accept_run(
        &database,
        7,
        queued_run("x7")
            .into_running()
            .cleared_for_exit()
            .with(|row| {
                row.lifecycle = "completed".to_string();
                row.terminal_at = "16".to_string();
                row.updated_at = 16;
            }),
    )
    .await?;
    // A cancel may land directly on a still-queued run; the terminal stamp
    // arrives inside the created..updated interval.
    accept_run(
        &database,
        8,
        queued_run("x8").with(|row| {
            row.lifecycle = "cancelled".to_string();
            row.terminal_at = "17".to_string();
            row.updated_at = 17;
        }),
    )
    .await?;

    assert_eq!(
        scalar_i64(&database, "SELECT count(*) FROM assistant_runs").await?,
        10
    );

    let completed = database
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT lifecycle, generation, owner, lease, claim_token, \
             provider_binding_version, terminal_at_ms FROM assistant_runs \
             WHERE lifecycle = 'completed'",
        ))
        .await?
        .ok_or_else(|| std::io::Error::other("completed run vanished"))?;
    assert_eq!(completed.try_get_by_index::<String>(0)?, "completed");
    assert_eq!(completed.try_get_by_index::<i64>(1)?, 1);
    let owner_after_terminal: Option<Vec<u8>> = completed.try_get_by_index(2)?;
    let lease_after_terminal: Option<Vec<u8>> = completed.try_get_by_index(3)?;
    let token_after_terminal: Option<Vec<u8>> = completed.try_get_by_index(4)?;
    assert!(owner_after_terminal.is_none());
    assert!(lease_after_terminal.is_none());
    assert!(token_after_terminal.is_none());
    assert_eq!(completed.try_get_by_index::<i64>(5)?, 1);
    assert_eq!(completed.try_get_by_index::<i64>(6)?, 16);
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn assistant_runs_reject_illegal_state_shapes() -> Result<(), Box<dyn Error>> {
    let database = connect_memory().await?;
    migrate_to_current(&database).await?;
    seed_execution_graph(&database).await?;

    reject_invalid_queued_and_launching_runs(&database).await?;
    reject_invalid_running_and_error_runs(&database).await?;
    reject_invalid_terminal_and_binding_runs(&database).await?;

    assert_eq!(
        scalar_i64(&database, "SELECT count(*) FROM assistant_runs").await?,
        2
    );
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn assistant_run_identity_keys_are_globally_unique() -> Result<(), Box<dyn Error>> {
    let database = connect_memory().await?;
    migrate_to_current(&database).await?;
    seed_execution_graph(&database).await?;

    expect_rejected(
        "duplicate start key",
        &database,
        &queued_run("dup_start")
            .with(|row| row.start_key = start_key_for("r1"))
            .sql(),
    )
    .await;
    expect_rejected(
        "one message originating two runs",
        &database,
        &queued_run("dup_message").sql(),
    )
    .await;
    expect_rejected(
        "one turn originating two runs",
        &database,
        &queued_run("dup_turn")
            .with(|row| row.origin_message = "m2".to_string())
            .sql(),
    )
    .await;
    expect_rejected(
        "origin message from another thread",
        &database,
        &queued_run("cross_message")
            .with_thread("t2")
            .with_origin("m1", "turn_b")
            .sql(),
    )
    .await;
    expect_rejected(
        "origin turn from another thread",
        &database,
        &queued_run("cross_turn")
            .with_thread("t2")
            .with_origin("m3", "turn_a")
            .sql(),
    )
    .await;

    assert_eq!(
        scalar_i64(&database, "SELECT count(*) FROM assistant_runs").await?,
        2
    );
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn assistant_run_transitions_end_in_a_completed_shape() -> Result<(), Box<dyn Error>> {
    let database = connect_memory().await?;
    migrate_to_current(&database).await?;
    seed_execution_graph(&database).await?;

    database
        .execute_unprepared(&format!(
            "UPDATE assistant_runs SET lifecycle = 'launching', generation = 1, \
             owner = {}, lease = {}, claim_token = {}, updated_at_ms = 11 WHERE run_id = 'r1'",
            filled_blob(32, 0x11),
            filled_blob(32, 0x22),
            filled_blob(32, 0x33),
        ))
        .await?;
    database
        .execute_unprepared(&format!(
            "UPDATE assistant_runs SET lifecycle = 'running', claim_token = NULL, \
             provider_binding_version = 1, provider_binding = {}, provider_bound_at_ms = 300, \
             updated_at_ms = 12 WHERE run_id = 'r1'",
            filled_blob(16, 0xee),
        ))
        .await?;
    database
        .execute_unprepared(
            "UPDATE assistant_runs SET lifecycle = 'cancel_requested', updated_at_ms = 13 \
             WHERE run_id = 'r1'",
        )
        .await?;
    database
        .execute_unprepared(
            "UPDATE assistant_runs SET lifecycle = 'interrupted', owner = NULL, lease = NULL, \
             claim_token = NULL, error_code = 'timeout', error_message = 'engine stalled', \
             updated_at_ms = 14 WHERE run_id = 'r1'",
        )
        .await?;
    database
        .execute_unprepared(
            "UPDATE assistant_runs SET lifecycle = 'completed', error_code = NULL, \
             error_message = NULL, terminal_at_ms = 16, updated_at_ms = 16 WHERE run_id = 'r1'",
        )
        .await?;

    let final_row = database
        .query_one_raw(Statement::from_string(
            DbBackend::Sqlite,
            "SELECT lifecycle, generation, terminal_at_ms, provider_binding_version, owner \
             FROM assistant_runs WHERE run_id = 'r1'",
        ))
        .await?
        .ok_or_else(|| std::io::Error::other("run vanished during transitions"))?;
    assert_eq!(final_row.try_get_by_index::<String>(0)?, "completed");
    assert_eq!(final_row.try_get_by_index::<i64>(1)?, 1);
    assert_eq!(final_row.try_get_by_index::<i64>(2)?, 16);
    assert_eq!(final_row.try_get_by_index::<i64>(3)?, 1);
    let owner_after_terminal: Option<Vec<u8>> = final_row.try_get_by_index(4)?;
    assert!(owner_after_terminal.is_none());
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn patches_accept_every_approved_kind() -> Result<(), Box<dyn Error>> {
    let database = connect_memory().await?;
    migrate_to_current(&database).await?;
    seed_execution_graph(&database).await?;
    insert_valid_item(&database, &assistant_item("ia_patched")).await?;
    insert_valid_item(&database, &user_item("iu_patched")).await?;

    turn_upsert_patch("pt_upsert", 1)
        .insert_on(&database)
        .await?;
    item_upsert_patch("pu_upsert", 2, "user_message")
        .insert_on(&database)
        .await?;
    item_upsert_patch("pa_upsert", 3, "assistant_message")
        .insert_on(&database)
        .await?;
    // An empty append fragment is still a valid replay payload.
    item_append_patch("pa_empty", 4, "")
        .insert_on(&database)
        .await?;
    item_append_patch("pa_append", 5, "continued")
        .insert_on(&database)
        .await?;
    lifecycle_patch("pi_done", 6, "item_lifecycle")
        .insert_on(&database)
        .await?;
    lifecycle_patch("pt_done", 7, "turn_lifecycle")
        .insert_on(&database)
        .await?;

    assert_eq!(
        scalar_i64(&database, "SELECT count(*) FROM conversation_patches").await?,
        7
    );
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn patches_reject_cross_kind_payloads_and_duplicates() -> Result<(), Box<dyn Error>> {
    let database = connect_memory().await?;
    migrate_to_current(&database).await?;
    seed_execution_graph(&database).await?;
    insert_valid_item(&database, &assistant_item("ia_patched")).await?;
    insert_valid_item(&database, &user_item("iu_patched")).await?;

    turn_upsert_patch("pt_seed", 1).insert_on(&database).await?;

    expect_rejected(
        "non-positive patch sequence",
        &database,
        &turn_upsert_patch("pt_zero", 0).sql(),
    )
    .await;
    expect_rejected(
        "duplicate thread sequence",
        &database,
        &turn_upsert_patch("pt_repeat", 1).sql(),
    )
    .await;
    expect_rejected(
        "unknown patch kind",
        &database,
        &turn_upsert_patch("pt_unknown", 2)
            .with(|patch| patch.kind = "turn_delta".to_string())
            .sql(),
    )
    .await;
    expect_rejected(
        "missing recorded timestamp",
        &database,
        &turn_upsert_patch("pt_unrecorded", 3)
            .with(|patch| patch.recorded_at = None)
            .sql(),
    )
    .await;
    expect_rejected(
        "turn upsert carrying a body",
        &database,
        &turn_upsert_patch("pt_body", 4)
            .with(|patch| patch.body = Some("no".to_string()))
            .sql(),
    )
    .await;
    expect_rejected(
        "inverted upsert timestamps",
        &database,
        &turn_upsert_patch("pt_times", 5)
            .with(|patch| patch.entity_created_at = Some(30))
            .with(|patch| patch.entity_updated_at = Some(29))
            .sql(),
    )
    .await;
    expect_rejected(
        "append carrying a lifecycle",
        &database,
        &item_append_patch("pa_lifecycle", 6, "more")
            .with(|patch| patch.lifecycle = Some("streaming".to_string()))
            .sql(),
    )
    .await;
    expect_rejected(
        "upsert carrying a fragment",
        &database,
        &item_upsert_patch("pu_fragment", 7, "user_message")
            .with(|patch| patch.fragment = Some(String::new()))
            .sql(),
    )
    .await;
    expect_rejected(
        "lifecycle patch carrying a fragment",
        &database,
        &lifecycle_patch("pi_fragment", 8, "item_lifecycle")
            .with(|patch| patch.fragment = Some("no".to_string()))
            .sql(),
    )
    .await;
    expect_rejected(
        "negative renderer ordinal",
        &database,
        &turn_upsert_patch("pt_ordinal", 9)
            .with(|patch| patch.ordinal = Some(-1))
            .sql(),
    )
    .await;

    assert_eq!(
        scalar_i64(&database, "SELECT count(*) FROM conversation_patches").await?,
        1
    );
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn patches_bind_targets_to_the_patch_thread() -> Result<(), Box<dyn Error>> {
    let database = connect_memory().await?;
    migrate_to_current(&database).await?;
    seed_execution_graph(&database).await?;
    insert_valid_item(&database, &assistant_item("ia_local")).await?;
    let foreign_item = assistant_item("ib_other").with(|item| {
        item.thread = "t2".to_string();
        item.turn = "turn_b".to_string();
        item.ordinal = 1;
        item.run = Some("r2".to_string());
    });
    insert_valid_item(&database, &foreign_item).await?;

    expect_rejected(
        "patch reaching another thread's turn",
        &database,
        &turn_upsert_patch("pt_cross", 1)
            .with(|patch| patch.turn = Some("turn_b".to_string()))
            .sql(),
    )
    .await;
    expect_rejected(
        "patch reaching another thread's item",
        &database,
        &item_append_patch("pa_cross", 2, "text")
            .with(|patch| patch.item = Some("ib_other".to_string()))
            .sql(),
    )
    .await;
    expect_rejected(
        "patch referencing a foreign run",
        &database,
        &item_upsert_patch("pa_run", 3, "assistant_message")
            .with(|patch| patch.item = Some("ia_local".to_string()))
            .with(|patch| patch.run = Some("r2".to_string()))
            .sql(),
    )
    .await;

    assert_eq!(
        scalar_i64(&database, "SELECT count(*) FROM conversation_patches").await?,
        0
    );
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn checkpoints_enforce_tuple_completeness_and_bounds() -> Result<(), Box<dyn Error>> {
    let database = connect_memory().await?;
    migrate_to_current(&database).await?;
    seed_execution_graph(&database).await?;

    database
        .execute_unprepared(
            "INSERT INTO run_checkpoints (run_id, generation, last_batch_sequence, updated_at_ms) \
             VALUES ('r1', 1, 0, 100)",
        )
        .await?;
    database
        .execute_unprepared(&format!(
            "UPDATE run_checkpoints SET engine_checkpoint_version = 3, engine_checkpoint_blob = {}, \
             updated_at_ms = 101 WHERE run_id = 'r1'",
            filled_blob(262_144, 0xcd),
        ))
        .await?;

    expect_rejected(
        "checkpoint generation below zero",
        &database,
        "UPDATE run_checkpoints SET generation = -1 WHERE run_id = 'r1'",
    )
    .await;
    expect_rejected(
        "checkpoint batch sequence below zero",
        &database,
        "UPDATE run_checkpoints SET last_batch_sequence = -1 WHERE run_id = 'r1'",
    )
    .await;
    expect_rejected(
        "checkpoint version zero",
        &database,
        "UPDATE run_checkpoints SET engine_checkpoint_version = 0 WHERE run_id = 'r1'",
    )
    .await;
    expect_rejected(
        "empty checkpoint blob",
        &database,
        &format!(
            "UPDATE run_checkpoints SET engine_checkpoint_blob = {} WHERE run_id = 'r1'",
            filled_blob(0, 0xcd),
        ),
    )
    .await;
    expect_rejected(
        "oversized checkpoint blob",
        &database,
        &format!(
            "UPDATE run_checkpoints SET engine_checkpoint_blob = {} WHERE run_id = 'r1'",
            filled_blob(262_145, 0xcd),
        ),
    )
    .await;
    expect_rejected(
        "half-written checkpoint tuple",
        &database,
        "UPDATE run_checkpoints SET engine_checkpoint_blob = NULL WHERE run_id = 'r1'",
    )
    .await;
    expect_rejected(
        "second checkpoint for the same run",
        &database,
        "INSERT INTO run_checkpoints (run_id, generation, last_batch_sequence, updated_at_ms) \
         VALUES ('r1', 0, 0, 100)",
    )
    .await;
    // r2 starts its own checkpoint row once r1 holds its single row.
    database
        .execute_unprepared(
            "INSERT INTO run_checkpoints (run_id, generation, last_batch_sequence, updated_at_ms) \
             VALUES ('r2', 0, 0, 100)",
        )
        .await?;
    expect_rejected(
        "checkpoint row for a missing run",
        &database,
        "INSERT INTO run_checkpoints (run_id, generation, last_batch_sequence, updated_at_ms) \
         VALUES ('ghost', 0, 0, 100)",
    )
    .await;

    assert_eq!(
        scalar_i64(&database, "SELECT count(*) FROM run_checkpoints").await?,
        2
    );
    database.close().await?;
    Ok(())
}

#[tokio::test]
async fn receipts_keep_per_run_sequences_across_generations() -> Result<(), Box<dyn Error>> {
    let database = connect_memory().await?;
    migrate_to_current(&database).await?;
    seed_execution_graph(&database).await?;

    let receipt_sql = |run: &str, sequence: i64, generation: i64, committed: i64| {
        format!(
            "INSERT INTO run_batch_receipts \
             (run_id, batch_sequence, generation, digest, committed) \
             VALUES ('{run}', {sequence}, {generation}, {}, {committed})",
            filled_blob(32, 0xab),
        )
    };

    // Batch sequences are per-run counters: both runs independently own
    // sequence 1 across different generations.
    database
        .execute_unprepared(&receipt_sql("r1", 1, 1, 1))
        .await?;
    database
        .execute_unprepared(&receipt_sql("r2", 1, 2, 0))
        .await?;
    database
        .execute_unprepared(&receipt_sql("r1", 2, 1, 0))
        .await?;

    expect_rejected(
        "receipt sequence below one",
        &database,
        &receipt_sql("r1", 0, 1, 1),
    )
    .await;
    expect_rejected(
        "duplicate receipt within a run",
        &database,
        &receipt_sql("r1", 1, 1, 0),
    )
    .await;
    expect_rejected(
        "receipt generation below zero",
        &database,
        &receipt_sql("r1", 3, -1, 0),
    )
    .await;
    expect_rejected(
        "short digest",
        &database,
        &format!(
            "INSERT INTO run_batch_receipts \
             (run_id, batch_sequence, generation, digest, committed) \
             VALUES ('r1', 3, 1, {}, 1)",
            filled_blob(31, 0xab),
        ),
    )
    .await;
    expect_rejected(
        "long digest",
        &database,
        &format!(
            "INSERT INTO run_batch_receipts \
             (run_id, batch_sequence, generation, digest, committed) \
             VALUES ('r1', 3, 1, {}, 1)",
            filled_blob(33, 0xab),
        ),
    )
    .await;
    expect_rejected(
        "receipt for a missing run",
        &database,
        &receipt_sql("ghost", 1, 0, 1),
    )
    .await;

    assert_eq!(
        scalar_i64(&database, "SELECT count(*) FROM run_batch_receipts").await?,
        3
    );
    assert_eq!(
        scalar_i64(
            &database,
            "SELECT count(*) FROM run_batch_receipts WHERE committed = 1"
        )
        .await?,
        1
    );
    database.close().await?;
    Ok(())
}
