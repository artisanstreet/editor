//! Serial server-side publication of conversation patch batches.
//!
//! This leaf owns one optional server-initiated unidirectional stream. It
//! opens that stream only for a real replay batch, sends the batch before
//! advancing the connection-local subscription registrar, and returns its
//! ownership only after the complete operation succeeds. Queueing,
//! notification, and connection-driver composition belong to later layers.

#![forbid(unsafe_code)]

use artisan_domain::{ConversationCursor, EngineObservationEvent, Event, ThreadId};
use artisan_protocol::{EventCursor, ProtocolVersion, ServerEvent, WireEnvelope, WireEnvelopeBody};
use artisan_transport::EnvelopeSendError;
use quinn::{ClosedStream, Connection, SendStream, VarInt};
use thiserror::Error;

use crate::ServerFrameStamp;
use crate::activated_conversation_replay::ActivatedConversationReplay;
use crate::conversation_subscription_registry::{
    ApplyBatchError, ApplyObservationBatchError, SubscriptionLease,
};
use crate::request_handler::{
    ActivatedConversationSubscription, ConversationSubscriptionRegistrar,
};

/// Fixed secret-free application code used when abandoning an unfinished
/// server-owned delivery stream.
const OUTBOUND_RESET_CODE: VarInt = VarInt::from_u32(0x01);

/// Result of delivering one accepted replay outcome.
#[must_use]
#[derive(Debug, Eq, PartialEq)]
pub enum ConversationReplayDelivery {
    /// The subscription was already at the durable tail.
    Current {
        /// The original activated subscription authority.
        subscription: ActivatedConversationSubscription,
        /// The exact durable tail cursor reported by the replay.
        cursor: ConversationCursor,
    },
    /// The replay batch crossed the wire and the registrar accepted it.
    Published {
        /// The original activated subscription authority.
        subscription: ActivatedConversationSubscription,
        /// The exact cursor returned by the registrar after publication.
        cursor: ConversationCursor,
    },
    /// The requested cursor is beyond the durable tail and needs a fresh
    /// snapshot rather than a fabricated or retried batch.
    ResnapshotRequired {
        /// The original activated subscription authority.
        subscription: ActivatedConversationSubscription,
        /// The exact cursor requested by the replay read.
        requested_cursor: ConversationCursor,
        /// The exact current durable cursor from the replay read.
        current_cursor: ConversationCursor,
    },
}

/// Failure while opening, writing, advancing, or finishing conversation
/// delivery. Every variant preserves the concrete lower-layer source.
#[derive(Debug, Error)]
pub enum ConversationDeliveryError {
    /// The server-owned unidirectional stream could not be opened.
    #[error("opening the conversation delivery stream failed")]
    Open(#[from] quinn::ConnectionError),
    /// The patch-batch envelope could not be sent.
    #[error("sending the conversation patch batch failed")]
    Send(#[from] EnvelopeSendError),
    /// The registrar rejected the sent patch batch.
    #[error("recording the published conversation patch batch failed")]
    Registry(#[from] ApplyBatchError),
    /// The registrar rejected the sent observation batch.
    #[error("recording the published observation batch failed")]
    ObservationRegistry(#[from] ApplyObservationBatchError),
    /// The server-owned stream could not be finished.
    #[error("finishing the conversation delivery stream failed")]
    Finish(#[from] ClosedStream),
}

/// Result of delivering one committed engine-observation batch.
#[must_use]
#[derive(Clone, Debug, PartialEq)]
pub enum ObservationBatchDelivery {
    /// Every observation in the batch was already delivered to this
    /// subscriber; nothing crossed the wire and the registrar is untouched.
    Current {
        /// Last observation sequence delivered to the subscriber.
        observation_cursor: u64,
    },
    /// The new observations crossed the wire in durable sequence order and
    /// the registrar advanced past them.
    Published {
        /// One event per newly delivered observation, in send order.
        delivered: Vec<ServerEvent>,
        /// Subscriber observation cursor before this batch.
        from_sequence: u64,
        /// Maximum observation sequence delivered by this batch.
        to_sequence: u64,
    },
}

/// Serial owner of one server-side conversation delivery stream.
///
/// The stream is intentionally private. A caller can deliver another
/// accepted replay or finish the owner, but cannot write arbitrary envelopes,
/// reset the stream, or duplicate the registrar authority.
#[must_use]
#[derive(Debug)]
pub struct ConversationDeliveryWriter {
    connection: Connection,
    registrar: ConversationSubscriptionRegistrar,
    protocol_version: ProtocolVersion,
    stream: Option<DeliveryStream>,
    /// Next one-based per-connection event cursor minted for an observation
    /// event. Patch batches never consume this space; observation events
    /// share the connection's event sequence exactly like other server
    /// events.
    next_event_cursor: u64,
}

impl ConversationDeliveryWriter {
    /// Creates a writer without opening a stream or performing network I/O.
    pub fn new(
        connection: Connection,
        registrar: ConversationSubscriptionRegistrar,
        protocol_version: ProtocolVersion,
    ) -> Self {
        Self {
            connection,
            registrar,
            protocol_version,
            stream: None,
            next_event_cursor: 1,
        }
    }

    /// Delivers one accepted replay outcome in serial wire-before-registry
    /// order.
    ///
    /// A `Current` or `ResnapshotRequired` replay is returned without opening
    /// a stream or touching the registrar. A `Batch` opens one stream lazily,
    /// sends exactly one patch-batch envelope, and records the batch only
    /// after that send succeeds. Any failure consumes the writer; the
    /// unfinished stream guard resets its send direction when the operation
    /// is cancelled, fails, or is otherwise dropped.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationDeliveryError::Open`] when the lazy stream
    /// cannot be opened, [`ConversationDeliveryError::Send`] when the batch
    /// envelope cannot be sent, or [`ConversationDeliveryError::Registry`]
    /// when the registrar rejects the sent batch. Every such error consumes
    /// the writer.
    ///
    /// # Panics
    ///
    /// Panics only if the private stream invariant is violated between the
    /// lazy-open branch and the send operation.
    pub async fn deliver(
        mut self,
        stamp: ServerFrameStamp,
        replay: ActivatedConversationReplay,
    ) -> Result<(Self, ConversationReplayDelivery), ConversationDeliveryError> {
        let (subscription, replay) = replay.into_parts();

        match replay {
            artisan_database::ConversationPatchReplay::Current { cursor } => Ok((
                self,
                ConversationReplayDelivery::Current {
                    subscription,
                    cursor,
                },
            )),
            artisan_database::ConversationPatchReplay::ResnapshotRequired {
                requested_cursor,
                current_cursor,
            } => Ok((
                self,
                ConversationReplayDelivery::ResnapshotRequired {
                    subscription,
                    requested_cursor,
                    current_cursor,
                },
            )),
            artisan_database::ConversationPatchReplay::Batch(batch) => {
                if self.stream.is_none() {
                    let send = self.connection.open_uni().await?;
                    // Install before the first write await. If the caller
                    // cancels or the write fails, Drop can reset this exact
                    // stream direction synchronously.
                    self.stream = Some(DeliveryStream::new(send));
                }

                let envelope = WireEnvelope {
                    protocol_version: self.protocol_version,
                    frame_id: stamp.frame_id,
                    sent_at: stamp.sent_at,
                    body: WireEnvelopeBody::PatchBatch(batch),
                };
                // The stream is installed immediately above whenever absent,
                // and `self.stream` is private with no other removal path
                // before Drop stores the owner; the option is `Some` here.
                let stream = self
                    .stream
                    .as_mut()
                    .expect("a batch always installs a delivery stream");
                artisan_transport::send_envelope(&mut stream.send, &envelope).await?;

                let cursor = match &envelope.body {
                    WireEnvelopeBody::PatchBatch(batch) => {
                        self.registrar
                            .record_published_batch(subscription.lease(), batch)
                            .await?
                    }
                    _ => unreachable!("the delivery envelope always carries a patch batch"),
                };

                Ok((
                    self,
                    ConversationReplayDelivery::Published {
                        subscription,
                        cursor,
                    },
                ))
            }
        }
    }

    /// Delivers one durably committed engine-observation batch to a single
    /// thread subscriber in thread-scoped durable order.
    ///
    /// The batch must arrive in committed `delivery_sequence` order (the
    /// thread-scoped durable cursor carried by
    /// `EngineObservationEvent.attribution`); `Observation.sequence` stays
    /// run-local and is used only as a legacy fallback when `attribution` is
    /// `None` in unit fixtures. Rows at or below the subscriber's current
    /// observation cursor are skipped as already delivered, so replaying a
    /// committed batch is idempotent — including across two runs that reset
    /// their run-local sequences. A fully skipped batch returns
    /// [`ObservationBatchDelivery::Current`] without opening a stream or
    /// touching the registrar. Otherwise every new row is sent as its own
    /// [`Event::EngineObservation`] envelope (attribution preserved verbatim,
    /// never restamped) on the lazily opened delivery stream — shared with
    /// patch batches — before the registrar advances past the batch maximum.
    /// The full batch crosses the wire before the registrar advances; any
    /// failure consumes the writer and retains the cursor. Any failure
    /// consumes the writer; the unfinished stream guard resets its send
    /// direction when the operation is cancelled, fails, or is dropped.
    ///
    /// Thread authority is fenced: every event's `thread_id` must equal both
    /// the `thread_id` argument and the lease thread, and production events
    /// carry `Some` attribution. Legacy `None` attributions are accepted only
    /// for unit fixtures and order by `observation.sequence()`. Approval and
    /// question rows carry their provider `approval_id` and `question_id`
    /// untouched so the later A-approve packet can answer them; this method
    /// never responds to them. No clocks are stamped and no turn ids are cast
    /// here.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationDeliveryError::Open`] when the lazy stream
    /// cannot be opened, [`ConversationDeliveryError::Send`] when an
    /// observation envelope cannot be sent, or
    /// [`ConversationDeliveryError::ObservationRegistry`] when the registrar
    /// rejects the sent batch. Every such error consumes the writer.
    ///
    /// # Panics
    ///
    /// Panics only if the private stream invariant is violated between the
    /// lazy-open branch and a send operation, or if the per-connection
    /// event cursor wraps, which would require 2^64 delivered events.
    pub async fn deliver_observation_batch(
        mut self,
        lease: &SubscriptionLease,
        thread_id: ThreadId,
        batch: Vec<(ServerFrameStamp, EngineObservationEvent)>,
    ) -> Result<(Self, ObservationBatchDelivery), ConversationDeliveryError> {
        let from_sequence = self
            .registrar
            .subscription_view(&thread_id)
            .await
            .map_or(0, |view| view.observation_cursor());
        let mut pending: Vec<(ServerFrameStamp, EngineObservationEvent)> = Vec::new();
        for (stamp, event) in batch {
            if event.thread_id != thread_id || event.thread_id != *lease.thread_id() {
                continue;
            }
            if observation_delivery_sequence(&event) <= from_sequence {
                continue;
            }
            pending.push((stamp, event));
        }
        pending.sort_by_key(|(_, event)| observation_delivery_sequence(event));
        if pending.is_empty() {
            return Ok((
                self,
                ObservationBatchDelivery::Current {
                    observation_cursor: from_sequence,
                },
            ));
        }

        if self.stream.is_none() {
            let send = self.connection.open_uni().await?;
            // Install before the first write await. If the caller
            // cancels or the write fails, Drop can reset this exact
            // stream direction synchronously.
            self.stream = Some(DeliveryStream::new(send));
        }

        let mut delivered = Vec::with_capacity(pending.len());
        let mut to_sequence = from_sequence;
        for (stamp, event) in pending {
            // `next_event_cursor` starts at one and only saturating-adds, so
            // it never wraps to the rejected zero cursor.
            let cursor = EventCursor::new(self.next_event_cursor)
                .expect("a connection delivers fewer than 2^64 events");
            to_sequence = observation_delivery_sequence(&event).max(to_sequence);
            let event = ServerEvent {
                cursor,
                event: Event::EngineObservation(event),
            };
            let envelope = WireEnvelope {
                protocol_version: self.protocol_version,
                frame_id: stamp.frame_id,
                sent_at: stamp.sent_at,
                body: WireEnvelopeBody::Event(event.clone()),
            };
            // The shared stream is installed before this loop whenever absent
            // and is never removed while the writer is owned; the option is
            // `Some` for every send.
            let stream = self
                .stream
                .as_mut()
                .expect("an observation batch always installs a delivery stream");
            artisan_transport::send_envelope(&mut stream.send, &envelope).await?;
            self.next_event_cursor = self.next_event_cursor.saturating_add(1);
            delivered.push(event);
        }

        let to_sequence = self
            .registrar
            .record_published_observation_batch(lease, &thread_id, from_sequence, to_sequence)
            .await?;
        Ok((
            self,
            ObservationBatchDelivery::Published {
                delivered,
                from_sequence,
                to_sequence,
            },
        ))
    }

    /// Finishes the one opened stream, if any.
    ///
    /// No stream is opened for an owner that never published a batch. A
    /// finish failure is terminal and the unfinished-stream guard remains
    /// armed while the consumed owner is dropped.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationDeliveryError::Finish`] when Quinn rejects the
    /// stream finish. The consumed owner remains terminal in that case.
    pub fn finish(mut self) -> Result<(), ConversationDeliveryError> {
        let Some(stream) = self.stream.as_mut() else {
            return Ok(());
        };
        stream.send.finish()?;
        stream.finished = true;
        Ok(())
    }
}

/// Returns the thread-scoped durable cursor of one ledger event.
///
/// Production events carry `Some` attribution and order by
/// `attribution.delivery_sequence`, which is strictly increasing per thread
/// across runs. Legacy unit fixtures may carry `None` and order by the
/// run-local `observation.sequence()` instead; production must never emit
/// `None`.
fn observation_delivery_sequence(event: &EngineObservationEvent) -> u64 {
    event.attribution.as_ref().map_or_else(
        || event.observation.sequence().get(),
        |attribution| attribution.delivery_sequence,
    )
}

/// Private synchronous cleanup guard for the writer's outbound direction.
#[derive(Debug)]
struct DeliveryStream {
    send: SendStream,
    finished: bool,
}

impl DeliveryStream {
    const fn new(send: SendStream) -> Self {
        Self {
            send,
            finished: false,
        }
    }
}

impl Drop for DeliveryStream {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let _discarded = self.send.reset(OUTBOUND_RESET_CODE);
    }
}
