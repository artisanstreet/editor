use crate::content::ContentError;
use crate::error::AtStep as _;
use crate::error::StorageError;
use rusqlite::{Connection, OptionalExtension};
use std::path::Path;
use std::time::{Duration, SystemTime};

mod backup;
mod frozen;

pub use backup::{BackupError, FinishUpgradeError, RollbackError};
pub(crate) use backup::{Cleared, Rollback};

#[derive(Clone, Debug)]
pub(crate) struct Upgrade {
    pub(crate) at: artisan_api::Timestamp,
    pub(crate) now: SystemTime,
    pub(crate) pace: crate::progress::Progress,
}

impl Upgrade {
    pub(crate) fn of(environment: &crate::Environment) -> Self {
        let at = environment.clock.now();
        let millis = u64::try_from(at).expect("invariant: the wall clock reads after 1970");
        Self {
            at,
            now: SystemTime::UNIX_EPOCH + Duration::from_millis(millis),
            pace: environment.progress.clone(),
        }
    }
}

type Result<T, E = MigrationFailure> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum MigrationFailure {
    #[error("question item {item} has no {part}")]
    Question { item: String, part: QuestionPart },
    #[error("item {item} is not JSON")]
    Body {
        item: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("outline user message {item}")]
    Outline {
        item: String,
        #[source]
        source: frozen::OutlineError,
    },
    #[error(transparent)]
    Content(#[from] ContentError),
    #[error(transparent)]
    Storage(#[from] StorageError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum QuestionPart {
    #[error("kind")]
    Kind,
    #[error("questions")]
    Questions,
    #[error("option list")]
    Options,
    #[error("text answer")]
    TextAnswer,
}

#[derive(Debug, thiserror::Error)]
pub enum SchemaError {
    #[error("read the database's schema version")]
    Version(#[source] rusqlite::Error),
    #[error("the database's schema {found} is newer than this Forge's {supported}")]
    Newer { found: usize, supported: usize },
    #[error("back the database up before migrating from schema {version}")]
    Backup {
        version: usize,
        #[source]
        source: BackupError,
    },
    #[error("report progress while applying schema migration {migration}")]
    Progress {
        migration: usize,
        #[source]
        source: Box<rusqlite::Error>,
    },
    #[error("apply schema migration {migration}")]
    Step {
        migration: usize,
        #[source]
        source: MigrationFailure,
    },
    #[error("finish the upgrade to schema {version}")]
    Finish {
        version: usize,
        #[source]
        source: FinishUpgradeError,
    },
}

#[cfg(test)]
#[path = "../tests/schema/model.rs"]
mod model_migration_tests;

#[cfg(test)]
#[path = "../tests/schema/released.rs"]
mod released_tests;

enum Step {
    Sql {
        file: &'static str,
        sql: &'static str,
    },
    Rust(fn(&Connection) -> Result<()>),
    Stamped(fn(&crate::content::Stamped<'_>) -> Result<()>),
}

macro_rules! sql {
    ($file:literal) => {
        Step::Sql {
            file: $file,
            sql: include_str!(concat!("sql/", $file)),
        }
    };
}

const MIGRATIONS: &[&[Step]] = &[
    &[sql!("migrations/v01.sql")],
    &[sql!("migrations/v02.sql")],
    &[sql!("migrations/v03.sql")],
    &[sql!("migrations/v04.sql")],
    &[sql!("migrations/v05.sql")],
    &[sql!("migrations/v06.sql")],
    &[sql!("migrations/v07.sql")],
    &[sql!("migrations/v08.sql")],
    &[sql!("migrations/v09.sql"), Step::Rust(backfill_edit_totals)],
    &[Step::Rust(migrate_payloads)],
    &[sql!("migrations/v11.sql")],
    &[sql!("migrations/v12.sql")],
    &[sql!("migrations/v13.sql"), Step::Stamped(migrate_sources)],
    &[
        sql!("migrations/v14.sql"),
        Step::Stamped(migrate_collections),
    ],
    &[sql!("migrations/v15.sql")],
    &[sql!("migrations/v16.sql")],
    &[Step::Rust(migrate_approvals), sql!("migrations/v17.sql")],
    &[sql!("migrations/v18.sql")],
    &[sql!("migrations/v19.sql")],
    &[],
    &[sql!("migrations/v21.sql")],
    &[
        sql!("migrations/v22_a.sql"),
        Step::Rust(backfill_work),
        sql!("migrations/v22_b.sql"),
    ],
    &[sql!("migrations/v23.sql")],
    &[Step::Rust(migrate_model_permissions)],
    &[sql!("migrations/v25.sql")],
    &[sql!("migrations/v26.sql")],
    &[sql!("migrations/v27.sql")],
    &[
        sql!("migrations/v28_a.sql"),
        Step::Rust(backfill_outline),
        sql!("migrations/v28_b.sql"),
    ],
    &[sql!("migrations/v29.sql")],
    &[],
    &[sql!("migrations/v31.sql")],
    &[sql!("migrations/v32.sql")],
];

pub(crate) const fn version() -> usize {
    MIGRATIONS.len()
}

fn migrate_model_permissions(conn: &Connection) -> Result<()> {
    use serde_json::Value;
    let Some(text) = conn
        .query_row(
            include_str!("sql/migrations/kv_value.sql"),
            [artisan_api::MODEL_CONFIGS_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .at("migrations/kv_value.sql")?
    else {
        return Ok(());
    };
    let mut configs: Value = crate::read::from_body(&text)?;
    let Some(configs_map) = configs.as_object_mut() else {
        return Ok(());
    };
    for config in configs_map.values_mut() {
        let Some(config) = config.as_object_mut() else {
            continue;
        };
        if config.contains_key("permissions") {
            continue;
        }
        let never_asks = config.get("approvals").and_then(Value::as_str) == Some("never")
            || config
                .get("options")
                .and_then(|options| options.get("permission_mode"))
                .and_then(Value::as_str)
                == Some("bypassPermissions");
        config.insert(
            "permissions".into(),
            Value::String(if never_asks { "full" } else { "auto" }.into()),
        );
        config.remove("access");
        config.remove("approvals");
        if let Some(options) = config.get_mut("options").and_then(Value::as_object_mut) {
            options.remove("permission_mode");
            options.remove("mode");
        }
    }
    conn.execute(
        include_str!("sql/migrations/set_kv_value.sql"),
        rusqlite::params![artisan_api::MODEL_CONFIGS_KEY, configs.to_string()],
    )
    .at("migrations/set_kv_value.sql")?;
    Ok(())
}

fn body(item: &str, text: &str) -> Result<serde_json::Value> {
    serde_json::from_str(text).map_err(|source| MigrationFailure::Body {
        item: item.to_owned(),
        source,
    })
}

fn backfill_work(conn: &Connection) -> Result<()> {
    let mut select = conn
        .prepare(include_str!("sql/migrations/work_candidates.sql"))
        .at("migrations/work_candidates.sql")?;
    let mut rows = select.query([]).at("migrations/work_candidates.sql")?;
    let mut update = conn
        .prepare(include_str!("sql/migrations/mark_work.sql"))
        .at("migrations/mark_work.sql")?;
    while let Some(row) = rows.next().at("migrations/work_candidates.sql")? {
        let id: String = row.get(0).at("migrations/work_candidates.sql")?;
        if frozen::folds_into_work(&body(
            &id,
            &row.get::<_, String>(1)
                .at("migrations/work_candidates.sql")?,
        )?) {
            update.execute([id]).at("migrations/mark_work.sql")?;
        }
    }
    Ok(())
}

pub(crate) fn legacy_approval(item: &mut serde_json::Value) {
    let Some(kind) = item.get_mut("kind") else {
        return;
    };
    if kind.get("type").and_then(serde_json::Value::as_str) != Some("approval") {
        return;
    }
    let approval = kind
        .get_mut("approval")
        .map(serde_json::Value::take)
        .unwrap_or_default();
    *kind = serde_json::json!({
        "type": "tool",
        "tool": {"kind": "other", "name": "approval", "input": null},
        "title": null,
        "output": null,
        "exit_code": null,
        "approval": approval,
    });
}

fn migrate_approvals(conn: &Connection) -> Result<()> {
    use serde_json::Value;
    let approvals: Vec<(String, String, String)> = conn
        .prepare(include_str!("sql/migrations/approvals.sql"))
        .at("migrations/approvals.sql")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .at("migrations/approvals.sql")?
        .collect::<rusqlite::Result<_>>()
        .at("migrations/approvals.sql")?;
    for (id, thread, body) in approvals {
        let mut value: Value = crate::read::from_body(&body)?;
        let target = value
            .pointer("/kind/target")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let gated: Option<(String, Value)> = match &target {
            Some(target) => conn
                .query_row(
                    include_str!("sql/migrations/approval_target.sql"),
                    rusqlite::params![target, thread],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .at("migrations/approval_target.sql")?
                .map(|(id, body)| crate::read::from_body(&body).map(|body| (id, body)))
                .transpose()?,
            None => None,
        };
        if let Some((target, mut gated)) = gated {
            gated["kind"]["approval"] = value["kind"]["approval"].take();
            conn.execute(
                include_str!("sql/migrations/set_item_body.sql"),
                rusqlite::params![target, gated.to_string()],
            )
            .at("migrations/set_item_body.sql")?;
            conn.execute(include_str!("sql/migrations/delete_item.sql"), [&id])
                .at("migrations/delete_item.sql")?;
        } else {
            legacy_approval(&mut value);
            conn.execute(
                include_str!("sql/migrations/approval_as_tool.sql"),
                rusqlite::params![id, value.to_string()],
            )
            .at("migrations/approval_as_tool.sql")?;
        }
    }
    Ok(())
}

pub(crate) fn migrate(conn: &mut Connection, upgrade: &Upgrade) -> Result<(), SchemaError> {
    migrate_to(conn, MIGRATIONS.len(), upgrade)
}

pub(crate) fn roll_back(
    database: &Path,
    at: artisan_api::Timestamp,
    pace: &crate::progress::Progress,
) -> Result<Rollback, RollbackError> {
    backup::roll_back(database, MIGRATIONS.len(), at, pace)
}

pub(crate) fn roll_back_to(
    database: &Path,
    supported: usize,
    at: artisan_api::Timestamp,
    pace: &crate::progress::Progress,
) -> Result<Rollback, RollbackError> {
    backup::roll_back(database, supported, at, pace)
}

pub(crate) fn clear(
    database: &Path,
    at: artisan_api::Timestamp,
    pace: &crate::progress::Progress,
) -> Result<Cleared, RollbackError> {
    backup::clear(database, at, pace)
}

fn migrate_to(conn: &mut Connection, target: usize, upgrade: &Upgrade) -> Result<(), SchemaError> {
    let version: usize = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(SchemaError::Version)?;
    if version > MIGRATIONS.len() {
        return Err(SchemaError::Newer {
            found: version,
            supported: MIGRATIONS.len(),
        });
    }
    if version < target {
        backup::before_migrating(conn, version, &upgrade.pace)
            .map_err(|source| SchemaError::Backup { version, source })?;
    }
    for (index, steps) in MIGRATIONS.iter().enumerate().take(target).skip(version) {
        let next = index + 1;
        tracing::info!("upgrading the database to schema {next} of {target}");
        let progress = |source| SchemaError::Progress {
            migration: next,
            source: Box::new(source),
        };
        crate::progress::watch(
            conn,
            crate::progress::Step::Migration { next },
            &upgrade.pace,
        )
        .map_err(progress)?;
        let applied = apply(conn, next, steps, upgrade.at);
        crate::progress::unwatch(conn).map_err(progress)?;
        applied.map_err(|source| SchemaError::Step {
            migration: next,
            source,
        })?;
    }
    backup::after_migrating(conn, upgrade.now, &upgrade.pace).map_err(|source| {
        SchemaError::Finish {
            version: target,
            source,
        }
    })
}

fn apply(
    conn: &mut Connection,
    next: usize,
    steps: &[Step],
    now: artisan_api::Timestamp,
) -> Result<()> {
    let tx = conn.transaction().at("begin the migration")?;
    for step in steps {
        match step {
            Step::Sql { file, sql } => tx.execute_batch(sql).at(file)?,
            Step::Rust(translate) => translate(&tx)?,
            Step::Stamped(translate) => translate(&crate::content::Stamped { conn: &tx, now })?,
        }
    }
    tx.pragma_update(None, "user_version", next)
        .at("record the new schema version")?;
    tx.commit().at("commit the migration")?;
    Ok(())
}

fn backfill_edit_totals(conn: &Connection) -> Result<()> {
    let mut select = conn
        .prepare(include_str!("sql/migrations/edit_tools.sql"))
        .at("migrations/edit_tools.sql")?;
    let mut rows = select.query([]).at("migrations/edit_tools.sql")?;
    let mut update = conn
        .prepare(include_str!("sql/migrations/set_edit_totals.sql"))
        .at("migrations/set_edit_totals.sql")?;
    while let Some(row) = rows.next().at("migrations/edit_tools.sql")? {
        let id: String = row.get(0).at("migrations/edit_tools.sql")?;
        let item = body(
            &id,
            &row.get::<_, String>(1).at("migrations/edit_tools.sql")?,
        )?;
        if let Some(stats) = item.pointer("/kind/tool").and_then(frozen::edit_totals) {
            update
                .execute(rusqlite::params![id, stats.added, stats.removed])
                .at("migrations/set_edit_totals.sql")?;
        }
    }
    Ok(())
}

fn backfill_outline(conn: &Connection) -> Result<()> {
    let mut select = conn
        .prepare(include_str!("sql/migrations/user_messages.sql"))
        .at("migrations/user_messages.sql")?;
    let mut rows = select.query([]).at("migrations/user_messages.sql")?;
    let mut update = conn
        .prepare(include_str!("sql/migrations/set_outline.sql"))
        .at("migrations/set_outline.sql")?;
    while let Some(row) = rows.next().at("migrations/user_messages.sql")? {
        let id: String = row.get(0).at("migrations/user_messages.sql")?;
        let item = body(
            &id,
            &row.get::<_, String>(1).at("migrations/user_messages.sql")?,
        )?;
        let label =
            frozen::outline_entry(conn, &item).map_err(|source| MigrationFailure::Outline {
                item: id.clone(),
                source,
            })?;
        if let Some(label) = label {
            update
                .execute(rusqlite::params![id, label])
                .at("migrations/set_outline.sql")?;
        }
    }
    Ok(())
}

fn migrate_sources(conn: &crate::content::Stamped<'_>) -> Result<()> {
    Ok(crate::content::migrate(conn, false)?)
}

fn migrate_collections(conn: &crate::content::Stamped<'_>) -> Result<()> {
    Ok(crate::content::migrate(conn, true)?)
}

fn canonical_config(config: &mut serde_json::Value) {
    use serde_json::Value;
    let engine = config
        .get("engine")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let has = |value: &Value, key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
    };
    match engine.as_str() {
        "codex" => {
            let old = config
                .get_mut("options")
                .and_then(Value::as_object_mut)
                .and_then(|options| options.remove("service_tier"));
            if !has(config, "speed")
                && let Some(Value::String(speed)) = old
                && !speed.is_empty()
            {
                config["speed"] = Value::String(speed);
            }
        }
        "cursor" => {
            let old = config
                .get_mut("options")
                .and_then(Value::as_object_mut)
                .and_then(|options| options.remove("fast"));
            if !has(config, "speed") && old == Some(Value::Bool(true)) {
                config["speed"] = Value::String("fast".into());
            }
        }
        "opencode" => {
            let old = config
                .as_object_mut()
                .and_then(|config| config.remove("effort"));
            if !has(config, "variant")
                && let Some(Value::String(variant)) = old
                && !variant.is_empty()
            {
                config["variant"] = Value::String(variant);
            }
        }
        _ => {}
    }
}
fn canonical_question(item: &str, body: &mut serde_json::Value) -> Result<()> {
    use serde_json::{Value, json};
    let lacks = |part| MigrationFailure::Question {
        item: item.to_owned(),
        part,
    };
    let kind = body
        .get_mut("kind")
        .ok_or_else(|| lacks(QuestionPart::Kind))?;
    let questions = kind
        .get_mut("questions")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| lacks(QuestionPart::Questions))?;
    for question in questions.iter_mut() {
        for (index, option) in question
            .get_mut("options")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| lacks(QuestionPart::Options))?
            .iter_mut()
            .enumerate()
        {
            if option.get("id").is_none() {
                option["id"] = Value::String(format!("option-{index}"));
            }
        }
    }
    let questions = questions.clone();
    if let Some(answers) = kind.get_mut("answers").and_then(Value::as_array_mut) {
        for (question, answer) in questions.iter().zip(answers) {
            let Some(old) = answer.as_array() else {
                continue;
            };
            if old.is_empty() {
                *answer = Value::Null;
                continue;
            }
            let labels: Vec<_> = old
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .ok_or_else(|| lacks(QuestionPart::TextAnswer))
                })
                .collect::<Result<_>>()?;
            let options = question["options"]
                .as_array()
                .ok_or_else(|| lacks(QuestionPart::Options))?;
            let ids: Option<Vec<_>> = labels
                .iter()
                .map(|label| {
                    options
                        .iter()
                        .find(|option| option["label"].as_str() == Some(*label))
                        .map(|option| option["id"].clone())
                })
                .collect();
            *answer = ids.map_or_else(
                || json!({"kind":"text","text":labels.join(", ")}),
                |ids| json!({"kind":"choices","ids":ids}),
            );
        }
    }
    Ok(())
}
fn migrate_payloads(conn: &Connection) -> Result<()> {
    let mut select = conn
        .prepare(include_str!("sql/migrations/threads.sql"))
        .at("migrations/threads.sql")?;
    let mut rows = select.query([]).at("migrations/threads.sql")?;
    let mut update = conn
        .prepare(include_str!("sql/migrations/set_thread_body.sql"))
        .at("migrations/set_thread_body.sql")?;
    while let Some(row) = rows.next().at("migrations/threads.sql")? {
        let id: String = row.get(0).at("migrations/threads.sql")?;
        let text: String = row.get(1).at("migrations/threads.sql")?;
        let mut value: serde_json::Value = crate::read::from_body(&text)?;
        if let Some(config) = value.get_mut("config") {
            canonical_config(config);
        }
        update
            .execute(rusqlite::params![id, value.to_string()])
            .at("migrations/set_thread_body.sql")?;
    }
    for key in ["default_engine_config", "model_engine_configs"] {
        let value: Option<String> = conn
            .query_row(include_str!("sql/migrations/kv_value.sql"), [key], |row| {
                row.get(0)
            })
            .optional()
            .at("migrations/kv_value.sql")?;
        if let Some(value) = value {
            let mut value: serde_json::Value = crate::read::from_body(&value)?;
            if key == "model_engine_configs" {
                if let Some(configs) = value.as_object_mut() {
                    for config in configs.values_mut() {
                        canonical_config(config);
                    }
                }
            } else {
                canonical_config(&mut value);
            }
            conn.execute(
                include_str!("sql/migrations/set_kv_value.sql"),
                rusqlite::params![key, value.to_string()],
            )
            .at("migrations/set_kv_value.sql")?;
        }
    }
    let mut select = conn
        .prepare(include_str!("sql/migrations/questions.sql"))
        .at("migrations/questions.sql")?;
    let mut rows = select.query([]).at("migrations/questions.sql")?;
    let mut update = conn
        .prepare(include_str!("sql/migrations/set_item_body.sql"))
        .at("migrations/set_item_body.sql")?;
    while let Some(row) = rows.next().at("migrations/questions.sql")? {
        let id: String = row.get(0).at("migrations/questions.sql")?;
        let mut value: serde_json::Value =
            crate::read::from_body(&row.get::<_, String>(1).at("migrations/questions.sql")?)?;
        canonical_question(&id, &mut value)?;
        update
            .execute(rusqlite::params![id, value.to_string()])
            .at("migrations/set_item_body.sql")?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/schema/canonical_migration.rs"]
mod canonical_migration_tests;

#[cfg(test)]
#[path = "../tests/schema/activity_time_migration.rs"]
mod activity_time_migration_tests;

#[cfg(test)]
#[path = "../tests/schema/item_column_migration.rs"]
mod item_column_migration_tests;

#[cfg(test)]
#[path = "../tests/schema/folded_text_migration.rs"]
mod folded_text_migration_tests;

#[cfg(test)]
#[path = "../tests/schema/frozen_migration.rs"]
mod frozen_migration_tests;

#[cfg(test)]
#[path = "../tests/schema/failure.rs"]
mod failure_tests;

#[cfg(test)]
#[path = "../tests/schema/diff_totals_migration.rs"]
mod diff_totals_migration_tests;
