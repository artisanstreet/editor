//! Focused Cap'n Proto coverage for the standalone composer-state leaf.

use artisan_domain::composer_state::{
    COMPOSER_STATE_IMAGE_MAX_BYTES, COMPOSER_STATE_IMAGE_MAX_COUNT,
    COMPOSER_STATE_IMAGE_TOTAL_MAX_BYTES, QueuedMessageWithdrawalResult, ReadRecalledMessage,
    ReadRunUsage, RecalledMessageResult, RunUsageResult, WithdrawQueuedMessageCommand,
    validate_payload_bounds,
};
use artisan_domain::{
    AuthoredText, CommandReceipt, DispatchError, EngineModelId, EngineRouteId, EngineVariantId,
    FAILED_MESSAGE_LIST_MAX, FailedMessageListing, FailedMessageSummary, ImageAttachment,
    ImageAttachmentRef, ListFailedMessages, ListQueuedMessages, MessageId, QueueMessagePayload,
    QueuedMessageListOrder, QueuedMessageListing, QueuedMessageSummary,
    QueuedMessageWithdrawalOutcome, ReceiptDisposition, RequestId, RunId, RunUsageBasis,
    RunUsageReport, RunUsageReportInput, ThreadId, UnixMillis,
};
use artisan_protocol::composer_state::{
    ComposerStateCodecError, decode_failed_message_listing, decode_list_failed_messages_request,
    decode_list_queued_messages_request, decode_queued_message_listing,
    decode_queued_message_withdrawal_result, decode_read_recalled_message_request,
    decode_read_run_usage_request, decode_recalled_message_result, decode_run_usage_result,
    decode_withdraw_queued_message_request, encode_failed_message_listing,
    encode_list_failed_messages_request, encode_list_queued_messages_request,
    encode_queued_message_listing, encode_queued_message_withdrawal_result,
    encode_read_recalled_message_request, encode_read_run_usage_request,
    encode_recalled_message_result, encode_run_usage_result,
    encode_withdraw_queued_message_request, validate_recalled_message_scope,
    validate_run_usage_scope,
};
use artisan_protocol::composer_state_capnp;
use capnp::message::{Builder, HeapAllocator, ReaderOptions};
use capnp::serialize;

fn thread() -> ThreadId {
    ThreadId::parse("composer-thread").expect("thread")
}

fn message_id() -> MessageId {
    MessageId::parse("composer-message").expect("message")
}

fn request_id(value: &str) -> RequestId {
    RequestId::parse(value).expect("request id")
}

fn run() -> RunId {
    RunId::parse("composer-run").expect("run")
}

fn image(index: usize) -> ImageAttachment {
    ImageAttachment::new(
        "image/png",
        vec![u8::try_from(index + 1).expect("fixture byte")],
        format!("capture-{index}.png"),
    )
    .expect("image")
}

fn payload(text: Option<AuthoredText>, count: usize) -> QueueMessagePayload {
    QueueMessagePayload::new(text, (0..count).map(image).collect()).expect("payload")
}

fn recalled(payload: Option<QueueMessagePayload>) -> RecalledMessageResult {
    RecalledMessageResult::new(
        thread(),
        message_id(),
        request_id("original-request"),
        payload,
    )
    .expect("recalled result")
}

fn round_trip_recalled(value: &RecalledMessageResult) -> RecalledMessageResult {
    let mut message = Builder::new(HeapAllocator::new());
    encode_recalled_message_result(
        message.init_root::<composer_state_capnp::recalled_message_result::Builder>(),
        value,
    )
    .expect("encode recalled result");
    let words = serialize::write_message_to_words(&message);
    let mut encoded = words.as_slice();
    let decoded = serialize::read_message_from_flat_slice(&mut encoded, ReaderOptions::new())
        .expect("read recalled result");
    decode_recalled_message_result(
        decoded
            .get_root::<composer_state_capnp::recalled_message_result::Reader>()
            .expect("recalled root"),
    )
    .expect("decode recalled result")
}

fn round_trip_usage(value: &RunUsageResult) -> RunUsageResult {
    let mut message = Builder::new(HeapAllocator::new());
    encode_run_usage_result(
        message.init_root::<composer_state_capnp::run_usage_result::Builder>(),
        value,
    )
    .expect("encode usage result");
    let words = serialize::write_message_to_words(&message);
    let mut encoded = words.as_slice();
    let decoded = serialize::read_message_from_flat_slice(&mut encoded, ReaderOptions::new())
        .expect("read usage result");
    decode_run_usage_result(
        decoded
            .get_root::<composer_state_capnp::run_usage_result::Reader>()
            .expect("usage root"),
    )
    .expect("decode usage result")
}

fn round_trip_listing(value: &QueuedMessageListing) -> QueuedMessageListing {
    let mut message = Builder::new(HeapAllocator::new());
    encode_queued_message_listing(
        message.init_root::<composer_state_capnp::queued_message_listing::Builder>(),
        value,
    )
    .expect("encode listing");
    let words = serialize::write_message_to_words(&message);
    let mut encoded = words.as_slice();
    let decoded = serialize::read_message_from_flat_slice(&mut encoded, ReaderOptions::new())
        .expect("read listing");
    decode_queued_message_listing(
        decoded
            .get_root::<composer_state_capnp::queued_message_listing::Reader>()
            .expect("listing root"),
    )
    .expect("decode listing")
}

#[test]
fn payload_preserves_absent_empty_and_whitespace_text_with_ten_images() {
    assert_eq!(COMPOSER_STATE_IMAGE_MAX_COUNT, 10);
    for text in [
        None,
        Some(AuthoredText::empty()),
        Some(AuthoredText::parse(" \n\t").expect("whitespace text")),
    ] {
        let original = recalled(Some(payload(text, COMPOSER_STATE_IMAGE_MAX_COUNT)));
        assert_eq!(round_trip_recalled(&original), original);
    }
}

#[test]
fn ten_image_boundary_uses_the_general_domain_budgets() {
    let value = payload(Some(AuthoredText::parse("ten").expect("text")), 10);
    assert_eq!(value.attachments().len(), 10);
    assert_eq!(value.total_attachment_bytes(), 10);
    assert!(validate_payload_bounds(&value).is_ok());
    assert_eq!(COMPOSER_STATE_IMAGE_MAX_BYTES, 5 * 1024 * 1024);
    assert_eq!(COMPOSER_STATE_IMAGE_TOTAL_MAX_BYTES, 12 * 1024 * 1024);
}

#[test]
fn recalled_payload_absence_round_trips_as_absence() {
    let original = recalled(None);
    let received = round_trip_recalled(&original);
    assert_eq!(received.payload, None);
}

#[test]
fn queued_listing_round_trips_ordered_metadata_and_count_state() {
    let message_id = message_id();
    let thread_id = thread();
    let summary = QueuedMessageSummary {
        message_id: message_id.clone(),
        thread_id: thread_id.clone(),
        original_request_id: request_id("original-request"),
        text: Some(AuthoredText::parse(" \n").expect("text")),
        attachments: vec![
            ImageAttachmentRef::new(
                message_id,
                thread_id.clone(),
                0,
                "image/png",
                "capture.png",
                3,
                [7; 32],
            )
            .expect("reference"),
        ],
        accepted_at: UnixMillis::from_millis(-7),
        last_error: Some(
            DispatchError::parse("engine unconfigured".to_owned()).expect("diagnostic"),
        ),
    };
    let original = QueuedMessageListing::new(
        thread_id,
        QueuedMessageListOrder::LatestFirst,
        2,
        3,
        vec![summary],
    )
    .expect("listing");
    assert!(round_trip_listing(&original).has_more());
    assert_eq!(round_trip_listing(&original), original);
    assert_eq!(
        round_trip_listing(&original).messages()[0]
            .last_error
            .as_ref()
            .expect("diagnostic survives the wire")
            .as_str(),
        "engine unconfigured"
    );
}

#[test]
fn listing_rejects_more_than_thirty_two_rows_before_row_allocation() {
    let mut message = Builder::new(HeapAllocator::new());
    {
        let mut listing =
            message.init_root::<composer_state_capnp::queued_message_listing::Builder>();
        listing.set_thread_id(thread().as_str());
        listing.set_order(composer_state_capnp::QueuedMessageListOrder::OldestFirst);
        listing.set_limit(32);
        listing.set_total_count(33);
        listing.set_has_more(true);
        listing.init_messages(33);
    }
    let words = serialize::write_message_to_words(&message);
    let mut encoded = words.as_slice();
    let decoded = serialize::read_message_from_flat_slice(&mut encoded, ReaderOptions::new())
        .expect("read malformed listing");
    assert!(matches!(
        decode_queued_message_listing(
            decoded
                .get_root::<composer_state_capnp::queued_message_listing::Reader>()
                .expect("listing root")
        ),
        Err(ComposerStateCodecError::Listing { .. })
    ));
}

#[test]
fn listing_rejects_inconsistent_has_more_flag() {
    let original =
        QueuedMessageListing::new(thread(), QueuedMessageListOrder::OldestFirst, 1, 2, vec![])
            .expect("listing");
    let mut message = Builder::new(HeapAllocator::new());
    let mut listing = message.init_root::<composer_state_capnp::queued_message_listing::Builder>();
    encode_queued_message_listing(listing.reborrow(), &original).expect("encode listing");
    listing.set_has_more(false);
    drop(listing);
    let words = serialize::write_message_to_words(&message);
    let mut encoded = words.as_slice();
    let decoded = serialize::read_message_from_flat_slice(&mut encoded, ReaderOptions::new())
        .expect("read malformed listing");
    assert!(matches!(
        decode_queued_message_listing(
            decoded
                .get_root::<composer_state_capnp::queued_message_listing::Reader>()
                .expect("listing root")
        ),
        Err(ComposerStateCodecError::Listing { .. })
    ));
}

#[test]
fn oversized_image_payload_is_rejected_before_copying_bytes() {
    let mut message = Builder::new(HeapAllocator::new());
    {
        let mut result =
            message.init_root::<composer_state_capnp::recalled_message_result::Builder>();
        result.set_thread_id(thread().as_str());
        result.set_message_id(message_id().as_str());
        result.set_original_request_id(request_id("original-request").as_str());
        let mut payload = result.init_payload();
        let mut attachments = payload.init_attachments(1);
        let mut image = attachments.reborrow().get(0);
        image.set_mime_type("image/png");
        image.set_name("too-large.png");
        image.set_bytes(&vec![1; COMPOSER_STATE_IMAGE_MAX_BYTES + 1]);
    }
    let words = serialize::write_message_to_words(&message);
    let mut encoded = words.as_slice();
    let decoded = serialize::read_message_from_flat_slice(&mut encoded, ReaderOptions::new())
        .expect("read oversized payload");
    assert!(
        decode_recalled_message_result(
            decoded
                .get_root::<composer_state_capnp::recalled_message_result::Reader>()
                .expect("recalled root")
        )
        .is_err()
    );
}

#[test]
fn aggregate_image_budget_is_rejected_before_copying_bytes() {
    let per_image = 4 * 1024 * 1024 + 1;
    let mut message = Builder::new(HeapAllocator::new());
    {
        let mut result =
            message.init_root::<composer_state_capnp::recalled_message_result::Builder>();
        result.set_thread_id(thread().as_str());
        result.set_message_id(message_id().as_str());
        result.set_original_request_id(request_id("original-request").as_str());
        let mut payload = result.init_payload();
        let mut attachments = payload.init_attachments(3);
        for index in 0..3 {
            let mut image = attachments.reborrow().get(index);
            image.set_mime_type("image/png");
            image.set_name(format!("large-{index}.png"));
            image.set_bytes(&vec![1; per_image]);
        }
    }
    let words = serialize::write_message_to_words(&message);
    let mut encoded = words.as_slice();
    let decoded = serialize::read_message_from_flat_slice(&mut encoded, ReaderOptions::new())
        .expect("read oversized aggregate");
    assert!(
        decode_recalled_message_result(
            decoded
                .get_root::<composer_state_capnp::recalled_message_result::Reader>()
                .expect("recalled root")
        )
        .is_err()
    );
}

fn usage_result() -> RunUsageResult {
    let thread_id = thread();
    let run_id = run();
    let report = RunUsageReport::new(RunUsageReportInput {
        run_id: run_id.clone(),
        thread_id: thread_id.clone(),
        provider_session_id: "provider-session".to_owned(),
        source_sequence: 0,
        model_id: EngineModelId::parse("model").expect("model"),
        provider_route_id: EngineRouteId::parse("route").expect("route"),
        variant_id: Some(EngineVariantId::parse("variant").expect("variant")),
        basis: RunUsageBasis::Delta,
        provider_turn_id: None,
        input_tokens: Some(0),
        cached_input_tokens: None,
        output_tokens: Some(4),
        context_tokens: None,
        context_window_tokens: Some(1),
        observed_at: UnixMillis::from_millis(11),
    })
    .expect("report");
    RunUsageResult::new(thread_id, run_id, Some(report)).expect("usage result")
}

#[test]
fn usage_round_trip_preserves_absent_vs_zero_and_optional_identity_fields() {
    let original = usage_result();
    let received = round_trip_usage(&original);
    assert_eq!(received, original);
    let report = received.report.as_ref().expect("report");
    assert_eq!(report.input_tokens(), Some(0));
    assert_eq!(report.cached_input_tokens(), None);
    assert_eq!(report.provider_turn_id(), None);
    assert_eq!(report.source_sequence(), 0);
}

#[test]
fn request_helpers_round_trip_exact_scopes_and_outer_request_identity() {
    let query = ListQueuedMessages::new(thread(), QueuedMessageListOrder::LatestFirst, 10)
        .expect("list query");
    let mut message = Builder::new(HeapAllocator::new());
    encode_list_queued_messages_request(
        message.init_root::<composer_state_capnp::list_queued_messages_request::Builder>(),
        &query,
    )
    .expect("encode list query");
    let words = serialize::write_message_to_words(&message);
    let mut encoded = words.as_slice();
    let decoded = serialize::read_message_from_flat_slice(&mut encoded, ReaderOptions::new())
        .expect("read list query");
    assert_eq!(
        decode_list_queued_messages_request(
            decoded
                .get_root::<composer_state_capnp::list_queued_messages_request::Reader>()
                .expect("list query root")
        )
        .expect("decode list query"),
        query
    );

    let recalled_query =
        ReadRecalledMessage::new(thread(), message_id(), request_id("original-request"));
    let mut message = Builder::new(HeapAllocator::new());
    encode_read_recalled_message_request(
        message.init_root::<composer_state_capnp::read_recalled_message_request::Builder>(),
        &recalled_query,
    );
    let words = serialize::write_message_to_words(&message);
    let mut encoded = words.as_slice();
    let decoded = serialize::read_message_from_flat_slice(&mut encoded, ReaderOptions::new())
        .expect("read recalled query");
    assert_eq!(
        decode_read_recalled_message_request(
            decoded
                .get_root::<composer_state_capnp::read_recalled_message_request::Reader>()
                .expect("recalled query root")
        )
        .expect("decode recalled query"),
        recalled_query
    );

    let usage_query = ReadRunUsage::new(thread(), run());
    let mut message = Builder::new(HeapAllocator::new());
    encode_read_run_usage_request(
        message.init_root::<composer_state_capnp::read_run_usage_request::Builder>(),
        &usage_query,
    );
    let words = serialize::write_message_to_words(&message);
    let mut encoded = words.as_slice();
    let decoded = serialize::read_message_from_flat_slice(&mut encoded, ReaderOptions::new())
        .expect("read usage query");
    assert_eq!(
        decode_read_run_usage_request(
            decoded
                .get_root::<composer_state_capnp::read_run_usage_request::Reader>()
                .expect("usage query root")
        )
        .expect("decode usage query"),
        usage_query
    );

    let withdrawal = WithdrawQueuedMessageCommand::new(
        request_id("withdrawal-request"),
        thread(),
        message_id(),
        request_id("original-request"),
    );
    let mut message = Builder::new(HeapAllocator::new());
    encode_withdraw_queued_message_request(
        message.init_root::<composer_state_capnp::withdraw_queued_message_request::Builder>(),
        &withdrawal,
    );
    let words = serialize::write_message_to_words(&message);
    let mut encoded = words.as_slice();
    let decoded = serialize::read_message_from_flat_slice(&mut encoded, ReaderOptions::new())
        .expect("read withdrawal query");
    assert_eq!(
        decode_withdraw_queued_message_request(
            decoded
                .get_root::<composer_state_capnp::withdraw_queued_message_request::Reader>()
                .expect("withdrawal query root"),
            request_id("withdrawal-request"),
        )
        .expect("decode withdrawal query"),
        withdrawal
    );
}

#[test]
fn withdrawal_result_round_trips_with_exact_outer_correlation() {
    let outer_request_id = request_id("withdrawal-request");
    let original = QueuedMessageWithdrawalResult {
        receipt: CommandReceipt {
            request_id: outer_request_id.clone(),
            disposition: ReceiptDisposition::Duplicate,
        },
        thread_id: thread(),
        message_id: message_id(),
        original_request_id: request_id("original-request"),
        accepted_at: UnixMillis::from_millis(-11),
        outcome: QueuedMessageWithdrawalOutcome::TooLate,
    };
    let mut message = Builder::new(HeapAllocator::new());
    encode_queued_message_withdrawal_result(
        message.init_root::<composer_state_capnp::queued_message_withdrawal_result::Builder>(),
        &outer_request_id,
        &original,
    )
    .expect("encode withdrawal result");
    let words = serialize::write_message_to_words(&message);
    let mut encoded = words.as_slice();
    let decoded = serialize::read_message_from_flat_slice(&mut encoded, ReaderOptions::new())
        .expect("read withdrawal result");
    assert_eq!(
        decode_queued_message_withdrawal_result(
            decoded
                .get_root::<composer_state_capnp::queued_message_withdrawal_result::Reader>()
                .expect("withdrawal result root"),
            &outer_request_id,
        )
        .expect("decode withdrawal result"),
        original
    );
}

#[test]
fn scope_and_receipt_correlation_mismatches_are_rejected() {
    let recalled_query =
        ReadRecalledMessage::new(thread(), message_id(), request_id("original-request"));
    let recalled_value = recalled(None);
    let wrong_recalled = RecalledMessageResult::new(
        ThreadId::parse("other-thread").expect("thread"),
        message_id(),
        request_id("original-request"),
        None,
    )
    .expect("wrong recalled result");
    assert!(matches!(
        validate_recalled_message_scope(&recalled_query, &wrong_recalled),
        Err(ComposerStateCodecError::ScopeMismatch { .. })
    ));
    assert!(validate_recalled_message_scope(&recalled_query, &recalled_value).is_ok());

    let usage_query = ReadRunUsage::new(thread(), run());
    let wrong_usage = RunUsageResult::new(
        ThreadId::parse("other-thread").expect("thread"),
        run(),
        None,
    )
    .expect("wrong usage result");
    assert!(matches!(
        validate_run_usage_scope(&usage_query, &wrong_usage),
        Err(ComposerStateCodecError::ScopeMismatch { .. })
    ));

    let result = QueuedMessageWithdrawalResult {
        receipt: CommandReceipt {
            request_id: request_id("nested-request"),
            disposition: ReceiptDisposition::Accepted,
        },
        thread_id: thread(),
        message_id: message_id(),
        original_request_id: request_id("original-request"),
        accepted_at: UnixMillis::EPOCH,
        outcome: QueuedMessageWithdrawalOutcome::Withdrawn,
    };
    assert!(matches!(
        artisan_protocol::composer_state::validate_withdrawal_response_correlation(
            &request_id("outer-request"),
            &result,
        ),
        Err(ComposerStateCodecError::ResponseCorrelationMismatch { .. })
    ));
}

#[test]
fn parent_envelope_dispatch_round_trips_composer_state() {
    use artisan_protocol::{ClientRequest, FrameId, ProtocolVersion, ResponsePayload, ServerResponse, WireEnvelope, WireEnvelopeBody, encode_envelope, decode_envelope};
    use artisan_domain::{Command, Query};
    let request = artisan_domain::RequestId::parse("state-envelope").unwrap();
    let query = ReadRunUsage::new(thread(), run());
    let command = WithdrawQueuedMessageCommand::new(request.clone(), thread(), artisan_domain::MessageId::parse("queued-message").unwrap(), artisan_domain::RequestId::parse("original-message").unwrap());
    let bodies = vec![
        WireEnvelopeBody::Request(ClientRequest::Query(Query::ReadRunUsage(query))),
        WireEnvelopeBody::Request(ClientRequest::Command(Command::WithdrawQueuedMessage(command))),
        WireEnvelopeBody::Response(ServerResponse { request_id: request.clone(), payload: ResponsePayload::RunUsage(usage_result()) }),
    ];
    for body in bodies {
        let envelope = WireEnvelope { protocol_version: ProtocolVersion::V1, frame_id: FrameId::parse(request.as_str()).unwrap(), sent_at: UnixMillis::from_millis(20), body };
        assert!(decode_envelope(&encode_envelope(&envelope).unwrap()).unwrap() == envelope);
    }
}

#[test]
fn parent_envelope_dispatch_round_trips_failed_dispatch_arms() {
    use artisan_protocol::{ClientRequest, FrameId, ProtocolVersion, ResponsePayload, ServerResponse, WireEnvelope, WireEnvelopeBody, encode_envelope, decode_envelope};
    use artisan_domain::Query;
    let request = artisan_domain::RequestId::parse("failed-envelope").unwrap();
    let query = ListFailedMessages::new(thread(), 5).expect("failed query");
    let listing =
        FailedMessageListing::new(thread(), 5, 1, vec![failed_summary()]).expect("listing");
    let bodies = vec![
        WireEnvelopeBody::Request(ClientRequest::Query(Query::ListFailedMessages(query))),
        WireEnvelopeBody::Response(ServerResponse { request_id: request.clone(), payload: ResponsePayload::FailedMessages(listing) }),
    ];
    for body in bodies {
        let envelope = WireEnvelope { protocol_version: ProtocolVersion::V1, frame_id: FrameId::parse(request.as_str()).unwrap(), sent_at: UnixMillis::from_millis(20), body };
        assert!(decode_envelope(&encode_envelope(&envelope).unwrap()).unwrap() == envelope);
    }
}

fn failed_summary() -> FailedMessageSummary {
    let message_id = message_id();
    let thread_id = thread();
    FailedMessageSummary {
        message_id: message_id.clone(),
        thread_id: thread_id.clone(),
        original_request_id: request_id("original-request"),
        text: Some(AuthoredText::parse("hello").expect("text")),
        attachments: vec![
            ImageAttachmentRef::new(
                message_id,
                thread_id,
                0,
                "image/png",
                "capture.png",
                3,
                [7; 32],
            )
            .expect("reference"),
        ],
        accepted_at: UnixMillis::from_millis(300),
        failed_at: UnixMillis::from_millis(500),
        reason: DispatchError::parse(
            "provider continuation unavailable: the prior run was interrupted with unknown outcome; start a new chat to continue".to_owned(),
        )
        .expect("diagnostic"),
    }
}

fn round_trip_failed_listing(value: &FailedMessageListing) -> FailedMessageListing {
    let mut message = Builder::new(HeapAllocator::new());
    encode_failed_message_listing(
        message.init_root::<composer_state_capnp::failed_message_listing::Builder>(),
        value,
    )
    .expect("encode failed listing");
    let words = serialize::write_message_to_words(&message);
    let mut encoded = words.as_slice();
    let decoded = serialize::read_message_from_flat_slice(&mut encoded, ReaderOptions::new())
        .expect("read failed listing");
    decode_failed_message_listing(
        decoded
            .get_root::<composer_state_capnp::failed_message_listing::Reader>()
            .expect("failed listing root"),
    )
    .expect("decode failed listing")
}

#[test]
fn failed_request_round_trips_exact_scope_and_limit() {
    let query = ListFailedMessages::new(thread(), 10).expect("failed query");
    let mut message = Builder::new(HeapAllocator::new());
    encode_list_failed_messages_request(
        message.init_root::<composer_state_capnp::list_failed_messages_request::Builder>(),
        &query,
    )
    .expect("encode failed query");
    let words = serialize::write_message_to_words(&message);
    let mut encoded = words.as_slice();
    let decoded = serialize::read_message_from_flat_slice(&mut encoded, ReaderOptions::new())
        .expect("read failed query");
    assert_eq!(
        decode_list_failed_messages_request(
            decoded
                .get_root::<composer_state_capnp::list_failed_messages_request::Reader>()
                .expect("failed query root")
        )
        .expect("decode failed query"),
        query
    );
    assert!(ListFailedMessages::new(thread(), 0).is_err());
    assert!(ListFailedMessages::new(thread(), FAILED_MESSAGE_LIST_MAX + 1).is_err());
}

#[test]
fn failed_listing_round_trips_reason_text_attachments_and_counts() {
    let original =
        FailedMessageListing::new(thread(), 2, 3, vec![failed_summary()]).expect("listing");
    let received = round_trip_failed_listing(&original);
    assert_eq!(received, original);
    assert!(received.has_more());
    assert_eq!(received.total_count(), 3);
    let row = &received.messages()[0];
    assert_eq!(
        row.reason.as_str(),
        "provider continuation unavailable: the prior run was interrupted with unknown outcome; start a new chat to continue"
    );
    assert_eq!(
        row.text.as_ref().map(AuthoredText::as_str),
        Some("hello")
    );
    assert_eq!(row.attachments.len(), 1);
    assert_eq!(row.failed_at, UnixMillis::from_millis(500));
}

#[test]
fn failed_listing_rejects_missing_reason_and_inconsistent_counts() {
    let mut message = Builder::new(HeapAllocator::new());
    {
        let mut listing =
            message.init_root::<composer_state_capnp::failed_message_listing::Builder>();
        listing.set_thread_id(thread().as_str());
        listing.set_limit(32);
        listing.set_total_count(1);
        listing.set_has_more(false);
        let mut messages = listing.reborrow().init_messages(1);
        let mut row = messages.reborrow().get(0);
        row.set_message_id(message_id().as_str());
        row.set_thread_id(thread().as_str());
        row.set_original_request_id("original-request");
        row.set_text("hello");
        row.set_accepted_at_millis(300);
        row.set_failed_at_millis(500);
    }
    let words = serialize::write_message_to_words(&message);
    let mut encoded = words.as_slice();
    let decoded = serialize::read_message_from_flat_slice(&mut encoded, ReaderOptions::new())
        .expect("read malformed failed listing");
    assert!(matches!(
        decode_failed_message_listing(
            decoded
                .get_root::<composer_state_capnp::failed_message_listing::Reader>()
                .expect("failed listing root")
        ),
        Err(ComposerStateCodecError::Listing { .. })
    ));

    let original =
        FailedMessageListing::new(thread(), 1, 2, vec![]).expect("listing");
    let mut message = Builder::new(HeapAllocator::new());
    let mut listing = message.init_root::<composer_state_capnp::failed_message_listing::Builder>();
    encode_failed_message_listing(listing.reborrow(), &original).expect("encode listing");
    listing.set_has_more(true);
    drop(listing);
    let words = serialize::write_message_to_words(&message);
    let mut encoded = words.as_slice();
    let decoded = serialize::read_message_from_flat_slice(&mut encoded, ReaderOptions::new())
        .expect("read malformed failed listing");
    assert!(matches!(
        decode_failed_message_listing(
            decoded
                .get_root::<composer_state_capnp::failed_message_listing::Reader>()
                .expect("failed listing root")
        ),
        Err(ComposerStateCodecError::Listing { .. })
    ));
}
