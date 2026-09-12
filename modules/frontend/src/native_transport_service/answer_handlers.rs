//! Approval and question answer handlers for the native transport service.
//!
//! Extracted from `handlers.rs` when the answer outcomes gained typed
//! gate-pairing failures, so each complete answer flow keeps one reviewable
//! home and the shared request-handler file stays within the size fence.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use super::handlers::{durable_save_request, known_thread_for_queue};
use super::*;

pub(super) async fn respond_approval(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    command: RespondApproval,
) -> Result<(), ServiceFailure> {
    let thread_id = command.thread_id().clone();
    let request_id = command.request_id().clone();
    if known_thread_for_queue(&runtime.known_threads, &thread_id).is_err() {
        return publish(
            events,
            NativeTransportEvent::ApprovalFailed {
                command,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request).into(),
            },
        );
    }
    let mutation = match approval_stable_mutation(command.clone()) {
        Ok(mutation) => mutation,
        Err(failure) => {
            return publish(
                events,
                NativeTransportEvent::ApprovalFailed {
                    command,
                    failure: failure.into(),
                },
            );
        }
    };
    let payload = match durable_save_request(
        runtime,
        frames,
        &mutation,
        ExpectedResponse::ApprovalAnswered {
            thread_id: thread_id.clone(),
            request_id: request_id.clone(),
        },
    )
    .await
    {
        Ok(payload) => payload,
        Err(error) => {
            return publish(
                events,
                NativeTransportEvent::ApprovalFailed {
                    command,
                    failure: error.into(),
                },
            );
        }
    };
    let ResponsePayload::ApprovalResponse(receipt) = payload else {
        return publish(
            events,
            NativeTransportEvent::ApprovalFailed {
                command,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request).into(),
            },
        );
    };
    if receipt.thread_id != thread_id || receipt.request_id != request_id {
        return publish(
            events,
            NativeTransportEvent::ApprovalFailed {
                command,
                failure: ServiceFailure::new(
                    ServiceFailureStage::Request,
                    ServiceFailureCategory::Integrity,
                )
                .into(),
            },
        );
    }
    publish(
        events,
        NativeTransportEvent::ApprovalAnswered { command, receipt },
    )
}

pub(super) async fn respond_question(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    command: RespondQuestion,
) -> Result<(), ServiceFailure> {
    let thread_id = command.thread_id().clone();
    let request_id = command.request_id().clone();
    if known_thread_for_queue(&runtime.known_threads, &thread_id).is_err() {
        return publish(
            events,
            NativeTransportEvent::QuestionFailed {
                command,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request).into(),
            },
        );
    }
    let mutation = match question_stable_mutation(command.clone()) {
        Ok(mutation) => mutation,
        Err(failure) => {
            return publish(
                events,
                NativeTransportEvent::QuestionFailed {
                    command,
                    failure: failure.into(),
                },
            );
        }
    };
    let payload = match durable_save_request(
        runtime,
        frames,
        &mutation,
        ExpectedResponse::QuestionAnswered {
            thread_id: thread_id.clone(),
            request_id: request_id.clone(),
        },
    )
    .await
    {
        Ok(payload) => payload,
        Err(error) => {
            return publish(
                events,
                NativeTransportEvent::QuestionFailed {
                    command,
                    failure: error.into(),
                },
            );
        }
    };
    let ResponsePayload::QuestionResponse(receipt) = payload else {
        return publish(
            events,
            NativeTransportEvent::QuestionFailed {
                command,
                failure: ServiceFailure::invalid(ServiceFailureStage::Request).into(),
            },
        );
    };
    if receipt.thread_id != thread_id || receipt.request_id != request_id {
        return publish(
            events,
            NativeTransportEvent::QuestionFailed {
                command,
                failure: ServiceFailure::new(
                    ServiceFailureStage::Request,
                    ServiceFailureCategory::Integrity,
                )
                .into(),
            },
        );
    }
    publish(
        events,
        NativeTransportEvent::QuestionAnswered { command, receipt },
    )
}
