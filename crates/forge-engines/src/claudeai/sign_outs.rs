use std::sync::{Mutex, PoisonError};

#[derive(Debug, Default)]
pub(crate) struct SignOuts(Mutex<u64>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ticket(u64);

impl SignOuts {
    fn count(&self) -> std::sync::MutexGuard<'_, u64> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn ticket(&self) -> Ticket {
        Ticket(*self.count())
    }

    pub(crate) fn keep<T>(&self, ticket: Ticket, save: impl FnOnce() -> T) -> Option<T> {
        let count = self.count();
        (*count == ticket.0).then(save)
    }

    pub(crate) fn sign_out<T>(&self, remove: impl FnOnce() -> T) -> T {
        let mut count = self.count();
        *count += 1;
        remove()
    }
}

#[cfg(test)]
#[path = "../../tests/claudeai/sign_outs.rs"]
mod tests;
