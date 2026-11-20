use crate::core::{Core, HostId, HostInstance, runtime::runtime};
use std::future::Future;
use tokio_util::task::AbortOnDropHandle;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionKey {
    host: HostId,
    instance: HostInstance,
    generation: u64,
}

impl CompletionKey {
    pub fn host(&self) -> &HostId {
        &self.host
    }

    fn is_live(&self, core: &Core) -> bool {
        core.host_if_instance(&self.host, self.instance).is_some()
    }
}

#[derive(Debug)]
struct Running {
    key: CompletionKey,
    _abort_on_drop: AbortOnDropHandle<()>,
}

#[derive(Debug, Default)]
pub struct CompletionSlot {
    generation: u64,
    running: Option<Running>,
}

impl CompletionSlot {
    pub fn spawn<F>(
        &mut self,
        host: &HostId,
        instance: HostInstance,
        task: impl FnOnce(CompletionKey) -> F,
    ) -> CompletionKey
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.generation = self.generation.wrapping_add(1);
        let key = CompletionKey {
            host: host.clone(),
            instance,
            generation: self.generation,
        };
        self.running = Some(Running {
            key: key.clone(),
            _abort_on_drop: AbortOnDropHandle::new(runtime().spawn(task(key.clone()))),
        });
        key
    }

    pub fn cancel(&mut self) {
        self.running = None;
    }

    pub const fn is_running(&self) -> bool {
        self.running.is_some()
    }

    fn current(&self) -> Option<&CompletionKey> {
        self.running.as_ref().map(|running| &running.key)
    }

    pub fn finish(&mut self, core: &Core, key: &CompletionKey) -> bool {
        if self.current() != Some(key) {
            return false;
        }
        self.running = None;
        key.is_live(core)
    }
}

#[derive(Debug, Default)]
pub struct CompletionScope {
    generation: u64,
}

impl CompletionScope {
    pub fn key(&self, host: &HostId, instance: HostInstance) -> CompletionKey {
        CompletionKey {
            host: host.clone(),
            instance,
            generation: self.generation,
        }
    }

    pub const fn renew(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn accepts(&self, core: &Core, key: &CompletionKey) -> bool {
        key.generation == self.generation && key.is_live(core)
    }
}
