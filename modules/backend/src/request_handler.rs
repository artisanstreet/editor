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
//! stay unbacked through [`RequestHandler::respond`]. The receipt paths
//! prepare subscriptions as `Pending` and leave activation to their caller
//! after the response has been written; the authenticated delivery path
//! supplies a fresh connection-owned registrar context for that work.

use std::{fmt, path::PathBuf, sync::Arc, time::Duration};

use artisan_database::{CreateThreadInput, QueueFirstMessageInput, Repository, RepositoryError};
use artisan_domain::{
    Command, ConversationCursor, ConversationRequest, ConversationSubscribe,
    ConversationUnsubscribe, CreateThread, EngineProfileId, MessageId, PatchBatch,
    QueueFirstMessage, RequestId, SetModelFavorite, ThreadId, UnixMillis,
};
use artisan_protocol::{
    ClientRequest, ErrorCode, FirstMessageReceipt, ProtocolFailure, ResponsePayload,
    ServerResponse, StopRunDisposition, StopRunReceipt,
};
use tokio::sync::Mutex;

use self::failures::{
    forged_identity_failure, origin_clock_failure, origin_entropy_failure, outcome,
    preparation_failure, repository_failure, run_cancellation_failure, typed_failure,
};
use crate::command_admission::{
    CommandOrigin, CommandOriginClockError, CommandOriginEntropyError, SystemCommandOrigin,
};
use crate::conversation_commit_notifier::ConversationCommitNotifier;
use crate::conversation_subscription_preparation::{
    prepare_conversation_subscription, stop_conversation_subscription,
};
use crate::conversation_subscription_registry::{
    ActivateError, ApplyBatchError, ApplyObservationBatchError, ConversationSubscriptionRegistry,
    SubscriptionLease, SubscriptionView,
};
use crate::directory_controller::{DirectoryController, ShutdownReport};
use crate::directory_selection::SelectedDirectoryAuthority;
use crate::run_cancellation::{CancelRequestOutcome, RunCancellationRegistry};
use crate::run_interaction::RunInteractionRegistry;
/// Detail used when a diagnostic text would exceed the protocol-owned
/// error-detail ceiling. Short by construction, so parsing it cannot fail.
const BOUNDED_DETAIL_FALLBACK: &str = "failure detail exceeded the protocol error-detail bound";

/// Stable detail returned when a resume cursor no longer has a durable tail.
const RESNAPSHOT_REQUIRED_DETAIL: &str = "a fresh conversation resnapshot is required";

/// Stable detail returned when a connection-local subscription generation is
/// exhausted.
const SUBSCRIPTION_GENERATION_EXHAUSTED_DETAIL: &str =
    "conversation subscription registration capacity is exhausted";

/// Stable detail for builds whose native dispatcher has not supplied the
/// shared live-run cancellation registry.
const RUN_CANCELLATION_UNAVAILABLE_DETAIL: &str =
    "live run cancellation is not available in this build";

/// Stable detail for builds whose native dispatcher has not supplied the
/// shared live-run interaction registry.
const RUN_INTERACTION_UNAVAILABLE_DETAIL: &str =
    "live run interaction is not available in this build";

/// Stable detail for a response whose target run cannot accept input right
/// now. The outcome is never stored, so the client may retry once the owning
/// run is live.
const RUN_INTERACTION_INBOX_BUSY_DETAIL: &str = "live run interaction inbox is busy";

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

    /// Records an observation batch whose wire publication has already succeeded.
    ///
    /// Callers must invoke this only after the later writer reports
    /// successful wire publication of every observation in the batch. This
    /// method performs no publication, retry, or cursor reinterpretation; it
    /// applies the exact registry lease, thread, state, and
    /// `from_sequence` fences once.
    ///
    /// # Errors
    ///
    /// Returns the exact [`ApplyObservationBatchError`] from the private
    /// registry when the lease, thread, lifecycle state, or cursor fence is
    /// not accepted.
    pub async fn record_published_observation_batch(
        &self,
        lease: &SubscriptionLease,
        thread_id: &ThreadId,
        from_sequence: u64,
        to_sequence: u64,
    ) -> Result<u64, ApplyObservationBatchError> {
        let mut registry = self.registry.lock().await;
        registry.publish_observation_batch(lease, thread_id, from_sequence, to_sequence)
    }

    /// Clears every entry owned by this connection's registrar.
    pub(crate) async fn clear_all(&self) {
        self.registry.lock().await.clear_all();
    }
}

/// Private, non-zero-sized allocation identity for one handler-owned
/// subscription registrar.
type SubscriptionRegistrarIdentity = u8;

/// Fresh connection-owned request state for native conversation delivery.
///
/// The production handler retains only process-wide configuration. Each
/// authenticated connection receives a new registrar, identity fence, and
/// repository handle while sharing the injected process-wide commit notifier.
/// The context is intentionally not cloneable: the connection driver is its
/// sole owner of the active subscription state.
#[derive(Debug)]
pub(crate) struct ConversationConnectionContext {
    repository: Repository,
    registrar: ConversationSubscriptionRegistrar,
    identity: Arc<SubscriptionRegistrarIdentity>,
    notifier: ConversationCommitNotifier,
    account_usage: Option<Arc<crate::account_usage_service::AccountUsageService>>,
    run_cancellation: Option<RunCancellationRegistry>,
    /// Held from this connection's first handled request: an Editor
    /// observing usage (lifecycle requests never reach the handler).
    usage_observer: std::sync::OnceLock<crate::account_usage_service::UsageObserver>,
}

impl ConversationConnectionContext {
    fn observe_usage(&self) {
        if let Some(usage) = &self.account_usage {
            self.usage_observer.get_or_init(|| usage.observe());
        }
    }

    /// The live run registered for `thread`, if any.
    pub(crate) fn live_run(&self, thread: &ThreadId) -> Option<artisan_domain::RunId> {
        self.run_cancellation.as_ref()?.active_run(thread).ok()?
    }

    pub(crate) fn repository(&self) -> &Repository {
        &self.repository
    }

    pub(crate) fn account_usage(
        &self,
    ) -> Option<&crate::account_usage_service::AccountUsageService> {
        self.account_usage.as_deref()
    }

    pub(crate) fn registrar(&self) -> &ConversationSubscriptionRegistrar {
        &self.registrar
    }

    pub(crate) fn notifier(&self) -> &ConversationCommitNotifier {
        &self.notifier
    }
}

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

    /// Advances the local delivery cursor to the exact cursor returned by a
    /// successful authoritative replay publication.
    pub(crate) fn advance_to(&mut self, cursor: ConversationCursor) {
        self.cursor = cursor;
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
    conversation_commit_notifier: Option<ConversationCommitNotifier>,
    directory_picker: Option<DirectoryPicker>,
    registered_engine_profiles: Option<Box<dyn RegisteredEngineProfilesReader>>,
    run_cancellation: Option<RunCancellationRegistry>,
    run_interaction: Option<RunInteractionRegistry>,
    composer_catalog: Option<crate::composer_catalog_service::ComposerCatalogService>,
    account_usage: Option<Arc<crate::account_usage_service::AccountUsageService>>,
    rich_link_resolver: Option<crate::rich_link_service::RichLinkResolver>,
    project_repository: Option<crate::project_repository_service::ProjectRepositoryService>,
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
    ///
    /// # Errors
    ///
    /// Returns [`RegisteredEngineProfilesError`] when the registry cannot be
    /// read because it is unavailable, malformed, or otherwise invalid. The
    /// error is finite and path-free with no registry bytes or authority
    /// details.
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
            conversation_commit_notifier: None,
            directory_picker: None,
            registered_engine_profiles: None,
            run_cancellation: None,
            run_interaction: None,
            composer_catalog: None,
            account_usage: None,
            rich_link_resolver: None,
            project_repository: None,
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
            conversation_commit_notifier: None,
            directory_picker: None,
            registered_engine_profiles: None,
            run_cancellation: None,
            run_interaction: None,
            composer_catalog: None,
            account_usage: None,
            rich_link_resolver: None,
            project_repository: None,
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
            conversation_commit_notifier: None,
            directory_picker: None,
            registered_engine_profiles: None,
            run_cancellation: None,
            run_interaction: None,
            composer_catalog: None,
            account_usage: None,
            rich_link_resolver: None,
            project_repository: None,
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

    /// Attaches the process-wide notifier used to create fresh connection
    /// subscription contexts.
    #[must_use]
    pub fn with_conversation_commit_notifier(
        mut self,
        notifier: ConversationCommitNotifier,
    ) -> Self {
        self.conversation_commit_notifier = Some(notifier);
        self
    }

    /// Attaches the one process-owned thread/profile catalog service shared
    /// with native dispatch. The service resolves its scope from durable
    /// repository state and performs bounded owner admission; it owns no run,
    /// provider session, or frontend publication state.
    #[must_use]
    pub(crate) fn with_composer_catalog(
        mut self,
        service: crate::composer_catalog_service::ComposerCatalogService,
    ) -> Self {
        self.composer_catalog = Some(service);
        self
    }

    /// Attaches the one process-owned account-usage fan-out service.
    ///
    /// The service fans out per requested engine with freshness caching and
    /// per-engine failure isolation; connection delivery pushes what it
    /// serves. Public so tests can inject scripted readers while production
    /// shares the provider-backed roster with its refresher.
    #[must_use]
    pub fn with_account_usage_service(
        self,
        service: crate::account_usage_service::AccountUsageService,
    ) -> Self {
        self.with_shared_account_usage_service(Arc::new(service))
    }

    /// Attaches an account-usage service shared with its refresher.
    #[must_use]
    pub fn with_shared_account_usage_service(
        mut self,
        service: Arc<crate::account_usage_service::AccountUsageService>,
    ) -> Self {
        self.account_usage = Some(service);
        self
    }

    /// Attaches the one process-owned bounded rich-link resolver.
    ///
    /// The resolver owns the outbound HTML fetch, metadata parse, and its
    /// bounded TTL cache. Without it, `ResolveRichLink` answers the
    /// established unsupported-capability failure instead of fabricating a
    /// title.
    #[must_use]
    pub fn with_rich_link_resolver(
        mut self,
        resolver: crate::rich_link_service::RichLinkResolver,
    ) -> Self {
        self.rich_link_resolver = Some(resolver);
        self
    }

    /// Attaches the one process-owned bounded project-repository reader.
    ///
    /// The reader inspects the durable catalog's stored project roots on
    /// demand and owns no persisted state. Without it,
    /// `QueryProjectRepository` answers the established
    /// unsupported-capability failure instead of fabricating repository
    /// facts.
    #[must_use]
    pub fn with_project_repository_service(
        mut self,
        service: crate::project_repository_service::ProjectRepositoryService,
    ) -> Self {
        self.project_repository = Some(service);
        self
    }

    /// Attaches the one process-owned live-run cancellation registry shared
    /// with native dispatch. The registry contains only exact live
    /// `(thread_id, run_id)` routes; it owns no terminal state or provider.
    #[must_use]
    pub fn with_run_cancellation_registry(mut self, registry: RunCancellationRegistry) -> Self {
        self.run_cancellation = Some(registry);
        self
    }

    /// Attaches the one process-owned live-run interaction registry shared
    /// with native dispatch. The registry contains only exact live
    /// `(thread_id, run_id)` inboxes; the owning dispatch loop behind each
    /// inbox resolves responses transactionally and owns every durable
    /// effect.
    #[must_use]
    pub fn with_run_interaction_registry(mut self, registry: RunInteractionRegistry) -> Self {
        self.run_interaction = Some(registry);
        self
    }

    /// Creates the private subscription context for one authenticated
    /// connection. A missing notifier leaves the legacy unbacked handler
    /// path unchanged; production composition must inject one to enable
    /// conversation delivery.
    pub(crate) fn new_conversation_connection_context(
        &self,
    ) -> Option<ConversationConnectionContext> {
        let context = ConversationConnectionContext {
            repository: self.repository.clone(),
            registrar: ConversationSubscriptionRegistrar::new(),
            identity: Arc::new(0_u8),
            notifier: self.conversation_commit_notifier.clone()?,
            account_usage: self.account_usage.clone(),
            run_cancellation: self.run_cancellation.clone(),
            usage_observer: std::sync::OnceLock::new(),
        };
        Some(context)
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
            ClientRequest::ValidateDirectory(path) => {
                self.pick_directory_outcome(request_id, Some(path.as_str()))
                    .await
            }
            ClientRequest::PickDirectory => self.pick_directory_outcome(request_id, None).await,
            ClientRequest::ResolveRichLink(request) => {
                self.resolve_rich_link_outcome(request_id, request).await
            }
            ClientRequest::QueryProjectRepository(query) => {
                self.query_project_repository_outcome(request_id, query)
                    .await
            }
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

    /// Resolves one request against a fresh connection-owned subscription
    /// context. Non-subscription requests retain the ordinary handler path.
    pub(crate) async fn respond_with_receipt_in_context(
        &self,
        context: &ConversationConnectionContext,
        request_id: &RequestId,
        request: &ClientRequest,
    ) -> RequestHandlerResponse {
        context.observe_usage();
        match request {
            ClientRequest::Conversation(ConversationRequest::Subscribe(subscribe)) => {
                self.subscribe_with_receipt_in_context(context, request_id, subscribe)
                    .await
            }
            ClientRequest::Conversation(ConversationRequest::Unsubscribe(unsubscribe)) => {
                self.unsubscribe_with_receipt_in_context(context, request_id, unsubscribe)
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
        Self::activate_receipt(
            receipt,
            self.subscription_identity.as_ref(),
            self.subscriptions.as_ref(),
        )
        .await
    }

    /// Activates a receipt against the supplied connection-owned context
    /// after its correlated response has crossed the wire and finished.
    pub(crate) async fn activate_after_response_in_context(
        &self,
        context: &ConversationConnectionContext,
        receipt: RequestHandlerReceipt,
    ) -> Result<Option<ActivatedConversationSubscription>, ActivateError> {
        Self::activate_receipt(receipt, Some(&context.identity), Some(context.registrar())).await
    }

    async fn activate_receipt(
        receipt: RequestHandlerReceipt,
        subscription_identity: Option<&Arc<SubscriptionRegistrarIdentity>>,
        subscriptions: Option<&ConversationSubscriptionRegistrar>,
    ) -> Result<Option<ActivatedConversationSubscription>, ActivateError> {
        let Some(activation) = receipt.activation else {
            return Ok(None);
        };
        let Some(subscription_identity) = subscription_identity else {
            return Err(ActivateError::StaleLease);
        };
        if !Arc::ptr_eq(subscription_identity, &activation.registrar) {
            return Err(ActivateError::StaleLease);
        }
        let Some(registrar) = subscriptions else {
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

    async fn subscribe_with_receipt_in_context(
        &self,
        context: &ConversationConnectionContext,
        request_id: &RequestId,
        subscribe: &ConversationSubscribe,
    ) -> RequestHandlerResponse {
        let mut registry = context.registrar.registry.lock().await;
        match prepare_conversation_subscription(&context.repository, &mut registry, subscribe).await
        {
            Ok(prepared) => {
                let (started, lease) = prepared.into_parts();
                RequestHandlerResponse::with_receipt(
                    Ok(outcome(
                        request_id,
                        ResponsePayload::ConversationSubscriptionStarted(started),
                    )),
                    RequestHandlerReceipt::activate(context.identity.clone(), lease),
                )
            }
            Err(error) => {
                RequestHandlerResponse::without_receipt(Err(preparation_failure(error, request_id)))
            }
        }
    }

    async fn unsubscribe_with_receipt_in_context(
        &self,
        context: &ConversationConnectionContext,
        request_id: &RequestId,
        unsubscribe: &ConversationUnsubscribe,
    ) -> RequestHandlerResponse {
        let mut registry = context.registrar.registry.lock().await;
        let (stopped, _outcome) =
            stop_conversation_subscription(&mut registry, unsubscribe).into_parts();
        RequestHandlerResponse::without_receipt(Ok(outcome(
            request_id,
            ResponsePayload::ConversationSubscriptionStopped(stopped),
        )))
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

        let response = match command {
            Command::AttachProject(attach) => self.attach_project_outcome(request_id, attach).await,
            Command::CreateThread(create) => self.create_thread_outcome(request_id, create).await,
            Command::QueueFirstMessage(queue) => {
                self.queue_first_message_outcome(request_id, queue).await
            }
            Command::QueueMessage(queue) => self.queue_message_outcome(request_id, queue).await,
            Command::StopRun(stop) => self.stop_run_outcome(request_id, stop),
            Command::RespondApproval(respond) => {
                self.respond_approval_outcome(request_id, respond).await
            }
            Command::RespondQuestion(respond) => {
                self.respond_question_outcome(request_id, respond).await
            }
            Command::SetThreadEngineConfig(config) => {
                let response = self
                    .set_thread_engine_config_outcome(request_id, config.as_ref())
                    .await;
                if response.is_ok() {
                    // A configuration the user saves is the new default.
                    self.remember_default_engine_config(config.config()).await;
                }
                response
            }
            Command::WithdrawQueuedMessage(command) => {
                self.withdraw_composer_message(request_id, command).await
            }
            Command::SetModelFavorite(favorite) => {
                self.set_model_favorite_outcome(request_id, favorite).await
            }
            Command::SaveComposerDraft(save) => self.save_composer_draft(request_id, save).await,
            Command::UploadComposerAttachment(upload) => {
                self.upload_composer_attachment(request_id, upload).await
            }
            Command::QueueStoredMessage(queue) => {
                self.queue_stored_message_outcome(request_id, queue).await
            }
            Command::RetryFailedMessage(retry) => {
                self.retry_failed_message_outcome(request_id, retry).await
            }
            Command::RecoverFailedMessage(recover) => {
                self.recover_failed_message_outcome(request_id, recover)
                    .await
            }
            Command::SubmitComposerDraft(submit) => {
                self.submit_composer_draft_outcome(request_id, submit).await
            }
            Command::RecordNavigation(record) => {
                self.record_navigation_outcome(request_id, record).await
            }
            Command::ImportLegacyPreferences(import) => {
                self.import_legacy_preferences_outcome(request_id, import)
                    .await
            }
        };
        if response.is_ok() {
            self.wake_message_outbox(command);
            self.wake_preferences(command);
        }
        response
    }

    fn stop_run_outcome(
        &self,
        request_id: &RequestId,
        stop: &artisan_domain::StopRun,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let Some(registry) = self.run_cancellation.as_ref() else {
            return Err(typed_failure(
                ErrorCode::UnsupportedFeature,
                RUN_CANCELLATION_UNAVAILABLE_DETAIL,
                false,
                request_id,
            ));
        };
        let disposition = registry
            .request_cancel(stop.thread_id(), stop.run_id())
            .map_err(|error| run_cancellation_failure(error, request_id))?;
        let disposition = match disposition {
            CancelRequestOutcome::Signalled => StopRunDisposition::Requested,
            CancelRequestOutcome::AlreadySignalled => StopRunDisposition::AlreadyRequested,
            CancelRequestOutcome::NotActive => StopRunDisposition::NotActive,
        };
        Ok(outcome(
            request_id,
            ResponsePayload::RunStopped(StopRunReceipt {
                request_id: request_id.clone(),
                thread_id: stop.thread_id().clone(),
                run_id: stop.run_id().clone(),
                disposition,
            }),
        ))
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
        let result = self.create_thread_record(request_id, create).await?;
        Ok(outcome(
            request_id,
            ResponsePayload::CreatedThread {
                thread: result.thread,
                disposition: result.receipt.disposition,
            },
        ))
    }

    /// Creates a thread, or replays the thread its request already created.
    async fn create_thread_record(
        &self,
        request_id: &RequestId,
        create: &CreateThread,
    ) -> Result<artisan_database::CreateThreadResult, ProtocolFailure> {
        if let Some(replay) = self
            .repository
            .lookup_create_thread(&create.request_id, &create.project_id, &create.title)
            .await
            .map_err(|error| repository_failure(&error, request_id))?
        {
            return Ok(replay);
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
        self.repository
            .create_thread(CreateThreadInput {
                request_id: create.request_id.clone(),
                thread_id,
                project_id: create.project_id.clone(),
                title: create.title.clone(),
                created_at: accepted_at,
                updated_at: accepted_at,
            })
            .await
            .map_err(|error| repository_failure(&error, request_id))
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

    /// Answers one catalog-backed favorite mutation.
    ///
    /// Durable receipt lookup runs before catalog discovery and before the
    /// acceptance clock. Exact repository replays therefore succeed while a
    /// provider/profile service is unavailable and do not consult the Forge
    /// admission origin. A lookup miss performs scoped catalog admission first;
    /// only an admitted mutation consumes one acceptance instant. The
    /// repository remains authoritative for the immediate favorite-state
    /// transaction and its race-safe receipt replay.
    async fn set_model_favorite_outcome(
        &self,
        request_id: &RequestId,
        favorite: &SetModelFavorite,
    ) -> Result<ServerResponse, ProtocolFailure> {
        if let Some(replay) = self
            .repository
            .lookup_set_model_favorite(
                favorite.request_id(),
                favorite.model_id(),
                favorite.favorite(),
            )
            .await
            .map_err(|error| {
                crate::composer_catalog_handler::protocol_failure(
                    crate::composer_catalog_handler::favorites_error(&error),
                    request_id,
                )
            })?
        {
            return Ok(crate::composer_catalog_handler::favorite_response(
                request_id, favorite, &replay,
            ));
        }

        crate::composer_catalog_handler::prepare_model_favorite(
            self.composer_catalog.as_ref(),
            &self.repository,
            favorite,
        )
        .await
        .map_err(|error| crate::composer_catalog_handler::protocol_failure(error, request_id))?;
        let accepted_at = self
            .origin
            .acceptance_instant()
            .map_err(|error| origin_clock_failure(error, request_id))?;
        crate::composer_catalog_handler::persist_model_favorite(
            &self.repository,
            favorite,
            accepted_at,
            request_id,
        )
        .await
    }
}

#[path = "request_handler/live_run.rs"]
mod live_run;

#[path = "request_handler/composer_drafts.rs"]
mod composer_drafts;

#[path = "request_handler/failed_messages.rs"]
mod failed_messages;

#[path = "request_handler/draft_submission.rs"]
mod draft_submission;

#[path = "request_handler/engine_config.rs"]
mod engine_config;

#[path = "request_handler/model_selection.rs"]
mod model_selection;

#[path = "request_handler/user_preferences.rs"]
mod user_preferences;

#[path = "request_handler/attach_project.rs"]
mod attach_project;

#[path = "request_handler/queries.rs"]
mod queries;

#[path = "request_handler/failures.rs"]
mod failures;

#[path = "composer_state_handler.rs"]
mod composer_state_handler;

#[cfg(test)]
#[path = "../../../tests/backend/composer_state_handler.rs"]
mod composer_state_handler_tests;

#[cfg(test)]
#[path = "../../../tests/backend/failed_messages.rs"]
mod failed_messages_tests;
