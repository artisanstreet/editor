//! Picker choices Forge remembers for every Editor of the host: the config
//! last chosen for each model and which model groups are open. One writer
//! sends them in turn, the newest value per choice, so a slower earlier
//! request never lands over a later one.
use super::HostData;
use artisan_api::{EngineConfig, EngineId, ModelGroup};
use gpui::{Context, Task};
use std::collections::VecDeque;

/// What one remembered choice is about; a newer value replaces the unsent one.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Choice {
    ModelConfig { engine: EngineId, model: String },
    ModelGroup { engine: EngineId, name: String },
}

#[derive(Clone, Debug, PartialEq)]
enum Preference {
    ModelConfig(EngineConfig),
    ModelGroup { engine: EngineId, group: ModelGroup },
}

#[derive(Default)]
pub(super) struct Preferences {
    pending: VecDeque<(Choice, Preference)>,
    writer: Option<Task<()>>,
}

impl HostData {
    /// Forge remembers `config` as the one its model comes back with.
    pub fn remember_model_config(&mut self, config: EngineConfig, cx: &Context<Self>) {
        let Some(model) = config.model.clone() else {
            return;
        };
        let choice = Choice::ModelConfig {
            engine: config.engine,
            model,
        };
        self.remember(choice, Preference::ModelConfig(config), cx);
    }

    /// The newest choice while Forge is still saving it.
    pub fn pending_harness_config(&self, engine: EngineId) -> Option<&EngineConfig> {
        self.preferences
            .pending
            .iter()
            .rev()
            .find_map(|(_, preference)| match preference {
                Preference::ModelConfig(config) if config.engine == engine => Some(config),
                Preference::ModelConfig(_) | Preference::ModelGroup { .. } => None,
            })
    }

    /// Forge remembers whether one of `engine`'s model groups is open.
    pub fn remember_model_group(
        &mut self,
        engine: EngineId,
        name: String,
        open: bool,
        cx: &Context<Self>,
    ) {
        let choice = Choice::ModelGroup {
            engine,
            name: name.clone(),
        };
        let group = ModelGroup { name, open };
        self.remember(choice, Preference::ModelGroup { engine, group }, cx);
    }

    fn remember(&mut self, choice: Choice, preference: Preference, cx: &Context<Self>) {
        // Keep choices in user order: each write also updates the harness's
        // last model, so sorting by model id would remember the wrong one.
        self.preferences.pending.retain(|(key, _)| key != &choice);
        self.preferences.pending.push_back((choice, preference));
        if self.preferences.writer.is_some() {
            return;
        }
        let client = self.client.clone();
        self.preferences.writer = Some(cx.spawn(async move |this, cx| {
            loop {
                let next = this.update(cx, |host, _| {
                    let next = host
                        .preferences
                        .pending
                        .front()
                        .map(|(choice, preference)| (choice.clone(), preference.clone()));
                    if next.is_none() {
                        host.preferences.writer = None;
                    }
                    next
                });
                let Ok(Some((choice, preference))) = next else {
                    break;
                };
                let result = match &preference {
                    Preference::ModelConfig(config) => {
                        client.set_model_config(config).await.map(drop)
                    }
                    Preference::ModelGroup { engine, group } => {
                        client.set_model_group(*engine, group).await
                    }
                };
                let host = this.update(cx, |host, _| {
                    if let Err(error) = result {
                        tracing::warn!(
                            host = %host.id,
                            ?choice,
                            "Forge didn't remember the picker choice: {error:#}"
                        );
                    }
                    // A newer value chosen meanwhile is still to send.
                    if host.preferences.pending.front() == Some(&(choice, preference)) {
                        host.preferences.pending.pop_front();
                    }
                });
                if host.is_err() {
                    break;
                }
            }
        }));
    }
}
