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
}

struct RunUsageResult {
  threadId @0 :Text;
  runId @1 :Text;
  # A null struct pointer means no authoritative usage report exists.
  report @2 :RunUsageReport;
}

