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

use std::{
    fmt,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use artisan_database::{
    AttachProjectInput, CreateThreadInput, QueueFirstMessageInput, Repository, RepositoryError,
    SetThreadEngineConfigInput,
};
use artisan_domain::{
    Command, ConversationCursor, ConversationRequest, ConversationSubscribe,
    ConversationUnsubscribe, CreateThread, DirectoryId, EngineProfileId, MessageId, PatchBatch,
    ProjectId, Query, QueueFirstMessage, RequestId, RootPath, SetThreadEngineConfig, ThreadId,
    UnixMillis,
};
use artisan_protocol::{
    ClientRequest, DirectoryPickOutcome as ProtocolDirectoryPickOutcome, ErrorCode, ErrorDetail,
    FirstMessageReceipt, ProtocolFailure, RegisteredEngineProfilesResult, ResponsePayload,
    ServerResponse, SetThreadEngineConfigResult,
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
use crate::directory_controller::{
    AdmissionError, DirectoryController, DirectoryPickOutcome, HelperOperationError, ShutdownReport,
};
use crate::directory_selection::{DirectorySelectionAdmissionError, SelectedDirectoryAuthority};

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

struct DirectoryPicker {
    controller: DirectoryController,
    authority: Mutex<SelectedDirectoryAuthority>,
    budget: Duration,
}

/// Answers decoded client requests from Forge-owned repository state.
pub struct RequestHandler {
    repository: Repository,
    origin: AdmissionOrigin,
    subscriptions: Option<ConversationSubscriptionRegistrar>,
    subscription_identity: Option<Arc<SubscriptionRegistrarIdentity>>,
    directory_picker: Option<DirectoryPicker>,
    registered_engine_profiles: Option<Box<dyn RegisteredEngineProfilesReader>>,
}

impl fmt::Debug for RequestHandler {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RequestHandler { <payload-free> }")
    }
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

/// Finite, path-free error for the registered engine profile catalogue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegisteredEngineProfilesError;

impl fmt::Display for RegisteredEngineProfilesError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("registered engine profiles are unavailable")
    }
}

impl std::error::Error for RegisteredEngineProfilesError {}

/// Narrow, path-free reader boundary for the registered engine profile
/// catalogue.
///
/// The trait surface is intentionally finite and `Send + Sync` so tests can
/// inject a deterministic implementation and production can delegate to the
/// certified native authority without leaking path, registry bytes,
/// executable, install, or raw authority details through `Debug` or `Display`.
pub trait RegisteredEngineProfilesReader: fmt::Debug + Send + Sync {
    /// Lists the validated registry entries.
    ///
    /// `Ok(None)` means the registry file is missing. `Ok(Some(ids))` means
    /// the registry file exists and contains exactly the ordered profile ids,
    /// which may be empty. Any `Err` is treated as an internal, non-retryable,
    /// path-free failure.
    fn list_profiles(&self) -> Result<Option<Vec<EngineProfileId>>, RegisteredEngineProfilesError>;
}

/// Production native reader that owns only the explicit database path and
/// delegates to the certified `artisan-native-engine` authority.
///
/// `Debug` is payload-free so no path or authority material can leak.
pub struct NativeRegisteredEngineProfilesReader {
    database_path: PathBuf,
}

impl NativeRegisteredEngineProfilesReader {
    /// Creates a production reader owning the explicit database path.
    #[must_use]
    pub fn new(database_path: PathBuf) -> Self {
        Self { database_path }
    }
}

impl fmt::Debug for NativeRegisteredEngineProfilesReader {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("NativeRegisteredEngineProfilesReader { <payload-free> }")
    }
}

impl RegisteredEngineProfilesReader for NativeRegisteredEngineProfilesReader {
    fn list_profiles(&self) -> Result<Option<Vec<EngineProfileId>>, RegisteredEngineProfilesError> {
        let authority = artisan_native_engine::NativeOpenCode2Authority::new();
        match authority.list_profiles(&self.database_path) {
            Ok(None) => Ok(None),
            Ok(Some(profiles)) => Ok(Some(
                profiles
                    .into_iter()
                    .map(|profile| profile.profile_id().clone())
                    .collect(),
            )),
            Err(_) => Err(RegisteredEngineProfilesError),
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
            directory_picker: None,
            registered_engine_profiles: None,
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
            directory_picker: None,
            registered_engine_profiles: None,
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
            directory_picker: None,
            registered_engine_profiles: None,
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

    /// Creates a handler with the process-owned native directory picker and
    /// one selection authority, retaining the normal subscription registrar
    /// and system command origin.
    #[must_use]
    pub fn with_directory_picker(
        repository: Repository,
        directory_controller: DirectoryController,
        pick_budget: Duration,
    ) -> Self {
        let mut handler = Self::with_subscriptions(repository);
        handler.directory_picker = Some(DirectoryPicker {
            controller: directory_controller,
            authority: Mutex::new(SelectedDirectoryAuthority::new()),
            budget: pick_budget,
        });
        handler
    }

    /// Attaches a deterministic or native registered engine profile reader.
    ///
    /// The reader is path-free and `Send + Sync` so tests can inject a scripted
    /// implementation while production wires the native authority. Existing
    /// constructors stay unconfigured by default; calling this method adds the
    /// single bounded reader without creating a second handler, repository
    /// connection, or authority.
    #[must_use]
    pub fn with_registered_engine_profiles_reader(
        mut self,
        reader: impl RegisteredEngineProfilesReader + 'static,
    ) -> Self {
        self.registered_engine_profiles = Some(Box::new(reader));
        self
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
            ClientRequest::PickDirectory => self.pick_directory_outcome(request_id).await,
        }
    }

    /// Stops the handler-owned directory controller and reports its observed
    /// shutdown result. Legacy handlers have no controller to stop.
    pub(crate) async fn shutdown_directory_controller(&mut self) -> Option<ShutdownReport> {
        let picker = self.directory_picker.as_mut()?;
        Some(picker.controller.shutdown().await)
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

    async fn pick_directory_outcome(
        &self,
        request_id: &RequestId,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let Some(directory_picker) = self.directory_picker.as_ref() else {
            return Err(unbacked_failure(request_id, "native directory picking"));
        };

        let pick_result = directory_picker
            .controller
            .pick_directory(directory_picker.budget)
            .map_err(|error| directory_admission_failure(error, request_id))?
            .await
            .map_err(|error| helper_operation_failure(&error, request_id))?;
        let root_path = match pick_result {
            DirectoryPickOutcome::Selected { canonical_path } => RootPath::parse(canonical_path)
                .map_err(|_| {
                    typed_failure(
                        ErrorCode::Internal,
                        "native directory picker returned an invalid canonical root path",
                        false,
                        request_id,
                    )
                })?,
            DirectoryPickOutcome::Cancelled => {
                return Ok(outcome(
                    request_id,
                    ResponsePayload::DirectoryPicked(ProtocolDirectoryPickOutcome::Cancelled),
                ));
            }
            other => return Err(picker_outcome_failure(&other, request_id)),
        };

        let identity = self
            .origin
            .mint_identity()
            .map_err(|error| origin_entropy_failure(&error, request_id))?;
        let directory_id = DirectoryId::parse(identity)
            .map_err(|_| forged_identity_failure("directory", request_id))?;
        let mut authority = directory_picker.authority.lock().await;
        let issued = authority
            .register(directory_id, root_path, Instant::now())
            .map_err(|error| directory_selection_failure(error, request_id))?;
        Ok(outcome(
            request_id,
            ResponsePayload::DirectoryPicked(ProtocolDirectoryPickOutcome::Selected(
                issued.directory_id,
            )),
        ))
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
            Query::ReadThreadEngineSettings(read) => {
                let thread_id = read.thread_id();
                match self.repository.read_thread_engine_settings(thread_id).await {
                    Ok(None) => Ok(outcome(
                        request_id,
                        ResponsePayload::ThreadEngineSettings(
                            artisan_protocol::ThreadEngineSettingsResult::Unconfigured {
                                thread_id: thread_id.clone(),
                            },
                        ),
                    )),
                    Ok(Some(settings)) => Ok(outcome(
                        request_id,
                        ResponsePayload::ThreadEngineSettings(
                            artisan_protocol::ThreadEngineSettingsResult::Configured {
                                thread_id: thread_id.clone(),
                                revision: settings.revision(),
                                config: Box::new(settings.config().clone()),
                            },
                        ),
                    )),
                    Err(error) => Err(repository_failure(&error, request_id)),
                }
            }
            Query::ListRegisteredEngineProfiles(_) => {
                let Some(reader) = self.registered_engine_profiles.as_ref() else {
                    return Err(typed_failure(
                        ErrorCode::UnsupportedFeature,
                        "registered engine profiles are not supported",
                        false,
                        request_id,
                    ));
                };
                match reader.list_profiles() {
                    Ok(None) => Ok(outcome(
                        request_id,
                        ResponsePayload::RegisteredEngineProfiles(
                            RegisteredEngineProfilesResult::RegistryMissing,
                        ),
                    )),
                    Ok(Some(profile_ids)) => Ok(outcome(
                        request_id,
                        ResponsePayload::RegisteredEngineProfiles(
                            RegisteredEngineProfilesResult::RegistryPresent { profile_ids },
                        ),
                    )),
                    Err(_) => Err(typed_failure(
                        ErrorCode::Internal,
                        "registered engine profiles are unavailable",
                        false,
                        request_id,
                    )),
                }
            }
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
                let Some(picker) = self.directory_picker.as_ref() else {
                    return Err(unknown_directory_failure(request_id, &attach.directory_id));
                };
                let mut authority = picker.authority.lock().await;
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
                let Some(selected) = authority.consume(&attach.directory_id, Instant::now()) else {
                    return Err(unknown_directory_failure(request_id, &attach.directory_id));
                };
                let identity = self
                    .origin
                    .mint_identity()
                    .map_err(|error| origin_entropy_failure(&error, request_id))?;
                let project_id = ProjectId::parse(identity)
                    .map_err(|_| forged_identity_failure("project", request_id))?;
                let attached_at = self
                    .origin
                    .acceptance_instant()
                    .map_err(|error| origin_clock_failure(error, request_id))?;
                let result = self
                    .repository
                    .attach_project(AttachProjectInput {
                        request_id: attach.request_id.clone(),
                        directory_id: selected.directory_id,
                        project_id,
                        root_path: selected.root_path,
                        display_name: selected.display_name,
                        attached_at,
                    })
                    .await
                    .map_err(|error| repository_failure(&error, request_id))?;
                Ok(outcome(
                    request_id,
                    ResponsePayload::AttachedProject {
                        project: result.project,
                        disposition: result.receipt.disposition,
                    },
                ))
            }
            Command::CreateThread(create) => self.create_thread_outcome(request_id, create).await,
            Command::QueueFirstMessage(queue) => {
                self.queue_first_message_outcome(request_id, queue).await
            }
            Command::SetThreadEngineConfig(config) => {
                self.set_thread_engine_config_outcome(request_id, config.as_ref())
                    .await
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

    /// Answers a durable thread engine-configuration mutation. Receipt
    /// lookup is deliberately before the acceptance clock so exact replays
    /// never consult fresh admission state.
    async fn set_thread_engine_config_outcome(
        &self,
        request_id: &RequestId,
        config: &SetThreadEngineConfig,
    ) -> Result<ServerResponse, ProtocolFailure> {
        if let Some(replay) = self
            .repository
            .lookup_set_thread_engine_config(
                config.request_id(),
                config.thread_id(),
                config.precondition(),
                config.config(),
            )
            .await
            .map_err(|error| repository_failure(&error, request_id))?
        {
            return Ok(set_thread_engine_config_response(request_id, &replay));
        }
        let accepted_at = self
            .origin
            .acceptance_instant()
            .map_err(|error| origin_clock_failure(error, request_id))?;
        let result = self
            .repository
            .set_thread_engine_config(SetThreadEngineConfigInput {
                request_id: config.request_id().clone(),
                thread_id: config.thread_id().clone(),
                precondition: config.precondition(),
                config: config.config().clone(),
                accepted_at,
            })
            .await
            .map_err(|error| repository_failure(&error, request_id))?;
        Ok(set_thread_engine_config_response(request_id, &result))
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
        Failure::EngineConfigRevisionConflict { .. } => (ErrorCode::EngineConfigConflict, false),
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

fn set_thread_engine_config_response(
    request_id: &RequestId,
    result: &artisan_database::SetThreadEngineConfigResult,
) -> ServerResponse {
    outcome(
        request_id,
        ResponsePayload::ThreadEngineConfigSet(SetThreadEngineConfigResult {
            request_id: result.receipt().request_id.clone(),
            thread_id: result.thread_id().clone(),
            revision: result.revision(),
            disposition: result.receipt().disposition,
        }),
    )
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

/// Maps controller queue admission without exposing any operation payload.
fn directory_admission_failure(error: AdmissionError, request_id: &RequestId) -> ProtocolFailure {
    let (detail, retryable) = match error {
        AdmissionError::Unavailable => ("native directory picker is unavailable", false),
        AdmissionError::Busy => ("native directory picker is busy", true),
        AdmissionError::InvalidDeadline => ("native directory picker deadline is invalid", false),
        AdmissionError::EmptyPath => ("native directory picker path is empty", false),
        AdmissionError::PathTooLong => ("native directory picker path is too long", false),
    };
    typed_failure(ErrorCode::Internal, detail, retryable, request_id)
}

/// Maps one controller operation failure to the stable protocol vocabulary.
fn helper_operation_failure(
    error: &HelperOperationError,
    request_id: &RequestId,
) -> ProtocolFailure {
    let (detail, retryable) = match error {
        HelperOperationError::Cancelled => (
            "native directory picker was cancelled or abandoned by its controller",
            true,
        ),
        HelperOperationError::Deadline => {
            ("native directory picker exceeded its request budget", true)
        }
        HelperOperationError::Shutdown => (
            "native directory picker shut down during the request",
            false,
        ),
        HelperOperationError::TaskLost => ("native directory picker owner task was lost", false),
        HelperOperationError::GenerationExhausted => (
            "native directory picker generation capacity is exhausted",
            false,
        ),
        HelperOperationError::SpawnFailed => {
            ("native directory picker helper could not start", true)
        }
        HelperOperationError::InvalidRequest => {
            ("native directory picker request was invalid", false)
        }
        HelperOperationError::WriteFailed => ("native directory picker request pipe failed", true),
        HelperOperationError::ReadFailed => ("native directory picker response pipe failed", true),
        HelperOperationError::MalformedFrame => (
            "native directory picker returned a malformed response",
            false,
        ),
        HelperOperationError::TruncatedFrame => (
            "native directory picker returned a truncated response",
            false,
        ),
        HelperOperationError::TrailingOutput => {
            ("native directory picker returned trailing output", false)
        }
        HelperOperationError::StaleGeneration => {
            ("native directory picker returned a stale response", false)
        }
        HelperOperationError::OversizedOutput => {
            ("native directory picker response exceeded its bound", false)
        }
        HelperOperationError::StderrCapExceeded => (
            "native directory picker diagnostic output exceeded its bound",
            false,
        ),
        HelperOperationError::ExitFailure => ("native directory picker helper failed", true),
        HelperOperationError::UnresolvedReapDuring { .. }
        | HelperOperationError::ReapUnresolved => (
            "native directory picker helper cleanup could not confirm reaping",
            false,
        ),
    };
    typed_failure(ErrorCode::Internal, detail, retryable, request_id)
}

/// Maps a successful helper outcome that cannot be represented by the wire
/// picker outcome.
fn picker_outcome_failure(
    outcome: &DirectoryPickOutcome,
    request_id: &RequestId,
) -> ProtocolFailure {
    let (code, detail, retryable) = match outcome {
        DirectoryPickOutcome::InvalidPath => (
            ErrorCode::Internal,
            "native directory picker returned an invalid path",
            false,
        ),
        DirectoryPickOutcome::UnsupportedEncoding => (
            ErrorCode::Internal,
            "native directory picker returned unsupported path encoding",
            false,
        ),
        DirectoryPickOutcome::UnsupportedPlatform => (
            ErrorCode::UnsupportedFeature,
            "native directory picking is unsupported on this platform",
            false,
        ),
        DirectoryPickOutcome::DialogFailed => (
            ErrorCode::Internal,
            "native directory picker dialog failed",
            true,
        ),
        DirectoryPickOutcome::Cancelled | DirectoryPickOutcome::Selected { .. } => {
            unreachable!("user cancellation and selection are handled before failure mapping")
        }
    };
    typed_failure(code, detail, retryable, request_id)
}

/// Maps authority admission while keeping selection payloads private.
fn directory_selection_failure(
    error: DirectorySelectionAdmissionError,
    request_id: &RequestId,
) -> ProtocolFailure {
    let (detail, retryable) = match error {
        DirectorySelectionAdmissionError::IdentityAlreadyIssued => (
            "native directory selection identity was already issued",
            false,
        ),
        DirectorySelectionAdmissionError::LiveCapacityFull => {
            ("native directory selection capacity is full", true)
        }
        DirectorySelectionAdmissionError::LifetimeExhausted => (
            "native directory selection lifetime capacity is exhausted",
            false,
        ),
        DirectorySelectionAdmissionError::DeadlineOverflow => {
            ("native directory selection deadline is invalid", false)
        }
        DirectorySelectionAdmissionError::DisplayName(_) => {
            ("native directory selection display name is invalid", false)
        }
        DirectorySelectionAdmissionError::UnnamedRootForm => {
            ("native directory selection root is unnamed", false)
        }
    };
    typed_failure(ErrorCode::Internal, detail, retryable, request_id)
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
