use editor_core::{HostId, HostInstance, HostSettings, client::CallError};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SettingKey {
    pub host: HostId,
    pub instance: HostInstance,
    pub key: &'static str,
}

#[derive(Clone, Debug)]
pub struct SettingRequest {
    pub(crate) target: SettingKey,
    pub(crate) value: Value,
}

#[derive(Debug, thiserror::Error)]
pub enum SaveSettingError {
    #[error("send setting {key} to {host}")]
    Send {
        host: HostId,
        key: &'static str,
        #[source]
        source: CallError,
    },
}

impl SaveSettingError {
    pub(crate) fn reason(&self) -> String {
        match self {
            Self::Send { source, .. } => ::diagnostics::chain(source),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SettingFailure {
    pub(crate) value: Value,
    pub(crate) cause: Arc<SaveSettingError>,
}

impl PartialEq for SettingFailure {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value && Arc::ptr_eq(&self.cause, &other.cause)
    }
}

#[derive(Debug)]
struct KeyWrite {
    sent: Value,
    confirmed: Option<Value>,
    next: Option<Value>,
}

#[derive(Debug, Default)]
pub struct SettingWrites {
    keys: HashMap<SettingKey, KeyWrite>,
    failures: HashMap<SettingKey, SettingFailure>,
}

impl SettingWrites {
    pub fn begin(
        &mut self,
        settings: &mut HostSettings,
        target: SettingKey,
        value: Value,
    ) -> Option<SettingRequest> {
        self.failures.remove(&target);
        let previous = settings.insert(target.key, value.clone());
        if let Some(write) = self.keys.get_mut(&target) {
            write.next = Some(value);
            return None;
        }
        self.keys.insert(
            target.clone(),
            KeyWrite {
                sent: value.clone(),
                confirmed: previous,
                next: None,
            },
        );
        Some(SettingRequest { target, value })
    }

    pub fn complete(
        &mut self,
        settings: Option<&mut HostSettings>,
        request: &SettingRequest,
        result: Result<(), SaveSettingError>,
    ) -> Option<SettingRequest> {
        let write = self.keys.remove(&request.target)?;
        let saved = result.is_ok();
        match (result, &write.next) {
            (Ok(()), _) => {
                self.failures.remove(&request.target);
            }
            (Err(cause), None) => {
                self.failures.insert(
                    request.target.clone(),
                    SettingFailure {
                        value: request.value.clone(),
                        cause: Arc::new(cause),
                    },
                );
            }
            (Err(_superseded_by_the_next_write), Some(_)) => {}
        }
        let confirmed = if saved {
            Some(write.sent.clone())
        } else {
            write.confirmed
        };
        if let Some(next) = write.next {
            self.keys.insert(
                request.target.clone(),
                KeyWrite {
                    sent: next.clone(),
                    confirmed,
                    next: None,
                },
            );
            return Some(SettingRequest {
                target: request.target.clone(),
                value: next,
            });
        }
        if !saved
            && let Some(settings) = settings
            && settings.get(request.target.key) == Some(&write.sent)
        {
            match confirmed {
                Some(value) => {
                    settings.insert(request.target.key, value);
                }
                None => {
                    settings.remove(request.target.key);
                }
            }
        }
        None
    }

    pub(crate) fn retain_instances(&mut self, live: impl Fn(&HostId, HostInstance) -> bool) {
        self.keys
            .retain(|target, _| live(&target.host, target.instance));
        self.failures
            .retain(|target, _| live(&target.host, target.instance));
    }

    pub(crate) fn failure(&self, target: &SettingKey) -> Option<&SettingFailure> {
        self.failures.get(target)
    }

    pub(crate) fn failures_for(
        &self,
        host: &HostId,
        instance: HostInstance,
    ) -> Vec<(SettingKey, SettingFailure)> {
        let mut failures: Vec<_> = self
            .failures
            .iter()
            .filter(|(target, _)| &target.host == host && target.instance == instance)
            .map(|(target, failure)| (target.clone(), failure.clone()))
            .collect();
        failures.sort_by_key(|(target, _)| target.key);
        failures
    }

    pub(crate) fn dismiss(&mut self, target: &SettingKey) -> bool {
        self.failures.remove(target).is_some()
    }
}
