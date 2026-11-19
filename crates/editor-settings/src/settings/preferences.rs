use editor_core::{HostId, HostInstance, HostSettings};
use serde_json::Value;
use std::collections::HashMap;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingFailure {
    pub(crate) value: Value,
    pub(crate) cause: String,
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
        result: &Result<(), String>,
    ) -> Option<SettingRequest> {
        let write = self.keys.remove(&request.target)?;
        match result {
            Ok(()) => {
                self.failures.remove(&request.target);
            }
            Err(cause) if write.next.is_none() => {
                self.failures.insert(
                    request.target.clone(),
                    SettingFailure {
                        value: request.value.clone(),
                        cause: cause.clone(),
                    },
                );
            }
            Err(_) => {}
        }
        let confirmed = match result {
            Ok(()) => Some(write.sent.clone()),
            Err(_) => write.confirmed,
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
        if result.is_err()
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
