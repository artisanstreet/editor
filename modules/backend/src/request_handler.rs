//! Application request-handler seam between the wire protocol and Forge
//! repositories.
//!
//! Maps one decoded, frame-correlated [`ClientRequest`] onto the repository
//! capabilities this Forge build owns and answers with the protocol's typed
//! success ([`ServerResponse`]) or failure ([`ProtocolFailure`]) contract.
//! The seam owns no listener, connection, QUIC endpoint, or accept loop:
//! transport hands a request plus its correlated domain request id to
//! [`RequestHandler::respond`] and delivers the returned outcome back over
//! the wire. Requests whose backing capability does not exist in this build
//! answer with established typed failures instead of fabricated success, so
//! every observable behavior remains backed by real repository state.
//!
//! Fresh `CreateThread` and `QueueFirstMessage` commands are admitted for
//! real: after a receipt-lookup miss, the handler acquires one Forge-minted
//! durable identity plus one acceptance instant at the narrow
//! [`crate::command_admission::CommandOrigin`] boundary and hands both to
//! the authoritative repository transaction. Responses are built exclusively
//! from repository-returned identities, summaries, and receipt dispositions,
//! including raced duplicate outcomes; replay lookups always precede origin
//! access, so exact replays, queries, persisted lookup conflicts, and
//! unsupported capabilities never consult it. Conflicts discovered by the
//! later transaction can follow fresh origin acquisition.

use artisan_database::{CreateThreadInput, QueueFirstMessageInput, Repository, RepositoryError};
use artisan_domain::{
    Command, CreateThread, DirectoryId, MessageId, Query, QueueFirstMessage, RequestId, ThreadId,
    UnixMillis,
};
use artisan_protocol::{
    ClientRequest, ErrorCode, ErrorDetail, FirstMessageReceipt, ProtocolFailure, ResponsePayload,
    ServerResponse,
};

use crate::command_admission::{
    CommandOrigin, CommandOriginClockError, CommandOriginEntropyError, SystemCommandOrigin,
};

/// Detail used when a diagnostic text would exceed the protocol-owned
/// error-detail ceiling. Short by construction, so parsing it cannot fail.
const BOUNDED_DETAIL_FALLBACK: &str = "failure detail exceeded the protocol error-detail bound";

/// Answers decoded client requests from Forge-owned repository state.
#[derive(Debug)]
pub struct RequestHandler {
    repository: Repository,
    origin: AdmissionOrigin,
}

/// Handler-owned admission source: the real system boundary by default, or
/// an explicitly injected implementation for deterministic tests.
///
/// The enum keeps [`RequestHandler::new`] a `const` constructor while still
/// allowing one boxed injection point; neither variant changes admission
/// sequencing or repository authority.
#[derive(Debug)]
enum AdmissionOrigin {
    System,
    Injected(Box<dyn CommandOrigin>),
}

impl AdmissionOrigin {
    const fn system() -> Self {
        Self::System
    }

    fn mint_identity(&self) -> Result<String, CommandOriginEntropyError> {
        match self {
            Self::System => SystemCommandOrigin.mint_identity(),
            Self::Injected(origin) => origin.mint_identity(),
        }
    }

    fn acceptance_instant(&self) -> Result<UnixMillis, CommandOriginClockError> {
        match self {
            Self::System => SystemCommandOrigin.acceptance_instant(),
            Self::Injected(origin) => origin.acceptance_instant(),
        }
    }
}

impl RequestHandler {
    /// Creates a handler answering from the supplied repository facade with
    /// real operating-system entropy and wall-clock time behind fresh
    /// admissions.
    #[must_use]
    pub const fn new(repository: Repository) -> Self {
        Self {
            repository,
            origin: AdmissionOrigin::system(),
        }
    }

    /// Creates a handler whose fresh admissions acquire identity text and
    /// instants from the explicitly supplied origin.
    ///
    /// This swaps only the narrow acquisition boundary. Admission ordering,
    /// receipt replay, conflict handling, and repository authority stay
    /// identical to [`RequestHandler::new`], and no test-only bypass exists:
    /// queries, replays, persisted conflicts, and unsupported capabilities
    /// behave exactly alike under both constructors.
    #[must_use]
    pub fn with_origin(repository: Repository, origin: Box<dyn CommandOrigin>) -> Self {
        Self {
            repository,
            origin: AdmissionOrigin::Injected(origin),
        }
    }

    /// Resolves one correlated application request to its typed outcome.
    ///
    /// `request_id` is the triggering frame's identity converted through
    /// [`artisan_protocol::FrameId::to_request_id`]; queries carry no domain
    /// request id of their own, so this correlated identity names every
    /// response and failure. Idempotent commands carry a request id that
    /// must equal the correlated identity; a mismatch fails as invalid input
    /// instead of guessing which correlation the client meant.
    ///
    /// Every durable effect is answered exactly once: an existing command
    /// receipt replays as a duplicate through the repository before any
    /// fresh acceptance is attempted, and only a lookup miss reaches the
    /// admission origin for identity and instant acquisition.
    ///
    /// # Errors
    ///
    /// Returns a typed [`ProtocolFailure`] when the request names unknown
    /// state, violates correlation rules, or requires a capability this
    /// Forge build does not own. Repository-backed failures preserve the
    /// retryability guidance implied by their persistence classification.
    pub async fn respond(
        &self,
        request_id: &RequestId,
        request: &ClientRequest,
    ) -> Result<ServerResponse, ProtocolFailure> {
        match request {
            ClientRequest::Query(query) => self.query_outcome(request_id, query).await,
            ClientRequest::Command(command) => self.command_outcome(request_id, command).await,
            ClientRequest::Conversation(_) => {
                Err(unbacked_failure(request_id, "conversation projection"))
            }
        }
    }

    /// Answers pure reads from repository listings.
    async fn query_outcome(
        &self,
        request_id: &RequestId,
        query: &Query,
    ) -> Result<ServerResponse, ProtocolFailure> {
        match query {
            Query::ListAttachedProjects(_) => {
                let listing = self
                    .repository
                    .list_projects()
                    .await
                    .map_err(|error| repository_failure(&error, request_id))?;
                Ok(outcome(
                    request_id,
                    ResponsePayload::ProjectListing(listing),
                ))
            }
            Query::ListProjectThreads(listing) => {
                let threads = self
                    .repository
                    .list_threads(&listing.project_id)
                    .await
                    .map_err(|error| repository_failure(&error, request_id))?;
                Ok(outcome(request_id, ResponsePayload::ThreadListing(threads)))
            }
            Query::ListDirectories(browse) => match &browse.parent {
                Some(parent) => Err(unknown_directory_failure(request_id, parent)),
                None => Err(unbacked_failure(request_id, "directory browsing")),
            },
        }
    }

    /// Answers idempotent mutations after replaying any durable receipt.
    async fn command_outcome(
        &self,
        request_id: &RequestId,
        command: &Command,
    ) -> Result<ServerResponse, ProtocolFailure> {
        if command.request_id() != request_id {
            return Err(typed_failure(
                ErrorCode::InvalidInput,
                "command request id must equal its correlated request frame id",
                false,
                request_id,
            ));
        }

        match command {
            Command::AttachProject(attach) => {
                if let Some(replay) = self
                    .repository
                    .lookup_attach_project(&attach.request_id, &attach.directory_id)
                    .await
                    .map_err(|error| repository_failure(&error, request_id))?
                {
                    return Ok(outcome(
                        request_id,
                        ResponsePayload::AttachedProject {
                            project: replay.project,
                            disposition: replay.receipt.disposition,
                        },
                    ));
                }
                // A first-time attach resolves the opaque directory identity
                // against host-visible directories; this build owns no
                // directory registry, so the identity stays unknown.
                Err(unknown_directory_failure(request_id, &attach.directory_id))
            }
            Command::CreateThread(create) => self.create_thread_outcome(request_id, create).await,
            Command::QueueFirstMessage(queue) => {
                self.queue_first_message_outcome(request_id, queue).await
            }
        }
    }

    /// Answers one create-thread mutation from its durable receipt or a
    /// fresh Forge-minted thread.
    ///
    /// Fresh acceptance acquires the thread identity and one shared creation
    /// instant only after replay lookup missed. The create-thread
    /// transaction stays authoritative: it repeats the lookup
    /// transactionally, resolves concurrent races into duplicate outcomes,
    /// and rejects unknown projects or colliding identities without any
    /// retry loop or timestamp repair here.
    async fn create_thread_outcome(
        &self,
        request_id: &RequestId,
        create: &CreateThread,
    ) -> Result<ServerResponse, ProtocolFailure> {
        if let Some(replay) = self
            .repository
            .lookup_create_thread(&create.request_id, &create.project_id, &create.title)
            .await
            .map_err(|error| repository_failure(&error, request_id))?
        {
            return Ok(outcome(
                request_id,
                ResponsePayload::CreatedThread {
                    thread: replay.thread,
                    disposition: replay.receipt.disposition,
                },
            ));
        }
        let identity = self
            .origin
            .mint_identity()
            .map_err(|error| origin_entropy_failure(&error, request_id))?;
        let accepted_at = self
            .origin
            .acceptance_instant()
            .map_err(|error| origin_clock_failure(error, request_id))?;
        let thread_id =
            ThreadId::parse(identity).map_err(|_| forged_identity_failure("thread", request_id))?;
        let result = self
            .repository
            .create_thread(CreateThreadInput {
                request_id: create.request_id.clone(),
                thread_id,
                project_id: create.project_id.clone(),
                title: create.title.clone(),
                created_at: accepted_at,
                updated_at: accepted_at,
            })
            .await
            .map_err(|error| repository_failure(&error, request_id))?;
        Ok(outcome(
            request_id,
            ResponsePayload::CreatedThread {
                thread: result.thread,
                disposition: result.receipt.disposition,
            },
        ))
    }

    /// Answers one first-message mutation from its durable receipt or a
    /// fresh Forge-minted queued message.
    ///
    /// Fresh acceptance mints the message identity and one acceptance
    /// instant after replay lookup missed. The queueing transaction
    /// atomically persists message, receipt, recency, and queued outbox, so
    /// this answers queued acceptance — never engine completion — and a
    /// raced identical request converges on the original durable identity as
    /// Duplicate.
    async fn queue_first_message_outcome(
        &self,
        request_id: &RequestId,
        queue: &QueueFirstMessage,
    ) -> Result<ServerResponse, ProtocolFailure> {
        if let Some(replay) = self
            .repository
            .lookup_queue_first_message(&queue.request_id, &queue.thread_id, &queue.body)
            .await
            .map_err(|error| repository_failure(&error, request_id))?
        {
            return Ok(outcome(
                request_id,
                ResponsePayload::FirstMessageQueued(FirstMessageReceipt {
                    request_id: replay.receipt.request_id,
                    message_id: replay.message.message_id,
                    thread_id: replay.message.thread_id,
                    disposition: replay.receipt.disposition,
                }),
            ));
        }
        let identity = self
            .origin
            .mint_identity()
            .map_err(|error| origin_entropy_failure(&error, request_id))?;
        let accepted_at = self
            .origin
            .acceptance_instant()
            .map_err(|error| origin_clock_failure(error, request_id))?;
        let message_id = MessageId::parse(identity)
            .map_err(|_| forged_identity_failure("message", request_id))?;
        let result = self
            .repository
            .queue_first_message(QueueFirstMessageInput {
                request_id: queue.request_id.clone(),
                message_id,
                thread_id: queue.thread_id.clone(),
                body: queue.body.clone(),
                accepted_at,
            })
            .await
            .map_err(|error| repository_failure(&error, request_id))?;
        Ok(outcome(
            request_id,
            ResponsePayload::FirstMessageQueued(FirstMessageReceipt {
                request_id: result.receipt.request_id,
                message_id: result.message.message_id,
                thread_id: result.message.thread_id,
                disposition: result.receipt.disposition,
            }),
        ))
    }
}

/// Wraps a successful payload in the response correlated to its request.
fn outcome(request_id: &RequestId, payload: ResponsePayload) -> ServerResponse {
    ServerResponse {
        request_id: request_id.clone(),
        payload,
    }
}

/// Classifies a repository failure into the stable protocol vocabulary.
///
/// Unknown client-named state maps to the entity-specific codes; reuse of a
/// persisted request identity for a different command kind or immutable
/// payload answers the dedicated non-retryable idempotency-conflict code so
/// the originally accepted outcome stands; deterministic client mistakes such
/// as a first message that already exists from another request stay
/// non-retryable input failures; persisted-state problems stay internal
/// without retry hope; only database-operation failures admit that an
/// identical later request may succeed.
fn repository_failure(error: &RepositoryError, request_id: &RequestId) -> ProtocolFailure {
    use RepositoryError as Failure;

    let (code, retryable) = match error {
        Failure::ProjectNotFound { .. } => (ErrorCode::ProjectUnknown, false),
        Failure::ThreadNotFound { .. } => (ErrorCode::ThreadUnknown, false),
        Failure::IdempotencyConflict { .. } => (ErrorCode::IdempotencyConflict, false),
        Failure::FirstMessageAlreadyExists { .. } => (ErrorCode::InvalidInput, false),
        Failure::ProjectConflict { .. }
        | Failure::ThreadConflict { .. }
        | Failure::MessageConflict { .. }
        | Failure::InvalidChronology { .. }
        | Failure::CorruptData { .. }
        | Failure::Invariant { .. }
        | Failure::ThreadListing { .. }
        | Failure::ProjectListing { .. }
        | Failure::InvalidDispatchLeaseWindow { .. }
        | Failure::DispatchAttemptLimit { .. }
        | Failure::DispatchNotFound { .. }
        | Failure::InvalidDispatchState { .. }
        | Failure::DispatchOwnerMismatch { .. }
        | Failure::DispatchLeaseExpired { .. } => (ErrorCode::Internal, false),
        Failure::Database { .. } => (ErrorCode::Internal, true),
    };
    typed_failure(code, error.to_string(), retryable, request_id)
}

/// Builds the typed failure for an operation without a backing capability.
///
/// The failure stays internal and non-retryable: repeating the identical
/// request against this build deterministically fails again, while the detail
/// records exactly which capability is absent rather than claiming an effect.
fn unbacked_failure(request_id: &RequestId, operation: &str) -> ProtocolFailure {
    typed_failure(
        ErrorCode::Internal,
        format!("{operation} is not backed by a Forge capability in this build"),
        false,
        request_id,
    )
}

/// Builds the typed failure for an unresolvable opaque directory identity.
fn unknown_directory_failure(request_id: &RequestId, directory: &DirectoryId) -> ProtocolFailure {
    typed_failure(
        ErrorCode::DirectoryUnknown,
        format!("directory `{directory}` is not known to this Forge build"),
        false,
        request_id,
    )
}

/// Builds the typed failure for a fresh-command entropy acquisition failure.
///
/// Entropy unavailability is an environmental fault, not a client mistake:
/// nothing was persisted, no receipt was recorded, and the identical retry
/// may succeed once the platform provider recovers, so the failure stays
/// internal and retryable. The detail carries only the bounded typed cause —
/// never command payloads.
fn origin_entropy_failure(
    error: &CommandOriginEntropyError,
    request_id: &RequestId,
) -> ProtocolFailure {
    typed_failure(
        ErrorCode::Internal,
        format!("fresh command could not acquire durable identity entropy: {error}"),
        true,
        request_id,
    )
}

/// Builds the typed failure for a fresh-command instant acquisition failure.
///
/// A clock reading outside the signed millisecond range is likewise
/// environmental and left nothing behind: conversion refuses to truncate or
/// clamp, so the failure stays internal, payload-free, correlated, and
/// retryable on the same terms as the entropy fault.
fn origin_clock_failure(error: CommandOriginClockError, request_id: &RequestId) -> ProtocolFailure {
    typed_failure(
        ErrorCode::Internal,
        format!("fresh command could not acquire an acceptance instant: {error}"),
        true,
        request_id,
    )
}

/// Builds the typed failure for a forged identity failing identifier
/// validation.
///
/// The bounded hex encoder cannot emit invalid identifier text, so this
/// records a deterministic internal defect instead of fabricating success.
/// It stays non-retryable because repeating through the same defective
/// encoder cannot mint differently, and payload-free because only the
/// Forge-owned kind is named.
fn forged_identity_failure(kind: &'static str, request_id: &RequestId) -> ProtocolFailure {
    typed_failure(
        ErrorCode::Internal,
        format!("forged {kind} identity failed identifier validation"),
        false,
        request_id,
    )
}

/// Bounds a diagnostic text into the protocol's failure contract.
fn typed_failure(
    code: ErrorCode,
    detail_text: impl Into<String>,
    retryable: bool,
    request_id: &RequestId,
) -> ProtocolFailure {
    let detail = ErrorDetail::parse(detail_text).ok().unwrap_or_else(|| {
        ErrorDetail::parse(BOUNDED_DETAIL_FALLBACK)
            .expect("static fallback detail satisfies the protocol bound")
    });
    ProtocolFailure {
        code,
        detail,
        retryable,
        request_id: Some(request_id.clone()),
    }
}
