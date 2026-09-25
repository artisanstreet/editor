# Bounded queued-message recall and run-usage wire leaf.
#
# This schema is intentionally self-contained. The parent artisan schema
# imports it, so importing artisan here would create a schema cycle. All
# identifiers and bounded values therefore remain primitive on this leaf and
# are converted through the owned domain constructors in the Rust codec.
#
# Schema evolution is append-only. The parent Request union reserves ordinals
# 21..24 for the first four requests and ordinal 28 for the failed-dispatch
# read; the parent Response union reserves ordinals 22..25 for the first four
# corresponding responses and ordinal 29 for the failed-dispatch listing.

@0xb7d9f2e4a16c8b03;

enum QueuedMessageListOrder {
  oldestFirst @0;
  latestFirst @1;
}

enum ReceiptDisposition {
  accepted @0;
  duplicate @1;
}

enum QueuedMessageWithdrawalOutcome {
  withdrawn @0;
  tooLate @1;
  notQueued @2;
}

enum RunUsageBasis {
  delta @0;
  cumulative @1;
  unknown @2;
}

struct ListQueuedMessagesRequest {
  threadId @0 :Text;
  order @1 :QueuedMessageListOrder;
  limit @2 :UInt16;
}

struct ListFailedMessagesRequest {
  threadId @0 :Text;
  limit @1 :UInt16;
}

struct WithdrawQueuedMessageRequest {
  threadId @0 :Text;
  messageId @1 :Text;
  originalRequestId @2 :Text;
  # The edit flow: a withdrawn payload becomes the thread's composer draft.
  recallToDraft @3 :Bool;
}

struct ReadRecalledMessageRequest {
  threadId @0 :Text;
  messageId @1 :Text;
  originalRequestId @2 :Text;
}

struct ReadRunUsageRequest {
  threadId @0 :Text;
  runId @1 :Text;
}

struct ImageAttachment {
  mimeType @0 :Text;
  name @1 :Text;
  bytes @2 :Data;
}

struct ImageAttachmentRef {
  messageId @0 :Text;
  threadId @1 :Text;
  index @2 :UInt32;
  mimeType @3 :Text;
  name @4 :Text;
  sizeBytes @5 :UInt32;
  digest @6 :Data;
}

struct QueueMessagePayload {
  # A null Text pointer means None; a present zero-length Text means Some("").
  text @0 :Text;
  attachments @1 :List(ImageAttachment);
}

struct QueuedMessageSummary {
  messageId @0 :Text;
  threadId @1 :Text;
  originalRequestId @2 :Text;
  # The same null-versus-present-empty rule as QueueMessagePayload.text.
  text @3 :Text;
  attachments @4 :List(ImageAttachmentRef);
  acceptedAtMillis @5 :Int64;
  # Latest dispatcher diagnostic, when the dispatcher has claimed and
  # requeued this message at least once. A null Text pointer means no
  # diagnostic; a never-attempted row carries no error.
  lastError @6 :Text;
  # Forge-owned delivery state of the row.
  state @7 :QueuedMessageState;
  # Engine of the accepted configuration snapshot; empty means none.
  engineId @8 :Text;
}

enum QueuedMessageState {
  queued @0;
  dispatching @1;
}

struct QueuedMessageListing {
  threadId @0 :Text;
  order @1 :QueuedMessageListOrder;
  limit @2 :UInt16;
  messages @3 :List(QueuedMessageSummary);
  totalCount @4 :UInt64;
  hasMore @5 :Bool;
}

struct FailedMessageSummary {
  messageId @0 :Text;
  threadId @1 :Text;
  originalRequestId @2 :Text;
  # The same null-versus-present-empty rule as QueueMessagePayload.text.
  text @3 :Text;
  attachments @4 :List(ImageAttachmentRef);
  acceptedAtMillis @5 :Int64;
  failedAtMillis @6 :Int64;
  # Terminal dispatcher diagnostic persisted with the failure. Always present
  # on the wire: a terminally failed row carries its reason verbatim.
  reason @7 :Text;
  # Whether retryFailedMessage can re-dispatch the stored payload.
  retryable @8 :Bool;
}

struct FailedMessageListing {
  threadId @0 :Text;
  limit @1 :UInt16;
  messages @2 :List(FailedMessageSummary);
  totalCount @3 :UInt64;
  hasMore @4 :Bool;
}

struct QueuedMessageWithdrawalResult {
  # Durable command receipt identity. The parent Response.requestId must
  # equal this field; the codec enforces that correlation exactly.
  requestId @0 :Text;
  threadId @1 :Text;
  messageId @2 :Text;
  originalRequestId @3 :Text;
  acceptedAtMillis @4 :Int64;
  disposition @5 :ReceiptDisposition;
  outcome @6 :QueuedMessageWithdrawalOutcome;
}

struct RecalledMessageResult {
  threadId @0 :Text;
  messageId @1 :Text;
  originalRequestId @2 :Text;
  # A null struct pointer means no authoritative withdrawn payload exists.
  payload @3 :QueueMessagePayload;
}

struct OptionalText {
  present @0 :Bool;
  value @1 :Text;
}

struct OptionalUInt64 {
  present @0 :Bool;
  value @1 :UInt64;
}

struct RunUsageReport {
  providerSessionId @0 :Text;
  sourceSequence @1 :UInt64;
  modelId @2 :Text;
  providerRouteId @3 :Text;
  variantId @4 :OptionalText;
  basis @5 :RunUsageBasis;
  providerTurnId @6 :OptionalText;
  inputTokens @7 :OptionalUInt64;
  cachedInputTokens @8 :OptionalUInt64;
  outputTokens @9 :OptionalUInt64;
  contextTokens @10 :OptionalUInt64;
  contextWindowTokens @11 :OptionalUInt64;
  observedAtMillis @12 :Int64;
  # Observed o200k visible-text estimate; absent for historical/unmeasurable output.
  streamingMillitokensPerSecond @13 :OptionalUInt64;
}

struct RunUsageResult {
  threadId @0 :Text;
  runId @1 :Text;
  # A null struct pointer means no authoritative usage report exists.
  report @2 :RunUsageReport;
  # Context size, in tokens, at which the reporting run's engine compacts;
  # absent when no documented policy applies (the window is the limit).
  # Decided by the Forge from the reporting run's engine and model.
  compactionAtTokens @3 :OptionalUInt64;
}


# ---------------------------------------------------------------------------
# Forge-owned composer drafts and the content-addressed attachment store.
# The parent Request union carries saveComposerDraft @32, readComposerDraft
# @33, uploadComposerAttachment @34, readComposerAttachment @35 and
# queueStoredMessage @36; the parent Response union carries
# composerDraftSaved @32, composerDraft @33, composerAttachmentUploaded @34
# and composerAttachment @35. A stored-message send is answered by the
# existing queuedMessageReceipt arm.
# ---------------------------------------------------------------------------

# The composer a draft belongs to: an existing thread, or the new-task
# composer of an attached project before its first message creates a thread.
struct ComposerDraftScope {
  union {
    thread @0 :Text;
    project @1 :Text;
  }
}

# Byte-free reference to one stored attachment. `digest` is the SHA-256 of
# the encoded bytes and the store key; mimeType and sizeBytes repeat the
# stored metadata for verification; name is the authored display name.
struct ComposerAttachmentRef {
  digest @0 :Data;
  mimeType @1 :Text;
  name @2 :Text;
  sizeBytes @3 :UInt32;
}

# Replaces the scope's draft. Every save applies (the last to arrive wins);
# the Forge assigns the scope's next revision.
struct SaveComposerDraftRequest {
  scope @0 :ComposerDraftScope;
  text @1 :Text;
  attachments @2 :List(ComposerAttachmentRef);
}

# Acknowledges one save with the revision the Forge assigned to it.
# requestId must equal the parent Response.requestId.
struct ComposerDraftSaved {
  requestId @0 :Text;
  scope @1 :ComposerDraftScope;
  revision @2 :UInt64;
}

struct ReadComposerDraftRequest {
  scope @0 :ComposerDraftScope;
}

struct ComposerDraft {
  revision @0 :UInt64;
  text @1 :Text;
  attachments @2 :List(ComposerAttachmentRef);
  updatedAtMillis @3 :Int64;
}

struct ComposerDraftResult {
  scope @0 :ComposerDraftScope;
  # A null struct pointer means the scope never saved a draft.
  draft @1 :ComposerDraft;
}

# Stores one image under the digest of its bytes: whole (`image`, one
# frame) or one chunk at a time (`chunk`, for images larger than a frame).
struct UploadComposerAttachmentRequest {
  image @0 :ImageAttachment;
  # A null pointer uploads `image` whole.
  chunk @1 :ComposerAttachmentChunk;
}

# requestId must equal the parent Response.requestId.
struct ComposerAttachmentUploaded {
  requestId @0 :Text;
  reference @1 :ComposerAttachmentRef;
  # Bytes of a chunked upload still missing; zero once stored.
  pendingBytes @2 :UInt32;
}

struct ReadComposerAttachmentRequest {
  digest @0 :Data;
  # A window of the stored bytes; maxBytes zero answers every byte from
  # offset.
  offset @1 :UInt32;
  maxBytes @2 :UInt32;
}

struct ComposerAttachmentResult {
  digest @0 :Data;
  mimeType @1 :Text;
  # The window of bytes starting at offset.
  bytes @2 :Data;
  # Byte length of the whole image.
  totalBytes @3 :UInt32;
  offset @4 :UInt32;
}

# Queues one message whose images are stored attachments; the Forge
# resolves them and admits the message exactly like queueMessage. Text-only
# messages use queueMessage.
struct QueueStoredMessageRequest {
  threadId @0 :Text;
  # A null Text pointer means None; a present zero-length Text means Some("").
  text @1 :Text;
  attachments @2 :List(ComposerAttachmentRef);
  # Empty means a fresh send; otherwise the observed live run to steer into.
  steerRunId @3 :Text;
}

# ---------------------------------------------------------------------------
# Forge-owned submissions. The parent Request union carries
# retryFailedMessage @37 and recoverFailedMessage @38; the parent Response
# union carries failedMessageRetried @36 and failedMessageRecovered @37; the
# parent Event union carries messageOutbox @5.
# ---------------------------------------------------------------------------

# Every undelivered message of one thread, pushed over its subscription.
struct MessageOutbox {
  queued @0 :QueuedMessageListing;
  failed @1 :FailedMessageListing;
}

# One terminally failed message.
struct FailedMessageTarget {
  threadId @0 :Text;
  messageId @1 :Text;
  originalRequestId @2 :Text;
}

enum FailedMessageRetryOutcome {
  requeued @0;
  notRetryable @1;
}

# requestId must equal the parent Response.requestId.
struct FailedMessageRetried {
  requestId @0 :Text;
  target @1 :FailedMessageTarget;
  outcome @2 :FailedMessageRetryOutcome;
}

# requestId must equal the parent Response.requestId. An empty newThreadId
# means the message was not a recoverable failure.
struct FailedMessageRecovered {
  requestId @0 :Text;
  target @1 :FailedMessageTarget;
  newThreadId @2 :Text;
  disposition @3 :ReceiptDisposition;
}

# ---------------------------------------------------------------------------
# Draft submission. The parent Request union carries submitComposerDraft @39;
# the parent Response union carries composerDraftSubmitted @38.
# ---------------------------------------------------------------------------

# Queues the thread's composer draft at exactly draftRevision. The parent
# request id only correlates the answer; the submission's identity is the
# thread and draft revision.
struct SubmitComposerDraftRequest {
  threadId @0 :Text;
  draftRevision @1 :UInt64;
  # Retired: the Forge decides whether a send steers the thread's live run.
  # Always empty; a nonempty value is rejected.
  steerRunId @2 :Text;
  # The model the user selected for this send; a null pointer sends with the
  # thread's saved configuration. The Forge resolves, saves, and admits it.
  selection @3 :CatalogSelection;
}

# The draft is the queued message messageId; the draft is empty at
# clearedRevision.
struct DraftSubmissionQueued {
  messageId @0 :Text;
  disposition @1 :ReceiptDisposition;
  clearedRevision @2 :UInt64;
  # The thread's engine configuration revision after admission.
  engineConfigRevision @3 :UInt64;
}

# requestId must equal the parent Response.requestId.
struct ComposerDraftSubmitted {
  requestId @0 :Text;
  threadId @1 :Text;
  draftRevision @2 :UInt64;
  union {
    queued @3 :DraftSubmissionQueued;
    # The stored draft is at another revision; nothing was queued. Zero
    # means the thread has no draft.
    stale @4 :UInt64;
    # The Forge refused the send; nothing was queued.
    refused @5 :SubmissionRefusal;
  }
}

# ---------------------------------------------------------------------------
# Stateless Editor step 6: the user's model selection as catalog identities
# and the Forge's typed refusals. Appended so existing node identities stay
# stable.
# ---------------------------------------------------------------------------

# A model selection named by catalog identities. Empty text means absent.
struct CatalogSelection {
  modelId @0 :Text;
  profileId @1 :Text;
  reasoningEffort @2 :Text;
  speed @3 :Text;
  contextWindow @4 :Text;
  permission @5 :Text;
}

enum SubmissionRefusalKind {
  invalidSelection @0;
  noSelection @1;
  engineNotReady @2;
  runStarting @3;
  attachmentRejected @4;
}

# A typed refusal; message is presentation-ready, at most 1024 UTF-8 bytes.
struct SubmissionRefusal {
  kind @0 :SubmissionRefusalKind;
  message @1 :Text;
}

# ---------------------------------------------------------------------------
# Stateless Editor step 7: picked images up to 32 MiB cross the wire in
# chunks of at most 4 MiB. Appended so existing node identities stay stable.
# ---------------------------------------------------------------------------

# One chunk of a picked image: digest is the SHA-256 of the whole image, and
# the Forge stores it once every byte arrived and the digest matches.
struct ComposerAttachmentChunk {
  digest @0 :Data;
  mimeType @1 :Text;
  name @2 :Text;
  totalBytes @3 :UInt32;
  offset @4 :UInt32;
  bytes @5 :Data;
}
