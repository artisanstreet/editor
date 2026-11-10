use crate::core::{CoreEvent, HostEvent};

#[derive(Default)]
pub struct LiveBatch {
    pending: Vec<CoreEvent>,
}

impl LiveBatch {
    pub fn push(&mut self, event: CoreEvent) -> Option<CoreEvent> {
        let CoreEvent::Host {
            host,
            event: HostEvent::Live { thread_id, target },
        } = &event
        else {
            return Some(event);
        };
        let duplicate = self.pending.iter().any(|queued| {
            matches!(queued, CoreEvent::Host { host: other_host, event: HostEvent::Live { thread_id: other_thread, target: other_target } }
                if host == other_host && thread_id == other_thread && target == other_target)
        });
        if !duplicate {
            self.pending.push(event);
        }
        None
    }

    pub fn take(&mut self) -> Vec<CoreEvent> {
        std::mem::take(&mut self.pending)
    }
}
