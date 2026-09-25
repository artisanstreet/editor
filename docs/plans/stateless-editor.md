# Stateless Editor, single host connection, and connection holds

- Status: approved 2026-09-25; implementation in progress (steps 0, 2, 3, 4 and 5 implemented)
- Scope: where Editor state may live, how the Editor connects to exactly one Forge, and how
  in-flight work keeps that connection open until it resolves

## Outcome

The Editor is a stateless, functional client. Every piece of domain and user-work state lives in
the Forge; the Editor renders a projection of Forge-delivered data plus ephemeral view state. The
machine selector chooses the one Forge the Editor is connected to: switching seals the current
connection, waits for its in-flight work, discards all host-scoped state, and connects to the new
host. Editor-only customization (for example the FPS limit) persists beside the Editor
installation in its own storage pool.

## 1. Storage pools

| Pool | Location | Holds | Rule |
| --- | --- | --- | --- |
| Editor pool | `<install root>/editor/settings.json` (beside `versions/`, survives updates and rollback; dev and release roots are separate) | Device/window-local presentation: FPS limit, FPS overlay, and future theme, font size, layout, keybindings, window geometry, motion, desktop notification behaviour; the "reopen last host" hint | Would be wrong to follow the user to another machine, or is needed before any Forge connection exists |
| Forge pool | Forge database | Drafts and attachments, sends and outbox, default model, project order, last route and last thread, favorites, engine settings, account profile, every business decision | Everything else. When in doubt it belongs to the Forge |

Connection bootstrap material (host invitations, TLS pins, reconnect capabilities, install
manifests) is neither pool. It stays in the credentials module with its own permissions.

### Editor pool contract

- One typed `EditorSettings` value, schema-versioned, every field defaulted.
- Loaded once at startup into a GPUI global. Values are immutable: `update(|settings| ...)`
  produces a new value that is applied, then persisted.
- Atomic persistence: write a temporary sibling file, then rename.
- Unknown keys are preserved on write so older and newer Editor builds can share the file.
- Only the settings module may write it. An architecture test fails if Editor code writes files
  outside the settings module and the credentials module.
- Migration: `ui/frame-rate-limit`, `ui/fps-overlay` and `ui/last-used-host` are imported once and
  then removed.

## 2. Connection holds

`ConnectionHolds` is an async counting lock. Any number of holds exist independently and release
in any order. The aggregate status is decided only by the live count: `Busy(n)` while `n > 0`,
`Idle` at zero. Nobody waits on an individual hold.

```rust
pub struct ConnectionHolds { state: watch::Sender<HoldState> }
pub struct HoldState { pub count: usize, pub sealed: bool }
pub struct Hold { /* Drop releases */ }

impl ConnectionHolds {
    pub fn try_hold(self: &Arc<Self>, kind: HoldKind) -> Option<Hold>; // None once sealed
    pub fn status(&self) -> HoldState;                                  // synchronous, for UI
    pub fn subscribe(&self) -> watch::Receiver<HoldState>;              // async observers
    pub async fn idle(&self);                                           // resolves at count == 0
    pub fn seal(&self);                                                 // refuse new holds
    pub fn unseal(&self);                                               // cancelled switch
}
```

- Release is `Drop`: impossible to forget, safe on panic and cancellation, no double release.
- A hold is acquired when a mutating command is admitted (before it enters the command queue),
  moves into the in-flight record keyed by request id, and drops when that request's reply arrives
  or when it fails definitively (timeout or connection loss). Every hold is bounded by its request
  timeout, so `idle()` always resolves.
- `HoldKind` labels holds for the UI ("Saving draft and 2 messages"); only the count decides
  status.
- Holding commands: `QueueFirstMessage`, `QueueMessage`, mutating `ComposerState`, `StopRun`,
  `RespondApproval`, `RespondQuestion`, `CreateTask`, `BeginProjectIntake`/`At`/`Retry`,
  `SetThreadEngineConfig`, `SetModelFavorite`, `SaveComposerDraft`. Reads, subscriptions and
  acknowledgements never hold.
- Drafts: each keystroke advances the thread's draft revision. At most one save is in flight per
  thread; newer text replaces the unsent body (latest wins). The thread's hold drops only when the
  acknowledged revision equals the latest sent revision.

Implemented (step 2): `native_transport_service/connection_holds.rs`. The service loop is serial and
awaits each Forge reply inside its handler, so the in-flight record is the queued command itself:
`submit` takes the hold on admission, the hold travels with the command through the bounded queue,
and it drops when the handler returns or fails (a refused or never-processed command drops it
too). `NativeTransportCommand::hold_kind` classifies every command exhaustively; the holding set
is the list above plus `UploadComposerAttachment` (both draft commands hold as `HoldKind::Draft`),
and among `ComposerState` commands only `WithdrawQueuedMessage` mutates. A sealed refusal
surfaces as `CommandSendError::Busy`, so callers keep their retryable state for a cancelled switch.
`HoldState` also counts holds per kind for progress copy.

Application-level holds: a message flight holds from admission until its correlated receipt or
failure arrives, the flight ends, or the service stops. The first send's `SetThreadEngineConfig`
and `QueueMessage` are admitted in the same UI turn, so their transport holds already cover the
pair. A send waiting for account readiness has not been admitted: sealing cancels it and the draft
stays in the composer. `BeginProjectIntake` holds while its native directory picker is open, which
is bounded by the user rather than a request timeout.

## 3. Single host connection

The Editor owns exactly one connection. `NativeWorkspace` with one `NativeApplication` per host is
removed. Host-scoped fields of `NativeApplication` move into one `HostState` value that is dropped
and rebuilt on a switch.

Switch sequence:

1. Sealing: `seal()`, composer read-only, the machine menu shows what is still saving.
2. Drained: `idle()` resolved; unsubscribe, session shutdown, reconnect-lease publish.
3. Reset: drop `HostState`.
4. Connecting, then Ready on the new host.

Re-selecting the current host while sealing unseals and cancels the switch. Quitting the Editor
uses the same seal and drain.

Implemented (step 3), with one deviation: instead of extracting a `HostState` value from
`NativeApplication`, the `NativeApplication` entity itself is the host-scoped state and is rebuilt
for each connection. Nearly every one of its well over a hundred fields is host-scoped (routes, projects,
threads, subscriptions, flights, caches, the composer and its drafts), so an extraction would move
almost the whole struct while every `impl_*` module rewrote its field paths; dropping the entity
drops everything it owns with no hand reset and no field that can be forgotten. `NativeWorkspace`
owns only what is not about a host: the window, the connected home, the switch transaction and
the connector seam. The sidebar and profile-menu disclosure carry over; settings globals live
outside the entity.

- Switch: `workspace.rs` seals through `NativeApplication::begin_host_switch` (composer
  read-only, progress such as "Saving 1 message to Ubuntu…" in the profile header and a window
  notice, repainted from `subscribe()`), awaits `idle()` on a background task, calls
  `prepare_shutdown`, requests shutdown and waits (bounded) for the service thread, then replaces
  the view entity and records the reopen-host hint. A selection during the drain retargets; the
  current host during `Draining` cancels; once `Disconnecting` the switch is committed.
- Quit: the workspace seals and returns its one service; `close_connection` waits up to 10 s for
  holds to drain while pumping the event bridge, then shuts down.
- Selecting the connected host still retries a failed or stopped connection in place.
- The host catalog is a presentation memo replaced wholesale on each credential-store refresh; no
  older invitation incarnation is retained.
- Drafts are Forge state since step 4: the old host's view drains its draft saves before it is
  dropped, and the new host's view reads its own drafts.

## 4. State that moves to the Forge

| Editor state today | Forge replacement |
| --- | --- |
| Composer drafts and undo history (`native_composer.rs`, `composer_draft_session_policy.rs`) | `composer_drafts` table per thread (revision, body, attachment refs), `SaveComposerDraft` / read with the thread snapshot |
| Attachment bytes held until send (`native_composer.rs`) and per-engine image policy | Upload to a Forge attachment store on attach; Forge owns image policy |
| `optimistic_messages`, `message_retry`, `pending_account_send`, `pending_failed_recovery` | Forge accepts a submission immediately, emits a pending transcript row, holds it until the engine is ready, retries by request id, `RecoverFailedMessageToNewThread` |
| Queue echo matching (`taken_up`, `retired_echoes`, `echo_watches`) and 5 s polling | Queue row state field pushed by subscription |
| `last-used-model` file | Per-user default engine configuration |
| Deferred model choice | Persist on selection |
| `project-orders` files, last thread per project, current route | Per-user navigation record |
| Process-global host catalog | Derived from the credential store on demand |
| `readiness/model-catalog.json` side channel | Catalog request over the protocol |
| Request ids minted from process counters and wall clock | Random UUIDv7 recorded with the Forge-side outbox or draft |
| Profile name from `USERNAME`/`COMPUTERNAME` | Account identity from the Forge |

Implemented (step 4), drafts and attachments:

- Forge: migration `m20260925_000015_composer_drafts` adds `composer_drafts` (one row per scope,
  Forge-assigned revision that may only grow, body, `updated_at`), `composer_draft_attachments`
  (ordered references) and `composer_attachments` (bytes stored once per SHA-256 digest, pinned by
  a restricting foreign key; unreferenced bytes are pruned after 24 h, long enough for a send that
  names them). The Forge is the revision authority: every save applies (the last to arrive wins)
  and gets the scope's next revision, which the acknowledgement reports. An emptied draft keeps
  its row, so its revision sequence never restarts. Deviation: the plan's client-proposed revision
  is dropped from the wire, because an Editor whose counter lagged the stored revision (a second
  Editor, or one whose first read failed) had its saves silently ignored; per-Editor ordering
  already follows from one save in flight per scope and the serial command loop.
- Protocol (`composer_state.capnp`): `SaveComposerDraft` → `ComposerDraftSaved { revision }`,
  `ReadComposerDraft`, `UploadComposerAttachment` → `ComposerAttachmentUploaded { reference }`,
  `ReadComposerAttachment`, and `QueueStoredMessage`, a message whose images are stored references;
  the Forge resolves them and admits it exactly like `QueueMessage` (same receipt, replay and
  dispatch). Deviation: the draft is read with its own request rather than with the thread
  snapshot, because snapshots are patch-replayed transcript projections and a per-keystroke draft
  does not belong in that ledger.
- Scope deviation: a draft belongs to a thread or to a project's new-task composer
  (`ComposerDraftScope::Project`), so the prompt of a task that does not exist yet also survives a
  switch. When that prompt moves into its newly created thread, the project scope's draft is cleared.
- Editor: the in-memory draft store is gone. The composer is a view: opening a scope reads its
  Forge draft (local typing that happens first wins), each authored change goes through a per-scope
  latest-wins chain (`composer_draft_sync.rs`: at most one save in flight, newer text replaces the
  unsent body), and ready images upload to the attachment store before the draft references them.
  Each busy scope holds the connection from its first unsaved change or upload until the
  acknowledgement of its latest sent save arrives, matched by a local per-scope send sequence;
  follow-up saves are admitted under that hold (`Hold::extend`,
  `NativeTransportService::submit_under`) even while a switch has sealed the connection. A switch
  captures the outgoing scope's body before the view rebinds, so a change typed in the same frame
  still reaches its own thread. An upload that completes while a switch drains is saved into the
  draft within the same event, under the upload's hold; quitting flushes unsent bodies, and
  references uploads still in flight by the digest of their bytes, queued behind them. Undo and
  redo remain local view history. Recalling a queued message fills the composer, which saves the
  Forge draft through the same chain.
- Sends: the transport sends a message by reference when every image is one this connection
  uploaded or read back, otherwise with inline bytes (recovered or retried payloads). Superseded
  in step 5: a message is sent by naming its stored draft revision.
- Left for later steps: the per-engine image policy still runs in the Editor's image preparation
  (step 6); the queue recall `restore_candidate` (a withdrawn payload waiting for an empty composer)
  is a recovery handshake rather than draft storage and moves with step 5. Known limit: two Editors
  on one thread each keep showing their own text; the last save to arrive is the stored draft,
  and the other Editor sees it on its next read.

Implemented (step 5), Forge-accepted submissions:

- Request ids: `RequestId::mint(label)` in `artisan_domain` mints `<label>-<UUIDv7>` (workspace
  `uuid` with `v7`) for every Editor-minted id (messages, saves, answers, model favourites,
  withdrawals, retries, recoveries), so ids stay unique across restarts and processes.
- Forge: an accepted message is a queued row with a Forge-owned delivery state
  (`QueuedMessageState::Queued | Dispatching`, the dispatcher's reason as `last_error`, and the
  engine of its accepted configuration snapshot). Deviation: the pending rows are delivered as a
  per-thread `MessageOutbox` event (queued listing + failed listing) on the existing subscription
  instead of as transcript patches, because the transcript ledger only holds delivered items and
  a message leaves the outbox in the same commit that projects it. The delivery driver pushes the
  outbox after the patches of every activation and wake, gated by a cheap fingerprint query, so an
  unchanged outbox is never resent; the dispatcher, retry, withdrawal and recovery wake it. The 5 s
  queue/failed polling is gone.
- Failed rows are decided by the Forge: a failure is offered until a later message of the thread
  is running or completed (this replaces the Editor's `hide_failures_before` pruning) or until it
  is recovered; `retryable` is true only when the message never reached the transcript.
- Retry: `RetryFailedMessage { target }` (thread, message id, original request id) re-queues the
  stored payload as a fresh send (lease, error and steer target cleared) and answers `Requeued` or
  `NotRetryable`; no payload crosses the wire. Recovery: one command, named `RecoverFailedMessage` rather than the plan's
  `RecoverFailedMessageToNewThread`; it creates a thread in the same project with the failed
  message's engine configuration, stores the payload as that thread's Forge draft (never sent),
  records the recovery (migration `m20260926_000016_failed_message_recoveries`) and answers the new
  thread id; a replay answers the same thread. Queue edit is `WithdrawQueuedMessage` with
  `recall_to_draft`: the Forge moves the withdrawn payload into the thread's draft and the Editor
  re-reads its draft, replacing `ReadRecalledMessage` and the `restore_candidate` handshake.
- Editor: removed `optimistic_messages`, `message_retry` (payload copy, `draft_matches`),
  `pending_account_send`, `pending_failed_recovery`, echo watches, `taken_up`, retired echoes,
  restore candidates and queue/failed refresh tokens. The message flight keeps only thread,
  request id and composer token under its connection hold. The transcript tail renders exactly the
  outbox rows ("Queued", "Waiting: <reason>", "Starting…"); the send entrance animation is view
  state keyed by the first appearance of a row's message id. A delivered turn's engine label comes
  from the engine on its outbox row. The composer stays locked while an edit recall is pending.
- Sending is idempotent on the draft (addition after review). The Editor never sends a message
  body: `SubmitComposerDraft { thread, draft_revision, steer }` names the revision the Forge gave
  the composer's body, and the Forge, in one transaction, reads the draft at exactly that
  revision, admits it through the queue-message admission (stored images resolved to bytes),
  records the submission under (thread, draft revision) (migration
  `m20260927_000017_composer_draft_submissions`) and empties the draft at the next revision. A
  repeat of the revision under any request id answers the first message as a duplicate and
  changes nothing, so re-pressing Send after a lost answer cannot queue twice; the request id is
  only a correlation id. Another revision is refused as data (`Stale { current_revision }`); the
  Editor keeps its text, saves it again and the user sends again. Send waits for the save that
  stores the body being sent (it is saved ahead of anything typed after Send, which is held
  until the send is on the wire), and the last revision the Forge reported per scope outlives a
  dropped connection. Draft saves are idempotent by request id (`composer_draft_save_receipts`),
  so a retransmitted save cannot write a sent draft back. The Editor no longer sends
  `QueueMessage`/`QueueStoredMessage`; both stay on the wire (general admission, frozen capnp
  ordinals). A send with images still uploading is refused with its reason.
- Left for step 6: the readiness verdict in `first_send_config` (catalog admission) still refuses
  an unrunnable first send in the Editor; it no longer holds the send. Run usage is still polled
  while a run is live.

Decisions that move to the Forge and arrive as data: send admission (typed refusals from one
`SubmitMessage`), account readiness, catalog readiness overlay, engine-config validation, context
compaction thresholds, thread title refinement, attachment image policy, failure-row pruning.

## 5. Forge resilience prerequisites

- A single failing request must not end the Forge serve loop; fail that connection only.
- The Forge prints the complete error source chain on exit.
- Importing a newer invitation for the same host identity removes superseded registrations.

## 6. Delivery order

0. Editor settings pool (independent).
1. Forge resilience prerequisites (independent).
2. `ConnectionHolds` and holds on every mutating command.
3. Single-connection workspace with `HostState` and the switch sequence.
4. Drafts and attachments on the Forge.
5. Forge-accepted submissions; remove optimistic, retry, pending-send and recovery state.
6. Business decisions to the Forge.
7. Preferences and navigation to the Forge; remove the last Editor-side domain files.
