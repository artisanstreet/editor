//! Owned conversions for the standalone composer-state Cap'n Proto leaf.
//!
//! The parent protocol codec owns envelope and union dispatch. This module
//! owns only the imported request/response structs, so the parent arms remain
//! small and the bounded payload cannot fall back to opaque JSON.

#![forbid(unsafe_code)]
#![allow(clippy::missing_errors_doc, clippy::module_name_repetitions)]

use artisan_domain::composer_state::{
    COMPOSER_STATE_IMAGE_MAX_BYTES, COMPOSER_STATE_IMAGE_MAX_COUNT,
    COMPOSER_STATE_IMAGE_TOTAL_MAX_BYTES, QueuedMessageWithdrawalResult, ReadRecalledMessage,
    ReadRunUsage, RecalledMessageResult, RunUsageResult, WithdrawQueuedMessageCommand,
    validate_payload_bounds,
};
use artisan_domain::{
    AuthoredText, AuthoredTextError, CommandReceipt, DispatchError, EngineModelId, EngineRouteId,
    EngineVariantId, IdentifierError, ImageAttachment, ImageAttachmentRef, ListQueuedMessages,
    MessageId, QUEUED_MESSAGE_LIST_MAX, QueueMessagePayload, QueueMessagePayloadError,
    QueuedMessageListOrder, QueuedMessageListing, QueuedMessageSummary, ReceiptDisposition,
    RequestId, RunId, RunUsageBasis, RunUsageReport, RunUsageReportInput, ThreadId, UnixMillis,
};
use thiserror::Error;

use crate::composer_state_capnp;

/// Failure while encoding or decoding one imported composer-state value.
#[derive(Debug, Error)]
pub enum ComposerStateCodecError {
    /// Cap'n Proto rejected the pointer graph or a generated accessor.
    #[error("invalid composer-state Cap'n Proto value: {source}")]
    Capnp {
        /// Runtime validation failure.
        #[source]
        source: capnp::Error,
    },
    /// A text pointer contained invalid UTF-8.
    #[error("{field} is not valid UTF-8: {source}")]
    InvalidUtf8 {
        /// Logical field path.
        field: &'static str,
        /// UTF-8 failure.
        #[source]
        source: std::str::Utf8Error,
    },
    /// An identifier failed the domain grammar.
    #[error("invalid {field}: {source}")]
    Identifier {
        /// Logical field path.
        field: &'static str,
        /// Domain identifier failure.
        #[source]
        source: IdentifierError,
    },
    /// Authored text failed its bounded domain validation.
    #[error("invalid {field}: {source}")]
    AuthoredText {
        /// Logical field path.
        field: &'static str,
        /// Domain text failure.
        #[source]
        source: AuthoredTextError,
    },
    /// A payload failed its domain-level invariant.
    #[error("invalid {field}: {source}")]
    Payload {
        /// Logical field path.
        field: &'static str,
        /// Domain payload failure.
        #[source]
        source: QueueMessagePayloadError,
    },
    /// Image metadata or bytes failed exact domain validation.
    #[error("invalid {field}: image metadata or bytes failed validation")]
    Image {
        /// Logical field path.
        field: &'static str,
    },
    /// A byte-free image reference failed exact domain validation.
    #[error("invalid {field}: image reference failed validation")]
    ImageReference {
        /// Logical field path.
        field: &'static str,
    },
    /// A bounded listing violated its count, scope, or ordering invariant.
    #[error("invalid {field}: queued-message listing invariant failed")]
    Listing {
        /// Logical field path.
        field: &'static str,
    },
    /// A usage report failed its bounded domain validation.
    #[error("invalid {field}: run-usage report failed validation")]
    Usage {
        /// Logical field path.
        field: &'static str,
    },
    /// A value failed one of the composer-state result invariants.
    #[error("invalid {field}: composer-state value failed validation")]
    StateValue {
        /// Logical field path.
        field: &'static str,
    },
    /// A parent response and its nested durable receipt disagreed.
    #[error("{field} does not match the enclosing response request id")]
    ResponseCorrelationMismatch {
        /// Nested field whose correlation failed.
        field: &'static str,
    },
    /// A result did not remain in the exact query scope supplied by its
    /// authenticated caller.
    #[error("{field} does not match the exact query scope")]
    ScopeMismatch {
        /// Scope field that failed.
        field: &'static str,
    },
    /// An enum ordinal is unknown to this revision.
    #[error("unknown composer-state enum value {value} for {field}")]
    UnknownEnum {
        /// Logical enum path.
        field: &'static str,
        /// Unknown ordinal.
        value: u16,
    },
    /// A collection length could not be represented by a Cap'n Proto list.
    #[error("{field} contains {length} entries and cannot be represented on the wire")]
    CollectionTooLarge {
        /// Logical collection path.
        field: &'static str,
        /// Offending native length.
        length: usize,
    },
    /// A noncanonical optional wrapper carried a value while marked absent.
    #[error("{field} carries a value while marked absent")]
    NonCanonicalOptional {
        /// Logical optional field path.
        field: &'static str,
    },
}

impl From<capnp::Error> for ComposerStateCodecError {
    fn from(source: capnp::Error) -> Self {
        Self::Capnp { source }
    }
}

/// Encodes a bounded list-queued-messages request into an imported builder.
pub fn encode_list_queued_messages_request(
    mut builder: composer_state_capnp::list_queued_messages_request::Builder<'_>,
    value: &ListQueuedMessages,
) -> Result<(), ComposerStateCodecError> {
    builder.set_thread_id(value.thread_id.as_str());
    builder.set_order(encode_list_order(value.order));
    builder.set_limit(u16::try_from(value.limit).map_err(|_| {
        ComposerStateCodecError::CollectionTooLarge {
            field: "request.listQueuedMessages.limit",
            length: value.limit,
        }
    })?);
    Ok(())
}

/// Decodes a bounded list-queued-messages request from an imported reader.
pub fn decode_list_queued_messages_request(
    value: composer_state_capnp::list_queued_messages_request::Reader<'_>,
) -> Result<ListQueuedMessages, ComposerStateCodecError> {
    let thread_id = parse_thread_id(
        read_text(value.get_thread_id(), "request.listQueuedMessages.threadId")?,
        "request.listQueuedMessages.threadId",
    )?;
    let order = decode_list_order(value.get_order(), "request.listQueuedMessages.order")?;
    ListQueuedMessages::new(thread_id, order, usize::from(value.get_limit())).map_err(|_| {
        ComposerStateCodecError::Listing {
            field: "request.listQueuedMessages.limit",
        }
    })
}

/// Encodes a withdrawal command. The client request id is the parent
/// envelope message id and is therefore intentionally not repeated here.
pub fn encode_withdraw_queued_message_request(
    mut builder: composer_state_capnp::withdraw_queued_message_request::Builder<'_>,
    value: &WithdrawQueuedMessageCommand,
) {
    builder.set_thread_id(value.thread_id.as_str());
    builder.set_message_id(value.message_id.as_str());
    builder.set_original_request_id(value.original_request_id.as_str());
}

/// Decodes a withdrawal command using the parent envelope request id as its
/// durable idempotency identity.
pub fn decode_withdraw_queued_message_request(
    value: composer_state_capnp::withdraw_queued_message_request::Reader<'_>,
    request_id: RequestId,
) -> Result<WithdrawQueuedMessageCommand, ComposerStateCodecError> {
    Ok(WithdrawQueuedMessageCommand::new(
        request_id,
        parse_thread_id(
            read_text(
                value.get_thread_id(),
                "request.withdrawQueuedMessage.threadId",
            )?,
            "request.withdrawQueuedMessage.threadId",
        )?,
        parse_message_id(
            read_text(
                value.get_message_id(),
                "request.withdrawQueuedMessage.messageId",
            )?,
            "request.withdrawQueuedMessage.messageId",
        )?,
        parse_request_id(
            read_text(
                value.get_original_request_id(),
                "request.withdrawQueuedMessage.originalRequestId",
            )?,
            "request.withdrawQueuedMessage.originalRequestId",
        )?,
    ))
}

/// Encodes an exact recalled-message query.
pub fn encode_read_recalled_message_request(
    mut builder: composer_state_capnp::read_recalled_message_request::Builder<'_>,
    value: &ReadRecalledMessage,
) {
    builder.set_thread_id(value.thread_id.as_str());
    builder.set_message_id(value.message_id.as_str());
    builder.set_original_request_id(value.original_request_id.as_str());
}

/// Decodes an exact recalled-message query.
pub fn decode_read_recalled_message_request(
    value: composer_state_capnp::read_recalled_message_request::Reader<'_>,
) -> Result<ReadRecalledMessage, ComposerStateCodecError> {
    Ok(ReadRecalledMessage::new(
        parse_thread_id(
            read_text(
                value.get_thread_id(),
                "request.readRecalledMessage.threadId",
            )?,
            "request.readRecalledMessage.threadId",
        )?,
        parse_message_id(
            read_text(
                value.get_message_id(),
                "request.readRecalledMessage.messageId",
            )?,
            "request.readRecalledMessage.messageId",
        )?,
        parse_request_id(
            read_text(
                value.get_original_request_id(),
                "request.readRecalledMessage.originalRequestId",
            )?,
            "request.readRecalledMessage.originalRequestId",
        )?,
    ))
}

/// Encodes an exact run-usage query.
pub fn encode_read_run_usage_request(
    mut builder: composer_state_capnp::read_run_usage_request::Builder<'_>,
    value: &ReadRunUsage,
) {
    builder.set_thread_id(value.thread_id.as_str());
    builder.set_run_id(value.run_id.as_str());
}

/// Decodes an exact run-usage query.
pub fn decode_read_run_usage_request(
    value: composer_state_capnp::read_run_usage_request::Reader<'_>,
) -> Result<ReadRunUsage, ComposerStateCodecError> {
    Ok(ReadRunUsage::new(
        parse_thread_id(
            read_text(value.get_thread_id(), "request.readRunUsage.threadId")?,
            "request.readRunUsage.threadId",
        )?,
        parse_run_id(
            read_text(value.get_run_id(), "request.readRunUsage.runId")?,
            "request.readRunUsage.runId",
        )?,
    ))
}

/// Encodes an exact queued-message listing.
pub fn encode_queued_message_listing(
    mut builder: composer_state_capnp::queued_message_listing::Builder<'_>,
    value: &QueuedMessageListing,
) -> Result<(), ComposerStateCodecError> {
    builder.set_thread_id(value.thread_id().as_str());
    builder.set_order(encode_list_order(value.order()));
    builder.set_limit(u16::try_from(value.limit()).map_err(|_| {
        ComposerStateCodecError::CollectionTooLarge {
            field: "response.queuedMessages.limit",
            length: value.limit(),
        }
    })?);
    let mut messages = builder.reborrow().init_messages(list_length(
        "response.queuedMessages.messages",
        value.messages().len(),
    )?);
    for (index, message) in value.messages().iter().enumerate() {
        encode_queued_message_summary(
            messages
                .reborrow()
                .get(list_index("response.queuedMessages.messages", index)?),
            message,
        )?;
    }
    builder.set_total_count(value.total_count());
    builder.set_has_more(value.has_more());
    Ok(())
}

/// Decodes and validates an exact queued-message listing. Count and
/// `hasMore` consistency are checked before constructing the owned listing.
pub fn decode_queued_message_listing(
    value: composer_state_capnp::queued_message_listing::Reader<'_>,
) -> Result<QueuedMessageListing, ComposerStateCodecError> {
    let thread_id = parse_thread_id(
        read_text(value.get_thread_id(), "response.queuedMessages.threadId")?,
        "response.queuedMessages.threadId",
    )?;
    let order = decode_list_order(value.get_order(), "response.queuedMessages.order")?;
    let limit = usize::from(value.get_limit());
    let encoded_messages = value.get_messages()?;
    let count = usize::try_from(encoded_messages.len()).map_err(|_| {
        ComposerStateCodecError::CollectionTooLarge {
            field: "response.queuedMessages.messages",
            length: usize::MAX,
        }
    })?;
    if count > QUEUED_MESSAGE_LIST_MAX {
        return Err(ComposerStateCodecError::Listing {
            field: "response.queuedMessages.messages",
        });
    }

    let mut messages = Vec::with_capacity(count);
    for encoded in encoded_messages.iter() {
        messages.push(decode_queued_message_summary(encoded)?);
    }
    let total_count = value.get_total_count();
    let has_more = value.get_has_more();
    let listing = QueuedMessageListing::new(thread_id, order, limit, total_count, messages)
        .map_err(|_| ComposerStateCodecError::Listing {
            field: "response.queuedMessages",
        })?;
    if listing.has_more() != has_more {
        return Err(ComposerStateCodecError::Listing {
            field: "response.queuedMessages.hasMore",
        });
    }
    Ok(listing)
}

/// Encodes a withdrawal receipt and checks its nested receipt id against the
/// enclosing response request id before writing any fields.
pub fn encode_queued_message_withdrawal_result(
    mut builder: composer_state_capnp::queued_message_withdrawal_result::Builder<'_>,
    outer_request_id: &RequestId,
    value: &QueuedMessageWithdrawalResult,
) -> Result<(), ComposerStateCodecError> {
    validate_withdrawal_response_correlation(outer_request_id, value)?;
    builder.set_request_id(value.receipt.request_id.as_str());
    builder.set_thread_id(value.thread_id.as_str());
    builder.set_message_id(value.message_id.as_str());
    builder.set_original_request_id(value.original_request_id.as_str());
    builder.set_accepted_at_millis(value.accepted_at.as_millis());
    builder.set_disposition(encode_disposition(value.receipt.disposition));
    builder.set_outcome(encode_withdrawal_outcome(value.outcome));
    Ok(())
}

/// Decodes a withdrawal receipt and enforces exact outer response
/// correlation.
pub fn decode_queued_message_withdrawal_result(
    value: composer_state_capnp::queued_message_withdrawal_result::Reader<'_>,
    outer_request_id: &RequestId,
) -> Result<QueuedMessageWithdrawalResult, ComposerStateCodecError> {
    let nested_request_id = parse_request_id(
        read_text(
            value.get_request_id(),
            "response.messageWithdrawn.requestId",
        )?,
        "response.messageWithdrawn.requestId",
    )?;
    if &nested_request_id != outer_request_id {
        return Err(ComposerStateCodecError::ResponseCorrelationMismatch {
            field: "response.messageWithdrawn.requestId",
        });
    }
    Ok(QueuedMessageWithdrawalResult {
        receipt: CommandReceipt {
            request_id: nested_request_id,
            disposition: decode_disposition(
                value.get_disposition(),
                "response.messageWithdrawn.disposition",
            )?,
        },
        thread_id: parse_thread_id(
            read_text(value.get_thread_id(), "response.messageWithdrawn.threadId")?,
            "response.messageWithdrawn.threadId",
        )?,
        message_id: parse_message_id(
            read_text(
                value.get_message_id(),
                "response.messageWithdrawn.messageId",
            )?,
            "response.messageWithdrawn.messageId",
        )?,
        original_request_id: parse_request_id(
            read_text(
                value.get_original_request_id(),
                "response.messageWithdrawn.originalRequestId",
            )?,
            "response.messageWithdrawn.originalRequestId",
        )?,
        accepted_at: UnixMillis::from_millis(value.get_accepted_at_millis()),
        outcome: decode_withdrawal_outcome(
            value.get_outcome(),
            "response.messageWithdrawn.outcome",
        )?,
    })
}

/// Encodes an exact recalled-message result, including a typed optional
/// payload.
pub fn encode_recalled_message_result(
    mut builder: composer_state_capnp::recalled_message_result::Builder<'_>,
    value: &RecalledMessageResult,
) -> Result<(), ComposerStateCodecError> {
    builder.set_thread_id(value.thread_id.as_str());
    builder.set_message_id(value.message_id.as_str());
    builder.set_original_request_id(value.original_request_id.as_str());
    if let Some(payload) = &value.payload {
        encode_queue_message_payload(builder.reborrow().init_payload(), payload)?;
    }
    Ok(())
}

/// Decodes an exact recalled-message result while retaining absent payload
/// versus a present image/text payload.
pub fn decode_recalled_message_result(
    value: composer_state_capnp::recalled_message_result::Reader<'_>,
) -> Result<RecalledMessageResult, ComposerStateCodecError> {
    let payload = if value.has_payload() {
        Some(decode_queue_message_payload(value.get_payload()?)?)
    } else {
        None
    };
    RecalledMessageResult::new(
        parse_thread_id(
            read_text(value.get_thread_id(), "response.recalledMessage.threadId")?,
            "response.recalledMessage.threadId",
        )?,
        parse_message_id(
            read_text(value.get_message_id(), "response.recalledMessage.messageId")?,
            "response.recalledMessage.messageId",
        )?,
        parse_request_id(
            read_text(
                value.get_original_request_id(),
                "response.recalledMessage.originalRequestId",
            )?,
            "response.recalledMessage.originalRequestId",
        )?,
        payload,
    )
    .map_err(|_| ComposerStateCodecError::StateValue {
        field: "response.recalledMessage.payload",
    })
}

/// Encodes a run-usage result with explicit optional wrappers for all
/// optional text and numeric fields.
pub fn encode_run_usage_result(
    mut builder: composer_state_capnp::run_usage_result::Builder<'_>,
    value: &RunUsageResult,
) -> Result<(), ComposerStateCodecError> {
    if let Some(report) = &value.report {
        if report.thread_id() != &value.thread_id || report.run_id() != &value.run_id {
            return Err(ComposerStateCodecError::StateValue {
                field: "response.runUsage.report",
            });
        }
    }
    builder.set_thread_id(value.thread_id.as_str());
    builder.set_run_id(value.run_id.as_str());
    if let Some(report) = &value.report {
        encode_run_usage_report(builder.reborrow().init_report(), report)?;
    }
    Ok(())
}

/// Decodes a run-usage result and preserves `None` separately from
/// `Some(0)` in every optional numeric field.
pub fn decode_run_usage_result(
    value: composer_state_capnp::run_usage_result::Reader<'_>,
) -> Result<RunUsageResult, ComposerStateCodecError> {
    let thread_id = parse_thread_id(
        read_text(value.get_thread_id(), "response.runUsage.threadId")?,
        "response.runUsage.threadId",
    )?;
    let run_id = parse_run_id(
        read_text(value.get_run_id(), "response.runUsage.runId")?,
        "response.runUsage.runId",
    )?;
    let report = if value.has_report() {
        Some(decode_run_usage_report(
            value.get_report()?,
            &thread_id,
            &run_id,
        )?)
    } else {
        None
    };
    RunUsageResult::new(thread_id, run_id, report).map_err(|_| {
        ComposerStateCodecError::StateValue {
            field: "response.runUsage.report",
        }
    })
}

/// Validates a recalled result against the exact authenticated read scope.
pub fn validate_recalled_message_scope(
    query: &ReadRecalledMessage,
    result: &RecalledMessageResult,
) -> Result<(), ComposerStateCodecError> {
    if query.thread_id != result.thread_id {
        return Err(ComposerStateCodecError::ScopeMismatch {
            field: "recalledMessage.threadId",
        });
    }
    if query.message_id != result.message_id {
        return Err(ComposerStateCodecError::ScopeMismatch {
            field: "recalledMessage.messageId",
        });
    }
    if query.original_request_id != result.original_request_id {
        return Err(ComposerStateCodecError::ScopeMismatch {
            field: "recalledMessage.originalRequestId",
        });
    }
    Ok(())
}

/// Validates a usage result against the exact authenticated read scope.
pub fn validate_run_usage_scope(
    query: &ReadRunUsage,
    result: &RunUsageResult,
) -> Result<(), ComposerStateCodecError> {
    if query.thread_id != result.thread_id {
        return Err(ComposerStateCodecError::ScopeMismatch {
            field: "runUsage.threadId",
        });
    }
    if query.run_id != result.run_id {
        return Err(ComposerStateCodecError::ScopeMismatch {
            field: "runUsage.runId",
        });
    }
    Ok(())
}

/// Validates the nested durable receipt identity against the parent response
/// correlation before either encoding or admitting the result.
pub fn validate_withdrawal_response_correlation(
    outer_request_id: &RequestId,
    value: &QueuedMessageWithdrawalResult,
) -> Result<(), ComposerStateCodecError> {
    if outer_request_id != &value.receipt.request_id {
        return Err(ComposerStateCodecError::ResponseCorrelationMismatch {
            field: "response.messageWithdrawn.requestId",
        });
    }
    Ok(())
}

fn encode_queue_message_payload(
    mut builder: composer_state_capnp::queue_message_payload::Builder<'_>,
    value: &QueueMessagePayload,
) -> Result<(), ComposerStateCodecError> {
    validate_payload_bounds(value).map_err(|_| ComposerStateCodecError::StateValue {
        field: "composerState.payload",
    })?;
    if let Some(text) = value.text() {
        builder.set_text(text.as_str());
    }
    let mut attachments = builder.reborrow().init_attachments(list_length(
        "composerState.payload.attachments",
        value.attachments().len(),
    )?);
    for (index, attachment) in value.attachments().iter().enumerate() {
        let mut encoded = attachments
            .reborrow()
            .get(list_index("composerState.payload.attachments", index)?);
        encoded.set_mime_type(attachment.mime_type_str());
        encoded.set_name(attachment.name());
        encoded.set_bytes(attachment.bytes());
    }
    Ok(())
}

fn decode_queue_message_payload(
    value: composer_state_capnp::queue_message_payload::Reader<'_>,
) -> Result<QueueMessagePayload, ComposerStateCodecError> {
    let encoded_attachments = value.get_attachments()?;
    let count = usize::try_from(encoded_attachments.len()).map_err(|_| {
        ComposerStateCodecError::CollectionTooLarge {
            field: "composerState.payload.attachments",
            length: usize::MAX,
        }
    })?;
    if count > COMPOSER_STATE_IMAGE_MAX_COUNT {
        return Err(ComposerStateCodecError::Payload {
            field: "composerState.payload.attachments",
            source: QueueMessagePayloadError::TooManyAttachments {
                count,
                maximum: COMPOSER_STATE_IMAGE_MAX_COUNT,
            },
        });
    }

    // Validate every encoded byte slice and the aggregate before creating a
    // Vec or copying one byte. This keeps hostile list lengths and image data
    // from causing an allocation before the finite budget is known.
    let mut total_bytes = 0usize;
    for attachment in encoded_attachments.iter() {
        let bytes = attachment.get_bytes()?;
        if bytes.is_empty() {
            return Err(ComposerStateCodecError::Image {
                field: "composerState.payload.attachments.bytes",
            });
        }
        if bytes.len() > COMPOSER_STATE_IMAGE_MAX_BYTES {
            return Err(ComposerStateCodecError::Image {
                field: "composerState.payload.attachments.bytes",
            });
        }
        total_bytes =
            total_bytes
                .checked_add(bytes.len())
                .ok_or(ComposerStateCodecError::Image {
                    field: "composerState.payload.attachments.bytes",
                })?;
        if total_bytes > COMPOSER_STATE_IMAGE_TOTAL_MAX_BYTES {
            return Err(ComposerStateCodecError::Payload {
                field: "composerState.payload.attachments",
                source: QueueMessagePayloadError::AttachmentsTooLarge {
                    length: total_bytes,
                    maximum: COMPOSER_STATE_IMAGE_TOTAL_MAX_BYTES,
                },
            });
        }
    }

    let mut attachments = Vec::with_capacity(count);
    for attachment in encoded_attachments.iter() {
        let mime_type = read_text(
            attachment.get_mime_type(),
            "composerState.payload.attachments.mimeType",
        )?;
        let name = read_text(
            attachment.get_name(),
            "composerState.payload.attachments.name",
        )?;
        let bytes = attachment.get_bytes()?.to_vec();
        attachments.push(ImageAttachment::new(mime_type, bytes, name).map_err(|_| {
            ComposerStateCodecError::Image {
                field: "composerState.payload.attachments",
            }
        })?);
    }
    let text = if value.has_text() {
        Some(
            AuthoredText::parse(read_text(value.get_text(), "composerState.payload.text")?)
                .map_err(|source| ComposerStateCodecError::AuthoredText {
                    field: "composerState.payload.text",
                    source,
                })?,
        )
    } else {
        None
    };
    QueueMessagePayload::new(text, attachments).map_err(|source| ComposerStateCodecError::Payload {
        field: "composerState.payload",
        source,
    })
}

fn encode_queued_message_summary(
    mut builder: composer_state_capnp::queued_message_summary::Builder<'_>,
    value: &QueuedMessageSummary,
) -> Result<(), ComposerStateCodecError> {
    validate_summary_attachment_scope(value)?;
    builder.set_message_id(value.message_id.as_str());
    builder.set_thread_id(value.thread_id.as_str());
    builder.set_original_request_id(value.original_request_id.as_str());
    if let Some(text) = &value.text {
        builder.set_text(text.as_str());
    }
    let mut attachments = builder.reborrow().init_attachments(list_length(
        "response.queuedMessages.messages.attachments",
        value.attachments.len(),
    )?);
    for (index, attachment) in value.attachments.iter().enumerate() {
        encode_image_attachment_ref(
            attachments.reborrow().get(list_index(
                "response.queuedMessages.messages.attachments",
                index,
            )?),
            attachment,
        );
    }
    builder.set_accepted_at_millis(value.accepted_at.as_millis());
    if let Some(error) = value.last_error.as_ref() {
        builder.set_last_error(error.as_str());
    }
    Ok(())
}

fn decode_queued_message_summary(
    value: composer_state_capnp::queued_message_summary::Reader<'_>,
) -> Result<QueuedMessageSummary, ComposerStateCodecError> {
    let message_id = parse_message_id(
        read_text(
            value.get_message_id(),
            "response.queuedMessages.messages.messageId",
        )?,
        "response.queuedMessages.messages.messageId",
    )?;
    let thread_id = parse_thread_id(
        read_text(
            value.get_thread_id(),
            "response.queuedMessages.messages.threadId",
        )?,
        "response.queuedMessages.messages.threadId",
    )?;
    let original_request_id = parse_request_id(
        read_text(
            value.get_original_request_id(),
            "response.queuedMessages.messages.originalRequestId",
        )?,
        "response.queuedMessages.messages.originalRequestId",
    )?;
    let text = if value.has_text() {
        Some(
            AuthoredText::parse(read_text(
                value.get_text(),
                "response.queuedMessages.messages.text",
            )?)
            .map_err(|source| ComposerStateCodecError::AuthoredText {
                field: "response.queuedMessages.messages.text",
                source,
            })?,
        )
    } else {
        None
    };
    let encoded_attachments = value.get_attachments()?;
    let count = usize::try_from(encoded_attachments.len()).map_err(|_| {
        ComposerStateCodecError::CollectionTooLarge {
            field: "response.queuedMessages.messages.attachments",
            length: usize::MAX,
        }
    })?;
    if count > COMPOSER_STATE_IMAGE_MAX_COUNT {
        return Err(ComposerStateCodecError::Listing {
            field: "response.queuedMessages.messages.attachments",
        });
    }
    let mut attachments = Vec::with_capacity(count);
    for (index, encoded) in encoded_attachments.iter().enumerate() {
        attachments.push(decode_image_attachment_ref(
            encoded,
            &message_id,
            &thread_id,
            u32::try_from(index).map_err(|_| ComposerStateCodecError::CollectionTooLarge {
                field: "response.queuedMessages.messages.attachments",
                length: index,
            })?,
        )?);
    }
    let last_error = if value.has_last_error() {
        Some(
            DispatchError::parse(read_text(
                value.get_last_error(),
                "response.queuedMessages.messages.lastError",
            )?)
            .map_err(|_| ComposerStateCodecError::Listing {
                field: "response.queuedMessages.messages.lastError",
            })?,
        )
    } else {
        None
    };
    Ok(QueuedMessageSummary {
        message_id,
        thread_id,
        original_request_id,
        text,
        attachments,
        accepted_at: UnixMillis::from_millis(value.get_accepted_at_millis()),
        last_error,
    })
}

fn validate_summary_attachment_scope(
    value: &QueuedMessageSummary,
) -> Result<(), ComposerStateCodecError> {
    if value.attachments.len() > COMPOSER_STATE_IMAGE_MAX_COUNT {
        return Err(ComposerStateCodecError::Listing {
            field: "response.queuedMessages.messages.attachments",
        });
    }
    for (index, attachment) in value.attachments.iter().enumerate() {
        if attachment.message_id != value.message_id {
            return Err(ComposerStateCodecError::Listing {
                field: "response.queuedMessages.messages.attachments.messageId",
            });
        }
        if attachment.thread_id != value.thread_id {
            return Err(ComposerStateCodecError::Listing {
                field: "response.queuedMessages.messages.attachments.threadId",
            });
        }
        if attachment.index != u32::try_from(index).unwrap_or(u32::MAX) {
            return Err(ComposerStateCodecError::Listing {
                field: "response.queuedMessages.messages.attachments.index",
            });
        }
    }
    Ok(())
}

fn encode_image_attachment_ref(
    mut builder: composer_state_capnp::image_attachment_ref::Builder<'_>,
    value: &ImageAttachmentRef,
) {
    builder.set_message_id(value.message_id.as_str());
    builder.set_thread_id(value.thread_id.as_str());
    builder.set_index(value.index);
    builder.set_mime_type(value.mime_type_str());
    builder.set_name(value.name.as_str());
    builder.set_size_bytes(value.size_bytes);
    builder.set_digest(&value.digest[..]);
}

fn decode_image_attachment_ref(
    value: composer_state_capnp::image_attachment_ref::Reader<'_>,
    expected_message_id: &MessageId,
    expected_thread_id: &ThreadId,
    expected_index: u32,
) -> Result<ImageAttachmentRef, ComposerStateCodecError> {
    if value.get_index() != expected_index {
        return Err(ComposerStateCodecError::ImageReference {
            field: "response.queuedMessages.messages.attachments.index",
        });
    }
    let message_id = parse_message_id(
        read_text(
            value.get_message_id(),
            "response.queuedMessages.messages.attachments.messageId",
        )?,
        "response.queuedMessages.messages.attachments.messageId",
    )?;
    let thread_id = parse_thread_id(
        read_text(
            value.get_thread_id(),
            "response.queuedMessages.messages.attachments.threadId",
        )?,
        "response.queuedMessages.messages.attachments.threadId",
    )?;
    if &message_id != expected_message_id {
        return Err(ComposerStateCodecError::ImageReference {
            field: "response.queuedMessages.messages.attachments.messageId",
        });
    }
    if &thread_id != expected_thread_id {
        return Err(ComposerStateCodecError::ImageReference {
            field: "response.queuedMessages.messages.attachments.threadId",
        });
    }
    let digest: [u8; 32] = value.get_digest()?.to_vec().try_into().map_err(|_| {
        ComposerStateCodecError::ImageReference {
            field: "response.queuedMessages.messages.attachments.digest",
        }
    })?;
    ImageAttachmentRef::new(
        message_id,
        thread_id,
        expected_index,
        read_text(
            value.get_mime_type(),
            "response.queuedMessages.messages.attachments.mimeType",
        )?,
        read_text(
            value.get_name(),
            "response.queuedMessages.messages.attachments.name",
        )?,
        value.get_size_bytes(),
        digest,
    )
    .map_err(|_| ComposerStateCodecError::ImageReference {
        field: "response.queuedMessages.messages.attachments",
    })
}

fn encode_run_usage_report(
    mut builder: composer_state_capnp::run_usage_report::Builder<'_>,
    value: &RunUsageReport,
) -> Result<(), ComposerStateCodecError> {
    builder.set_provider_session_id(value.provider_session_id());
    builder.set_source_sequence(value.source_sequence());
    builder.set_model_id(value.model_id().as_str());
    builder.set_provider_route_id(value.provider_route_id().as_str());
    encode_optional_text(
        builder.reborrow().init_variant_id(),
        value.variant_id().map(EngineVariantId::as_str),
    );
    builder.set_basis(encode_usage_basis(value.basis()));
    encode_optional_text(
        builder.reborrow().init_provider_turn_id(),
        value.provider_turn_id(),
    );
    encode_optional_u64(builder.reborrow().init_input_tokens(), value.input_tokens());
    encode_optional_u64(
        builder.reborrow().init_cached_input_tokens(),
        value.cached_input_tokens(),
    );
    encode_optional_u64(
        builder.reborrow().init_output_tokens(),
        value.output_tokens(),
    );
    encode_optional_u64(
        builder.reborrow().init_context_tokens(),
        value.context_tokens(),
    );
    encode_optional_u64(
        builder.reborrow().init_context_window_tokens(),
        value.context_window_tokens(),
    );
    builder.set_observed_at_millis(value.observed_at().as_millis());
    Ok(())
}

fn decode_run_usage_report(
    value: composer_state_capnp::run_usage_report::Reader<'_>,
    thread_id: &ThreadId,
    run_id: &RunId,
) -> Result<RunUsageReport, ComposerStateCodecError> {
    let provider_session_id = read_text(
        value.get_provider_session_id(),
        "response.runUsage.report.providerSessionId",
    )?;
    let source_sequence = value.get_source_sequence();
    let model_id = parse_model_id(
        read_text(value.get_model_id(), "response.runUsage.report.modelId")?,
        "response.runUsage.report.modelId",
    )?;
    let provider_route_id = parse_route_id(
        read_text(
            value.get_provider_route_id(),
            "response.runUsage.report.providerRouteId",
        )?,
        "response.runUsage.report.providerRouteId",
    )?;
    let variant_id = match decode_optional_text(
        value.get_variant_id()?,
        "response.runUsage.report.variantId",
    )? {
        Some(value) => Some(parse_variant_id(
            value,
            "response.runUsage.report.variantId",
        )?),
        None => None,
    };
    let basis = decode_usage_basis(value.get_basis(), "response.runUsage.report.basis")?;
    let provider_turn_id = decode_optional_text(
        value.get_provider_turn_id()?,
        "response.runUsage.report.providerTurnId",
    )?;
    let input_tokens = decode_optional_u64(
        value.get_input_tokens()?,
        "response.runUsage.report.inputTokens",
    )?;
    let cached_input_tokens = decode_optional_u64(
        value.get_cached_input_tokens()?,
        "response.runUsage.report.cachedInputTokens",
    )?;
    let output_tokens = decode_optional_u64(
        value.get_output_tokens()?,
        "response.runUsage.report.outputTokens",
    )?;
    let context_tokens = decode_optional_u64(
        value.get_context_tokens()?,
        "response.runUsage.report.contextTokens",
    )?;
    let context_window_tokens = decode_optional_u64(
        value.get_context_window_tokens()?,
        "response.runUsage.report.contextWindowTokens",
    )?;
    RunUsageReport::new(RunUsageReportInput {
        run_id: run_id.clone(),
        thread_id: thread_id.clone(),
        provider_session_id,
        source_sequence,
        model_id,
        provider_route_id,
        variant_id,
        basis,
        provider_turn_id,
        input_tokens,
        cached_input_tokens,
        output_tokens,
        context_tokens,
        context_window_tokens,
        observed_at: UnixMillis::from_millis(value.get_observed_at_millis()),
    })
    .map_err(|_| ComposerStateCodecError::Usage {
        field: "response.runUsage.report",
    })
}

fn encode_optional_text(
    mut builder: composer_state_capnp::optional_text::Builder<'_>,
    value: Option<&str>,
) {
    if let Some(value) = value {
        builder.set_present(true);
        builder.set_value(value);
    } else {
        builder.set_present(false);
    }
}

fn decode_optional_text(
    value: composer_state_capnp::optional_text::Reader<'_>,
    field: &'static str,
) -> Result<Option<String>, ComposerStateCodecError> {
    let present = value.get_present();
    let text = read_text(value.get_value(), field)?;
    if !present && !text.is_empty() {
        return Err(ComposerStateCodecError::NonCanonicalOptional { field });
    }
    Ok(present.then_some(text))
}

fn encode_optional_u64(
    mut builder: composer_state_capnp::optional_u_int64::Builder<'_>,
    value: Option<u64>,
) {
    if let Some(value) = value {
        builder.set_present(true);
        builder.set_value(value);
    } else {
        builder.set_present(false);
    }
}

fn decode_optional_u64(
    value: composer_state_capnp::optional_u_int64::Reader<'_>,
    field: &'static str,
) -> Result<Option<u64>, ComposerStateCodecError> {
    let present = value.get_present();
    let number = value.get_value();
    if !present && number != 0 {
        return Err(ComposerStateCodecError::NonCanonicalOptional { field });
    }
    Ok(present.then_some(number))
}

fn read_text(
    value: capnp::Result<capnp::text::Reader<'_>>,
    field: &'static str,
) -> Result<String, ComposerStateCodecError> {
    value?
        .to_str()
        .map(str::to_owned)
        .map_err(|source| ComposerStateCodecError::InvalidUtf8 { field, source })
}

fn parse_request_id(
    value: String,
    field: &'static str,
) -> Result<RequestId, ComposerStateCodecError> {
    RequestId::parse(value).map_err(|source| ComposerStateCodecError::Identifier { field, source })
}

fn parse_thread_id(
    value: String,
    field: &'static str,
) -> Result<ThreadId, ComposerStateCodecError> {
    ThreadId::parse(value).map_err(|source| ComposerStateCodecError::Identifier { field, source })
}

fn parse_message_id(
    value: String,
    field: &'static str,
) -> Result<MessageId, ComposerStateCodecError> {
    MessageId::parse(value).map_err(|source| ComposerStateCodecError::Identifier { field, source })
}

fn parse_run_id(value: String, field: &'static str) -> Result<RunId, ComposerStateCodecError> {
    RunId::parse(value).map_err(|source| ComposerStateCodecError::Identifier { field, source })
}

fn parse_model_id(
    value: String,
    field: &'static str,
) -> Result<EngineModelId, ComposerStateCodecError> {
    EngineModelId::parse(value)
        .map_err(|source| ComposerStateCodecError::Identifier { field, source })
}

fn parse_route_id(
    value: String,
    field: &'static str,
) -> Result<EngineRouteId, ComposerStateCodecError> {
    EngineRouteId::parse(value)
        .map_err(|source| ComposerStateCodecError::Identifier { field, source })
}

fn parse_variant_id(
    value: String,
    field: &'static str,
) -> Result<EngineVariantId, ComposerStateCodecError> {
    EngineVariantId::parse(value)
        .map_err(|source| ComposerStateCodecError::Identifier { field, source })
}

fn list_length(field: &'static str, length: usize) -> Result<u32, ComposerStateCodecError> {
    u32::try_from(length).map_err(|_| ComposerStateCodecError::CollectionTooLarge { field, length })
}

fn list_index(field: &'static str, index: usize) -> Result<u32, ComposerStateCodecError> {
    u32::try_from(index).map_err(|_| ComposerStateCodecError::CollectionTooLarge {
        field,
        length: index,
    })
}

fn encode_list_order(
    value: QueuedMessageListOrder,
) -> composer_state_capnp::QueuedMessageListOrder {
    match value {
        QueuedMessageListOrder::OldestFirst => {
            composer_state_capnp::QueuedMessageListOrder::OldestFirst
        }
        QueuedMessageListOrder::LatestFirst => {
            composer_state_capnp::QueuedMessageListOrder::LatestFirst
        }
    }
}

fn decode_list_order(
    value: Result<composer_state_capnp::QueuedMessageListOrder, capnp::NotInSchema>,
    field: &'static str,
) -> Result<QueuedMessageListOrder, ComposerStateCodecError> {
    match value.map_err(|source| ComposerStateCodecError::UnknownEnum {
        field,
        value: source.0,
    })? {
        composer_state_capnp::QueuedMessageListOrder::OldestFirst => {
            Ok(QueuedMessageListOrder::OldestFirst)
        }
        composer_state_capnp::QueuedMessageListOrder::LatestFirst => {
            Ok(QueuedMessageListOrder::LatestFirst)
        }
    }
}

fn encode_disposition(value: ReceiptDisposition) -> composer_state_capnp::ReceiptDisposition {
    match value {
        ReceiptDisposition::Accepted => composer_state_capnp::ReceiptDisposition::Accepted,
        ReceiptDisposition::Duplicate => composer_state_capnp::ReceiptDisposition::Duplicate,
    }
}

fn decode_disposition(
    value: Result<composer_state_capnp::ReceiptDisposition, capnp::NotInSchema>,
    field: &'static str,
) -> Result<ReceiptDisposition, ComposerStateCodecError> {
    match value.map_err(|source| ComposerStateCodecError::UnknownEnum {
        field,
        value: source.0,
    })? {
        composer_state_capnp::ReceiptDisposition::Accepted => Ok(ReceiptDisposition::Accepted),
        composer_state_capnp::ReceiptDisposition::Duplicate => Ok(ReceiptDisposition::Duplicate),
    }
}

fn encode_withdrawal_outcome(
    value: artisan_domain::QueuedMessageWithdrawalOutcome,
) -> composer_state_capnp::QueuedMessageWithdrawalOutcome {
    match value {
        artisan_domain::QueuedMessageWithdrawalOutcome::Withdrawn => {
            composer_state_capnp::QueuedMessageWithdrawalOutcome::Withdrawn
        }
        artisan_domain::QueuedMessageWithdrawalOutcome::TooLate => {
            composer_state_capnp::QueuedMessageWithdrawalOutcome::TooLate
        }
        artisan_domain::QueuedMessageWithdrawalOutcome::NotQueued => {
            composer_state_capnp::QueuedMessageWithdrawalOutcome::NotQueued
        }
    }
}

fn decode_withdrawal_outcome(
    value: Result<composer_state_capnp::QueuedMessageWithdrawalOutcome, capnp::NotInSchema>,
    field: &'static str,
) -> Result<artisan_domain::QueuedMessageWithdrawalOutcome, ComposerStateCodecError> {
    match value.map_err(|source| ComposerStateCodecError::UnknownEnum {
        field,
        value: source.0,
    })? {
        composer_state_capnp::QueuedMessageWithdrawalOutcome::Withdrawn => {
            Ok(artisan_domain::QueuedMessageWithdrawalOutcome::Withdrawn)
        }
        composer_state_capnp::QueuedMessageWithdrawalOutcome::TooLate => {
            Ok(artisan_domain::QueuedMessageWithdrawalOutcome::TooLate)
        }
        composer_state_capnp::QueuedMessageWithdrawalOutcome::NotQueued => {
            Ok(artisan_domain::QueuedMessageWithdrawalOutcome::NotQueued)
        }
    }
}

fn encode_usage_basis(value: RunUsageBasis) -> composer_state_capnp::RunUsageBasis {
    match value {
        RunUsageBasis::Delta => composer_state_capnp::RunUsageBasis::Delta,
        RunUsageBasis::Cumulative => composer_state_capnp::RunUsageBasis::Cumulative,
        RunUsageBasis::Unknown => composer_state_capnp::RunUsageBasis::Unknown,
    }
}

fn decode_usage_basis(
    value: Result<composer_state_capnp::RunUsageBasis, capnp::NotInSchema>,
    field: &'static str,
) -> Result<RunUsageBasis, ComposerStateCodecError> {
    match value.map_err(|source| ComposerStateCodecError::UnknownEnum {
        field,
        value: source.0,
    })? {
        composer_state_capnp::RunUsageBasis::Delta => Ok(RunUsageBasis::Delta),
        composer_state_capnp::RunUsageBasis::Cumulative => Ok(RunUsageBasis::Cumulative),
        composer_state_capnp::RunUsageBasis::Unknown => Ok(RunUsageBasis::Unknown),
    }
}
