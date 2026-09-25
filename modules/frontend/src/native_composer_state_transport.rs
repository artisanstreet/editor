//! Transport-thread child for queue withdrawals, failed-message retries, and
//! run-usage reads.
//!
//! The parent `native_transport_service` remains the owner of the session,
//! frame factory, reconnect policy, and public transport enums. Root mounts
//! this file as a child module so it can use the existing private
//! `ServiceRuntime`, `StableMutation`, `ExpectedResponse`, `publish`, and
//! `durable_save_request` seams without widening those internals.
//!
//! The queued and failed rows themselves are never read here: the Forge
//! pushes the thread's message outbox over the conversation subscription.
//! No command carries a message payload; retries and withdrawals name the
//! Forge's stored message by identity.

#![forbid(unsafe_code)]
#![allow(clippy::large_enum_variant)]
#![allow(clippy::module_name_repetitions)]

use artisan_domain::{
    Command, FailedMessageRetried, QueuedMessageWithdrawalResult, RetryFailedMessage,
    RunUsageResult, WithdrawQueuedMessageCommand,
};
use artisan_protocol::ResponsePayload;

use super::*;

/// One bounded command sent from the application thread to the service child.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComposerStateCommand {
    /// Read usage for a settled footer, independently of the composer model.
    ReadFooterUsage {
        /// Exact immutable thread and run scope.
        query: artisan_domain::ReadRunUsage,
    },
    /// Withdraw one exact queued message. The command id is stable across a
    /// deliberate transport retry.
    WithdrawQueuedMessage {
        /// Application generation of the controls row.
        generation: u64,
        /// Exact durable withdrawal command.
        command: Box<WithdrawQueuedMessageCommand>,
    },
    /// Ask the Forge to dispatch one failed message again from its stored
    /// payload. The command id is stable across a deliberate transport retry.
    RetryFailedMessage {
        /// Application generation of the failure row.
        generation: u64,
        /// Exact durable retry command.
        command: Box<RetryFailedMessage>,
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
    /// Optional exact-run footer measurement; failures remain absent.
    FooterUsage {
        /// Immutable requested scope.
        query: artisan_domain::ReadRunUsage,
        /// No result means the read failed.
        result: Option<RunUsageResult>,
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
    /// The Forge answered a failed-message retry.
    FailedMessageRetried {
        /// Generation of the failure row.
        generation: u64,
        /// The Forge's answer.
        result: FailedMessageRetried,
    },
    /// A failed-message retry could not reach the Forge.
    FailedMessageRetryFailed {
        /// Generation of the failure row.
        generation: u64,
        /// Exact command that failed.
        command: Box<RetryFailedMessage>,
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
        ComposerStateCommand::WithdrawQueuedMessage {
            generation,
            command,
        } => withdraw_queued_message(runtime, frames, events, generation, *command).await,
        ComposerStateCommand::RetryFailedMessage {
            generation,
            command,
        } => retry_failed_message(runtime, frames, events, generation, *command).await,
        ComposerStateCommand::ReadFooterUsage { query } => {
            let result = query_run_usage(runtime, frames, &query).await.ok();
            publish_composer_event(events, ComposerStateEvent::FooterUsage { query, result })
        }
        ComposerStateCommand::ReadRunUsage {
            generation,
            sequence,
            query,
        } => read_run_usage(runtime, frames, events, generation, sequence, query).await,
    }
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

async fn retry_failed_message(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    generation: u64,
    command: RetryFailedMessage,
) -> Result<(), ServiceFailure> {
    let failed = |failure| ComposerStateEvent::FailedMessageRetryFailed {
        generation,
        command: Box::new(command.clone()),
        failure,
    };
    if known_thread_for_queue(&runtime.known_threads, &command.target.thread_id).is_err() {
        return publish_composer_event(
            events,
            failed(ServiceFailure::invalid(ServiceFailureStage::Request)),
        );
    }
    let mutation = match stable_mutation(
        &command.request_id,
        Command::RetryFailedMessage(command.clone()),
    ) {
        Ok(mutation) => mutation,
        Err(failure) => return publish_composer_event(events, failed(failure)),
    };
    let payload = match durable_save_request(
        runtime,
        frames,
        &mutation,
        ExpectedResponse::FailedMessageRetried {
            request_id: command.request_id.clone(),
        },
    )
    .await
    {
        Ok(payload) => payload,
        Err(error) => return publish_composer_event(events, failed(error.into())),
    };
    match payload {
        ResponsePayload::FailedMessageRetried(result) if result.target == command.target => {
            publish_composer_event(
                events,
                ComposerStateEvent::FailedMessageRetried { generation, result },
            )
        }
        _ => publish_composer_event(
            events,
            failed(ServiceFailure::new(
                ServiceFailureStage::Request,
                ServiceFailureCategory::Integrity,
            )),
        ),
    }
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
    stable_mutation(&request_id, Command::WithdrawQueuedMessage(command))
}

/// Frames one idempotent command under its own request id, so a retried
/// frame is answered by the Forge's durable receipt.
pub(super) fn stable_mutation(
    request_id: &RequestId,
    command: Command,
) -> Result<StableMutation, ServiceFailure> {
    let frame_id = FrameId::parse(request_id.as_str().to_owned())
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    let frame_request_id = frame_id
        .to_request_id()
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    if &frame_request_id != request_id || command.request_id() != request_id {
        return Err(ServiceFailure::invalid(ServiceFailureStage::Request));
    }
    let sent_at =
        real_unix_millis().map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    Ok(StableMutation {
        frame_id,
        sent_at,
        command,
    })
}

fn publish_composer_event(
    events: &SyncSender<NativeTransportEvent>,
    event: ComposerStateEvent,
) -> Result<(), ServiceFailure> {
    publish(events, NativeTransportEvent::ComposerState(event))
}
