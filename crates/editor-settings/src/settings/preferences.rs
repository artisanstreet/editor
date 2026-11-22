use editor_core::client::{CallError, SettingAck};
use editor_core::{HostData, HostId, HostInstance, Restaged, Staging};
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
    pub(crate) fn problem(&self, headline: &str) -> editor_core::Problem {
        match self {
            Self::Send { source, .. } => editor_core::Problem::calling_forge(headline, source),
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
    staging: Staging,
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
        host: &mut HostData,
        target: SettingKey,
        value: Value,
    ) -> Option<SettingRequest> {
        self.failures.remove(&target);
        if let Some(write) = self.keys.get_mut(&target) {
            match write.staging.restage(host, value.clone()) {
                Restaged::Shown => {}
                Restaged::Superseded => {
                    write.staging = host.stage_setting(target.key, value.clone());
                }
            }
            write.next = Some(value);
            return None;
        }
        let staging = host.stage_setting(target.key, value.clone());
        self.keys.insert(
            target.clone(),
            KeyWrite {
                staging,
                next: None,
            },
        );
        Some(SettingRequest { target, value })
    }

    pub fn complete(
        &mut self,
        mut host: Option<&mut HostData>,
        request: &SettingRequest,
        result: Result<SettingAck, SaveSettingError>,
    ) -> Option<SettingRequest> {
        let KeyWrite { staging, next } = self.keys.remove(&request.target)?;
        if let (Ok(ack), Some(host)) = (&result, host.as_deref_mut()) {
            staging.accepted(host, request.value.clone(), *ack);
        }
        if let (Some(next), Some(host)) = (&next, host.as_deref_mut()) {
            staging.dispatched(host, next.clone());
        }
        match (result, next) {
            (Ok(_), Some(next)) => Some(self.send_next(request, staging, next)),
            (Err(superseded), Some(next)) => {
                tracing::debug!(
                    host = %request.target.host,
                    key = request.target.key,
                    error = %::diagnostics::chain(&superseded),
                    "a newer choice replaces a setting write Forge refused"
                );
                Some(self.send_next(request, staging, next))
            }
            (Ok(_), None) => {
                self.failures.remove(&request.target);
                Self::release(host, staging);
                None
            }
            (Err(cause), None) => {
                tracing::warn!(
                    host = %request.target.host,
                    key = request.target.key,
                    error = %::diagnostics::chain(&cause),
                    "saving a Forge setting"
                );
                self.failures.insert(
                    request.target.clone(),
                    SettingFailure {
                        value: request.value.clone(),
                        cause: Arc::new(cause),
                    },
                );
                Self::release(host, staging);
                None
            }
        }
    }

    fn release(host: Option<&mut HostData>, staging: Staging) {
        match host {
            Some(host) => staging.release(host),
            None => drop(staging),
        }
    }

    fn send_next(
        &mut self,
        request: &SettingRequest,
        staging: Staging,
        next: Value,
    ) -> SettingRequest {
        self.keys.insert(
            request.target.clone(),
            KeyWrite {
                staging,
                next: None,
            },
        );
        SettingRequest {
            target: request.target.clone(),
            value: next,
        }
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
