use super::{Entry, Reachability};

impl Reachability {
    pub(crate) fn probed(&self, endpoints: usize) -> bool {
        let entries = self.entries();
        entries.len() == endpoints
            && entries
                .values()
                .all(|entry| matches!(entry, Entry::Probed { .. }))
    }
}
