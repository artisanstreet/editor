//! Forge settings, drafts and secrets. Settings sync to clients; secrets
//! never leave Forge.

use crate::{Store, read};
use anyhow::{Context, Result, ensure};
use artisan_api::EngineId;
use rusqlite::{OptionalExtension, params};

impl Store {
    pub async fn setting(&self, key: &str) -> Result<Option<serde_json::Value>> {
        let key = key.to_owned();
        self.read(move |conn| {
            read::setting(conn, &key).with_context(|| format!("read setting {key}"))
        })
        .await
    }

    pub async fn set_setting(&self, key: String, value: serde_json::Value) -> Result<()> {
        self.write(move |tx| tx.save_setting(key, value)).await
    }

    /// Sets a setting unless it exists; returns whether it was set.
    pub async fn set_setting_if_absent(
        &self,
        key: String,
        value: serde_json::Value,
    ) -> Result<bool> {
        self.write(move |tx| {
            if read::setting(tx, &key)?.is_some() {
                return Ok(false);
            }
            tx.save_setting(key, value)?;
            Ok(true)
        })
        .await
        .context("set a setting unless present")
    }

    /// Metadata invalidation is small and durable, independent of a text value changing.
    pub async fn bump_revision(&self, key: String) -> Result<u64> {
        self.write(move |tx| {
            let revision = read::setting(tx, &key)?
                .and_then(|value| value.as_u64())
                .unwrap_or(0)
                .checked_add(1)
                .context("revision overflow")?;
            tx.save_setting(key, serde_json::json!(revision))?;
            Ok(revision)
        })
        .await
    }

    /// Compare and change one MCP entry inside the canonical setting transaction.
    pub async fn mutate_mcp(
        &self,
        mutation: artisan_api::McpMutation,
    ) -> Result<Vec<artisan_api::McpServer>> {
        self.write(move |tx| {
            use artisan_api::McpMutation;
            let mut servers: Vec<artisan_api::McpServer> = read::setting(tx, "mcp_servers")?
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            let previous = match &mutation {
                McpMutation::Upsert { previous, .. } => previous.as_ref(),
                McpMutation::Remove { previous } => Some(previous),
            };
            if let Some(previous) = previous {
                ensure!(
                    servers.iter().find(|server| server.name == previous.name) == Some(previous),
                    "MCP entry changed; reload before saving"
                );
            }
            if let McpMutation::Upsert { server, .. } = &mutation {
                ensure!(!server.name.trim().is_empty(), "MCP server needs a name");
                ensure!(
                    !server.command.trim().is_empty()
                        || server
                            .url
                            .as_ref()
                            .is_some_and(|url| !url.trim().is_empty()),
                    "MCP server needs a command or URL"
                );
                ensure!(
                    !servers.iter().any(|other| other.name == server.name
                        && previous.is_none_or(|old| old.name != other.name)),
                    "MCP name already exists"
                );
            }
            if let Some(previous) = previous {
                servers.retain(|server| server.name != previous.name);
            }
            if let McpMutation::Upsert { server, .. } = mutation {
                servers.push(server);
            }
            servers.sort_by(|a, b| a.name.cmp(&b.name));
            tx.save_setting("mcp_servers".into(), serde_json::to_value(&servers)?)?;
            Ok(servers)
        })
        .await
    }

    /// Changes the model against the latest config instead of a client's
    /// stale full snapshot; the model comes back as it was last chosen.
    pub async fn set_default_model(
        &self,
        engine: EngineId,
        model: artisan_api::Model,
    ) -> Result<artisan_api::EngineConfig> {
        self.write(move |tx| {
            let saved = read::setting(tx, artisan_api::MODEL_CONFIGS_KEY)?
                .and_then(|configs| artisan_api::saved_model_config(&configs, engine, &model.id));
            let mut config = match saved {
                Some(config) => config,
                None => read::setting(tx, "default_engine_config")?
                    .map(serde_json::from_value::<artisan_api::EngineConfig>)
                    .transpose()?
                    .filter(|config| config.engine == engine)
                    .unwrap_or_else(|| artisan_api::EngineConfig::new(engine)),
            };
            config.model = Some(model.id.clone());
            model.fit(&mut config);
            tx.save_setting(
                "default_engine_config".into(),
                serde_json::to_value(&config)?,
            )?;
            tx.remember_model_config(&config)?;
            Ok(config)
        })
        .await
    }

    /// Remembers `config` as the one its model comes back with, fitted to
    /// what `model` offers.
    pub async fn set_model_config(
        &self,
        mut config: artisan_api::EngineConfig,
        model: &artisan_api::Model,
    ) -> Result<artisan_api::EngineConfig> {
        config.model = Some(model.id.clone());
        model.fit(&mut config);
        self.write(move |tx| {
            tx.remember_model_config(&config)?;
            Ok(config)
        })
        .await
        .context("remember a model's config")
    }

    /// Remembers whether the user opened or closed one of `engine`'s model
    /// groups.
    pub async fn set_model_group(
        &self,
        engine: EngineId,
        group: artisan_api::ModelGroup,
    ) -> Result<()> {
        self.write(move |tx| {
            let mut groups = match read::setting(tx, artisan_api::MODEL_GROUPS_KEY)? {
                Some(serde_json::Value::Object(groups)) => groups,
                _ => serde_json::Map::new(),
            };
            let engine_groups = groups
                .entry(engine.to_string())
                .or_insert_with(|| serde_json::json!({}));
            if !engine_groups.is_object() {
                *engine_groups = serde_json::json!({});
            }
            engine_groups[group.name.as_str()] = serde_json::Value::Bool(group.open);
            tx.save_setting(
                artisan_api::MODEL_GROUPS_KEY.into(),
                serde_json::Value::Object(groups),
            )
        })
        .await
        .context("remember a model group")
    }

    pub async fn set_skill_enabled(&self, id: String, enabled: bool) -> Result<()> {
        self.write(move |tx| {
            let mut disabled: std::collections::BTreeSet<String> =
                read::setting(tx, "disabled_skills")?
                    .map(serde_json::from_value)
                    .transpose()?
                    .unwrap_or_default();
            if enabled {
                disabled.remove(&id);
            } else {
                disabled.insert(id);
            }
            tx.save_setting("disabled_skills".into(), serde_json::to_value(disabled)?)
        })
        .await
    }

    /// Saves a new version of the global prompt and makes it the
    /// `global_prompt` setting, if `base` is still the latest version.
    /// Returns the new version, or `Err(latest)` when `base` is stale.
    pub async fn save_prompt_version(
        &self,
        base: u64,
        text: String,
    ) -> Result<std::result::Result<u64, u64>> {
        self.write(move |tx| {
            let latest = read::prompt_version(tx)?;
            if latest != base {
                return Ok(Err(latest));
            }
            let version = latest + 1;
            tx.execute(
                "INSERT INTO prompt_versions (version, text, saved_at) VALUES (?1, ?2, ?3)",
                params![
                    i64::try_from(version).unwrap_or(i64::MAX),
                    text,
                    artisan_api::now()
                ],
            )?;
            tx.save_setting("global_prompt".to_owned(), serde_json::Value::String(text))?;
            tx.save_setting("global_prompt_version".into(), serde_json::json!(version))?;
            Ok(Ok(version))
        })
        .await
        .context("save the global prompt")
    }

    /// The latest saved version of the global prompt; `0` before the first.
    pub async fn prompt_version(&self) -> Result<u64> {
        self.read(read::prompt_version)
            .await
            .context("read the global prompt's version")
    }

    pub async fn draft(&self, scope: &str) -> Result<artisan_api::DraftSnapshot> {
        let scope = scope.to_owned();
        self.read(move |conn| {
            read::draft(conn, &scope).with_context(|| format!("read draft {scope}"))
        })
        .await
    }

    /// Compare-and-swap prevents delayed writes and clears from overwriting newer input.
    pub async fn set_draft(
        &self,
        scope: String,
        request: artisan_api::WriteDraft,
    ) -> Result<artisan_api::DraftWriteResult> {
        self.write(move |tx| {
            let current = read::draft(tx, &scope)?;
            if current.revision != request.expected_revision {
                return Ok(artisan_api::DraftWriteResult {
                    saved: false,
                    current,
                });
            }
            if current.body == request.body {
                return Ok(artisan_api::DraftWriteResult {
                    saved: true,
                    current,
                });
            }
            let current = artisan_api::DraftSnapshot {
                revision: current
                    .revision
                    .checked_add(1)
                    .context("draft revision exhausted")?,
                body: request.body,
            };
            tx.execute(
                "INSERT INTO drafts (scope, body, revision) VALUES (?1, ?2, ?3)
                ON CONFLICT (scope) DO UPDATE SET body=excluded.body, revision=excluded.revision",
                params![scope, read::to_body(&current.body)?, current.revision],
            )?;
            Ok(artisan_api::DraftWriteResult {
                saved: true,
                current,
            })
        })
        .await
        .context("save draft")
    }

    pub async fn secret(&self, key: &str) -> Result<Option<String>> {
        let key = key.to_owned();
        self.read(move |conn| {
            Ok(conn
                .query_row("SELECT value FROM secrets WHERE key = ?1", [&key], |row| {
                    row.get(0)
                })
                .optional()?)
        })
        .await
    }

    /// Replaces a value only Forge reads, or deletes it with `None`. Not
    /// synced to clients.
    pub async fn set_secret(&self, key: &str, value: Option<String>) -> Result<()> {
        let key = key.to_owned();
        self.write(move |tx| {
            match value {
                Some(value) => tx.execute(
                    "INSERT INTO secrets (key, value) VALUES (?1, ?2)
                     ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                    params![key, value],
                )?,
                None => tx.execute("DELETE FROM secrets WHERE key = ?1", [&key])?,
            };
            Ok(())
        })
        .await
        .context("save secret")
    }

    /// Stores a value only Forge reads (its identity), unless one is there
    /// already; returns whichever is stored. Not synced to clients.
    pub async fn secret_or_insert(&self, key: &str, value: String) -> Result<String> {
        let key = key.to_owned();
        self.write(move |tx| {
            tx.execute(
                "INSERT INTO secrets (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO NOTHING",
                params![key, value],
            )?;
            Ok(
                tx.query_row("SELECT value FROM secrets WHERE key = ?1", [&key], |row| {
                    row.get(0)
                })?,
            )
        })
        .await
        .context("save secret")
    }
}

#[cfg(test)]
mod context_window_tests {
    use super::*;
    use artisan_api::{
        ContextWindow, CreateProject, CreateThread, EngineConfig, Model, ProjectId, ThreadId,
    };

    #[tokio::test]
    async fn saved_context_choice_survives_roundtrip_and_clears_on_incompatible_model() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let mut config = EngineConfig::new(EngineId::Codex);
        config
            .options
            .insert("context_window".to_owned(), serde_json::json!(1_000_000));
        store
            .set_setting(
                "default_engine_config".into(),
                serde_json::to_value(config).unwrap(),
            )
            .await
            .unwrap();
        let extended = Model {
            id: "extended".to_owned(),
            context_windows: vec![
                ContextWindow {
                    tokens: 272_000,
                    override_tokens: None,
                },
                ContextWindow {
                    tokens: 1_000_000,
                    override_tokens: Some(1_000_000),
                },
            ],
            ..Model::default()
        };
        let saved = store
            .set_default_model(EngineId::Codex, extended)
            .await
            .unwrap();
        assert_eq!(saved.context_window_override(), Some(1_000_000));
        let reread: EngineConfig = serde_json::from_value(
            store
                .setting("default_engine_config")
                .await
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(reread, saved);
        let standard = Model {
            id: "standard".to_owned(),
            context_windows: vec![ContextWindow {
                tokens: 400_000,
                override_tokens: None,
            }],
            ..Model::default()
        };
        let saved = store
            .set_default_model(EngineId::Codex, standard)
            .await
            .unwrap();
        assert!(!saved.options.contains_key("context_window"));
    }

    #[tokio::test]
    async fn invalid_context_config_is_rejected_before_thread_is_saved() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let project = store
            .create_project(CreateProject {
                id: ProjectId::new(),
                path: "/context-validation".into(),
                name: None,
            })
            .await
            .unwrap();
        for (engine, value) in [
            (EngineId::Codex, serde_json::json!(0)),
            (EngineId::Claude, serde_json::json!(100_000_001_u64)),
            (EngineId::Codex, serde_json::json!("1000000")),
            (EngineId::OpenCode, serde_json::json!(1_000_000)),
        ] {
            let mut config = EngineConfig::new(engine);
            config.options.insert("context_window".to_owned(), value);
            let result = store
                .create_thread(CreateThread {
                    id: ThreadId::new(),
                    project_id: project.id.clone(),
                    title: None,
                    config,
                    parent_thread_id: None,
                    spawned_by: None,
                })
                .await;
            assert!(result.is_err());
        }
    }
}
