# Visible send interface (send lane)

Owner: reference-send-frontend lane (Muse Go). Read-only for other lanes.
Scene/state-machine, backend, protocol, domain, and DB edits belong to
their lanes; exact cross-refs below.

## 1. Source diagnosis

- Send path is coherent end to end: submit → `MessageQueued` receipt →
  composer clears → forced queue refresh → lip row from the listing →
  claim drops the row → slow polls sustain during Running.
- `pending_lip_rows` (`composer_queue_state.rs`) mapped every listed entry
  with no take-up: once the transcript echoed the message, the lip still
  showed it until the queue listing dropped the row. Reference releases the
  lip at echo (`TakeUp`).
- No optimistic transcript state exists in the frontend; after the receipt
  clears the composer the sent text is visible nowhere until the backend
  echo. Backend persistence IS incremental (evidence
  `visible-live-patch-timing.txt` in the canonical checkout: 129 appends
  over ~2s); the missing pieces are the launch wake and the Pending
  narration, both outside this lane.
- Native transcript items carry no engine attribution; the receipt carries
  the Forge `message_id` but no engine either. The only send-time engine
  truth lives in this lane (authoritative config) and in run observation.

## 2. Frozen correlation contract (root)

- `UserMessageItem` / `MultimodalUserMessageItem` gain
  `source_message_id: Option<MessageId>` (domain/protocol/DB worker owns
  the fields, wire, and backfill; mechanical fixtures theirs too).
- Echo match rule (this lane implements): an echo item matches a staged
  watch **iff** `item.source_message_id == Some(receipt.message_id)`.
  Never item-id equality. Absent legacy source id matches nothing: no
  guessed take-up or label; the forced queue refresh stays the fallback.
- The approved consumer event is
  `ConversationStateEvent::SetTurnEngineLabel { turn_id, engine_label }`
  with `TurnNarrationEntry` metadata (projection lane owns the variant and
  the narration). Producer code is written against that exact shape; root
  integrates prerequisites before gate.

## 3. Producer contract (implemented here)

- **Capture at send** (`begin_message_submission`, retry): validated routed
  engine display label from the *authoritative* config selection
  (`profile_usage_display_name`, provider-owned roster, id fallback) —
  never from the picker/choice. Retained in `NativeMessageFlight`,
  preserved verbatim in `NativeMessageRetry` (same rule as the steer
  target, same request id).
- **Stage at receipt**: `EchoWatch { message_id, steer_run_id,
  engine_label }` in a bounded per-source-ID collection
  (`ECHO_WATCH_LIMIT = 8`; replayed receipts replace by id; oldest evicted
  past the cap). Receipts end flights, so several sends can await echo.
- **Match**: patch-batch `ItemUpsert` user/multimodal items and installed
  snapshots (pre-move scan) plus a canonical-snapshot rescan after staging
  (covers echo-before-receipt) and on later queue refreshes (covers a label
  dispatch lost to backpressure). Retired watches re-resolve nothing.
- **On match, exactly once per id**: mark taken up immediately (lip retires
  even while still listed); dispatch `SetTurnEngineLabel` to the same host;
  keep the watch until the dispatch succeeds, then clear and
  `pump_host_boundary` so the raised invalidation drains instead of
  accumulating one effect per send. Duplicates find
  no watch (lip) or re-dispatch idempotently (label; consumer dedups).
- **Label resolution**: send-time captured label stands UNLESS exact proof —
  watch `steer_run_id` equals the observed active run, or the canonical
  snapshot holds an assistant item of the echoed turn produced by the
  observed run. A merely same-thread observed run never overrides (stale
  polls describe previous engines). `None` renders the generic fallback,
  never "Waiting for Other".
- **Lip bounds**: `taken_up` prunes against each authoritative listing;
  a finite `retired_echoes` history (32) keeps re-listed echoes retired;
  both clear on thread-scope change.
- **Fences**: thread match at stage/match/dispatch; failure matches only
  active flights (which never staged); scope change clears watches.

## 4. Needs from other lanes

1. Domain/protocol/DB: `source_message_id` fields + wire + backfill +
   mechanical fixtures (fifth worker; gate needs it first).
2. Projection: `SetTurnEngineLabel` variant + narration metadata + Pending
   narration/wake + mounted visible-projection proof (their files/tests).
3. Backend: accept-time settings snapshot + typed unconfigured refusal
   (their lane); held-provider QUIC test proves timely delivery.
4. Open question for root: no new generic scope taken here; the 150ms
   reference lip minimum is covered only by the existing closing fade.
