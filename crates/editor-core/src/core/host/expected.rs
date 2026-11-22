use artisan_api::ThreadId;
use std::collections::BTreeSet;

#[derive(Debug, Default)]
pub(super) struct ExpectedThreads(BTreeSet<ThreadId>);

impl ExpectedThreads {
    pub(super) fn expect(&mut self, id: ThreadId) {
        self.0.insert(id);
    }

    pub(super) fn listed(&mut self, id: &ThreadId) {
        self.0.remove(id);
    }

    pub(super) fn kept(&self) -> impl Iterator<Item = &ThreadId> {
        self.0.iter()
    }
}
