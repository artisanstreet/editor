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
    task: AbortOnDropHandle<()>,
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
        let task = AbortOnDropHandle::new(runtime().spawn(task(key.clone())));
        self.running = Some(Running {
            key: key.clone(),
            task,
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

    pub fn is_finished(&self) -> bool {
        self.running
            .as_ref()
            .is_some_and(|running| running.task.is_finished())
    }

    pub fn finish(&mut self, core: &Core, key: &CompletionKey) -> bool {
        if self.current() != Some(key) {
            return false;
        }
        self.running = None;
        key.is_live(core)
    }
}
