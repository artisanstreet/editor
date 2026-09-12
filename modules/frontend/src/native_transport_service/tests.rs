//! Unit tests for the native transport service: command/event framing,
//! response-family validation, intake retry classification, durable-save
//! retries, reconnect policy, and subscription continuity.
//!
//! Extracted verbatim from `native_transport_service.rs` during the module
//! split.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use super::{
    COMMAND_CAPACITY, ExpectedResponse, FrameFactory, IntakeRetry, NativeTransportCommand,
    PeerFailure, ReadinessValidationError, RequestAttemptError, RequestFailure, ServiceFailure,
    ServiceFailureCategory, StartupError, ThreadSelectionDecision, approval_stable_mutation,
    attach_mutation, build_reconnect_binding, contains_exact_project, contains_exact_thread,
    create_command_values, create_mutation, engine_config_stable_mutation, finite_duration,
    first_message_stable_mutation, known_thread_for_queue, make_request_frame,
    message_stable_mutation, payload_health_decision, project_repository_request, project_request,
    question_stable_mutation, reconnect_hello, rich_link_request, session_needs_reconnect,
    snapshot_request, thread_engine_settings_request, thread_selection_decision, threads_request,
    try_send_command, validate_readiness, validate_response_family,
};
use artisan_domain::UnixMillis;
use artisan_domain::{
    AttachProject, CONVERSATION_QUERY_MAX_TURNS, Command, ConversationCursor,
    ConversationQueryBounds, ConversationSnapshot, CreateThread, DirectoryId, DisplayName,
    EngineProfileId, ListProjectThreads, MessageBody, ModelFavoriteId, ProjectId, ProjectListing,
    ProjectSummary, Query, QueryTurnCount, QueueFirstMessage, ReceiptDisposition, RequestId,
    RootPath, SetThreadEngineConfig, ThreadId, ThreadListing, ThreadSummary, ThreadTitle,
};
use artisan_editor_cli::payload::PayloadHealth;
use artisan_protocol::{
    CatalogSnapshotWire, ClientRequest, ComposerCatalogResult, DirectoryPickOutcome, ErrorCode,
    FirstMessageReceipt, HelloCredential, ModelFavoritesSnapshot, ProjectRepository,
    ProjectRepositoryEntry, ProjectRepositoryQueryResult, ProtocolVersion, QueueMessageReceipt,
    RECONNECT_CAPABILITY_BYTES, ReconnectCapability, RegisteredEngineProfilesResult,
    ResponsePayload, SetModelFavoriteReceipt, SetThreadEngineConfigResult, WireEnvelopeBody,
    encode_envelope,
};
use artisan_transport::{
    ClientRequestError, DeadlineError, EnvelopeReceiveError, EnvelopeSendError, ExchangeError,
    FrameError, LoopbackTarget, OperationKind, PinnedIdentity,
};
use std::{num::NonZeroU32, time::Duration};

fn project(value: &str, name: &str) -> ProjectSummary {
    ProjectSummary {
        project_id: ProjectId::parse(value).expect("valid project"),
        display_name: DisplayName::parse(name).expect("valid display name"),
        root_path: RootPath::parse(format!("/{value}")).expect("valid root"),
        attached_at: UnixMillis::EPOCH,
    }
}

fn thread(value: &str, project_id: &str) -> ThreadSummary {
    ThreadSummary {
        has_active_work: false,
        last_message_at: None,
        thread_id: ThreadId::parse(value).expect("valid thread"),
        project_id: ProjectId::parse(project_id).expect("valid project"),
        title: artisan_domain::ThreadTitle::parse(value).expect("valid title"),
        created_at: UnixMillis::EPOCH,
        updated_at: UnixMillis::EPOCH,
    }
}

fn catalog_result(thread_id: &str, profile_id: &str) -> ComposerCatalogResult {
    let thread_id = ThreadId::parse(thread_id).expect("valid thread");
    let profile_id = EngineProfileId::parse(profile_id).expect("valid profile");
    let mut catalog =
        crate::native_model_catalog::NativeModelCatalog::offline().expect("bundled catalog");
    catalog.scope = Some(artisan_catalog::NativeCatalogScope {
        profile_id: profile_id.as_str().to_owned(),
        working_directory: "C:/workspace".to_owned(),
        workspace_trust: "safe".to_owned(),
    });
    let bytes = artisan_catalog::wire::encode_catalog(&catalog).expect("catalog wire");
    let snapshot = CatalogSnapshotWire::new(bytes).expect("bounded catalog wire");
    ComposerCatalogResult::new(thread_id, profile_id, snapshot).expect("catalog result")
}

#[test]
fn payload_acceptance_is_exact() {
    assert!(payload_health_decision(&PayloadHealth::Verified).is_ok());
    assert_eq!(
        payload_health_decision(&PayloadHealth::Modified(Vec::new())),
        Err(StartupError::PayloadUnverified)
    );
    assert_eq!(
        payload_health_decision(&PayloadHealth::Unverifiable),
        Err(StartupError::PayloadUnverified)
    );
}

#[test]
fn readiness_requires_endpoint_pid_and_pin_agreement() {
    let pin = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    assert!(validate_readiness("127.0.0.1:40123", 19, 19, pin, pin).is_ok());
    assert_eq!(
        validate_readiness("127.0.0.1:40123", 18, 19, pin, pin),
        Err(ReadinessValidationError::Pid)
    );
    assert_eq!(
        validate_readiness("127.0.0.1:40123", 19, 19, "not-a-pin", pin),
        Err(ReadinessValidationError::Certificate)
    );
    assert_eq!(
        validate_readiness("192.168.0.1:40123", 19, 19, pin, pin),
        Err(ReadinessValidationError::Endpoint)
    );
}

#[test]
fn frames_are_unique_and_seed_correlations() {
    let mut frames = FrameFactory::new();
    let first = frames.next().expect("first frame");
    let second = frames.next().expect("second frame");
    assert_ne!(first.frame_id, second.frame_id);
    assert_ne!(first.sent_at, UnixMillis::MIN);
    assert_eq!(
        first
            .frame_id
            .to_request_id()
            .expect("correlation")
            .as_str(),
        first.frame_id.as_str()
    );
}

#[test]
fn request_frame_stamps_a_validated_protocol_correlation() {
    let mut frames = FrameFactory::new();
    let (envelope, request_id) =
        make_request_frame(&mut frames, ProtocolVersion::V1, project_request())
            .expect("request frame");
    assert_eq!(
        envelope.frame_id.to_request_id().expect("request id"),
        request_id
    );
    assert!(envelope.validate_correlation().is_ok());
    assert!(matches!(envelope.body, WireEnvelopeBody::Request(_)));
}

#[test]
fn picker_and_query_retries_are_fresh_but_mutation_retries_are_stable() {
    let mut frames = FrameFactory::new();
    let (first_picker, first_picker_id) = make_request_frame(
        &mut frames,
        ProtocolVersion::V1,
        ClientRequest::PickDirectory,
    )
    .expect("first picker frame");
    let (second_picker, second_picker_id) = make_request_frame(
        &mut frames,
        ProtocolVersion::V1,
        ClientRequest::PickDirectory,
    )
    .expect("second picker frame");
    assert_ne!(first_picker.frame_id, second_picker.frame_id);
    assert_ne!(first_picker_id, second_picker_id);

    let directory_id = DirectoryId::parse("directory-a").expect("directory");
    let attach = attach_mutation(&mut frames, directory_id.clone()).expect("attach mutation");
    let (attach_first, attach_first_id) = attach
        .envelope(ProtocolVersion::V1)
        .expect("attach envelope");
    let (attach_retry, attach_retry_id) = attach
        .envelope(ProtocolVersion::V1)
        .expect("attach retry envelope");
    assert_eq!(attach_first.frame_id, attach_retry.frame_id);
    assert_eq!(attach_first.sent_at, attach_retry.sent_at);
    assert_eq!(attach_first_id, attach_retry_id);
    assert_eq!(
        attach_first.frame_id.to_request_id().expect("attach id"),
        attach_first_id
    );
    assert!(matches!(
        attach_first.body,
        WireEnvelopeBody::Request(ClientRequest::Command(Command::AttachProject(
            AttachProject { request_id, directory_id: selected }
        ))) if request_id == attach_first_id && selected == directory_id
    ));

    let (query_retry, query_retry_id) =
        make_request_frame(&mut frames, ProtocolVersion::V1, project_request())
            .expect("query retry frame");
    assert_ne!(query_retry.frame_id, attach_retry.frame_id);
    assert_ne!(query_retry_id, attach_retry_id);

    let title = ThreadTitle::parse("New thread").expect("title");
    let project_id = ProjectId::parse("project-a").expect("project");
    let create =
        create_mutation(&mut frames, project_id.clone(), title.clone()).expect("create mutation");
    let (create_first, create_first_id) = create
        .envelope(ProtocolVersion::V1)
        .expect("create envelope");
    let (create_retry, create_retry_id) = create
        .envelope(ProtocolVersion::V1)
        .expect("create retry envelope");
    assert_eq!(create_first.frame_id, create_retry.frame_id);
    assert_eq!(create_first.sent_at, create_retry.sent_at);
    assert_eq!(create_first_id, create_retry_id);
    assert!(matches!(
        create_first.body,
        WireEnvelopeBody::Request(ClientRequest::Command(Command::CreateThread(
            CreateThread { request_id, project_id: selected, title: selected_title }
        ))) if request_id == create_first_id
            && selected == project_id
            && selected_title == title
    ));
}

#[test]
fn stable_retry_plan_keeps_the_complete_mutation_without_a_second_identity() {
    let mut frames = FrameFactory::new();
    let project_id = ProjectId::parse("project-a").expect("project");
    let title = ThreadTitle::parse("New thread").expect("title");
    let mutation =
        create_mutation(&mut frames, project_id.clone(), title.clone()).expect("create mutation");
    let (original_frame, original_id) = mutation.envelope(ProtocolVersion::V1).expect("frame");
    let retry = IntakeRetry::Create(mutation);
    let IntakeRetry::Create(stable) = retry else {
        panic!("create retry plan changed variant");
    };
    let (retry_frame, retry_id) = stable.envelope(ProtocolVersion::V1).expect("retry frame");
    assert_eq!(original_frame.frame_id, retry_frame.frame_id);
    assert_eq!(original_frame.sent_at, retry_frame.sent_at);
    assert_eq!(original_id, retry_id);
    assert_eq!(create_command_values(&stable), Some((project_id, title)));
}

#[test]
fn response_families_cover_intake_mutations_and_reject_cross_family_payloads() {
    let attached = project("project-a", "A");
    assert!(matches!(
        validate_response_family(
            ExpectedResponse::AttachedProject,
            ResponsePayload::AttachedProject {
                project: attached.clone(),
                disposition: ReceiptDisposition::Accepted,
            },
        ),
        Ok(ResponsePayload::AttachedProject { .. })
    ));

    let created = thread("thread-a", "project-a");
    assert!(matches!(
        validate_response_family(
            ExpectedResponse::CreatedThread,
            ResponsePayload::CreatedThread {
                thread: created,
                disposition: ReceiptDisposition::Duplicate,
            },
        ),
        Ok(ResponsePayload::CreatedThread { .. })
    ));

    let listing = ProjectListing::new(vec![attached]).expect("projects");
    assert!(
        validate_response_family(
            ExpectedResponse::AttachedProject,
            ResponsePayload::ProjectListing(listing),
        )
        .is_err()
    );
    let threads = ThreadListing::new(vec![thread("thread-a", "project-a")]).expect("threads");
    assert!(
        validate_response_family(
            ExpectedResponse::CreatedThread,
            ResponsePayload::ThreadListing(threads),
        )
        .is_err()
    );
}

#[test]
fn first_message_response_family_requires_exact_request_and_thread() {
    let thread_id = ThreadId::parse("thread-a").expect("thread");
    let other_thread_id = ThreadId::parse("thread-b").expect("thread");
    let request_id = RequestId::parse("native-message-a").expect("request");
    let other_request_id = RequestId::parse("native-message-b").expect("request");
    let receipt = FirstMessageReceipt {
        request_id: request_id.clone(),
        message_id: artisan_domain::MessageId::parse("message-a").expect("message"),
        thread_id: thread_id.clone(),
        disposition: ReceiptDisposition::Accepted,
    };
    let expected = ExpectedResponse::FirstMessageQueued {
        thread_id: thread_id.clone(),
        request_id: request_id.clone(),
    };
    assert!(
        validate_response_family(
            expected.clone(),
            ResponsePayload::FirstMessageQueued(receipt.clone())
        )
        .is_ok()
    );
    assert!(
        validate_response_family(
            expected.clone(),
            ResponsePayload::FirstMessageQueued(FirstMessageReceipt {
                disposition: ReceiptDisposition::Duplicate,
                ..receipt.clone()
            })
        )
        .is_ok()
    );
    assert!(
        validate_response_family(
            expected.clone(),
            ResponsePayload::FirstMessageQueued(FirstMessageReceipt {
                request_id: other_request_id,
                ..receipt.clone()
            })
        )
        .is_err()
    );
    assert!(
        validate_response_family(
            expected.clone(),
            ResponsePayload::FirstMessageQueued(FirstMessageReceipt {
                thread_id: other_thread_id,
                ..receipt.clone()
            })
        )
        .is_err()
    );
    assert!(
        validate_response_family(
            expected,
            ResponsePayload::ProjectListing(
                ProjectListing::new(vec![project("project-a", "A")]).expect("projects"),
            )
        )
        .is_err()
    );
}

#[test]
fn general_message_response_family_requires_exact_request_and_thread() {
    let thread_id = ThreadId::parse("thread-a").expect("thread");
    let other_thread_id = ThreadId::parse("thread-b").expect("thread");
    let request_id = RequestId::parse("native-message-a").expect("request");
    let other_request_id = RequestId::parse("native-message-b").expect("request");
    let receipt = QueueMessageReceipt {
        request_id: request_id.clone(),
        message_id: artisan_domain::MessageId::parse("message-a").expect("message"),
        thread_id: thread_id.clone(),
        disposition: ReceiptDisposition::Accepted,
    };
    let expected = ExpectedResponse::MessageQueued {
        thread_id: thread_id.clone(),
        request_id: request_id.clone(),
    };
    assert!(
        validate_response_family(
            expected.clone(),
            ResponsePayload::MessageQueued(receipt.clone())
        )
        .is_ok()
    );
    assert!(
        validate_response_family(
            expected.clone(),
            ResponsePayload::MessageQueued(QueueMessageReceipt {
                disposition: ReceiptDisposition::Duplicate,
                ..receipt.clone()
            })
        )
        .is_ok()
    );
    assert!(
        validate_response_family(
            expected.clone(),
            ResponsePayload::MessageQueued(QueueMessageReceipt {
                request_id: other_request_id,
                ..receipt.clone()
            })
        )
        .is_err()
    );
    assert!(
        validate_response_family(
            expected.clone(),
            ResponsePayload::MessageQueued(QueueMessageReceipt {
                thread_id: other_thread_id,
                ..receipt.clone()
            })
        )
        .is_err()
    );
    assert!(
        validate_response_family(
            expected,
            ResponsePayload::ProjectListing(
                ProjectListing::new(vec![project("project-a", "A")]).expect("projects"),
            )
        )
        .is_err()
    );
}

#[test]
fn history_image_response_requires_exact_reference_and_content() {
    use sha2::{Digest, Sha256};
    let bytes = vec![1, 2, 3];
    let reference = artisan_domain::ImageAttachmentRef::new(
        artisan_domain::MessageId::parse("image-message").expect("message"),
        ThreadId::parse("image-thread").expect("thread"),
        0,
        "image/png",
        "image.png",
        3,
        Sha256::digest(&bytes).into(),
    )
    .expect("reference");
    let response = artisan_protocol::MessageImageResult {
        reference: reference.clone(),
        bytes,
    };
    let expected = ExpectedResponse::MessageImage(reference);
    assert!(
        validate_response_family(
            expected.clone(),
            ResponsePayload::MessageImage(response.clone())
        )
        .is_ok()
    );
    let mut wrong_content = response.clone();
    wrong_content.bytes[0] = 9;
    assert!(
        validate_response_family(
            expected.clone(),
            ResponsePayload::MessageImage(wrong_content)
        )
        .is_err()
    );
    let mut wrong_owner = response.clone();
    wrong_owner.reference.thread_id = ThreadId::parse("other-thread").expect("thread");
    assert!(
        validate_response_family(expected.clone(), ResponsePayload::MessageImage(wrong_owner))
            .is_err()
    );
    let mut wrong_size = response;
    wrong_size.bytes.push(4);
    assert!(validate_response_family(expected, ResponsePayload::MessageImage(wrong_size)).is_err());
}

#[test]
fn image_message_retry_preserves_full_payload_and_wire_identity() {
    let request_id = RequestId::parse("native-image-stable").expect("request");
    let thread_id = ThreadId::parse("thread-image").expect("thread");
    let payload = artisan_domain::QueueMessagePayload::new(
        None,
        vec![
            artisan_domain::ImageAttachment::new("image/png", vec![1, 2, 3], "one.png")
                .expect("first image"),
            artisan_domain::ImageAttachment::new("image/webp", vec![4, 5], "two.webp")
                .expect("second image"),
        ],
    )
    .expect("image-only payload");
    let mutation = message_stable_mutation(artisan_domain::QueueMessage::new(
        request_id.clone(),
        thread_id.clone(),
        payload.clone(),
    ))
    .expect("stable image mutation");
    let (first, first_id) = mutation.envelope(ProtocolVersion::V1).expect("first");
    let (retry, retry_id) = mutation.envelope(ProtocolVersion::V1).expect("retry");
    assert_eq!(first_id, request_id);
    assert_eq!(retry_id, request_id);
    assert_eq!(
        encode_envelope(&first).expect("first bytes"),
        encode_envelope(&retry).expect("retry bytes")
    );
    let WireEnvelopeBody::Request(ClientRequest::Command(Command::QueueMessage(command))) =
        retry.body
    else {
        panic!("retry must remain a general message command");
    };
    assert_eq!(command.thread_id, thread_id);
    assert_eq!(command.payload, payload);
}

#[test]
fn first_message_stable_retry_keeps_every_wire_byte_and_identity() {
    let request_id = RequestId::parse("native-message-stable").expect("request");
    let thread_id = ThreadId::parse("thread-a").expect("thread");
    let body = MessageBody::parse("  hello\n世界  ").expect("body");
    let mutation = first_message_stable_mutation(QueueFirstMessage {
        request_id: request_id.clone(),
        thread_id: thread_id.clone(),
        body: body.clone(),
    })
    .expect("stable mutation");
    let (first, first_id) = mutation
        .envelope(ProtocolVersion::V1)
        .expect("first envelope");
    let (retry, retry_id) = mutation
        .envelope(ProtocolVersion::V1)
        .expect("retry envelope");
    assert_eq!(first_id, request_id);
    assert_eq!(retry_id, request_id);
    assert_eq!(first.frame_id, retry.frame_id);
    assert_eq!(first.sent_at, retry.sent_at);
    assert_eq!(
        encode_envelope(&first).expect("first bytes"),
        encode_envelope(&retry).expect("retry bytes")
    );
    assert!(matches!(
        first.body,
        WireEnvelopeBody::Request(ClientRequest::Command(Command::QueueFirstMessage(command)))
            if command.request_id == request_id
                && command.thread_id == thread_id
                && command.body == body
    ));
}

fn answer_thread() -> ThreadId {
    ThreadId::parse("thread-answer").expect("answer thread")
}

fn answer_run() -> artisan_domain::RunId {
    artisan_domain::RunId::parse("run-answer").expect("answer run")
}

fn approval_answer(request_id: RequestId) -> artisan_domain::RespondApproval {
    artisan_domain::RespondApproval::new(
        request_id,
        answer_thread(),
        answer_run(),
        artisan_domain::ObservationId::parse("approval-1").expect("approval target"),
        true,
    )
}

fn question_answer(request_id: RequestId) -> artisan_domain::RespondQuestion {
    artisan_domain::RespondQuestion::new(
        request_id,
        answer_thread(),
        answer_run(),
        artisan_domain::ObservationId::parse("question-1").expect("question target"),
        vec![String::from("tokio")],
    )
    .expect("answer bounds")
}

fn approval_receipt(request_id: RequestId) -> artisan_protocol::RespondApprovalReceipt {
    artisan_protocol::RespondApprovalReceipt {
        request_id,
        thread_id: answer_thread(),
        run_id: answer_run(),
        approval_id: artisan_domain::ObservationId::parse("approval-1").expect("approval target"),
        approved: true,
        outcome: artisan_protocol::RunInteractionOutcome::Applied,
        disposition: artisan_domain::ReceiptDisposition::Accepted,
    }
}

fn question_receipt(request_id: RequestId) -> artisan_protocol::RespondQuestionReceipt {
    artisan_protocol::RespondQuestionReceipt {
        request_id,
        thread_id: answer_thread(),
        run_id: answer_run(),
        question_id: artisan_domain::ObservationId::parse("question-1").expect("question target"),
        answers: vec![String::from("tokio")],
        outcome: artisan_protocol::RunInteractionOutcome::Applied,
        disposition: artisan_domain::ReceiptDisposition::Accepted,
    }
}

#[test]
fn approval_answer_mutation_maps_to_command_with_minted_identity() {
    let request_id = RequestId::parse("native-approve-stable").expect("request");
    let mutation = approval_stable_mutation(approval_answer(request_id.clone())).expect("mutation");
    let (envelope, envelope_id) = mutation.envelope(ProtocolVersion::V1).expect("envelope");
    assert_eq!(envelope_id, request_id);
    assert!(matches!(
        envelope.body,
        WireEnvelopeBody::Request(ClientRequest::Command(Command::RespondApproval(answer)))
            if answer.request_id() == &request_id
                && answer.thread_id() == &answer_thread()
                && answer.run_id() == &answer_run()
                && answer.approval_id().as_str() == "approval-1"
                && answer.approved
    ));
}

#[test]
fn question_answer_mutation_maps_to_command_with_minted_identity() {
    let request_id = RequestId::parse("native-question-stable").expect("request");
    let mutation = question_stable_mutation(question_answer(request_id.clone())).expect("mutation");
    let (envelope, envelope_id) = mutation.envelope(ProtocolVersion::V1).expect("envelope");
    assert_eq!(envelope_id, request_id);
    assert!(matches!(
        envelope.body,
        WireEnvelopeBody::Request(ClientRequest::Command(Command::RespondQuestion(answer)))
            if answer.request_id() == &request_id
                && answer.thread_id() == &answer_thread()
                && answer.run_id() == &answer_run()
                && answer.question_id().as_str() == "question-1"
                && answer.answers() == &vec![String::from("tokio")]
    ));
}

#[test]
fn answer_response_family_checks_thread_and_request_correlation() {
    let request_id = RequestId::parse("native-approve-stable").expect("request");
    let expected = ExpectedResponse::ApprovalAnswered {
        thread_id: answer_thread(),
        request_id: request_id.clone(),
    };
    let matched = validate_response_family(
        expected,
        ResponsePayload::ApprovalResponse(approval_receipt(request_id.clone())),
    )
    .expect("correlated approval response");
    assert!(matches!(matched, ResponsePayload::ApprovalResponse(_)));

    let mismatched = ExpectedResponse::ApprovalAnswered {
        thread_id: answer_thread(),
        request_id: RequestId::parse("native-approve-other").expect("other request"),
    };
    assert!(
        validate_response_family(
            mismatched,
            ResponsePayload::ApprovalResponse(approval_receipt(request_id.clone())),
        )
        .is_err(),
        "a receipt echoing another request must not validate"
    );

    let expected = ExpectedResponse::QuestionAnswered {
        thread_id: answer_thread(),
        request_id: request_id.clone(),
    };
    let matched = validate_response_family(
        expected,
        ResponsePayload::QuestionResponse(question_receipt(request_id)),
    )
    .expect("correlated question response");
    assert!(matches!(matched, ResponsePayload::QuestionResponse(_)));
}

#[test]
fn unknown_first_message_thread_is_rejected_before_forge_admission() {
    let known = ThreadId::parse("known-thread").expect("thread");
    let unknown = ThreadId::parse("unknown-thread").expect("thread");
    let mut known_threads = std::collections::HashSet::new();
    known_threads.insert(known.clone());
    assert!(known_thread_for_queue(&known_threads, &known).is_ok());
    assert_eq!(
        known_thread_for_queue(&known_threads, &unknown),
        Err(ServiceFailure::invalid(super::ServiceFailureStage::Request))
    );
}

#[test]
fn command_debug_for_body_bearing_queue_is_variant_only() {
    let command = NativeTransportCommand::QueueFirstMessage(Box::new(QueueFirstMessage {
        request_id: RequestId::parse("native-message-redacted").expect("request"),
        thread_id: ThreadId::parse("thread-a").expect("thread"),
        body: MessageBody::parse("secret message text").expect("body"),
    }));
    let diagnostic = format!("{command:?}");
    assert_eq!(diagnostic, "NativeTransportCommand::QueueFirstMessage");
    assert!(!diagnostic.contains("secret message text"));
}

#[test]
fn authoritative_refreshes_require_full_summary_equality() {
    let attached = project("project-a", "A");
    let same_identity_different_summary = project("project-a", "Renamed");
    let projects = ProjectListing::new(vec![attached.clone()]).expect("projects");
    assert!(contains_exact_project(&projects, &attached));
    assert!(!contains_exact_project(
        &projects,
        &same_identity_different_summary
    ));

    let created = thread("thread-a", "project-a");
    let same_identity_different_thread = thread("thread-a", "project-a");
    let threads = ThreadListing::new(vec![created.clone()]).expect("threads");
    assert!(contains_exact_thread(&threads, &created));
    let live_threads = ThreadListing::new(vec![ThreadSummary {
        has_active_work: true,
        last_message_at: Some(UnixMillis::from_millis(500)),
        ..created.clone()
    }])
    .expect("live listing");
    assert!(contains_exact_thread(&live_threads, &created));
    assert!(!contains_exact_thread(
        &threads,
        &ThreadSummary {
            title: ThreadTitle::parse("Different title").expect("title"),
            ..same_identity_different_thread
        }
    ));
}

#[test]
fn cancelled_picker_outcome_has_no_durable_command_input() {
    let payload = ResponsePayload::DirectoryPicked(DirectoryPickOutcome::Cancelled);
    assert!(matches!(
        &payload,
        ResponsePayload::DirectoryPicked(DirectoryPickOutcome::Cancelled)
    ));
    assert!(!matches!(
        &payload,
        ResponsePayload::AttachedProject { .. }
            | ResponsePayload::CreatedThread { .. }
            | ResponsePayload::FirstMessageQueued(_)
    ));
}

#[test]
fn request_families_are_exact() {
    let project_id = ProjectId::parse("project-a").expect("project");
    let thread_id = ThreadId::parse("thread-a").expect("thread");
    assert!(matches!(
        project_request(),
        ClientRequest::Query(Query::ListAttachedProjects(_))
    ));
    assert!(matches!(
        threads_request(project_id),
        ClientRequest::Query(Query::ListProjectThreads(ListProjectThreads { .. }))
    ));
    assert!(matches!(
        snapshot_request(thread_id).expect("snapshot"),
        ClientRequest::Conversation(artisan_domain::ConversationRequest::Query(query))
            if matches!(query.bounds, ConversationQueryBounds::Window { .. })
    ));
    let _ = WireEnvelopeBody::Request(project_request());
}

#[test]
fn response_families_are_correlated_and_identity_scoped() {
    let project_id = ProjectId::parse("project-a").expect("project");
    let other_project_id = ProjectId::parse("project-b").expect("project");
    let thread_id = ThreadId::parse("thread-a").expect("thread");
    let other_thread_id = ThreadId::parse("thread-b").expect("thread");
    assert!(matches!(
        validate_response_family(
            ExpectedResponse::Directory,
            ResponsePayload::DirectoryPicked(DirectoryPickOutcome::Selected(
                DirectoryId::parse("directory-a").expect("directory")
            )),
        ),
        Ok(ResponsePayload::DirectoryPicked(_))
    ));
    let projects = ProjectListing::new(vec![project("project-a", "A")]).expect("projects");
    assert!(
        validate_response_family(
            ExpectedResponse::Projects,
            ResponsePayload::ProjectListing(projects),
        )
        .is_ok()
    );
    let threads = ThreadListing::new(vec![thread("thread-a", "project-a")]).expect("threads");
    assert!(
        validate_response_family(
            ExpectedResponse::Threads(project_id.clone()),
            ResponsePayload::ThreadListing(threads.clone()),
        )
        .is_ok()
    );
    assert!(
        validate_response_family(
            ExpectedResponse::Threads(other_project_id),
            ResponsePayload::ThreadListing(threads),
        )
        .is_err()
    );
    let snapshot = ConversationSnapshot::new(
        thread_id.clone(),
        ConversationCursor::new(0),
        Vec::new(),
        Vec::new(),
        UnixMillis::EPOCH,
    )
    .expect("snapshot");
    assert!(
        validate_response_family(
            ExpectedResponse::Snapshot(thread_id),
            ResponsePayload::ConversationSnapshot(snapshot.clone()),
        )
        .is_ok()
    );
    assert!(
        validate_response_family(
            ExpectedResponse::Snapshot(other_thread_id),
            ResponsePayload::ConversationSnapshot(snapshot),
        )
        .is_err()
    );
    assert!(
        validate_response_family(
            ExpectedResponse::Projects,
            ResponsePayload::ThreadListing(threads_from_project("project-a")),
        )
        .is_err()
    );
}

#[test]
fn response_correlation_requires_the_exact_outer_request_id() {
    let expected = RequestId::parse("request-a").expect("request");
    let other = RequestId::parse("request-b").expect("request");
    assert!(super::request_id_matches(&expected, &expected));
    assert!(!super::request_id_matches(&expected, &other));
    assert!(super::optional_request_id_matches(
        &expected,
        Some(&expected)
    ));
    assert!(!super::optional_request_id_matches(&expected, Some(&other)));
    assert!(!super::optional_request_id_matches(&expected, None));
}

#[test]
fn empty_and_first_real_rows_are_selected_in_forge_order() {
    let empty = ProjectListing::new(Vec::new()).expect("empty projects");
    assert!(empty.projects().is_empty());
    let listing = ProjectListing::new(vec![project("p1", "First"), project("p2", "Second")])
        .expect("projects");
    assert_eq!(
        listing
            .projects()
            .first()
            .expect("first")
            .project_id
            .as_str(),
        "p1"
    );
    let empty_threads = ThreadListing::new(Vec::new()).expect("empty threads");
    assert!(empty_threads.threads().is_empty());
    let threads =
        ThreadListing::new(vec![thread("t1", "p1"), thread("t2", "p1")]).expect("threads");
    assert_eq!(
        threads.threads().first().expect("first").thread_id.as_str(),
        "t1"
    );
}

#[test]
fn thread_selection_waits_for_host_snapshot_request() {
    let empty = ThreadListing::new(Vec::new()).expect("empty threads");
    assert_eq!(
        thread_selection_decision(&empty),
        ThreadSelectionDecision::Empty
    );

    let listing = ThreadListing::new(vec![thread("t1", "p1")]).expect("threads");
    assert_eq!(
        thread_selection_decision(&listing),
        ThreadSelectionDecision::AwaitHostSnapshot(ThreadId::parse("t1").expect("thread"))
    );
}

#[test]
fn query_bounds_cover_zero_one_maximum_and_overflow() {
    assert!(QueryTurnCount::new(0).is_err());
    assert_eq!(QueryTurnCount::new(1).expect("one").get(), 1);
    assert_eq!(
        QueryTurnCount::new(u64::from(CONVERSATION_QUERY_MAX_TURNS))
            .expect("maximum")
            .get(),
        CONVERSATION_QUERY_MAX_TURNS
    );
    assert!(QueryTurnCount::new(u64::from(CONVERSATION_QUERY_MAX_TURNS) + 1).is_err());
}

#[test]
fn bounded_command_admission_reports_full_and_closed() {
    let (sender, receiver) = tokio::sync::mpsc::channel(COMMAND_CAPACITY.min(1));
    try_send_command(&sender, NativeTransportCommand::Shutdown).expect("first admission");
    assert_eq!(
        try_send_command(&sender, NativeTransportCommand::Shutdown),
        Err(super::CommandSendError::Busy)
    );
    drop(receiver);
    assert_eq!(
        try_send_command(&sender, NativeTransportCommand::Shutdown),
        Err(super::CommandSendError::Stopped)
    );
}

#[test]
fn local_session_request_errors_are_terminal() {
    let error = RequestAttemptError::Terminal {
        failure: ServiceFailure::local_session(),
        retryable_local_session_loss: true,
    };
    assert!(!error.preserves_session());
    assert_eq!(
        ServiceFailure::local_session().category,
        ServiceFailureCategory::LocalSession
    );
}

#[test]
fn settings_load_generation_is_checked_and_monotonic() {
    let first = super::SettingsLoadGeneration::first();
    assert_eq!(first.get(), 1);
    assert_eq!(first.checked_next().expect("next").get(), 2);
    let exhausted = super::SettingsLoadGeneration(u64::MAX);
    assert!(exhausted.checked_next().is_none());
}

#[test]
fn durable_save_retry_allows_local_loss_or_retryable_peer() {
    let local_loss = RequestFailure {
        failure: ServiceFailure::local_session(),
        peer: None,
        retryable_local_session_loss: true,
    };
    assert_eq!(
        super::durable_save_retry_classification(local_loss),
        super::DurableSaveRetryClassification::LocalSessionLoss
    );
    assert!(local_loss.durable_save_retry_allowed());

    let retryable_peer = RequestFailure {
        failure: ServiceFailure::new(
            super::ServiceFailureStage::Request,
            ServiceFailureCategory::Peer,
        ),
        peer: Some(PeerFailure {
            code: ErrorCode::Internal,
            retryable: true,
        }),
        retryable_local_session_loss: false,
    };
    assert_eq!(
        super::durable_save_retry_classification(retryable_peer),
        super::DurableSaveRetryClassification::RetryablePeer
    );
    assert!(retryable_peer.durable_save_retry_allowed());
}

#[test]
fn durable_save_retry_rejects_invalid_conflicting_or_unsupported_peer() {
    let invalid_input = RequestFailure {
        failure: ServiceFailure::new(
            super::ServiceFailureStage::Request,
            ServiceFailureCategory::Peer,
        ),
        peer: Some(PeerFailure {
            code: ErrorCode::InvalidInput,
            retryable: true,
        }),
        retryable_local_session_loss: false,
    };
    assert!(!invalid_input.durable_save_retry_allowed());

    for code in [
        ErrorCode::IdempotencyConflict,
        ErrorCode::UnsupportedVersion,
        ErrorCode::UnsupportedFeature,
    ] {
        let excluded = RequestFailure {
            failure: ServiceFailure::new(
                super::ServiceFailureStage::Request,
                ServiceFailureCategory::Peer,
            ),
            peer: Some(PeerFailure {
                code,
                retryable: true,
            }),
            retryable_local_session_loss: false,
        };
        assert!(!excluded.durable_save_retry_allowed());
    }

    let conflict = RequestFailure {
        failure: ServiceFailure::new(
            super::ServiceFailureStage::Request,
            ServiceFailureCategory::Peer,
        ),
        peer: Some(PeerFailure {
            code: ErrorCode::EngineConfigConflict,
            retryable: true,
        }),
        retryable_local_session_loss: false,
    };
    assert!(!conflict.durable_save_retry_allowed());
}

#[test]
fn durable_save_retry_rejects_nonretryable_and_terminal_failures() {
    let nonretryable_peer = RequestFailure {
        failure: ServiceFailure::new(
            super::ServiceFailureStage::Request,
            ServiceFailureCategory::Peer,
        ),
        peer: Some(PeerFailure {
            code: ErrorCode::Internal,
            retryable: false,
        }),
        retryable_local_session_loss: false,
    };
    assert_eq!(
        super::durable_save_retry_classification(nonretryable_peer),
        super::DurableSaveRetryClassification::NonRetryablePeer
    );
    assert!(!nonretryable_peer.durable_save_retry_allowed());

    let integrity = RequestFailure::terminal(ServiceFailure::new(
        super::ServiceFailureStage::Request,
        ServiceFailureCategory::Integrity,
    ));
    assert_eq!(
        super::durable_save_retry_classification(integrity),
        super::DurableSaveRetryClassification::Integrity
    );
    assert!(!integrity.durable_save_retry_allowed());

    let authentication = RequestFailure::terminal(ServiceFailure::new(
        super::ServiceFailureStage::Handshake,
        ServiceFailureCategory::Authentication,
    ));
    assert_eq!(
        super::durable_save_retry_classification(authentication),
        super::DurableSaveRetryClassification::Authentication
    );
    assert!(!authentication.durable_save_retry_allowed());
}

#[test]
fn local_session_loss_retry_excludes_integrity_and_cancellation() {
    let timeout = ClientRequestError::Exchange(DeadlineError::Timeout {
        operation: OperationKind::Receive,
        limit: Duration::from_secs(1),
    });
    assert!(super::local_session_request_loss_is_retryable(&timeout));

    let stream_loss = ClientRequestError::Exchange(DeadlineError::Peer {
        operation: OperationKind::Receive,
        error: ExchangeError::Receive(EnvelopeReceiveError::Frame(FrameError::Truncated {
            expected: 4,
            received: 0,
        })),
    });
    assert!(super::local_session_request_loss_is_retryable(&stream_loss));

    let integrity = ClientRequestError::Exchange(DeadlineError::Peer {
        operation: OperationKind::Receive,
        error: ExchangeError::Send(EnvelopeSendError::Frame(FrameError::Empty)),
    });
    assert!(!super::local_session_request_loss_is_retryable(&integrity));

    let cancelled = ClientRequestError::Exchange(DeadlineError::Cancelled {
        operation: OperationKind::Receive,
    });
    assert!(!super::local_session_request_loss_is_retryable(&cancelled));
}

#[test]
fn only_correlated_retryable_peer_classification_can_retain_a_retry_plan() {
    let retryable = RequestFailure {
        failure: ServiceFailure::new(
            super::ServiceFailureStage::Request,
            ServiceFailureCategory::Peer,
        ),
        peer: Some(PeerFailure {
            code: ErrorCode::Internal,
            retryable: true,
        }),
        retryable_local_session_loss: false,
    };
    assert!(retryable.retryable());
    assert_eq!(retryable.code(), Some(ErrorCode::Internal));

    let nonretryable = RequestFailure {
        failure: ServiceFailure::new(
            super::ServiceFailureStage::Request,
            ServiceFailureCategory::Peer,
        ),
        peer: Some(PeerFailure {
            code: ErrorCode::IdempotencyConflict,
            retryable: false,
        }),
        retryable_local_session_loss: false,
    };
    assert!(!nonretryable.retryable());
    assert_eq!(nonretryable.code(), Some(ErrorCode::IdempotencyConflict));

    let terminal = RequestFailure::terminal(ServiceFailure::local_session());
    assert!(!terminal.retryable());
    assert_eq!(terminal.code(), None);
}

#[test]
fn admission_budget_one_rolls_before_the_next_request_and_stable_retry_forces_it() {
    assert!(!session_needs_reconnect(0, 1, false));
    assert!(session_needs_reconnect(1, 1, false));
    assert!(session_needs_reconnect(1, 1, true));
    assert!(session_needs_reconnect(0, 1, true));
}

#[test]
fn reconnect_hello_consumes_a_capability_without_formatting_or_copying_it() {
    let capability = ReconnectCapability::from_bytes([0xA5; RECONNECT_CAPABILITY_BYTES]);
    let mut frames = FrameFactory::new();
    let hello = reconnect_hello(&mut frames, capability).expect("reconnect hello");
    let WireEnvelopeBody::Hello(message) = hello.body else {
        panic!("reconnect hello body");
    };
    assert!(matches!(message.credential, HelloCredential::Reconnect(_)));
    assert!(!message.supports_lifecycle_control);
}

#[test]
fn reconnect_binding_uses_validated_target_pin_and_owned_pid() {
    let target = LoopbackTarget::new("127.0.0.1:40123".parse().expect("socket address"))
        .expect("loopback target");
    let pinned_identity = PinnedIdentity::from_digest([0xB6; 32]);
    let binding = build_reconnect_binding([0xC7; 16], target, pinned_identity, 4_242)
        .expect("reconnect binding");
    assert_eq!(binding.instance_id, [0xC7; 16]);
    assert_eq!(binding.endpoint_port, 40_123);
    assert_eq!(binding.certificate_sha256, [0xB6; 32]);
    assert_eq!(binding.pid, NonZeroU32::new(4_242).expect("pid"));
    assert!(build_reconnect_binding([0xC7; 16], target, pinned_identity, 0).is_err());
}

#[test]
fn directory_unknown_is_a_terminal_attach_classification() {
    let failure = RequestFailure {
        failure: ServiceFailure::new(
            super::ServiceFailureStage::Request,
            ServiceFailureCategory::Peer,
        ),
        peer: Some(PeerFailure {
            code: ErrorCode::DirectoryUnknown,
            retryable: true,
        }),
        retryable_local_session_loss: false,
    };
    assert!(failure.retryable());
    assert_eq!(failure.code(), Some(ErrorCode::DirectoryUnknown));
    // The attach path treats this exact code as terminal; the retained
    // failure classification itself remains redacted.
    assert!(!super::attach_retry_allowed(failure));
}

#[test]
fn custody_trace_is_session_then_quarantine_then_lease_then_release() {
    assert_eq!(
        super::custody_trace(),
        vec![
            super::CustodyStep::SessionShutdown,
            super::CustodyStep::ReconnectQuarantine,
            super::CustodyStep::LeaseShutdown,
            super::CustodyStep::ReconnectRelease,
            super::CustodyStep::Stopped,
        ]
    );
}

fn threads_from_project(project_id: &str) -> ThreadListing {
    ThreadListing::new(vec![thread("thread-a", project_id)]).expect("threads")
}

#[test]
fn listener_duration_rejects_unbounded_zero() {
    assert!(finite_duration(0).is_err());
    assert_eq!(finite_duration(1_250).expect("finite").as_millis(), 1_250);
}

#[test]
fn thread_engine_settings_responses_are_thread_scoped() {
    let thread_a = ThreadId::parse("thread-a").expect("thread");
    let thread_b = ThreadId::parse("thread-b").expect("thread");
    let unconfigured = artisan_protocol::ThreadEngineSettingsResult::Unconfigured {
        thread_id: thread_a.clone(),
    };
    assert!(
        validate_response_family(
            ExpectedResponse::ThreadEngineSettings(thread_a.clone()),
            ResponsePayload::ThreadEngineSettings(unconfigured.clone())
        )
        .is_ok()
    );
    assert!(
        validate_response_family(
            ExpectedResponse::ThreadEngineSettings(thread_b),
            ResponsePayload::ThreadEngineSettings(unconfigured)
        )
        .is_err()
    );
    let snapshot = ConversationSnapshot::new(
        thread_a.clone(),
        ConversationCursor::new(0),
        Vec::new(),
        Vec::new(),
        UnixMillis::EPOCH,
    )
    .expect("snapshot");
    assert!(
        validate_response_family(
            ExpectedResponse::ThreadEngineSettings(thread_a),
            ResponsePayload::ConversationSnapshot(snapshot)
        )
        .is_err()
    );
}

#[test]
fn composer_catalog_response_family_requires_exact_thread_and_profile() {
    let expected_thread = ThreadId::parse("thread-a").expect("thread");
    let expected_profile = EngineProfileId::parse("profile-a").expect("profile");
    let expected = ExpectedResponse::ComposerCatalog {
        thread_id: expected_thread.clone(),
        profile_id: expected_profile.clone(),
    };
    let matching = catalog_result("thread-a", "profile-a");
    assert!(
        validate_response_family(expected.clone(), ResponsePayload::ComposerCatalog(matching))
            .is_ok()
    );
    let wrong_profile = catalog_result("thread-a", "profile-b");
    assert!(
        validate_response_family(
            expected.clone(),
            ResponsePayload::ComposerCatalog(wrong_profile)
        )
        .is_err()
    );
    let wrong_thread = catalog_result("thread-b", "profile-a");
    assert!(
        validate_response_family(expected, ResponsePayload::ComposerCatalog(wrong_thread)).is_err()
    );
}

#[test]
fn rich_link_response_family_requires_the_exact_requested_url() {
    let metadata =
        artisan_protocol::RichLinkPageMetadata::new("https://example.com/page", "Example", 5)
            .expect("valid metadata");
    assert!(
        validate_response_family(
            ExpectedResponse::RichLink {
                requested_url: "https://example.com/page".to_owned(),
            },
            ResponsePayload::RichLink(metadata.clone()),
        )
        .is_ok()
    );
    assert!(
        validate_response_family(
            ExpectedResponse::RichLink {
                requested_url: "https://example.com/other".to_owned(),
            },
            ResponsePayload::RichLink(metadata),
        )
        .is_err()
    );
}

#[test]
fn rich_link_requests_apply_the_shared_url_policy() {
    let (ClientRequest::ResolveRichLink(request), expected_url) =
        rich_link_request("https://Example.com/dir/../page#section").expect("valid request")
    else {
        panic!("rich-link helper must build the resolve arm");
    };
    assert_eq!(request.url(), "https://example.com/page");
    assert_eq!(expected_url, "https://example.com/page");

    assert!(rich_link_request("example.com/page").is_err());
    assert!(rich_link_request("mailto:user@example.com").is_err());
    assert!(rich_link_request("ftp://example.com/file").is_err());
}

#[test]
fn project_repository_requests_name_exactly_one_project() {
    let project = ProjectId::parse("project-a").expect("valid project");
    let ClientRequest::QueryProjectRepository(query) = project_repository_request(project.clone())
    else {
        panic!("repository helper must build the query arm");
    };
    assert_eq!(query.project_ids(), &[project]);
}

#[test]
fn project_repository_response_family_requires_the_requested_project_entry() {
    let project = ProjectId::parse("project-a").expect("valid project");
    let other = ProjectId::parse("project-b").expect("valid project");
    let result = |entries| ProjectRepositoryQueryResult::new(entries).expect("valid result");

    assert!(
        validate_response_family(
            ExpectedResponse::ProjectRepository {
                project_id: project.clone(),
            },
            ResponsePayload::ProjectRepository(result(vec![ProjectRepositoryEntry::new(
                project.clone(),
                ProjectRepository::NotRepository,
            )])),
        )
        .is_ok()
    );
    assert!(
        validate_response_family(
            ExpectedResponse::ProjectRepository {
                project_id: project,
            },
            ResponsePayload::ProjectRepository(result(vec![ProjectRepositoryEntry::new(
                other,
                ProjectRepository::NotRepository,
            )])),
        )
        .is_err(),
        "a response for another project must not settle this read"
    );
}

#[test]
fn favorite_receipt_response_family_requires_exact_request_model_and_desired_state() {
    let request_id = RequestId::parse("favorite-a").expect("request");
    let model_id = ModelFavoriteId::parse("model-a").expect("model");
    let snapshot = ModelFavoritesSnapshot::new(
        artisan_domain::ModelFavoritesRevision::default(),
        vec![model_id.clone()],
    )
    .expect("snapshot");
    let receipt = SetModelFavoriteReceipt {
        request_id: request_id.clone(),
        model_id: model_id.clone(),
        favorite: true,
        disposition: ReceiptDisposition::Accepted,
        snapshot,
    };
    let expected = ExpectedResponse::ModelFavoriteSet {
        request_id: request_id.clone(),
        model_id: model_id.clone(),
        favorite: true,
    };
    assert!(
        validate_response_family(
            expected.clone(),
            ResponsePayload::ModelFavoriteSet(receipt.clone())
        )
        .is_ok()
    );
    let mut wrong_state = receipt;
    wrong_state.favorite = false;
    assert!(
        validate_response_family(expected, ResponsePayload::ModelFavoriteSet(wrong_state)).is_err()
    );
}

#[test]
fn registered_profiles_response_family_is_exact() {
    let missing = RegisteredEngineProfilesResult::RegistryMissing;
    assert!(
        validate_response_family(
            ExpectedResponse::RegisteredProfiles,
            ResponsePayload::RegisteredEngineProfiles(missing)
        )
        .is_ok()
    );
    let present_empty = RegisteredEngineProfilesResult::RegistryPresent {
        profile_ids: Vec::new(),
    };
    assert!(
        validate_response_family(
            ExpectedResponse::RegisteredProfiles,
            ResponsePayload::RegisteredEngineProfiles(present_empty)
        )
        .is_ok()
    );
    let present = RegisteredEngineProfilesResult::RegistryPresent {
        profile_ids: vec![EngineProfileId::parse("default").expect("profile")],
    };
    assert!(
        validate_response_family(
            ExpectedResponse::RegisteredProfiles,
            ResponsePayload::RegisteredEngineProfiles(present)
        )
        .is_ok()
    );
    let listing = ProjectListing::new(vec![project("project-a", "A")]).expect("projects");
    assert!(
        validate_response_family(
            ExpectedResponse::RegisteredProfiles,
            ResponsePayload::ProjectListing(listing)
        )
        .is_err()
    );
}

#[test]
fn thread_engine_config_set_response_requires_exact_thread_and_request() {
    let thread_id = ThreadId::parse("thread-a").expect("thread");
    let request_id = RequestId::parse("request-a").expect("request");
    let other_thread = ThreadId::parse("thread-b").expect("thread");
    let other_request = RequestId::parse("request-b").expect("request");
    let result = SetThreadEngineConfigResult {
        request_id: request_id.clone(),
        thread_id: thread_id.clone(),
        revision: artisan_domain::EngineConfigRevision::new(1).expect("rev"),
        disposition: artisan_domain::ReceiptDisposition::Accepted,
    };
    assert!(
        validate_response_family(
            ExpectedResponse::ThreadEngineConfigSet {
                thread_id: thread_id.clone(),
                request_id: request_id.clone()
            },
            ResponsePayload::ThreadEngineConfigSet(result.clone())
        )
        .is_ok()
    );
    assert!(
        validate_response_family(
            ExpectedResponse::ThreadEngineConfigSet {
                thread_id: other_thread,
                request_id: request_id.clone()
            },
            ResponsePayload::ThreadEngineConfigSet(result.clone())
        )
        .is_err()
    );
    assert!(
        validate_response_family(
            ExpectedResponse::ThreadEngineConfigSet {
                thread_id,
                request_id: other_request
            },
            ResponsePayload::ThreadEngineConfigSet(result)
        )
        .is_err()
    );
}

#[test]
fn engine_config_stable_retry_retains_request_id_and_payload() {
    let thread_id = ThreadId::parse("thread-a").expect("thread");
    let profile = EngineProfileId::parse("default").expect("profile");
    let model = artisan_domain::EngineModelId::parse("model-test").expect("model");
    let route = artisan_domain::EngineRouteId::parse("route-test").expect("route");
    let permission = artisan_domain::PermissionId::parse("perm-a").expect("perm");
    let agent = artisan_domain::EngineAgentId::parse("agent-a").expect("agent");
    let permission_policy = artisan_domain::EnginePermissionPolicy::new(
        permission,
        agent,
        artisan_domain::ApprovalMode::Never,
        artisan_domain::FilesystemAccess::None,
        artisan_domain::NetworkAccess::Disabled,
        artisan_domain::WebSearchAccess::Disabled,
    );
    let selection = artisan_domain::EngineSelection::OpenCode2(
        artisan_domain::OpenCode2Selection::new(profile, model, route, None, permission_policy),
    );
    let runtime =
        artisan_domain::EngineRuntimeControls::new(artisan_domain::EngineRuntimeControlsInput {
            attempt_budget: artisan_domain::FiniteMillis::new(5).expect("budget"),
            readiness_budget: artisan_domain::FiniteMillis::new(1).expect("budget"),
            health_budget: artisan_domain::FiniteMillis::new(1).expect("budget"),
            prompt_budget: artisan_domain::FiniteMillis::new(1).expect("budget"),
            stream_budget: artisan_domain::FiniteMillis::new(1).expect("budget"),
            close_budget: artisan_domain::FiniteMillis::new(1).expect("budget"),
            max_json_body_bytes: artisan_domain::ByteLimit::new(1).expect("limit"),
            max_sse_line_bytes: artisan_domain::ByteLimit::new(1).expect("limit"),
            max_sse_event_bytes: artisan_domain::ByteLimit::new(1).expect("limit"),
            max_readiness_line_bytes: artisan_domain::ByteLimit::new(1).expect("limit"),
            max_header_count: artisan_domain::CountLimit::new(1).expect("limit"),
            max_http_buffer_bytes: artisan_domain::ByteLimit::new(1).expect("limit"),
            max_stderr_bytes: artisan_domain::ByteLimit::new(1).expect("limit"),
            observation_capacity: artisan_domain::CountLimit::new(1).expect("limit"),
        })
        .expect("runtime");
    let config = artisan_domain::EngineRunConfig::new(selection, runtime);
    let request_id = RequestId::parse("engine-save-1").expect("request");
    let command = Box::new(SetThreadEngineConfig::new(
        request_id.clone(),
        thread_id.clone(),
        artisan_domain::EngineConfigUpdatePrecondition::Unconfigured,
        config.clone(),
    ));
    let mutation = engine_config_stable_mutation(command).expect("mutation");
    let (first_envelope, first_request_id) =
        mutation.envelope(ProtocolVersion::V1).expect("envelope");
    let (second_envelope, second_request_id) = mutation
        .envelope(ProtocolVersion::V1)
        .expect("retry envelope");
    assert_eq!(
        first_envelope.protocol_version,
        second_envelope.protocol_version
    );
    assert!(first_envelope.body == second_envelope.body);
    assert_eq!(first_request_id, second_request_id);
    assert_eq!(first_envelope.frame_id, second_envelope.frame_id);
    assert_eq!(first_envelope.sent_at, second_envelope.sent_at);
    assert_eq!(
        first_envelope.frame_id.to_request_id().expect("id"),
        first_request_id
    );
    assert_eq!(first_request_id, request_id);
    // Fresh read uses fresh frame.
    let mut frames = FrameFactory::new();
    let (fresh_first, _) = make_request_frame(
        &mut frames,
        ProtocolVersion::V1,
        thread_engine_settings_request(thread_id.clone()),
    )
    .expect("fresh");
    let (fresh_second, _) = make_request_frame(
        &mut frames,
        ProtocolVersion::V1,
        thread_engine_settings_request(thread_id),
    )
    .expect("fresh second");
    assert_ne!(fresh_first.frame_id, fresh_second.frame_id);
}

#[test]
fn engine_config_conflict_is_identifiable_and_retryable_flag_is_preserved() {
    let conflict = RequestFailure {
        failure: ServiceFailure::new(
            super::ServiceFailureStage::Request,
            ServiceFailureCategory::Peer,
        ),
        peer: Some(PeerFailure {
            code: ErrorCode::EngineConfigConflict,
            retryable: false,
        }),
        retryable_local_session_loss: false,
    };
    assert_eq!(conflict.code(), Some(ErrorCode::EngineConfigConflict));
    assert!(!conflict.retryable());
    let retryable = RequestFailure {
        failure: ServiceFailure::new(
            super::ServiceFailureStage::Request,
            ServiceFailureCategory::Peer,
        ),
        peer: Some(PeerFailure {
            code: ErrorCode::Internal,
            retryable: true,
        }),
        retryable_local_session_loss: false,
    };
    assert!(retryable.retryable());
}

#[test]
fn redacted_service_failures_do_not_reveal_engine_values() {
    let failure = ServiceFailure::new(
        super::ServiceFailureStage::Request,
        ServiceFailureCategory::Peer,
    );
    let text = failure.to_string();
    assert!(!text.contains("model-test"));
    assert!(!text.contains("route-test"));
    assert!(!text.contains("default"));
    assert!(!text.contains("engine-save"));
}

#[test]
fn forwarded_continuity_covers_second_batch_before_delayed_ack() {
    use super::SubscriptionCustody;

    let thread = ThreadId::parse("custody-thread").expect("thread");
    let mut custody = SubscriptionCustody::new();
    assert_eq!(custody.expected_batch_from(), None);
    custody.on_subscribe(thread.clone(), Some(ConversationCursor::new(5)));
    assert_eq!(
        custody.expected_batch_from(),
        Some(ConversationCursor::new(5))
    );
    // The first batch validates against the subscribe baseline;
    // forwarding records transport continuity without marking the
    // application acknowledgement the resume baseline still waits for.
    custody.on_batch_forwarded(ConversationCursor::new(6));
    assert_eq!(
        custody.expected_batch_from(),
        Some(ConversationCursor::new(6))
    );
    assert_eq!(custody.last_accepted_cursor(), None);
    // The delayed UI ack advances acceptance only; continuity still
    // rides the forwarded cursor for the batch already in flight.
    custody
        .on_acknowledge(&thread, ConversationCursor::new(6))
        .expect("ack");
    assert_eq!(
        custody.last_accepted_cursor(),
        Some(ConversationCursor::new(6))
    );
    assert_eq!(
        custody.expected_batch_from(),
        Some(ConversationCursor::new(6))
    );
    custody.on_batch_forwarded(ConversationCursor::new(7));
    assert_eq!(
        custody.expected_batch_from(),
        Some(ConversationCursor::new(7))
    );
    // A fresh subscribe epoch resets forwarded continuity; redelivery
    // validates against the new baseline while the application dedups
    // replay above this layer.
    custody.on_subscribe(thread.clone(), Some(ConversationCursor::new(6)));
    assert_eq!(custody.received_cursor(), None);
    assert_eq!(
        custody.expected_batch_from(),
        Some(ConversationCursor::new(6))
    );
    custody.on_unsubscribe(&thread);
    assert_eq!(custody.expected_batch_from(), None);
    assert_eq!(custody.active_thread(), None);
}

#[test]
fn two_contiguous_batches_publish_before_delayed_ack_without_reconnect() {
    use super::{NativeTransportEvent, PrivateDelivery, command_loop_with_delivery};
    use artisan_domain::{
        ConversationLifecycle, ConversationPatch, PatchBatch, PatchId, PatchSequence, Revision,
        TurnId, TurnOrdinal,
    };

    fn batch(thread: &ThreadId, from: u64, to: u64) -> PatchBatch {
        let turn = artisan_domain::ConversationTurn {
            turn_id: TurnId::parse("turn-a").expect("turn"),
            ordinal: TurnOrdinal::new(0),
            revision: Revision::new(0),
            lifecycle: ConversationLifecycle::Pending,
            created_at: UnixMillis::EPOCH,
            updated_at: UnixMillis::EPOCH,
        };
        let patch = ConversationPatch::TurnUpsert {
            patch_id: PatchId::parse("patch-a").expect("patch"),
            sequence: PatchSequence::new(to).expect("sequence"),
            turn,
        };
        PatchBatch::new(
            thread.clone(),
            ConversationCursor::new(from),
            ConversationCursor::new(to),
            vec![patch],
        )
        .expect("batch")
    }

    async fn next_patch(events: &std::sync::mpsc::Receiver<NativeTransportEvent>) -> PatchBatch {
        for _ in 0..10_000 {
            match events.try_recv() {
                Ok(NativeTransportEvent::PatchBatch(batch)) => return batch,
                Ok(_) => tokio::task::yield_now().await,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    tokio::task::yield_now().await;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    panic!("event bridge closed before both batches published");
                }
            }
        }
        panic!("second contiguous batch was not published before any UI ack");
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("loop test runtime");
    runtime.block_on(async {
        let mut service = super::ServiceRuntime::new_for_batch_tests();
        let thread = ThreadId::parse("loop-thread").expect("thread");
        service
            .custody
            .on_subscribe(thread.clone(), Some(ConversationCursor::new(5)));
        let (command_tx, mut command_rx) = tokio::sync::mpsc::channel::<NativeTransportCommand>(8);
        let (delivery_tx, mut delivery_rx) = tokio::sync::mpsc::channel::<PrivateDelivery>(8);
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel::<NativeTransportEvent>(16);
        let mut frames = FrameFactory::new();
        let join = tokio::spawn(async move {
            let outcome = command_loop_with_delivery(
                &mut command_rx,
                &mut delivery_rx,
                &mut service,
                &mut frames,
                &event_tx,
            )
            .await;
            (outcome, service.custody)
        });
        delivery_tx
            .send(PrivateDelivery::Batch(batch(&thread, 5, 6)))
            .await
            .expect("first batch");
        let first = next_patch(&event_rx).await;
        assert_eq!(first.from_cursor(), ConversationCursor::new(5));
        assert_eq!(first.to_cursor(), ConversationCursor::new(6));
        delivery_tx
            .send(PrivateDelivery::Batch(batch(&thread, 6, 7)))
            .await
            .expect("second batch");
        let second = next_patch(&event_rx).await;
        assert_eq!(second.from_cursor(), ConversationCursor::new(6));
        assert_eq!(second.to_cursor(), ConversationCursor::new(7));
        command_tx
            .send(NativeTransportCommand::Shutdown)
            .await
            .expect("shutdown");
        let (outcome, custody) = join.await.expect("loop join");
        assert!(
            outcome.is_ok(),
            "two contiguous batches must publish without reconnecting"
        );
        assert_eq!(custody.received_cursor(), Some(ConversationCursor::new(7)));
        assert_eq!(
            custody.last_accepted_cursor(),
            None,
            "forwarding must not mark application acceptance"
        );
    });
}
