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
#   attempt. The host-interaction pickDirectory request sits outside that
#   verbatim-retry guarantee: it persists nothing, must not be automatically
#   replayed, and every deliberate new attempt is a new logical attempt that
#   mints a fresh frame identity.
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

# The request arms of the native protocol: the six of the first workflow
# plus the three conversation read/subscription requests appended below.
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
  }
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
}

# Renderer-visible conversation item vocabulary for this phase. Appending a
# second kind later adds one union member; existing members never move.
#
# The domain models exactly one item kind this phase, but a Cap'n Proto
# union requires two members, so `unmodeled` occupies the second slot: it
# carries no data and is never sent by a conforming peer. Owned conversion
# rejects it, and the next item kind claims its own fresh ordinal when it
# lands.
struct ConversationItem {
  union {
    # Canonical user input durably queued before any engine dispatch.
    userMessage @0 :UserMessageItem;

    # Placeholder keeping the union compilable while only one item kind is
    # modeled; never produced this revision.
    unmodeled @1 :Void;
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
}

# Lifecycle transition applied to one renderer-visible item.
struct ItemLifecyclePatch {
  # Target item identity. Identifier rule.
  itemId @0 :Text;

  # Revision after applying this transition. Zero-based.
  revision @1 :UInt64;

  # New lifecycle.
  lifecycle @2 :ConversationLifecycle;
}

# Lifecycle transition applied to one canonical turn.
struct TurnLifecyclePatch {
  # Target turn identity. Identifier rule.
  turnId @0 :Text;

  # Revision after applying this transition. Zero-based.
  revision @1 :UInt64;

  # New lifecycle.
  lifecycle @2 :ConversationLifecycle;
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

    # Appended for conversation replay delivery; seventh union member.
    # Ordinals @3-@8 were frozen when the six-member body contract was
    # committed, so the field ordinal below is deliberately @9 even though
    # this is member index 6 of the union -- never renumbered onto @6,
    # which is already response. Existing members stay untouched.
    patchBatch @9 :PatchBatch;
  }
}
