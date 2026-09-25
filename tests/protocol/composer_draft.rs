//! Envelope round trips for composer drafts, stored attachments, and
//! stored-attachment messages.

use artisan_domain::{
    AuthoredText, Command, ComposerAttachmentDigest, ComposerAttachmentRef,
    ComposerAttachmentResult, ComposerAttachmentUploaded, ComposerDraft, ComposerDraftResult,
    ComposerDraftRevision, ComposerDraftSaved, ComposerDraftScope, ImageAttachment, ImageMimeType,
    ProjectId, Query, QueueStoredMessage, ReadComposerAttachment, ReadComposerDraft, RequestId,
    RunId, SaveComposerDraft, SteerTarget, ThreadId, UnixMillis, UploadComposerAttachment,
};
use artisan_protocol::{
    ClientRequest, FrameId, ProtocolVersion, ResponsePayload, ServerResponse, WireEnvelope,
    WireEnvelopeBody, decode_envelope, encode_envelope,
};

const REQUEST: &str = "draft-envelope";

fn request_id() -> RequestId {
    RequestId::parse(REQUEST).unwrap()
}

fn thread_scope() -> ComposerDraftScope {
    ComposerDraftScope::Thread(ThreadId::parse("draft-thread").unwrap())
}

fn reference(byte: u8, name: &str) -> ComposerAttachmentRef {
    ComposerAttachmentRef::new(
        ComposerAttachmentDigest::new([byte; 32]),
        ImageMimeType::Webp,
        name,
        12,
    )
    .unwrap()
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

#[test]
fn draft_requests_round_trip_both_scopes_text_and_ordered_references() {
    for scope in [
        thread_scope(),
        ComposerDraftScope::Project(ProjectId::parse("draft-project").unwrap()),
    ] {
        let save = SaveComposerDraft::new(
            request_id(),
            scope.clone(),
            AuthoredText::parse("  unsent\ntext  ").unwrap(),
            vec![reference(2, "second.webp"), reference(1, "first.webp")],
        )
        .unwrap();
        round_trip(WireEnvelopeBody::Request(ClientRequest::Command(
            Command::SaveComposerDraft(save),
        )));
        round_trip(WireEnvelopeBody::Request(ClientRequest::Query(
            Query::ReadComposerDraft(ReadComposerDraft { scope }),
        )));
    }
    round_trip(WireEnvelopeBody::Request(ClientRequest::Query(
        Query::ReadComposerAttachment(ReadComposerAttachment {
            digest: ComposerAttachmentDigest::new([5; 32]),
        }),
    )));
    round_trip(WireEnvelopeBody::Request(ClientRequest::Command(
        Command::UploadComposerAttachment(UploadComposerAttachment {
            request_id: request_id(),
            image: ImageAttachment::new("image/png", vec![1, 2, 3], "paste.png").unwrap(),
        }),
    )));
}

#[test]
fn stored_messages_round_trip_absent_and_empty_text_and_steer_targets() {
    for (text, steer) in [
        (None, None),
        (Some(AuthoredText::empty()), None),
        (
            Some(AuthoredText::parse("look").unwrap()),
            Some(SteerTarget::new(RunId::parse("live-run").unwrap())),
        ),
    ] {
        let message = QueueStoredMessage::new(
            request_id(),
            ThreadId::parse("draft-thread").unwrap(),
            text,
            vec![reference(3, "shot.webp")],
            steer,
        )
        .unwrap();
        round_trip(WireEnvelopeBody::Request(ClientRequest::Command(
            Command::QueueStoredMessage(message),
        )));
    }
}

#[test]
fn draft_responses_round_trip_and_absent_drafts_stay_absent() {
    round_trip(response(ResponsePayload::ComposerDraftSaved(
        ComposerDraftSaved {
            request_id: request_id(),
            scope: thread_scope(),
            revision: ComposerDraftRevision::new(9).unwrap(),
        },
    )));
    round_trip(response(ResponsePayload::ComposerDraft(
        ComposerDraftResult {
            scope: thread_scope(),
            draft: None,
        },
    )));
    round_trip(response(ResponsePayload::ComposerDraft(
        ComposerDraftResult {
            scope: thread_scope(),
            draft: Some(
                ComposerDraft::new(
                    ComposerDraftRevision::new(3).unwrap(),
                    AuthoredText::empty(),
                    vec![reference(4, "kept.webp")],
                    UnixMillis::from_millis(99),
                )
                .unwrap(),
            ),
        },
    )));
    round_trip(response(ResponsePayload::ComposerAttachmentUploaded(
        ComposerAttachmentUploaded {
            request_id: request_id(),
            reference: reference(6, "uploaded.webp"),
        },
    )));
    round_trip(response(ResponsePayload::ComposerAttachment(
        ComposerAttachmentResult {
            digest: ComposerAttachmentDigest::new([6; 32]),
            mime_type: ImageMimeType::Webp,
            bytes: vec![9; 12],
        },
    )));
}

#[test]
fn receipts_must_echo_the_enclosing_request() {
    let other = RequestId::parse("someone-else").unwrap();
    for payload in [
        ResponsePayload::ComposerDraftSaved(ComposerDraftSaved {
            request_id: other.clone(),
            scope: thread_scope(),
            revision: ComposerDraftRevision::new(1).unwrap(),
        }),
        ResponsePayload::ComposerAttachmentUploaded(ComposerAttachmentUploaded {
            request_id: other.clone(),
            reference: reference(1, "a.webp"),
        }),
    ] {
        assert!(encode_envelope(&envelope(response(payload))).is_err());
    }
}
