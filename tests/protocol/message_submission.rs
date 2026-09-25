//! Wire coverage for Forge-owned submissions: sending a composer draft by
//! revision, the pushed message outbox, failed-message retry and recovery by
//! identity, and the edit withdrawal that recalls a payload into the Forge
//! draft.

use artisan_domain::{
    AuthoredText, CatalogOptionId, CatalogSelection, Command, ComposerDraftRevision,
    ComposerDraftSubmitted, DispatchError, DraftSubmissionOutcome, EngineConfigRevision, EngineId,
    EngineProfileId, Event, FailedMessageListing, FailedMessageRecovered, FailedMessageRetried,
    FailedMessageRetryOutcome, FailedMessageSummary, FailedMessageTarget, MessageId, MessageOutbox,
    ModelFavoriteId, QueuedMessageListOrder, QueuedMessageListing, QueuedMessageState,
    QueuedMessageSummary, ReceiptDisposition, RecoverFailedMessage, RequestId, RetryFailedMessage,
    SubmissionRefusal, SubmissionRefusalKind, SubmitComposerDraft, ThreadId, UnixMillis,
    WithdrawQueuedMessageCommand,
};
use artisan_protocol::{
    ClientRequest, EventCursor, FrameId, ProtocolVersion, ResponsePayload, ServerEvent,
    ServerResponse, WireEnvelope, WireEnvelopeBody, decode_envelope, encode_envelope,
};

const REQUEST: &str = "native-message-0199b1a2-7c3d-7e4f-8a5b-6c7d8e9f0a1b";

fn request_id() -> RequestId {
    RequestId::parse(REQUEST).unwrap()
}

fn thread() -> ThreadId {
    ThreadId::parse("outbox-thread").unwrap()
}

fn target() -> FailedMessageTarget {
    FailedMessageTarget {
        thread_id: thread(),
        message_id: MessageId::parse("failed-message").unwrap(),
        original_request_id: RequestId::parse("original-request").unwrap(),
    }
}

fn envelope(body: WireEnvelopeBody) -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse(REQUEST).unwrap(),
        sent_at: UnixMillis::from_millis(7),
        body,
    }
}

fn round_trip(body: WireEnvelopeBody) {
    let envelope = envelope(body);
    let decoded = decode_envelope(&encode_envelope(&envelope).expect("encode")).expect("decode");
    assert!(decoded == envelope, "envelope must round trip exactly");
}

fn response(payload: ResponsePayload) -> WireEnvelopeBody {
    WireEnvelopeBody::Response(ServerResponse {
        request_id: request_id(),
        payload,
    })
}

fn queued(id: &str, state: QueuedMessageState, reason: Option<&str>) -> QueuedMessageSummary {
    QueuedMessageSummary {
        message_id: MessageId::parse(id).unwrap(),
        thread_id: thread(),
        original_request_id: RequestId::parse(format!("{id}-request")).unwrap(),
        text: Some(AuthoredText::parse("hello").unwrap()),
        attachments: Vec::new(),
        accepted_at: UnixMillis::from_millis(10),
        last_error: reason.map(|reason| DispatchError::parse(reason.to_owned()).unwrap()),
        state,
        engine: Some(EngineId::Claude),
    }
}

fn failed(retryable: bool) -> FailedMessageSummary {
    FailedMessageSummary {
        message_id: MessageId::parse("failed-message").unwrap(),
        thread_id: thread(),
        original_request_id: RequestId::parse("original-request").unwrap(),
        text: None,
        attachments: Vec::new(),
        accepted_at: UnixMillis::from_millis(5),
        failed_at: UnixMillis::from_millis(6),
        reason: DispatchError::parse("engine unavailable".to_owned()).unwrap(),
        retryable,
    }
}

fn outbox() -> MessageOutbox {
    let queued = QueuedMessageListing::new(
        thread(),
        QueuedMessageListOrder::OldestFirst,
        32,
        2,
        vec![
            queued("first", QueuedMessageState::Dispatching, None),
            queued(
                "second",
                QueuedMessageState::Queued,
                Some("engine profile unavailable"),
            ),
        ],
    )
    .unwrap();
    let failed = FailedMessageListing::new(thread(), 32, 1, vec![failed(true)]).unwrap();
    MessageOutbox::new(queued, failed).unwrap()
}

#[test]
fn message_outbox_event_round_trips_every_row_state() {
    round_trip(WireEnvelopeBody::Event(ServerEvent {
        cursor: EventCursor::new(3).unwrap(),
        event: Event::MessageOutbox(outbox()),
    }));
}

#[test]
fn empty_outbox_round_trips() {
    let empty = MessageOutbox::new(
        QueuedMessageListing::new(thread(), QueuedMessageListOrder::OldestFirst, 32, 0, vec![])
            .unwrap(),
        FailedMessageListing::new(thread(), 32, 0, vec![]).unwrap(),
    )
    .unwrap();
    round_trip(WireEnvelopeBody::Event(ServerEvent {
        cursor: EventCursor::new(1).unwrap(),
        event: Event::MessageOutbox(empty),
    }));
}

#[test]
fn outbox_listings_must_name_one_thread() {
    let other =
        FailedMessageListing::new(ThreadId::parse("other").unwrap(), 32, 0, vec![]).unwrap();
    assert!(MessageOutbox::new(outbox().queued().clone(), other).is_err());
}

#[test]
fn retry_and_recovery_requests_carry_only_the_failed_identity() {
    round_trip(WireEnvelopeBody::Request(ClientRequest::Command(
        Command::RetryFailedMessage(RetryFailedMessage {
            request_id: request_id(),
            target: target(),
        }),
    )));
    round_trip(WireEnvelopeBody::Request(ClientRequest::Command(
        Command::RecoverFailedMessage(RecoverFailedMessage {
            request_id: request_id(),
            target: target(),
        }),
    )));
}

#[test]
fn retry_and_recovery_answers_round_trip() {
    for outcome in [
        FailedMessageRetryOutcome::Requeued,
        FailedMessageRetryOutcome::NotRetryable,
    ] {
        round_trip(response(ResponsePayload::FailedMessageRetried(
            FailedMessageRetried {
                request_id: request_id(),
                target: target(),
                outcome,
            },
        )));
    }
    for (new_thread_id, disposition) in [
        (
            Some(ThreadId::parse("recovered").unwrap()),
            ReceiptDisposition::Accepted,
        ),
        (
            Some(ThreadId::parse("recovered").unwrap()),
            ReceiptDisposition::Duplicate,
        ),
        (None, ReceiptDisposition::Accepted),
    ] {
        round_trip(response(ResponsePayload::FailedMessageRecovered(
            FailedMessageRecovered {
                request_id: request_id(),
                target: target(),
                new_thread_id,
                disposition,
            },
        )));
    }
}

#[test]
fn answers_must_correlate_with_their_response() {
    let other = RequestId::parse("another-request").unwrap();
    assert!(
        encode_envelope(&envelope(response(ResponsePayload::FailedMessageRetried(
            FailedMessageRetried {
                request_id: other.clone(),
                target: target(),
                outcome: FailedMessageRetryOutcome::Requeued,
            },
        ))))
        .is_err()
    );
    assert!(
        encode_envelope(&envelope(response(
            ResponsePayload::FailedMessageRecovered(FailedMessageRecovered {
                request_id: other,
                target: target(),
                new_thread_id: None,
                disposition: ReceiptDisposition::Accepted,
            },)
        )))
        .is_err()
    );
}

#[test]
fn edit_withdrawal_recalls_into_the_draft_on_the_wire() {
    let target = target();
    for command in [
        WithdrawQueuedMessageCommand::new(
            request_id(),
            target.thread_id.clone(),
            target.message_id.clone(),
            target.original_request_id.clone(),
        ),
        WithdrawQueuedMessageCommand::new(
            request_id(),
            target.thread_id.clone(),
            target.message_id.clone(),
            target.original_request_id.clone(),
        )
        .recalling_to_draft(),
    ] {
        round_trip(WireEnvelopeBody::Request(ClientRequest::Command(
            Command::WithdrawQueuedMessage(command),
        )));
    }
}

#[test]
fn draft_submission_names_the_revision_and_the_users_selection() {
    let selection = CatalogSelection {
        model_id: ModelFavoriteId::parse("codex-sol").unwrap(),
        profile_id: Some(EngineProfileId::parse("default").unwrap()),
        reasoning_effort: Some(CatalogOptionId::parse("low").unwrap()),
        speed: None,
        context_window: Some(CatalogOptionId::parse("standard").unwrap()),
        permission: Some(CatalogOptionId::parse("autonomous").unwrap()),
    };
    for selection in [None, Some(selection)] {
        round_trip(WireEnvelopeBody::Request(ClientRequest::Command(
            Command::SubmitComposerDraft(SubmitComposerDraft {
                request_id: request_id(),
                thread_id: thread(),
                draft_revision: ComposerDraftRevision::new(4).unwrap(),
                selection,
            }),
        )));
    }
}

#[test]
fn draft_submission_answers_round_trip_and_correlate() {
    let answer = |request_id, outcome| {
        response(ResponsePayload::ComposerDraftSubmitted(
            ComposerDraftSubmitted {
                request_id,
                thread_id: thread(),
                draft_revision: ComposerDraftRevision::new(4).unwrap(),
                outcome,
            },
        ))
    };
    for disposition in [ReceiptDisposition::Accepted, ReceiptDisposition::Duplicate] {
        round_trip(answer(
            request_id(),
            DraftSubmissionOutcome::Queued {
                message_id: MessageId::parse("queued-message").unwrap(),
                disposition,
                cleared_revision: ComposerDraftRevision::new(5).unwrap(),
                engine_config_revision: EngineConfigRevision::new(3).unwrap(),
            },
        ));
    }
    for kind in [
        SubmissionRefusalKind::InvalidSelection,
        SubmissionRefusalKind::NoSelection,
        SubmissionRefusalKind::EngineNotReady,
        SubmissionRefusalKind::RunStarting,
        SubmissionRefusalKind::AttachmentRejected,
    ] {
        round_trip(answer(
            request_id(),
            DraftSubmissionOutcome::Refused(
                SubmissionRefusal::new(kind, "Codex account sign-in is required.").unwrap(),
            ),
        ));
    }
    for current_revision in [None, Some(ComposerDraftRevision::new(9).unwrap())] {
        round_trip(answer(
            request_id(),
            DraftSubmissionOutcome::Stale { current_revision },
        ));
    }
    let other = RequestId::parse("another-request").unwrap();
    let outcome = DraftSubmissionOutcome::Stale {
        current_revision: None,
    };
    assert!(encode_envelope(&envelope(answer(other, outcome))).is_err());
}
