use super::{HostCompletion, HostData, HostEvent, HostMessage};
use artisan_api::DatabaseRollback;
use tokio_util::task::AbortOnDropHandle;

#[derive(Debug, Default)]
pub enum RollbackNotice {
    #[default]
    Hidden,
    Shown(DatabaseRollback),
    Dismissing(DatabaseRollback, AbortOnDropHandle<()>),
    Failed {
        rollback: DatabaseRollback,
        error: String,
    },
}

impl RollbackNotice {
    pub(super) fn from_state(rollback: Option<DatabaseRollback>) -> Self {
        rollback.map_or(Self::Hidden, Self::Shown)
    }

    #[must_use]
    pub fn message(&self) -> Option<String> {
        match self {
            Self::Hidden | Self::Dismissing(..) => None,
            Self::Shown(rollback) => Some(text(rollback)),
            Self::Failed { rollback, error } => {
                Some(format!("{} Couldn't dismiss this: {error}", text(rollback)))
            }
        }
    }
}

fn text(rollback: &DatabaseRollback) -> String {
    format!(
        "Forge went back to the database it had before its last upgrade (schema {} instead of {}). Threads and settings changed since then are kept in {}.",
        rollback.restored, rollback.found, rollback.set_aside
    )
}

impl HostData {
    #[must_use]
    pub const fn rollback_notice(&self) -> &RollbackNotice {
        &self.rollback_notice
    }

    pub fn dismiss_rollback_notice(&mut self) {
        let rollback = match std::mem::take(&mut self.rollback_notice) {
            RollbackNotice::Shown(rollback) | RollbackNotice::Failed { rollback, .. } => rollback,
            unchanged @ (RollbackNotice::Hidden | RollbackNotice::Dismissing(..)) => {
                self.rollback_notice = unchanged;
                return;
            }
        };
        let client = self.client.clone();
        let sender = self.delivery.sender().map(HostMessage::completion);
        let task = crate::core::runtime::runtime().spawn(async move {
            let result = client.dismiss_database_rollback().await;
            if sender
                .send(HostCompletion::RollbackDismissed(result))
                .await
                .is_err()
            {
                tracing::debug!("the host closed before the rollback notice was dismissed");
            }
        });
        self.rollback_notice = RollbackNotice::Dismissing(rollback, AbortOnDropHandle::new(task));
        self.pending_events.push(HostEvent::Changed);
    }

    pub(super) fn finish_rollback_dismissal(
        &mut self,
        result: Result<(), crate::core::client::CallError>,
    ) {
        self.rollback_notice = match (std::mem::take(&mut self.rollback_notice), result) {
            (RollbackNotice::Dismissing(..), Ok(())) => RollbackNotice::Hidden,
            (RollbackNotice::Dismissing(rollback, _), Err(error)) => {
                let error = ::diagnostics::chain(&error);
                tracing::warn!(error, "dismiss the database rollback notice");
                RollbackNotice::Failed { rollback, error }
            }
            (current, _) => current,
        };
    }
}
