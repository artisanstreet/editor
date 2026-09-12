//! Conversation query, subscription, snapshot, item, and patch codec.
//!
//! Owns conversation request decode, conversation response encode, the
//! conversation lifecycle and assistant-message-phase conversions, image
//! attachment codec, and the conversation snapshot/patch-batch codec. Each
//! decoded row re-validates through its domain constructor.

#[allow(clippy::wildcard_imports)]
use super::*;

pub(crate) fn encode_conversation_query_request(
    mut builder: artisan_capnp::request::Builder<'_>,
    query: &ConversationQuery,
) {
    let mut encoded = builder.reborrow().init_conversation_query();
    encoded.set_thread_id(query.thread_id.as_str());
    match query.bounds {
        ConversationQueryBounds::Window { maximum_turn_count } => {
            encoded
                .init_bounds()
                .init_window()
                .set_maximum_turn_count(maximum_turn_count.get());
        }
        ConversationQueryBounds::Range {
            before_turn_ordinal,
            minimum_turn_ordinal,
            maximum_turn_count,
        } => {
            let mut range = encoded.init_bounds().init_range();
            range.set_before_turn_ordinal(before_turn_ordinal.get());
            let mut minimum = range.reborrow().init_minimum_turn_ordinal();
            if let Some(minimum_turn_ordinal) = minimum_turn_ordinal {
                minimum.set_minimum(minimum_turn_ordinal.get());
            } else {
                minimum.set_no_minimum(());
            }
            range.set_maximum_turn_count(maximum_turn_count.get());
        }
    }
}

pub(crate) fn encode_conversation_snapshot(
    mut builder: artisan_capnp::conversation_snapshot::Builder<'_>,
    value: &ConversationSnapshot,
) -> Result<(), ProtocolEncodeError> {
    builder.set_thread_id(value.thread_id().as_str());
    builder.set_cursor(value.cursor().get());

    let mut turns = builder.reborrow().init_turns(list_length(
        "conversationSnapshot.turns",
        value.turns().len(),
    )?);
    for (index, turn) in value.turns().iter().enumerate() {
        encode_conversation_turn(
            turns
                .reborrow()
                .get(list_index("conversationSnapshot.turns", index)?),
            turn,
        );
    }

    let mut items = builder.reborrow().init_items(list_length(
        "conversationSnapshot.items",
        value.items().len(),
    )?);
    for (index, item) in value.items().iter().enumerate() {
        encode_conversation_item(
            items
                .reborrow()
                .get(list_index("conversationSnapshot.items", index)?),
            item,
        )?;
    }

    builder.set_updated_at_millis(value.updated_at().as_millis());
    Ok(())
}

pub(crate) fn encode_conversation_turn(
    mut builder: artisan_capnp::conversation_turn::Builder<'_>,
    value: &ConversationTurn,
) {
    builder.set_turn_id(value.turn_id.as_str());
    builder.set_ordinal(value.ordinal.get());
    builder.set_revision(value.revision.get());
    builder.set_lifecycle(encode_conversation_lifecycle(value.lifecycle));
    builder.set_created_at_millis(value.created_at.as_millis());
    builder.set_updated_at_millis(value.updated_at.as_millis());
}

pub(crate) fn encode_conversation_item(
    builder: artisan_capnp::conversation_item::Builder<'_>,
    value: &ConversationItem,
) -> Result<(), ProtocolEncodeError> {
    match value {
        ConversationItem::UserMessage(message) => {
            let mut encoded = builder.init_user_message();
            encoded.set_item_id(message.item_id.as_str());
            encoded.set_turn_id(message.turn_id.as_str());
            encoded.set_ordinal(message.ordinal.get());
            encoded.set_revision(message.revision.get());
            encoded.set_lifecycle(encode_conversation_lifecycle(message.lifecycle));
            encoded.set_body(message.body.as_str());
            encoded.set_created_at_millis(message.created_at.as_millis());
            encoded.set_updated_at_millis(message.updated_at.as_millis());
            if let Some(source_message_id) = message.source_message_id.as_ref() {
                encoded.set_source_message_id(source_message_id.as_str());
            }
        }
        ConversationItem::MultimodalUserMessage(message) => {
            let mut encoded = builder.init_multimodal_user_message();
            encoded.set_item_id(message.item_id.as_str());
            encoded.set_turn_id(message.turn_id.as_str());
            encoded.set_ordinal(message.ordinal.get());
            encoded.set_revision(message.revision.get());
            encoded.set_lifecycle(encode_conversation_lifecycle(message.lifecycle));
            match message.text.as_ref() {
                Some(text) => encoded.reborrow().init_text().set_present(text.as_str()),
                None => encoded.reborrow().init_text().set_absent(()),
            }
            let mut attachments = encoded.reborrow().init_attachments(list_length(
                "conversationItem.multimodalUserMessage.attachments",
                message.attachments.len(),
            )?);
            for (index, attachment) in message.attachments.iter().enumerate() {
                let mut encoded_attachment = attachments.reborrow().get(list_index(
                    "conversationItem.multimodalUserMessage.attachments",
                    index,
                )?);
                encoded_attachment.set_message_id(attachment.message_id.as_str());
                encoded_attachment.set_thread_id(attachment.thread_id.as_str());
                encoded_attachment.set_index(attachment.index);
                encoded_attachment.set_mime_type(attachment.mime_type_str());
                encoded_attachment.set_name(attachment.name.as_str());
                encoded_attachment.set_size_bytes(attachment.size_bytes);
                encoded_attachment.set_digest(&attachment.digest[..]);
            }
            encoded.set_created_at_millis(message.created_at.as_millis());
            encoded.set_updated_at_millis(message.updated_at.as_millis());
            if let Some(source_message_id) = message.source_message_id.as_ref() {
                encoded.set_source_message_id(source_message_id.as_str());
            }
        }
        ConversationItem::AssistantMessage(message) => {
            let mut encoded = builder.init_assistant_message();
            encoded.set_item_id(message.item_id.as_str());
            encoded.set_turn_id(message.turn_id.as_str());
            encoded.set_run_id(message.run_id.as_str());
            encoded.set_ordinal(message.ordinal.get());
            encoded.set_revision(message.revision.get());
            encoded.set_lifecycle(encode_conversation_lifecycle(message.lifecycle));
            encoded.set_body(message.body.as_str());
            encoded.set_phase(encode_assistant_message_phase(message.phase));
            encoded.set_created_at_millis(message.created_at.as_millis());
            encoded.set_updated_at_millis(message.updated_at.as_millis());
        }
    }
    Ok(())
}

pub(crate) fn encode_image_attachment_ref(
    mut builder: artisan_capnp::image_attachment_ref::Builder<'_>,
    reference: &ImageAttachmentRef,
) {
    builder.set_message_id(reference.message_id.as_str());
    builder.set_thread_id(reference.thread_id.as_str());
    builder.set_index(reference.index);
    builder.set_mime_type(reference.mime_type_str());
    builder.set_name(reference.name.as_str());
    builder.set_size_bytes(reference.size_bytes);
    builder.set_digest(&reference.digest[..]);
}

pub(crate) fn encode_patch_batch(
    mut builder: artisan_capnp::patch_batch::Builder<'_>,
    value: &PatchBatch,
) -> Result<(), ProtocolEncodeError> {
    builder.set_thread_id(value.thread_id().as_str());
    builder.set_from_cursor(value.from_cursor().get());
    builder.set_to_cursor(value.to_cursor().get());
    let mut patches = builder
        .reborrow()
        .init_patches(list_length("patchBatch.patches", value.patches().len())?);
    for (index, patch) in value.patches().iter().enumerate() {
        encode_conversation_patch(
            patches
                .reborrow()
                .get(list_index("patchBatch.patches", index)?),
            patch,
        )?;
    }
    Ok(())
}

pub(crate) fn encode_conversation_patch(
    mut builder: artisan_capnp::conversation_patch::Builder<'_>,
    value: &ConversationPatch,
) -> Result<(), ProtocolEncodeError> {
    builder.set_patch_id(value.patch_id().as_str());
    builder.set_sequence(value.sequence().get());
    match value {
        ConversationPatch::TurnUpsert { turn, .. } => {
            encode_conversation_turn(builder.init_turn_upsert(), turn);
        }
        ConversationPatch::ItemUpsert { item, .. } => {
            encode_conversation_item(builder.init_item_upsert(), item)?;
        }
        ConversationPatch::ItemAppend {
            item_id,
            revision,
            text,
            updated_at,
            ..
        } => {
            let mut append = builder.init_item_append();
            append.set_item_id(item_id.as_str());
            append.set_revision(revision.get());
            append.set_text(text.as_str());
            append.set_updated_at_millis(updated_at.as_millis());
        }
        ConversationPatch::ItemLifecycle {
            item_id,
            revision,
            lifecycle,
            updated_at,
            ..
        } => {
            let mut transition = builder.init_item_lifecycle();
            transition.set_item_id(item_id.as_str());
            transition.set_revision(revision.get());
            transition.set_lifecycle(encode_conversation_lifecycle(*lifecycle));
            transition.set_updated_at_millis(updated_at.as_millis());
        }
        ConversationPatch::TurnLifecycle {
            turn_id,
            revision,
            lifecycle,
            updated_at,
            ..
        } => {
            let mut transition = builder.init_turn_lifecycle();
            transition.set_turn_id(turn_id.as_str());
            transition.set_revision(revision.get());
            transition.set_lifecycle(encode_conversation_lifecycle(*lifecycle));
            transition.set_updated_at_millis(updated_at.as_millis());
        }
    }
    Ok(())
}

pub(crate) const fn encode_conversation_lifecycle(
    value: ConversationLifecycle,
) -> artisan_capnp::ConversationLifecycle {
    match value {
        ConversationLifecycle::Pending => artisan_capnp::ConversationLifecycle::Pending,
        ConversationLifecycle::Streaming => artisan_capnp::ConversationLifecycle::Streaming,
        ConversationLifecycle::Active => artisan_capnp::ConversationLifecycle::Active,
        ConversationLifecycle::Waiting => artisan_capnp::ConversationLifecycle::Waiting,
        ConversationLifecycle::Completed => artisan_capnp::ConversationLifecycle::Completed,
        ConversationLifecycle::Failed => artisan_capnp::ConversationLifecycle::Failed,
        ConversationLifecycle::Interrupted => artisan_capnp::ConversationLifecycle::Interrupted,
        ConversationLifecycle::Cancelled => artisan_capnp::ConversationLifecycle::Cancelled,
    }
}

pub(crate) const fn encode_assistant_message_phase(
    value: AssistantMessagePhase,
) -> artisan_capnp::AssistantMessagePhase {
    match value {
        AssistantMessagePhase::Unspecified => artisan_capnp::AssistantMessagePhase::Unspecified,
        AssistantMessagePhase::Commentary => artisan_capnp::AssistantMessagePhase::Commentary,
        AssistantMessagePhase::Final => artisan_capnp::AssistantMessagePhase::Final,
    }
}

pub(crate) fn decode_image_attachments(
    encoded_attachments: capnp::struct_list::Reader<'_, artisan_capnp::image_attachment::Owned>,
    mime_field: &'static str,
    name_field: &'static str,
    bytes_field: &'static str,
    attachment_field: &'static str,
) -> Result<Vec<ImageAttachment>, ProtocolDecodeError> {
    let count = encoded_attachments.len() as usize;
    if count > MESSAGE_IMAGE_ATTACHMENT_MAX_COUNT {
        return Err(ProtocolDecodeError::MessagePayload {
            source: QueueMessagePayloadError::TooManyAttachments {
                count,
                maximum: MESSAGE_IMAGE_ATTACHMENT_MAX_COUNT,
            },
        });
    }
    let mut attachments = Vec::with_capacity(count);
    for encoded in encoded_attachments {
        let bytes = encoded.get_bytes()?;
        if bytes.len() > MESSAGE_IMAGE_ATTACHMENT_MAX_BYTES {
            return Err(ProtocolDecodeError::ImageAttachment {
                field: bytes_field,
                source: ImageAttachmentError::BytesTooLarge {
                    length: bytes.len(),
                    maximum: MESSAGE_IMAGE_ATTACHMENT_MAX_BYTES,
                },
            });
        }
        let mime_type = read_text(encoded.get_mime_type(), mime_field)?;
        let name = read_text(encoded.get_name(), name_field)?;
        attachments.push(
            ImageAttachment::new(mime_type, bytes.to_vec(), name).map_err(|source| {
                ProtocolDecodeError::ImageAttachment {
                    field: attachment_field,
                    source,
                }
            })?,
        );
    }
    Ok(attachments)
}

pub(crate) fn decode_image_attachment_refs(
    encoded_refs: capnp::struct_list::Reader<'_, artisan_capnp::image_attachment_ref::Owned>,
    field: &'static str,
) -> Result<Vec<ImageAttachmentRef>, ProtocolDecodeError> {
    let count = encoded_refs.len() as usize;
    if count > MESSAGE_IMAGE_ATTACHMENT_MAX_COUNT {
        return Err(ProtocolDecodeError::MessagePayload {
            source: QueueMessagePayloadError::TooManyAttachments {
                count,
                maximum: MESSAGE_IMAGE_ATTACHMENT_MAX_COUNT,
            },
        });
    }
    let mut refs = Vec::with_capacity(count);
    for (expected_index, encoded) in encoded_refs.iter().enumerate() {
        let expected_index = u32::try_from(expected_index).map_err(|_| {
            ProtocolDecodeError::ImageAttachmentReference {
                field,
                reason: "attachment index overflow",
            }
        })?;
        refs.push(decode_image_attachment_ref(
            encoded,
            field,
            Some(expected_index),
        )?);
    }
    Ok(refs)
}

pub(crate) fn decode_image_attachment_ref(
    encoded: artisan_capnp::image_attachment_ref::Reader<'_>,
    field: &'static str,
    expected_index: Option<u32>,
) -> Result<ImageAttachmentRef, ProtocolDecodeError> {
    let index = encoded.get_index();
    if expected_index.is_some_and(|expected| expected != index) {
        return Err(ProtocolDecodeError::ImageAttachmentReference {
            field,
            reason: "attachment indexes are not ordered",
        });
    }
    let digest: [u8; 32] = encoded.get_digest()?.to_vec().try_into().map_err(|_| {
        ProtocolDecodeError::ImageAttachmentReference {
            field,
            reason: "digest must be exactly 32 bytes",
        }
    })?;
    ImageAttachmentRef::new(
        parse_message_id(
            read_text(encoded.get_message_id(), "imageAttachmentRef.messageId")?,
            "imageAttachmentRef.messageId",
        )?,
        parse_thread_id(
            read_text(encoded.get_thread_id(), "imageAttachmentRef.threadId")?,
            "imageAttachmentRef.threadId",
        )?,
        index,
        read_text(encoded.get_mime_type(), "imageAttachmentRef.mimeType")?,
        read_text(encoded.get_name(), "imageAttachmentRef.name")?,
        encoded.get_size_bytes(),
        digest,
    )
    .map_err(|source| ProtocolDecodeError::ImageAttachmentReference {
        field,
        reason: match source {
            ImageAttachmentRefError::UnsupportedMimeType => "unsupported MIME type",
            ImageAttachmentRefError::InvalidSize { .. } => "invalid image size",
            ImageAttachmentRefError::InvalidName => "invalid filename",
        },
    })
}

pub(crate) fn decode_conversation_query_request(
    query: artisan_capnp::conversation_query_request::Reader<'_>,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let thread_id = parse_thread_id(
        read_text(query.get_thread_id(), "request.conversationQuery.threadId")?,
        "request.conversationQuery.threadId",
    )?;
    let bounds = match query.get_bounds().which()? {
        conversation_query_request::bounds::Which::Window(window) => {
            ConversationQueryBounds::Window {
                maximum_turn_count: QueryTurnCount::new(u64::from(
                    window?.get_maximum_turn_count(),
                ))?,
            }
        }
        conversation_query_request::bounds::Which::Range(range) => {
            let range = range?;
            let minimum_turn_ordinal = match range.get_minimum_turn_ordinal().which()? {
                query_range::minimum_turn_ordinal::Which::NoMinimum(()) => None,
                query_range::minimum_turn_ordinal::Which::Minimum(value) => {
                    Some(TurnOrdinal::new(value))
                }
            };
            ConversationQueryBounds::Range {
                before_turn_ordinal: TurnOrdinal::new(range.get_before_turn_ordinal()),
                minimum_turn_ordinal,
                maximum_turn_count: QueryTurnCount::new(u64::from(range.get_maximum_turn_count()))?,
            }
        }
    };
    Ok(ClientRequest::Conversation(ConversationRequest::Query(
        ConversationQuery { thread_id, bounds },
    )))
}

pub(crate) fn decode_conversation_subscribe_request(
    subscribe: artisan_capnp::conversation_subscribe_request::Reader<'_>,
) -> Result<ClientRequest, ProtocolDecodeError> {
    let thread_id = parse_thread_id(
        read_text(
            subscribe.get_thread_id(),
            "request.conversationSubscribe.threadId",
        )?,
        "request.conversationSubscribe.threadId",
    )?;
    let value = match subscribe.get_start().which()? {
        conversation_subscribe_request::start::Which::Fresh(()) => {
            ConversationSubscribe::fresh(thread_id)
        }
        conversation_subscribe_request::start::Which::ResumeAfter(cursor) => {
            ConversationSubscribe::resume(thread_id, ConversationCursor::new(cursor))
        }
    };
    Ok(ClientRequest::Conversation(ConversationRequest::Subscribe(
        value,
    )))
}

pub(crate) fn decode_conversation_unsubscribe_request(
    unsubscribe: artisan_capnp::conversation_unsubscribe_request::Reader<'_>,
) -> Result<ClientRequest, ProtocolDecodeError> {
    Ok(ClientRequest::Conversation(
        ConversationRequest::Unsubscribe(ConversationUnsubscribe {
            thread_id: parse_thread_id(
                read_text(
                    unsubscribe.get_thread_id(),
                    "request.conversationUnsubscribe.threadId",
                )?,
                "request.conversationUnsubscribe.threadId",
            )?,
        }),
    ))
}

pub(crate) fn decode_conversation_subscription_started(
    started: artisan_capnp::conversation_subscription_started::Reader<'_>,
) -> Result<ConversationSubscriptionStarted, ProtocolDecodeError> {
    match started.which()? {
        conversation_subscription_started::Which::Fresh(snapshot) => {
            Ok(ConversationSubscriptionStarted::Fresh(
                ConversationSubscriptionStart::new(decode_conversation_snapshot(snapshot?)?),
            ))
        }
        conversation_subscription_started::Which::Resumed(point) => {
            let point = point?;
            Ok(ConversationSubscriptionStarted::Resumed {
                thread_id: parse_thread_id(
                    read_text(
                        point.get_thread_id(),
                        "response.conversationSubscriptionStarted.resumed.threadId",
                    )?,
                    "response.conversationSubscriptionStarted.resumed.threadId",
                )?,
                cursor: ConversationCursor::new(point.get_cursor()),
            })
        }
    }
}

pub(crate) fn decode_conversation_subscription_stopped(
    stopped: artisan_capnp::conversation_subscription_stopped::Reader<'_>,
) -> Result<ConversationSubscriptionStopped, ProtocolDecodeError> {
    Ok(ConversationSubscriptionStopped {
        thread_id: parse_thread_id(
            read_text(
                stopped.get_thread_id(),
                "response.conversationSubscriptionStopped.threadId",
            )?,
            "response.conversationSubscriptionStopped.threadId",
        )?,
    })
}

pub(crate) fn decode_conversation_snapshot(
    value: artisan_capnp::conversation_snapshot::Reader<'_>,
) -> Result<ConversationSnapshot, ProtocolDecodeError> {
    let turns = value.get_turns()?;
    let turn_count = turns.len() as usize;
    let maximum_turn_count = usize::from(CONVERSATION_QUERY_MAX_TURNS);
    if turn_count > maximum_turn_count {
        return Err(ConversationSnapshotError::TooManyTurns {
            count: turn_count,
            maximum: maximum_turn_count,
        }
        .into());
    }
    let turns = turns
        .iter()
        .map(decode_conversation_turn)
        .collect::<Result<Vec<_>, _>>()?;
    let items = value
        .get_items()?
        .iter()
        .map(decode_conversation_item)
        .collect::<Result<Vec<_>, _>>()?;
    ConversationSnapshot::new(
        parse_thread_id(
            read_text(value.get_thread_id(), "conversationSnapshot.threadId")?,
            "conversationSnapshot.threadId",
        )?,
        ConversationCursor::new(value.get_cursor()),
        turns,
        items,
        UnixMillis::from_millis(value.get_updated_at_millis()),
    )
    .map_err(ProtocolDecodeError::from)
}

pub(crate) fn decode_conversation_turn(
    value: artisan_capnp::conversation_turn::Reader<'_>,
) -> Result<ConversationTurn, ProtocolDecodeError> {
    Ok(ConversationTurn {
        turn_id: parse_turn_id(
            read_text(value.get_turn_id(), "conversationTurn.turnId")?,
            "conversationTurn.turnId",
        )?,
        ordinal: TurnOrdinal::new(value.get_ordinal()),
        revision: Revision::new(value.get_revision()),
        lifecycle: decode_conversation_lifecycle(value.get_lifecycle()?),
        created_at: UnixMillis::from_millis(value.get_created_at_millis()),
        updated_at: UnixMillis::from_millis(value.get_updated_at_millis()),
    })
}

/// Decodes the optional source-message identity carried by user items.
///
/// Empty or absent wire text means the row predates the field and decodes
/// to `None` (legacy compatibility). Present text validates as a message
/// id; corrupt text fails typed instead of fabricating an identity.
pub(crate) fn decode_source_message_id(
    value: capnp::Result<capnp::text::Reader<'_>>,
    field: &'static str,
) -> Result<Option<MessageId>, ProtocolDecodeError> {
    let text = read_text(value, field)?;
    if text.is_empty() {
        Ok(None)
    } else {
        parse_message_id(text, field).map(Some)
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "single conversation-item union dispatcher; each arm maps one item variant"
)]
pub(crate) fn decode_conversation_item(
    value: artisan_capnp::conversation_item::Reader<'_>,
) -> Result<ConversationItem, ProtocolDecodeError> {
    match value.which()? {
        conversation_item::Which::UserMessage(message) => {
            let message = message?;
            Ok(ConversationItem::UserMessage(UserMessageItem {
                item_id: parse_item_id(
                    read_text(message.get_item_id(), "conversationItem.userMessage.itemId")?,
                    "conversationItem.userMessage.itemId",
                )?,
                source_message_id: decode_source_message_id(
                    message.get_source_message_id(),
                    "conversationItem.userMessage.sourceMessageId",
                )?,
                turn_id: parse_turn_id(
                    read_text(message.get_turn_id(), "conversationItem.userMessage.turnId")?,
                    "conversationItem.userMessage.turnId",
                )?,
                ordinal: ItemOrdinal::new(message.get_ordinal()),
                revision: Revision::new(message.get_revision()),
                lifecycle: decode_conversation_lifecycle(message.get_lifecycle()?),
                body: MessageBody::parse(read_text(
                    message.get_body(),
                    "conversationItem.userMessage.body",
                )?)
                .map_err(|source| ProtocolDecodeError::MessageBody { source })?,
                created_at: UnixMillis::from_millis(message.get_created_at_millis()),
                updated_at: UnixMillis::from_millis(message.get_updated_at_millis()),
            }))
        }
        conversation_item::Which::MultimodalUserMessage(message) => {
            let message = message?;
            let text = match message.get_text().which()? {
                artisan_capnp::multimodal_user_message_item::text::Which::Absent(()) => None,
                artisan_capnp::multimodal_user_message_item::text::Which::Present(value) => Some(
                    AuthoredText::parse(read_text(
                        value,
                        "conversationItem.multimodalUserMessage.text",
                    )?)
                    .map_err(|source| {
                        ProtocolDecodeError::MessagePayload {
                            source: QueueMessagePayloadError::Text(source),
                        }
                    })?,
                ),
            };
            let attachments = decode_image_attachment_refs(
                message.get_attachments()?,
                "conversationItem.multimodalUserMessage.attachments",
            )?;
            if attachments.is_empty() {
                return Err(ProtocolDecodeError::MessagePayload {
                    source: QueueMessagePayloadError::Empty,
                });
            }
            Ok(ConversationItem::MultimodalUserMessage(
                MultimodalUserMessageItem {
                    item_id: parse_item_id(
                        read_text(
                            message.get_item_id(),
                            "conversationItem.multimodalUserMessage.itemId",
                        )?,
                        "conversationItem.multimodalUserMessage.itemId",
                    )?,
                    source_message_id: decode_source_message_id(
                        message.get_source_message_id(),
                        "conversationItem.multimodalUserMessage.sourceMessageId",
                    )?,
                    turn_id: parse_turn_id(
                        read_text(
                            message.get_turn_id(),
                            "conversationItem.multimodalUserMessage.turnId",
                        )?,
                        "conversationItem.multimodalUserMessage.turnId",
                    )?,
                    ordinal: ItemOrdinal::new(message.get_ordinal()),
                    revision: Revision::new(message.get_revision()),
                    lifecycle: decode_conversation_lifecycle(message.get_lifecycle()?),
                    text,
                    attachments,
                    created_at: UnixMillis::from_millis(message.get_created_at_millis()),
                    updated_at: UnixMillis::from_millis(message.get_updated_at_millis()),
                },
            ))
        }
        conversation_item::Which::AssistantMessage(message) => {
            let message = message?;
            Ok(ConversationItem::AssistantMessage(AssistantMessageItem {
                item_id: parse_item_id(
                    read_text(
                        message.get_item_id(),
                        "conversationItem.assistantMessage.itemId",
                    )?,
                    "conversationItem.assistantMessage.itemId",
                )?,
                turn_id: parse_turn_id(
                    read_text(
                        message.get_turn_id(),
                        "conversationItem.assistantMessage.turnId",
                    )?,
                    "conversationItem.assistantMessage.turnId",
                )?,
                run_id: parse_run_id(
                    read_text(
                        message.get_run_id(),
                        "conversationItem.assistantMessage.runId",
                    )?,
                    "conversationItem.assistantMessage.runId",
                )?,
                ordinal: ItemOrdinal::new(message.get_ordinal()),
                revision: Revision::new(message.get_revision()),
                lifecycle: decode_conversation_lifecycle(message.get_lifecycle()?),
                body: AssistantBody::parse(read_text(
                    message.get_body(),
                    "conversationItem.assistantMessage.body",
                )?)
                .map_err(|source| ProtocolDecodeError::AssistantBody { source })?,
                phase: decode_assistant_message_phase(message.get_phase()?),
                created_at: UnixMillis::from_millis(message.get_created_at_millis()),
                updated_at: UnixMillis::from_millis(message.get_updated_at_millis()),
            }))
        }
        conversation_item::Which::Unmodeled(()) => {
            Err(ProtocolDecodeError::UnmodeledConversationItem)
        }
    }
}

pub(crate) fn decode_patch_batch(
    value: artisan_capnp::patch_batch::Reader<'_>,
) -> Result<PatchBatch, ProtocolDecodeError> {
    let patches = value.get_patches()?;
    let patch_count = patches.len() as usize;
    if patch_count > CONVERSATION_PATCH_BATCH_MAX_PATCHES {
        return Err(PatchBatchError::TooManyPatches {
            count: patch_count,
            maximum: CONVERSATION_PATCH_BATCH_MAX_PATCHES,
        }
        .into());
    }
    let patches = patches
        .iter()
        .map(decode_conversation_patch)
        .collect::<Result<Vec<_>, _>>()?;
    PatchBatch::new(
        parse_thread_id(
            read_text(value.get_thread_id(), "patchBatch.threadId")?,
            "patchBatch.threadId",
        )?,
        ConversationCursor::new(value.get_from_cursor()),
        ConversationCursor::new(value.get_to_cursor()),
        patches,
    )
    .map_err(ProtocolDecodeError::from)
}

pub(crate) fn decode_conversation_patch(
    value: artisan_capnp::conversation_patch::Reader<'_>,
) -> Result<ConversationPatch, ProtocolDecodeError> {
    let patch_id = parse_patch_id(
        read_text(value.get_patch_id(), "conversationPatch.patchId")?,
        "conversationPatch.patchId",
    )?;
    let sequence = PatchSequence::new(value.get_sequence()).map_err(|source| {
        ProtocolDecodeError::Counter {
            field: "conversationPatch.sequence",
            source,
        }
    })?;
    match value.which()? {
        conversation_patch::Which::TurnUpsert(turn) => Ok(ConversationPatch::TurnUpsert {
            patch_id,
            sequence,
            turn: decode_conversation_turn(turn?)?,
        }),
        conversation_patch::Which::ItemUpsert(item) => Ok(ConversationPatch::ItemUpsert {
            patch_id,
            sequence,
            item: decode_conversation_item(item?)?,
        }),
        conversation_patch::Which::ItemAppend(append) => {
            let append = append?;
            Ok(ConversationPatch::ItemAppend {
                patch_id,
                sequence,
                item_id: parse_item_id(
                    read_text(append.get_item_id(), "conversationPatch.itemAppend.itemId")?,
                    "conversationPatch.itemAppend.itemId",
                )?,
                revision: Revision::new(append.get_revision()),
                text: IncrementalText::parse(read_text(
                    append.get_text(),
                    "conversationPatch.itemAppend.text",
                )?)?,
                updated_at: UnixMillis::from_millis(append.get_updated_at_millis()),
            })
        }
        conversation_patch::Which::ItemLifecycle(transition) => {
            let transition = transition?;
            Ok(ConversationPatch::ItemLifecycle {
                patch_id,
                sequence,
                item_id: parse_item_id(
                    read_text(
                        transition.get_item_id(),
                        "conversationPatch.itemLifecycle.itemId",
                    )?,
                    "conversationPatch.itemLifecycle.itemId",
                )?,
                revision: Revision::new(transition.get_revision()),
                lifecycle: decode_conversation_lifecycle(transition.get_lifecycle()?),
                updated_at: UnixMillis::from_millis(transition.get_updated_at_millis()),
            })
        }
        conversation_patch::Which::TurnLifecycle(transition) => {
            let transition = transition?;
            Ok(ConversationPatch::TurnLifecycle {
                patch_id,
                sequence,
                turn_id: parse_turn_id(
                    read_text(
                        transition.get_turn_id(),
                        "conversationPatch.turnLifecycle.turnId",
                    )?,
                    "conversationPatch.turnLifecycle.turnId",
                )?,
                revision: Revision::new(transition.get_revision()),
                lifecycle: decode_conversation_lifecycle(transition.get_lifecycle()?),
                updated_at: UnixMillis::from_millis(transition.get_updated_at_millis()),
            })
        }
    }
}

pub(crate) const fn decode_conversation_lifecycle(
    value: artisan_capnp::ConversationLifecycle,
) -> ConversationLifecycle {
    match value {
        artisan_capnp::ConversationLifecycle::Pending => ConversationLifecycle::Pending,
        artisan_capnp::ConversationLifecycle::Streaming => ConversationLifecycle::Streaming,
        artisan_capnp::ConversationLifecycle::Active => ConversationLifecycle::Active,
        artisan_capnp::ConversationLifecycle::Waiting => ConversationLifecycle::Waiting,
        artisan_capnp::ConversationLifecycle::Completed => ConversationLifecycle::Completed,
        artisan_capnp::ConversationLifecycle::Failed => ConversationLifecycle::Failed,
        artisan_capnp::ConversationLifecycle::Interrupted => ConversationLifecycle::Interrupted,
        artisan_capnp::ConversationLifecycle::Cancelled => ConversationLifecycle::Cancelled,
    }
}

pub(crate) const fn decode_assistant_message_phase(
    value: artisan_capnp::AssistantMessagePhase,
) -> AssistantMessagePhase {
    match value {
        artisan_capnp::AssistantMessagePhase::Unspecified => AssistantMessagePhase::Unspecified,
        artisan_capnp::AssistantMessagePhase::Commentary => AssistantMessagePhase::Commentary,
        artisan_capnp::AssistantMessagePhase::Final => AssistantMessagePhase::Final,
    }
}
