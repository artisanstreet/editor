use crate::core::hosts::HostId;
use artisan_api::{
    EngineConfig, EngineId, MCP_AUTH_KEY, MODEL_CONFIGS_KEY, MODEL_CUSTOMIZATIONS_KEY,
    MODEL_GROUPS_KEY, McpAuthStatus, McpServer, ModelCustomization, ModelCustomizationsError, Seq,
};
use serde::Deserialize as _;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, VecDeque};
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
    ArtisanSystemPrompt,
}

impl HostSwitch {
    const ALL: [Self; 7] = [
        Self::RetryWhenOverloaded,
        Self::ContinueAfterUsageLimit,
        Self::ContinueAfterRestart,
        Self::Suggestions,
        Self::YouShouldKnow,
        Self::SummaryTitles,
        Self::ArtisanSystemPrompt,
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
            Self::ArtisanSystemPrompt => "artisan_system_prompt",
        }
    }

    fn of_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|switch| switch.key() == key)
    }

    fn declared(self) -> bool {
        artisan_api::switch_decl(self.key()).switch(None).on()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StageId(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Snapshots(u64);

#[derive(Debug)]
struct Flight {
    value: Value,
    snapshots: Snapshots,
    echoed: bool,
}

#[derive(Debug)]
enum Staged {
    Held {
        stage: StageId,
        shown: Value,
        flight: Flight,
        echoes: VecDeque<Value>,
    },
    Echoing {
        echoes: VecDeque<Value>,
    },
}

impl Staged {
    fn shown(&self) -> &Value {
        match self {
            Self::Held { shown, .. } => shown,
            Self::Echoing { echoes } => echoes
                .back()
                .expect("invariant: a released setting stays staged only while echoes are due"),
        }
    }

    fn held_by(&mut self, token: StageId) -> Option<(&mut Flight, &mut VecDeque<Value>)> {
        match self {
            Self::Held {
                stage,
                flight,
                echoes,
                ..
            } if *stage == token => Some((flight, echoes)),
            Self::Held { .. } | Self::Echoing { .. } => None,
        }
    }

    fn into_echoes(self) -> VecDeque<Value> {
        match self {
            Self::Held { echoes, .. } | Self::Echoing { echoes } => echoes,
        }
    }
}

#[must_use = "a superseded staging token no longer shows its value"]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Restaged {
    Shown,
    Superseded,
}

#[must_use = "a staged setting stays on screen until its token is released"]
pub struct Staging {
    sender: crate::core::delivery::MappedSender<super::HostCompletion, super::HostMessage>,
    host: HostId,
    instance: super::HostInstance,
    held: Option<(String, StageId)>,
}

impl Staging {
    pub fn restage(&self, host: &mut super::HostData, value: Value) -> Restaged {
        self.check(host);
        let (key, stage) = self.held();
        host.settings.restage(key, *stage, value)
    }

    pub fn accepted(
        &self,
        host: &mut super::HostData,
        value: Value,
        ack: crate::core::client::SettingAck,
    ) {
        self.check(host);
        let (key, stage) = self.held();
        host.settings.accepted(key, *stage, value, ack);
    }

    pub fn dispatched(&self, host: &mut super::HostData, value: Value) {
        self.check(host);
        let (key, stage) = self.held();
        host.settings.dispatched(key, *stage, value);
    }

    pub fn release(mut self, host: &mut super::HostData) {
        self.check(host);
        let (key, stage) = self
            .held
            .take()
            .expect("invariant: a staging token holds its setting until released");
        host.settings.release(&key, stage);
    }

    fn check(&self, host: &super::HostData) {
        assert!(
            host.id == self.host && host.instance() == self.instance,
            "invariant: setting staged on {} {:?} is changed only through that host, not {} {:?}",
            self.host,
            self.instance,
            host.id,
            host.instance()
        );
    }

    fn held(&self) -> &(String, StageId) {
        self.held
            .as_ref()
            .expect("invariant: a staging token holds its setting until released")
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if let Some((key, stage)) = self.held.take() {
            let sender = self.sender.clone();
            crate::core::runtime::runtime().spawn(async move {
                sender
                    .offer(super::HostCompletion::SettingReleased(key, stage))
                    .await;
            });
        }
    }
}

impl std::fmt::Debug for Staging {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Staging")
            .field("host", &self.host)
            .field("instance", &self.instance)
            .field("held", &self.held)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct HostSettings {
    host: HostId,
    forge: Map<String, Value>,
    revisions: BTreeMap<String, Seq>,
    snapshot: Seq,
    staged: BTreeMap<String, Staged>,
    stages: u64,
    snapshots: Snapshots,
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
            forge: values,
            revisions: BTreeMap::new(),
            snapshot: 0,
            staged: BTreeMap::new(),
            stages: 0,
            snapshots: Snapshots(0),
            mcp_servers: Ok(Vec::new()),
            mcp_auth: Ok(BTreeMap::new()),
            default_engine_config: None,
            model_configs: Vec::new(),
            model_customizations: Vec::new(),
            model_groups: BTreeMap::new(),
            switches: BTreeMap::new(),
        };
        settings.decode_all();
        settings
    }

    pub fn replace(&mut self, mut values: Map<String, Value>, head: Seq) {
        self.revisions.retain(|_, revision| *revision > head);
        for key in self.revisions.keys() {
            match self.forge.get(key) {
                Some(value) => values.insert(key.clone(), value.clone()),
                None => values.remove(key),
            };
        }
        self.snapshot = head;
        self.forge = values;
        self.snapshots = Snapshots(self.snapshots.0 + 1);
        self.staged.retain(|_, staged| match staged {
            Staged::Held { echoes, .. } => {
                echoes.clear();
                true
            }
            Staged::Echoing { .. } => false,
        });
        self.decode_all();
    }

    #[must_use]
    pub(crate) fn get(&self, key: &str) -> Option<&Value> {
        self.staged
            .get(key)
            .map(Staged::shown)
            .or_else(|| self.forge.get(key))
    }

    fn known(&self, key: &str) -> Seq {
        self.revisions.get(key).copied().unwrap_or(self.snapshot)
    }

    fn accepted(
        &mut self,
        key: &str,
        stage: StageId,
        value: Value,
        ack: crate::core::client::SettingAck,
    ) {
        match ack {
            crate::core::client::SettingAck::Revision(revision) => {
                self.insert(key, value, revision);
            }
            crate::core::client::SettingAck::Unversioned => {
                self.unversioned(key, stage, value);
            }
        }
    }

    fn unversioned(&mut self, key: &str, stage: StageId, value: Value) {
        let snapshots = self.snapshots;
        let current = self.forge.get(key) == Some(&value);
        let Some((flight, echoes)) = self
            .staged
            .get_mut(key)
            .and_then(|staged| staged.held_by(stage))
        else {
            tracing::debug!(
                host = %self.host,
                key,
                "an unversioned save answer arrived for a superseded staging"
            );
            return;
        };
        if flight.snapshots != snapshots {
            tracing::debug!(
                host = %self.host,
                key,
                "an unversioned save answer arrived after a resnapshot, which already holds it"
            );
            return;
        }
        if flight.echoed || (current && echoes.is_empty()) {
            return;
        }
        echoes.push_back(value);
    }

    fn dispatched(&mut self, key: &str, stage: StageId, value: Value) {
        let snapshots = self.snapshots;
        if let Some((flight, _)) = self
            .staged
            .get_mut(key)
            .and_then(|staged| staged.held_by(stage))
        {
            *flight = Flight {
                value,
                snapshots,
                echoed: false,
            };
        }
    }

    pub(crate) fn streamed(&mut self, key: &str, value: Value, revision: Seq) {
        match self.staged.get_mut(key) {
            Some(Staged::Held { flight, echoes, .. }) => {
                if echoes.front() == Some(&value) {
                    echoes.pop_front();
                } else if flight.value == value {
                    flight.echoed = true;
                }
            }
            Some(Staged::Echoing { echoes }) => {
                if echoes.front() == Some(&value) {
                    echoes.pop_front();
                }
                if echoes.is_empty() {
                    self.staged.remove(key);
                }
            }
            None => {}
        }
        self.insert(key, value, revision);
        self.decode(key);
    }

    fn insert(&mut self, key: &str, value: Value, revision: Seq) {
        let known = self.known(key);
        if revision <= known {
            return;
        }
        self.revisions.insert(key.to_owned(), revision);
        self.forge.insert(key.to_owned(), value);
        self.decode(key);
    }

    fn stage(&mut self, key: &str, value: Value) -> StageId {
        self.stages += 1;
        let stage = StageId(self.stages);
        let echoes = self
            .staged
            .remove(key)
            .map(Staged::into_echoes)
            .unwrap_or_default();
        self.staged.insert(
            key.to_owned(),
            Staged::Held {
                stage,
                shown: value.clone(),
                flight: Flight {
                    value,
                    snapshots: self.snapshots,
                    echoed: false,
                },
                echoes,
            },
        );
        self.decode(key);
        stage
    }

    fn restage(&mut self, key: &str, stage: StageId, value: Value) -> Restaged {
        match self.staged.get_mut(key) {
            Some(Staged::Held {
                stage: held, shown, ..
            }) if *held == stage => {
                *shown = value;
                self.decode(key);
                Restaged::Shown
            }
            Some(Staged::Held { .. } | Staged::Echoing { .. }) | None => Restaged::Superseded,
        }
    }

    pub(crate) fn release(&mut self, key: &str, stage: StageId) -> bool {
        let held = self
            .staged
            .get_mut(key)
            .is_some_and(|staged| staged.held_by(stage).is_some());
        if held {
            let echoes = self
                .staged
                .remove(key)
                .map(Staged::into_echoes)
                .unwrap_or_default();
            if !echoes.is_empty() {
                self.staged
                    .insert(key.to_owned(), Staged::Echoing { echoes });
            }
            self.decode(key);
        }
        held
    }

    pub(super) fn staging(
        &mut self,
        sender: crate::core::delivery::MappedSender<super::HostCompletion, super::HostMessage>,
        instance: super::HostInstance,
        key: &str,
        value: Value,
    ) -> Staging {
        let stage = self.stage(key, value);
        Staging {
            sender,
            host: self.host.clone(),
            instance,
            held: Some((key.to_owned(), stage)),
        }
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

    fn decode_all(&mut self) {
        for switch in HostSwitch::ALL {
            self.decode(switch.key());
        }
        for key in [
            MCP_SERVERS_KEY,
            MCP_AUTH_KEY,
            DEFAULT_ENGINE_CONFIG_KEY,
            MODEL_CONFIGS_KEY,
            MODEL_CUSTOMIZATIONS_KEY,
            MODEL_GROUPS_KEY,
        ] {
            self.decode(key);
        }
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
        let value = self.get(key);
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
        self.get(key)
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
