use crate::Hub;
use anyhow::{Context, Result, ensure};
use artisan_api::{ItemId, ThreadId};
use forge_store::coordination::{Content, MessageState, Record, Update, WatchState};
use std::time::Duration;

impl Hub {
    pub async fn deliver_coordination_message(
        &self,
        caller: &ThreadId,
        id: String,
    ) -> Result<Record> {
        let record = self
            .store()
            .coordination_get(id.clone())
            .await?
            .context("message not found")?;
        ensure!(
            &record.owner == caller,
            "only the sender can deliver this message"
        );
        let Content::Message {
            recipient,
            text,
            intent,
            state,
            ..
        } = &record.content
        else {
            anyhow::bail!("record is not a coordination message");
        };
        if *state != MessageState::Prepared {
            return Ok(record);
        }
        let body = format!(
            "Coordination request {} ({intent:?}) from thread {}.\n\n{text}\n\nAcknowledge with acknowledge_coordination_message using this request's id and current revision. Delivery does not mean agreement.",
            record.id, caller
        );
        let item = self
            .send_as_agent_with_id(caller, recipient, body, ItemId::from(id.clone()))
            .await?;
        for _ in 0..8 {
            let current = self
                .store()
                .coordination_get(id.clone())
                .await?
                .context("message disappeared")?;
            match self
                .store()
                .coordination_update(
                    caller.clone(),
                    id.clone(),
                    current.revision,
                    Update::MessageDelivered {
                        item: item.id.clone(),
                    },
                )
                .await
            {
                Ok(record) => return Ok(record),
                Err(error) => {
                    let latest = self
                        .store()
                        .coordination_get(id.clone())
                        .await?
                        .context("message disappeared")?;
                    if latest.revision == current.revision {
                        return Err(error);
                    }
                }
            }
        }
        anyhow::bail!(
            "message was delivered; reload its coordination record for the latest receipt"
        )
    }

    pub async fn acknowledge_coordination(
        &self,
        caller: &ThreadId,
        id: String,
        revision: u64,
        state: MessageState,
        reply: Option<String>,
    ) -> Result<Record> {
        let record = self
            .store()
            .coordination_update(
                caller.clone(),
                id,
                revision,
                Update::Acknowledge { state, reply },
            )
            .await?;
        Ok(record)
    }

    pub(crate) fn start_coordination(&self) {
        let hub = self.clone();
        self.inner.tasks.spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    () = hub.inner.shutdown.cancelled() => break,
                    _tick = interval.tick() => {
                        if let Err(error) = hub.deliver_work_watches().await {
                            tracing::warn!("coordination watches: {error:#}");
                        }
                        if let Err(error) = hub.deliver_coordination_receipts().await {
                            tracing::warn!("coordination receipts: {error:#}");
                        }
                    }
                }
            }
        });
    }

    async fn deliver_work_watches(&self) -> Result<()> {
        for mut watch in self.store().coordination_watches().await? {
            let Content::Watch {
                target,
                after_revision,
                ..
            } = &watch.content
            else {
                continue;
            };
            if matches!(
                watch.content,
                Content::Watch {
                    state: WatchState::Pending,
                    ..
                }
            ) {
                let Some(work) = self
                    .store()
                    .coordination_get(format!("work:{target}"))
                    .await?
                else {
                    continue;
                };
                let Content::Work {
                    state,
                    summary,
                    evidence,
                    ..
                } = &work.content
                else {
                    continue;
                };
                if !state.terminal() || work.revision <= *after_revision {
                    continue;
                }
                let text = format!(
                    "Work dependency {target} is {state:?} at revision {}.\n\n{summary}\n\nEvidence:\n{}\n\nInspect get_work before consuming this result; completed work may have changed since validation.",
                    work.revision,
                    evidence.join("\n")
                );
                watch = self
                    .store()
                    .coordination_update(
                        watch.owner.clone(),
                        watch.id.clone(),
                        watch.revision,
                        Update::PrepareWatch { outcome: text },
                    )
                    .await?;
            }
            let Content::Watch {
                outcome: Some(text),
                ..
            } = &watch.content
            else {
                continue;
            };
            let text = text.clone();
            match self
                .send_as_agent_with_id(
                    &watch.owner,
                    &watch.owner,
                    text.clone(),
                    ItemId::from(format!("coordination-watch:{}", watch.id)),
                )
                .await
            {
                Ok(_) => {
                    self.store()
                        .coordination_update(
                            watch.owner.clone(),
                            watch.id,
                            watch.revision,
                            Update::FinishWatch {
                                state: WatchState::Delivered,
                                outcome: text,
                            },
                        )
                        .await?;
                }
                Err(error) => {
                    tracing::warn!(watch = %watch.id, "deliver coordination watch: {error:#}");
                }
            }
        }
        Ok(())
    }

    async fn deliver_coordination_receipts(&self) -> Result<()> {
        for record in self.store().coordination_receipts().await? {
            let Content::Message {
                recipient,
                state,
                reply,
                ..
            } = &record.content
            else {
                continue;
            };
            let text = format!(
                "Coordination receipt: thread {recipient} marked request {} as {state:?}.\n\n{}",
                record.id,
                reply.as_deref().unwrap_or("No additional reply.")
            );
            self.send_as_agent_with_id(
                &record.owner,
                &record.owner,
                text,
                ItemId::from(format!(
                    "coordination-receipt:{}:{}",
                    record.id, record.revision
                )),
            )
            .await?;
            self.store()
                .coordination_update(
                    record.owner,
                    record.id,
                    record.revision,
                    Update::ReceiptDelivered,
                )
                .await?;
        }
        Ok(())
    }
}
