use crate::core::hosts::HostId;
use artisan_api::{
    EngineConfig, EngineId, MCP_AUTH_KEY, MODEL_CONFIGS_KEY, MODEL_CUSTOMIZATIONS_KEY,
    MODEL_GROUPS_KEY, McpAuthStatus, McpServer, ModelCustomization, ModelCustomizationsError,
};
use serde::Deserialize as _;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

pub const MCP_SERVERS_KEY: &str = "mcp_servers";
pub const DEFAULT_ENGINE_CONFIG_KEY: &str = "default_engine_config";

#[derive(Debug, thiserror::Error)]
#[error("read the MCP servers {host} holds")]
pub struct McpServersError {
    pub host: HostId,
    #[source]
    pub source: serde_json::Error,
}

#[derive(Debug, thiserror::Error)]
pub enum HostSettingError {
    #[error("read the {key} setting {host} holds")]
    Decode {
        host: HostId,
        key: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("read the model customizations {host} holds")]
    Customizations {
        host: HostId,
        #[source]
        source: ModelCustomizationsError,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum HostSwitch {
    RetryWhenOverloaded,
    ContinueAfterUsageLimit,
    ContinueAfterRestart,
    Suggestions,
    YouShouldKnow,
    SummaryTitles,
}

impl HostSwitch {
    const ALL: [Self; 6] = [
        Self::RetryWhenOverloaded,
        Self::ContinueAfterUsageLimit,
        Self::ContinueAfterRestart,
        Self::Suggestions,
        Self::YouShouldKnow,
        Self::SummaryTitles,
    ];

    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::RetryWhenOverloaded => "retry_when_overloaded",
            Self::ContinueAfterUsageLimit => "continue_after_usage_limit",
            Self::ContinueAfterRestart => "continue_after_restart",
            Self::Suggestions => "suggestions",
            Self::YouShouldKnow => "you_should_know",
            Self::SummaryTitles => "summary_titles",
        }
    }

    fn of_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|switch| switch.key() == key)
    }

    fn declared(self) -> bool {
        artisan_api::setting_decl(self.key())
            .and_then(|decl| decl.default_value().as_bool())
            .unwrap_or_else(|| {
                unreachable!(
                    "invariant: host switch {} is a declared boolean setting",
                    self.key()
                )
            })
    }
}

#[derive(Debug)]
pub struct HostSettings {
    host: HostId,
    values: Map<String, Value>,
    mcp_servers: Result<Vec<McpServer>, Arc<McpServersError>>,
    mcp_auth: Result<BTreeMap<String, McpAuthStatus>, Arc<HostSettingError>>,
    default_engine_config: Option<EngineConfig>,
    model_configs: Vec<(String, EngineConfig)>,
    model_customizations: Vec<ModelCustomization>,
    model_groups: BTreeMap<String, BTreeMap<String, bool>>,
    switches: BTreeMap<HostSwitch, bool>,
}

impl HostSettings {
    #[must_use]
    pub fn new(host: HostId) -> Self {
        Self::from_values(host, Map::new())
    }

    #[must_use]
    pub fn from_values(host: HostId, values: Map<String, Value>) -> Self {
        let mut settings = Self {
            host,
            values,
            mcp_servers: Ok(Vec::new()),
            mcp_auth: Ok(BTreeMap::new()),
            default_engine_config: None,
            model_configs: Vec::new(),
            model_customizations: Vec::new(),
            model_groups: BTreeMap::new(),
            switches: BTreeMap::new(),
        };
        for switch in HostSwitch::ALL {
            settings.decode(switch.key());
        }
        for key in [
            MCP_SERVERS_KEY,
            MCP_AUTH_KEY,
            DEFAULT_ENGINE_CONFIG_KEY,
            MODEL_CONFIGS_KEY,
            MODEL_CUSTOMIZATIONS_KEY,
            MODEL_GROUPS_KEY,
        ] {
            settings.decode(key);
        }
        settings
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    pub fn insert(&mut self, key: &str, value: Value) -> Option<Value> {
        let previous = self.values.insert(key.to_owned(), value);
        self.decode(key);
        previous
    }

    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let previous = self.values.remove(key);
        self.decode(key);
        previous
    }

    pub fn mcp_servers(&self) -> Result<&[McpServer], &McpServersError> {
        match &self.mcp_servers {
            Ok(servers) => Ok(servers),
            Err(error) => Err(error),
        }
    }

    pub fn mcp_auth(&self) -> Result<&BTreeMap<String, McpAuthStatus>, &HostSettingError> {
        match &self.mcp_auth {
            Ok(auth) => Ok(auth),
            Err(error) => Err(error),
        }
    }

    #[must_use]
    pub fn default_engine_config(&self) -> Option<&EngineConfig> {
        self.default_engine_config.as_ref()
    }

    #[must_use]
    pub fn saved_model_config(&self, engine: EngineId, model: &str) -> Option<&EngineConfig> {
        let named = artisan_api::model_config_key(engine, model);
        let matches = |config: &&EngineConfig| {
            config.engine == engine && config.model.as_deref() == Some(model)
        };
        self.model_configs
            .iter()
            .find(|(key, _)| *key == named)
            .map(|(_, config)| config)
            .filter(matches)
            .or_else(|| {
                self.model_configs
                    .iter()
                    .map(|(_, config)| config)
                    .find(matches)
            })
    }

    #[must_use]
    pub fn model_customizations(&self) -> &[ModelCustomization] {
        &self.model_customizations
    }

    #[must_use]
    pub fn model_group_open(&self, engine: EngineId, group: &str) -> Option<bool> {
        self.model_groups.get(engine.as_str())?.get(group).copied()
    }

    #[must_use]
    pub fn switch(&self, switch: HostSwitch) -> bool {
        self.switches
            .get(&switch)
            .copied()
            .unwrap_or_else(|| switch.declared())
    }

    fn decode(&mut self, key: &str) {
        if let Some(switch) = HostSwitch::of_key(key) {
            let on = self
                .decoded::<bool>(switch.key())
                .unwrap_or_else(|error| {
                    self.damaged(&error);
                    None
                })
                .unwrap_or_else(|| switch.declared());
            self.switches.insert(switch, on);
            return;
        }
        let value = self.values.get(key);
        match key {
            MCP_SERVERS_KEY => {
                self.mcp_servers = match value {
                    None => Ok(Vec::new()),
                    Some(value) => Vec::<McpServer>::deserialize(value).map_err(|source| {
                        let error = McpServersError {
                            host: self.host.clone(),
                            source,
                        };
                        self.damaged(&error);
                        Arc::new(error)
                    }),
                };
            }
            MCP_AUTH_KEY => {
                self.mcp_auth = self
                    .decoded(MCP_AUTH_KEY)
                    .map(Option::unwrap_or_default)
                    .map_err(|error| {
                        self.damaged(&error);
                        Arc::new(error)
                    });
            }
            DEFAULT_ENGINE_CONFIG_KEY => {
                self.default_engine_config = self.lenient(DEFAULT_ENGINE_CONFIG_KEY);
            }
            MODEL_CONFIGS_KEY => {
                self.model_configs = self.model_configs();
            }
            MODEL_CUSTOMIZATIONS_KEY => {
                self.model_customizations = match value {
                    None => Vec::new(),
                    Some(value) => {
                        artisan_api::model_customizations(value).unwrap_or_else(|source| {
                            self.damaged(&HostSettingError::Customizations {
                                host: self.host.clone(),
                                source,
                            });
                            Vec::new()
                        })
                    }
                };
            }
            MODEL_GROUPS_KEY => {
                self.model_groups = self.lenient(MODEL_GROUPS_KEY).unwrap_or_default();
            }
            _ => {}
        }
    }

    fn decoded<T: serde::de::DeserializeOwned>(
        &self,
        key: &'static str,
    ) -> Result<Option<T>, HostSettingError> {
        self.values
            .get(key)
            .map(|value| {
                T::deserialize(value).map_err(|source| HostSettingError::Decode {
                    host: self.host.clone(),
                    key,
                    source,
                })
            })
            .transpose()
    }

    fn lenient<T: serde::de::DeserializeOwned>(&self, key: &'static str) -> Option<T> {
        self.decoded(key).unwrap_or_else(|error| {
            self.damaged(&error);
            None
        })
    }

    fn model_configs(&self) -> Vec<(String, EngineConfig)> {
        let Some(entries) = self.lenient::<Map<String, Value>>(MODEL_CONFIGS_KEY) else {
            return Vec::new();
        };
        entries
            .into_iter()
            .filter_map(|(name, value)| match EngineConfig::deserialize(&value) {
                Ok(config) => Some((name, config)),
                Err(source) => {
                    self.damaged(&HostSettingError::Decode {
                        host: self.host.clone(),
                        key: MODEL_CONFIGS_KEY,
                        source,
                    });
                    None
                }
            })
            .collect()
    }

    fn damaged(&self, error: &dyn std::error::Error) {
        tracing::warn!(
            host = %self.host,
            error = %::diagnostics::chain(error),
            "Forge holds a setting the Editor cannot read"
        );
    }
}
