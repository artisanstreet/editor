//! Serialized connection-owned conversation subscription delivery.
//!
//! The driver retains one fresh subscription context, one process-wide wake
//! receiver, one delivery writer, and at most one activated subscription per
//! thread. It never spawns work or queues patch data: every wake performs a
//! bounded authoritative re-read, and the writer advances the connection
//! registry only after the corresponding batch has crossed the wire.
//!
//! Beside each subscription's patches, observations, outbox, and display
//! title, the driver pushes connection-scoped host state (every engine's
//! usage with its readiness, the user's preferences) whenever the
//! notifier's host-state revision moves, and, once the connection read them,
//! the recent threads across every project whenever they change; only a
//! changed value is sent.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::time::Duration;

use artisan_database::{MessageOutboxFingerprint, QueuedMessageRepositoryError, Repository};
use artisan_domain::{
    EngineUsageSnapshot, Event, FAILED_MESSAGE_LIST_MAX, ListFailedMessages, ListQueuedMessages,
    MessageOutbox, QUEUED_MESSAGE_LIST_MAX, QueuedMessageListOrder, RunUsageResult, ThreadId,
    ThreadRetitled, ThreadTitle, UserPreferences,
};
use artisan_transport::{CancelHandle, DeadlineError, OperationKind, run_with_deadline};

use crate::activated_conversation_replay::{
    OBSERVATION_HISTORY_PAGE_LIMIT, read_activated_conversation_replay,
    read_activated_observation_history,
};
use crate::connection::{
    DeliveryStageError, RequestDispatchOutcome, RequestStageError, ServerFrameStamp,
};
use crate::conversation_commit_notifier::{
    ConversationCommitAnySubscription, ConversationCommitWaitError,
};
use crate::conversation_delivery_writer::{
    ConversationDeliveryError, ConversationDeliveryWriter, ConversationReplayDelivery,
    ObservationBatchDelivery,
};
use crate::request_handler::{ActivatedConversationSubscription, ConversationConnectionContext};

#[path = "conversation_delivery_driver/recent_threads.rs"]
mod recent_threads;
use recent_threads::DeliveredRecentThreads;
pub(crate) use recent_threads::RequestFollowUp;

/// One serialized conversation delivery owner for an authenticated
/// connection.
#[derive(Debug)]
pub(crate) struct ConversationDeliveryDriver {
    context: ConversationConnectionContext,
    writer: Option<ConversationDeliveryWriter>,
    wake: ConversationCommitAnySubscription,
    active: BTreeMap<artisan_domain::ThreadId, ActivatedConversationSubscription>,
    /// The message outbox last pushed to each active subscription.
    outboxes: BTreeMap<ThreadId, DeliveredOutbox>,
    /// The display title last pushed for each active subscription.
    titles: BTreeMap<ThreadId, ThreadTitle>,
    /// The live run's usage last pushed for each active subscription.
    run_usage: BTreeMap<ThreadId, RunUsageResult>,
    /// The connection-scoped host state last pushed.
    host: DeliveredHostState,
    /// The recent threads last served or pushed, once the connection read
    /// them.
    recent: Option<DeliveredRecentThreads>,
}

/// The host state last pushed to this connection and the host-state
/// revision it was read at.
#[derive(Debug, Default)]
struct DeliveredHostState {
    revision: Option<u64>,
    preferences: Option<UserPreferences>,
    /// The usage snapshot last pushed per engine.
    usage: BTreeMap<String, EngineUsageSnapshot>,
}

/// One subscription's last pushed outbox and the fingerprint it was read at.
#[derive(Debug)]
struct DeliveredOutbox {
    fingerprint: MessageOutboxFingerprint,
    outbox: MessageOutbox,
}

impl ConversationDeliveryDriver {
    /// Creates the sole writer and the sole process-wide wake receiver for a
    /// connection.
    pub(crate) fn new(
        connection: quinn::Connection,
        protocol_version: artisan_protocol::ProtocolVersion,
        context: ConversationConnectionContext,
    ) -> Self {
        let registrar = context.registrar().clone();
        let wake = context.notifier().subscribe_any();
        Self {
            context,
            writer: Some(ConversationDeliveryWriter::new(
                connection,
                registrar,
                protocol_version,
            )),
            wake,
            active: BTreeMap::new(),
            outboxes: BTreeMap::new(),
            titles: BTreeMap::new(),
            run_usage: BTreeMap::new(),
            host: DeliveredHostState::default(),
            recent: None,
        }
    }

    /// Borrows the connection-owned request context for dispatch and
    /// activation.
    pub(crate) fn context(&self) -> &ConversationConnectionContext {
        &self.context
    }

    /// Waits for one coalesced process-wide commit wake.
    pub(crate) async fn wait_for_wake(&mut self) -> Result<(), DeliveryStageError> {
        self.wake
            .wait()
            .await
            .map_err(|ConversationCommitWaitError::Closed| DeliveryStageError::NotifierClosed)
    }

    /// Applies the post-response request outcome in connection order.
    ///
    /// Unsubscribe removes the old active state after its acknowledgement has
    /// finished; replacement removes the old state before the new activated
    /// state is installed. The initial replay is always read from the exact
    /// activated cursor before the state is retained, and the authoritative
    /// observation history (thread-scoped `delivery_sequence`, including
    /// settled turns) is drained from the subscriber's observation cursor
    /// before the state is retained.
    pub(crate) async fn handle_request<F>(
        &mut self,
        outcome: RequestDispatchOutcome,
        stamp: &mut F,
        limit: Duration,
        cancel: &CancelHandle,
    ) -> Result<(), DeadlineError<RequestStageError>>
    where
        F: FnMut() -> Result<ServerFrameStamp, RequestStageError>,
    {
        if let Some(thread_id) = outcome.follow_up.stopped_thread {
            self.forget(&thread_id);
        }
        if let Some(listing) = outcome.follow_up.recent_threads {
            // The read is what the Editor now shows; anything that changed
            // since it was read is pushed below.
            self.recent = Some(DeliveredRecentThreads {
                fingerprint: None,
                subtitle_generation: self.context.subtitle_generation(),
                listing,
            });
        }

        if let Some(subscription) = outcome.activation {
            let thread_id = subscription.lease().thread_id().clone();
            self.forget(&thread_id);
            let subscription = self
                .deliver_until_current(subscription, stamp, limit, cancel)
                .await?;
            let subscription = self
                .deliver_observation_history(subscription, stamp, limit, cancel)
                .await?;
            self.deliver_message_outbox(&thread_id, stamp, limit, cancel)
                .await?;
            self.deliver_thread_title(&thread_id, false, stamp, limit, cancel)
                .await?;
            self.deliver_run_usage(&thread_id, stamp, limit, cancel)
                .await?;
            self.active.insert(thread_id, subscription);
        }
        // Every request looks for a host-state change; the first one also
        // pushes the current usage.
        self.deliver_host_state(stamp, limit, cancel).await?;
        self.deliver_recent_threads(stamp, limit, cancel).await
    }

    fn forget(&mut self, thread_id: &ThreadId) {
        self.active.remove(thread_id);
        self.outboxes.remove(thread_id);
        self.titles.remove(thread_id);
        self.run_usage.remove(thread_id);
    }

    /// Re-reads every active subscription once after a coalesced process-wide
    /// wake. A successful batch advances its state and the next read uses the
    /// cursor returned by the writer, so repeated wakes cannot duplicate it.
    /// Patch replay and authoritative observation history both drain to the
    /// durable tail, so two commits before one wake lose no events.
    pub(crate) async fn deliver_wake<F>(
        &mut self,
        stamp: &mut F,
        limit: Duration,
        cancel: &CancelHandle,
    ) -> Result<(), DeadlineError<RequestStageError>>
    where
        F: FnMut() -> Result<ServerFrameStamp, RequestStageError>,
    {
        let threads: Vec<_> = self.active.keys().cloned().collect();
        for thread_id in threads {
            let Some(subscription) = self.active.remove(&thread_id) else {
                continue;
            };
            let subscription = self
                .deliver_until_current(subscription, stamp, limit, cancel)
                .await?;
            let subscription = self
                .deliver_observation_history(subscription, stamp, limit, cancel)
                .await?;
            self.deliver_message_outbox(&thread_id, stamp, limit, cancel)
                .await?;
            self.deliver_thread_title(&thread_id, true, stamp, limit, cancel)
                .await?;
            self.deliver_run_usage(&thread_id, stamp, limit, cancel)
                .await?;
            self.active.insert(thread_id, subscription);
        }
        self.deliver_host_state(stamp, limit, cancel).await?;
        self.deliver_recent_threads(stamp, limit, cancel).await
    }

    /// Pushes the latest usage report of a subscribed thread's live run when
    /// it changed (on activation when one exists), so an Editor shows a
    /// running turn's context usage without polling. A thread without a
    /// live run pushes nothing.
    async fn deliver_run_usage<F>(
        &mut self,
        thread_id: &ThreadId,
        stamp: &mut F,
        limit: Duration,
        cancel: &CancelHandle,
    ) -> Result<(), DeadlineError<RequestStageError>>
    where
        F: FnMut() -> Result<ServerFrameStamp, RequestStageError>,
    {
        let Some(run_id) = self.context.live_run(thread_id) else {
            return Ok(());
        };
        let repository = self.context.repository().clone();
        let report = run_with_deadline(
            OperationKind::Receive,
            limit,
            cancel,
            repository.read_latest_run_usage(&run_id, thread_id),
        )
        .await
        .map_err(|error| map_deadline(&error, DeliveryStageError::Replay))?;
        let Some(report) = report else {
            return Ok(());
        };
        let compaction_at = crate::context_compaction_policy::compaction_at_tokens(&report);
        let Ok(usage) = RunUsageResult::new(thread_id.clone(), run_id, Some(report))
            .map(|usage| usage.with_compaction_at(compaction_at))
        else {
            return Ok(());
        };
        if self.run_usage.get(thread_id) == Some(&usage) {
            return Ok(());
        }
        self.send_state_event(Event::RunUsage(usage.clone()), stamp, limit, cancel)
            .await?;
        self.run_usage.insert(thread_id.clone(), usage);
        Ok(())
    }

    /// Pushes a subscribed thread's display title when it changed, so a
    /// title the Forge refines appears without waiting for a listing read.
    /// Activation records the title the Editor just listed without pushing
    /// it (`announce` false).
    async fn deliver_thread_title<F>(
        &mut self,
        thread_id: &ThreadId,
        announce: bool,
        stamp: &mut F,
        limit: Duration,
        cancel: &CancelHandle,
    ) -> Result<(), DeadlineError<RequestStageError>>
    where
        F: FnMut() -> Result<ServerFrameStamp, RequestStageError>,
    {
        let repository = self.context.repository().clone();
        let title = run_with_deadline(
            OperationKind::Receive,
            limit,
            cancel,
            repository.thread_display_title(thread_id),
        )
        .await
        .map_err(|error| map_deadline(&error, DeliveryStageError::Replay))?;
        let Some(title) = title else {
            return Ok(());
        };
        if self.titles.get(thread_id) == Some(&title) {
            return Ok(());
        }
        if announce {
            let event = Event::ThreadRetitled(ThreadRetitled {
                thread_id: thread_id.clone(),
                title: title.clone(),
            });
            self.send_state_event(event, stamp, limit, cancel).await?;
        }
        self.titles.insert(thread_id.clone(), title);
        Ok(())
    }

    /// Pushes the connection-scoped host state when its revision moved since
    /// the last push; each value crosses the wire only when changed. Each
    /// engine's usage with its readiness is pushed as its own narrowed
    /// snapshot from the connection's first request on; the user's
    /// preferences only when they change (an Editor reads them as it
    /// connects).
    async fn deliver_host_state<F>(
        &mut self,
        stamp: &mut F,
        limit: Duration,
        cancel: &CancelHandle,
    ) -> Result<(), DeadlineError<RequestStageError>>
    where
        F: FnMut() -> Result<ServerFrameStamp, RequestStageError>,
    {
        let revision = self.wake.host_revision();
        if self.host.revision == Some(revision) {
            return Ok(());
        }
        let repository = self.context.repository().clone();
        let stored = run_with_deadline(
            OperationKind::Receive,
            limit,
            cancel,
            repository.read_user_preferences(),
        )
        .await
        .map_err(|error| map_deadline(&error, DeliveryStageError::Replay))?;
        let preferences = crate::account_profile::user_preferences(stored);
        // An Editor reads the preferences when it connects; this connection
        // pushes only their later changes.
        if self.host.revision.is_some() && self.host.preferences.as_ref() != Some(&preferences) {
            let event = Event::UserPreferences(preferences.clone());
            self.send_state_event(event, stamp, limit, cancel).await?;
        }
        self.host.preferences = Some(preferences);
        let usage = self
            .context
            .account_usage()
            .map(crate::account_usage_service::AccountUsageService::current_snapshots)
            .unwrap_or_default();
        for snapshot in usage {
            let Some(engine) = snapshot.engines().first().map(|report| report.engine_id()) else {
                continue;
            };
            if self.host.usage.get(engine) == Some(&snapshot) {
                continue;
            }
            let engine = engine.to_owned();
            let event = Event::AccountUsage(snapshot.clone());
            self.send_state_event(event, stamp, limit, cancel).await?;
            self.host.usage.insert(engine, snapshot);
        }
        self.host.revision = Some(revision);
        Ok(())
    }

    /// Sends one state event on the shared delivery stream.
    async fn send_state_event<F>(
        &mut self,
        event: Event,
        stamp: &mut F,
        limit: Duration,
        cancel: &CancelHandle,
    ) -> Result<(), DeadlineError<RequestStageError>>
    where
        F: FnMut() -> Result<ServerFrameStamp, RequestStageError>,
    {
        let frame = stamp().map_err(|error| DeadlineError::Peer {
            operation: OperationKind::Send,
            error,
        })?;
        let writer = self
            .writer
            .take()
            .ok_or_else(|| delivery_failure(OperationKind::Send, DeliveryStageError::Writer))?;
        let writer = run_with_deadline(
            OperationKind::Send,
            limit,
            cancel,
            writer.deliver_state_event(frame, event),
        )
        .await
        .map_err(map_writer_deadline)?;
        self.writer = Some(writer);
        Ok(())
    }

    /// Pushes the thread's message outbox when it changed since the last
    /// push to this subscription (always on activation).
    ///
    /// It runs after the thread's patches, so a message that reached the
    /// transcript leaves the outbox in the same wake that delivered its
    /// transcript item. A cheap fingerprint skips the listing reads when no
    /// dispatch of the thread moved; an unchanged outbox is not resent.
    async fn deliver_message_outbox<F>(
        &mut self,
        thread_id: &ThreadId,
        stamp: &mut F,
        limit: Duration,
        cancel: &CancelHandle,
    ) -> Result<(), DeadlineError<RequestStageError>>
    where
        F: FnMut() -> Result<ServerFrameStamp, RequestStageError>,
    {
        let repository = self.context.repository().clone();
        let fingerprint = run_with_deadline(
            OperationKind::Receive,
            limit,
            cancel,
            repository.message_outbox_fingerprint(thread_id),
        )
        .await
        .map_err(|error| map_deadline(&error, DeliveryStageError::Replay))?;
        if self
            .outboxes
            .get(thread_id)
            .is_some_and(|delivered| delivered.fingerprint == fingerprint)
        {
            return Ok(());
        }
        let outbox = run_with_deadline(
            OperationKind::Receive,
            limit,
            cancel,
            read_message_outbox(&repository, thread_id),
        )
        .await
        .map_err(|error| map_deadline(&error, DeliveryStageError::Replay))?;
        let unchanged = self
            .outboxes
            .get(thread_id)
            .is_some_and(|delivered| delivered.outbox == outbox);
        if !unchanged {
            self.send_state_event(Event::MessageOutbox(outbox.clone()), stamp, limit, cancel)
                .await?;
        }
        self.outboxes.insert(
            thread_id.clone(),
            DeliveredOutbox {
                fingerprint,
                outbox,
            },
        );
        Ok(())
    }

    /// Drains the authoritative observation history for one active
    /// subscription to the durable tail.
    ///
    /// Reads bounded pages via `read_observation_history` from the
    /// subscriber's thread-scoped observation cursor and publishes each full
    /// page through the writer before the registrar advances; any send
    /// failure retains the cursor. A coalesced wake that arrives mid-drain is
    /// preserved by the process-wide notifier and observed on the next scan,
    /// so bounded pagination never drops events. With no active subscription
    /// there is nothing to publish and the durable history stays available
    /// for reconnect replay.
    pub(crate) async fn deliver_observation_history<F>(
        &mut self,
        subscription: ActivatedConversationSubscription,
        stamp: &mut F,
        limit: Duration,
        cancel: &CancelHandle,
    ) -> Result<ActivatedConversationSubscription, DeadlineError<RequestStageError>>
    where
        F: FnMut() -> Result<ServerFrameStamp, RequestStageError>,
    {
        let thread_id: ThreadId = subscription.lease().thread_id().clone();
        loop {
            let after = self
                .context
                .registrar()
                .subscription_view(&thread_id)
                .await
                .map_or(0, |view| view.observation_cursor());
            // Fall back to the just-activated subscription when the registrar
            // has not yet observed it (initial activation path): the cursor
            // is zero there by construction.
            let page = run_with_deadline(
                OperationKind::Receive,
                limit,
                cancel,
                read_activated_observation_history(
                    self.context.repository(),
                    &subscription,
                    after,
                    OBSERVATION_HISTORY_PAGE_LIMIT,
                ),
            )
            .await
            .map_err(|error| map_deadline(&error, DeliveryStageError::Replay))?;
            if page.is_empty() {
                return Ok(subscription);
            }
            let lease = subscription.lease().clone();
            let mut batch = Vec::with_capacity(page.len());
            for event in page {
                let frame = stamp().map_err(|error| DeadlineError::Peer {
                    operation: OperationKind::Send,
                    error,
                })?;
                batch.push((frame, event));
            }
            let writer = self
                .writer
                .take()
                .ok_or_else(|| delivery_failure(OperationKind::Send, DeliveryStageError::Writer))?;
            let delivered = run_with_deadline(
                OperationKind::Send,
                limit,
                cancel,
                writer.deliver_observation_batch(&lease, thread_id.clone(), batch),
            )
            .await
            .map_err(map_writer_deadline)?;
            let (writer, delivered) = delivered;
            self.writer = Some(writer);
            match delivered {
                ObservationBatchDelivery::Current { .. } => return Ok(subscription),
                ObservationBatchDelivery::Published { .. } => {
                    // Another page may have committed during the send; loop
                    // until the durable tail instead of stopping after one
                    // page so two commits before one ack lose no events.
                }
            }
        }
    }

    /// Finishes the one writer and then clears all connection-local registry
    /// state. For peer/error cleanup the unfinished writer is dropped so its
    /// existing guard resets any open output instead of attempting a finish.
    pub(crate) async fn cleanup(&mut self, graceful: bool) -> Result<(), DeliveryStageError> {
        let writer_result = if graceful {
            self.finish_writer()
        } else {
            let _writer = self.writer.take();
            Ok(())
        };

        self.context.registrar().clear_all().await;
        self.active.clear();
        self.outboxes.clear();
        self.titles.clear();
        self.run_usage.clear();
        self.recent = None;
        writer_result
    }

    async fn deliver_until_current<F>(
        &mut self,
        mut subscription: ActivatedConversationSubscription,
        stamp: &mut F,
        limit: Duration,
        cancel: &CancelHandle,
    ) -> Result<ActivatedConversationSubscription, DeadlineError<RequestStageError>>
    where
        F: FnMut() -> Result<ServerFrameStamp, RequestStageError>,
    {
        loop {
            let replay = run_with_deadline(
                OperationKind::Receive,
                limit,
                cancel,
                read_activated_conversation_replay(self.context.repository(), subscription),
            )
            .await
            .map_err(|error| map_deadline(&error, DeliveryStageError::Replay))?;

            let stamp = stamp().map_err(|error| DeadlineError::Peer {
                operation: OperationKind::Send,
                error,
            })?;
            let writer = self
                .writer
                .take()
                .ok_or_else(|| delivery_failure(OperationKind::Send, DeliveryStageError::Writer))?;
            let delivered = run_with_deadline(
                OperationKind::Send,
                limit,
                cancel,
                writer.deliver(stamp, replay),
            )
            .await
            .map_err(map_writer_deadline)?;
            let (writer, delivered) = delivered;
            self.writer = Some(writer);

            match delivered {
                ConversationReplayDelivery::Current {
                    subscription: activated,
                    cursor,
                } => {
                    subscription = activated;
                    subscription.advance_to(cursor);
                    return Ok(subscription);
                }
                ConversationReplayDelivery::Published {
                    subscription: activated,
                    cursor,
                } => {
                    subscription = activated;
                    subscription.advance_to(cursor);
                }
                ConversationReplayDelivery::ResnapshotRequired { .. } => {
                    return Err(delivery_failure(
                        OperationKind::Receive,
                        DeliveryStageError::ResnapshotRequired,
                    ));
                }
            }
        }
    }

    fn finish_writer(&mut self) -> Result<(), DeliveryStageError> {
        let Some(writer) = self.writer.take() else {
            return Ok(());
        };
        writer.finish().map_err(|error| match error {
            ConversationDeliveryError::Finish(_) => DeliveryStageError::Finish,
            ConversationDeliveryError::Open(_)
            | ConversationDeliveryError::Send(_)
            | ConversationDeliveryError::Registry(_)
            | ConversationDeliveryError::ObservationRegistry(_) => DeliveryStageError::Writer,
        })
    }
}

/// Reads one thread's complete message outbox: every queued or dispatching
/// message (oldest first) and the failures still offered to the user.
async fn read_message_outbox(
    repository: &Repository,
    thread_id: &ThreadId,
) -> Result<MessageOutbox, QueuedMessageRepositoryError> {
    let queued = repository
        .read_queued_messages(
            ListQueuedMessages::new(
                thread_id.clone(),
                QueuedMessageListOrder::OldestFirst,
                QUEUED_MESSAGE_LIST_MAX,
            )
            .map_err(QueuedMessageRepositoryError::InvalidListLimit)?,
        )
        .await?;
    let failed = repository
        .read_failed_messages(
            ListFailedMessages::new(thread_id.clone(), FAILED_MESSAGE_LIST_MAX)
                .map_err(QueuedMessageRepositoryError::InvalidFailedListLimit)?,
        )
        .await?;
    MessageOutbox::new(queued, failed).map_err(|_| QueuedMessageRepositoryError::Invariant {
        reason: "message outbox listings name different threads",
    })
}

fn delivery_failure(
    operation: OperationKind,
    error: DeliveryStageError,
) -> DeadlineError<RequestStageError> {
    DeadlineError::Peer {
        operation,
        error: RequestStageError::Delivery(error),
    }
}

fn map_deadline<T>(
    error: &DeadlineError<T>,
    delivery_error: DeliveryStageError,
) -> DeadlineError<RequestStageError> {
    match error {
        DeadlineError::Timeout { operation, limit } => DeadlineError::Timeout {
            operation: *operation,
            limit: *limit,
        },
        DeadlineError::Cancelled { operation } => DeadlineError::Cancelled {
            operation: *operation,
        },
        DeadlineError::InvalidLimit { operation } => DeadlineError::InvalidLimit {
            operation: *operation,
        },
        DeadlineError::Peer { operation, .. } => delivery_failure(*operation, delivery_error),
    }
}

fn map_writer_deadline(
    error: DeadlineError<ConversationDeliveryError>,
) -> DeadlineError<RequestStageError> {
    match error {
        DeadlineError::Timeout { operation, limit } => DeadlineError::Timeout { operation, limit },
        DeadlineError::Cancelled { operation } => DeadlineError::Cancelled { operation },
        DeadlineError::InvalidLimit { operation } => DeadlineError::InvalidLimit { operation },
        DeadlineError::Peer { operation, error } => {
            let stage = match error {
                ConversationDeliveryError::Registry(_)
                | ConversationDeliveryError::ObservationRegistry(_) => DeliveryStageError::Registry,
                ConversationDeliveryError::Finish(_) => DeliveryStageError::Finish,
                ConversationDeliveryError::Open(_) | ConversationDeliveryError::Send(_) => {
                    DeliveryStageError::Writer
                }
            };
            delivery_failure(operation, stage)
        }
    }
}
