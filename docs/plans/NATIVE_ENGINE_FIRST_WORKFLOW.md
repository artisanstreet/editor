# Native engine first workflow — architecture and packet plan

Status: architecture plan authored 2026-08-26 against immutable HEAD
`e9bb11888d2e6225d6679962ddb0f8ced4e3ca55`. This document designs the
smallest coherent real first workflow — one real assistant turn with a
restartable durable result — on top of the accepted source. It grants no
implementation GO by itself: root freezes each packet's worktree, exact file
ownership, and gates before any writer starts. Every "accepted" claim below
cites source in this tree; everything else is explicitly proposed work, a
decision request, or an evidence gate. No test run is claimed by this
document.

Integration checkpoint: this plan is published above `52ee041385055dd03a4d1409c3f6ae8006107bc5`.
Since the immutable analysis base, PR #119 added the owning `ForgeListener`
and PR #120 added the native `DirectoryController` with child/pipe cleanup.
The absence claims and line references below describe the stated analysis
base, not those later commits. E4/E6 workers must reuse these accepted
owners or justify a narrow extraction, not recreate their responsibilities.
Publication accepts the architecture document, not blanket implementation
authority for E2–E8; E1 has its separately frozen worker contract.

## 1. Verified current state

Accepted, implemented, and covered by declared Bazel tests (`tests/database/
BUILD.bazel:231-246` registers `run_launch_test`; sibling targets cover the
rest):

| Stage | Accepted source fact |
| --- | --- |
| Message acceptance | `modules/backend/src/request_handler.rs:295-346` admits a fresh first message through the `CommandOrigin` boundary (`command_admission.rs:72-96`); `modules/database/src/repository/first_message.rs:73-143` commits immutable message + queued dispatch + command receipt + thread recency in one transaction. |
| Dispatch claim | `repository/message_dispatch.rs:20-48,329-387`: atomic claim of the oldest queued-or-expired-LEASED row with a fresh 32-byte owner; RUNNING is excluded from eligibility, so a RUNNING dispatch can never be reclaimed by this SQL. |
| Payload read | `repository/dispatch_payload.rs:43-69`: read-only execution payload in any dispatch state; not renewed authority. |
| Atomic launch | `repository/run_launch.rs:328-411`: one transaction whose first statement (`LAUNCH_FENCE_SQL`, `:59-75`) fences the exact claimed snapshot from `leased` to `running` and **overwrites `updated_at_ms` to the launch `operated_at`**; the same transaction inserts conversation state/ordinals, the pending turn, the completed user item, the `launching` run row (generation 1, owner/lease/claim_token present, binding NULL, `run_launch.rs:695-727`), and two replay patches. Exact replay answers `AlreadyStarted` from persisted rows (`:779-862,915-979`), requiring `created_at_ms == updated_at_ms == operated_at`. |
| Legacy dispatch dispositions | `message_dispatch.rs:50-106`: complete/fail/requeue fence **LEASED only** — they cannot settle a RUNNING dispatch and stay untouched. Failure text bound: 4096 bytes (`:109`). |
| Run storage | `entities/assistant_run.rs:9-29` plus `run_checkpoint.rs:9-17` and `run_batch_receipt.rs:9-17` exist with no repository writer beyond launch (`repository/mod.rs:3-8`). |
| Schema fences | `modules/migrations/src/m20260824_000003_conversation_execution.rs`: state-shape CHECK `:459-479` (launching = claim_token present + binding NULL; running/waiting/cancel_requested = claim_token **NULL** + full binding tuple; terminal/interrupted = owner/lease/claim NULL), binding tuple `:420-430` (version > 0, blob 1..262144 bytes), error pairing `:441-447` (interrupted/failed require error_code), terminal_at `:448-458` (NULL for interrupted), unique run_start_key/origin_message/origin_turn `:489-513`, assistant item shape `:645-654` (run_id + phase required, source_message NULL), patch payload shapes `:853-887`. |
| Domain vocabulary | `modules/domain/src/conversation/assistant_message.rs`: `AssistantBody` (empty/whitespace valid, 65,536-byte ceiling), `AssistantMessagePhase`, `AssistantMessageItem` with opaque `RunId`; `conversation.rs:515-583` patches carry explicit Forge-supplied `updated_at`. |
| Protocol | `modules/protocol/schema/artisan.capnp:677-690`: `userMessage @0`, rejected `unmodeled @1`, `assistantMessage @2`; codec encodes/decodes both arms. `types.rs:740-757`: `WireEnvelopeBody` already carries `Event`, `PatchBatch`, and conversation subscription responses (`:531-601`). |
| Transport | `modules/transport/src/lib.rs:16-54`: framing, deadlines, pinned-identity handshake, single-request server dispatch (`server_dispatch.rs`), sequential consuming `ClientSession`, and the contiguous `EventSequenceTracker` (`event_sequence.rs:1-37`). |
| Backend composition | `app.rs`/`storage.rs`: `ForgeApp::start` opens+migrates storage and owns shutdown (`app.rs:52-58`, `storage.rs:31-43`). `connection.rs:1-45`: one admitted connection, credential authority lease, strictly sequential request dispatch (one accepted bidirectional stream per request, `connection.rs:419-426`); caller owns endpoint/lifetimes. Opening the database proves no exclusivity: `database/src/connection.rs:14-18,139-152` configures a WAL-journal pool (up to four connections, busy timeout), and WAL admits concurrent processes on the same file. |

Absent — the actual missing seams, none of which may be assumed:

- **Provider binding**: no `bind_run_provider`; a run cannot leave `launching`.
- **Observation commit**: no writer for batch receipts, checkpoints, or
  assistant items/patches; `ClientRequest::Conversation` answers a typed
  unbacked failure (`request_handler.rs:138-140`).
- **Terminal pairing**: no transaction settles a RUNNING dispatch together
  with a terminal run row; no recovery/reconciliation sweep exists.
- **Engine process owner**: no native child-process controller. The manifest
  already declares tokio's `process` feature (`modules/backend/Cargo.toml:18`)
  but no production code uses it; the directory helper implements only the
  child side of a stdin-lifeline protocol (`directory_helper.rs:1-30`) and
  names the parent controller as future work. No HTTP/SSE client dependency
  exists anywhere in the native graph.
- **Forge assembly**: `modules/backend/src/lib.rs:46-48` — `run()` is a stub;
  no data-directory policy, endpoint bind, launch handoff, accept loop, or
  dispatch worker is composed.
- **Server-initiated delivery**: transport has event/patch vocabulary and
  client-side ordering, but no server-side session loop ever sends an
  `Event` or `PatchBatch` frame, and the sequential `ClientSession`
  exposes only connect/request/shutdown (`client_session.rs:1-68`) — no
  client-side event or patch receiver exists either.
- **Frontend**: `frontend/src/lib.rs:1-23` runs only the proof window +
  project picker; `transcript.rs`, `composer.rs`, `attention.rs`,
  `thread_list_selection.rs` are unrendered state models; `ui/markdown.rs`
  is an engine seam without a renderer. No packaging target exists (no
  `pkg_*`/archive rules in any BUILD file).

Legacy evidence (behavioral reference only, never the shipping design),
verified in this tree: `modules/engines/src/opencode2/service.ts:95-125`
launches `opencode2 serve --stdio --port 0` with a per-service password,
strict first-line JSON readiness under a 15-second timeout, then a
10-second authenticated loopback health probe (`:140-150`) against the
certified `0.0.0-beta-17778` build (`toolchain/distribution.ts:445-446`);
`opencode2/protocol.ts:249-276` shows session create (agent/directory/
model/provider/variant), global SSE, and interrupt; `opencode2/engine.ts:
496-523` validates resume directory and re-establishes instructions.
Stream EOF or process exit is **not** provider completion in the legacy
design either: completion is decided from durable session state plus
explicit terminal observations. Nothing about installed-engine behavior,
prompt-ID deduplication, or credential acquisition is proven here.

## 2. Target flow and ordering invariants

One Forge process owns the whole durable chain. The invariant order, each
step fenced on the previous step's durable stamps:

1. queue receipt (accepted) → 2. atomic claim (accepted) → 3. atomic launch
(accepted) → 4. **spawn/CreateSession** (external, non-transactional) →
5. **atomic provider binding** (E1, before any Prompt) → 6. **Prompt**
(external) → 7. **committed observation batches** (E2) → 8. **paired
terminal disposition** (E2) → 9. post-commit publication to subscribers
(E5) → 10. client projection/render (E7).

Three rules govern every external step: a database receipt never makes an
external effect transactional or exactly-once; a NULL binding never proves
no external effect occurred (`run_launch.rs:1-15`); and after any unknown
outcome the only safe moves are exact idempotent replay of the durable
operation or a conservative interrupted disposition — never blind
resubmission of a provider prompt.

## 3. Packet E1 — provider binding (settled contract)

This packet is settled by this document, subject only to root freezing the
worktree and gates. The appended PM proposal was evaluated against source
and is adopted **with corrections noted below**.

**Ownership (six paths):** new `modules/database/src/repository/
run_binding.rs`; `run_launch.rs` changes visibility keywords only. The
settled internal seam — verified against the actual private items
(`RunStartKey` with its private `expose`, `run_launch.rs:82-106`; the
private type `RunCapability` with `expose`/`matches_stored`, `:109-129`;
`RunLaunchCredentials::parts`/`matches_stored`, `:157-172`; free
`stored_bytes_match`, `:1351-1354`) — marks exactly those items plus the
`RUN_CAPABILITY_BYTES` const `pub(super)`. `RunCapability` itself must
be included because `parts()` returns `&RunCapability` and Rust forbids
a `pub(super)` method from exposing a more-private type. No new method,
no duplicate wrapper, no signature or behavior change, and nothing
crate-public: the items become nameable only within `repository/`, and
the public credential lifecycle (constructors, zeroize-on-drop, absent
formatting/duplication traits) is untouched. This surface is
sufficient: fresh-bind fence BLOB parameters are built from `expose`
exactly as `insert_launched_run` builds them (`:702-713`); replay
diagnosis compares owner and lease per-component through `parts()` plus
`RunCapability::matches_stored`, with the erased claim-token component
deliberately unused; and the dispatch owner's hex form reuses
`DispatchLeaseOwner::to_storage`, already `pub(super)` in the sibling
module (`message_dispatch.rs:213`) and therefore already visible
throughout the `repository/` subtree. Additive exports in
`repository/mod.rs` and
`database/src/lib.rs`; new `tests/database/run_binding.rs`; one
`run_binding_test` target in `tests/database/BUILD.bazel` using the
existing dependency set. No Cargo/lock/schema/migration/backend change.

**API.** One borrowed operation on the existing `Repository`:
`bind_run_provider(BindRunProvider<'_>) -> Result<BindRunProviderOutcome,
RunBindingError>`. The command borrows the original
`ClaimedMessageDispatch`, the `LaunchedRunReceipt`, the `RunStartKey`, and
the `RunLaunchCredentials`, and adds: `expected_launch_at: UnixMillis`
(the exact launch `operated_at`), `bound_at: UnixMillis`, positive-`i64`
`binding_version`, and a new private `ProviderBindingBytes` wrapper
enforcing 1..=262,144 bytes with no `Debug`/`Display`/clone/raw-access
API, persisted through the redacted `OpaqueBytes` type. Validate that the
receipt's message equals the claim's message and that generation equals
the receipt's generation (never the dispatch attempt count). Outcomes are
payload-free: `Bound(BoundRunReceipt)` for the committed transition,
`AlreadyBound(BoundRunReceipt)` for an exact durable replay, carrying only
run/thread/message identities, generation, binding version, and bound
time. Neither outcome authorizes prompt resubmission or proves external
delivery. `RunBindingError` is capability-specific with
`#[error(transparent)] Repository(#[from] RepositoryError)`, following the
`RunLaunchError` pattern; no global error variants and no secret bytes in
any variant.

**Why `expected_launch_at` is a parameter, not a receipt change:** the
launch fence overwrote `message_dispatches.updated_at_ms` to `operated_at`
(`run_launch.rs:59-75`), and both run stamps equal `operated_at`
(`:715-721`), but `LaunchedRunReceipt` (`:203-219`) carries no timestamp.
The launch replay contract already forces the caller to retain the exact
`operated_at` (an `AlreadyStarted` replay validates `created_at_ms ==
operated_at`, `:961-963`), so the caller provably holds this value;
passing it explicitly preserves the accepted launch API and its tests
untouched. Extending the receipt with `launched_at` was considered and
rejected for this packet (it reopens accepted public API and every
existing construction site); it remains a legitimate later cleanup.

**Atomic write.** After local bounds/generation/attempt/chronology checks
(`claimed.updated_at <= expected_launch_at <= bound_at`, claimed lease
expiry `> bound_at`): ONE transaction, ONE commit. The FIRST statement is
a conditional full-snapshot UPDATE of the RUNNING dispatch — exact
message/correlation/attempt/queued/available/hex-owner/lease-expiry,
`state = 'running'`, `updated_at_ms = expected_launch_at`, and
`lease_expires_at_ms > bound_at` — tentatively moving `updated_at_ms` to
`bound_at`. This follows the accepted first-statement-writer idiom
(`LAUNCH_FENCE_SQL`, `CLAIM_NEXT_SQL`) so SQLite serializes competing
writers, and it deliberately does NOT require the stale original
`claimed.updated_at` to equal the current stamp — launch overwrote it.
The SECOND statement, inside the same transaction, conditionally updates
the run: exact run id, `lifecycle = 'launching'`, exact generation, start
key, thread/origin message/origin turn, owner, lease, claim token,
`created_at_ms = updated_at_ms = expected_launch_at`, binding tuple and
error/terminal fields absent → set `lifecycle = 'running'`, clear ONLY
`claim_token`, write the binding version/blob, `provider_bound_at_ms =
bound_at`, `updated_at_ms = bound_at`. Capability-byte equality inside
the fence SQL follows the accepted dispatch-owner-in-SQL precedent; the
zero-row diagnosis path reloads rows and classifies with the existing
constant-time comparisons. Dispatch retains its owner and expiry and
stays RUNNING; no conversation/receipt/counter row is touched. Any
zero-row fence or mismatch rolls back the entire transaction with a typed
diagnosis — never resurrection, never error-string parsing.

**Replay after claim-token erasure.** `AlreadyBound` is read-only receipt
information classified inside the same (rolled-back) transaction:
dispatch RUNNING with `updated_at_ms = bound_at` and all other original
claim fields intact; run `running` with exact binding version/blob/
`bound_at`, matching owner/lease/start key/generation/origins,
`created_at_ms = expected_launch_at`, `updated_at_ms = bound_at`,
`claim_token` NULL, no error/terminal fields. The erased claim-token
bytes cannot be compared — schema requires NULL while running
(`m20260824_000003:459-479`) — so replay classification deliberately
ignores that one component of the caller's credentials while requiring
every remaining component to match exactly. A different supplied claim
token after erasure is unobservable and must be documented as
unverifiable, not misrepresented as verified; no digest/tombstone/schema
addition pretends otherwise. Boundary case: when `bound_at ==
expected_launch_at`, a replay can pass the dispatch fence and fail only
the run fence; the diagnosis path must classify it and roll back the
tentative dispatch write. Replay repeats the original `bound_at` and its
expiry relation; the result does not assert the lease is live at retry
wall time.

**Acceptance (real SQLite, seeded through existing repository APIs):**
successful bind with exact stored blob/version/time, RUNNING pair,
retained owner/lease, NULL claim token, all unrelated rows unchanged;
deterministic post-dispatch-fence failure proving full rollback including
the dispatch stamp; rejection of wrong dispatch/run owner, generation,
lease, claim token, start key, origin tuple, stale snapshot fields,
expiry-equality, and chronology violations; payload bytes 0/1/262144/
262145 and version 0/negative/positive boundaries without leaking bytes;
exact replay preserving all rows; changed version/payload/time/remaining
credential rejection; the erased-token limitation exercised and
documented; the `bound_at == expected_launch_at` replay boundary; a
file-backed two-connection race proving first-write ownership. Preserve
every existing launch/lease/dispatch test unchanged.

## 4. Packets E2–E8

**E2 — observation commit and paired terminal disposition (database).**
Depends on E1. New `repository/run_observation.rs` (name indicative; root
freezes): `commit_run_batch` — one transaction fencing the RUNNING
dispatch pair (same both-row idiom as E1, stamps advancing from the
previous commit's stamp) and the `running` run (id/generation/owner/
lease), writing one contiguous batch of durable effects:

- the `run_batch_receipts` row (batch_sequence = checkpoint's
  last_batch_sequence + 1, generation, 32-byte digest over the batch's
  canonical content, committed = true) and the `run_checkpoints` upsert
  (same generation, new last_batch_sequence, optional bounded engine
  checkpoint blob/version);
- the assistant item on its first appearance: one fresh renderer
  ordinal allocated from `conversation_state` by checked arithmetic
  (launch consumed the first two), the item row (`streaming`, revision
  0, run_id + phase per the schema shape, `:645-654`), and its
  `item_upsert` patch;
- incremental text as `item_append` patches (fragment ≤ 4,096 bytes),
  each advancing the item row's revision by checked increment and its
  `updated_at_ms` to the batch's operated time;
- **turn state**: the launch-created turn row is Pending at revision 0
  (`run_launch.rs:644-663`) and must not remain Pending once progress
  exists. The FIRST committed batch advances it Pending → Active
  (revision 1) with one `turn_lifecycle` patch; later batches touch the
  turn only when its state actually changes. Turn and item transitions
  obey the accepted domain rules (`conversation.rs:254-284`: terminal
  lifecycles are sealed except identical replay, which stays harmless;
  non-terminal movement is free), and every turn/item mutation pairs
  its row update with a patch at its own contiguous sequence carrying
  the post-application revision;
- the writer/reader stamp mapping, stated once for E2 and E5 together:
  the patch-payload CHECK requires NULL entity stamps for append and
  lifecycle patches (`m20260824_000003:871-885`), so their
  authoritative domain `updated_at` is persisted as `recorded_at_ms`
  (= the batch operated time) and the row's `revision` column carries
  the post-application revision; upsert patches carry explicit entity
  stamps exactly as launch already writes them.

Exact replay is classified from the durable `(run_id, batch_sequence)`
receipt digest: identical digest → read-only `AlreadyCommitted`;
different digest → typed conflict; no row moves on replay. Because
later batches legitimately advance the same turn/item rows, only the
LATEST batch's replay may additionally validate current row state
exactly; an older batch's replay is classified by its receipt digest
alone. Terminal variants `complete_run` / `fail_run` pair, in ONE
transaction: the run's terminal transition (owner/lease/claim cleared,
`terminal_at_ms`, error pair for `failed`), the dispatch's
`completed`/`failed` transition, the final assistant item's terminal
lifecycle with its settled body/phase, AND the turn's terminal
lifecycle (`completed`/`failed`) — each with its patch — satisfying the
schema's documented co-commit obligation (`m20260824_000003:23-24`) and
leaving no Pending/Active turn behind a settled run. These are NEW
fenced operations; the accepted LEASED-only
`complete_message_dispatch`/`fail_message_dispatch` are not loosened or
called. Acceptance: contiguity/gap/duplicate-sequence rejection, digest
replay vs conflict, paired rollback (a failing item or turn write rolls
back receipt+checkpoint+dispatch stamp), generation/owner/lease
fencing, patch bounds (65,536-byte body, 4,096-byte fragment,
`bounds.rs:80-93`), turn transition sealing and identical-replay
harmlessness, terminal shape CHECK compliance, and the
recorded_at/revision mapping round-tripping through the E5 reader.

**E3 — conservative startup reconciliation (database + backend).**
Depends on E2. Two custody layers precede any sweep authority, because
opening the database proves nothing about other processes:
`ForgeApp::start` → `ForgeStorage::open` only connects and migrates
(`app.rs:52-58`, `storage.rs:31-43`), and the accepted connection
policy is a WAL-mode pool with a busy timeout
(`database/src/connection.rs:14-18,139-152`) — WAL admits concurrent
processes, so a second Forge on the same file is not excluded by
construction.

- *In-process* exclusivity IS establishable, by assembly ordering
  alone, and is a stated E6 requirement: the sweep runs after
  `ForgeApp::start` and before any dispatch worker, delivery owner, or
  accepted connection exists, so no in-process competitor can hold work
  while it runs.
- *Cross-process* exclusivity is a named prerequisite with an evidence
  gate (§5), and it must be acquired BEFORE open+migrate: migration
  already writes, so custody obtained after `ForgeApp::start` arrives
  too late. The mechanism is an OS-level advisory lock held for the
  whole process lifetime (a new dependency root selects from pinned
  source; none is invented here). SQLite's own exclusive locking mode
  is NOT that mechanism: it is unproven in-tree (`connection.rs:9`
  imports prove only sibling option types exist), it engages only
  while opening — after custody is already required — and an unproven
  exclusive-open is not an acceptable fallback. Until the chosen
  mechanism is verified, E6 assembly is BLOCKED (§5).

The sweep design remains PROPOSED, and its fences stay mandatory
defense in depth in every mode. With the custody lock verified and
held, no live same-database owner can exist, so the startup sweep may
treat every non-terminal pair as immediately eligible. Without custody
(tests and development only, since E6 is blocked), the sweep must be
fence-safe against a possibly live owner instead of assuming one
cannot exist, and eligibility narrows to: only
run/dispatch pairs whose dispatch lease has EXPIRED at the sweep's
operated time (`lease_expires_at_ms <=` now — the same eligibility
notion the accepted claim SQL uses, `message_dispatch.rs:31-38`); pairs
with an unexpired lease are left untouched and re-examined by a
deferred pass once their expiry has passed. Lease expiry proves neither
process death nor prompt non-execution — it only bounds how long the
store waits before durably recording that the outcome is unknown. Each
disposition is one fenced transaction of full-snapshot conditional
updates: run → `interrupted` with a typed bounded error pair naming the
unknown outcome, owner/lease/claim cleared, **provider binding tuple
retained** (the schema permits this: `:459-479` constrains only
owner/lease/claim for interrupted, and `terminal_at_ms` stays NULL,
`:448-458`); dispatch → `failed` with a bounded reason; AND the
conversation view sealed truthfully — the turn and the assistant item
(when one exists) move to `interrupted` with their lifecycle patches
and counter advancement, so a restarted client renders the
interruption instead of a forever-Pending turn. Interrupted stays
deliberately non-terminal and resumable in both the domain
(`conversation.rs:254-261`) and the run schema. A zero-row fence means
the row moved — possibly a live owner progressing — so the sweep skips
it with a typed note and never retries blindly. If a genuinely live
owner is swept this way, its next E2 commit fails its owner/lease
fence with a typed error, so no CONFLICTING DURABLE WRITE lands; that
failed local fence proves nothing about the external provider turn,
which may still be running and whose effects remain unknown — the
sweep only records that unknown honestly. Requeue stays structurally
wrong: the
permanent unique origin indexes (`:489-513`) guarantee a relaunch of
the same message would conflict, so the only consistent disposition is
terminal-for-the-dispatch, interrupted-for-the-run, preserving durable
provider identity for a later, separately approved resume capability
(generation + 1). The sweep never contacts a provider and never
deletes anything; startup-time `launching` rows (spawn outcome
unknown) receive the same disposition under the same lease-expiry
eligibility. Acceptance: sweep idempotence, expired-only eligibility
(unexpired rows untouched), deferred-pass coverage, pair-plus-
conversation atomicity, binding retention, zero-row skip behavior, no
touch of terminal or queued/leased rows.

**E4 — native engine process owner (backend).** Interface can be frozen
in parallel with E2/E3; implementation gated on the dependency decisions
in §6. One backend-owned, caller-configured executable/argv child per
active run via `tokio::process` (feature already declared,
`backend/Cargo.toml:18`), independent of any editor connection. One
operation permit per run, owned through observed child exit and pipe-task
completion: malformed/oversized output, timeout, permission failure,
cancellation, or owner shutdown fails the operation and initiates owned
termination/reaping, and never releases the permit while the child is
known alive; terminal OS-cleanup failure is explicit and blocks silently
starting a replacement run. Readiness follows the legacy contract as
behavioral evidence (single strict JSON stdout line, bounded length and
deadline, loopback endpoint + secret, drain both pipes —
`service.ts:95-137`), re-expressed natively; the parent-side custody
contract already written down for the directory helper
(`directory_helper.rs:14-24`: exact child handle, sole stdin writer,
generation assignment, deadlines, cancellation, kill, reap) is the
in-tree custody precedent to follow. Observation normalization converts
provider events into bounded typed observations (text fragments within
the 4,096-byte patch bound, phases, terminal outcomes) and normalizes
external failure text into bounded catalog-safe diagnostics — raw
credential-bearing HTTP/SSE frames never reach storage or errors.
Acceptance runs against a deterministic test-only fixture child through
the real owner: readiness deadline, malformed/over-budget frames,
backpressure, text-then-terminal, abrupt exit, cancel/drop, real PID
reap, cleanup-before-permit-release, no orphan reader. Windows
descendant/orphan containment is a separate, explicitly gated fixture
set (§6); direct-child success is not tree proof.

**E5 — conversation projection reads and post-commit delivery
(database + backend + transport).** Reads depend only on the accepted
schema and can start immediately; delivery depends on E2. No new
envelope vocabulary is needed (`types.rs:740-757`).

*Repository readers.* Bounded conversation snapshot (window/range per
`ConversationQueryBounds`, ceiling 512 turns) and patch replay batches
(≤ 64 patches, `bounds.rs:88-93`) reconstructed from durable
`conversation_patches` rows — the patch table IS the outbox; delivery
is read-after-commit, never a dual write. The reader materializes
domain patches through the E2 stamp mapping (lifecycle/append
`updated_at` from `recorded_at_ms`, revision from the row's revision
column). `RequestHandler` gains real `ClientRequest::Conversation`
answers (query/subscribe/unsubscribe), replacing the unbacked failure
at `request_handler.rs:138-140`; a fresh subscription answers
snapshot-first (`ConversationSubscriptionStarted::Fresh`), resume
validates the cursor and replays, and an invalid cursor produces the
explicit resnapshot requirement per the scope decision.

*Two ordering vocabularies, kept separate.* `WireEnvelopeBody::
PatchBatch` (`types.rs:756`) carries the domain `PatchBatch`, whose
PER-THREAD `from_cursor`/`to_cursor` contiguity is validated by its own
constructor (`conversation.rs:707-757`). `WireEnvelopeBody::Event`
carries `ServerEvent` with a PER-SESSION `EventCursor`
(`types.rs:612-619`) validated by `EventSequenceTracker::accept`, which
takes only an `EventCursor` (`event_sequence.rs:77`) — the tracker
orders Event frames and nothing else, and is never applied to patch
batches. First-workflow delivery sends ONLY PatchBatch frames: the
domain `Event` vocabulary today carries project/thread/queue facts
(`events.rs:34-43`) that the single local client already learns from
its own responses, so Event frames stay reserved vocabulary and the
tracker stays unused until a capability needs them. Client thread-
ordering rule: per subscribed thread the client holds one applied
`ConversationCursor`; a batch applies iff its `from_cursor` equals it;
anything else discards the batch and re-subscribes fresh (resnapshot).
The per-thread cursor is durable ordering and survives reconnect; the
per-session tracker (when used) is connection-scoped by its own
contract (`event_sequence.rs:1-10`).

*Delivery stream credit — accepted vs proposed.* ACCEPTED: the shared
transport configuration grants peers ZERO unidirectional-stream credit
(`MAX_INCOMING_UNI_STREAMS = 0`, `endpoint.rs:46-51`), and
`transport_config` (`:71-85`) is applied to BOTH the server
configuration (`:110`) and the client configuration (`:162-164`). A
server-initiated delivery stream therefore CANNOT be opened against
today's client endpoint; delivery does not work until this policy
explicitly changes. PROPOSED (this packet, no new envelope, no other
transport constant touched): split the single shared constant by role —
the server-side configuration keeps 0 incoming uni streams (clients
never open uni streams toward Forge), the client-side configuration
grants exactly ONE concurrent incoming uni stream, the delivery
stream. The existing per-stream and per-connection receive windows
(`:53-58`) already bound delivery memory and stay unchanged.
Acceptance: the server endpoint still refuses a client-initiated uni
stream; the client endpoint admits exactly one live server-initiated
uni stream and does not admit a second concurrent one while the first
is open; bidirectional request behavior is unchanged.

*Server ownership.* Per admitted connection, ONE backend-owned
`SessionDelivery` value created by the connection driver after
Welcome. It owns: the connection's single server-initiated
unidirectional delivery stream — opened together with the FIRST frame
it actually writes (the first activated subscription's first batch),
so peer visibility is carried by that real write; no dummy or control
frame exists, and nothing assumes `open_uni` alone is peer-visible
before a write — the subscription table (thread → activation state and
last published per-thread cursor), one bounded outgoing queue (the §5
queued-delivery budget), and one writer task. Frames reuse the
existing envelope seam — `send_envelope` takes a plain
`&mut SendStream` (`application.rs:49-56`) — and envelope stamps
follow the established injected-metadata policy (`connection.rs:
84-103`): the driver supplies server frame identities and timestamps.
Subscription registration flows through a narrow per-connection
registrar injected into that connection's `RequestHandler`
(constructing one handler per connection is cheap — `Repository` is
`Clone`, `repository/mod.rs:139-141`): a successful Subscribe
registers the thread as Pending inside dispatch, and the driver
ACTIVATES it only after `respond_next` returns, proving the
snapshot-first response was written before the first batch is
enqueued. Publication is durable-cursor driven; notifications are only
wakeups, never the source of truth. On activation the owner
UNCONDITIONALLY runs one replay pass from the exact cursor the
subscription response declared (the snapshot cursor for Fresh, the
validated resume cursor otherwise), so any commit that landed between
the handler's snapshot read, Pending registration, and activation is
delivered without any notification having fired. The subscription
response carries the snapshot exactly as the handler read it: a
commit after that read never rewrites the already-formed response —
it arrives only as an ordinary batch from the declared cursor, so the
snapshot/activation rule and the bounded early-batch rule stay
coherent. Thereafter the owner
loops with lost-wakeup-safe ordering: register interest in the
process-wide coalescing per-thread notifier (tokio's `sync` feature is
already declared, `backend/Cargo.toml:18`) FIRST, then re-read
committed patches after the thread's published cursor through the
bounded replay reader; publish anything found and repeat; await the
signal only after a recheck that found nothing. A coalesced or dropped
wakeup is therefore harmless — the next register-then-recheck pass
observes the durable tail. The committer never blocks on delivery and
the request path never blocks on the queue.

*Lifetimes and failure.* Unsubscribe removes the thread from the
table; already-enqueued batches may still flush, and the client
ignores batches for unsubscribed threads. Queue overflow, send
failure, cancellation, or connection close terminates delivery for the
whole connection: the writer task ends and the send side is reset
following the existing stream-guard discipline (`connection.rs:19-29`);
dropping `ForgeConnection` already closes the connection. Delivery
termination is never silent success, and it is TERMINAL for that
connection's delivery: the one delivery stream and its one receiver
cannot be revived. The client observes stream end/reset as a typed
delivery-lost condition; recovery is a FRESH `ClientSession::connect`
(using the rotated reconnect credential), one immediate
`take_delivery` on that new session, then fresh Subscribe requests
resuming from the durable per-thread cursors while the new receiver
pends for the server's first frame — never a resubscribe over the
dead stream or dead session.

*Client ownership — settled acquisition interface.* `ClientSession`
privately owns its endpoint and connection and exposes no raw getter
(`client_session.rs:263-296`), so a receiver cannot be obtained beside
the session; the session itself must hand it over, exactly once — but
it must NOT wait for the server while doing so. The server opens the
delivery stream only with its first activated batch, so a consuming
acquisition that awaits the incoming stream before any Subscribe can
be sent would deadlock startup. The settled interface is therefore a
non-awaiting split in the session's accepted consuming-owner idiom
(`request`/`shutdown`, `client_session.rs:444,524`):
`take_delivery(self) -> Result<(ClientSession, DeliveryReceiver),
ClientSessionError>` performs NO network I/O and returns immediately —
it marks the one-shot flag and moves into the receiver the private,
narrow means to later accept exactly one incoming unidirectional
stream (an internally held connection handle; no connection,
endpoint, or raw-stream API is exposed on either value, and no
credential state reaches the receiver). Any SECOND `take_delivery` is
a typed error that consumes the session — the accepted conservative
rule that every session-local error is terminal
(`client_session.rs:29-44`) applies unchanged. All waiting lives
inside the receiver, and each receive is CONSUMING in that same
accepted style, because the accepted framing is not resumable:
`receive_envelope` → `read_next_frame` consumes the first prefix
byte, the remaining prefix bytes, and the body across separate awaits
holding only future-local buffers (`application.rs:68-73`,
`frame.rs:101-148`), so a future abandoned mid-frame has already
eaten bytes and the stream is desynchronized. The settled operation
is `recv(self, cancel) -> Result<(DeliveryReceiver, frame),
DeliveryLost>`: the receiver comes back ONLY alongside a successfully
decoded frame; on first use it awaits the single `accept_uni` —
resolved only when the server's first real frame opens the stream —
then reads exactly one envelope frame, with the inbound-stop guard
installed before the first frame await (the accepted stream-guard
discipline). It is polled by its own dedicated task, concurrently
with and independently of the sequential consuming request API, so a
quiet subscribed thread with no next patch pends harmlessly and never
delays Subscribe or any other request. Abandonment is terminal in
BOTH phases, with different wire meaning: PRE-ACCEPT (still awaiting
`accept_uni`) cancellation or drop has consumed nothing — the wire is
clean — but the moved-in receiver is gone with the future, so
delivery is lost; IN-FRAME cancellation, drop, or any read/decode
error means bytes may already be consumed — the guard stops the
inbound direction synchronously and that desynchronized stream is
NEVER read again, so silent reuse is impossible. No cancelled,
abandoned, or failed `recv` ever yields a reusable receiver, and no
persistent partial-frame state is introduced. Custody invariants: at
most one receiver per session lifetime, ever — no re-acquisition;
every terminal path in either phase surfaces as the one typed
delivery-lost condition, whose only consequence is the fresh path
(new `ClientSession::connect` → `take_delivery` → subscribe);
dropping the receiver value behaves identically to abandoning its
current phase; none of this closes the session, while dropping or
shutting down the session closes the connection, which a pending or
active `recv` reports as delivery-lost. Startup is therefore
cycle-free: connect → `take_delivery` (immediate) → spawn the
receiver task → Subscribe → server activates and writes the first
batch → the pending accept resolves.
The frontend
service (E7) owns both values: it drives the receiver's read loop off
the GPUI thread, forwards owned decoded frames through the established
`async-channel` boundary (per `docs/PLAN.md` frontend dependencies),
and owns per-thread subscription state — it routes an applied batch
only for a thread whose subscription start it has already applied,
holds early-arriving batches for a pending-subscribe thread up to the
same queued-delivery budget (QUIC gives no cross-stream ordering, so a
batch can physically arrive before the Subscribe response), and treats
overflow or an unroutable batch as delivery-lost. Every delivery-lost
recovery and every reconnect is the same fresh path: new
`ClientSession::connect`, one immediate `take_delivery`, spawn the
new receiver task, then Subscribe resume at the durable per-thread
cursor — consuming durable state only, never reviving a dead stream
and never spawning a replacement engine.

*Acceptance (loopback, implementer-checkable).* Startup completes
cycle-free: connect → `take_delivery` returns without any network
wait → Subscribe succeeds while the receiver is still pending → the
first activated batch resolves the pending accept, and the wire
carries NO delivery frame before that first real batch (no
dummy/control frame). Fresh subscribe yields the snapshot response
before any batch for that thread (activation rule); a commit landed
between the snapshot read and activation never mutates the
already-formed response and is delivered by the unconditional
post-activation replay with the notifier disabled in that test; a
dropped/coalesced wakeup while the owner is mid-pass is recovered by
the register-then-recheck loop; committed batches arrive contiguous
with `from_cursor` equal to the applied cursor each time; an injected
gap/duplicate/regression triggers the client resnapshot path; resume
with a stale or invalid cursor is answered with the explicit
resnapshot requirement; queue overflow terminates delivery typed, and
recovery requires a fresh connection, fresh receiver, and fresh
subscribe that together restore the full transcript; unsubscribe
stops publication and late batches are ignored; ordinary requests and
further Subscribes proceed normally while a quiet connection's
receiver still pends with no delivery stream in existence;
`take_delivery` yields exactly one receiver and a second call is a
typed terminal error; cancelling or dropping a PRE-ACCEPT `recv`
consumes nothing on the wire, leaves the session usable, and is the
typed delivery-lost outcome recovered by the fresh path; causal
in-frame tests write one prefix byte (and, in a second case, a full
prefix plus a partial body), then cancel/drop the in-flight `recv`
and prove the inbound direction was stopped, no further read of that
stream ever occurs, the outcome is the typed delivery-lost condition,
and a fresh connection + receiver + subscribe recovers the full
transcript; the client endpoint admits exactly one live
server-initiated uni stream while the server endpoint still refuses
client-initiated uni streams; receiver drop — before or after the
stream exists — stops delivery without killing the session, and
session shutdown terminates a pending or active `recv` typed;
delivery-owner drop leaves no half-open stream; neither
the committer nor `respond_next` blocks on a full delivery queue; no
delivery work runs on the GPUI thread. The
`SessionDelivery`/`DeliveryReceiver`/`take_delivery` interfaces and
the role-split uni-stream credit are settled here; root freezes exact
names/paths and integrates them across the backend, transport, and
frontend packets. One verification note: at the §5 interface freeze
the implementer confirms the pinned quinn's `open_uni`/`accept_uni`
yield the same `SendStream`/`RecvStream` types the envelope seam
takes, and the connection handle's clone semantics relied on inside
`take_delivery`.

**E6 — Forge process assembly.** Depends on E1–E3 and E5 interfaces,
and is BLOCKED until the §5 cross-process custody gate resolves:
custody is a startup prerequisite, not later hardening. Replace the
`run()` stub with this exact startup order — the ordering is what
makes E3's custody assumptions true by construction: injected
data-directory policy → acquire the OS-level cross-process custody
lock (fail closed on conflict; nothing is opened or migrated without
it) → `ForgeApp::start` (open + migrate) → E3 reconciliation sweep →
loopback endpoint via the existing
`server_config`/`bind_loopback_server` (`endpoint.rs:96,253`) → launch
handoff per the approved scope decision (exact endpoint + pinned
certificate + one-time capability into `CredentialAuthority::new`,
`credential_authority.rs:95-105`) → accept loop composing
`ForgeConnection` with one per-connection `RequestHandler` plus
subscription registrar and one `SessionDelivery` owner (E5) → ONE
dispatch worker driving claim → payload → launch → E4 spawn/session →
E1 bind → prompt → E2 commits → terminal pairing, running E3's
deferred lease-expiry pass before claiming new work, and minting
run/turn/item/patch identities and run credentials through the
existing `CommandOrigin` idiom and `getrandom` capability pattern — no
second token or origin service. Graceful shutdown: stop admissions,
resolve or interrupt the active run through E3's disposition, close
storage via `ForgeApp::shutdown`, map typed failures to exit codes.

**E7 — frontend client service, projection, transcript render.**
Protocol/transport client pieces are accepted; this packet may proceed
against the deterministic test event source permitted by the scope
decision (fake engines never ship as proof). Off-UI-thread client
service owning `ClientSession` (sequential; reconnect = new session +
resume cursor, consuming durable state, never spawning a replacement
engine); conversion of subscription snapshots/patches into the existing
transcript/composer/attention models; native transcript rendering
through the `ui` markdown seam per the plan's streaming rules; composer
submission wired to the real `QueueFirstMessage`/receipt path.

**E8 — packaged first-workflow proof.** Depends on E6/E7. First Bazel
packaging target (none exists today): relocatable deterministic portable
archive of both binaries + declared resources; editor-owned sibling
Forge launch with readiness, authenticated handoff, bounded shutdown,
orphan containment; clean-host workflow: attach → thread → message →
real assistant turn → restart both processes → recover project, thread,
user message, final assistant message, and observable run state.

Disjoint ownership: E1/E2/E3 are serial in `modules/database` (same
repository module); E4 (backend process owner), E5 readers, and E7 can
proceed in parallel once E1/E2 public shapes and the E5
`SessionDelivery`/`DeliveryReceiver` interfaces (designed above) are
frozen by root. Root owns every shared boundary: `repository/
mod.rs`/`lib.rs` export merges, integration of the E5 interfaces
across transport/backend/frontend, backend assembly, manifests/locks,
and all dependency additions.

## 5. Decision register

**User/product decisions required (block only the packets named):**
1. Shipping engine executable, certified version, and installation
   custody for the native product (legacy certifies `0.0.0-beta-17778`;
   that pin is legacy-product evidence, not a native decision). Blocks
   E4 real-engine configuration and E8's real proof; E4's fixture-driven
   implementation proceeds without it.
2. Provider/model/credential source and acquisition path. Never chosen
   silently; blocks the real proof only.
3. Permission/tool policy for the first real turn (legacy evidence shows
   explicit permission requests exist; auto-accepting is forbidden
   without an explicit policy). Blocks the real proof.
4. Runtime budget approval (below). Implementation uses injected limits;
   values are fixed at assembly.
5. Forge data-directory location and the concrete local handoff channel
   (scope decision fixes the handoff's shape, not its mechanism). Blocks
   E6 final assembly.

**Proposed runtime budgets — visibly proposed, NOT adopted; owner: root
with user approval; acceptance: each limit injected, tested at its
boundary, and recorded in the packet that enforces it.** Readiness line
64 KiB / 15 s and control-request deadline 10 s (both grounded in legacy
source, `service.ts:114-125,140-150`); one HTTP response or SSE event ≤
8 MiB; queued delivery ≤ 8 events and ≤ 8 MiB; per-run observation
intake ≤ 32 MiB / 4,096 events; overall run deadline 30 min; cleanup
escalation report at 5 s (a reporting bound, not a termination
guarantee). The non-legacy numbers are research guesses and carry no
authority until approved. Long-running-turn policy (what happens at the
run deadline) is a product decision, not a timeout default.

**Evidence/implementation gates (resolve with pinned source or a spike,
never by assumption):**
- HTTP/SSE client crate for the native engine adapter: no such
  dependency exists in the native graph; root owns the manifest/lock/
  Crate Universe change after evaluating candidates.
- Cross-process database custody mechanism for E3/E6: an OS-level
  advisory lock (a lock-file crate — a new dependency) acquired BEFORE
  open+migrate and held for the whole process lifetime. Root selects
  the crate after inspecting pinned source. SQLite exclusive locking
  mode is explicitly NOT the mechanism (unproven in-tree —
  `database/src/connection.rs:9` imports prove only sibling option
  types exist — and it engages only during opening, after custody is
  already required); it may be evaluated later as additional defense
  only. Until the mechanism is verified, E6 assembly is BLOCKED; the
  E3 sweep outside custody remains a fence-safe lease-expiry-eligible
  design for tests and development.
- Quinn unidirectional streams for E5: `send_envelope`/
  `receive_envelope` already take the plain `SendStream`/`RecvStream`
  types (`application.rs:49-73`); the implementer confirms the pinned
  quinn's `open_uni`/`accept_uni` yield exactly those types at E5
  interface freeze — a verification step, not a new dependency.
  The accepted shared transport config currently grants ZERO incoming
  uni-stream credit to both roles (`endpoint.rs:46-51,71-85,110,
  162-164`); E5's proposed role-split credit (client 1, server 0) is
  frozen as part of the same interface freeze.
  The same freeze also verifies the pinned quinn connection handle's
  clone semantics relied on inside `take_delivery`.
- Safe-Rust Windows descendant/orphan containment: workspace forbids
  first-party unsafe (`Cargo.toml:74`); a Job-Object wrapper crate (or
  an explicit decision to accept direct-child-only containment for the
  proof) requires inspection and real fixtures. The earlier
  "exact tree termination on drop" claim remains withdrawn.
- Native-launch engine stdout/readiness contract for the pinned engine
  in `serve --stdio` mode, and its stdin-EOF semantics: verified only
  against legacy TypeScript expectations, not against a running binary.
- Provider prompt idempotency: deterministic prompt IDs are a
  mitigation, not proof; no idempotency guarantee may be encoded until
  demonstrated against the certified engine.

## 6. Rejected alternatives

- **Run-only binding update (no dispatch write in E1):** rejected — it
  loses the accepted first-statement writer serialization on the shared
  row pair and leaves later observation commits without a coherent
  advancing stamp to fence; the pairing also proves the claim/lease
  chain is intact at bind time.
- **Extending `LaunchedRunReceipt` with a launch timestamp:** rejected
  for E1 (reopens accepted API and tests); legitimate later cleanup.
- **Requeue-on-recovery:** rejected — contradicts the permanent unique
  origin indexes and risks a second provider prompt for one message.
- **Loosening LEASED complete/fail/requeue to accept RUNNING:** rejected
  — those predicates are accepted behavior with tests; RUNNING
  settlement needs both-row pairing they cannot express.
- **A second event envelope or streaming vocabulary:** rejected — the
  protocol already carries `Event`/`PatchBatch`/subscription responses.
- **Shipping the deterministic event source as the proof:** rejected by
  the scope decision; it remains test/harness-only.
- **Ordering patch batches with `EventSequenceTracker`:** rejected —
  the tracker admits only per-session `EventCursor`s
  (`event_sequence.rs:77`); patch batches carry their own per-thread
  contiguity, and conflating the two would make an ordinary reconnect
  (which resets session cursors) look like a thread gap.
- **Interleaving delivery frames on request streams:** rejected — the
  accepted dispatcher validates exactly one correlated reply per
  stream (`server_dispatch.rs:1-26`); a dedicated server-initiated
  delivery stream preserves that contract unchanged.
- **Treating database-open or lease expiry as proof of old-owner
  death:** rejected — the accepted WAL pool admits concurrent
  processes and expiry is a wait bound, not evidence; custody requires
  the explicit §5 mechanism, and until then only the fence-safe sweep.

## 7. Opt-in real-engine proof (named prerequisites, not run here)

Prerequisites: user decisions 1–5 above, root-approved dependency gates,
and an explicitly caller-approved executable/version/project/model/
provider/credential source. Observable success criteria: one real prompt
produces a durable `assistant_message` item reaching phase `Final` and
lifecycle `completed`, a `completed` run row with retained binding and
`terminal_at_ms`, a `completed` dispatch, contiguous batch receipts whose
digests replay as `AlreadyCommitted`; the transcript renders the text
through the native markdown seam; killing and restarting both processes
recovers the identical transcript and run state from the database alone;
and killing Forge mid-turn then restarting yields the E3 interrupted
disposition — with turn, item, run, and dispatch all sealed coherently
and no automatic re-prompt — immediately, because an assembled product
exists only after the §5 custody mechanism has landed and the restart
holds the custody lock before opening the database. No such experiment
was authorized or executed for this document.
