use crate::conversation::controller::Conversation;
use crate::core::{Core, HostId};
use artisan_api::{Seq, ThreadId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ReadWatch {
    focused: bool,
    thread: Option<(HostId, ThreadId)>,
    run: Option<(Seq, bool)>,
    read: Option<(Seq, Seq, bool)>,
}

impl ReadWatch {
    pub(super) fn of(conversation: &Conversation, core: &Core, focused: bool) -> Self {
        let thread = conversation.open.clone();
        let host = thread.as_ref().and_then(|(host, _)| core.host(host));
        let id = thread.as_ref().map(|(_, id)| id);
        let run = host
            .zip(id)
            .and_then(|(host, id)| host.runs.get(id))
            .map(|run| (run.seq, run.status().is_live()));
        let read = host
            .zip(id)
            .and_then(|(host, id)| host.threads.get(id))
            .map(|thread| (thread.activity_seq, thread.read_seq, thread.marked_unread));
        Self {
            focused,
            thread,
            run,
            read,
        }
    }

    pub(super) fn wants_recheck(&self, previous: Option<&Self>) -> bool {
        self.focused
            && self
                .read
                .is_some_and(|(activity, read, marked)| marked || activity > read)
            && previous != Some(self)
    }
}
