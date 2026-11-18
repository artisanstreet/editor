//! Approvals Forge asks for itself: an Artisan MCP tool that changes trust
//! or deletes something (removing a host or project, adding an MCP server)
//! shows an approval card in the calling thread, like an engine's own, and
//! runs only once the user allows it. A thread with Full access gives
//! these permissions itself, as it does an engine's.

use crate::Hub;
use crate::Refused;
use crate::error::{self, ErrorClass};
use artisan_api::{
    Approval, ApprovalRequest, Decision, ItemId, ItemKind, Permissions, RunStatus, Status,
    ThreadId, ToolKind,
};
use forge_store::{LatestRunError, NewItem, ReadThreadError, UpsertItemError};
use std::time::Duration;
use tokio::sync::oneshot;

/// How long a card waits, under every harness's MCP call timeout.
const ANSWER_WITHIN: Duration = Duration::from_mins(9);

/// A card Forge showed itself, waiting for its answer.
pub(crate) struct OwnApproval {
    thread: ThreadId,
    /// A permission Full access gives; otherwise only the user answers.
    permission: bool,
    answer: oneshot::Sender<Decision>,
}

#[derive(Debug, thiserror::Error)]
pub enum ApprovalError {
    #[error("read the thread's access to ask approval for {tool}")]
    ReadThread {
        tool: String,
        #[source]
        source: ReadThreadError,
    },
    #[error("read the running turn to ask approval for {tool}")]
    LatestRun {
        tool: String,
        #[source]
        source: LatestRunError,
    },
    #[error("show the approval card for {tool}")]
    ShowCard {
        tool: String,
        #[source]
        source: UpsertItemError,
    },
    #[error("settle the approval card for {tool}")]
    SettleCard {
        tool: String,
        #[source]
        source: UpsertItemError,
    },
    #[error("approval card {item} for {tool} in thread {thread} went unanswered")]
    Unanswered {
        thread: ThreadId,
        item: ItemId,
        tool: String,
        #[source]
        refused: Refused,
    },
    #[error("the user declined approval card {item} for {tool} in thread {thread}")]
    Declined {
        thread: ThreadId,
        item: ItemId,
        tool: String,
        #[source]
        refused: Refused,
    },
}

impl ApprovalError {
    #[must_use]
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::ReadThread { .. } | Self::LatestRun { .. } => ErrorClass::Internal,
            Self::ShowCard { source, .. } | Self::SettleCard { source, .. } => {
                error::write(&source.cause)
            }
            Self::Unanswered { .. } | Self::Declined { .. } => ErrorClass::Conflict,
        }
    }
}

impl Hub {
    pub async fn ask_approval(
        &self,
        thread: &ThreadId,
        tool: &str,
        description: String,
    ) -> Result<(), ApprovalError> {
        self.ask_approval_on(forge_engines::ARTISAN_MCP, thread, tool, description)
            .await
    }

    pub async fn ask_approval_on(
        &self,
        server: &str,
        thread: &ThreadId,
        tool: &str,
        description: String,
    ) -> Result<(), ApprovalError> {
        let full = self
            .store()
            .thread(thread)
            .await
            .map_err(|source| ApprovalError::ReadThread {
                tool: tool.to_owned(),
                source,
            })?
            .is_some_and(|thread| thread.config.permissions == Permissions::Full);
        if full {
            return Ok(());
        }
        self.show_card(server, thread, tool, description, true)
            .await
    }

    pub async fn ask_user_on(
        &self,
        server: &str,
        thread: &ThreadId,
        tool: &str,
        description: String,
    ) -> Result<(), ApprovalError> {
        self.show_card(server, thread, tool, description, false)
            .await
    }

    async fn show_card(
        &self,
        server: &str,
        thread: &ThreadId,
        tool: &str,
        description: String,
        permission: bool,
    ) -> Result<(), ApprovalError> {
        let store = self.store();
        let run_id = store
            .latest_run(thread)
            .await
            .map_err(|source| ApprovalError::LatestRun {
                tool: tool.to_owned(),
                source,
            })?
            .filter(|run| run.status() == RunStatus::Running)
            .map(|run| run.id);
        let id = ItemId::new();
        let (answer, answered) = oneshot::channel();
        self.approvals_lock().insert(
            id.clone(),
            OwnApproval {
                thread: thread.clone(),
                permission,
                answer,
            },
        );
        let card = |status, decision| NewItem {
            id: id.clone(),
            thread_id: thread.clone(),
            run_id: run_id.clone(),
            parent_id: None,
            native_id: None,
            status,
            kind: ItemKind::Tool {
                tool: ToolKind::Mcp {
                    server: server.to_owned(),
                    tool: tool.to_owned(),
                    input: None,
                },
                title: Some(description.clone()),
                output: None,
                exit_code: None,
                approval: Some(Approval {
                    request: ApprovalRequest::Action {
                        description: description.clone().into(),
                    },
                    reason: None,
                    options: vec![Decision::AllowOnce, Decision::DenyOnce],
                    decision,
                }),
            },
        };
        if let Err(source) = store.upsert_item(card(Status::AwaitingUser, None)).await {
            self.approvals_lock().remove(&id);
            return Err(ApprovalError::ShowCard {
                tool: tool.to_owned(),
                source,
            });
        }
        let decision = tokio::select! {
            decision = answered => decision.unwrap_or(Decision::Cancelled),
            () = tokio::time::sleep(ANSWER_WITHIN) => Decision::Cancelled,
            () = self.inner.shutdown.cancelled() => Decision::Cancelled,
        };
        self.approvals_lock().remove(&id);
        let status = if decision.allows() {
            Status::Done
        } else if decision == Decision::Cancelled {
            Status::Cancelled
        } else {
            Status::Declined
        };
        store
            .upsert_item(card(status, Some(decision)))
            .await
            .map_err(|source| ApprovalError::SettleCard {
                tool: tool.to_owned(),
                source,
            })?;
        if decision.allows() {
            Ok(())
        } else if decision == Decision::Cancelled {
            Err(ApprovalError::Unanswered {
                thread: thread.clone(),
                item: id,
                tool: tool.to_owned(),
                refused: Refused(
                    "The user didn't answer the approval card; nothing changed.".to_owned(),
                ),
            })
        } else {
            Err(ApprovalError::Declined {
                thread: thread.clone(),
                item: id,
                tool: tool.to_owned(),
                refused: Refused("The user declined; nothing changed.".to_owned()),
            })
        }
    }

    /// Answers a card [`Hub::ask_approval`] showed; `false` when `item`
    /// isn't one.
    pub(crate) fn answer_own_approval(&self, item: &ItemId, decision: Decision) -> bool {
        let Some(card) = self.approvals_lock().remove(item) else {
            return false;
        };
        if card.answer.send(decision).is_err() {
            tracing::debug!(%item, "the asker stopped waiting; the card is answered anyway");
        }
        true
    }

    /// Allows the permission cards waiting in `thread`, once it has Full
    /// access.
    pub(crate) fn allow_own_approvals(&self, thread: &ThreadId) {
        let mut approvals = self.approvals_lock();
        let waiting: Vec<ItemId> = approvals
            .iter()
            .filter(|(_, card)| card.permission && card.thread == *thread)
            .map(|(id, _)| id.clone())
            .collect();
        for id in waiting {
            if let Some(card) = approvals.remove(&id)
                && card.answer.send(Decision::AllowOnce).is_err()
            {
                tracing::debug!(item = %id, "the asker stopped waiting; the card is allowed anyway");
            }
        }
    }

    fn approvals_lock(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<ItemId, OwnApproval>> {
        self.inner
            .approvals
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
