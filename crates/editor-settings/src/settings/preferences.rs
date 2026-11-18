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
    pub target: SettingKey,
    pub value: Value,
}

struct KeyWrite {
    sent: Value,
    confirmed: Option<Value>,
    next: Option<Value>,
}

#[derive(Default)]
pub struct SettingWrites {
    keys: HashMap<SettingKey, KeyWrite>,
}

impl SettingWrites {
    pub fn begin(
        &mut self,
        settings: &mut HostSettings,
        target: SettingKey,
        value: Value,
    ) -> Option<SettingRequest> {
        let previous = settings.insert(target.key.to_owned(), value.clone());
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
                    settings.insert(request.target.key.to_owned(), value);
                }
                None => {
                    settings.remove(request.target.key);
                }
            }
        }
        None
    }

    pub fn retain_instances(&mut self, live: impl Fn(&HostId, HostInstance) -> bool) {
        self.keys
            .retain(|target, _| live(&target.host, target.instance));
    }
}
