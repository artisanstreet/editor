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
//!
//! Bounded [`artisan_domain::ConversationQuery`] reads are answered directly
//! from the durable projection reader
//! [`Repository::read_conversation_snapshot`]; subscription start and stop
//! stay unbacked through [`RequestHandler::respond`] while the opt-in
//! [`RequestHandler::respond_with_receipt`] path owns one connection-local
//! registrar. That path prepares subscriptions as `Pending` and leaves their
//! activation to its caller after the response has been written.

use std::sync::Arc;

use artisan_database::{CreateThreadInput, QueueFirstMessageInput, Repository, RepositoryError};
use artisan_domain::{
    Command, ConversationCursor, ConversationRequest, ConversationSubscribe,
    ConversationUnsubscribe, CreateThread, DirectoryId, MessageId, PatchBatch, Query,
    QueueFirstMessage, RequestId, ThreadId, UnixMillis,
};
use artisan_protocol::{
    ClientRequest, ErrorCode, ErrorDetail, FirstMessageReceipt, ProtocolFailure, ResponsePayload,
    ServerResponse,
};
use tokio::sync::Mutex;

use crate::command_admission::{
    CommandOrigin, CommandOriginClockError, CommandOriginEntropyError, SystemCommandOrigin,
};
use crate::conversation_subscription_preparation::{
    PrepareSubscriptionError, prepare_conversation_subscription, stop_conversation_subscription,
};
use crate::conversation_subscription_registry::{
    ActivateError, ApplyBatchError, ConversationSubscriptionRegistry, RegisterError,
    SubscriptionLease, SubscriptionView,
};

/// Detail used when a diagnostic text would exceed the protocol-owned
/// error-detail ceiling. Short by construction, so parsing it cannot fail.
const BOUNDED_DETAIL_FALLBACK: &str = "failure detail exceeded the protocol error-detail bound";

/// Stable detail returned when a resume cursor no longer has a durable tail.
const RESNAPSHOT_REQUIRED_DETAIL: &str = "a fresh conversation resnapshot is required";

/// Stable detail returned when a connection-local subscription generation is
/// exhausted.
const SUBSCRIPTION_GENERATION_EXHAUSTED_DETAIL: &str =
    "conversation subscription registration capacity is exhausted";

/// Cloneable owner of one connection-local conversation subscription table.
///
/// Cloning this registrar shares custody of the same private table without
/// exposing its mutex, map, or synchronous registry. Request-handler receipt
/// identity is deliberately kept outside this value so a registrar clone
/// cannot activate another handler's receipt.
#[derive(Clone, Debug)]
pub struct ConversationSubscriptionRegistrar {
    registry: Arc<Mutex<ConversationSubscriptionRegistry>>,
}

impl Default for ConversationSubscriptionRegistrar {
    fn default() -> Self {
        Self::new()
    }
}

impl ConversationSubscriptionRegistrar {
    /// Creates one empty, independent connection-local subscription table.
    #[must_use]
    pub fn new() -> Self {
        Self {
            registry: Arc::new(Mutex::new(ConversationSubscriptionRegistry::new())),
        }
    }

    /// Returns an owned snapshot of one registered subscription, if present.
    ///
    /// The registry remains private and the returned [`SubscriptionView`] is
    /// detached from later table mutations.
    pub async fn subscription_view(&self, thread_id: &ThreadId) -> Option<SubscriptionView> {
        self.registry.lock().await.view(thread_id)
    }

    /// Records a patch batch whose wire publication has already succeeded.
    ///
    /// Callers must invoke this only after the later writer reports successful
    /// wire publication. This method performs no publication, retry, or cursor
    /// reinterpretation; it applies the exact registry lease, thread, state,
    /// and `from_cursor` fences once.
    ///
    /// # Errors
    ///
    /// Returns the exact [`ApplyBatchError`] from the private registry when
    /// the lease, thread, lifecycle state, or cursor fence is not accepted.
    pub async fn record_published_batch(
        &self,
        lease: &SubscriptionLease,
        batch: &PatchBatch,
    ) -> Result<ConversationCursor, ApplyBatchError> {
        let mut registry = self.registry.lock().await;
        registry.publish_batch(lease, batch)
    }
}

/// Private, non-zero-sized allocation identity for one handler-owned
/// subscription registrar.
type SubscriptionRegistrarIdentity = u8;

/// One private activation capability carried by a local request receipt.
#[derive(Debug)]
struct SubscriptionActivation {
    registrar: Arc<SubscriptionRegistrarIdentity>,
    lease: SubscriptionLease,
}

/// The wire result and local post-write work for one handled request.
///
/// The response result is exactly the value a connection adapter can map to a
/// wire response or correlated protocol error. The receipt remains local and
/// is never part of that wire value.
#[must_use]
#[derive(Debug)]
pub struct RequestHandlerResponse {
    response: Result<ServerResponse, ProtocolFailure>,
    receipt: RequestHandlerReceipt,
}

impl RequestHandlerResponse {
    /// Consumes the handled request into its wire result and local receipt.
    pub fn into_parts(
        self,
    ) -> (
        Result<ServerResponse, ProtocolFailure>,
        RequestHandlerReceipt,
    ) {
        (self.response, self.receipt)
    }

    fn without_receipt(response: Result<ServerResponse, ProtocolFailure>) -> Self {
        Self {
            response,
            receipt: RequestHandlerReceipt::none(),
        }
    }

    fn with_receipt(
        response: Result<ServerResponse, ProtocolFailure>,
        receipt: RequestHandlerReceipt,
    ) -> Self {
        Self { response, receipt }
    }
}

/// Local post-write work produced by a subscription-enabled request.
///
/// A receipt is single-owner and deliberately does not implement [`Clone`].
/// Its private lease and per-handler registrar identity prevent callers from
/// fabricating an activation receipt or using one with a different handler.
/// The identity is checked by allocation identity before registry activation.
/// `None` means that no post-write work exists; `Some` contains exactly one
/// prepared lease and its registrar capability.
#[must_use]
#[derive(Debug)]
pub struct RequestHandlerReceipt {
    activation: Option<SubscriptionActivation>,
}

impl RequestHandlerReceipt {
    /// Returns whether this receipt carries no post-write activation work.
    #[must_use]
    pub const fn is_no_work(&self) -> bool {
        self.activation.is_none()
    }

    fn none() -> Self {
        Self { activation: None }
    }

    fn activate(registrar: Arc<SubscriptionRegistrarIdentity>, lease: SubscriptionLease) -> Self {
        Self {
            activation: Some(SubscriptionActivation { registrar, lease }),
        }
    }
}

/// The activated subscription handed to a future delivery owner.
///
/// The lease is the exact lease that was prepared for the successful request,
/// and the cursor is the exact cursor stored by activation. This value is
/// local state only; it does not claim that any wire response was delivered.
#[must_use]
#[derive(Debug, Eq, PartialEq)]
pub struct ActivatedConversationSubscription {
    lease: SubscriptionLease,
    cursor: ConversationCursor,
}

impl ActivatedConversationSubscription {
    /// Returns the exact lease activated in the handler registry.
    #[must_use]
    pub fn lease(&self) -> &SubscriptionLease {
        &self.lease
    }

    /// Returns the exact cursor declared by the prepared subscription.
    #[must_use]
    pub const fn cursor(&self) -> ConversationCursor {
        self.cursor
    }

    /// Consumes the activated value into its lease and cursor.
    #[must_use]
    pub fn into_parts(self) -> (SubscriptionLease, ConversationCursor) {
        (self.lease, self.cursor)
    }
}

/// Answers decoded client requests from Forge-owned repository state.
#[derive(Debug)]
pub struct RequestHandler {
    repository: Repository,
    origin: AdmissionOrigin,
    subscriptions: Option<ConversationSubscriptionRegistrar>,
    subscription_identity: Option<Arc<SubscriptionRegistrarIdentity>>,
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
            subscriptions: None,
            subscription_identity: None,
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
            subscriptions: None,
            subscription_identity: None,
        }
    }

    /// Creates a handler with the normal system admission origin and a
    /// supplied connection-local conversation subscription registrar.
    ///
    /// A new private allocation identity fences this handler's receipts even
    /// when the registrar is deliberately shared with another handler.
    #[must_use]
    pub fn with_subscription_registrar(
        repository: Repository,
        registrar: ConversationSubscriptionRegistrar,
    ) -> Self {
        let subscription_identity = Arc::new(0_u8);
        Self {
            repository,
            origin: AdmissionOrigin::system(),
            subscriptions: Some(registrar),
            subscription_identity: Some(subscription_identity),
        }
    }

    /// Creates a handler with a fresh connection-local conversation
    /// subscription registrar.
    ///
    /// Ordinary [`Self::respond`] calls continue to use the unbacked
    /// subscription behavior.
    #[must_use]
    pub fn with_subscriptions(repository: Repository) -> Self {
        Self::with_subscription_registrar(repository, ConversationSubscriptionRegistrar::new())
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
            ClientRequest::Conversation(conversation) => {
                self.conversation_outcome(request_id, conversation).await
            }
            ClientRequest::Lifecycle(_) => Err(typed_failure(
                ErrorCode::UnsupportedFeature,
                "native lifecycle control was not negotiated",
                false,
                request_id,
            )),
            // No native picker exists in this build; the separately owned
            // process/admission packet integrates the real chooser. Answer
            // through the established bounded, non-retryable unbacked path
            // instead of claiming a picker outcome.
            ClientRequest::PickDirectory => {
                Err(unbacked_failure(request_id, "native directory picking"))
            }
        }
    }

    /// Resolves one correlated request and returns its local post-write work.
    ///
    /// Queries, commands, directory picking, and every protocol failure use
    /// the exact [`Self::respond`] behavior and carry no post-write work.
    /// Subscription-enabled Subscribe requests perform one durable
    /// preparation and leave the registry `Pending`; activation is never done
    /// in this method. Unsubscribe mutates the same registry immediately and
    /// returns only its protocol acknowledgement.
    ///
    /// # Errors
    ///
    /// The returned wire result contains the same correlated
    /// [`ProtocolFailure`] classifications as [`Self::respond`], plus the
    /// bounded resnapshot-required and generation-exhaustion failures for
    /// subscription preparation. All failures carry a no-work receipt.
    pub async fn respond_with_receipt(
        &self,
        request_id: &RequestId,
        request: &ClientRequest,
    ) -> RequestHandlerResponse {
        match request {
            ClientRequest::Conversation(ConversationRequest::Subscribe(subscribe))
                if self.subscriptions.is_some() =>
            {
                self.subscribe_with_receipt(request_id, request, subscribe)
                    .await
            }
            ClientRequest::Conversation(ConversationRequest::Unsubscribe(unsubscribe))
                if self.subscriptions.is_some() =>
            {
                self.unsubscribe_with_receipt(request_id, request, unsubscribe)
                    .await
            }
            _ => RequestHandlerResponse::without_receipt(self.respond(request_id, request).await),
        }
    }

    /// Activates a real subscription receipt after the caller proves its
    /// response write and send-side finish.
    ///
    /// A no-work receipt returns `Ok(None)` without locking the registry. A
    /// receipt from another handler is rejected by private allocation identity
    /// before this handler's registry is locked or activated. A receipt from
    /// this handler locks its one registry and attempts exactly one typed
    /// activation; the caller owns the wire-order proof.
    ///
    /// # Errors
    ///
    /// Returns [`ActivateError::StaleLease`] when the receipt's lease no
    /// longer identifies the current entry, or
    /// [`ActivateError::AlreadyActive`] when the current entry was already
    /// activated. Neither error is converted into success.
    pub async fn activate_after_response(
        &self,
        receipt: RequestHandlerReceipt,
    ) -> Result<Option<ActivatedConversationSubscription>, ActivateError> {
        let Some(activation) = receipt.activation else {
            return Ok(None);
        };
        let Some(subscription_identity) = self.subscription_identity.as_ref() else {
            return Err(ActivateError::StaleLease);
        };
        if !Arc::ptr_eq(subscription_identity, &activation.registrar) {
            return Err(ActivateError::StaleLease);
        }
        let Some(registrar) = self.subscriptions.as_ref() else {
            return Err(ActivateError::StaleLease);
        };
        let mut registry = registrar.registry.lock().await;
        let cursor = registry.activate(&activation.lease)?;
        Ok(Some(ActivatedConversationSubscription {
            lease: activation.lease,
            cursor,
        }))
    }

    /// Returns an owned read-only view of one handler-local subscription.
    ///
    /// A handler created without a subscription registrar always returns
    /// `None`; the registry itself is never exposed.
    pub async fn subscription_view(&self, thread_id: &ThreadId) -> Option<SubscriptionView> {
        self.subscriptions
            .as_ref()?
            .subscription_view(thread_id)
            .await
    }

    async fn subscribe_with_receipt(
        &self,
        request_id: &RequestId,
        request: &ClientRequest,
        subscribe: &ConversationSubscribe,
    ) -> RequestHandlerResponse {
        let Some(registrar) = self.subscriptions.as_ref() else {
            return RequestHandlerResponse::without_receipt(
                self.respond(request_id, request).await,
            );
        };
        let Some(subscription_identity) = self.subscription_identity.as_ref() else {
            return RequestHandlerResponse::without_receipt(
                self.respond(request_id, request).await,
            );
        };
        let mut registry = registrar.registry.lock().await;
        match prepare_conversation_subscription(&self.repository, &mut registry, subscribe).await {
            Ok(prepared) => {
                let (started, lease) = prepared.into_parts();
                RequestHandlerResponse::with_receipt(
                    Ok(outcome(
                        request_id,
                        ResponsePayload::ConversationSubscriptionStarted(started),
                    )),
                    RequestHandlerReceipt::activate(subscription_identity.clone(), lease),
                )
            }
            Err(error) => {
                RequestHandlerResponse::without_receipt(Err(preparation_failure(error, request_id)))
            }
        }
    }

    async fn unsubscribe_with_receipt(
        &self,
        request_id: &RequestId,
        request: &ClientRequest,
        unsubscribe: &ConversationUnsubscribe,
    ) -> RequestHandlerResponse {
        let Some(registrar) = self.subscriptions.as_ref() else {
            return RequestHandlerResponse::without_receipt(
                self.respond(request_id, request).await,
            );
        };
        let mut registry = registrar.registry.lock().await;
        let (stopped, _outcome) =
            stop_conversation_subscription(&mut registry, unsubscribe).into_parts();
        RequestHandlerResponse::without_receipt(Ok(outcome(
            request_id,
            ResponsePayload::ConversationSubscriptionStopped(stopped),
        )))
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

    /// Answers conversation reads and subscription control.
    ///
    /// `ConversationRequest::Query` calls `Repository::read_conversation_snapshot`
    /// exactly once and preserves the caller's correlated `RequestId`. A query
    /// carries no command id and never consults [`CommandOrigin`]. Failures map
    /// through [`repository_failure`]: absent thread is `ThreadUnknown`,
    /// corrupt or invariant persisted state is non-retryable `Internal`, and
    /// database operation failures are retryable `Internal`. Subscribe and
    /// unsubscribe remain unbacked in this build pending a per-connection
    /// registrar and answer with a subscription-specific non-retryable
    /// `Internal` failure.
    async fn conversation_outcome(
        &self,
        request_id: &RequestId,
        conversation: &ConversationRequest,
    ) -> Result<ServerResponse, ProtocolFailure> {
        match conversation {
            ConversationRequest::Query(query) => {
                let snapshot = self
                    .repository
                    .read_conversation_snapshot(query)
                    .await
                    .map_err(|error| repository_failure(&error, request_id))?;
                Ok(outcome(
                    request_id,
                    ResponsePayload::ConversationSnapshot(snapshot),
                ))
            }
            ConversationRequest::Subscribe(_) => {
                Err(unbacked_failure(request_id, "conversation subscription"))
            }
            ConversationRequest::Unsubscribe(_) => {
                Err(unbacked_failure(request_id, "conversation unsubscription"))
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

/// Maps a subscription-preparation failure without exposing its durable
/// cursor or client request values.
fn preparation_failure(error: PrepareSubscriptionError, request_id: &RequestId) -> ProtocolFailure {
    match error {
        PrepareSubscriptionError::Repository(error) => repository_failure(&error, request_id),
        PrepareSubscriptionError::ResnapshotRequired { .. } => typed_failure(
            ErrorCode::InvalidInput,
            RESNAPSHOT_REQUIRED_DETAIL,
            false,
            request_id,
        ),
        PrepareSubscriptionError::Register(RegisterError::GenerationExhausted) => typed_failure(
            ErrorCode::Internal,
            SUBSCRIPTION_GENERATION_EXHAUSTED_DETAIL,
            false,
            request_id,
        ),
    }
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
