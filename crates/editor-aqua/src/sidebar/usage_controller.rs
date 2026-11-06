use crate::core::{Core, HostId, HostInstance, Screen, delivery::Delivery, runtime::runtime};
use artisan_api::{EngineCommand, EngineId, SignInState};
use std::{
    collections::{HashMap, HashSet},
    num::NonZeroUsize,
};

pub const CLOCK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

struct Completion {
    host: HostId,
    instance: HostInstance,
    engine: EngineId,
    result: anyhow::Result<()>,
}

pub struct UsagePage {
    shown: bool,
    refreshing: HashSet<(HostId, HostInstance, EngineId)>,
    errors: HashMap<(HostId, HostInstance), String>,
    completions: Delivery<Completion>,
}

impl Default for UsagePage {
    fn default() -> Self {
        Self {
            shown: false,
            refreshing: HashSet::new(),
            errors: HashMap::new(),
            completions: Delivery::new(NonZeroUsize::new(64).unwrap()),
        }
    }
}

impl UsagePage {
    pub fn wake_mailbox(&self) -> Option<&aqua::mailbox::Mailbox<()>> {
        self.completions.wake_mailbox()
    }
    pub fn sync(&mut self, core: &Core) {
        self.refreshing
            .retain(|(host, instance, _)| core.host_if_instance(host, *instance).is_some());
        self.errors
            .retain(|(host, instance), _| core.host_if_instance(host, *instance).is_some());
        let shown = matches!(core.screen(), Screen::Usage);
        if shown && !self.shown {
            let hosts: Vec<_> = core.hosts().map(|(_, host)| host.id.clone()).collect();
            for host in hosts {
                self.refresh_host(core, &host);
            }
        }
        self.shown = shown;
    }
    pub fn refresh_host(&mut self, core: &Core, id: &HostId) -> bool {
        let Some(host) = core.host(id) else {
            return false;
        };
        let instance = host.instance();
        let was_refreshing = self.refreshing(id, instance);
        let cleared_error = self.errors.remove(&(id.clone(), instance)).is_some();
        for status in host.engines.values().filter(|status| {
            matches!(status.sign_in, SignInState::SignedIn { .. }) || status.usage.is_some()
        }) {
            let engine = status.engine;
            if !self.refreshing.insert((id.clone(), instance, engine)) {
                continue;
            }
            let client = host.client.clone();
            let host = id.clone();
            let sender = self.completions.sender();
            runtime().spawn(async move {
                let result = client
                    .engine_action(engine, &EngineCommand::Refresh {})
                    .await
                    .map(drop);
                let _ = sender
                    .send(Completion {
                        host,
                        instance,
                        engine,
                        result,
                    })
                    .await;
            });
        }
        cleared_error || was_refreshing != self.refreshing(id, instance)
    }
    pub fn drain(&mut self, core: &Core) -> bool {
        let visible_refreshing = |refreshing: &HashSet<(HostId, HostInstance, EngineId)>| {
            refreshing
                .iter()
                .filter(|(host, instance, _)| core.host_if_instance(host, *instance).is_some())
                .map(|(host, instance, _)| (host.clone(), *instance))
                .collect::<HashSet<_>>()
        };
        let before_refreshing = visible_refreshing(&self.refreshing);
        let before_errors = self.errors.clone();
        for completion in self.completions.drain() {
            self.refreshing.remove(&(
                completion.host.clone(),
                completion.instance,
                completion.engine,
            ));
            if core
                .host_if_instance(&completion.host, completion.instance)
                .is_none()
            {
                continue;
            }
            if let Err(error) = completion.result {
                self.errors.insert(
                    (completion.host, completion.instance),
                    format!(
                        "Couldn't refresh {}: {error:#}",
                        completion.engine.display_name()
                    ),
                );
            }
        }
        before_refreshing != visible_refreshing(&self.refreshing) || before_errors != self.errors
    }
    pub fn refreshing(&self, host: &HostId, instance: HostInstance) -> bool {
        self.refreshing
            .iter()
            .any(|(id, current, _)| id == host && *current == instance)
    }
    pub fn error(&self, host: &HostId, instance: HostInstance) -> Option<&str> {
        self.errors
            .get(&(host.clone(), instance))
            .map(String::as_str)
    }
}
