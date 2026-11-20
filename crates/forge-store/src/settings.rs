use crate::error::AtStep as _;
use crate::error::{StorageError, operation_error};
use crate::writer::WriteFailure;
use crate::{Store, read};
use artisan_api::EngineId;
use rusqlite::params;

#[derive(Debug, thiserror::Error)]
pub enum SettingFailure {
    #[error("the model customizations are invalid")]
    Customizations(#[source] artisan_api::ModelCustomizationsError),
    #[error("the revision is exhausted")]
    RevisionExhausted,
    #[error("the global prompt's version {version} doesn't fit the store")]
    PromptVersion {
        version: u64,
        #[source]
        source: std::num::TryFromIntError,
    },
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

operation_error!(ReadSettingError(StorageError), "read setting {key}", key: String);
operation_error!(SetSettingError(SettingFailure), "save setting {key}", key: String);
operation_error!(BumpRevisionError(SettingFailure), "bump revision {key}", key: String);
operation_error!(MutateMcpError(SettingFailure), "change the MCP servers");
operation_error!(SetDefaultModelError(SettingFailure), "make {model} the default {engine} model", engine: EngineId, model: String);
operation_error!(SetModelConfigError(SettingFailure), "remember the config of {engine} model {model}", engine: EngineId, model: String);
operation_error!(SetModelGroupError(WriteFailure), "remember {engine} model group {group}", engine: EngineId, group: String);
operation_error!(SetSkillEnabledError(WriteFailure), "turn skill {skill} on or off", skill: String);
operation_error!(SavePromptError(SettingFailure), "save the global prompt over version {base}", base: u64);
operation_error!(
    ApplyConfigBundleError(SettingFailure),
    "apply a config bundle"
);

pub const CONFIG_SYNC_KEY: &str = "config_sync";

#[derive(Debug)]
pub struct PromptEdit {
    pub base: u64,
    pub text: String,
}

#[derive(Debug)]
pub struct ConfigBundle {
    pub enable_skills: Vec<String>,
    pub prompt: Option<PromptEdit>,
    pub mcp: Vec<artisan_api::McpMutation>,
    pub record: serde_json::Value,
}

#[derive(Debug)]
pub enum BundleCommit {
    Applied { forgotten: ForgottenMcpServers },
    PromptChanged { latest: u64 },
}
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
        let bytes = crate::encoded::len(&value);
        self.write_acknowledged_carrying(bytes, move |tx| {
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
        let bytes = key.len().saturating_add(crate::encoded::len(&value));
        self.write_acknowledged_carrying(bytes, move |tx| {
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
    ) -> Result<McpChange, MutateMcpError> {
        self.mutate_mcp_together(vec![mutation]).await
    }

    async fn mutate_mcp_together(
        &self,
        mutations: Vec<artisan_api::McpMutation>,
    ) -> Result<McpChange, MutateMcpError> {
        let bytes = crate::encoded::len(&mutations);
        self.write_acknowledged_carrying(bytes, move |tx| {
            let mut servers: Vec<artisan_api::McpServer> = read::setting(tx, "mcp_servers")?
                .map(read::from_value)
                .transpose()?
                .unwrap_or_default();
            let mut forgotten = Vec::new();
            for mutation in mutations {
                forgotten.extend(apply_mcp_mutation(tx, &mut servers, mutation)?);
            }
            servers.sort_by(|a, b| a.name.cmp(&b.name));
            tx.save_setting("mcp_servers".into(), read::to_value(&servers)?)?;
            Ok(McpChange {
                servers,
                forgotten: ForgottenMcpServers(forgotten),
            })
        })
        .await
        .map_err(|cause| MutateMcpError {
            cause: Box::new(cause),
        })
    }

    pub async fn set_default_model(
        &self,
        engine: EngineId,
        model: artisan_api::Model,
    ) -> Result<artisan_api::EngineConfig, SetDefaultModelError> {
        let chosen = model.id.clone();
        self.write_acknowledged(move |tx| {
            let remembered = tx.remembered_model_configs()?;
            let saved = artisan_api::saved_model_config(&remembered, engine, &model.id).cloned();
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

    pub async fn set_model_group(
        &self,
        engine: EngineId,
        group: artisan_api::ModelGroup,
    ) -> Result<(), SetModelGroupError> {
        let name = group.name.clone();
        self.write_acknowledged(move |tx| {
            let mut groups = tx.object_setting(artisan_api::MODEL_GROUPS_KEY)?;
            let entry = groups
                .entry(engine.to_string())
                .or_insert_with(|| serde_json::json!({}));
            if !entry.is_object() {
                tracing::warn!(
                    key = artisan_api::MODEL_GROUPS_KEY,
                    %engine,
                    found = crate::writer::json_kind(entry),
                    "replacing a damaged model group entry that should hold an object"
                );
                *entry = serde_json::json!({});
            }
            entry
                .as_object_mut()
                .expect("invariant: the model group entry was just made an object")
                .insert(group.name.clone(), serde_json::Value::Bool(group.open));
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
            turn_skills(tx, |disabled| {
                if enabled {
                    disabled.remove(&id);
                } else {
                    disabled.insert(id);
                }
            })
        })
        .await
        .map_err(|cause| SetSkillEnabledError {
            skill,
            cause: Box::new(cause),
        })
    }

    pub async fn save_prompt_version(
        &self,
        base: u64,
        text: String,
    ) -> Result<Result<u64, u64>, SavePromptError> {
        self.write_acknowledged_carrying(text.len(), move |tx| {
            let latest = read::prompt_version(tx)?;
            if latest != base {
                return Ok(Err(latest));
            }
            save_prompt(tx, latest, text).map(Ok)
        })
        .await
        .map_err(|cause| SavePromptError {
            base,
            cause: Box::new(cause),
        })
    }

    pub async fn apply_config_bundle(
        &self,
        bundle: ConfigBundle,
    ) -> Result<BundleCommit, ApplyConfigBundleError> {
        let bytes = bundle.prompt.as_ref().map_or(0, |prompt| prompt.text.len())
            + crate::encoded::len(&bundle.record)
            + bundle.mcp.iter().map(crate::encoded::len).sum::<usize>();
        self.write_acknowledged_carrying(bytes, move |tx| {
            let ConfigBundle {
                enable_skills,
                prompt,
                mcp,
                record,
            } = bundle;
            if let Some(prompt) = &prompt {
                let latest = read::prompt_version(tx)?;
                if latest != prompt.base {
                    return Ok(BundleCommit::PromptChanged { latest });
                }
            }
            if !enable_skills.is_empty() {
                turn_skills(tx, |disabled| {
                    for skill in &enable_skills {
                        disabled.remove(skill);
                    }
                })?;
            }
            if let Some(PromptEdit { base, text }) = prompt {
                save_prompt(tx, base, text)?;
            }
            let mut forgotten = Vec::new();
            if !mcp.is_empty() {
                let mut servers: Vec<artisan_api::McpServer> = read::setting(tx, "mcp_servers")?
                    .map(read::from_value)
                    .transpose()?
                    .unwrap_or_default();
                for mutation in mcp {
                    forgotten.extend(apply_mcp_mutation(tx, &mut servers, mutation)?);
                }
                servers.sort_by(|a, b| a.name.cmp(&b.name));
                tx.save_setting("mcp_servers".into(), read::to_value(&servers)?)?;
            }
            tx.save_setting(CONFIG_SYNC_KEY.to_owned(), record)?;
            Ok(BundleCommit::Applied {
                forgotten: ForgottenMcpServers(forgotten),
            })
        })
        .await
        .map_err(|cause| ApplyConfigBundleError {
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

    pub async fn set_draft(
        &self,
        scope: String,
        request: artisan_api::WriteDraft,
    ) -> Result<artisan_api::DraftWriteResult, SetDraftError> {
        let saved = scope.clone();
        let bytes = crate::encoded::len(&request);
        self.write_acknowledged_carrying(bytes, move |tx| {
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
                include_str!("sql/settings/set_draft.sql"),
                params![scope, read::to_body(&current.body)?, current.revision],
            )
            .at("settings/set_draft.sql")?;
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
        let bytes = server
            .len()
            .saturating_add(url.len())
            .saturating_add(value.len());
        self.write_acknowledged_carrying(bytes, move |tx| {
            let servers: Vec<artisan_api::McpServer> = read::setting(tx, "mcp_servers")?
                .map(read::from_value)
                .transpose()?
                .unwrap_or_default();
            let listed = servers
                .iter()
                .any(|listed| listed.name == server && listed.url.as_deref() == Some(url.as_str()));
            if listed {
                crate::secrets::set_secret(tx, &secret.key(&server), Some(&value))?;
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
        self.read(move |conn| crate::secrets::secret(conn, &read_key))
            .await
            .map_err(|cause| ReadSecretError {
                key,
                cause: Box::new(cause),
            })
    }

    pub async fn set_secret(&self, key: &str, value: Option<String>) -> Result<(), SetSecretError> {
        let saved = key.to_owned();
        let key = key.to_owned();
        let bytes = key
            .len()
            .saturating_add(value.as_ref().map_or(0, String::len));
        self.write_acknowledged_carrying(bytes, move |tx| {
            crate::secrets::set_secret(tx, &key, value.as_deref())
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
        let bytes = key.len().saturating_add(value.len());
        self.write_acknowledged_carrying(bytes, move |tx| {
            crate::secrets::secret_or_insert(tx, &key, &value)
        })
        .await
        .map_err(|cause| SetSecretError {
            key: saved,
            cause: Box::new(cause),
        })
    }
}

#[cfg(test)]
#[path = "../tests/settings/context_window.rs"]
mod context_window_tests;

#[derive(Debug)]
#[must_use = "servers whose sign-in was forgotten must have their pending sign-ins ended"]
pub struct McpChange {
    pub servers: Vec<artisan_api::McpServer>,
    pub forgotten: ForgottenMcpServers,
}

#[derive(Debug)]
#[must_use = "servers whose sign-in was forgotten must have their pending sign-ins ended"]
pub struct ForgottenMcpServers(pub(crate) Vec<String>);

impl ForgottenMcpServers {
    pub fn into_names(self) -> Vec<String> {
        self.0
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
    tx.prepare_cached(include_str!("sql/mcp/forget.sql"))
        .at("mcp/forget.sql")?
        .execute(params![
            McpSecret::Tokens.key(server),
            McpSecret::Client.key(server)
        ])
        .at("mcp/forget.sql")?;
    Ok(())
}

fn turn_skills(
    tx: &mut crate::writer::Tx<'_>,
    turn: impl FnOnce(&mut std::collections::BTreeSet<String>),
) -> Result<(), WriteFailure> {
    let mut disabled: std::collections::BTreeSet<String> = read::setting(tx, "disabled_skills")?
        .map(read::from_value)
        .transpose()?
        .unwrap_or_default();
    turn(&mut disabled);
    tx.save_setting("disabled_skills".into(), read::to_value(&disabled)?)
}

fn save_prompt(
    tx: &mut crate::writer::Tx<'_>,
    base: u64,
    text: String,
) -> Result<u64, SettingFailure> {
    let version = base
        .checked_add(1)
        .ok_or(SettingFailure::RevisionExhausted)?;
    let stored = i64::try_from(version)
        .map_err(|source| SettingFailure::PromptVersion { version, source })?;
    tx.execute(
        include_str!("sql/settings/save_prompt_version.sql"),
        params![stored, text, tx.now()],
    )
    .at("settings/save_prompt_version.sql")?;
    tx.save_setting("global_prompt".to_owned(), serde_json::Value::String(text))?;
    tx.save_setting("global_prompt_version".into(), serde_json::json!(version))?;
    Ok(version)
}

fn apply_mcp_mutation(
    tx: &crate::writer::Tx<'_>,
    servers: &mut Vec<artisan_api::McpServer>,
    mutation: artisan_api::McpMutation,
) -> Result<Option<String>, SettingFailure> {
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
    let forgotten = match previous {
        Some(previous) => {
            servers.retain(|server| server.name != previous.name);
            let kept = match &mutation {
                McpMutation::Upsert { server, .. } => {
                    server.name == previous.name && server.url == previous.url
                }
                McpMutation::Remove { .. } => false,
            };
            if kept {
                None
            } else {
                forget_mcp_secrets(tx, &previous.name)?;
                Some(previous.name.clone())
            }
        }
        None => None,
    };
    if let McpMutation::Upsert { server, .. } = mutation {
        servers.push(server);
    }
    Ok(forgotten)
}
