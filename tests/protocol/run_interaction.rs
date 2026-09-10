//! A-approve wire proof: additive approval/question RPCs plus result receipts.
//!
//! Every new arm round-trips through the owned codec without disturbing the
//! frozen v1 arms: a pre-existing stop-run frame still encodes and decodes
//! byte-identically, and the new receipts enforce their nested request
//! correlation exactly like the established receipt arms.

use artisan_domain::{
    Command, ObservationId, ReceiptDisposition, RequestId, RespondApproval, RespondQuestion, RunId,
    ThreadId, UnixMillis,
};
use artisan_protocol::{
    ClientRequest, FrameId, ProtocolVersion, RespondApprovalReceipt, RespondQuestionReceipt,
    ResponsePayload, RunInteractionOutcome, ServerResponse, WireEnvelope, WireEnvelopeBody,
    decode_envelope, encode_envelope,
};

fn request_id(value: &str) -> RequestId {
    RequestId::parse(value).expect("fixture request id should be valid")
}

fn thread_id() -> ThreadId {
    ThreadId::parse("approve-thread").expect("fixture thread id should be valid")
}

fn run_id() -> RunId {
    RunId::parse("approve-run").expect("fixture run id should be valid")
}

fn frame_id(value: &str) -> FrameId {
    FrameId::parse(value).expect("fixture frame id should be valid")
}

fn request_frame(command: Command) -> WireEnvelope {
    let frame = command.request_id().as_str().to_owned();
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: frame_id(&frame),
        sent_at: UnixMillis::EPOCH,
        body: WireEnvelopeBody::Request(ClientRequest::Command(command)),
    }
}

fn response_frame(payload: ResponsePayload, request: &str) -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: frame_id("server-frame-approve"),
        sent_at: UnixMillis::EPOCH,
        body: WireEnvelopeBody::Response(ServerResponse {
            request_id: request_id(request),
            payload,
        }),
    }
}

fn assert_roundtrip(value: &WireEnvelope) {
    let bytes = encode_envelope(value).expect("new arms should encode");
    let decoded = decode_envelope(&bytes).expect("new arms should decode");
    assert!(decoded == *value, "wire changed the interaction payload");
}

fn approval_command(approved: bool) -> Command {
    Command::RespondApproval(RespondApproval::new(
        request_id("respond-approval-1"),
        thread_id(),
        run_id(),
        ObservationId::parse("approval-1").expect("fixture approval id should be valid"),
        approved,
    ))
}

fn question_command(answers: Vec<String>) -> Command {
    Command::RespondQuestion(
        RespondQuestion::new(
            request_id("respond-question-1"),
            thread_id(),
            run_id(),
            ObservationId::parse("question-1").expect("fixture question id should be valid"),
            answers,
        )
        .expect("fixture answers should be valid"),
    )
}

#[test]
fn approval_and_question_requests_roundtrip() {
    assert_roundtrip(&request_frame(approval_command(true)));
    assert_roundtrip(&request_frame(approval_command(false)));
    assert_roundtrip(&request_frame(question_command(vec!["first".to_owned()])));
    assert_roundtrip(&request_frame(question_command(Vec::new())));
}

#[test]
fn approval_receipts_roundtrip_every_outcome_and_disposition() {
    for outcome in [
        RunInteractionOutcome::Applied,
        RunInteractionOutcome::UnknownTarget,
        RunInteractionOutcome::AlreadyResolved,
        RunInteractionOutcome::WrongRun,
    ] {
        for disposition in [ReceiptDisposition::Accepted, ReceiptDisposition::Duplicate] {
            assert_roundtrip(&response_frame(
                ResponsePayload::ApprovalResponse(RespondApprovalReceipt {
                    request_id: request_id("respond-approval-1"),
                    thread_id: thread_id(),
                    run_id: run_id(),
                    approval_id: ObservationId::parse("approval-1")
                        .expect("fixture approval id should be valid"),
                    approved: true,
                    outcome,
                    disposition,
                }),
                "respond-approval-1",
            ));
        }
    }
}

#[test]
fn question_receipts_roundtrip_answers_and_skips() {
    assert_roundtrip(&response_frame(
        ResponsePayload::QuestionResponse(RespondQuestionReceipt {
            request_id: request_id("respond-question-1"),
            thread_id: thread_id(),
            run_id: run_id(),
            question_id: ObservationId::parse("question-1")
                .expect("fixture question id should be valid"),
            answers: vec!["first".to_owned(), "second".to_owned()],
            outcome: RunInteractionOutcome::Applied,
            disposition: ReceiptDisposition::Accepted,
        }),
        "respond-question-1",
    ));
    assert_roundtrip(&response_frame(
        ResponsePayload::QuestionResponse(RespondQuestionReceipt {
            request_id: request_id("respond-question-1"),
            thread_id: thread_id(),
            run_id: run_id(),
            question_id: ObservationId::parse("question-1")
                .expect("fixture question id should be valid"),
            answers: Vec::new(),
            outcome: RunInteractionOutcome::Applied,
            disposition: ReceiptDisposition::Accepted,
        }),
        "respond-question-1",
    ));
}

#[test]
fn interaction_receipts_require_exact_nested_request_identity() {
    let mut value = response_frame(
        ResponsePayload::ApprovalResponse(RespondApprovalReceipt {
            request_id: request_id("respond-approval-1"),
            thread_id: thread_id(),
            run_id: run_id(),
            approval_id: ObservationId::parse("approval-1")
                .expect("fixture approval id should be valid"),
            approved: false,
            outcome: RunInteractionOutcome::Applied,
            disposition: ReceiptDisposition::Accepted,
        }),
        "respond-approval-1",
    );
    assert_roundtrip(&value);
    let WireEnvelopeBody::Response(response) = &mut value.body else {
        unreachable!()
    };
    response.request_id = request_id("different-request");
    assert!(encode_envelope(&value).is_err());

    let mut value = response_frame(
        ResponsePayload::QuestionResponse(RespondQuestionReceipt {
            request_id: request_id("respond-question-1"),
            thread_id: thread_id(),
            run_id: run_id(),
            question_id: ObservationId::parse("question-1")
                .expect("fixture question id should be valid"),
            answers: vec!["yes".to_owned()],
            outcome: RunInteractionOutcome::Applied,
            disposition: ReceiptDisposition::Accepted,
        }),
        "respond-question-1",
    );
    assert_roundtrip(&value);
    let WireEnvelopeBody::Response(response) = &mut value.body else {
        unreachable!()
    };
    let ResponsePayload::QuestionResponse(receipt) = &mut response.payload else {
        unreachable!()
    };
    receipt.request_id = request_id("another-request");
    assert!(encode_envelope(&value).is_err());
}

#[test]
fn oversized_answer_lists_are_rejected() {
    let value = response_frame(
        ResponsePayload::QuestionResponse(RespondQuestionReceipt {
            request_id: request_id("respond-question-1"),
            thread_id: thread_id(),
            run_id: run_id(),
            question_id: ObservationId::parse("question-1")
                .expect("fixture question id should be valid"),
            answers: vec!["answer".to_owned(); 17],
            outcome: RunInteractionOutcome::Applied,
            disposition: ReceiptDisposition::Accepted,
        }),
        "respond-question-1",
    );
    assert!(encode_envelope(&value).is_err());
}

#[test]
fn frozen_v1_stop_run_frames_survive_the_additive_arms() {
    // The v1 stop-run arms predate A-approve: they must encode and decode
    // unchanged with the new union members present in the schema.
    let request = WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: frame_id("stop-run-1"),
        sent_at: UnixMillis::EPOCH,
        body: WireEnvelopeBody::Request(ClientRequest::Command(Command::StopRun(
            artisan_domain::StopRun::new(request_id("stop-run-1"), thread_id(), run_id()),
        ))),
    };
    let bytes = encode_envelope(&request).expect("v1 request should encode");
    assert!(
        decode_envelope(&bytes).expect("v1 request should decode") == request,
        "v1 request should decode"
    );

    let response = response_frame(
        ResponsePayload::RunStopped(artisan_protocol::StopRunReceipt {
            request_id: request_id("stop-run-1"),
            thread_id: thread_id(),
            run_id: run_id(),
            disposition: artisan_protocol::StopRunDisposition::Requested,
        }),
        "stop-run-1",
    );
    let bytes = encode_envelope(&response).expect("v1 response should encode");
    assert!(
        decode_envelope(&bytes).expect("v1 response should decode") == response,
        "v1 response should decode"
    );
}

#[test]
fn mismatched_command_correlation_is_rejected_before_encoding() {
    let mut value = request_frame(approval_command(true));
    value.frame_id = frame_id("different-request");
    assert!(encode_envelope(&value).is_err());
}
