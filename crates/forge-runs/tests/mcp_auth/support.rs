use super::Mcp;
use std::sync::Arc;

impl Mcp {
    pub(crate) fn lifetime_holders(&self, server: &str) -> usize {
        self.flows_lock()
            .lifetimes
            .get(server)
            .map_or(0, Arc::strong_count)
    }
}
