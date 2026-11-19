use crate::content::ContentError;
use crate::error::StorageError;
use rusqlite::{Connection, OptionalExtension};
use std::path::Path;

mod backup;

pub use backup::{BackupError, Rollback, RollbackError};

type Result<T, E = MigrationFailure> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum MigrationFailure {
    #[error("question item {item} has no {part}")]
    Question { item: String, part: QuestionPart },
    #[error(transparent)]
    Content(#[from] ContentError),
    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl From<rusqlite::Error> for MigrationFailure {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error.into())
    }
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
    #[error("apply schema migration {migration}")]
    Step {
        migration: usize,
        #[source]
        source: MigrationFailure,
    },
}

#[cfg(test)]
#[path = "../tests/schema/model.rs"]
mod model_migration_tests;

#[cfg(test)]
#[path = "../tests/schema/released.rs"]
mod released_tests;

enum Step {
    Sql(&'static str),
    Rust(fn(&Connection) -> Result<()>),
}

const MIGRATIONS: &[&[Step]] = &[
    &[Step::Sql(include_str!("sql/migrations/v01.sql"))],
    &[Step::Sql(include_str!("sql/migrations/v02.sql"))],
    &[Step::Sql(include_str!("sql/migrations/v03.sql"))],
    &[Step::Sql(include_str!("sql/migrations/v04.sql"))],
    &[Step::Sql(include_str!("sql/migrations/v05.sql"))],
    &[Step::Sql(include_str!("sql/migrations/v06.sql"))],
    &[Step::Sql(include_str!("sql/migrations/v07.sql"))],
    &[Step::Sql(include_str!("sql/migrations/v08.sql"))],
    &[
        Step::Sql(include_str!("sql/migrations/v09.sql")),
        Step::Rust(backfill_edit_totals),
    ],
    &[Step::Rust(migrate_payloads)],
    &[Step::Sql(include_str!("sql/migrations/v11.sql"))],
    &[Step::Sql(include_str!("sql/migrations/v12.sql"))],
    &[
        Step::Sql(include_str!("sql/migrations/v13.sql")),
        Step::Rust(migrate_sources),
    ],
    &[Step::Sql(COLLECTIONS), Step::Rust(migrate_collections)],
    &[Step::Sql(SOURCE_INDEXES)],
    &[Step::Sql(include_str!("sql/migrations/v16.sql"))],
    &[
        Step::Rust(migrate_approvals),
        Step::Sql(include_str!("sql/migrations/v17.sql")),
    ],
    &[Step::Sql(include_str!("sql/migrations/v18.sql"))],
    &[Step::Sql(include_str!("sql/migrations/v19.sql"))],
    &[],
    &[Step::Sql(include_str!("sql/migrations/v21.sql"))],
    &[
        Step::Sql(include_str!("sql/migrations/v22_a.sql")),
        Step::Rust(backfill_work),
        Step::Sql(include_str!("sql/migrations/v22_b.sql")),
    ],
    &[Step::Sql(include_str!("sql/migrations/v23.sql"))],
    &[Step::Rust(migrate_model_permissions)],
    &[Step::Sql(include_str!("sql/migrations/v25.sql"))],
    &[Step::Sql(include_str!("sql/coordination/schema.sql"))],
    &[Step::Sql(include_str!("sql/migrations/v27.sql"))],
    &[
        Step::Sql(include_str!("sql/migrations/v28_a.sql")),
        Step::Rust(backfill_outline),
        Step::Sql(include_str!("sql/migrations/v28_b.sql")),
    ],
    &[Step::Sql(include_str!("sql/pairing/track_revisions.sql"))],
    &[],
];

pub const fn version() -> usize {
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
        .optional()?
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
    )?;
    Ok(())
}

fn backfill_work(conn: &Connection) -> Result<()> {
    let mut select = conn.prepare(include_str!("sql/migrations/work_candidates.sql"))?;
    let mut rows = select.query([])?;
    let mut update = conn.prepare(include_str!("sql/migrations/mark_work.sql"))?;
    while let Some(row) = rows.next()? {
        let body: String = row.get(1)?;
        if serde_json::from_str::<artisan_api::Item>(&body).is_ok_and(|item| item.folds_into_work())
        {
            update.execute([row.get::<_, String>(0)?])?;
        }
    }
    Ok(())
}

pub fn legacy_approval(item: &mut serde_json::Value) {
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
        .prepare(include_str!("sql/migrations/approvals.sql"))?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
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
                .optional()?
                .map(|(id, body)| crate::read::from_body(&body).map(|body| (id, body)))
                .transpose()?,
            None => None,
        };
        if let Some((target, mut gated)) = gated {
            gated["kind"]["approval"] = value["kind"]["approval"].take();
            let item: artisan_api::Item = crate::read::from_value(gated)?;
            conn.execute(
                include_str!("sql/migrations/set_item_body.sql"),
                rusqlite::params![target, crate::read::to_body(&item)?],
            )?;
            crate::content::own(conn, &item)?;
            conn.execute(include_str!("sql/migrations/delete_item.sql"), [&id])?;
        } else {
            legacy_approval(&mut value);
            let item: artisan_api::Item = crate::read::from_value(value)?;
            conn.execute(
                include_str!("sql/migrations/approval_as_tool.sql"),
                rusqlite::params![id, crate::read::to_body(&item)?],
            )?;
        }
    }
    Ok(())
}

const COLLECTIONS: &str = include_str!("sql/migrations/v14.sql");

const SOURCE_INDEXES: &str = include_str!("sql/migrations/v15.sql");

pub fn migrate(conn: &mut Connection) -> Result<(), SchemaError> {
    migrate_to(conn, MIGRATIONS.len())
}

pub fn roll_back(database: &Path) -> Result<Rollback, RollbackError> {
    backup::roll_back(database, MIGRATIONS.len())
}

fn migrate_to(conn: &mut Connection, target: usize) -> Result<(), SchemaError> {
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
        backup::before_migrating(conn, version)
            .map_err(|source| SchemaError::Backup { version, source })?;
    }
    for (index, steps) in MIGRATIONS.iter().enumerate().take(target).skip(version) {
        let next = index + 1;
        tracing::info!("upgrading the database to schema {next} of {target}");
        apply(conn, next, steps).map_err(|source| SchemaError::Step {
            migration: next,
            source,
        })?;
    }
    Ok(())
}

fn apply(conn: &mut Connection, next: usize, steps: &[Step]) -> Result<()> {
    let tx = conn.transaction()?;
    for step in steps {
        match step {
            Step::Sql(sql) => tx.execute_batch(sql)?,
            Step::Rust(translate) => translate(&tx)?,
        }
    }
    tx.pragma_update(None, "user_version", next)?;
    tx.commit()?;
    Ok(())
}

fn backfill_edit_totals(conn: &Connection) -> Result<()> {
    let mut select = conn.prepare(include_str!("sql/migrations/edit_tools.sql"))?;
    let mut rows = select.query([])?;
    let mut update = conn.prepare(include_str!("sql/migrations/set_edit_totals.sql"))?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let body: serde_json::Value = crate::read::from_body(&row.get::<_, String>(1)?)?;
        if let Some(tool) = body.pointer("/kind/tool") {
            let tool: artisan_api::ToolKind = crate::read::from_value(tool.clone())?;
            if let Some(stats) = crate::content::inline_edit_totals(&tool) {
                update.execute(rusqlite::params![id, stats.added, stats.removed])?;
            }
        }
    }
    Ok(())
}

fn backfill_outline(conn: &Connection) -> Result<()> {
    let mut select = conn.prepare(include_str!("sql/migrations/user_messages.sql"))?;
    let mut rows = select.query([])?;
    let mut update = conn.prepare(include_str!("sql/migrations/set_outline.sql"))?;
    while let Some(row) = rows.next()? {
        let body: String = row.get(1)?;
        let Ok(item) = serde_json::from_str::<artisan_api::Item>(&body) else {
            continue;
        };
        if let Some(label) = crate::read::outline_entry(conn, &item)? {
            update.execute(rusqlite::params![row.get::<_, String>(0)?, label])?;
        }
    }
    Ok(())
}

fn migrate_sources(conn: &Connection) -> Result<()> {
    Ok(crate::content::migrate(conn, false)?)
}

fn migrate_collections(conn: &Connection) -> Result<()> {
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
    let mut select = conn.prepare(include_str!("sql/migrations/threads.sql"))?;
    let mut rows = select.query([])?;
    let mut update = conn.prepare(include_str!("sql/migrations/set_thread_body.sql"))?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let text: String = row.get(1)?;
        let mut value: serde_json::Value = crate::read::from_body(&text)?;
        if let Some(config) = value.get_mut("config") {
            canonical_config(config);
        }
        update.execute(rusqlite::params![id, value.to_string()])?;
    }
    for key in ["default_engine_config", "model_engine_configs"] {
        let value: Option<String> = conn
            .query_row(include_str!("sql/migrations/kv_value.sql"), [key], |row| {
                row.get(0)
            })
            .optional()?;
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
            )?;
        }
    }
    let mut select = conn.prepare(include_str!("sql/migrations/questions.sql"))?;
    let mut rows = select.query([])?;
    let mut update = conn.prepare(include_str!("sql/migrations/set_item_body.sql"))?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let mut value: serde_json::Value = crate::read::from_body(&row.get::<_, String>(1)?)?;
        canonical_question(&id, &mut value)?;
        update.execute(rusqlite::params![id, value.to_string()])?;
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
