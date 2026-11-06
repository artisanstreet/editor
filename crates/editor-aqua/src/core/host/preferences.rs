use super::{HostCompletion, HostData, HostMessage};
use artisan_api::{EngineConfig, EngineId, ModelGroup};
use std::collections::VecDeque;
use tokio_util::task::AbortOnDropHandle;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::core) enum Choice {
    ModelConfig { engine: EngineId, model: String },
    ModelGroup { engine: EngineId, name: String },
}
#[derive(Clone, Debug, PartialEq)]
pub(in crate::core) enum Preference {
    ModelConfig(EngineConfig),
    ModelGroup { engine: EngineId, group: ModelGroup },
}
#[derive(Default)]
pub(super) struct Preferences {
    pending: VecDeque<(Choice, Preference)>,
    writer: Option<AbortOnDropHandle<()>>,
    generation: u64,
}

impl HostData {
    pub fn remember_model_config(&mut self, config: EngineConfig) {
        let Some(model) = config.model.clone() else {
            return;
        };
        let choice = Choice::ModelConfig {
            engine: config.engine,
            model,
        };
        self.remember_preference(choice, Preference::ModelConfig(config));
    }
    pub fn pending_harness_config(&self, engine: EngineId) -> Option<&EngineConfig> {
        self.preferences
            .pending
            .iter()
            .rev()
            .find_map(|(_, preference)| match preference {
                Preference::ModelConfig(config) if config.engine == engine => Some(config),
                _ => None,
            })
    }
    pub fn remember_model_group(&mut self, engine: EngineId, name: String, open: bool) {
        let choice = Choice::ModelGroup {
            engine,
            name: name.clone(),
        };
        self.remember_preference(
            choice,
            Preference::ModelGroup {
                engine,
                group: ModelGroup { name, open },
            },
        );
    }
    fn remember_preference(&mut self, choice: Choice, preference: Preference) {
        self.preferences.pending.retain(|(key, _)| key != &choice);
        self.preferences.pending.push_back((choice, preference));
        self.pump_preferences();
    }
    fn pump_preferences(&mut self) {
        if self.preferences.writer.is_some() {
            return;
        }
        let Some((choice, preference)) = self.preferences.pending.front().cloned() else {
            return;
        };
        self.preferences.generation = self.preferences.generation.wrapping_add(1);
        let generation = self.preferences.generation;
        let client = self.client.clone();
        let sender = self.delivery.sender().map(HostMessage::Completion);
        let task = crate::core::runtime::runtime().spawn(async move {
            let result = match &preference {
                Preference::ModelConfig(config) => client.set_model_config(config).await.map(drop),
                Preference::ModelGroup { engine, group } => {
                    client.set_model_group(*engine, group).await
                }
            };
            let _ = sender
                .send(HostCompletion::Preference(
                    generation, choice, preference, result,
                ))
                .await;
        });
        self.preferences.writer = Some(AbortOnDropHandle::new(task));
    }
    pub(super) fn finish_preference(
        &mut self,
        generation: u64,
        choice: Choice,
        preference: Preference,
        result: anyhow::Result<()>,
    ) {
        if self.preferences.generation != generation {
            return;
        }
        self.preferences.writer = None;
        if let Err(error) = result {
            tracing::warn!(host = %self.id, ?choice, "Forge didn't remember the picker choice: {error:#}");
        }
        if self.preferences.pending.front() == Some(&(choice, preference)) {
            self.preferences.pending.pop_front();
        }
        self.pump_preferences();
    }
}
