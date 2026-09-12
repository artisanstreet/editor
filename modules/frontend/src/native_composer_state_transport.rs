//! Transport-thread child for queued-message and run-usage state reads.
//!
//! The parent `native_transport_service` remains the owner of the session,
//! frame factory, reconnect policy, and public transport enums. Root mounts
//! this file as a child module so it can use the existing private
//! `ServiceRuntime`, `StableMutation`, `ExpectedResponse`, `publish`, and
//! `durable_save_request` seams without widening those internals.
//!
//! No byte-bearing queue payload is requested by either listing path. Only an
//! authoritative edit withdrawal followed by a matching `Withdrawn` receipt,
//! or an explicit new-chat recovery over a terminally failed row, may issue
//! the recalled-message read.

#![forbid(unsafe_code)]
#![allow(clippy::large_enum_variant)]
#![allow(clippy::module_name_repetitions)]

use artisan_domain::{
    Command, FailedMessageListing, ListFailedMessages, ListQueuedMessages, QueuedMessageListOrder,
    QueuedMessageListing, QueuedMessageWithdrawalResult, ReadRecalledMessage,
    RecalledMessageResult, RunUsageResult, ThreadId, WithdrawQueuedMessageCommand,
};
use artisan_protocol::ResponsePayload;

use super::*;

/// Maximum rows any queue read may request or return.
pub(crate) const COMPOSER_STATE_READ_LIMIT: usize = artisan_domain::QUEUED_MESSAGE_LIST_MAX;

/// One bounded command sent from the application thread to the service child.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComposerStateCommand {
    /// Read one bounded, byte-free queue page for the mounted scope.
    ListQueuedMessages {
        /// Thread whose still-queued rows are requested.
        thread_id: ThreadId,
        /// Application generation used to fence the response.
        generation: u64,
        /// Stable order for the returned rows.
        order: QueuedMessageListOrder,
        /// Page size, bounded by the domain constant.
        limit: usize,
    },
    /// Read one bounded, byte-free failed-dispatch page for the mounted scope.
    ListFailedMessages {
        /// Thread whose terminally failed rows are requested.
        thread_id: ThreadId,
        /// Application generation used to fence the response.
        generation: u64,
        /// Page size, bounded by the failed-listing domain constant.
        limit: usize,
    },
    /// Withdraw one exact queued message. The command id is stable across a
    /// deliberate transport retry.
    WithdrawQueuedMessage {
        /// Application generation of the controls row.
        generation: u64,
        /// Exact durable withdrawal command.
        command: Box<WithdrawQueuedMessageCommand>,
    },
    /// Read one exact payload after an authoritative `Withdrawn` receipt.
    ReadRecalledMessage {
        /// Application generation that captured the recall target.
        generation: u64,
        /// Exact thread/message/original-request scope.
        query: ReadRecalledMessage,
    },
    /// Read immutable usage for one exact reporting run.
    ReadRunUsage {
        /// Application generation that owns the run surface.
        generation: u64,
        /// Monotonic application read sequence.
        sequence: u64,
        /// Exact thread/run scope.
        query: artisan_domain::ReadRunUsage,
    },
}

/// One bounded event returned by the service child.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComposerStateEvent {
    /// Authoritative byte-free queue listing.
    QueuedMessages {
        /// Thread requested by the application.
        thread_id: ThreadId,
        /// Generation carried by the request.
        generation: u64,
        /// Exact listing and count from Forge.
        listing: QueuedMessageListing,
    },
    /// Queue listing request failed; no local placeholder rows are implied.
    QueuedMessagesFailed {
        /// Thread requested by the application.
        thread_id: ThreadId,
        /// Generation carried by the request.
        generation: u64,
        /// Redacted request failure.
        failure: ServiceFailure,
    },
    /// Authoritative byte-free failed-dispatch listing, newest failures first.
    FailedMessages {
        /// Thread requested by the application.
        thread_id: ThreadId,
        /// Generation carried by the request.
        generation: u64,
        /// Exact listing and count from Forge.
        listing: FailedMessageListing,
    },
    /// Failed-dispatch listing request failed; no local placeholder rows are implied.
    FailedMessagesFailed {
        /// Thread requested by the application.
        thread_id: ThreadId,
        /// Generation carried by the request.
        generation: u64,
        /// Redacted request failure.
        failure: ServiceFailure,
    },
    /// Authoritative withdrawal receipt for one exact command.
    MessageWithdrawn {
        /// Generation of the controls row.
        generation: u64,
        /// Command identity retained for application-side ownership checks.
        command: Box<WithdrawQueuedMessageCommand>,
        /// Forge withdrawal disposition and receipt.
        result: QueuedMessageWithdrawalResult,
    },
    /// Withdrawal failed; the application retains the exact command for an
    /// explicit retry if its policy permits one.
    MessageWithdrawalFailed {
        /// Generation of the controls row.
        generation: u64,
        /// Exact command that failed.
        command: Box<WithdrawQueuedMessageCommand>,
        /// Redacted request failure.
        failure: ServiceFailure,
    },
    /// Exact byte-bearing recalled payload result.
    RecalledMessage {
        /// Generation that captured the target.
        generation: u64,
        /// Exact read scope.
        query: ReadRecalledMessage,
        /// Boxed to keep the event enum's stack footprint bounded.
        result: Box<RecalledMessageResult>,
    },
    /// Recalled payload read failed; no payload is fabricated.
    RecalledMessageFailed {
        /// Generation that captured the target.
        generation: u64,
        /// Exact read scope.
        query: ReadRecalledMessage,
        /// Redacted request failure.
        failure: ServiceFailure,
    },
    /// Exact immutable run-usage result.
    RunUsage {
        /// Generation that owns the read.
        generation: u64,
        /// Application read sequence.
        sequence: u64,
        /// Exact read scope.
        query: artisan_domain::ReadRunUsage,
        /// Report is optional; absent values remain absent.
        result: RunUsageResult,
    },
    /// Usage read failed; no catalog fallback is implied.
    RunUsageFailed {
        /// Generation that owns the read.
        generation: u64,
        /// Application read sequence.
        sequence: u64,
        /// Exact read scope.
        query: artisan_domain::ReadRunUsage,
        /// Redacted request failure.
        failure: ServiceFailure,
    },
}

/// Handles one child command on the existing authenticated service runtime.
///
/// The parent command loop should call this from its one added
/// `NativeTransportCommand::ComposerState(command)` arm and wrap every event
/// emitted here in its one added
/// `NativeTransportEvent::ComposerState(event)` variant.
pub(super) async fn handle_composer_state_command(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    command: ComposerStateCommand,
) -> Result<(), ServiceFailure> {
    match command {
        ComposerStateCommand::ListQueuedMessages {
            thread_id,
            generation,
            order,
            limit,
        } => {
            list_queued_messages(runtime, frames, events, thread_id, generation, order, limit).await
        }
        ComposerStateCommand::ListFailedMessages {
            thread_id,
            generation,
            limit,
        } => list_failed_messages(runtime, frames, events, thread_id, generation, limit).await,
        ComposerStateCommand::WithdrawQueuedMessage {
            generation,
            command,
        } => withdraw_queued_message(runtime, frames, events, generation, *command).await,
        ComposerStateCommand::ReadRecalledMessage { generation, query } => {
            read_recalled_message(runtime, frames, events, generation, query).await
        }
        ComposerStateCommand::ReadRunUsage {
            generation,
            sequence,
            query,
        } => read_run_usage(runtime, frames, events, generation, sequence, query).await,
    }
}

async fn list_queued_messages(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    thread_id: ThreadId,
    generation: u64,
    order: QueuedMessageListOrder,
    limit: usize,
) -> Result<(), ServiceFailure> {
    if limit > COMPOSER_STATE_READ_LIMIT {
        return publish_composer_event(
            events,
            ComposerStateEvent::QueuedMessagesFailed {
                thread_id,
                generation,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        );
    }
    if known_thread_for_queue(&runtime.known_threads, &thread_id).is_err() {
        return publish_composer_event(
            events,
            ComposerStateEvent::QueuedMessagesFailed {
                thread_id,
                generation,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        );
    }
    let Ok(query) = ListQueuedMessages::new(thread_id.clone(), order, limit) else {
        return publish_composer_event(
            events,
            ComposerStateEvent::QueuedMessagesFailed {
                thread_id,
                generation,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        );
    };
    let payload = match runtime
        .request(
            frames,
            query_request(Query::ListQueuedMessages(query)),
            ExpectedResponse::QueuedMessages {
                thread_id: thread_id.clone(),
            },
        )
        .await
    {
        Ok(payload) => payload,
        Err(error) => {
            return publish_composer_event(
                events,
                ComposerStateEvent::QueuedMessagesFailed {
                    thread_id,
                    generation,
                    failure: error.into(),
                },
            );
        }
    };
    let ResponsePayload::QueuedMessages(listing) = payload else {
        return publish_composer_event(
            events,
            ComposerStateEvent::QueuedMessagesFailed {
                thread_id,
                generation,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        );
    };
    if listing.thread_id() != &thread_id {
        return publish_composer_event(
            events,
            ComposerStateEvent::QueuedMessagesFailed {
                thread_id,
                generation,
                failure: ServiceFailure::new(
                    ServiceFailureStage::Request,
                    ServiceFailureCategory::Integrity,
                ),
            },
        );
    }
    publish_composer_event(
        events,
        ComposerStateEvent::QueuedMessages {
            thread_id,
            generation,
            listing,
        },
    )
}

async fn list_failed_messages(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    thread_id: ThreadId,
    generation: u64,
    limit: usize,
) -> Result<(), ServiceFailure> {
    if limit > COMPOSER_STATE_READ_LIMIT {
        return publish_composer_event(
            events,
            ComposerStateEvent::FailedMessagesFailed {
                thread_id,
                generation,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        );
    }
    if known_thread_for_queue(&runtime.known_threads, &thread_id).is_err() {
        return publish_composer_event(
            events,
            ComposerStateEvent::FailedMessagesFailed {
                thread_id,
                generation,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        );
    }
    let Ok(query) = ListFailedMessages::new(thread_id.clone(), limit) else {
        return publish_composer_event(
            events,
            ComposerStateEvent::FailedMessagesFailed {
                thread_id,
                generation,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        );
    };
    let payload = match runtime
        .request(
            frames,
            query_request(Query::ListFailedMessages(query)),
            ExpectedResponse::FailedMessages {
                thread_id: thread_id.clone(),
            },
        )
        .await
    {
        Ok(payload) => payload,
        Err(error) => {
            return publish_composer_event(
                events,
                ComposerStateEvent::FailedMessagesFailed {
                    thread_id,
                    generation,
                    failure: error.into(),
                },
            );
        }
    };
    let ResponsePayload::FailedMessages(listing) = payload else {
        return publish_composer_event(
            events,
            ComposerStateEvent::FailedMessagesFailed {
                thread_id,
                generation,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        );
    };
    if listing.thread_id() != &thread_id {
        return publish_composer_event(
            events,
            ComposerStateEvent::FailedMessagesFailed {
                thread_id,
                generation,
                failure: ServiceFailure::new(
                    ServiceFailureStage::Request,
                    ServiceFailureCategory::Integrity,
                ),
            },
        );
    }
    publish_composer_event(
        events,
        ComposerStateEvent::FailedMessages {
            thread_id,
            generation,
            listing,
        },
    )
}

async fn withdraw_queued_message(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    generation: u64,
    command: WithdrawQueuedMessageCommand,
) -> Result<(), ServiceFailure> {
    let thread_id = command.thread_id.clone();
    let command_for_event = Box::new(command.clone());
    if known_thread_for_queue(&runtime.known_threads, &thread_id).is_err() {
        return publish_composer_event(
            events,
            ComposerStateEvent::MessageWithdrawalFailed {
                generation,
                command: command_for_event,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        );
    }
    let mutation = match composer_state_stable_mutation(command.clone()) {
        Ok(mutation) => mutation,
        Err(failure) => {
            return publish_composer_event(
                events,
                ComposerStateEvent::MessageWithdrawalFailed {
                    generation,
                    command: command_for_event,
                    failure,
                },
            );
        }
    };
    let request_id = command.request_id.clone();
    let message_id = command.message_id.clone();
    let original_request_id = command.original_request_id.clone();
    let payload = match durable_save_request(
        runtime,
        frames,
        &mutation,
        ExpectedResponse::MessageWithdrawn {
            thread_id: thread_id.clone(),
            message_id: message_id.clone(),
            original_request_id: original_request_id.clone(),
            request_id: request_id.clone(),
        },
    )
    .await
    {
        Ok(payload) => payload,
        Err(error) => {
            return publish_composer_event(
                events,
                ComposerStateEvent::MessageWithdrawalFailed {
                    generation,
                    command: command_for_event,
                    failure: error.into(),
                },
            );
        }
    };
    let ResponsePayload::MessageWithdrawn(result) = payload else {
        return publish_composer_event(
            events,
            ComposerStateEvent::MessageWithdrawalFailed {
                generation,
                command: command_for_event,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        );
    };
    if result.withdrawal_request_id() != &request_id
        || result.thread_id != thread_id
        || result.message_id != message_id
        || result.original_request_id != original_request_id
    {
        return publish_composer_event(
            events,
            ComposerStateEvent::MessageWithdrawalFailed {
                generation,
                command: command_for_event,
                failure: ServiceFailure::new(
                    ServiceFailureStage::Request,
                    ServiceFailureCategory::Integrity,
                ),
            },
        );
    }
    publish_composer_event(
        events,
        ComposerStateEvent::MessageWithdrawn {
            generation,
            command: command_for_event,
            result,
        },
    )
}

async fn read_recalled_message(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    generation: u64,
    query: ReadRecalledMessage,
) -> Result<(), ServiceFailure> {
    let thread_id = query.thread_id.clone();
    let query_for_event = query.clone();
    if known_thread_for_queue(&runtime.known_threads, &thread_id).is_err() {
        return publish_composer_event(
            events,
            ComposerStateEvent::RecalledMessageFailed {
                generation,
                query: query_for_event,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        );
    }
    let payload = match runtime
        .request(
            frames,
            query_request(Query::ReadRecalledMessage(query.clone())),
            ExpectedResponse::RecalledMessage {
                thread_id: query.thread_id.clone(),
                message_id: query.message_id.clone(),
                original_request_id: query.original_request_id.clone(),
            },
        )
        .await
    {
        Ok(payload) => payload,
        Err(error) => {
            return publish_composer_event(
                events,
                ComposerStateEvent::RecalledMessageFailed {
                    generation,
                    query: query_for_event,
                    failure: error.into(),
                },
            );
        }
    };
    let ResponsePayload::RecalledMessage(result) = payload else {
        return publish_composer_event(
            events,
            ComposerStateEvent::RecalledMessageFailed {
                generation,
                query: query_for_event,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request),
            },
        );
    };
    if result.thread_id != query.thread_id
        || result.message_id != query.message_id
        || result.original_request_id != query.original_request_id
    {
        return publish_composer_event(
            events,
            ComposerStateEvent::RecalledMessageFailed {
                generation,
                query: query_for_event,
                failure: ServiceFailure::new(
                    ServiceFailureStage::Request,
                    ServiceFailureCategory::Integrity,
                ),
            },
        );
    }
    publish_composer_event(
        events,
        ComposerStateEvent::RecalledMessage {
            generation,
            query,
            result: Box::new(result),
        },
    )
}

async fn read_run_usage(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    generation: u64,
    sequence: u64,
    query: artisan_domain::ReadRunUsage,
) -> Result<(), ServiceFailure> {
    let event = match query_run_usage(runtime, frames, &query).await {
        Ok(result) => ComposerStateEvent::RunUsage {
            generation,
            sequence,
            query,
            result,
        },
        Err(failure) => ComposerStateEvent::RunUsageFailed {
            generation,
            sequence,
            query,
            failure,
        },
    };
    publish_composer_event(events, event)
}

async fn query_run_usage(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    query: &artisan_domain::ReadRunUsage,
) -> Result<RunUsageResult, ServiceFailure> {
    known_thread_for_queue(&runtime.known_threads, &query.thread_id)
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    let payload = runtime
        .request(
            frames,
            query_request(Query::ReadRunUsage(query.clone())),
            ExpectedResponse::RunUsage {
                thread_id: query.thread_id.clone(),
                run_id: query.run_id.clone(),
            },
        )
        .await
        .map_err(ServiceFailure::from)?;
    let ResponsePayload::RunUsage(result) = payload else {
        return Err(ServiceFailure::invalid(ServiceFailureStage::Request));
    };
    if result.thread_id != query.thread_id || result.run_id != query.run_id {
        return Err(ServiceFailure::new(
            ServiceFailureStage::Request,
            ServiceFailureCategory::Integrity,
        ));
    }
    Ok(result)
}

fn composer_state_stable_mutation(
    command: WithdrawQueuedMessageCommand,
) -> Result<StableMutation, ServiceFailure> {
    let request_id = command.request_id.clone();
    let frame_id = FrameId::parse(request_id.as_str().to_owned())
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    let frame_request_id = frame_id
        .to_request_id()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    if frame_request_id != request_id || command.request_id != request_id {
        return Err(ServiceFailure::invalid(ServiceFailureStage::Request));
    }
    let sent_at =
        real_unix_millis().map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    Ok(StableMutation {
        frame_id,
        sent_at,
        command: Command::WithdrawQueuedMessage(command),
    })
}

fn publish_composer_event(
    events: &SyncSender<NativeTransportEvent>,
    event: ComposerStateEvent,
) -> Result<(), ServiceFailure> {
    publish(events, NativeTransportEvent::ComposerState(event))
}
