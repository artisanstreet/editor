//! Approvals Forge asks for itself: an Artisan MCP tool that changes trust
//! or deletes something (removing a host or project, adding an MCP server)
//! shows an approval card in the calling thread, like an engine's own, and
//! runs only once the user allows it. A thread with Full access gives
//! these permissions itself, as it does an engine's.

use crate::Hub;
use anyhow::Result;
use artisan_api::{
    Approval, ApprovalRequest, Decision, ItemId, ItemKind, Permissions, RunStatus, Status,
    ThreadId, ToolKind,
};
use forge_store::{NewItem, Refused};
use std::time::Duration;
use tokio::sync::oneshot;

/// How long a card waits, under every harness's MCP call timeout.
const ANSWER_WITHIN: Duration = Duration::from_mins(9);

/// A card Forge showed itself, waiting for its answer.
pub struct OwnApproval {
    thread: ThreadId,
    /// A permission Full access gives; otherwise only the user answers.
    permission: bool,
    answer: oneshot::Sender<Decision>,
}

impl Hub {
    /// Asks the user's permission for `tool` in `thread`: allowed at once
    /// under Full access, otherwise an approval card waits for the user.
    /// `Ok(())` once allowed; refused when denied or unanswered.
    pub async fn ask_approval(
        &self,
        thread: &ThreadId,
        tool: &str,
        description: String,
    ) -> Result<()> {
        self.ask_approval_on(forge_engines::ARTISAN_MCP, thread, tool, description)
            .await
    }

    /// [`Hub::ask_approval`] for a tool of another of Forge's own servers
    /// (`server`, as engines know it).
    pub async fn ask_approval_on(
        &self,
        server: &str,
        thread: &ThreadId,
        tool: &str,
        description: String,
    ) -> Result<()> {
        let full = self
            .store()
            .thread(thread)
            .await?
            .is_some_and(|thread| thread.config.permissions == Permissions::Full);
        if full {
            return Ok(());
        }
        self.show_card(server, thread, tool, description, true)
            .await
    }

    /// Shows a card only the user answers, whatever the thread's access:
    /// for steps the user takes themselves, like a handoff.
    pub async fn ask_user_on(
        &self,
        server: &str,
        thread: &ThreadId,
        tool: &str,
        description: String,
    ) -> Result<()> {
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
    ) -> Result<()> {
        let store = self.store();
        let run_id = store
            .latest_run(thread)
            .await?
            .filter(|run| run.status == RunStatus::Running)
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
        if let Err(error) = store.upsert_item(card(Status::AwaitingUser, None)).await {
            self.approvals_lock().remove(&id);
            return Err(error);
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
        store.upsert_item(card(status, Some(decision))).await?;
        if decision.allows() {
            Ok(())
        } else if decision == Decision::Cancelled {
            Err(
                Refused("The user didn't answer the approval card; nothing changed.".to_owned())
                    .into(),
            )
        } else {
            Err(Refused("The user declined; nothing changed.".to_owned()).into())
        }
    }

    /// Answers a card [`Hub::ask_approval`] showed; `false` when `item`
    /// isn't one.
    pub(crate) fn answer_own_approval(&self, item: &ItemId, decision: Decision) -> bool {
        let Some(card) = self.approvals_lock().remove(item) else {
            return false;
        };
        // The asker stopped waiting; the card is answered either way.
        card.answer.send(decision).ok();
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
            if let Some(card) = approvals.remove(&id) {
                card.answer.send(Decision::AllowOnce).ok();
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
