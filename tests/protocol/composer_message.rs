//! Composer payloads retain their authored bytes and identities across the wire.
use artisan_domain::{
    AuthoredText, Command, ImageAttachment, ImageAttachmentRef, MessageId, Query, QueueMessage,
    QueueMessagePayload, ReadMessageImage, ReceiptDisposition, RequestId, ThreadId, UnixMillis,
};
use artisan_protocol::{
    ClientRequest, FrameId, MessageImageResult, ProtocolVersion, QueueMessageReceipt,
    ResponsePayload, ServerResponse, WireEnvelope, WireEnvelopeBody, decode_envelope,
    encode_envelope,
};

fn frame(body: WireEnvelopeBody) -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("composer-request").unwrap(),
        sent_at: UnixMillis::EPOCH,
        body,
    }
}
fn thread() -> ThreadId {
    ThreadId::parse("composer-thread").unwrap()
}
fn request(payload: QueueMessagePayload) -> WireEnvelope {
    frame(WireEnvelopeBody::Request(ClientRequest::Command(
        Command::QueueMessage(QueueMessage::new(
            RequestId::parse("composer-request").unwrap(),
            thread(),
            payload,
        )),
    )))
}
fn image(byte: u8, size: usize) -> ImageAttachment {
    ImageAttachment::new("image/png", vec![byte; size], format!("capture-{byte}.png")).unwrap()
}
fn assert_roundtrip(value: &WireEnvelope) {
    let bytes = encode_envelope(value).expect("encode");
    let decoded = decode_envelope(&bytes).expect("decode");
    assert!(
        decoded == *value,
        "wire changed authored payload or identity"
    );
}
#[test]
fn image_only_absent_empty_and_whitespace_text_remain_distinct() {
    let mut frames = Vec::new();
    for text in [
        None,
        Some(AuthoredText::empty()),
        Some(AuthoredText::parse(" \n\t").unwrap()),
    ] {
        let value =
            request(QueueMessagePayload::new(text, vec![image(3, 29), image(7, 71)]).unwrap());
        assert_roundtrip(&value);
        frames.push(encode_envelope(&value).unwrap());
    }
    assert_ne!(frames[0], frames[1]);
    assert_ne!(frames[1], frames[2]);
}
#[test]
fn full_twelve_mib_image_budget_roundtrips_below_frame_ceiling() {
    let payload = QueueMessagePayload::new(
        Some(AuthoredText::parse(" exact text\n😀 ").unwrap()),
        vec![
            image(1, 4 * 1024 * 1024),
            image(2, 4 * 1024 * 1024),
            image(3, 4 * 1024 * 1024),
        ],
    )
    .unwrap();
    let value = request(payload);
    let bytes = encode_envelope(&value).unwrap();
    assert!(bytes.len() < 16 * 1024 * 1024);
    assert!(decode_envelope(&bytes).unwrap() == value);
}
#[test]
fn changed_command_correlation_is_rejected_before_encoding() {
    let mut value = request(QueueMessagePayload::text_only("hello").unwrap());
    value.frame_id = FrameId::parse("different-request").unwrap();
    assert!(encode_envelope(&value).is_err());
}
#[test]
fn general_receipt_requires_exact_nested_request_identity() {
    for disposition in [ReceiptDisposition::Accepted, ReceiptDisposition::Duplicate] {
        let receipt = QueueMessageReceipt {
            request_id: RequestId::parse("composer-request").unwrap(),
            message_id: MessageId::parse("composer-message").unwrap(),
            thread_id: thread(),
            disposition,
        };
        let mut value = frame(WireEnvelopeBody::Response(ServerResponse {
            request_id: receipt.request_id.clone(),
            payload: ResponsePayload::MessageQueued(receipt),
        }));
        assert_roundtrip(&value);
        let WireEnvelopeBody::Response(response) = &mut value.body else {
            unreachable!()
        };
        response.request_id = RequestId::parse("different-request").unwrap();
        assert!(encode_envelope(&value).is_err());
    }
}
fn reference() -> ImageAttachmentRef {
    ImageAttachmentRef::new(
        MessageId::parse("composer-message").unwrap(),
        thread(),
        2,
        "image/png",
        "capture.png",
        37,
        [9; 32],
    )
    .unwrap()
}
#[test]
fn single_image_read_retains_full_ownership_and_integrity_metadata() {
    let reference = reference();
    assert_roundtrip(&frame(WireEnvelopeBody::Request(ClientRequest::Query(
        Query::ReadMessageImage(ReadMessageImage::new(
            reference.thread_id.clone(),
            reference.message_id.clone(),
            reference.index,
        )),
    ))));
    assert_roundtrip(&frame(WireEnvelopeBody::Response(ServerResponse {
        request_id: RequestId::parse("composer-request").unwrap(),
        payload: ResponsePayload::MessageImage(MessageImageResult {
            reference,
            bytes: vec![4; 37],
        }),
    })));
}
#[test]
fn image_read_with_wrong_byte_length_is_rejected() {
    let value = frame(WireEnvelopeBody::Response(ServerResponse {
        request_id: RequestId::parse("composer-request").unwrap(),
        payload: ResponsePayload::MessageImage(MessageImageResult {
            reference: reference(),
            bytes: vec![4; 36],
        }),
    }));
    let bytes = encode_envelope(&value).unwrap();
    assert!(decode_envelope(&bytes).is_err());
}

#[test]
fn stop_receipt_preserves_and_checks_its_nested_request_identity() {
    let mut value = frame(WireEnvelopeBody::Response(ServerResponse {
        request_id: RequestId::parse("composer-request").unwrap(),
        payload: ResponsePayload::RunStopped(artisan_protocol::StopRunReceipt {
            request_id: RequestId::parse("composer-request").unwrap(),
            thread_id: thread(),
            run_id: artisan_domain::RunId::parse("composer-run").unwrap(),
            disposition: artisan_protocol::StopRunDisposition::Requested,
        }),
    }));
    assert_roundtrip(&value);
    let WireEnvelopeBody::Response(response) = &mut value.body else { unreachable!() };
    let ResponsePayload::RunStopped(receipt) = &mut response.payload else { unreachable!() };
    receipt.request_id = RequestId::parse("another-request").unwrap();
    assert!(encode_envelope(&value).is_err());
}
