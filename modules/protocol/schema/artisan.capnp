# Artisan application protocol: Phase 2 wire contract.
#
# This schema defines wire shape only. Production code crosses the generated
# binding boundary through total owned conversions in
# `modules/protocol/src/codec.rs`.
#
# Evolution policy
# ----------------
# * The schema ID below pins this wire contract. It must never change while
#   any peer can still send a frame described by it.
# * Field, union-member, and enumerator ordinals are frozen once committed.
#   Never renumber, reorder, or reuse an ordinal; grow every struct, union,
#   and enum by appending new members with fresh ordinals.
# * Removing or re-typing a field requires a new application protocol version
#   negotiated through Hello / Welcome.
#
# Versioning and correlation semantics
# ------------------------------------
# * The initially (and currently only) supported application protocol version
#   is 1. A client hello offers its supported versions in
#   Hello.supportedVersions and stamps its preferred version on
#   Envelope.protocolVersion; a server either answers with welcome selecting
#   exactly one offered version or rejects with a protocolError whose code is
#   ErrorCode.unsupportedVersion. Every frame after welcome stamps the
#   negotiated version on Envelope.protocolVersion. Version and revision
#   fields stay integers.
# * Envelope.messageId is the protocol-owned FrameId minted by whichever side
#   sends the frame. Client request frames carry one client-minted FrameId
#   which also serves as the domain RequestId and remains stable across
#   retries of durable or idempotent requests: a retrying client resends the
#   same envelope verbatim and never mints a second id for the same logical
#   attempt. The host-interaction pickDirectory request is the one documented
#   exception to that verbatim-retry guarantee (see Request.pickDirectory
#   below).
#   Server frames (welcome, response, event, protocol error) carry
#   independently server-minted FrameIds. Forge mints durable queued-message
#   identities separately (see FirstMessageReceipt and FirstMessageQueued).
#   These vocabularies never alias.
# * Response.requestId, FirstMessageReceipt.requestId,
#   FirstMessageQueued.requestId, and correlated ProtocolError arms echo the
#   client RequestId -- that is, the triggering request's Envelope.messageId
#   -- and must never be conflated with the responding server frame's own
#   FrameId. Uncorrelated errors (for example version rejection before any
#   request) use the uncorrelated arm.
#
# Authentication posture
# ----------------------
# * Editor-to-Forge: Hello.capability carries high-entropy local client
#   authentication material. It reaches the editor out-of-band through the
#   restricted parent-child handoff used to launch its local Forge process,
#   then travels only in Hello. It is secret: never displayed, logged,
#   formatted, or embedded in diagnostics. It proves the editor was the
#   intended launcher of this Forge instance.
# * Both credential vocabularies are SINGLE-USE: an initial capability
#   authenticates exactly one brand-new session and a rotated reconnect
#   capability authenticates exactly one resumption. Every successful
#   Welcome therefore hands back the next rotated reconnect credential for
#   the following connection. Consuming, binding credentials to sessions,
#   and rejecting replay belong to session enforcement in Phase 3; this
#   schema fixes only the wire shapes.
# * Forge-to-editor: the transport layer authenticates the server through
#   certificate fingerprint pinning established during local bootstrap. That
#   pinning lives below this schema; no application frame repeats it.
#
# Units and bounds
# ----------------
# * Timestamps are signed Unix epoch milliseconds (UTC) at the wire boundary.
# * Version and revision values are unsigned integers.
# * Every Text bound below is measured in UTF-8 bytes (not characters or
#   UTF-16 code units) and is enforced by owned conversion code in a later
#   packet; the wire shape itself stays finite and explicit.
# * Identifier rule (domain-owned vocabulary): nonblank, containing no
#   Unicode whitespace or control characters, at most 128 UTF-8 bytes.
# * Protocol-owned boundary and security metadata: error detail at most 1024
#   UTF-8 bytes, hello version list at most 8 entries, capability exactly 32
#   bytes. Domain business bounds (identifiers, names, paths, titles,
#   bodies) stay shared with artisan-domain.

@0xe149e88b3badbc60;
using ComposerState = import "composer_state.capnp";

# ---------------------------------------------------------------------------
# Shared vocabulary
# ---------------------------------------------------------------------------

# How Forge's durable queue received a retried-safe submission.
enum ReceiptDisposition {
  # First acceptance of this exact request id.
  accepted @0;
  # This request id was already accepted; the original outcome stands.
  duplicate @1;
}

# Delivery state of a durably queued first message.
#
# Intentionally a single value for this protocol revision: execution,
# dispatch, engines, and providers are outside the Phase 2 workflow. Later
# states may only be appended.
enum QueuedState {
  # Durably persisted in Forge's queue; nothing has run yet.
  queued @0;
}

# Kind of a listed Forge-visible directory entry.
enum DirectoryEntryKind {
  # A fixed browsing root offered by Forge (home, standard folders).
  root @0;
  # A browsable subdirectory beneath some root.
  directory @1;
}

# Standard user folder shortcuts offered beside every directory listing.
enum PlaceKind {
  home @0;
  desktop @1;
  documents @2;
  downloads @3;
  music @4;
  pictures @5;
  videos @6;
}

# One standard-folder shortcut in a listing.
struct Place {
  kind @0 :PlaceKind;

  # Opaque Forge-minted directory id for browsing into this place.
  # Identifier rule.
  directoryId @1 :Text;

  # Human-readable folder name. At most 256 UTF-8 bytes, nonblank.
  displayName @2 :Text;
}

# Why Forge rejected or failed to satisfy a frame.
#
# Enumerators are appended only; readers that meet an unknown value surface a
# typed decode failure rather than guessing.
enum ErrorCode {
  # The peer offered/stamped a version we cannot speak.
  unsupportedVersion @0;
  # A field violated its documented bound, charset, or required presence.
  invalidInput @1;
  # An opaque directory id is unknown, stale, or expired on this host.
  directoryUnknown @2;
  # No attached project matches the referenced project id.
  projectUnknown @3;
  # No thread matches the referenced thread id.
  threadUnknown @4;
  # Forge-side failure (storage, listing); retry may succeed later.
  internal @5;
  # The client's stable request identity was already accepted for a different
  # command kind or immutable payload. The originally accepted outcome stands,
  # and repeating this conflicting request is never retryable. Appended after
  # the six-code contract was committed; fresh ordinal, existing ordinals
  # frozen.
  idempotencyConflict @6;
  unsupportedFeature @7;
  lifecycleBusy @8;
  engineConfigConflict @9;
}

# One Forge-visible directory in a listing.
struct DirectoryEntry {
  # Opaque Forge-minted identity. Host paths never cross this boundary.
  # Identifier rule.
  directoryId @0 :Text;

  # Human-readable folder name. At most 256 UTF-8 bytes, nonblank.
  displayName @1 :Text;

  kind @2 :DirectoryEntryKind;

  # True when Forge can list children beneath this entry.
  hasChildren @3 :Bool;
}

# Answer to a directory listing request. Losslessly mirrors the domain
# listing value: optional parent plus bounded place and entry lists.
struct DirectoryListing {
  # The directory whose children are listed; absent for root listings.
  parent :union {
    noParent @0 :Void;
    parent @1 :Text;
  }

  # Standard-folder shortcuts, always present so navigation never depends on
  # which level is open. Bounded to at most 16 places by owned conversion.
  places @2 :List(Place);

  # Child directories. Bounded to at most 256 entries by owned conversion.
  entries @3 :List(DirectoryEntry);
}

# An attached project as Forge sees it.
struct Project {
  # Opaque Forge-minted identity. Identifier rule.
  projectId @0 :Text;

  # Display name derived from the attached folder.
  # At most 256 UTF-8 bytes, nonblank.
  displayName @1 :Text;

  # Absolute host path of the attached project root, as resolved by Forge.
  # At most 32768 UTF-8 bytes, nonblank.
  rootPath @2 :Text;

  # When the attachment became durable. Signed Unix milliseconds.
  attachedAtMillis @3 :Int64;
}

# Result of an idempotent attach mutation. The outer Response.requestId
# carries correlation; this payload carries the durable outcome and whether
# Forge accepted it now or replayed the original result.
struct AttachProjectResult {
  project @0 :Project;
  disposition @1 :ReceiptDisposition;
}

# Summary row for one project-scoped thread.
struct ThreadSummary {
  # Opaque Forge-minted identity. Identifier rule.
  threadId @0 :Text;

  # Owning project id. Identifier rule.
  projectId @1 :Text;

  # Thread title. At most 256 UTF-8 bytes after trim validation.
  title @2 :Text;

  # Creation time. Signed Unix milliseconds.
  createdAtMillis @3 :Int64;

  # Last observed activity time. Signed Unix milliseconds.
  updatedAtMillis @4 :Int64;
}

# Answer to a project thread listing request.
struct ThreadList {
  # Bounded to at most 256 summaries by owned conversion (the legacy list was
  # unbounded; this cap is a deliberate, documented improvement).
  threads @0 :List(ThreadSummary);
}

# Answer to an attached-project listing request. Losslessly mirrors the
# domain's bounded `ProjectListing`: complete `Project` rows -- the exact row
# shape carried by AttachProjectResult and the projectAttached event -- in
# Forge-supplied order. Bounded to at most 256 rows by owned conversion (the
# legacy catalog array was unbounded; this cap is a deliberate, documented
# improvement).
struct ProjectList {
  projects @0 :List(Project);
}

# Result of an idempotent create-thread mutation. The outer
# Response.requestId carries correlation; the thread is always the original
# durable thread for duplicate replay.
struct CreateProjectThreadResult {
  thread @0 :ThreadSummary;
  disposition @1 :ReceiptDisposition;
}

# Receipt for a durably queued first message.
struct FirstMessageReceipt {
  # Stable client RequestId echoed from the triggering request. Distinct
  # from the server frame's own FrameId and from messageId below.
  requestId @0 :Text;

  # Forge-minted durable identity of the queued message. Distinct from the
  # client's request id. Identifier rule.
  messageId @1 :Text;

  # The thread that owns the queued message. Identifier rule.
  threadId @2 :Text;

  disposition @3 :ReceiptDisposition;

  state @4 :QueuedState;
}

# Event payload announcing a queued first message.
struct FirstMessageQueued {
  # Stable client RequestId echoed from the accepted request. Distinct from
  # the event frame's own FrameId and from messageId below.
  requestId @0 :Text;

  # Forge-minted durable identity of the queued message. Identifier rule.
  messageId @1 :Text;

  # The thread that owns the newly queued message. Identifier rule.
  threadId @2 :Text;

  # Message body carried losslessly from the accepted submission so the
  # event is never a thinner projection of the receipt.
  # At most 65536 UTF-8 bytes, nonblank.
  body @3 :Text;
}

# ---------------------------------------------------------------------------
# Handshake
# ---------------------------------------------------------------------------

# Client offer: the application protocol versions it can speak plus the
# single-use credential proving it may open (or resume) this Forge session.
struct Hello {
  # Offered versions, ascending, unique, each >= 1. At most 8 entries.
  # Revision 1 supports exactly [1].
  supportedVersions @0 :List(UInt32);

  credential :union {
    # First contact: exactly 32 secret bytes received out-of-band via the
    # restricted parent-child launcher handoff.
    initial @1 :Data;

    # Reconnection: exactly 32 secret bytes rotated by the previous
    # successful Welcome. A second presentation fails once session
    # enforcement lands in Phase 3.
    reconnect @2 :Data;
  }

  # Optional feature offer. An absent field decodes as false, so peers that
  # predate lifecycle control remain compatible. A client must not send the
  # Request.lifecycleControl arm unless the Welcome negotiated support; the
  # transport and backend enforce that authorization in later packets.
  supportsLifecycleControl @3 :Bool;
}

# Server answer: the single negotiated application protocol version plus the
# next rotated reconnect credential.
struct Welcome {
  # Exactly one of the versions offered by the triggering hello.
  negotiatedVersion @0 :UInt32;

  # Opaque connection-scoped id for diagnostics. Identifier rule.
  connectionId @1 :Text;

  # Rotated single-use reconnect credential for resuming a later session:
  # exactly 32 secret bytes, replacing whatever credential authenticated the
  # triggering hello. Every successful Welcome carries one. Never displayed,
  # logged, or formatted. Owned conversion enforces the exact byte length;
  # the Data wire type alone does not. Appended at a fresh ordinal so existing
  # readers see empty bytes and reject them at the owned boundary.
  reconnectCapability @2 :Data;

  # Optional feature acceptance. An absent field decodes as false, so peers
  # that predate lifecycle control remain compatible. Only a true negotiated
  # value authorizes a client to send Request.lifecycleControl; enforcement is
  # outside this wire-only packet.
  lifecycleControlSupported @3 :Bool;
}

# ---------------------------------------------------------------------------
# Negotiated Forge lifecycle control
# ---------------------------------------------------------------------------

# Empty status request. Lifecycle control is available only after the hello /
# welcome feature negotiation above; no request id is nested here because the
# enclosing Envelope.messageId supplies the request correlation.
struct LifecycleStatusRequest {}

# Request to stop lifecycle work. `requireIdle` is the only peer-controlled
# option; transport and backend policy remain outside this wire-only packet.
struct LifecycleStopRequest {
  requireIdle @0 :Bool;
}

# Native lifecycle control request selected by Request.lifecycleControl.
struct LifecycleRequest {
  union {
    status @0 :LifecycleStatusRequest;
    stop @1 :LifecycleStopRequest;
  }
}

# Coarse Forge lifecycle state reported by status and stop receipts.
enum LifecycleState {
  ready @0;
  busy @1;
  draining @2;
}

# Current lifecycle state and bounded active-work count.
struct LifecycleStatus {
  state @0 :LifecycleState;
  activeWorkCount @1 :UInt32;
}

# Result of a lifecycle stop request.
enum LifecycleStopDisposition {
  accepted @0;
  duplicate @1;
  alreadyStopping @2;
}

# Lifecycle stop result. The state is reported independently of disposition.
struct LifecycleStopReceipt {
  disposition @0 :LifecycleStopDisposition;
  state @1 :LifecycleState;
}

# Native lifecycle control response selected by Response.lifecycleControl.
struct LifecycleResponse {
  union {
    status @0 :LifecycleStatus;
    stop @1 :LifecycleStopReceipt;
  }
}

# ---------------------------------------------------------------------------
# Requests. Existing identities are referenced by their opaque ids (projects
# and threads already known to Forge); Forge mints only NEW identities: the
# project id when a directory is attached, the thread id when a thread is
# created, and the queued-message id when a first message is durably queued.
# Mutation retries resend the request frame verbatim, reusing the client
# FrameId that doubles as its stable RequestId. Only durable and idempotent
# requests carry that replay guarantee: the host-interaction pickDirectory
# request below persists nothing and must not be automatically replayed --
# each deliberate new attempt sends a fresh frame identity.
# ---------------------------------------------------------------------------

struct ListDirectoriesRequest {
  # Where to list from: Forge-visible roots, or the children of one opaque
  # directory id from a prior listing. Identifier rule applies when set.
  scope :union {
    noParent @0 :Void;
    parent @1 :Text;
  }
}

struct AttachProjectRequest {
  # The opaque directory id being attached (from a prior listing). Forge
  # mints or resolves the stable project id itself. Identifier rule.
  directoryId @0 :Text;
}

struct ListProjectThreadsRequest {
  # The attached project whose threads are listed. Identifier rule.
  projectId @0 :Text;
}

struct CreateProjectThreadRequest {
  # The attached project that will own the new thread. Identifier rule.
  projectId @0 :Text;

  # Initial title. At most 256 UTF-8 bytes after trim validation.
  title @1 :Text;
}

struct QueueFirstMessageRequest {
  # The (already created) thread receiving its first message.
  # Identifier rule.
  threadId @0 :Text;

  # Message body. At most 65536 UTF-8 bytes.
  body @1 :Text;
}

# Rediscovery read: lists every currently attached project. Carries no
# identity at all -- a returning client asks once and Forge answers with the
# complete catalog, so stale or unknown ids can never fail this request.
struct ListAttachedProjectsRequest {}

struct SetThreadEngineConfigRequest {
  threadId @0 :Text;
  precondition @1 :EngineConfigPrecondition;
  config @2 :EngineRunConfig;
}

struct EngineConfigPrecondition {
  kind @0 :Text;
  revision @1 :UInt64;
}

struct EngineRunConfig {
  schemaVersion @0 :UInt16;
  engine @1 :Text;
  profileId @2 :Text;
  modelId @3 :Text;
  routeId @4 :Text;
  variant @5 :EngineVariant;
  permission @6 :EnginePermissionPolicy;
  runtime @7 :EngineRuntimeControls;
  # Versioned per-engine selection appended without touching @0-@7.
  # schemaVersion 1 carries `unset` and keeps the legacy OpenCode2 shape
  # above as the authority. schemaVersion 2 carries one per-engine arm
  # matching `engine` as the authority; the legacy fields above stay
  # populated as diagnostic mirrors (profileId must equal the arm's
  # profileId) so older readers still reject the frame as an unsupported
  # engine instead of misreading it.
  selectionV2 @8 :EngineSelectionV2;
}

struct EngineVariant {
  kind @0 :Text;
  id @1 :Text;
}

struct EnginePermissionPolicy {
  permissionId @0 :Text;
  agentId @1 :Text;
  approval @2 :Text;
  filesystem @3 :Text;
  network @4 :Text;
  webSearch @5 :Text;
}

struct EngineRuntimeControls {
  attemptBudgetMs @0 :UInt64;
  readinessBudgetMs @1 :UInt64;
  healthBudgetMs @2 :UInt64;
  promptBudgetMs @3 :UInt64;
  streamBudgetMs @4 :UInt64;
  closeBudgetMs @5 :UInt64;
  maxJsonBodyBytes @6 :UInt64;
  maxSseLineBytes @7 :UInt64;
  maxSseEventBytes @8 :UInt64;
  maxReadinessLineBytes @9 :UInt64;
  maxHeaderCount @10 :UInt64;
  maxHttpBufferBytes @11 :UInt64;
  maxStderrBytes @12 :UInt64;
  observationCapacity @13 :UInt64;
}

# Tagged per-engine selection for EngineRunConfig.selectionV2. Each arm
# carries the engine's explicit typed settings; there is no generic options
# map. Empty text (and zero modelContextWindow) means the optional setting
# is absent, mirroring the EngineVariant kind/id convention.
struct EngineSelectionV2 {
  union {
    unset @0 :Void;
    codex @1 :CodexEngineSelection;
    claude @2 :ClaudeEngineSelection;
    grok @3 :GrokEngineSelection;
    cursor @4 :CursorEngineSelection;
  }
}

struct CodexEngineSelection {
  profileId @0 :Text;
  modelId @1 :Text;
  reasoningEffort @2 :Text;
  serviceTier @3 :Text;
  modelContextWindow @4 :UInt64;
  permission @5 :EnginePermissionPolicy;
}

struct ClaudeEngineSelection {
  profileId @0 :Text;
  modelId @1 :Text;
  effort @2 :Text;
  permissionMode @3 :Text;
  disableTools @4 :Bool;
  safeMode @5 :Bool;
  permission @6 :EnginePermissionPolicy;
}

struct GrokEngineSelection {
  profileId @0 :Text;
  modelId @1 :Text;
  reasoningEffort @2 :Text;
  permissionMode @3 :Text;
  permission @4 :EnginePermissionPolicy;
}

struct CursorEngineSelection {
  profileId @0 :Text;
  modelId @1 :Text;
  reasoningEffort @2 :Text;
  speed @3 :Text;
  permissionMode @4 :Text;
  permission @5 :EnginePermissionPolicy;
}

struct SetThreadEngineConfigResult {
  requestId @0 :Text;
  threadId @1 :Text;
  revision @2 :UInt64;
  disposition @3 :ReceiptDisposition;
}

struct ReadThreadEngineSettingsRequest {
  threadId @0 :Text;
}

struct ThreadEngineSettingsResult {
  threadId @0 :Text;
  state :union {
    unconfigured @1 :Void;
    configured @2 :ConfiguredThreadEngineSettings;
  }
}

struct ConfiguredThreadEngineSettings {
  revision @0 :UInt64;
  config @1 :EngineRunConfig;
}

struct ListRegisteredEngineProfilesRequest {}

struct RegisteredEngineProfilesResult {
  state :union {
    registryMissing @0 :Void;
    registryPresent @1 :RegisteredEngineProfileList;
  }
}

struct RegisteredEngineProfileList {
  profileIds @0 :List(Text);
}

# Runtime catalog read scoped to the authenticated thread and native engine
# profile. The backend resolves the catalog through its certified native
# engine session; this request does not start a prompt or assistant run.
struct ReadComposerCatalogRequest {
  threadId @0 :Text;
  profileId @1 :Text;
}

# Durable favorite mutation scoped to the catalog revision observed by the
# caller. The enclosing Envelope.messageId supplies the request correlation.
struct SetModelFavoriteRequest {
  threadId @0 :Text;
  profileId @1 :Text;
  catalogRevision @2 :Text;
  modelId @3 :Text;
  favorite @4 :Bool;
}

# Exact shared catalog wire bytes. Owned conversion enforces the 15 MiB
# SnapshotData ceiling before decoding and preserves every ordered model,
# route, variant, and capability field without truncation.
struct ComposerCatalogResult {
  threadId @0 :Text;
  profileId @1 :Text;
  snapshotData @2 :Data;
}

# Ordered durable favorites snapshot. The owned conversion checks the list
# length before allocation, then validates revision and every model id through
# the domain snapshot constructors.
struct ModelFavoritesSnapshot {
  revision @0 :UInt64;
  modelIds @1 :List(Text);
}

# Correlated favorite mutation receipt. The nested request id must equal the
# enclosing Response.requestId exactly; the complete post-mutation snapshot
# is retained so a client never has to infer state from a boolean alone.
struct SetModelFavoriteReceipt {
  requestId @0 :Text;
  modelId @1 :Text;
  favorite @2 :Bool;
  disposition @3 :ReceiptDisposition;
  snapshot @4 :ModelFavoritesSnapshot;
}

# One bounded rich-link metadata read for an absolute HTTP(S) URL.
#
# The URL is untrusted assistant-authored text. Receivers re-apply the shared
# absolute-HTTP(S) policy, resolve through Forge's bounded outbound fetch, and
# never treat the value as a filesystem path or credential carrier.
struct ResolveRichLinkRequest {
  url @0 :Text;
}

# Resolved rich-link page metadata for one requested URL.
#
# `pageName` already applies the reference fallback chain (`og:title`,
# `twitter:title`, document title, or the canonical host) and is never empty.
# `cacheExpiresAtMs` is the backend cache entry's absolute Unix epoch
# millisecond expiry so clients can bound their own retained titles without
# inventing a second freshness policy. The requested URL is echoed so a client
# can prove the response answers the exact URL it asked for.
struct RichLinkPageMetadata {
  requestedUrl @0 :Text;
  pageName @1 :Text;
  cacheExpiresAtMs @2 :Int64;
}

# The hosting service identified from a Git remote URL. Detection is
# structural, so a self-hosted GitLab or Gitea is recognised the same way the
# public services are.
enum RepositoryHost {
  azure @0;
  bitbucket @1;
  codeberg @2;
  gitea @3;
  github @4;
  gitlab @5;
  other @6;
  sourcehut @7;
  unknown @8;
}

# The branch HEAD names, with its unborn and detached states preserved.
# `name` is empty exactly for the detached state.
enum RepositoryBranchKind {
  attached @0;
  detached @1;
  unborn @2;
}

struct RepositoryBranch {
  kind @0 :RepositoryBranchKind;
  name @1 :Text;
}

# One configured remote projected for presentation. `url` is the remote
# exactly as Git reports it; `webUrl` is empty when the URL does not resolve to
# an https page a browser can open. Receivers re-apply the bounded text rules.
struct RepositoryRemote {
  host @0 :RepositoryHost;
  name @1 :Text;
  url @2 :Text;
  webUrl @3 :Text;
}

# One repository's identity: where HEAD sits and where it publishes. `remotes`
# is bounded to 64; `defaultRemote` is empty exactly when `remotes` is empty,
# and otherwise names one configured remote.
struct RepositorySnapshot {
  branch @0 :RepositoryBranch;
  defaultRemote @1 :Text;
  remotes @2 :List(RepositoryRemote);
}

# Every observation of one project's repository state. `state` distinguishes a
# directory Git does not track from a repository identity; `snapshot` is
# meaningful only in the repository state.
enum ProjectRepositoryState {
  notRepository @0;
  repository @1;
}

struct ProjectRepository {
  state @0 :ProjectRepositoryState;
  snapshot @1 :RepositorySnapshot;
}

# One project paired with the repository observed at its root.
struct ProjectRepositoryEntry {
  projectId @0 :Text;
  repository @1 :ProjectRepository;
}

# Bounded repository-identity read for named attached projects. An empty
# `projectIds` asks for every project in the catalog.
struct ProjectRepositoryQuery {
  projectIds @0 :List(Text);
}

# Repository state per requested project, bounded to 128 entries. A root that
# has moved or lost its repository reports `notRepository` rather than failing
# the whole query.
struct ProjectRepositoryQueryResult {
  repositories @0 :List(ProjectRepositoryEntry);
}

# The request arms of the native protocol: the five original workflow
# requests, project rediscovery, the three conversation read/subscription
# requests, explicit host interaction, lifecycle control, durable engine
# configuration at arm @11, and the authoritative thread engine settings read
# appended at arm @12; existing ordinals remain frozen.
struct Request {
  union {
    listDirectories @0 :ListDirectoriesRequest;
    attachProject @1 :AttachProjectRequest;
    listProjectThreads @2 :ListProjectThreadsRequest;
    createProjectThread @3 :CreateProjectThreadRequest;
    queueFirstMessage @4 :QueueFirstMessageRequest;

    # Appended for durable-project rediscovery after the five-request
    # contract was committed; fresh ordinal, existing ordinals frozen.
    listAttachedProjects @5 :ListAttachedProjectsRequest;

    # Conversation read/subscription arms appended in domain order after the
    # rediscovery request; fresh ordinals, existing ordinals frozen.
    conversationQuery @6 :ConversationQueryRequest;
    conversationSubscribe @7 :ConversationSubscribeRequest;
    conversationUnsubscribe @8 :ConversationUnsubscribeRequest;

    # Explicit user interaction: ask the local Forge process to show its
    # native directory picker once. Deliberately a unit arm outside the pure
    # domain Query and durable Command vocabularies above: nothing durable is
    # created or mutated, the request must not be automatically replayed, and
    # every deliberate new attempt uses a fresh frame identity.
    # Duplicate-request suppression and cancellation propagation stay outside
    # this slice until the separately owned process/admission packet lands.
    # Appended after the conversation requests; fresh ordinal, existing
    # ordinals frozen.
    pickDirectory @9 :Void;

  # Negotiated native lifecycle status/stop control. A client must not send
  # this arm unless the preceding Welcome negotiated support; old peers
  # remain compatible with messages that omit this fresh arm. Authorization
  # is enforced by transport/backend packets, not by this wire-only leaf.
    lifecycleControl @10 :LifecycleRequest;
    setThreadEngineConfig @11 :SetThreadEngineConfigRequest;
    readThreadEngineSettings @12 :ReadThreadEngineSettingsRequest;
    listRegisteredEngineProfiles @13 :ListRegisteredEngineProfilesRequest;
    queueMessage @14 :QueueMessageRequest;
    readMessageImage @15 :ReadMessageImageRequest;
    stopRun @16 :StopRunRequest;
    readActiveRun @17 :ReadActiveRunRequest;
    readComposerCatalog @18 :ReadComposerCatalogRequest;
    readModelFavorites @19 :Void;
    setModelFavorite @20 :SetModelFavoriteRequest;
    listQueuedMessages @21 :ComposerState.ListQueuedMessagesRequest;
    withdrawQueuedMessage @22 :ComposerState.WithdrawQueuedMessageRequest;
    readRecalledMessage @23 :ComposerState.ReadRecalledMessageRequest;
    readRunUsage @24 :ComposerState.ReadRunUsageRequest;

    # Terminally failed-dispatch read. Appended after readAccountUsage;
    # fresh ordinal, existing ordinals frozen.
    listFailedMessages @28 :ComposerState.ListFailedMessagesRequest;

    # Provider-account usage read. The engine scope narrows the read to one
    # engine so clients can fan out per engine; force re-asks providers even
    # when cached reports are fresh. Appended after readRunUsage; fresh
    # ordinal, existing ordinals frozen.
    readAccountUsage @25 :ReadAccountUsageRequest;
    respondApproval @26 :RespondApprovalRequest;
    respondQuestion @27 :RespondQuestionRequest;

    # Bounded rich-link metadata read for one absolute HTTP(S) URL. Appended
    # after respondQuestion; fresh ordinal, existing ordinals frozen.
    resolveRichLink @29 :ResolveRichLinkRequest;

    # Bounded repository-identity read for named attached projects. Appended
    # after resolveRichLink; fresh ordinal, existing ordinals frozen.
    queryProjectRepository @30 :ProjectRepositoryQuery;
  }
}

# ---------------------------------------------------------------------------
# Responses. Every response echoes the triggering envelope message id through
# requestId, which makes retries correlatable without extra state.
# ---------------------------------------------------------------------------

struct Response {
  # Client RequestId echoed from the triggering request's
  # Envelope.messageId. Distinct from this server frame's own FrameId.
  requestId @0 :Text;

  union {
    directoryList @1 :DirectoryListing;
    attachedProject @2 :AttachProjectResult;
    threadList @3 :ThreadList;
    createdThread @4 :CreateProjectThreadResult;
    queuedReceipt @5 :FirstMessageReceipt;

    # Appended for durable-project rediscovery after the five-arm contract
    # was committed; fresh ordinal, existing ordinals frozen.
    projectList @6 :ProjectList;

    # Conversation arms appended after the rediscovery response; fresh
    # ordinals, existing ordinals frozen. conversationSnapshot answers
    # conversationQuery, started/stopped answer subscribe/unsubscribe.
    conversationSnapshot @7 :ConversationSnapshot;
    conversationSubscriptionStarted @8 :ConversationSubscriptionStarted;

    conversationSubscriptionStopped @9 :ConversationSubscriptionStopped;

    # Outcome of one explicit pickDirectory interaction; answers only real
    # picker results. cancelled reports an actual user dismissal of the
    # picker rather than a request cancellation or a dropped frame;
    # cancellation propagation and late-response behavior stay explicitly
    # outside this slice. Appended after the conversation responses; fresh
    # ordinal, existing ordinals frozen.
    directoryPicked @10 :DirectoryPickOutcome;

    # Negotiated native lifecycle status/stop result. Appended at a fresh
    # ordinal; existing response arms remain frozen.
    lifecycleControl @11 :LifecycleResponse;
    threadEngineConfigSet @12 :SetThreadEngineConfigResult;
    threadEngineSettings @13 :ThreadEngineSettingsResult;
    registeredEngineProfiles @14 :RegisteredEngineProfilesResult;
    queuedMessageReceipt @15 :QueueMessageReceipt;
    messageImage @16 :MessageImageResult;
    stopRunReceipt @17 :StopRunReceipt;
    activeRun @18 :ActiveRunResult;
    composerCatalog @19 :ComposerCatalogResult;
    modelFavorites @20 :ModelFavoritesSnapshot;
    modelFavoriteSet @21 :SetModelFavoriteReceipt;
    queuedMessages @22 :ComposerState.QueuedMessageListing;
    messageWithdrawn @23 :ComposerState.QueuedMessageWithdrawalResult;
    recalledMessage @24 :ComposerState.RecalledMessageResult;
    runUsage @25 :ComposerState.RunUsageResult;

    # Provider-account usage snapshot for the requested engines. Appended
    # after runUsage; fresh ordinal, existing ordinals frozen.
    accountUsage @26 :EngineUsageSnapshot;
    approvalResponse @27 :RespondApprovalReceipt;
    questionResponse @28 :RespondQuestionReceipt;

    # Terminally failed-dispatch listing. Appended after the question
    # response; fresh ordinal, existing ordinals frozen.
    failedMessages @29 :ComposerState.FailedMessageListing;

    # Resolved rich-link page metadata for one resolveRichLink request.
    # Appended after failedMessages; fresh ordinal, existing ordinals frozen.
    richLink @30 :RichLinkPageMetadata;

    # Repository state per requested project for one queryProjectRepository
    # request. Appended after richLink; fresh ordinal, existing ordinals frozen.
    projectRepository @31 :ProjectRepositoryQueryResult;
  }
}

# A live exact-run cancellation request. This is not a durable completion
# command: the response reports only registry signal disposition.
struct StopRunRequest {
  threadId @0 :Text;
  runId @1 :Text;
}

# A bounded read of the process-owned live-run registry for one thread.
struct ReadActiveRunRequest {
  threadId @0 :Text;
}

enum StopRunDisposition {
  requested @0;
  alreadyRequested @1;
  notActive @2;
}

struct StopRunReceipt {
  threadId @0 :Text;
  runId @1 :Text;
  disposition @2 :StopRunDisposition;
  requestId @3 :Text;
}

struct ActiveRunResult {
  threadId @0 :Text;
  state :union {
    noActive @1 :Void;
    active @2 :Text;
  }
  # Live lifecycle of the registered run. The current backend always
  # emits queued/running/waiting; `unknown` is a strict decode error,
  # never a tolerated state (native QUIC is a same-version build).
  runStatus @3 :RunStatus;
  # Engine backing the live run. Empty is a strict decode error.
  runEngineId @4 :Text;
}

enum RunStatus {
  unknown @0;
  queued @1;
  running @2;
  waiting @3;
}

# Outcome of one explicit native directory-pick interaction.
#
# selected carries only the validated opaque DirectoryId of the chosen
# directory under the shared identifier rule -- never a filesystem path,
# display label, directory enumeration, or has-children projection.
# cancelled reports an actual user dismissal of the picker. Every deliberate
# new pick attempt is a fresh request frame with its own identity (see
# Request.pickDirectory).
struct DirectoryPickOutcome {
  union {
    # Opaque Forge-minted directory identity. Identifier rule.
    selected @0 :Text;

    # The user dismissed the picker.
    cancelled @1 :Void;
  }
}

# ---------------------------------------------------------------------------
# Events. Forge-originated notifications about the first workflow; they carry
# no engine, provider, journal, or replay machinery.
# ---------------------------------------------------------------------------

struct Event {
  union {
    # A project was attached (possibly by another session).
    projectAttached @0 :Project;

    # A thread was created inside an attached project.
    threadCreated @1 :ThreadSummary;

    # A first message was durably queued on a thread.
    firstMessageQueued @2 :FirstMessageQueued;

    # Finite engine-observation delivery (S1b). One durably committed,
    # sanitized observation row for its thread subscribers, published in
    # durable sequence order. Fresh union member at @4: @3 is already the
    # cursor field below, @0-@2 are the frozen first-workflow arms. Old
    # readers that predate this member observe an unknown discriminant and
    # surface a typed decode failure; old writers never set it, so their
    # frames decode unchanged.
    engineObservation @4 :EngineObservationEvent;
  }

  # One-based per-session event cursor. Starts at 1 on a session's first
  # event and increments contiguously with every subsequent Event frame;
  # zero is never sent by a conforming peer. A client observing a gap,
  # duplicate, or regression must resnapshot rather than apply; sequencing
  # enforcement belongs to session machinery in Phase 3. Appended outside
  # the existing union after the three-arm contract was committed; fresh
  # ordinal, existing ordinals frozen. Readers of older frames observe the
  # zero default and owned conversion rejects it.
  #
  # This cursor counts events only; conversation replay ordering uses
  # PatchBatch and ConversationSnapshot cursors below.
  cursor @3 :UInt64;
}

# ---------------------------------------------------------------------------
# Engine observation delivery (S1b). Sanitized, provider-neutral rows
# committed durably by S1a checkpoints and published to thread subscribers.
#
# Additive only: every struct and enum below is new. Existing field
# types/ordinals are untouched. Every Text bound is measured in UTF-8 bytes
# and enforced by the owned codec; an empty Text decodes as absent unless
# the arm documents otherwise. Optional counts where zero is a measured
# value (token counts, result counts, line counts, durations, exit codes,
# costs, decisions, answers, output chunks, scopes) use explicit presence
# unions so `Some(0)` never collapses into `None`.
# ---------------------------------------------------------------------------

# Renderer-disclosed display phase of one agent-authored message.
enum ObservationMessagePhase {
  unspecified @0;
  commentary @1;
  final @2;
}

# Lifecycle action of one tool invocation.
enum ObservationToolAction {
  started @0;
  progress @1;
  completed @2;
  failed @3;
}

# File action of one file observation.
enum ObservationFileAction {
  created @0;
  modified @1;
  deleted @2;
  read @3;
}

# Scope of one search observation.
enum ObservationSearchScope {
  workspace @0;
  web @1;
}

# Lifecycle state of one search observation.
enum ObservationSearchState {
  started @0;
  completed @1;
}

# Output channel of one terminal activity observation.
enum ObservationTerminalChannel {
  stdout @0;
  stderr @1;
}

# Lifecycle state of one terminal activity observation.
enum ObservationTerminalState {
  started @0;
  output @1;
  completed @2;
  failed @3;
}

# Lifecycle state of one approval observation.
enum ObservationApprovalState {
  requested @0;
  resolved @1;
}

# Kind of action bound to one approval request.
enum ObservationApprovalKind {
  command @0;
  fileChange @1;
  action @2;
}

# Lifecycle state of one question observation.
enum ObservationQuestionState {
  requested @0;
  resolved @1;
}

# Status of one plan entry.
enum ObservationPlanEntryStatus {
  pending @0;
  inProgress @1;
  completed @2;
}

# Lifecycle state of one compaction observation.
enum ObservationCompactionState {
  started @0;
  completed @1;
}

# Provider attempt state of one retry observation.
enum ObservationRetryAttemptState {
  retrying @0;
  terminal @1;
}

# Non-terminal lifecycle state of one run.
enum ObservationRunState {
  opening @0;
  running @1;
  waiting @2;
}

# Lifecycle state of one provider turn.
enum ObservationTurnState {
  started @0;
  waiting @1;
  completed @2;
  cancelled @3;
  failed @4;
}

# Lifecycle state of one provider-native subagent.
enum ObservationSubagentState {
  discovered @0;
  running @1;
  waiting @2;
  completed @3;
  failed @4;
  interrupted @5;
}

# Provider usage accounting basis, preserved verbatim.
enum ObservationUsageBasis {
  delta @0;
  cumulative @1;
  unknown @2;
}

# Severity of one process or protocol diagnostic.
enum ObservationDiagnosticLevel {
  info @0;
  warning @1;
  error @2;
}

# The only outcomes that can complete an engine run.
enum ObservationRunTerminalState {
  completed @0;
  cancelled @1;
  failed @2;
  interrupted @3;
  closed @4;
}

# Scope of a depleted provider allowance, when disclosed.
enum ObservationLimitScope {
  shared @0;
  model @1;
  unknown @2;
}

# One streamed fragment of an agent-authored message.
struct ObservationAgentMessageDelta {
  id @0 :Text;
  sequence @1 :UInt64;
  itemId @2 :Text;
  phase @3 :ObservationMessagePhase;
  delta @4 :Text;
  turnId @5 :Text;
}

# One completed agent-authored message.
struct ObservationAgentMessageCompleted {
  id @0 :Text;
  sequence @1 :UInt64;
  itemId @2 :Text;
  phase @3 :ObservationMessagePhase;
  message @4 :Text;
  turnId @5 :Text;
}

# One streamed fragment of a provider-authored reasoning summary.
struct ObservationReasoningSummaryDelta {
  id @0 :Text;
  sequence @1 :UInt64;
  itemId @2 :Text;
  summaryIndex @3 :UInt64;
  delta @4 :Text;
  thinkingTokens :union {
    noThinkingTokens @5 :Void;
    thinkingTokens @6 :UInt64;
  }
  turnId @7 :Text;
}

# One settled reasoning phase for a turn.
struct ObservationReasoningSummaryCompleted {
  id @0 :Text;
  sequence @1 :UInt64;
  itemId @2 :Text;
  # Empty decodes as absent: a present text is never empty.
  text @3 :Text;
  turnId @4 :Text;
}

# One tool lifecycle event without provider-specific tool types.
struct ObservationTool {
  id @0 :Text;
  sequence @1 :UInt64;
  toolId @2 :Text;
  toolName @3 :Text;
  action @4 :ObservationToolAction;
  # Empty decodes as absent: a present detail is never empty.
  detail @5 :Text;
}

# One file mutation or inspection performed during a run.
struct ObservationFile {
  id @0 :Text;
  sequence @1 :UInt64;
  path @2 :Text;
  action @3 :ObservationFileAction;
  linesAdded :union {
    noLinesAdded @4 :Void;
    linesAdded @5 :UInt64;
  }
  linesDeleted :union {
    noLinesDeleted @6 :Void;
    linesDeleted @7 :UInt64;
  }
}

# One search operation performed during a run.
struct ObservationSearch {
  id @0 :Text;
  sequence @1 :UInt64;
  query @2 :Text;
  scope :union {
    noScope @3 :Void;
    scope @4 :ObservationSearchScope;
  }
  # Empty decodes as absent: a present search id is never empty.
  searchId @5 :Text;
  state @6 :ObservationSearchState;
  resultCount :union {
    noResultCount @7 :Void;
    resultCount @8 :UInt64;
  }
}

# One shell or process activity row, independent from the run outcome.
struct ObservationTerminalActivity {
  id @0 :Text;
  sequence @1 :UInt64;
  activityId @2 :Text;
  channel :union {
    noChannel @3 :Void;
    channel @4 :ObservationTerminalChannel;
  }
  # Empty decodes as absent: a present command or shell is never empty.
  command @5 :Text;
  shell @6 :Text;
  # An emitted output chunk may itself be empty, so presence is explicit.
  output :union {
    noOutput @7 :Void;
    output @8 :Text;
  }
  # Exit zero is a measured outcome, so presence is explicit.
  exitCode :union {
    noExitCode @9 :Void;
    exitCode @10 :Int32;
  }
  state @11 :ObservationTerminalState;
}

# Provider-neutral action bound to one approval request. `command` and `cwd`
# are populated only for the command kind; other kinds must leave them
# empty and carry only an optional reason.
struct ObservationApprovalRequest {
  kind @0 :ObservationApprovalKind;
  command @1 :Text;
  cwd @2 :Text;
  reason @3 :Text;
}

# One approval request or its resolution. A requested row never carries a
# decision; a resolved row always does.
struct ObservationApproval {
  id @0 :Text;
  sequence @1 :UInt64;
  approvalId @2 :Text;
  state @3 :ObservationApprovalState;
  description @4 :Text;
  request @5 :ObservationApprovalRequest;
  decision :union {
    noDecision @6 :Void;
    decision @7 :Bool;
  }
}

# One provider-offered answer to a question.
struct ObservationQuestionOption {
  label @0 :Text;
  # Empty decodes as absent: a present description is never empty.
  description @1 :Text;
}

# One question request or its resolution. A requested row never carries
# answers; a resolved row always carries the answer list, which may itself
# be empty for an explicitly skipped question.
struct ObservationQuestion {
  id @0 :Text;
  sequence @1 :UInt64;
  questionId @2 :Text;
  state @3 :ObservationQuestionState;
  text @4 :Text;
  # Empty decodes as absent: a present header is never empty.
  header @5 :Text;
  multiSelect @6 :Bool;
  # Empty decodes as absent: a present option list is never empty.
  options @7 :List(ObservationQuestionOption);
  answers :union {
    noAnswers @8 :Void;
    answers @9 :List(Text);
  }
}

# One provider-neutral plan entry.
struct ObservationPlanEntry {
  id @0 :Text;
  status @1 :ObservationPlanEntryStatus;
  text @2 :Text;
}

# One provider-neutral plan update.
struct ObservationPlan {
  id @0 :Text;
  sequence @1 :UInt64;
  entries @2 :List(ObservationPlanEntry);
  # Empty decodes as absent: a present turn id is never empty.
  turnId @3 :Text;
}

# One provider context compaction report.
struct ObservationCompaction {
  id @0 :Text;
  sequence @1 :UInt64;
  state @2 :ObservationCompactionState;
  # Empty decodes as absent: a present compaction id is never empty.
  compactionId @3 :Text;
  durationMs :union {
    noDurationMs @4 :Void;
    durationMs @5 :UInt64;
  }
  # Empty decodes as absent: a present summary is never empty.
  summary @6 :Text;
}

# One provider error report with its continuation intent.
struct ObservationRetry {
  id @0 :Text;
  sequence @1 :UInt64;
  turnId @2 :Text;
  attemptState @3 :ObservationRetryAttemptState;
  willRetry @4 :Bool;
  message @5 :Text;
}

# One non-terminal lifecycle change for the run.
struct ObservationRunStateObservation {
  id @0 :Text;
  sequence @1 :UInt64;
  state @2 :ObservationRunState;
}

# One lifecycle progress report for a single provider turn.
struct ObservationTurnStateObservation {
  id @0 :Text;
  sequence @1 :UInt64;
  turnId @2 :Text;
  state @3 :ObservationTurnState;
}

# One provider-native subagent activity report.
struct ObservationSubagent {
  id @0 :Text;
  sequence @1 :UInt64;
  agentNativeThreadId @2 :Text;
  parentNativeThreadId @3 :Text;
  state @4 :ObservationSubagentState;
  # Empty decodes as absent: a present activity or agent path is never empty.
  activity @5 :Text;
  agentPath @6 :Text;
  # Empty decodes as absent: a present turn id is never empty.
  turnId @7 :Text;
}

# Child agent message fragment.
struct ObservationTranscriptAgentMessageDelta {
  itemId @0 :Text;
  phase @1 :ObservationMessagePhase;
  delta @2 :Text;
}

# Child completed agent message.
struct ObservationTranscriptAgentMessageCompleted {
  itemId @0 :Text;
  phase @1 :ObservationMessagePhase;
  message @2 :Text;
}

# Child reasoning summary fragment.
struct ObservationTranscriptReasoningSummaryDelta {
  itemId @0 :Text;
  summaryIndex @1 :UInt64;
  delta @2 :Text;
}

# Child settled reasoning phase.
struct ObservationTranscriptReasoningSummaryCompleted {
  itemId @0 :Text;
  # Empty decodes as absent: a present text is never empty.
  text @1 :Text;
}

# Child terminal activity row.
struct ObservationTranscriptTerminalActivity {
  activityId @0 :Text;
  channel :union {
    noChannel @1 :Void;
    channel @2 :ObservationTerminalChannel;
  }
  # Empty decodes as absent: a present command is never empty.
  command @3 :Text;
  # Exit zero is a measured outcome, so presence is explicit.
  exitCode :union {
    noExitCode @4 :Void;
    exitCode @5 :Int32;
  }
  # An emitted output chunk may itself be empty, so presence is explicit.
  output :union {
    noOutput @6 :Void;
    output @7 :Text;
  }
  state @8 :ObservationTerminalState;
}

# Child tool row.
struct ObservationTranscriptTool {
  toolId @0 :Text;
  toolName @1 :Text;
  action @2 :ObservationToolAction;
  # Empty decodes as absent: a present detail is never empty.
  detail @3 :Text;
}

# Child file row.
struct ObservationTranscriptFile {
  path @0 :Text;
  action @1 :ObservationFileAction;
  linesAdded :union {
    noLinesAdded @2 :Void;
    linesAdded @3 :UInt64;
  }
  linesDeleted :union {
    noLinesDeleted @4 :Void;
    linesDeleted @5 :UInt64;
  }
}

# Child search row.
struct ObservationTranscriptSearch {
  query @0 :Text;
  resultCount :union {
    noResultCount @1 :Void;
    resultCount @2 :UInt64;
  }
  scope :union {
    noScope @3 :Void;
    scope @4 :ObservationSearchScope;
  }
  # Empty decodes as absent: a present search id is never empty.
  searchId @5 :Text;
  state @6 :ObservationSearchState;
}

# Renderer-safe content of one native subagent row. Only the eight
# projectable kinds exist here.
struct ObservationSubagentTranscriptContent {
  union {
    agentMessageDelta @0 :ObservationTranscriptAgentMessageDelta;
    agentMessageCompleted @1 :ObservationTranscriptAgentMessageCompleted;
    reasoningSummaryDelta @2 :ObservationTranscriptReasoningSummaryDelta;
    reasoningSummaryCompleted @3 :ObservationTranscriptReasoningSummaryCompleted;
    terminalActivity @4 :ObservationTranscriptTerminalActivity;
    tool @5 :ObservationTranscriptTool;
    file @6 :ObservationTranscriptFile;
    search @7 :ObservationTranscriptSearch;
  }
}

# One public content row emitted by a native subagent.
struct ObservationSubagentTranscript {
  id @0 :Text;
  sequence @1 :UInt64;
  agentNativeThreadId @2 :Text;
  parentNativeThreadId @3 :Text;
  content @4 :ObservationSubagentTranscriptContent;
}

# One provider failure transferred into Artisan's custody. Everything
# downstream reasons in the `AE-*` vocabulary while the provider's own code
# rides along as evidence. Empty Text fields decode as absent.
struct ObservationEngineErrorRef {
  artisanCode @0 :Text;
  providerCode @1 :Text;
  detail @2 :Text;
  affectedModelId @3 :Text;
  limitId @4 :Text;
  limitLabel @5 :Text;
  limitScope :union {
    noLimitScope @6 :Void;
    limitScope @7 :ObservationLimitScope;
  }
  resetsAt @8 :Text;
}

# One provider usage measurement for the run or one turn. Context tokens
# are a gauge and must never be summed across reports, no matter the basis.
struct ObservationUsage {
  id @0 :Text;
  sequence @1 :UInt64;
  basis @2 :ObservationUsageBasis;
  inputTokens :union {
    noInputTokens @3 :Void;
    inputTokens @4 :UInt64;
  }
  cachedInputTokens :union {
    noCachedInputTokens @5 :Void;
    cachedInputTokens @6 :UInt64;
  }
  outputTokens :union {
    noOutputTokens @7 :Void;
    outputTokens @8 :UInt64;
  }
  contextTokens :union {
    noContextTokens @9 :Void;
    contextTokens @10 :UInt64;
  }
  # Zero decodes as absent: a present window is never zero.
  contextWindowTokens @11 :UInt64;
  # A reported cost may itself be zero, so presence is explicit.
  cost :union {
    noCost @12 :Void;
    cost @13 :Float64;
  }
  # Empty decodes as absent: a present route or turn id is never empty.
  providerRouteId @14 :Text;
  turnId @15 :Text;
}

# One provider-native action with no canonical tool equivalent.
struct ObservationNativeAction {
  id @0 :Text;
  sequence @1 :UInt64;
  action @2 :Text;
  # Empty decodes as absent: a present detail is never empty.
  detail @3 :Text;
  diagnostic @4 :Bool;
  errorRef :union {
    noErrorRef @5 :Void;
    errorRef @6 :ObservationEngineErrorRef;
  }
}

# One process-level diagnostic from the engine host.
struct ObservationProcessDiagnostic {
  id @0 :Text;
  sequence @1 :UInt64;
  level @2 :ObservationDiagnosticLevel;
  message @3 :Text;
  errorRef :union {
    noErrorRef @4 :Void;
    errorRef @5 :ObservationEngineErrorRef;
  }
}

# One decoded transport or protocol diagnostic.
struct ObservationProtocolDiagnostic {
  id @0 :Text;
  sequence @1 :UInt64;
  level @2 :ObservationDiagnosticLevel;
  message @3 :Text;
}

# The sole terminal outcome emitted by a run.
struct ObservationRunTerminal {
  id @0 :Text;
  sequence @1 :UInt64;
  state @2 :ObservationRunTerminalState;
  errorRef :union {
    noErrorRef @3 :Void;
    errorRef @4 :ObservationEngineErrorRef;
  }
  # Empty decodes as absent: a present session title is never empty.
  summaryTitle @5 :Text;
}

# One sanitized engine observation row. Member ordinals follow the domain
# `Observation` variant order and are frozen once committed.
struct EngineObservation {
  union {
    agentMessageDelta @0 :ObservationAgentMessageDelta;
    agentMessageCompleted @1 :ObservationAgentMessageCompleted;
    approval @2 :ObservationApproval;
    compaction @3 :ObservationCompaction;
    file @4 :ObservationFile;
    nativeAction @5 :ObservationNativeAction;
    plan @6 :ObservationPlan;
    processDiagnostic @7 :ObservationProcessDiagnostic;
    protocolDiagnostic @8 :ObservationProtocolDiagnostic;
    question @9 :ObservationQuestion;
    reasoningSummaryCompleted @10 :ObservationReasoningSummaryCompleted;
    reasoningSummaryDelta @11 :ObservationReasoningSummaryDelta;
    retry @12 :ObservationRetry;
    runState @13 :ObservationRunStateObservation;
    runTerminal @14 :ObservationRunTerminal;
    search @15 :ObservationSearch;
    subagent @16 :ObservationSubagent;
    subagentTranscript @17 :ObservationSubagentTranscript;
    terminalActivity @18 :ObservationTerminalActivity;
    tool @19 :ObservationTool;
    turnState @20 :ObservationTurnStateObservation;
    usage @21 :ObservationUsage;
  }
}

# Durable thread-scoped attribution for one committed engine observation.
# `deliverySequence` is the strictly increasing thread-scoped durable cursor
# across runs (one-based, never zero); `Observation.sequence` stays run-local.
# `runId`/`turnId` and `committedAtMillis` are the Forge-persisted launch
# receipt and batch `operated_at` facts, never delivery-stamped clocks.
struct EngineObservationAttribution {
  runId @0 :Text;
  turnId @1 :Text;
  committedAtMillis @2 :Int64;
  deliverySequence @3 :UInt64;
}

# One committed engine observation routed to its thread subscribers.
struct EngineObservationEvent {
  # Thread whose subscribers receive the observation. Identifier rule.
  threadId @0 :Text;
  observation @1 :EngineObservation;
  # Additive optional attribution: absent on pre-attribution frames, which
  # decode as `None`. When present, run/turn ids must parse, the commit time
  # must be positive, and the delivery sequence must be positive.
  attribution :union {
    noAttribution @2 :Void;
    attribution @3 :EngineObservationAttribution;
  }
}

# ---------------------------------------------------------------------------
# Conversation replay values. Renderer-facing durable state mirroring the
# bounded conversation types of `artisan-domain` (`modules/domain/src/
# conversation.rs`). Forge mints every turn, item, and patch identity;
# counters express ordering without conflating identities with positions.
# Total owned conversions in `modules/protocol/src/codec.rs` validate these
# messages before they cross application service boundaries.
#
# Counter conventions shared with the domain: turn/item ordinals and entity
# revisions are zero-based UInt64; patch sequences are one-based UInt64 that
# reject zero; conversation cursors are zero-based UInt64 where zero means
# "before the first patch" (a fresh projection), so cursors -- unlike
# sequences -- may legitimately be zero. Timestamps are signed Unix epoch
# milliseconds (UTC). Every Text bound below is measured in UTF-8 bytes and
# enforced by owned conversion; the wire shapes stay finite and explicit here.
# ---------------------------------------------------------------------------

# Renderer-visible lifecycle shared by conversation turns and items.
#
# Enumerators mirror `ConversationLifecycle` in exact domain order and may
# only be appended; readers that meet an unknown value surface a typed
# decode failure rather than guessing.
enum ConversationLifecycle {
  # Durable entity exists but work has not started.
  pending @0;

  # Text or reasoning is arriving incrementally.
  streaming @1;

  # Work is actively progressing.
  active @2;

  # Work is waiting for input or another dependency.
  waiting @3;

  # Work completed successfully.
  completed @4;

  # Work ended because of a failure.
  failed @5;

  # Work was externally stopped and may be resumed.
  interrupted @6;

  # Work was deliberately cancelled.
  cancelled @7;
}

# One canonical conversation turn: complete value, never a projection.
struct ConversationTurn {
  # Forge-minted turn identity. Identifier rule.
  turnId @0 :Text;

  # Stable zero-based position in the containing conversation. Rejects
  # nothing at the wire layer; owned conversion rejects duplicates.
  ordinal @1 :UInt64;

  # Current zero-based entity revision; newly queued turns start at zero.
  revision @2 :UInt64;

  # Renderer-visible lifecycle.
  lifecycle @3 :ConversationLifecycle;

  # Creation time. Signed Unix milliseconds.
  createdAtMillis @4 :Int64;

  # Last update time. Signed Unix milliseconds.
  updatedAtMillis @5 :Int64;
}

# One durably queued user-message item: complete value, never a projection.
struct UserMessageItem {
  # Forge-minted item identity. Identifier rule.
  itemId @0 :Text;

  # Turn that owns the message. Identifier rule.
  turnId @1 :Text;

  # Stable zero-based position in the containing conversation.
  ordinal @2 :UInt64;

  # Current zero-based entity revision; newly queued items start at zero.
  revision @3 :UInt64;

  # Renderer-visible lifecycle.
  lifecycle @4 :ConversationLifecycle;

  # Complete, bounded body stored durably by Forge. At most 65536 UTF-8
  # bytes, nonblank (shared message bound).
  body @5 :Text;

  # Creation time. Signed Unix milliseconds.
  createdAtMillis @6 :Int64;

  # Last update time. Signed Unix milliseconds.
  updatedAtMillis @7 :Int64;

  # Original queued message identity for truthful receipt echo correlation.
  # Empty on rows written before this field existed; present values
  # validate as message ids. Identifier rule.
  sourceMessageId @8 :Text;
}

# Renderer-disclosed display phase of one assistant message's text.
#
# Mirrors `AssistantMessagePhase` in exact domain order and may only be
# appended; readers that meet an unknown value surface a typed decode
# failure rather than guessing. It classifies only the text a renderer was
# given, never hidden reasoning, and it is independent of the item
# lifecycle: final does not imply completed.
enum AssistantMessagePhase {
  # No phase was disclosed for this text.
  unspecified @0;

  # Progress commentary rather than the settled reply.
  commentary @1;

  # The settled reply text.
  final @2;
}

# One durably stored assistant-output item: complete value, never a
# projection.
struct AssistantMessageItem {
  # Forge-minted item identity. Identifier rule.
  itemId @0 :Text;

  # Turn that owns the item. Identifier rule.
  turnId @1 :Text;

  # Stable zero-based position in the containing conversation.
  ordinal @2 :UInt64;

  # Current zero-based entity revision; newly stored items start at zero.
  revision @3 :UInt64;

  # Renderer-visible lifecycle.
  lifecycle @4 :ConversationLifecycle;

  # Complete bounded assistant text stored durably by Forge. At most 65536
  # UTF-8 bytes (shared message bound); EMPTY IS VALID because a stored
  # assistant row may exist before its first visible token arrived. Owned
  # conversion preserves every accepted byte exactly.
  body @5 :Text;

  # Creation time. Signed Unix milliseconds.
  createdAtMillis @6 :Int64;

  # Last update time. Signed Unix milliseconds.
  updatedAtMillis @7 :Int64;

  # Opaque Forge-minted routing id of the run that produced this output.
  # Identifier rule. Nonsecret evidence of origin only -- never a lease,
  # credential, engine id, or public run-state machine, and never an alias
  # of a message id or frame id.
  runId @8 :Text;

  # Renderer-disclosed text phase.
  phase @9 :AssistantMessagePhase;
}

# Renderer-visible conversation item vocabulary. Appending another kind
# adds one union member; existing members never move.
#
# `userMessage` keeps ordinal @0. `unmodeled` occupied the second slot while
# only one item kind was modeled; it carries no data and is never sent by a
# conforming peer -- owned conversion rejects it in every revision. The
# assistant kind appended below took the next fresh ordinal; there is no
# protocol-version compatibility claim for an older peer decoding it.
struct ConversationItem {
  union {
    # Canonical user input durably queued before any engine dispatch.
    userMessage @0 :UserMessageItem;

    # Placeholder that kept the union compilable while only one item kind
    # was modeled; never produced by any revision, rejected forever.
    unmodeled @1 :Void;

    # Appended assistant output under the run that produced it; fresh
    # ordinal, existing ordinals frozen.
    assistantMessage @2 :AssistantMessageItem;

    # Ordered image-bearing user input. Text-only items keep the legacy
    # userMessage arm; this fresh arm represents absent text without a
    # placeholder body.
    multimodalUserMessage @3 :MultimodalUserMessageItem;
  }
}

# Canonical renderer snapshot at one per-thread replay cursor. The turn
# list is bounded by owned conversion to the shared query ceiling of at
# most 512 turns; items are bounded by the transport frame size and their
# own per-field bounds rather than a separate count cap. Older history
# hydrates through additional range queries instead of unbounded frames.
#
# Structural validity (unique turn ids, unique item ids, globally unique
# ordinals, every item referencing a present turn) belongs to owned
# conversion; this shape deliberately represents invalid combinations so
# malformed peers can be rejected there with typed errors.
struct ConversationSnapshot {
  # Thread this projection belongs to. Identifier rule.
  threadId @0 :Text;

  # Last patch sequence incorporated into this snapshot. Zero means the
  # empty projection before the first patch; a conforming fresh snapshot
  # over a patched thread carries a positive cursor.
  cursor @1 :UInt64;

  # Turns in stable ordinal order. Bounded to at most 512 entries by owned
  # conversion (the shared query-turn ceiling).
  turns @2 :List(ConversationTurn);

  # Items in stable ordinal order, bounded by the transport frame size and
  # their own per-field bounds; structural invariants belong to owned
  # conversion.
  items @3 :List(ConversationItem);

  # Projection update time. Signed Unix milliseconds.
  updatedAtMillis @4 :Int64;
}

# Exact incremental fragment appended to a text-bearing item.
struct ItemAppend {
  # Target item identity. Identifier rule.
  itemId @0 :Text;

  # Revision after applying this append. Zero-based.
  revision @1 :UInt64;

  # Fragment carried verbatim. At most 4096 UTF-8 bytes; EMPTY IS VALID
  # because a stream may open before its first visible token. Owned
  # conversion enforces the byte ceiling.
  text @2 :Text;

  # Authoritative entity update time supplied by Forge. Signed Unix epoch
  # milliseconds; every i64 value including MIN, MAX, negative and zero is
  # legal here. An absent field decodes as exactly 0 -- indistinguishable
  # from a sender-supplied epoch zero, since an Int64 has no presence
  # information. No zero sentinel, clock fallback, or older-peer
  # compatibility claim exists at this boundary.
  updatedAtMillis @3 :Int64;
}

# Lifecycle transition applied to one renderer-visible item.
struct ItemLifecyclePatch {
  # Target item identity. Identifier rule.
  itemId @0 :Text;

  # Revision after applying this transition. Zero-based.
  revision @1 :UInt64;

  # New lifecycle.
  lifecycle @2 :ConversationLifecycle;

  # Authoritative entity update time supplied by Forge. Signed Unix epoch
  # milliseconds; every i64 value including MIN, MAX, negative and zero is
  # legal here. An absent field decodes as exactly 0 -- indistinguishable
  # from a sender-supplied epoch zero, since an Int64 has no presence
  # information. No zero sentinel, clock fallback, or older-peer
  # compatibility claim exists at this boundary.
  updatedAtMillis @3 :Int64;
}

# Lifecycle transition applied to one canonical turn.
struct TurnLifecyclePatch {
  # Target turn identity. Identifier rule.
  turnId @0 :Text;

  # Revision after applying this transition. Zero-based.
  revision @1 :UInt64;

  # New lifecycle.
  lifecycle @2 :ConversationLifecycle;

  # Authoritative entity update time supplied by Forge. Signed Unix epoch
  # milliseconds; every i64 value including MIN, MAX, negative and zero is
  # legal here. An absent field decodes as exactly 0 -- indistinguishable
  # from a sender-supplied epoch zero, since an Int64 has no presence
  # information. No zero sentinel, clock fallback, or older-peer
  # compatibility claim exists at this boundary.
  updatedAtMillis @3 :Int64;
}

# One sequenced mutation against a conversation snapshot.
#
# patchId and sequence are shared by every variant because replay ordering
# is batch-wide. The five variants mirror `ConversationPatch` exactly; all
# carry complete values where the domain does.
struct ConversationPatch {
  # Forge-minted patch identity. Identifier rule.
  patchId @0 :Text;

  # Contiguous one-based replay sequence. Zero is reserved for "before the
  # first patch" cursors only; a conforming patch always carries >= 1.
  sequence @1 :UInt64;

  union {
    # Inserts or replaces one canonical turn with its complete current
    # value.
    turnUpsert @2 :ConversationTurn;

    # Inserts or replaces one renderer-visible item with its complete
    # current value.
    itemUpsert @3 :ConversationItem;

    # Appends a bounded fragment to a text-bearing item.
    itemAppend @4 :ItemAppend;

    # Advances an item's renderer lifecycle.
    itemLifecycle @5 :ItemLifecyclePatch;

    # Advances a turn's renderer lifecycle.
    turnLifecycle @6 :TurnLifecyclePatch;
  }
}

# One non-empty, bounded, contiguous patch replay after a known cursor.
# Delivered as its own Envelope body (see Envelope.body.patchBatch).
#
# Contiguity (from+1..=to with no gaps, duplicates, or regressions), the
# endpoint agreement, uniqueness of patch ids, the one-patch minimum, and
# the 64-patch maximum all belong to owned conversion; the wire shape
# deliberately represents violations so they can be rejected with typed
# errors rather than silently truncated.
struct PatchBatch {
  # Thread whose projection advances. Identifier rule.
  threadId @0 :Text;

  # Subscriber cursor before this batch. Zero valid (fresh subscriber).
  fromCursor @1 :UInt64;

  # Cursor after this batch; must equal the final patch's sequence under
  # owned validation.
  toCursor @2 :UInt64;

  # Patches in replay order. At least 1 and at most 64 entries by owned
  # conversion (the legacy replay-read ceiling).
  patches @3 :List(ConversationPatch);
}

# Newest-N half of a bounded conversation read.
#
# maximumTurnCount mirrors the domain's `QueryTurnCount` exactly: valid
# requests stay within 1..=512, and the 16-bit width keeps out-of-range
# values such as 0 or 513 representable so owned conversion can reject them
# with typed errors instead of the wire truncating them into validity.
struct QueryWindow {
  # Maximum turns to include. Owned conversion accepts 1..=512.
  maximumTurnCount @0 :UInt16;
}

# Older-history half of a bounded conversation read.
struct QueryRange {
  # Exclusive upper bound: load turns strictly before this zero-based
  # ordinal.
  beforeTurnOrdinal @0 :UInt64;

  # Optional inclusive floor for navigation toward one target turn.
  #
  # A union rather than a sentinel: zero is a legitimate ordinal, so
  # "absent" must be distinguishable from floor zero.
  minimumTurnOrdinal :union {
    # No floor: page until maximumTurnCount is reached.
    noMinimum @1 :Void;

    # Inclusive lower bound as a zero-based ordinal.
    minimum @2 :UInt64;
  }

  # Maximum turns to include. Same 16-bit rationale as QueryWindow:
  # 1..=512 accepted, out-of-range values such as 513 stay representable
  # for typed rejection by owned conversion.
  maximumTurnCount @3 :UInt16;
}

# Request for one bounded canonical snapshot. Reads are always windowed or
# ranged; older history hydrates with additional range requests instead of
# admitting an unbounded snapshot frame.
struct ConversationQueryRequest {
  # Thread whose projection is requested. Identifier rule.
  threadId @0 :Text;

  bounds :union {
    # Newest bounded turns.
    window @1 :QueryWindow;

    # Older bounded turns before a loaded ordinal.
    range @2 :QueryRange;
  }
}

# Request to begin authoritative conversation delivery for one thread.
struct ConversationSubscribeRequest {
  # Thread to observe. Identifier rule.
  threadId @0 :Text;

  start :union {
    # Fresh subscription: the server's first delivered value must be a full
    # snapshot.
    fresh @1 :Void;

    # Resume delivery strictly after a previously applied cursor. Zero is
    # valid and replays from the first patch.
    resumeAfter @2 :UInt64;
  }
}

# Request to end authoritative conversation delivery for one thread.
struct ConversationUnsubscribeRequest {
  # Thread no longer observed by the client. Identifier rule.
  threadId @0 :Text;
}

# Where a resumed subscription picks up.
struct ConversationResumePoint {
  # Thread being resumed. Identifier rule.
  threadId @0 :Text;

  # The last patch sequence already applied by the subscriber. Delivery
  # resumes with the very next patch, cursor + 1; nothing at or below this
  # cursor is retransmitted. Zero valid (nothing applied yet).
  cursor @1 :UInt64;
}

# Acknowledgement that authoritative conversation delivery began. The union
# makes the mandatory-first-value contract expressible on the wire: fresh
# subscriptions must start with a complete snapshot; resumed subscriptions
# instead state where replay continues. Owned conversion maps these onto
# the domain's snapshot-first guarantee.
struct ConversationSubscriptionStarted {
  union {
    # Full canonical snapshot establishing the projection.
    fresh @0 :ConversationSnapshot;

    # Resume acknowledgement carrying the continuation point only.
    resumed @1 :ConversationResumePoint;
  }
}

# Acknowledgement that authoritative conversation delivery ended cleanly.
struct ConversationSubscriptionStopped {
  # Thread no longer being delivered. Identifier rule.
  threadId @0 :Text;
}

# Typed rejection or failure report.
struct ProtocolError {
  code @0 :ErrorCode;

  # Human-readable detail. At most 1024 UTF-8 bytes; empty allowed so codes
  # alone remain renderable.
  message @1 :Text;

  # True when repeating the identical request later may succeed.
  retryable @2 :Bool;

  union {
    # The triggering request's client RequestId (its Envelope.messageId).
    # Identifier rule.
    correlated @3 :Text;

    # No request is implicated (e.g. hello-time version rejection).
    uncorrelated @4 :Void;
  }
}

# ---------------------------------------------------------------------------
# Root envelope. Every frame on the wire is exactly one of these.
# ---------------------------------------------------------------------------

struct Envelope {
  # Application protocol version the sender speaks for this frame. See the
  # header comment for hello/welcome negotiation semantics. Currently 1.
  protocolVersion @0 :UInt32;

  # Protocol-owned FrameId minted by the sending side. On request frames the
  # client FrameId is also the domain RequestId and stays stable across
  # durable or idempotent retries. PickDirectory must not be automatically
  # replayed; each deliberate new attempt uses a fresh frame identity (see
  # Request.pickDirectory). Welcome, response, event, and error frames carry
  # independently server-minted FrameIds. Identifier rule.
  messageId @1 :Text;

  # Sender timestamp. Signed Unix epoch milliseconds (UTC).
  sentAtMillis @2 :Int64;

  body :union {
    hello @3 :Hello;
    welcome @4 :Welcome;
    request @5 :Request;
    response @6 :Response;
    event @7 :Event;
    protocolError @8 :ProtocolError;

    # Appended for conversation replay delivery; seventh union member.
    # Ordinals @3-@8 were frozen when the six-member body contract was
    # committed, so the field ordinal below is deliberately @9 even though
    # this is member index 6 of the union -- never renumbered onto @6,
    # which is already response. Existing members stay untouched.
    patchBatch @9 :PatchBatch;
  }
}

# One owned image attachment in a general queued message. `bytes` is encoded
# image content, never a filesystem path or URI. Owned conversion enforces the
# accepted MIME set, filename-only name policy, and aggregate byte bounds.
struct ImageAttachment {
  mimeType @0 :Text;
  name @1 :Text;
  bytes @2 :Data;
}

# General message submission. `text` preserves absent versus present-empty so
# an image-only command never needs a placeholder string. `steerRunId`
# names the observed live run the message must steer into; empty means a
# fresh send in every state.
struct QueueMessageRequest {
  threadId @0 :Text;
  text :union {
    absent @1 :Void;
    present @2 :Text;
  }
  attachments @3 :List(ImageAttachment);
  steerRunId @4 :Text;
}

# Receipt for a general queued message. It intentionally mirrors
# FirstMessageReceipt but has its own fresh type so consumers cannot infer a
# first-message-only uniqueness rule from the response arm.
struct QueueMessageReceipt {
  requestId @0 :Text;
  messageId @1 :Text;
  threadId @2 :Text;
  disposition @3 :ReceiptDisposition;
  state @4 :QueuedState;
}

# Renderer-visible user input carrying ordered image bytes. The payload is
# deliberately separate from UserMessageItem so image-only input is never
# coerced into the legacy nonblank body field.
struct MultimodalUserMessageItem {
  itemId @0 :Text;
  turnId @1 :Text;
  ordinal @2 :UInt64;
  revision @3 :UInt64;
  lifecycle @4 :ConversationLifecycle;
  text :union {
    absent @5 :Void;
    present @6 :Text;
  }
  attachments @7 :List(ImageAttachmentRef);
  createdAtMillis @8 :Int64;
  updatedAtMillis @9 :Int64;

  # Original queued message identity for truthful receipt echo correlation.
  # Empty on rows written before this field existed; present values
  # validate as message ids. Identifier rule.
  sourceMessageId @10 :Text;
}

# Byte-free renderer reference for one persisted image. The bytes are
# returned only by the authenticated single-image read query.
struct ImageAttachmentRef {
  messageId @0 :Text;
  threadId @1 :Text;
  index @2 :UInt32;
  mimeType @3 :Text;
  name @4 :Text;
  sizeBytes @5 :UInt32;
  digest @6 :Data;
}

# Authenticated bounded query for one owned image. The backend verifies the
# thread/message relation and exact ordered index before returning bytes.
struct ReadMessageImageRequest {
  threadId @0 :Text;
  messageId @1 :Text;
  index @2 :UInt32;
}

# One bounded image read response. Metadata is repeated so the caller can
# verify the bytes against the exact renderer reference it requested.
struct MessageImageResult {
  reference @0 :ImageAttachmentRef;
  bytes @1 :Data;
}

# ---------------------------------------------------------------------------
# Provider-account usage. Owned conversions in `modules/protocol/src/codec.rs`
# validate these messages before they cross application service boundaries.
# Every Text bound below is measured in UTF-8 bytes and enforced by owned
# conversion; the wire shapes stay finite and explicit here. Appended after
# the image-read contract was committed; fresh ordinals throughout, existing
# ordinals frozen. Mirrors `EngineUsageQuery`, `EngineUsageReport`, and
# `EngineUsageSnapshot` in `modules/protocol/src/engine-usage.ts` of the
# TypeScript reference.
# ---------------------------------------------------------------------------

# Requests provider-account usage. The scope narrows the read to one engine
# so clients can fan out per engine; force re-asks providers even when
# cached reports are fresh.
struct ReadAccountUsageRequest {
  scope :union {
    # Every registered engine reports.
    all @0 :Void;
    # One engine reports. Identifier rule (for example "codex").
    one @1 :Text;
  }

  # User-initiated refresh bypassing the backend freshness window.
  force @2 :Bool;
}

# Classifies one provider quota window by its billing cadence.
enum EngineUsageWindowKind {
  session @0;
  weekly @1;
  monthly @2;
  unknown @3;
}

# Reports whether the provider account behind an engine can be billed.
enum EngineUsageAuthentication {
  authenticated @0;
  unauthenticated @1;
  unknown @2;
}

# Carries the explicit quota-surface distinction. An empty window list never
# implies anything about the provider's quota API on its own.
enum QuotaSurface {
  supported @0;
  unknown @1;
  unsupported @2;
}

# One provider-reported quota window as a used percentage. Owned conversion
# enforces the identifier rule on id, the 0..=100 clamp on percentUsed, the
# ISO-8601 shape on a nonempty resetsAt, and positivity on a nonzero
# windowMinutes.
struct EngineUsageWindow {
  # Provider's stable bucket identifier (for example "five_hour").
  # Identifier rule.
  id @0 :Text;

  kind @1 :EngineUsageWindowKind;

  # Provider's human bucket name, when one exists. Empty means absent;
  # otherwise at most 256 UTF-8 bytes, nonblank, no control characters.
  label @2 :Text;

  # Used percentage in 0..=100. Non-finite values are rejected, never
  # clamped into meaning.
  percentUsed @3 :Float64;

  # ISO-8601 reset instant, when known. Empty means absent.
  resetsAt @4 :Text;

  # Provider-reported window cadence in minutes, when known. Zero means
  # absent; otherwise strictly positive.
  windowMinutes @5 :UInt32;
}

# One engine's provider-account usage report. Owned conversion enforces the
# identifier rule on engineId, the display-name bound, the email bound on a
# nonempty accountEmail, the reason bound on nonempty authReason and failure,
# and at most 64 windows.
struct EngineUsageReport {
  # Stable engine id (for example "codex"). Identifier rule.
  engineId @0 :Text;

  # Engine display name. At most 256 UTF-8 bytes, nonblank.
  displayName @1 :Text;

  authentication @2 :EngineUsageAuthentication;

  # Bounded human reason for the authentication state, when supplied. Empty
  # means absent; otherwise at most 1024 UTF-8 bytes, nonblank.
  authReason @3 :Text;

  # Provider account email, when the transport discloses one. Empty means
  # absent; otherwise at most 320 UTF-8 bytes, nonblank.
  accountEmail @4 :Text;

  # Explicit quota-surface distinction. Absent means the reader did not
  # determine it; present carries exactly one surface.
  quotaSurface :union {
    absent @5 :Void;
    present @6 :QuotaSurface;
  }

  # Artisan-owned failure reason, when the read failed. Empty means absent;
  # otherwise at most 1024 UTF-8 bytes, nonblank. Never a provider payload.
  failure @7 :Text;

  # Bounded quota windows. At most 64 entries by owned conversion.
  windows @8 :List(EngineUsageWindow);
}

# Provider-account usage snapshot for the requested engines. Owned conversion
# enforces at most 16 engine reports and a valid ISO-8601 fetch instant.
struct EngineUsageSnapshot {
  # Per-engine reports in backend roster order. At most 16 entries by owned
  # conversion.
  engines @0 :List(EngineUsageReport);

  # ISO-8601 fetch instant shared by every report in this snapshot.
  fetchedAt @1 :Text;
}

# A-approve run-interaction round-trip (appended 2026-09-08).
#
# These declarations live at the end of the file on purpose: the schema
# compiler assigns top-level node identities in declaration order, so
# appending here keeps every pre-existing node identity stable. Union arms
# above (`Request.respondApproval/respondQuestion`,
# `Response.approvalResponse/questionResponse`) are fields, not nodes, and
# grow by fresh ordinals with existing ordinals frozen.
# ---------------------------------------------------------------------------

# An explicit answer to one pending approval request. Like StopRun this is a
# live routing request authenticated by its exact thread/run ownership: the
# response receipt reports only the per-target outcome, while the owning run
# settles the durable resolution separately. There is no default decision;
# `approved` is always an explicit choice.
struct RespondApprovalRequest {
  threadId @0 :Text;
  runId @1 :Text;
  approvalId @2 :Text;
  approved @3 :Bool;
}

# An explicit answer to one pending question. Same live-routing contract as
# the approval request above. An empty answer list records an explicitly
# skipped question.
struct RespondQuestionRequest {
  threadId @0 :Text;
  runId @1 :Text;
  questionId @2 :Text;
  # Each answer is non-empty and bounded by owned conversion (the observation
  # answer ceiling); the list itself is bounded to the observation answer
  # count. An empty list is valid and means the question was skipped.
  answers @3 :List(Text);
}

# How one live approval/question response settled its target. These are
# per-target routing results for a well-formed, authenticated request, not
# wire rejections: the request was valid, but its target may be absent,
# already settled, or owned by another run.
enum RespondInteractionOutcome {
  applied @0;
  unknownTarget @1;
  alreadyResolved @2;
  wrongRun @3;
}

# Correlated result of one approval response. The nested request id must
# equal the enclosing Response.requestId exactly; the decision echoes so a
# replay can prove it answers the identical intent.
struct RespondApprovalReceipt {
  requestId @0 :Text;
  threadId @1 :Text;
  runId @2 :Text;
  approvalId @3 :Text;
  approved @4 :Bool;
  outcome @5 :RespondInteractionOutcome;
  disposition @6 :ReceiptDisposition;
}

# Correlated result of one question response. Same correlation and
# intent-echo contract as the approval receipt; an empty answer list echoes
# an explicitly skipped question.
struct RespondQuestionReceipt {
  requestId @0 :Text;
  threadId @1 :Text;
  runId @2 :Text;
  questionId @3 :Text;
  answers @4 :List(Text);
  outcome @5 :RespondInteractionOutcome;
  disposition @6 :ReceiptDisposition;
}
