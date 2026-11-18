use crate::error::{self, ErrorClass};
use crate::{Hub, SendAsAgentError};
use artisan_api::{ItemId, ThreadId};
use forge_store::coordination::{Content, MessageState, Record, Update, WatchState};
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum DeliverMessageError {
    #[error("coordination message {id} does not exist")]
    NotFound { id: String },
    #[error("only the sender can deliver coordination message {id}; thread {caller} is not it")]
    NotSender { id: String, caller: ThreadId },
    #[error("coordination record {id} is not a message")]
    NotAMessage { id: String },
    #[error(transparent)]
    Get(#[from] forge_store::coordination::CoordinationGetError),
    #[error("deliver coordination message {id}")]
    Send {
        id: String,
        #[source]
        source: Box<SendAsAgentError>,
    },
    #[error(transparent)]
    Update(#[from] forge_store::coordination::CoordinationUpdateError),
    #[error("coordination message {id} was delivered; reload its record for the latest receipt")]
    Raced { id: String },
}

impl DeliverMessageError {
    #[must_use]
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::NotFound { .. } => ErrorClass::NotFound,
            Self::NotSender { .. } | Self::NotAMessage { .. } | Self::Raced { .. } => {
                ErrorClass::Conflict
            }
            Self::Get(get) => error::coordination(&get.cause),
            Self::Update(update) => error::coordination(&update.cause),
            Self::Send { source, .. } => source.class(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum CoordinationDeliveryError {
    #[error(transparent)]
    Watches(#[from] forge_store::coordination::CoordinationWatchesError),
    #[error(transparent)]
    Receipts(#[from] forge_store::coordination::CoordinationReceiptsError),
    #[error(transparent)]
    Get(#[from] forge_store::coordination::CoordinationGetError),
    #[error(transparent)]
    Update(#[from] forge_store::coordination::CoordinationUpdateError),
    #[error("send the receipt of coordination message {id} to thread {owner}")]
    Receipt {
        id: String,
        owner: ThreadId,
        #[source]
        source: Box<SendAsAgentError>,
    },
}

impl Hub {
    pub async fn deliver_coordination_message(
        &self,
        caller: &ThreadId,
        id: String,
    ) -> Result<Record, DeliverMessageError> {
        let gone = || DeliverMessageError::NotFound { id: id.clone() };
        let record = self
            .store()
            .coordination_get(id.clone())
            .await?
            .ok_or_else(gone)?;
        if &record.owner != caller {
            return Err(DeliverMessageError::NotSender {
                id,
                caller: caller.clone(),
            });
        }
        let Content::Message {
            recipient,
            text,
            intent,
            state,
            ..
        } = &record.content
        else {
            return Err(DeliverMessageError::NotAMessage { id });
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
            .await
            .map_err(|source| DeliverMessageError::Send {
                id: id.clone(),
                source: Box::new(source),
            })?;
        for _ in 0..8 {
            let current = self
                .store()
                .coordination_get(id.clone())
                .await?
                .ok_or_else(gone)?;
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
                        .ok_or_else(gone)?;
                    if latest.revision == current.revision {
                        return Err(error.into());
                    }
                }
            }
        }
        Err(DeliverMessageError::Raced { id })
    }

    pub async fn acknowledge_coordination(
        &self,
        caller: &ThreadId,
        id: String,
        revision: u64,
        state: MessageState,
        reply: Option<String>,
    ) -> Result<Record, forge_store::coordination::CoordinationUpdateError> {
        self.store()
            .coordination_update(
                caller.clone(),
                id,
                revision,
                Update::Acknowledge { state, reply },
            )
            .await
    }

    pub(crate) fn start_coordination(&self) {
        let hub = self.clone();
        self.inner.tasks.spawn(async move {
            let mut changed = hub.store().watch_coordination();
            let mut overlap = hub.generation().map(crate::runtime::Generation::watch);
            let mut deferred = Deferred::default();
            loop {
                changed.borrow_and_update();
                if hub
                    .generation()
                    .is_none_or(crate::runtime::Generation::accepting)
                {
                    hub.deliver_coordination(&mut deferred).await;
                }
                let retry = deferred.next_retry();
                tokio::select! {
                    () = hub.inner.shutdown.cancelled() => break,
                    changed = changed.changed() => if changed.is_err() { break },
                    () = overlap_changed(overlap.as_mut()) => {}
                    () = until(retry) => {}
                }
            }
        });
    }

    async fn deliver_coordination(&self, deferred: &mut Deferred) {
        let now = tokio::time::Instant::now();
        deferred.pass = None;
        if let Err(error) = self.deliver_work_watches(deferred, now).await {
            tracing::warn!("coordination watches: {}", diagnostics::chain(&error));
            deferred.pass_failed(now);
        }
        if let Err(error) = self.deliver_coordination_receipts(deferred, now).await {
            tracing::warn!("coordination receipts: {}", diagnostics::chain(&error));
            deferred.pass_failed(now);
        }
        deferred.forget_settled(now);
    }

    async fn deliver_work_watches(
        &self,
        deferred: &mut Deferred,
        now: tokio::time::Instant,
    ) -> Result<(), CoordinationDeliveryError> {
        for mut watch in self.store().coordination_watches().await? {
            if deferred.waiting_on(&watch.id, now) {
                continue;
            }
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
            let (id, revision) = (watch.id.clone(), watch.revision);
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
                    deferred.delivered(&id);
                }
                Err(error) => {
                    let retry = deferred.failed(&id, revision, now);
                    tracing::warn!(watch = %id, revision, retry_in_s = retry.as_secs(), "deliver coordination watch: {}", diagnostics::chain(&error));
                }
            }
        }
        Ok(())
    }

    async fn deliver_coordination_receipts(
        &self,
        deferred: &mut Deferred,
        now: tokio::time::Instant,
    ) -> Result<(), CoordinationDeliveryError> {
        for record in self
            .store()
            .coordination_receipts(deferred.waiting(now))
            .await?
        {
            let (id, revision) = (record.id.clone(), record.revision);
            match self.deliver_receipt(record).await {
                Ok(()) => deferred.delivered(&id),
                Err(error) => {
                    let retry = deferred.failed(&id, revision, now);
                    tracing::warn!(receipt = %id, revision, retry_in_s = retry.as_secs(), "deliver coordination receipt: {}", diagnostics::chain(&error));
                }
            }
        }
        Ok(())
    }

    async fn deliver_receipt(&self, record: Record) -> Result<(), CoordinationDeliveryError> {
        let Content::Message {
            recipient,
            state,
            reply,
            ..
        } = &record.content
        else {
            unreachable!("invariant: coordination receipt {} is a message", record.id);
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
        .await
        .map_err(|source| CoordinationDeliveryError::Receipt {
            id: record.id.clone(),
            owner: record.owner.clone(),
            source: Box::new(source),
        })?;
        self.store()
            .coordination_update(
                record.owner,
                record.id,
                record.revision,
                Update::ReceiptDelivered,
            )
            .await?;
        Ok(())
    }
}

const FIRST_RECEIPT_RETRY: Duration = Duration::from_secs(2);
const LAST_RECEIPT_RETRY: Duration = Duration::from_mins(5);

struct Retry {
    revision: u64,
    failures: u32,
    at: tokio::time::Instant,
}

async fn overlap_changed(
    overlap: Option<&mut tokio::sync::watch::Receiver<crate::runtime::Overlap>>,
) {
    match overlap {
        Some(overlap) => {
            if overlap.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
        None => std::future::pending().await,
    }
}

async fn until(retry: Option<tokio::time::Instant>) {
    match retry {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

#[derive(Default)]
struct Deferred {
    receipts: std::collections::HashMap<String, Retry>,
    pass: Option<tokio::time::Instant>,
}

impl Deferred {
    fn waiting(&self, now: tokio::time::Instant) -> Vec<String> {
        self.receipts
            .iter()
            .filter(|(_, retry)| retry.at > now)
            .map(|(id, _)| id.clone())
            .collect()
    }

    fn waiting_on(&self, id: &str, now: tokio::time::Instant) -> bool {
        self.receipts.get(id).is_some_and(|retry| retry.at > now)
    }

    fn delivered(&mut self, id: &str) {
        self.receipts.remove(id);
    }

    fn pass_failed(&mut self, now: tokio::time::Instant) {
        self.pass = Some(now + FIRST_RECEIPT_RETRY);
    }

    fn forget_settled(&mut self, now: tokio::time::Instant) {
        self.receipts.retain(|_, retry| retry.at > now);
    }

    fn next_retry(&self) -> Option<tokio::time::Instant> {
        self.receipts
            .values()
            .map(|retry| retry.at)
            .chain(self.pass)
            .min()
    }

    fn failed(&mut self, id: &str, revision: u64, now: tokio::time::Instant) -> Duration {
        let failures = self
            .receipts
            .get(id)
            .filter(|retry| retry.revision == revision)
            .map_or(0, |retry| retry.failures)
            .saturating_add(1);
        let wait = FIRST_RECEIPT_RETRY
            .saturating_mul(2_u32.saturating_pow(failures.saturating_sub(1).min(16)))
            .min(LAST_RECEIPT_RETRY);
        self.receipts.insert(
            id.to_owned(),
            Retry {
                revision,
                failures,
                at: now + wait,
            },
        );
        wait
    }
}
