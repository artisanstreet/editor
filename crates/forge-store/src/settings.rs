use crate::error::{StorageError, operation_error};
use crate::writer::WriteFailure;
use crate::{Store, read};
use artisan_api::EngineId;
use rusqlite::{OptionalExtension, params};

#[derive(Debug, thiserror::Error)]
pub enum SettingFailure {
    #[error("the model customizations are invalid")]
    Customizations(#[source] artisan_api::ModelCustomizationsError),
    #[error("the revision is exhausted")]
    RevisionExhausted,
    #[error("MCP entry {server} changed; reload before saving")]
    McpChanged { server: String },
    #[error("MCP server needs a name")]
    McpUnnamed,
    #[error("MCP server {server} needs a command or URL")]
    McpTargetMissing { server: String },
    #[error("MCP name {server} already exists")]
    McpNameTaken { server: String },
    #[error(transparent)]
    Write(#[from] WriteFailure),
}

impl From<StorageError> for SettingFailure {
    fn from(error: StorageError) -> Self {
        Self::Write(error.into())
    }
}

impl From<rusqlite::Error> for SettingFailure {
    fn from(error: rusqlite::Error) -> Self {
        Self::Write(error.into())
    }
}

operation_error!(ReadSettingError(StorageError), "read setting {key}", key: String);
operation_error!(SetSettingError(SettingFailure), "save setting {key}", key: String);
operation_error!(BumpRevisionError(SettingFailure), "bump revision {key}", key: String);
operation_error!(MutateMcpError(SettingFailure), "change the MCP servers");
operation_error!(SetDefaultModelError(SettingFailure), "make {model} the default {engine} model", engine: EngineId, model: String);
operation_error!(SetModelConfigError(SettingFailure), "remember the config of {engine} model {model}", engine: EngineId, model: String);
operation_error!(SetModelGroupError(WriteFailure), "remember {engine} model group {group}", engine: EngineId, group: String);
operation_error!(SetSkillEnabledError(WriteFailure), "turn skill {skill} on or off", skill: String);
operation_error!(SavePromptError(WriteFailure), "save the global prompt over version {base}", base: u64);
operation_error!(
    PromptVersionError(StorageError),
    "read the global prompt's version"
);
operation_error!(ReadDraftError(StorageError), "read draft {scope}", scope: String);
operation_error!(SetDraftError(SettingFailure), "save draft {scope}", scope: String);
operation_error!(SetMcpSecretError(StorageError), "save a secret of MCP server {server}", server: String);
operation_error!(ReadSecretError(StorageError), "read secret {key}", key: String);
operation_error!(SetSecretError(StorageError), "save secret {key}", key: String);

impl Store {
    pub async fn setting(&self, key: &str) -> Result<Option<serde_json::Value>, ReadSettingError> {
        let read_key = key.to_owned();
        let key = key.to_owned();
        self.read(move |conn| read::setting(conn, &read_key))
            .await
            .map_err(|cause| ReadSettingError {
                key,
                cause: Box::new(cause),
            })
    }

    pub async fn set_setting(
        &self,
        key: String,
        value: serde_json::Value,
    ) -> Result<(), SetSettingError> {
        let saved = key.clone();
        self.write_acknowledged(move |tx| {
            if key == artisan_api::MODEL_CUSTOMIZATIONS_KEY {
                artisan_api::model_customizations(&value)
                    .map_err(SettingFailure::Customizations)?;
            }
            Ok(tx.save_setting(key, value)?)
        })
        .await
        .map_err(|cause| SetSettingError {
            key: saved,
            cause: Box::new(cause),
        })
    }

    pub async fn set_setting_if_absent(
        &self,
        key: String,
        value: serde_json::Value,
    ) -> Result<bool, SetSettingError> {
        let saved = key.clone();
        self.write_acknowledged(move |tx| {
            if read::setting(tx, &key)?.is_some() {
                return Ok(false);
            }
            tx.save_setting(key, value)?;
            Ok(true)
        })
        .await
        .map_err(|cause| SetSettingError {
            key: saved,
            cause: Box::new(cause),
        })
    }

    pub async fn bump_revision(&self, key: String) -> Result<u64, BumpRevisionError> {
        let bumped = key.clone();
        self.write(move |tx| {
            let revision = read::setting(tx, &key)?
                .and_then(|value| value.as_u64())
                .unwrap_or(0)
                .checked_add(1)
                .ok_or(SettingFailure::RevisionExhausted)?;
            tx.save_setting(key, serde_json::json!(revision))?;
            Ok(revision)
        })
        .await
        .map_err(|cause| BumpRevisionError {
            key: bumped,
            cause: Box::new(cause),
        })
    }

    pub async fn mutate_mcp(
        &self,
        mutation: artisan_api::McpMutation,
    ) -> Result<Vec<artisan_api::McpServer>, MutateMcpError> {
        self.mutate_mcp_together(vec![mutation]).await
    }

    pub async fn mutate_mcp_together(
        &self,
        mutations: Vec<artisan_api::McpMutation>,
    ) -> Result<Vec<artisan_api::McpServer>, MutateMcpError> {
        self.write_acknowledged(move |tx| {
            let mut servers: Vec<artisan_api::McpServer> = read::setting(tx, "mcp_servers")?
                .map(read::from_value)
                .transpose()?
                .unwrap_or_default();
            for mutation in mutations {
                apply_mcp_mutation(tx, &mut servers, mutation)?;
            }
            servers.sort_by(|a, b| a.name.cmp(&b.name));
            tx.save_setting("mcp_servers".into(), read::to_value(&servers)?)?;
            Ok(servers)
        })
        .await
        .map_err(|cause| MutateMcpError {
            cause: Box::new(cause),
        })
    }

    /// Changes the model against the latest config instead of a client's
    /// stale full snapshot; the model comes back as it was last chosen.
    pub async fn set_default_model(
        &self,
        engine: EngineId,
        model: artisan_api::Model,
    ) -> Result<artisan_api::EngineConfig, SetDefaultModelError> {
        let chosen = model.id.clone();
        self.write_acknowledged(move |tx| {
            let saved = read::setting(tx, artisan_api::MODEL_CONFIGS_KEY)?
                .and_then(|configs| artisan_api::saved_model_config(&configs, engine, &model.id));
            let mut config = match saved {
                Some(config) => config,
                None => read::setting(tx, artisan_api::HARNESS_CONFIGS_KEY)?
                    .and_then(|configs| configs.get(engine.as_str()).cloned())
                    .or(read::setting(tx, "default_engine_config")?)
                    .map(read::from_value::<artisan_api::EngineConfig>)
                    .transpose()?
                    .filter(|config| config.engine == engine)
                    .unwrap_or_else(|| artisan_api::EngineConfig::new(engine)),
            };
            config.model = Some(model.id.clone());
            let customizations = read::setting(tx, artisan_api::MODEL_CUSTOMIZATIONS_KEY)?
                .as_ref()
                .map(artisan_api::model_customizations)
                .transpose()
                .map_err(SettingFailure::Customizations)?
                .unwrap_or_default();
            config = model.resolve_config(&config, &customizations).effective;
            tx.save_setting("default_engine_config".into(), read::to_value(&config)?)?;
            tx.remember_model_config(&config)?;
            Ok(config)
        })
        .await
        .map_err(|cause| SetDefaultModelError {
            engine,
            model: chosen,
            cause: Box::new(cause),
        })
    }

    /// Remembers `config` as the one its model comes back with, fitted to
    /// what `model` offers.
    pub async fn set_model_config(
        &self,
        mut config: artisan_api::EngineConfig,
        model: &artisan_api::Model,
    ) -> Result<artisan_api::EngineConfig, SetModelConfigError> {
        config.model = Some(model.id.clone());
        let (engine, chosen) = (config.engine, model.id.clone());
        let model = model.clone();
        self.write_acknowledged(move |tx| {
            let customizations = read::setting(tx, artisan_api::MODEL_CUSTOMIZATIONS_KEY)?
                .as_ref()
                .map(artisan_api::model_customizations)
                .transpose()
                .map_err(SettingFailure::Customizations)?
                .unwrap_or_default();
            config = model.resolve_config(&config, &customizations).effective;
            tx.remember_model_config(&config)?;
            Ok(config)
        })
        .await
        .map_err(|cause| SetModelConfigError {
            engine,
            model: chosen,
            cause: Box::new(cause),
        })
    }

    /// Remembers whether the user opened or closed one of `engine`'s model
    /// groups.
    pub async fn set_model_group(
        &self,
        engine: EngineId,
        group: artisan_api::ModelGroup,
    ) -> Result<(), SetModelGroupError> {
        let name = group.name.clone();
        self.write_acknowledged(move |tx| {
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
        .map_err(|cause| SetModelGroupError {
            engine,
            group: name,
            cause: Box::new(cause),
        })
    }

    pub async fn set_skill_enabled(
        &self,
        id: String,
        enabled: bool,
    ) -> Result<(), SetSkillEnabledError> {
        let skill = id.clone();
        self.write_acknowledged(move |tx| {
            let mut disabled: std::collections::BTreeSet<String> =
                read::setting(tx, "disabled_skills")?
                    .map(read::from_value)
                    .transpose()?
                    .unwrap_or_default();
            if enabled {
                disabled.remove(&id);
            } else {
                disabled.insert(id);
            }
            tx.save_setting("disabled_skills".into(), read::to_value(&disabled)?)
        })
        .await
        .map_err(|cause| SetSkillEnabledError {
            skill,
            cause: Box::new(cause),
        })
    }

    /// Saves a new version of the global prompt and makes it the
    /// `global_prompt` setting, if `base` is still the latest version.
    /// Returns the new version, or `Err(latest)` when `base` is stale.
    pub async fn save_prompt_version(
        &self,
        base: u64,
        text: String,
    ) -> Result<Result<u64, u64>, SavePromptError> {
        self.write_acknowledged(move |tx| {
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
        .map_err(|cause| SavePromptError {
            base,
            cause: Box::new(cause),
        })
    }

    pub async fn prompt_version(&self) -> Result<u64, PromptVersionError> {
        self.read(read::prompt_version)
            .await
            .map_err(|cause| PromptVersionError {
                cause: Box::new(cause),
            })
    }

    pub async fn draft(&self, scope: &str) -> Result<artisan_api::DraftSnapshot, ReadDraftError> {
        let read_scope = scope.to_owned();
        let scope = scope.to_owned();
        self.read(move |conn| read::draft(conn, &read_scope))
            .await
            .map_err(|cause| ReadDraftError {
                scope,
                cause: Box::new(cause),
            })
    }

    /// Compare-and-swap prevents delayed writes and clears from overwriting newer input.
    pub async fn set_draft(
        &self,
        scope: String,
        request: artisan_api::WriteDraft,
    ) -> Result<artisan_api::DraftWriteResult, SetDraftError> {
        let saved = scope.clone();
        self.write_acknowledged(move |tx| {
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
                    .ok_or(SettingFailure::RevisionExhausted)?,
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
        .map_err(|cause| SetDraftError {
            scope: saved,
            cause: Box::new(cause),
        })
    }

    pub async fn set_mcp_secret(
        &self,
        server: String,
        url: String,
        secret: McpSecret,
        value: String,
    ) -> Result<bool, SetMcpSecretError> {
        let named = server.clone();
        self.write_acknowledged(move |tx| {
            let servers: Vec<artisan_api::McpServer> = read::setting(tx, "mcp_servers")?
                .map(read::from_value)
                .transpose()?
                .unwrap_or_default();
            let listed = servers
                .iter()
                .any(|listed| listed.name == server && listed.url.as_deref() == Some(url.as_str()));
            if listed {
                tx.execute(
                    "INSERT INTO secrets (key, value) VALUES (?1, ?2)
                     ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                    params![secret.key(&server), value],
                )?;
            }
            Ok(listed)
        })
        .await
        .map_err(|cause| SetMcpSecretError {
            server: named,
            cause: Box::new(cause),
        })
    }

    pub async fn secret(&self, key: &str) -> Result<Option<String>, ReadSecretError> {
        let read_key = key.to_owned();
        let key = key.to_owned();
        self.read(move |conn| {
            Ok(conn
                .query_row(
                    "SELECT value FROM secrets WHERE key = ?1",
                    [&read_key],
                    |row| row.get(0),
                )
                .optional()?)
        })
        .await
        .map_err(|cause| ReadSecretError {
            key,
            cause: Box::new(cause),
        })
    }

    pub async fn set_secret(&self, key: &str, value: Option<String>) -> Result<(), SetSecretError> {
        let saved = key.to_owned();
        let key = key.to_owned();
        self.write_acknowledged(move |tx| {
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
        .map_err(|cause| SetSecretError {
            key: saved,
            cause: Box::new(cause),
        })
    }

    pub async fn secret_or_insert(
        &self,
        key: &str,
        value: String,
    ) -> Result<String, SetSecretError> {
        let saved = key.to_owned();
        let key = key.to_owned();
        self.write_acknowledged(move |tx| {
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
        .map_err(|cause| SetSecretError {
            key: saved,
            cause: Box::new(cause),
        })
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
    async fn each_harness_remembers_its_model_and_preferences_after_reopening() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let mut opencode = EngineConfig::new(EngineId::OpenCode);
        let muse = Model {
            id: "gateway/muse-spark-1.3".into(),
            ..Model::default()
        };
        opencode.model = Some(muse.id.clone());
        opencode.permissions = artisan_api::Permissions::Full;
        opencode
            .options
            .insert("custom".into(), serde_json::json!(true));
        let opencode = store.set_model_config(opencode, &muse).await.unwrap();
        let claude = store
            .set_default_model(
                EngineId::Claude,
                Model {
                    id: "opus".into(),
                    ..Model::default()
                },
            )
            .await
            .unwrap();
        // A new model in OpenCode inherits its own harness preferences,
        // even though Claude is now the global default.
        let other = store
            .set_default_model(
                EngineId::OpenCode,
                Model {
                    id: "gateway/other".into(),
                    ..Model::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(other.permissions, opencode.permissions);
        assert_eq!(other.options, opencode.options);
        store
            .set_model_config(opencode.clone(), &muse)
            .await
            .unwrap();
        drop(store);
        let store = Store::open(directory.path()).unwrap();
        let saved = store
            .setting(artisan_api::HARNESS_CONFIGS_KEY)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved["opencode"], serde_json::to_value(opencode).unwrap());
        assert_eq!(saved["claude"], serde_json::to_value(claude).unwrap());
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum McpSecret {
    Tokens,
    Client,
}

impl McpSecret {
    #[must_use]
    pub fn key(self, server: &str) -> String {
        match self {
            Self::Tokens => format!("mcp_oauth:{server}"),
            Self::Client => format!("mcp_oauth_client:{server}"),
        }
    }
}

fn forget_mcp_secrets(tx: &crate::writer::Tx<'_>, server: &str) -> Result<(), StorageError> {
    tx.prepare_cached(include_str!("sql/mcp/forget.sql"))?
        .execute(params![
            McpSecret::Tokens.key(server),
            McpSecret::Client.key(server)
        ])?;
    Ok(())
}

fn apply_mcp_mutation(
    tx: &crate::writer::Tx<'_>,
    servers: &mut Vec<artisan_api::McpServer>,
    mutation: artisan_api::McpMutation,
) -> Result<(), SettingFailure> {
    use artisan_api::McpMutation;
    let previous = match &mutation {
        McpMutation::Upsert { previous, .. } => previous.as_ref(),
        McpMutation::Remove { previous } => Some(previous),
    };
    if let Some(previous) = previous
        && servers.iter().find(|server| server.name == previous.name) != Some(previous)
    {
        return Err(SettingFailure::McpChanged {
            server: previous.name.clone(),
        });
    }
    if let McpMutation::Upsert { server, .. } = &mutation {
        if server.name.trim().is_empty() {
            return Err(SettingFailure::McpUnnamed);
        }
        if server.command.trim().is_empty()
            && server.url.as_ref().is_none_or(|url| url.trim().is_empty())
        {
            return Err(SettingFailure::McpTargetMissing {
                server: server.name.clone(),
            });
        }
        if servers.iter().any(|other| {
            other.name == server.name && previous.is_none_or(|old| old.name != other.name)
        }) {
            return Err(SettingFailure::McpNameTaken {
                server: server.name.clone(),
            });
        }
    }
    if let Some(previous) = previous {
        servers.retain(|server| server.name != previous.name);
        let kept = match &mutation {
            McpMutation::Upsert { server, .. } => {
                server.name == previous.name && server.url == previous.url
            }
            McpMutation::Remove { .. } => false,
        };
        if !kept {
            forget_mcp_secrets(tx, &previous.name)?;
        }
    }
    if let McpMutation::Upsert { server, .. } = mutation {
        servers.push(server);
    }
    Ok(())
}
