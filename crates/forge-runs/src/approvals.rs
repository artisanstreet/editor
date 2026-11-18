use crate::Hub;
use crate::Refused;
use crate::error::{self, ErrorClass};
use artisan_api::{
    Approval, ApprovalRequest, Decision, ItemId, ItemKind, Permissions, Status, ThreadId, ToolKind,
};
use forge_store::{NewItem, ReadThreadError, UpsertItemError};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

const ANSWER_WITHIN: Duration = Duration::from_mins(9);

pub(crate) struct OwnApproval {
    thread: ThreadId,
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
            Self::ReadThread { .. } => ErrorClass::Internal,
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
        let id = ItemId::new();
        let (answer, answered) = oneshot::channel();
        let (run_id, stopped) = self.lock().get(thread).map_or_else(
            || (None, CancellationToken::new()),
            |run| (Some(run.id.clone()), run.cancel.child_token()),
        );
        let mut waiting = Waiting::register(
            self,
            id.clone(),
            OwnApproval {
                thread: thread.clone(),
                permission,
                answer,
            },
        );
        if permission
            && store
                .thread(thread)
                .await
                .map_err(|source| ApprovalError::ReadThread {
                    tool: tool.to_owned(),
                    source,
                })?
                .is_some_and(|thread| thread.config.permissions == Permissions::Full)
        {
            return Ok(());
        }
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
        store
            .upsert_item(card(Status::AwaitingUser, None))
            .await
            .map_err(|source| ApprovalError::ShowCard {
                tool: tool.to_owned(),
                source,
            })?;
        waiting.shown(card(Status::Cancelled, Some(Decision::Cancelled)));
        let decision = tokio::select! {
            decision = answered => decision.unwrap_or(Decision::Cancelled),
            () = tokio::time::sleep(ANSWER_WITHIN) => Decision::Cancelled,
            () = self.inner.shutdown.cancelled() => Decision::Cancelled,
            () = stopped.cancelled() => Decision::Cancelled,
        };
        let decision = if stopped.is_cancelled() {
            Decision::Cancelled
        } else {
            decision
        };
        waiting.settled();
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
        verdict(decision, thread, id, tool)
    }

    pub(crate) fn answer_own_approval(&self, item: &ItemId, decision: Decision) -> bool {
        let Some(card) = self.approvals_lock().remove(item) else {
            return false;
        };
        if card.answer.send(decision).is_err() {
            tracing::debug!(%item, "the asker stopped waiting; the card is answered anyway");
        }
        true
    }

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

fn verdict(
    decision: Decision,
    thread: &ThreadId,
    id: ItemId,
    tool: &str,
) -> Result<(), ApprovalError> {
    if decision.allows() {
        Ok(())
    } else if decision == Decision::Cancelled {
        Err(ApprovalError::Unanswered {
            thread: thread.clone(),
            item: id,
            tool: tool.to_owned(),
            refused: Refused(
                "The user didn't answer the approval card, or stopped the thread; nothing changed."
                    .to_owned(),
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

struct Waiting {
    hub: Hub,
    id: ItemId,
    abandoned: Option<NewItem>,
}

impl Waiting {
    fn register(hub: &Hub, id: ItemId, card: OwnApproval) -> Self {
        hub.approvals_lock().insert(id.clone(), card);
        Self {
            hub: hub.clone(),
            id,
            abandoned: None,
        }
    }

    fn shown(&mut self, abandoned: NewItem) {
        self.abandoned = Some(abandoned);
    }

    fn settled(&mut self) {
        self.abandoned = None;
        self.hub.approvals_lock().remove(&self.id);
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        self.hub.approvals_lock().remove(&self.id);
        let Some(card) = self.abandoned.take() else {
            return;
        };
        let item = card.id.clone();
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(%item, "no runtime left to settle the abandoned approval card");
            return;
        };
        let store = self.hub.store().clone();
        self.hub.inner.tasks.spawn_on(
            async move {
                if let Err(error) = store.upsert_item(card).await {
                    tracing::warn!(%item, "settle the abandoned approval card: {}", diagnostics::chain(&error));
                }
            },
            &runtime,
        );
    }
}
