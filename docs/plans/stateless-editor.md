# Stateless Editor, single host connection, and connection holds

- Status: complete (approved 2026-09-25; steps 0 to 7 implemented)
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
  an unrunnable first send in the Editor; it no longer holds the send (done in step 6). Run usage
  is still polled while a run is live.

Decisions that move to the Forge and arrive as data: send admission (typed refusals from one
`SubmitMessage`), account readiness, catalog readiness overlay, engine-config validation, context
compaction thresholds, thread title refinement, attachment image policy, failure-row pruning.

Implemented (step 6), business decisions to the Forge:

- Account readiness: the Forge judges each engine account when it serves a usage report
  (`account_readiness.rs`) against its own 180 s freshness window: authenticated and fresh is
  ready (also with a transient refresh failure served beside last-good), unauthenticated needs
  sign-in, anything else is not ready, and an engine never observed is still being checked. Each
  verdict carries a presentation-ready reason. `EngineUsageReport.readiness` carries it (a missing
  verdict decodes as not ready); `AccountUsageService::readiness` answers the current verdict
  from what the Forge last observed. The Editor renders the verdict; a read in flight without any
  report still shows Checking. Deviation: the verdict is served with each report rather than
  pushed; the Editor still schedules its usage re-reads by each report's age (refresh cadence,
  not the verdict).
- Catalog readiness overlay: the Forge applies readiness to every catalog it serves (Codex and
  Claude, whose usage read proves a local CLI, are runnable exactly when ready; other harnesses
  keep the catalog's marking). `catalog_with_usage_readiness`/`effective_catalog_snapshot` are
  gone; the Editor reads the catalog again when a report changes an engine's verdict.
- Model catalog side channel: `ReadHostCatalog` (answered by `hostCatalog`, live discovery with
  readiness applied) replaces `<root>/readiness/model-catalog.json`. The Editor asks once the
  connection lists its projects and every five minutes. The Forge's file publisher is removed:
  nothing else read the file, and a file beside the local readiness receipt could only ever
  describe the local host, not the connected one.
- Engine configuration: the Editor names the model it shows by catalog identities
  (`CatalogSelection`: model id, profile, reasoning/speed/context/permission option ids). The
  Forge resolves it against its catalog and the thread's saved configuration
  (`engine_selection.rs`, the former Editor `config_for_policy`, `with_default_native_profile`
  and run-choice validation) or refuses with a reason. Deviation: a selection-time save is a
  resolution query (`ResolveModelSelection` → `modelSelectionResolved`) followed by the existing
  `SetThreadEngineConfig`, so receipts, compare-and-swap and conflict handling stay one path. The
  picker's pure presentation stays in the Editor (`picker_selection.rs`): naming a displayed
  policy by identities, and finding the catalog row that displays a saved configuration by
  matching its values (it never builds one).
- Send admission: `SubmitComposerDraft` carries the displayed selection instead of a steer target
  (the retired `steerRunId` must be empty). The Forge refuses as data (`refused`, a
  `SubmissionRefusal` kind and message) a send while the run is still starting, an unconfigured
  thread without a model, a selection its catalog cannot build, and a selection that changes the
  configuration to an engine that cannot run (with its readiness reason); a selection equal to
  the saved configuration is admitted and held as before. A changing selection is saved before
  the message is queued, and the answer reports the configuration revision so the Editor re-reads
  its settings. The send steers the live run exactly when it runs or waits on the same engine. A
  repeated revision skips admission. `first_send_config`, `admit_first_send`, the starting-run
  guard and `observed_steer_target` are removed from the Editor.
- Compaction thresholds: `RunUsageResult.compactionAtTokens` (Forge `context_compaction_policy.rs`,
  same rules including the exact `claude-sonnet-5` case); the Editor paints the marker and
  defaults to the window. `context_auto_compaction.rs` is removed.
- Thread title: the listing title already is the Forge's resolved display title (the generated
  title once recorded, otherwise the first message while the placeholder stands). The Editor's
  `refined_thread_title`/`thread_display_title` re-derivation is removed. Deviation: no new wire
  field; the Editor shows the listing it re-reads every 1.5 s.
- Attachment image policy: the Editor keeps and uploads each image exactly as picked (it still
  decodes one for its thumbnail). The composer store holds picked images up to 12 MiB, one upload
  frame (`ComposerImage`, migration `m20260928_000018_composer_attachment_sources` rebuilds both
  attachment tables with the larger bound). When a draft is sent the Forge fits its images to the
  admitted engine (`attachment_policy.rs` with the moved `image_policy.rs`: 2576 px long edge, the
  engine's best encoding when rescaled or smaller, GIFs untouched, then the message bounds) and
  refuses what cannot fit (`AttachmentRejected`); the fitted images are what the submission
  queues. Deviations: the Editor's raw intake ceiling drops from 32 MiB to the 12 MiB upload
  bound, so an image between the two that the Editor used to shrink is now refused at intake;
  `QueueStoredMessage` (unused by the Editor) cannot send a stored image over the message bound.
- Left for step 7: run usage is still polled while a run is live, and the OpenCode settings
  editor still saves a configuration it assembles from the managed registry.

Implemented (step 7), preferences and navigation, and the step 6 gaps:

- User preferences: the Forge serves one account, so its preferences are a singleton (migration
  `m20260929_000019_user_preferences`): the default engine configuration, the last route (project
  and open thread) and a revision that grows with every change. `navigation_projects` keeps one
  row per used project with its recency (the revision at its last use, so the order needs no
  clock) and the thread last open in it; deleting a project or thread clears its references.
  Protocol: `readUserPreferences` (Request @42) and `recordNavigation` (@43) answer
  `userPreferences` (Response @41); `importLegacyPreferences` (@44) answers
  `legacyPreferencesImported` (@42). `UserPreferences` carries the default configuration, the
  projects in most-recently-used order with their last threads, the route, and the account.
- Default model: a configuration the user saves on a thread (`SetThreadEngineConfig`, or the
  selection a send saves) becomes the default; a thread without its own configuration shows it.
  Choosing a model writes nothing in the Editor.
- Navigation: the Editor reads the preferences before the project listing; a new connection
  orders its projects by the Forge's record, selects the last route's project and opens the
  thread last open in it. Every change is reported with `RecordNavigation` once, under a
  `Preferences` connection hold, so a report made before quitting or switching still lands.
- Legacy import: on its first read of a host's preferences the Editor hands the older Editor's
  `ui/last-used-model` (when the Forge has no default) and that host's project order (when the
  Forge has no record) to the Forge and removes the files once it answered. The Forge fills only
  what it lacks: the model is resolved against its catalog, the order is adopted only when no
  project was used yet, unknown projects dropped. The old fallback that wrote a project order into
  a host's credential folder is gone with `native_last_used.rs`; the file-write guard allows only
  `editor_settings/storage.rs` and the two opt-in development writers, pinned by a test.
- Account identity: the profile name and host come from the account the connected Forge runs as
  (`account_profile.rs`), and `ArtisanAccountIdentity` is installed from it; the Editor no longer
  reads `USERNAME`/`COMPUTERNAME` for them. The local machine tile's avatar seed stays the local
  machine's name: tile presentation shown before any connection, never the account. The host
  catalog memo (`native_hosts/catalog.rs`) is documented as a presentation cache of what the
  credential store says, holding no domain state.
- Pushes on the delivery stream: the Event union gains `accountUsage` @6, `userPreferences` @7,
  `threadRetitled` @8 and `runUsage` @9. The commit notifier carries a host-state revision beside
  its commit wakes; each delivery driver pushes every engine's usage with its readiness verdict
  (one snapshot per engine) from the connection's first request, preferences when they change
  after the Editor read them, a thread's display title when a wake finds it changed, and a live
  run's usage (with its compaction threshold) when it differs from the last push. The Forge owns
  the usage cadence: while a connection that made a handler request is open (an Editor; a
  lifecycle-only connection never counts, so it cannot hold a stopping Forge on a CLI read) it
  re-reads each engine a minute before its 180 s report goes stale. Removed from the Editor:
  every usage read that was not the user's (profile menu, machine list, Settings, thread
  selection, catalog load, picker retry, refused sends) and the five-second composer usage
  timer. The explicit refresh controls still force a read.
- OpenCode settings editor: the manual settings document is `ManualEngineConfiguration` in the
  domain; the Forge builds and validates a configuration from it
  (`engine_selection/manual.rs`). `resolveEngineConfiguration` (Request @45) answers
  `engineConfigurationResolved` (Response @43) with the built configuration or a refusal naming
  the first invalid field; the Editor saves exactly what the Forge built through the shared save.
- Picked images up to 32 MiB again: an image larger than one 4 MiB chunk uploads in chunks
  (`ComposerAttachmentChunk`, `UploadComposerAttachment.chunk`) keyed by the digest of the whole
  image under request ids derived from the stable one; the Forge keeps them in
  `composer_attachment_upload_chunks` (migration `m20260930_000020_chunked_composer_attachments`,
  which also raises the store bound to 32 MiB), assembles and verifies the image, and prunes an
  abandoned upload after the 24 h grace period. `ComposerAttachmentUploaded.pendingBytes` reports
  the bytes still missing. Reads name a window (`offset`, `maxBytes`, answered with `totalBytes`
  and `offset`, at most one chunk) and the Editor verifies the joined image against its digest.
  `QueueStoredMessage` fits its images to the thread's engine like a draft send, so it is no
  longer bound to 5 MiB per image.

Implemented (after step 7), recent threads in the sidebar:

- The sidebar no longer switches projects. Its project dropdown, the previous/next project
  buttons, project cycling and their focus handles are removed from the Editor; the new-task
  project picker on the home surface remains. The sidebar lists recent threads across every
  project of the connected Forge, and choosing one opens it in its own project.
- Forge: `Repository::list_recent_threads` reads one page of saved threads (assistant text
  started) across projects, newest activity first (latest message, else last update), bounded
  to 100 (`RECENT_THREADS_MAX`). Each row carries its thread summary (naming its project) and a
  Forge-resolved subtitle (`project_subtitle_policy`): the default remote's repository path on
  its host (`owner/repo`, nested groups kept, host omitted), with ` · <branch>` for a linked
  worktree (detached: short commit, else the worktree directory); otherwise the project's
  display name. `ProjectSubtitles` caches one repository observation per project root for every
  connection, refreshed in the background when older than a minute, so a listing never waits on
  Git; a changed observation wakes delivery. Migration `m20261001_000021` indexes
  `conversation_items(thread_id, item_kind)`.
- Protocol: `readRecentThreads` (Request @46) answers `recentThreads` (Response @44); the
  `recentThreads` Event (@10) pushes the same `RecentThreadList`. A connection that read the
  recent threads receives their changes: after every request and wake its delivery driver
  compares a cheap fingerprint of the listing's inputs and the subtitle generation, re-reads
  only when either moved, and pushes only a changed list.
- Editor: the recent threads are read as the connection starts and after a reconnect, then
  rendered as pushed. Grouping into Last 24 hours, Last 3 days, Last 30 days and Older is
  presentation, a pure function of the list and the clock (`recent_thread_groups.rs`); the
  sidebar repaints when a row crosses an age boundary. The 1.5 s sidebar listing poll is
  removed: the selected project's listing (thread picker, command menu, opening a thread) is
  read again only when a pushed list shows one of its threads differently, once per list.

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

## 7. Editor state inventory

What the Editor keeps after step 7, and why each is not Forge state:

| State | Where | Why it stays |
| --- | --- | --- |
| FPS limit, FPS overlay, "reopen last host" hint | Editor pool (`editor_settings`) | Device-local presentation, or needed before any Forge connection exists |
| Host invitations, TLS pins, reconnect capabilities | Credentials module | Connection bootstrap material, neither pool. The Editor has no built-in host: a window opens the reopen hint's host (resolved to its current registration), else the first registered host, else offers to add one; only `cargo dev` (`ARTISAN_DEV_OWNED_FORGE=1`) and `ARTISAN_DEV_FORGE_HOME` connect to a Forge on this machine |
| Machine menu memo (name, home, subtitle, avatar seed per host) | `native_hosts/catalog.rs` | Presentation cache rebuilt from the credential store on every refresh |
| Composer view: text being typed, undo and redo, thumbnails, the per-scope save chain | `native_composer*`, `composer_draft_sync.rs` | View history and in-flight saves; the Forge draft is the stored copy |
| Last draft revision the Forge reported per scope | `composer_draft_sync.rs` | Echo of Forge data, so a send names the revision it saved |
| Listings, catalogs, usage rows, preferences, outbox and transcript projections | Application and view state | Renders of Forge data, replaced by each read or push and dropped on switch |
| Recent threads across projects, with their subtitles | Sidebar view state (`impl_sidebar_threads.rs`) | Render of the Forge's pushed list; the age groups are derived from it and the clock on each paint |
| Selected project and its thread listing | Application state | Which project's threads the picker and command menu show; follows the opened thread. No sidebar project switcher or project cycling state remains |
| Selection, scroll, focus, open menus, animations | View state | Ephemeral view state |
| Connection holds and in-flight request ids | Transport | Liveness of work in flight, not stored |
| Dev startup receipt, frame capture | Opt-in development writers | Development tooling, allowed by the file-write guard |

Connection upkeep (after step 7):

- The Forge serves a connection in order and pushes after each response, so it can be mid-way
  through a long push when the Editor's next request arrives. The Editor forwards pushes while
  any request waits (`ServiceRuntime::exchange`); before, a thread's history push could fill the
  delivery channel and the stream window and stall both sides until the Forge's 30 s send limit
  dropped the connection.
- A reconnect's cancel-and-join of the delivery task no longer waits on a full delivery channel.
- A draft save lost with the connection releases its hold and becomes unsent text, saved again
  (latest wins) when the service reports `Reconnected`; a host switch's drain is bounded.
- The service reports the host registration it resolved to (`HostHome`); the window's home, label
  and reopen hint follow it, so a newer incarnation that retired the opened one is not shown as
  "Unavailable host".
- The built-in "This computer" host is removed: the machine menu lists registered hosts only.

Still pulled rather than pushed (cadence only, the data is the Forge's): the host catalog is read
again every five minutes and when an engine's verdict changes. The sidebar's 1.5 s listing poll
is gone: recent threads are pushed, and the selected project's listing is read only when a push
shows it changed.
