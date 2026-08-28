# Artisan application protocol: Phase 2 wire contract.
#
# This schema is wire shape only. Owned conversions between these generated
# bindings and `artisan-domain` values arrive in a later packet; until then no
# production code reads or writes these messages outside of tests.
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
#   retries: a retrying client resends the same envelope verbatim and never
#   mints a second id for the same logical attempt. Server frames (welcome,
#   response, event, protocol error) carry independently server-minted
#   FrameIds. Forge mints durable queued-message identities separately (see
#   FirstMessageReceipt and FirstMessageQueued). These vocabularies never
#   alias.
# * Response.requestId, FirstMessageReceipt.requestId,
#   FirstMessageQueued.requestId, and correlated ProtocolError arms echo the
#   client RequestId -- that is, the triggering request's Envelope.messageId
#   -- and must never be conflated with the responding server frame's own
#   FrameId. Uncorrelated errors (for example version rejection before any
#   request) use the uncorrelated arm.
#
# Authentication posture
# ----------------------
# * Editor-to-Forge: Hello.capability carries high-entropy, one-time local
#   client capability material. It reaches the editor out-of-band through the
#   restricted parent-child handoff used to launch its local Forge process,
#   then travels only in Hello. It is secret: never displayed, logged,
#   formatted, or embedded in diagnostics. It proves the editor was the
#   intended launcher of this Forge instance.
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
# one-time capability proving it was the intended launcher of this Forge.
struct Hello {
  # Offered versions, ascending, unique, each >= 1. At most 8 entries.
  # Revision 1 supports exactly [1].
  supportedVersions @0 :List(UInt32);

  # High-entropy one-time local client capability: exactly 32 bytes of secret
  # authentication material received out-of-band via the restricted
  # parent-child handoff that launched this Forge process. Authenticates the
  # editor to Forge (transport fingerprint pinning covers the reverse
  # direction). Never displayed, logged, formatted, or persisted beyond the
  # handshake decision. Owned conversion enforces the exact 32-byte length;
  # the Data wire type alone does not.
  capability @1 :Data;
}

# Server answer: the single negotiated application protocol version.
struct Welcome {
  # Exactly one of the versions offered by the triggering hello.
  negotiatedVersion @0 :UInt32;

  # Opaque connection-scoped id for diagnostics. Identifier rule.
  connectionId @1 :Text;
}

# ---------------------------------------------------------------------------
# Requests. Existing identities are referenced by their opaque ids (projects
# and threads already known to Forge); Forge mints only NEW identities: the
# project id when a directory is attached, the thread id when a thread is
# created, and the queued-message id when a first message is durably queued.
# Mutation retries resend the request frame verbatim, reusing the client
# FrameId that doubles as its stable RequestId.
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

# The six requests of the first native workflow.
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
  }
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
  # retries; welcome, response, event, and error frames carry independently
  # server-minted FrameIds. Identifier rule.
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
  }
}
