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

use artisan_database::{Repository, RepositoryError};
use artisan_domain::{Command, DirectoryId, Query, RequestId};
use artisan_protocol::{
    ClientRequest, ErrorCode, ErrorDetail, FirstMessageReceipt, ProtocolFailure, ResponsePayload,
    ServerResponse,
};

/// Detail used when a diagnostic text would exceed the protocol-owned
/// error-detail ceiling. Short by construction, so parsing it cannot fail.
const BOUNDED_DETAIL_FALLBACK: &str = "failure detail exceeded the protocol error-detail bound";

/// Answers decoded client requests from Forge-owned repository state.
#[derive(Debug)]
pub struct RequestHandler {
    repository: Repository,
}

impl RequestHandler {
    /// Creates a handler answering from the supplied repository facade.
    #[must_use]
    pub const fn new(repository: Repository) -> Self {
        Self { repository }
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
    /// fresh acceptance is attempted.
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
            Command::CreateThread(create) => {
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
                // Fresh acceptance mints a Forge thread identity; no existing
                // backend capability mints one, so fabricating a thread would
                // claim an effect that never happened.
                Err(unbacked_failure(request_id, "fresh thread creation"))
            }
            Command::QueueFirstMessage(queue) => {
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
                // Fresh acceptance mints a Forge message identity and its
                // durable dispatch; neither exists in this build yet.
                Err(unbacked_failure(request_id, "fresh first-message queueing"))
            }
        }
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
/// Unknown client-named state maps to the entity-specific codes; deterministic
/// client mistakes such as reused request identities stay non-retryable input
/// failures; persisted-state problems stay internal without retry hope; only
/// database-operation failures admit that an identical later request may
/// succeed.
fn repository_failure(error: &RepositoryError, request_id: &RequestId) -> ProtocolFailure {
    use RepositoryError as Failure;

    let (code, retryable) = match error {
        Failure::ProjectNotFound { .. } => (ErrorCode::ProjectUnknown, false),
        Failure::ThreadNotFound { .. } => (ErrorCode::ThreadUnknown, false),
        Failure::IdempotencyConflict { .. } | Failure::FirstMessageAlreadyExists { .. } => {
            (ErrorCode::InvalidInput, false)
        }
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
