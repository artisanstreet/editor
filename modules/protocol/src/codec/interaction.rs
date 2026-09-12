//! Run-interaction requests, receipts, usage, and favorites codec.
//!
//! Owns queue/stop/approval/question request decode, run-interaction and
//! receipt encode/decode, lifecycle response decode, and account-usage and
//! model-favorites decode. Receipts verify nested request identity before
//! returning an owned value.

#[allow(clippy::wildcard_imports)]
use super::*;

pub(crate) fn decode_lifecycle_request(
    value: artisan_capnp::lifecycle_request::Reader<'_>,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let request = match value.which()? {
        lifecycle_request::Which::Status(status) => {
            status?;
            LifecycleRequest::Status
        }
        lifecycle_request::Which::Stop(stop) => LifecycleRequest::Stop {
            require_idle: stop?.get_require_idle(),
        },
    };
    Ok(ClientRequest::Lifecycle(request))
}

pub(crate) fn decode_queue_first_message(
    command: artisan_capnp::queue_first_message_request::Reader<'_>,
    request_id: RequestId,
) -> Result<ClientRequest, ProtocolDecodeError> {
    Ok(ClientRequest::Command(Command::QueueFirstMessage(
        QueueFirstMessage {
            request_id,
            thread_id: parse_thread_id(
                read_text(
                    command.get_thread_id(),
                    "request.queueFirstMessage.threadId",
                )?,
                "request.queueFirstMessage.threadId",
            )?,
            body: MessageBody::parse(read_text(
                command.get_body(),
                "request.queueFirstMessage.body",
            )?)
            .map_err(|source| ProtocolDecodeError::MessageBody { source })?,
        },
    )))
}

pub(crate) fn decode_queue_message(
    command: artisan_capnp::queue_message_request::Reader<'_>,
    request_id: RequestId,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let thread_id = parse_thread_id(
        read_text(command.get_thread_id(), "request.queueMessage.threadId")?,
        "request.queueMessage.threadId",
    )?;
    let text = match command.get_text().which()? {
        artisan_capnp::queue_message_request::text::Which::Absent(()) => None,
        artisan_capnp::queue_message_request::text::Which::Present(value) => Some(
            AuthoredText::parse(read_text(value, "request.queueMessage.text")?).map_err(
                |source| ProtocolDecodeError::MessagePayload {
                    source: QueueMessagePayloadError::Text(source),
                },
            )?,
        ),
    };
    let attachments = decode_image_attachments(
        command.get_attachments()?,
        "request.queueMessage.attachments.mimeType",
        "request.queueMessage.attachments.name",
        "request.queueMessage.attachments.bytes",
        "request.queueMessage.attachments",
    )?;
    let payload = QueueMessagePayload::new(text, attachments)?;
    let command = match read_text(
        command.get_steer_run_id(),
        "request.queueMessage.steerRunId",
    )? {
        steer_run_id if steer_run_id.is_empty() => {
            QueueMessage::new(request_id, thread_id, payload)
        }
        steer_run_id => {
            let run_id = parse_run_id(steer_run_id, "request.queueMessage.steerRunId")?;
            QueueMessage::new(request_id, thread_id, payload)
                .with_steer_target(SteerTarget::new(run_id))
        }
    };
    Ok(ClientRequest::Command(Command::QueueMessage(command)))
}

pub(crate) fn decode_stop_run(
    command: artisan_capnp::stop_run_request::Reader<'_>,
    request_id: RequestId,
) -> Result<ClientRequest, ProtocolDecodeError> {
    Ok(ClientRequest::Command(Command::StopRun(StopRun::new(
        request_id,
        parse_thread_id(
            read_text(command.get_thread_id(), "request.stopRun.threadId")?,
            "request.stopRun.threadId",
        )?,
        parse_run_id(
            read_text(command.get_run_id(), "request.stopRun.runId")?,
            "request.stopRun.runId",
        )?,
    ))))
}

pub(crate) fn decode_respond_approval(
    command: artisan_capnp::respond_approval_request::Reader<'_>,
    request_id: RequestId,
) -> Result<ClientRequest, ProtocolDecodeError> {
    Ok(ClientRequest::Command(Command::RespondApproval(
        RespondApproval::new(
            request_id,
            parse_thread_id(
                read_text(command.get_thread_id(), "request.respondApproval.threadId")?,
                "request.respondApproval.threadId",
            )?,
            parse_run_id(
                read_text(command.get_run_id(), "request.respondApproval.runId")?,
                "request.respondApproval.runId",
            )?,
            parse_observation_id(
                read_text(
                    command.get_approval_id(),
                    "request.respondApproval.approvalId",
                )?,
                "request.respondApproval.approvalId",
            )?,
            command.get_approved(),
        ),
    )))
}

pub(crate) fn decode_answer_list(
    encoded: capnp::text_list::Reader<'_>,
    field: &'static str,
) -> Result<Vec<String>, ProtocolDecodeError> {
    let count = encoded.len() as usize;
    if count > OBSERVATION_ANSWERS_MAX {
        return Err(ProtocolDecodeError::RunInteraction {
            source: RunInteractionError::TooManyAnswers {
                count,
                maximum: OBSERVATION_ANSWERS_MAX,
            },
        });
    }
    let mut answers = Vec::with_capacity(count);
    for answer in encoded {
        answers.push(read_text(answer, field)?);
    }
    Ok(answers)
}

pub(crate) fn decode_respond_question(
    command: artisan_capnp::respond_question_request::Reader<'_>,
    request_id: RequestId,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let thread_id = parse_thread_id(
        read_text(command.get_thread_id(), "request.respondQuestion.threadId")?,
        "request.respondQuestion.threadId",
    )?;
    let run_id = parse_run_id(
        read_text(command.get_run_id(), "request.respondQuestion.runId")?,
        "request.respondQuestion.runId",
    )?;
    let question_id = parse_observation_id(
        read_text(
            command.get_question_id(),
            "request.respondQuestion.questionId",
        )?,
        "request.respondQuestion.questionId",
    )?;
    let answers = decode_answer_list(command.get_answers()?, "request.respondQuestion.answers")?;
    RespondQuestion::new(request_id, thread_id, run_id, question_id, answers)
        .map(Command::RespondQuestion)
        .map(ClientRequest::Command)
        .map_err(|source| ProtocolDecodeError::RunInteraction { source })
}

pub(crate) const fn encode_run_interaction_outcome(
    value: RunInteractionOutcome,
) -> artisan_capnp::RespondInteractionOutcome {
    match value {
        RunInteractionOutcome::Applied => artisan_capnp::RespondInteractionOutcome::Applied,
        RunInteractionOutcome::UnknownTarget => {
            artisan_capnp::RespondInteractionOutcome::UnknownTarget
        }
        RunInteractionOutcome::AlreadyResolved => {
            artisan_capnp::RespondInteractionOutcome::AlreadyResolved
        }
        RunInteractionOutcome::WrongRun => artisan_capnp::RespondInteractionOutcome::WrongRun,
    }
}

pub(crate) const fn decode_run_interaction_outcome(
    value: artisan_capnp::RespondInteractionOutcome,
) -> RunInteractionOutcome {
    match value {
        artisan_capnp::RespondInteractionOutcome::Applied => RunInteractionOutcome::Applied,
        artisan_capnp::RespondInteractionOutcome::UnknownTarget => {
            RunInteractionOutcome::UnknownTarget
        }
        artisan_capnp::RespondInteractionOutcome::AlreadyResolved => {
            RunInteractionOutcome::AlreadyResolved
        }
        artisan_capnp::RespondInteractionOutcome::WrongRun => RunInteractionOutcome::WrongRun,
    }
}

pub(crate) fn encode_respond_approval_receipt(
    mut receipt: artisan_capnp::respond_approval_receipt::Builder<'_>,
    value: &RespondApprovalReceipt,
) {
    receipt.set_request_id(value.request_id.as_str());
    receipt.set_thread_id(value.thread_id.as_str());
    receipt.set_run_id(value.run_id.as_str());
    receipt.set_approval_id(value.approval_id.as_str());
    receipt.set_approved(value.approved);
    receipt.set_outcome(encode_run_interaction_outcome(value.outcome));
    receipt.set_disposition(encode_disposition(value.disposition));
}

pub(crate) fn encode_respond_question_receipt(
    mut receipt: artisan_capnp::respond_question_receipt::Builder<'_>,
    value: &RespondQuestionReceipt,
) -> Result<(), ProtocolEncodeError> {
    // Stored answers were validated when the response resolved; rebuilding
    // the domain command keeps encode total for hand-built receipts too.
    let command = RespondQuestion::new(
        value.request_id.clone(),
        value.thread_id.clone(),
        value.run_id.clone(),
        value.question_id.clone(),
        value.answers.clone(),
    )
    .map_err(|source| ProtocolEncodeError::RunInteraction { source })?;
    receipt.set_request_id(value.request_id.as_str());
    receipt.set_thread_id(value.thread_id.as_str());
    receipt.set_run_id(value.run_id.as_str());
    receipt.set_question_id(value.question_id.as_str());
    let mut answers = receipt.reborrow().init_answers(list_length(
        "response.questionResponse.answers",
        command.answers().len(),
    )?);
    for (index, answer) in command.answers().iter().enumerate() {
        answers.set(
            list_index("response.questionResponse.answers", index)?,
            answer.as_str(),
        );
    }
    receipt.set_outcome(encode_run_interaction_outcome(value.outcome));
    receipt.set_disposition(encode_disposition(value.disposition));
    Ok(())
}

pub(crate) fn decode_lifecycle_response(
    value: artisan_capnp::lifecycle_response::Reader<'_>,
) -> Result<LifecycleResponse, ProtocolDecodeError> {
    match value.which()? {
        lifecycle_response::Which::Status(status) => {
            let status = status?;
            Ok(LifecycleResponse::Status(LifecycleStatus::new(
                decode_lifecycle_state(status.get_state()?),
                status.get_active_work_count(),
            )?))
        }
        lifecycle_response::Which::Stop(receipt) => {
            let receipt = receipt?;
            Ok(LifecycleResponse::Stop(LifecycleStopReceipt {
                disposition: decode_lifecycle_stop_disposition(receipt.get_disposition()?),
                state: decode_lifecycle_state(receipt.get_state()?),
            }))
        }
    }
}

pub(crate) fn decode_queued_receipt(
    receipt: artisan_capnp::first_message_receipt::Reader<'_>,
    request_id: &RequestId,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let nested_request_id = parse_request_id(
        read_text(receipt.get_request_id(), "response.queuedReceipt.requestId")?,
        "response.queuedReceipt.requestId",
    )?;
    if &nested_request_id != request_id {
        return Err(ProtocolDecodeError::CorrelationMismatch {
            field: "response.queuedReceipt.requestId",
        });
    }
    match receipt.get_state()? {
        artisan_capnp::QueuedState::Queued => {}
    }
    Ok(ResponsePayload::FirstMessageQueued(FirstMessageReceipt {
        request_id: nested_request_id,
        message_id: parse_message_id(
            read_text(receipt.get_message_id(), "response.queuedReceipt.messageId")?,
            "response.queuedReceipt.messageId",
        )?,
        thread_id: parse_thread_id(
            read_text(receipt.get_thread_id(), "response.queuedReceipt.threadId")?,
            "response.queuedReceipt.threadId",
        )?,
        disposition: decode_disposition(receipt.get_disposition()?),
    }))
}

pub(crate) fn decode_queue_message_receipt(
    receipt: artisan_capnp::queue_message_receipt::Reader<'_>,
    request_id: &RequestId,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let nested_request_id = parse_request_id(
        read_text(
            receipt.get_request_id(),
            "response.queuedMessageReceipt.requestId",
        )?,
        "response.queuedMessageReceipt.requestId",
    )?;
    if &nested_request_id != request_id {
        return Err(ProtocolDecodeError::CorrelationMismatch {
            field: "response.queuedMessageReceipt.requestId",
        });
    }
    match receipt.get_state()? {
        artisan_capnp::QueuedState::Queued => {}
    }
    Ok(ResponsePayload::MessageQueued(QueueMessageReceipt {
        request_id: nested_request_id,
        message_id: parse_message_id(
            read_text(
                receipt.get_message_id(),
                "response.queuedMessageReceipt.messageId",
            )?,
            "response.queuedMessageReceipt.messageId",
        )?,
        thread_id: parse_thread_id(
            read_text(
                receipt.get_thread_id(),
                "response.queuedMessageReceipt.threadId",
            )?,
            "response.queuedMessageReceipt.threadId",
        )?,
        disposition: decode_disposition(receipt.get_disposition()?),
    }))
}

pub(crate) fn decode_message_image(
    result: artisan_capnp::message_image_result::Reader<'_>,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let reference = decode_image_attachment_ref(
        result.get_reference()?,
        "response.messageImage.reference",
        None,
    )?;
    let bytes = result.get_bytes()?.to_vec();
    if bytes.len() != usize::try_from(reference.size_bytes).unwrap_or(usize::MAX) {
        return Err(ProtocolDecodeError::ImageAttachmentReference {
            field: "response.messageImage.bytes",
            reason: "image bytes do not match reference size",
        });
    }
    Ok(ResponsePayload::MessageImage(MessageImageResult {
        reference,
        bytes,
    }))
}

pub(crate) fn decode_stop_run_receipt(
    receipt: artisan_capnp::stop_run_receipt::Reader<'_>,
    request_id: &RequestId,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let nested_request_id = parse_request_id(
        read_text(
            receipt.get_request_id(),
            "response.stopRunReceipt.requestId",
        )?,
        "response.stopRunReceipt.requestId",
    )?;
    if &nested_request_id != request_id {
        return Err(ProtocolDecodeError::CorrelationMismatch {
            field: "response.stopRunReceipt.requestId",
        });
    }
    Ok(ResponsePayload::RunStopped(StopRunReceipt {
        request_id: nested_request_id,
        thread_id: parse_thread_id(
            read_text(receipt.get_thread_id(), "response.stopRunReceipt.threadId")?,
            "response.stopRunReceipt.threadId",
        )?,
        run_id: parse_run_id(
            read_text(receipt.get_run_id(), "response.stopRunReceipt.runId")?,
            "response.stopRunReceipt.runId",
        )?,
        disposition: decode_stop_run_disposition(receipt.get_disposition()?),
    }))
}

pub(crate) fn decode_respond_approval_receipt(
    receipt: artisan_capnp::respond_approval_receipt::Reader<'_>,
    request_id: &RequestId,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let nested_request_id = parse_request_id(
        read_text(
            receipt.get_request_id(),
            "response.approvalResponse.requestId",
        )?,
        "response.approvalResponse.requestId",
    )?;
    if &nested_request_id != request_id {
        return Err(ProtocolDecodeError::CorrelationMismatch {
            field: "response.approvalResponse.requestId",
        });
    }
    Ok(ResponsePayload::ApprovalResponse(RespondApprovalReceipt {
        request_id: nested_request_id,
        thread_id: parse_thread_id(
            read_text(
                receipt.get_thread_id(),
                "response.approvalResponse.threadId",
            )?,
            "response.approvalResponse.threadId",
        )?,
        run_id: parse_run_id(
            read_text(receipt.get_run_id(), "response.approvalResponse.runId")?,
            "response.approvalResponse.runId",
        )?,
        approval_id: parse_observation_id(
            read_text(
                receipt.get_approval_id(),
                "response.approvalResponse.approvalId",
            )?,
            "response.approvalResponse.approvalId",
        )?,
        approved: receipt.get_approved(),
        outcome: decode_run_interaction_outcome(receipt.get_outcome()?),
        disposition: decode_disposition(receipt.get_disposition()?),
    }))
}

pub(crate) fn decode_respond_question_receipt(
    receipt: artisan_capnp::respond_question_receipt::Reader<'_>,
    request_id: &RequestId,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let nested_request_id = parse_request_id(
        read_text(
            receipt.get_request_id(),
            "response.questionResponse.requestId",
        )?,
        "response.questionResponse.requestId",
    )?;
    if &nested_request_id != request_id {
        return Err(ProtocolDecodeError::CorrelationMismatch {
            field: "response.questionResponse.requestId",
        });
    }
    let thread_id = parse_thread_id(
        read_text(
            receipt.get_thread_id(),
            "response.questionResponse.threadId",
        )?,
        "response.questionResponse.threadId",
    )?;
    let run_id = parse_run_id(
        read_text(receipt.get_run_id(), "response.questionResponse.runId")?,
        "response.questionResponse.runId",
    )?;
    let question_id = parse_observation_id(
        read_text(
            receipt.get_question_id(),
            "response.questionResponse.questionId",
        )?,
        "response.questionResponse.questionId",
    )?;
    let answers = decode_answer_list(receipt.get_answers()?, "response.questionResponse.answers")?;
    // The echoed answers prove the identical intent; rebuilding the domain
    // command validates them exactly once.
    let command = RespondQuestion::new(
        nested_request_id.clone(),
        thread_id.clone(),
        run_id.clone(),
        question_id.clone(),
        answers,
    )
    .map_err(|source| ProtocolDecodeError::RunInteraction { source })?;
    Ok(ResponsePayload::QuestionResponse(RespondQuestionReceipt {
        request_id: nested_request_id,
        thread_id,
        run_id,
        question_id,
        answers: command.answers().clone(),
        outcome: decode_run_interaction_outcome(receipt.get_outcome()?),
        disposition: decode_disposition(receipt.get_disposition()?),
    }))
}

pub(crate) fn decode_active_run_result(
    result: artisan_capnp::active_run_result::Reader<'_>,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let thread_id = parse_thread_id(
        read_text(result.get_thread_id(), "response.activeRun.threadId")?,
        "response.activeRun.threadId",
    )?;
    let result = match result.get_state().which()? {
        artisan_capnp::active_run_result::state::Which::NoActive(()) => {
            ActiveRunResult::NoActive { thread_id }
        }
        artisan_capnp::active_run_result::state::Which::Active(run_id) => {
            let status = decode_run_status(result.get_run_status()?)?;
            let engine_id = EngineId::parse(
                read_text(result.get_run_engine_id(), "response.activeRun.runEngineId")?.as_str(),
            )?;
            ActiveRunResult::Active {
                thread_id,
                run_id: parse_run_id(
                    read_text(run_id, "response.activeRun.runId")?,
                    "response.activeRun.runId",
                )?,
                status,
                engine_id,
            }
        }
    };
    Ok(ResponsePayload::ActiveRun(result))
}

pub(crate) fn decode_composer_catalog(
    result: artisan_capnp::composer_catalog_result::Reader<'_>,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let thread_id = parse_thread_id(
        read_text(result.get_thread_id(), "response.composerCatalog.threadId")?,
        "response.composerCatalog.threadId",
    )?;
    let profile_id = parse_profile_id(
        read_text(
            result.get_profile_id(),
            "response.composerCatalog.profileId",
        )?,
        "response.composerCatalog.profileId",
    )?;
    let snapshot = CatalogSnapshotWire::new(result.get_snapshot_data()?.to_vec())?;
    Ok(ResponsePayload::ComposerCatalog(
        ComposerCatalogResult::new(thread_id, profile_id, snapshot)?,
    ))
}

pub(crate) fn decode_read_account_usage(
    value: read_account_usage_request::Reader<'_>,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let engine_id = match value.get_scope().which()? {
        read_account_usage_request::scope::Which::All(()) => None,
        read_account_usage_request::scope::Which::One(engine_id) => {
            Some(read_text(engine_id, "request.readAccountUsage.engineId")?)
        }
    };
    Ok(ClientRequest::Query(Query::ReadAccountUsage(
        ReadAccountUsage::new(engine_id, value.get_force())?,
    )))
}

pub(crate) fn decode_engine_usage_window_kind(
    kind: artisan_capnp::EngineUsageWindowKind,
) -> EngineUsageWindowKind {
    match kind {
        artisan_capnp::EngineUsageWindowKind::Session => EngineUsageWindowKind::Session,
        artisan_capnp::EngineUsageWindowKind::Weekly => EngineUsageWindowKind::Weekly,
        artisan_capnp::EngineUsageWindowKind::Monthly => EngineUsageWindowKind::Monthly,
        artisan_capnp::EngineUsageWindowKind::Unknown => EngineUsageWindowKind::Unknown,
    }
}

pub(crate) fn decode_engine_usage_authentication(
    state: artisan_capnp::EngineUsageAuthentication,
) -> EngineUsageAuthentication {
    match state {
        artisan_capnp::EngineUsageAuthentication::Authenticated => {
            EngineUsageAuthentication::Authenticated
        }
        artisan_capnp::EngineUsageAuthentication::Unauthenticated => {
            EngineUsageAuthentication::Unauthenticated
        }
        artisan_capnp::EngineUsageAuthentication::Unknown => EngineUsageAuthentication::Unknown,
    }
}

pub(crate) fn decode_quota_surface(surface: artisan_capnp::QuotaSurface) -> QuotaSurface {
    match surface {
        artisan_capnp::QuotaSurface::Supported => QuotaSurface::Supported,
        artisan_capnp::QuotaSurface::Unknown => QuotaSurface::Unknown,
        artisan_capnp::QuotaSurface::Unsupported => QuotaSurface::Unsupported,
    }
}

pub(crate) fn optional_wire_text(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

pub(crate) fn decode_engine_usage_window(
    window: engine_usage_window::Reader<'_>,
) -> Result<EngineUsageWindow, ProtocolDecodeError> {
    let window_minutes = match window.get_window_minutes() {
        0 => None,
        minutes => Some(minutes),
    };
    Ok(EngineUsageWindow::new(
        read_text(window.get_id(), "response.accountUsage.window.id")?,
        decode_engine_usage_window_kind(window.get_kind()?),
        optional_wire_text(read_text(
            window.get_label(),
            "response.accountUsage.window.label",
        )?),
        window.get_percent_used(),
        optional_wire_text(read_text(
            window.get_resets_at(),
            "response.accountUsage.window.resetsAt",
        )?),
        window_minutes,
    )?)
}

pub(crate) fn decode_engine_usage_report(
    report: engine_usage_report::Reader<'_>,
) -> Result<EngineUsageReport, ProtocolDecodeError> {
    let authentication = EngineUsageAuth::new(
        decode_engine_usage_authentication(report.get_authentication()?),
        optional_wire_text(read_text(
            report.get_auth_reason(),
            "response.accountUsage.authReason",
        )?),
    )?;
    let quota_surface = match report.get_quota_surface().which()? {
        engine_usage_report::quota_surface::Which::Absent(()) => None,
        engine_usage_report::quota_surface::Which::Present(surface) => {
            Some(decode_quota_surface(surface?))
        }
    };
    let encoded_windows = report.get_windows()?;
    let count = encoded_windows.len() as usize;
    if count > ENGINE_USAGE_WINDOWS_MAX_PER_ENGINE {
        return Err(EngineUsageError::TooManyWindows {
            count,
            maximum: ENGINE_USAGE_WINDOWS_MAX_PER_ENGINE,
        }
        .into());
    }
    let mut windows = Vec::with_capacity(count);
    for encoded_window in encoded_windows {
        windows.push(decode_engine_usage_window(encoded_window)?);
    }
    Ok(EngineUsageReport::new(
        optional_wire_text(read_text(
            report.get_account_email(),
            "response.accountUsage.accountEmail",
        )?),
        authentication,
        read_text(
            report.get_display_name(),
            "response.accountUsage.displayName",
        )?,
        read_text(report.get_engine_id(), "response.accountUsage.engineId")?,
        optional_wire_text(read_text(
            report.get_failure(),
            "response.accountUsage.failure",
        )?),
        quota_surface,
        windows,
    )?)
}

pub(crate) fn decode_engine_usage_snapshot(
    value: engine_usage_snapshot::Reader<'_>,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let fetched_at = read_text(value.get_fetched_at(), "response.accountUsage.fetchedAt")?;
    let encoded_engines = value.get_engines()?;
    let count = encoded_engines.len() as usize;
    if count > ENGINE_USAGE_ENGINES_MAX {
        return Err(EngineUsageError::TooManyEngines {
            count,
            maximum: ENGINE_USAGE_ENGINES_MAX,
        }
        .into());
    }
    let mut engines = Vec::with_capacity(count);
    for encoded_engine in encoded_engines {
        engines.push(decode_engine_usage_report(encoded_engine)?);
    }
    Ok(ResponsePayload::AccountUsage(EngineUsageSnapshot::new(
        engines, fetched_at,
    )?))
}

pub(crate) fn decode_model_favorites_snapshot(
    value: artisan_capnp::model_favorites_snapshot::Reader<'_>,
    model_ids_field: &'static str,
) -> Result<ModelFavoritesSnapshot, ProtocolDecodeError> {
    let encoded_model_ids = value.get_model_ids()?;
    let count = encoded_model_ids.len() as usize;
    if count > artisan_domain::MODEL_FAVORITES_MAX_MODELS {
        return Err(ProtocolDecodeError::ModelFavoritesSnapshot {
            source: ModelFavoritesSnapshotError::TooManyModels {
                count,
                maximum: artisan_domain::MODEL_FAVORITES_MAX_MODELS,
            },
        });
    }
    let mut model_ids = Vec::with_capacity(count);
    for encoded_model_id in encoded_model_ids {
        model_ids.push(ModelFavoriteId::parse(read_text(
            encoded_model_id,
            model_ids_field,
        )?)?);
    }
    let revision = ModelFavoritesRevision::new(value.get_revision())?;
    ModelFavoritesSnapshot::new(revision, model_ids).map_err(ProtocolDecodeError::from)
}

pub(crate) fn decode_model_favorite_set(
    receipt: artisan_capnp::set_model_favorite_receipt::Reader<'_>,
    request_id: &RequestId,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let nested_request_id = parse_request_id(
        read_text(
            receipt.get_request_id(),
            "response.modelFavoriteSet.requestId",
        )?,
        "response.modelFavoriteSet.requestId",
    )?;
    if &nested_request_id != request_id {
        return Err(ProtocolDecodeError::CorrelationMismatch {
            field: "response.modelFavoriteSet.requestId",
        });
    }
    Ok(ResponsePayload::ModelFavoriteSet(SetModelFavoriteReceipt {
        request_id: nested_request_id,
        model_id: ModelFavoriteId::parse(read_text(
            receipt.get_model_id(),
            "response.modelFavoriteSet.modelId",
        )?)?,
        favorite: receipt.get_favorite(),
        disposition: decode_disposition(receipt.get_disposition()?),
        snapshot: decode_model_favorites_snapshot(
            receipt.get_snapshot()?,
            "response.modelFavoriteSet.snapshot.modelIds",
        )?,
    }))
}

pub(crate) fn decode_directory_picked(
    picked: artisan_capnp::directory_pick_outcome::Reader<'_>,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    let outcome = match picked.which()? {
        directory_pick_outcome::Which::Selected(directory_id) => {
            DirectoryPickOutcome::Selected(parse_directory_id(
                read_text(directory_id, "response.directoryPicked.selected")?,
                "response.directoryPicked.selected",
            )?)
        }
        directory_pick_outcome::Which::Cancelled(()) => DirectoryPickOutcome::Cancelled,
    };
    Ok(ResponsePayload::DirectoryPicked(outcome))
}
