//! Serialized connection-owned conversation subscription delivery.
//!
//! The driver retains one fresh subscription context, one process-wide wake
//! receiver, one delivery writer, and at most one activated subscription per
//! thread. It never spawns work or queues patch data: every wake performs a
//! bounded authoritative re-read, and the writer advances the connection
//! registry only after the corresponding batch has crossed the wire.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::time::Duration;

use artisan_transport::{CancelHandle, DeadlineError, OperationKind, run_with_deadline};

use crate::activated_conversation_replay::read_activated_conversation_replay;
use crate::connection::{
    DeliveryStageError, RequestDispatchOutcome, RequestStageError, ServerFrameStamp,
};
use crate::conversation_commit_notifier::{
    ConversationCommitAnySubscription, ConversationCommitWaitError,
};
use crate::conversation_delivery_writer::{
    ConversationDeliveryError, ConversationDeliveryWriter, ConversationReplayDelivery,
};
use crate::request_handler::{ActivatedConversationSubscription, ConversationConnectionContext};

/// One serialized conversation delivery owner for an authenticated
/// connection.
#[derive(Debug)]
pub(crate) struct ConversationDeliveryDriver {
    context: ConversationConnectionContext,
    writer: Option<ConversationDeliveryWriter>,
    wake: ConversationCommitAnySubscription,
    active: BTreeMap<artisan_domain::ThreadId, ActivatedConversationSubscription>,
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
    /// activated cursor before the state is retained.
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
        if let Some(thread_id) = outcome.stopped_thread {
            self.active.remove(&thread_id);
        }

        let Some(subscription) = outcome.activation else {
            return Ok(());
        };

        let thread_id = subscription.lease().thread_id().clone();
        self.active.remove(&thread_id);
        let subscription = self
            .deliver_until_current(subscription, stamp, limit, cancel)
            .await?;
        self.active.insert(thread_id, subscription);
        Ok(())
    }

    /// Re-reads every active subscription once after a coalesced process-wide
    /// wake. A successful batch advances its state and the next read uses the
    /// cursor returned by the writer, so repeated wakes cannot duplicate it.
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
            self.active.insert(thread_id, subscription);
        }
        Ok(())
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
            | ConversationDeliveryError::Registry(_) => DeliveryStageError::Writer,
        })
    }
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
                ConversationDeliveryError::Registry(_) => DeliveryStageError::Registry,
                ConversationDeliveryError::Finish(_) => DeliveryStageError::Finish,
                ConversationDeliveryError::Open(_) | ConversationDeliveryError::Send(_) => {
                    DeliveryStageError::Writer
                }
            };
            delivery_failure(operation, stage)
        }
    }
}
