# Artisan Editor Completion Matrix

Scope: the V1 prototype described by [`artisan-editor-v1.md`](../prds/artisan-editor-v1.md), including the backend, desktop shell, renderer, and release harness. Status is verified implementation status as of 2026-08-18, not design intent. Codex and Claude are production adapters over the user's external provider executables; Forge and Editor contain only Artisan-owned runtime code. Embedded browsers/WebViews and broad Git mutation commands remain deliberately outside this prototype rather than incomplete hidden scope.

Verification snapshot: on 2026-08-10, the integrated worktree's `pnpm run validate` passed formatting, lint, root TypeScript, static production frontend and Forge builds, 374 passing Vitest files plus 3 skipped files, 2,571 passing tests plus 6 explicit skips, the dev-TUI Bun smoke test, native formatting/clippy, and 73 Rust tests. The exact staged Bazel milestone passed `//:forge_sea`, TypeScript, frontend, Forge, native, and 30 focused tests with one skip; the release target emitted only the 386,274,304-byte `Artisan Forge.exe`. Full staged `//:test` reached Vitest but reported seven existing baseline assertions outside this slice.

On 2026-08-15, installed cold boot was repaired and replayed through 0.2.63. Electron no
longer rewrites the installer-owned Start Menu shortcut, and a directly launched packaged
Editor derives the stable installation `bin/ae.exe` instead of an absent resource script.
Native readiness now allows 30 seconds for the observed 20-second SEA/state cold start;
the desktop handoff allows 40 seconds, both paired and recovery navigations are bounded at
five seconds, and failure replaces the temporary loader with a safe retry document. Both
installed shortcuts retained `ae.exe open`; Forge became ready after 20.54 seconds and the
first Editor handoff established loopback transport after 20.03 seconds without a second
open command. `validate:desktop` passes 46 tests, `validate:native` passes 33 installer plus
45 CLI tests, and independent lifecycle/path/security review is clean.

On 2026-08-17, installed diagnostics proved the unreloadable black window was renderer
process loss: 0.2.74 and 0.2.77 exited for OOM, while 0.2.76 crashed. The desktop now
defers and coalesces an authenticated renderer replacement after `render-process-gone`,
rejects recovery during shutdown, and awaits cancellation before Forge cleanup. Installed
0.2.78 contains the recovery in `app.asar`; a controlled renderer loss kept desktop main
PID 7372 alive and replaced renderer PID 13744 with PID 18400 in 1.6 seconds, with the
diagnostic trace completing from the replacement. The OOM allocator source remains
unproven; this milestone is verified containment, not a root-cause claim. Separately, the
new-thread controller now installs the retained first-submission release in the route scope
under `Effect.uninterruptibleMask` before publishing the claim, so route replacement cannot
silently strand and erase the first message. Exact scope-handoff regressions pass; the
post-change desktop gate passes 13 files / 52 tests plus the production build, and the
focused frontend controller/route cluster passes 3 files / 8 tests. Independent lifecycle
review is clean. The fixes are pushed on `master` in `68c2ebb3` and `25a54741`.

On 2026-08-18, installed 0.2.85 durable evidence showed a second first-submission
failure after the earlier claim-lifetime repair: the failed thread accepted creation
and later acknowledged attention, but never accepted `thread.send_message` and created
no run or conversation item. Emitted SER output showed asynchronous claim acquisition
and the untracked delivery launch as independent one-shot reactive sites; launch could
observe no claim before acquisition completed and never rerun, leaving the composer
blocked without an exception. The route now sequences claim and thread-scoped launch
inside one Effect startup boundary. A transform regression proves there is one emitted
startup site and no standalone claim site; the focused controller/draft/open/route
cluster passes 6 files / 26 tests, touched formatting and lint pass, and independent
lifecycle review is clean. The full frontend gate passed formatting, lint, and both
production builds; 962/963 tests passed, with only the protected unrelated composer
line-budget assertion remaining. Installed 0.2.85 predates this source fix.

Artisan-owned presentation instructions are resolved through an immutable Effect
Service for every ordinary, recovered, and graph engine run. Codex app-server
and exec fallback plus Claude start/resume receive the guidance through their
additive system-instruction boundaries without changing user input or public
conversation projections. The contract documents the installed Comark fence
grammar and renderer. Conversation fences use Shiki's GitHub light/dark themes,
Barekey-derived cards, filename chips, copy actions, line numbers, and selected
lines from `{1,3-5}` metadata; raw HTML stays inert and directive-supplied pre
styles are discarded. Parser, renderer, presentation-drift, provider, recovery,
and non-leak tests cover the boundary.

On 2026-08-12, Forge shutdown gained an explicit bounded orchestration drain.
Authenticated stop, restart, signals, and parent disconnect now latch admission,
reject new root and graph commands, cancel live provider-neutral runs, close their
owned scopes and subprocess trees, await bounded durable settlement, and only then
release the database lease and host scope. Concurrent close requests share one
result, while a stalled drain yields to Forge's 12-second deadline below the native
CLI's 15-second restart budget. Focused root/graph/Forge lifecycle coverage passes
18 tests; the complete Forge gate passes 44 tests with one explicit skip. The full
backend aggregate reached its existing long-running test phase twice without a
result before the 3- and 5-minute command deadlines; formatting, lint, TypeScript,
and the focused backend lifecycle tests pass.

Crash recovery now publishes its authoritative interrupted root-run state before
attempting provider-native resume in the orchestration service scope. Resume has a
15-second cooperative handshake deadline and catches typed failures and provider
defects, so a stalled attempt cannot prevent Forge or its settled conversation
snapshot from becoming available; failed recovery closes its owned scope and does
not replay the user's command. The 11-test orchestrator lifecycle suite covers
successful resume, rejection, defects, and cold-start availability during a stalled
resume. Backend formatting, lint, and root TypeScript pass.

Provider-native child graphs now participate in the same crash recovery contract.
Startup replays committed child inbox evidence before interrupting stale root
ownership, immediately stops children whose roots cannot resume, and keeps children
provisionally active only while a verified native resume handshake is in flight.
Every failed, unsupported, defective, or timed-out resume reconciles child bindings,
assignments, runs, conversation lifecycle, and group state to terminal authority, so
dead workers disappear from the renderer's authoritative roster. Three focused
native-subagent recovery tests plus the 23-test orchestration/adoption/lifecycle set,
backend formatting, lint, and TypeScript pass.

Participant conversation inspection now preserves identity and late provider evidence
across root settlement. The renderer's Back and Escape controls directly yield their
roster-owned Effect instead of attempting to call it as a callback. Forge rejects any
child-shaped observation whose native identity is the root itself, and terminal roots
admit late lifecycle or transcript frames only for an exact existing child binding.
Monotonic natural completion or failure can replace the provisional stop assigned when
the root settled first, while unknown children remain rejected and late prose projects
under the child's durable turn and wakes existing subscriptions. Installed evidence
confirmed the reported empty Johanne view was a malformed root-as-worker duplicate,
whose real transcript correctly remained in the root conversation. Three focused files
pass 14 tests; scoped format/lint, production frontend and Forge validation builds, and
independent review pass. Aggregate frontend tests retain the six protected source-contract
failures, while root TypeScript retains the protected steering and agent-graph errors.

The renderer's active-agent roster filters every terminal worker lifecycle state,
including completed and failed assignments whose parent orchestration group remains
active. The 13-test focused roster suite covers all terminal and non-terminal states.

Live-run steering preserves tool-chain continuity without violating transcript order.
Post-steer activity remains below the acknowledged user message, but adjacent standalone
activity and diagnostic blocks now enter one trace disclosure instead of one disclosure
per item. User or assistant content, turn changes, and pagination remain hard boundaries;
focused steering/trace coverage passes 18 tests, frontend formatting, lint, and production
SSR/client builds pass, and independent review is clean.

The development-only `/debug/components` route provides a deep-linkable gallery of 22
important thread specimens, including message, work, trace, approval, question, change,
usage-recovery, error, compaction, model-handoff, context, and turn-action states. Its
previous/next controls loop through one mounted specimen at a time, fixtures are decoded
through the public protocol schemas, and the production build replaces the route with an
empty stub. Three focused suites pass 46 tests; scoped format/lint, development and
production SSR/client builds, production-exclusion inspection, and independent review pass.

On 2026-08-13, project attachment gained a Forge-owned native directory picker shared
by paired browsers and the Electron renderer. Its authenticated request contains no
path; Windows Forge opens the system dialog through a scoped, bounded Effect child
process, canonicalizes and stats the private result, and returns only a newly minted
opaque directory identity for the existing select-and-attach flow. Filesystem roots
receive a neutral display label so even that edge case cannot disclose a host path.
Cancellation is a normal result, concurrent dialogs fail immediately, and unavailable
native selection falls back to the existing Forge-owned opaque directory browser. Six
focused suites pass 29 tests, the production frontend and Forge validation builds pass,
and the full transport suite passes 129 tests. The aggregate backend and transport
gates currently stop on unrelated dirty orchestration formatting and type errors before
their test phases.

The Windows picker now uses the Explorer common-item `IFileOpenDialog` in folder mode.
This replaces a broken WinForms `AutoUpgradeEnabled` call that does not exist in Windows
PowerShell's .NET Framework and caused every request to fall back. Both the committed
legacy renderer and the protected new-thread renderer invoke native selection first;
the legacy browser remains only for an unavailable native boundary. A real Windows
compile/instantiate probe plus five focused cross-layer suites pass 23 tests, and the
production frontend and Forge validation builds pass.

All Artisan-owned background subprocesses now suppress Windows console allocation. The
pinned Effect Node process adapter applies `windowsHide` to normal launches and both of
its process-tree cleanup calls, covering Git status/diff, PowerShell, registry, scheduler,
picker-host, ACL, and preview commands through one boundary. Direct Node launchers already
carry the same option, while the explicit foreground CLI remains visible. Native installer
discovery, retirement, shortcut, diagnostic, and cleanup helpers share a `CREATE_NO_WINDOW`
factory. Focused process/Git/picker/permission coverage passes 26 tests with one skip;
desktop, Forge, and native gates pass, the production desktop/Forge build succeeds, and
artifact inspection confirms all three Effect launch paths in the emitted Forge bundle.

Thread opening now crosses the control boundary once. A typed `thread.open.query`
resolves the authoritative thread identity and reads session, work, and conversation
state concurrently; the renderer mounts from that response instead of a four-RPC
waterfall. Its conversation subscription resumes from the returned patch watermark,
so Forge reads a cheap cursor head and sends only retained patches rather than decoding
and transmitting the complete transcript again. Mismatched, ahead, gapped, erased, and
legacy no-cursor cases retain safe snapshot/error behavior, while reconnect envelopes
advance only after lossless queue admission. Generic events no longer reread the
conversation, and participant views use maintained root/agent group indexes instead of
rebuilding a filtered full-history projection. Real MessagePort reconnect tests, cursor
fallback/erasure tests, and the full 21-file/132-test transport suite pass; frontend
formatting, lint, and production SSR/client builds pass before the seven protected stale
source-contract assertions outside this slice.

Recovered native roots now have a bounded post-open liveness proof. Thirty seconds
after a nominally successful resume, Forge compare-and-sets the run to failed only
when its durable provider observation watermark is still unchanged; real persisted
progress wins every race. Root-run transitions synchronously reconcile the thread's
live status, and startup also repairs orphaned legacy `Working` projections with no
root run. Focused orchestration and lifecycle coverage passes 43 backend tests.

Thread metadata refinement no longer contains `live_status` in its schema, domain
result, worker intent, or persistence update, so delayed and replayed content cannot
represent lifecycle state. The exact coordinator `active_run_id` is the sole status
authority: queued creation and every run transition reconcile in the same transaction,
startup recomputes every thread without a journal idempotency key, and projection rebuild
overlays the same authority instead of replaying stale historical labels. The installed
stale thread `19595619924447232` was guardedly repaired from its completed active run.
Twelve focused backend/protocol/transport files pass 110 tests; scoped format and lint
pass, while the aggregate backend gate stops on the protected steering-test formatting
baseline before later phases. The renderer also suppresses a checklist whose owning
canonical turn is terminal, even when a provider left plan entries active or pending.

Renderer connection establishment now has a 10-second deadline spanning WebSocket
open, transport pairing, protocol welcome, and subscription readiness. A stale socket
first discovered by a control send can no longer leave Editor indefinitely parked on
its reconnecting gate: stalled replacement handshakes are interrupted, consume the
existing bounded retry epoch, and leave pending requests with a settled failure. The
deadline ends at ready, so healthy live sessions remain unbounded. Focused coverage
proves initial exhaustion and explicit retry, a healthy session beyond the deadline,
and the exact ready → failed send → stalled reconnect path. The complete transport
suite passes 22 files / 135 tests; transport formatting and lint pass, while the gate's
root TypeScript stage retains the protected steering and agent-graph baseline errors.

On 2026-08-15, rapid installed false disconnects were traced to WebSocket ingress
overflow while Forge remained healthy, not renderer background throttling (which is
already disabled). The prior 256-frame logical default exactly matched Forge's replay
event limit, leaving no room for `welcome`, `replay.complete`, or concurrent startup
control frames; installed `forge.log` contains 388 matching control-channel overflows.
The concrete fix is lossless queueing, not another capacity factor. Every Artisan-owned
transport work queue is now unbounded: physical WebSocket ingress, logical control and
stream ingress, MessagePort ingress, pending RPC registrations, client/server binary
streams, projection subscriptions, event observers, error delivery, and diagnostics.
Queue-capacity options plus local request/subscription/stream overflow admission were
removed, while the client still starts scoped readers before resubscribing. External
overflow reports remain decodable, and byte-size, deadline, retry, concurrency, cache,
and durable-retention bounds remain because they do not cap queued work. Delayed-consumer
regressions cross the old capacities without disconnect, loss, or reordering.
Final queue-focused verification passes 8 transport files / 62 tests, 14 backend/Engine/lifecycle
files / 149 tests, and 13 frontend/protocol/dev files / 166 tests. Touched formatting/lint and
independent cross-area re-review pass. Root TypeScript is blocked only by the unrelated dirty
recovery-test assertion and four Guidance inference errors; the frontend area passes format,
lint, check, and production build before five unrelated dirty aggregate-test failures.

An already-hydrated renderer now keeps its live workspace visible and interactive
through transient connecting, reconnecting, and authoritative rehydration phases.
The shared gate predicate no longer masks or inerts running work while the finite-retry
client supervisor retains and retries control requests; first hydration and settled
connection or hydration failures still block with their recovery controls. Installed
0.2.61 evidence kept Forge and provider processes alive across the observed client
WebSocket close. The 26-test gate suite, six transport retry/deadline tests, scoped
formatting and lint, the production frontend build, and independent review pass. The
aggregate frontend gate stops on protected formatting outside this slice.

Failed coordinator runs now expose one explicit, idempotent `run.retry` action on
their exact current work session. Forge authorizes only the current failed root,
creates a fresh queued run, and reuses the original durable start payload—including
ordered rich content and hydrated images—without projecting another user message.
The renderer holds submit ownership through its authoritative work refresh, treats
queued work as busy, and both the command builder and backend reject a distinct
ordinary send while that root is still queued, closing the duplicate-root race.
Five focused frontend/protocol/backend files pass 58 tests, the production frontend
build and scoped lint/format checks pass, and independent re-review is clean. The
aggregate frontend and backend gates stop at unrelated protected formatting files;
root TypeScript retains only the existing steering, provider-management, and
agent-graph baselines.

Installed 0.2.61 run failures were traced to five saturated engine delivery buffers,
three recovered runs with no durable post-resume progress, and eight follow-up launches
that failed while preparing continuation state. Engine event, notification, diagnostic,
and persistence-delivery queues are now unbounded and lossless; no warning threshold or
finite persistence cushion remains. Private Codex reasoning deltas validate without
entering the public observation path, and adjacent identical unclassified native actions
compact before serial durable persistence. Durable writes retry the same work under paced
backpressure, while failed fallback writes and interruptions propagate their original
Effect cause instead of silently draining evidence. Every failed terminal, startup
failure, and recovery-liveness failure
now carries a fixed renderer-safe error reference through journal replay into a visible
work-session error card beside exact-run Retry. Eight focused engine/backend/frontend
files pass 113 tests; scoped format/lint/diff checks and independent post-fix review pass.
Root TypeScript retains only the unrelated steering, engine-installation, and agent-graph
baselines, and the area gates still stop on unrelated protected formatting files.

On 2026-07-27, the Windows distribution artifact, hermetic lifecycle, and real
isolated packaged-bootstrap gates passed alongside root TypeScript. This
verifies the Windows x64 first implementation only; it is not npm publication,
Windows arm64, macOS, or Linux acceptance.

## Status Legend

- `implemented`: the V1 requirement exists in production composition and has direct verification.
- `deferred`: explicitly outside the Codex-only prototype; no production implementation is claimed.
- `planned`: required by another PRD but not yet implemented and release-verified.

## Protocol, Persistence, And Transport

| Requirement                                                                                                     | Status      | Evidence and gate                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| --------------------------------------------------------------------------------------------------------------- | ----------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Shared transport-safe Effect Schemas and exhaustive versioned envelopes                                         | implemented | `modules/protocol/src/`; codec, lifecycle, malformed-input, import-boundary, and router suites.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| Correlated receipts, stable errors, trace metadata, and semantic command idempotency                            | implemented | `JournalStore`, transactional domain repositories, and transactional acceptance tests cover exact replay, regenerated transport timestamps, conflicting intent, and rollback.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| SQLite WAL, generated Drizzle migrations, event ledger, per-stream ordering, and synchronous projection updates | implemented | Production `Database`/runtime Layers plus persistence, restart, contention, erasure, and migration suites. Raw Engine frames are transient behind monotonic run watermarks; render-cadence text batching, 256-patch/512-surface per-thread retention, consumed native inboxes, complete attachment-aware erasure, incremental vacuum, and an offline lease-protected compactor bound write amplification and retained bytes.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| Replay, ACK, heartbeat, gap detection, cursor resume, snapshots, patches, and lossless binary/control streams   | implemented | `ProtocolServer` and `modules/transport/` suites cover reconnect, gap recovery, independent control/stream traffic, and continued operation. Artisan-owned transport, subscription, observer, request, error, and diagnostic queues are unbounded; stalled consumers retain every accepted frame or event in order instead of dropping work or closing a healthy connection.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| Canonical renderer-ready conversation projection and reducer                                                    | implemented | Versioned typed turns/items, stable provider lifecycle identities for streamed activity, transaction-local journal/Engine projection, atomic per-thread ordinal/patch allocation, source idempotency, durable active-run reconciliation, live snapshot/patch resync, and focused protocol/backend/frontend tests.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| Deterministic public-projection rebuild                                                                         | implemented | `ProjectionRebuildService` and deep rebuild tests recreate public ledger-derived projections while preserving private operational bytes, dispatch state, and external-system facts that are intentionally not ledger projections.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |
| Standalone HTTP/WebSocket daemon transport                                                                      | implemented | `Artisan Forge` owns the typed control/subscription endpoint independently of Electron. `.tests/deep/forge/standalone-process.test.ts` covers immutable HTTP, authenticated WebSocket access, a real Codex app-server subprocess turn, and durable restart.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| Stable `ae` Forge product CLI                                                                                   | implemented | `modules/cli/` implements `setup/start/stop/restart/status/logs/doctor/open` against the home's single Forge instance (config/secrets/state/log/data at the Artisan home root; legacy single-profile layouts auto-migrate, ambiguous multi-profile layouts fail actionably), authenticated exact-instance lifecycle ownership, detached and foreground launch, current-user autostart, and browser pairing. Installed Editor launch returns before Forge readiness; hidden handoff and exact shutdown use monotonic global deadlines, and Windows logon tasks support both native `ae.exe` and legacy stable `ae.cmd` launchers without a visible console. Detached diagnostics are startup/live-size guarded, log tail/follow reads are byte-bounded, and Forge-spawned Codex SQLite state is isolated from the user's ordinary store. Direct packaged acceptance proves the branded product lifecycle and owned-state cleanup. Doctor additionally verifies the active version payload against the bootstrap-written `payload-manifest.json`, reporting modified/missing/unexpected files as drift and pre-manifest versions as unverifiable. |

## Codex Engine And Orchestration

| Requirement                                                                                                | Status      | Evidence and gate                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| ---------------------------------------------------------------------------------------------------------- | ----------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Provider-neutral `Engine` contract, explicit capabilities, registry, and Effect Layers                     | implemented | Engine conformance, registry, fake-process, transcript, and import-boundary suites.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| Codex CLI subprocess integration                                                                           | implemented | `codex app-server` is spawned and driven over stdin/stdout, with `codex exec --json` fallback. Notification and diagnostic ingress is deliberately unbounded and lossless for run/lifecycle traffic, with no soft capacity threshold. Initialize opts out of exact unused account/MCP/remote-control bookkeeping methods, with a local compatibility filter for id-less frames from older servers; correlated responses and server requests remain lossless. One-shot version probes explicitly close stdin before draining output, covered by an EOF-dependent Windows process-host regression. Burst, transcript, lifecycle, cancellation, cleanup, and raw-origin suites pass. No Agent SDK is used.                                                                                                                                                                                                                                                                                                                                                                                              |
| Claude or other production execution adapters                                                              | implemented | The external Claude Code CLI adapter (`modules/engines/src/claude/`) is registered in Forge alongside Codex. It drives `claude -p` with bidirectional stream JSON, preserves exact raw frames, and supports session start/resume, streaming text/image input, steering, stdio permission decisions, native child lifecycle/transcripts, lossless diagnostics, inactivity failure, and Windows process-tree cleanup. Focused real-process, probe, normalizer, usage, lifecycle, transcript-isolation, and conformance suites pass. No provider SDK or executable is linked, staged, or embedded. Live billable Claude turns remain excluded from ordinary validation. Codex and Claude are the only runnable adapters; Grok and Cursor remain catalog previews.                                                                                                                                                                                                                                                                                                                                       |
| Cross-thread engine/model continuation                                                                     | implemented | Settled threads expose every runnable engine/model in the live selector and lock provider changes only while a run is active. Exact-version native continuation is used only when the selected engine/model proves compatibility. Cross-engine switches use immutable private checkpoints whose summary is generated by one constrained turn on the configurable compaction model (Forge session default, falling back to the thread's own model), from Artisan's bounded, schema-decoded canonical history ordered by logical same-agent runs; a failed or empty compaction turn fails closed to the mechanical canonical summary. Queue interleaving, tamper fallback, multimodal input, lineage privacy, erasure, recovery, rapid serialized launches, selector policy routing, and the active-run UI fence are covered by frontend and continuation suites.                                                                                                                                                                                                                                      |
| Durable provider-usage interruption recovery                                                               | implemented | Terminal Claude/Codex usage-limit evidence creates one revisioned interruption separate from the truthfully failed provider attempt. Forge revalidates provider-owned quota windows, blocks shared and multi-window depletion, schedules absolute reset recovery across restart/suspend, and atomically claims exactly one continuation only after the exact source is failed. Direct model switches appear only from fresh independent allowance evidence (including Codex Spark and Claude Fable-to-Opus refinement); all launches reuse the native/portable continuation boundary with a private recovery directive and no fabricated user message. New incidents capture the Forge-owned auto-continue default and expose per-incident control in a renderer card. Detection, restart polling, stale revisions, duplicate replay, source ordering, atomic row/event/outbox launch, policy preservation, supersession, erasure, hidden transcript behavior, and terminal reconciliation pass 9 focused feature files / 97 tests; production frontend builds and independent post-fix review pass. |
| Engine discovery, auth state, start/resume/stream/steer/approve/cancel, timeout, crash, and orphan cleanup | implemented | Codex and fake lifecycle scenarios plus Windows process/job ownership tests.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| Normalized canonical surfaces and exact raw-origin attribution                                             | implemented | Work, Time, Guidance, Routine, Capability, Process, Change, Permission, native-action, usage, and lifecycle projections are renderer-safe and covered by orchestration and architecture tests.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       |
| Durable agent graph and intake policy                                                                      | implemented | Identity/roles, scope, policy, strict clarification, assumptions, bounded fan-out (sixteen assignments and four globally concurrent Engine processes), DAG gates, retries, steering, stop/pause/resume, joins, usage, conflicts, recovery, and heartbeats are repository/service tested. Provider-native child lifecycles and renderer-safe transcript content enter durable replay inboxes and are adopted into the same monotonic graph with explicit delegation edges, ordered/idempotent restart replay, stale-event suppression, terminal-root child retirement/recovery, and complete thread erasure. Durable identities allocate uniformly at random without replacement across every group in a thread from the schema-validated Norwegian or British female name bank selected in Settings; the removed mock `playful` preference upgrades to British, dataset order is not authoritative, and existing identities retain their stored names.                                                                                                                                               |

## Backend Capabilities

| Requirement                                                                  | Status      | Evidence and gate                                                                                                                                                                                                                                                                                                                                                                                                                   |
| ---------------------------------------------------------------------------- | ----------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Terminal/process ownership and lifecycle                                     | implemented | Backend terminal Services own PTY creation, input/output, restart, kill, exit, failure, cleanup, output pinning, and scoped watchers; real/deterministic driver suites pass.                                                                                                                                                                                                                                                        |
| Controlled files, immutable diffs, review, rollback, and attributed evidence | implemented | Root-confined reads, conditional replacement, generic bounded-store contracts, private rollback bytes, transactional authority, restart recovery, public protocol, and renderer methods are covered. The dormant Windows native implementation was removed.                                                                                                                                                                         |
| Git status/worktrees/diffs and approval-bound stage/unstage                  | implemented | Installed Git CLI is spawned with argv arrays and bounded I/O; durable projections, leases, recovery, literal paths, public routes, and real temporary-repository integration pass. Broader destructive Git commands remain intentionally unsupported.                                                                                                                                                                              |
| Durable preview and rich-link services                                       | implemented | Target/inspection projections, loopback health probes, external launch, pinned-address metadata, content-addressed assets, binary transport, restart, security, and connector lifecycle tests pass. Artisan embeds no browser/WebView.                                                                                                                                                                                              |
| Artisan tool control plane                                                   | implemented | Policy-aware tools for questions, assumptions, terminals/processes, files, Git, previews, approvals, and native actions use durable invocation/approval/evidence projections and fail closed when unavailable.                                                                                                                                                                                                                      |
| Thread metadata, retention, guidance, Model Behaviour, and project affinity  | implemented | Default retention/fences, refinement, guidance/provider sync, settings reconciliation, affinity/rehome, restart, and WebSocket scenarios pass. Forge owns durable reader acknowledgement separately from lifecycle. Unread inactive Complete stays Working blue and failures in red until focused root reading or explicit Settle. Acknowledgement persists through rebuild/restart; later root-visible activity reopens attention. |
| Routine/skill Marketplace                                                    | implemented | Scoped registry, search/detail, compatibility, approval-bound inspection/install, rollback, enable/disable/remove, provider sync/drift, inert `npx skills` discovery, and runtime-only fallback are tested.                                                                                                                                                                                                                         |
| MCP Capability Marketplace                                                   | implemented | Scoped stdio/HTTP sessions, secret references, OAuth lifecycle seams, health/discovery, policy, two-phase invocation approval, bounded artifacts, crash/restart/disconnect/uninstall, sync/drift, and invocation ledger are tested.                                                                                                                                                                                                 |

## Electron And Frontend

| Requirement                                            | Status      | Evidence and gate                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| ------------------------------------------------------ | ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Secure desktop process boundary                        | implemented | The per-user `artisan://` handler points directly at stable permanent `ae`, accepts only the fixed `artisan://forge/start` capability, and reuses nonblocking `ae open` to launch the installed editor. The sandboxed, context-isolated Electron window has no preload, IPC, Electron-owned project picker, or Forge credential ownership. It loads the bundled `artisan://app` connection screen before starting a hidden background handoff, coalesces reconnects, and gives each completed handoff a fresh non-secret document-navigation marker while keeping pairing capabilities fragment-only. It retains only an exact shutdown lease when the handoff proves it spawned the ready Forge; pre-existing, manual, and autostart Forge instances are never stopped on Editor quit. Its named partition persists page-owned preferences and drafts, clears stale Forge cookies before initial pairing, and talks to Forge over plain HTTP/WS like any paired browser. |
| Standalone Forge lifecycle and native packaging        | implemented | Forge and `ae` own lifecycle and pairing. Production Forge is one branded Node 24 SEA; a schema-bounded manifest hashes only Artisan's embedded `node-pty`, Koffi, and migration assets before fail-closed, concurrency-safe, content-addressed materialization. The release build loads both extracted native addons, verifies migrations and zero provider payloads, exercises process-host IPC, and preserves the last good executable before publishing. Provider executables remain external child processes. The SEA re-enters itself for the Windows process host, while development/validation retain explicit ESM entries. Static web hosting is a development opt-in; installed homes serve health and control/WS surfaces only.                                                                                                                                                                                                                                |
| Live UI-only renderer                                  | implemented | Production uses `ArtisanClient` over the standalone WebSocket endpoint, never fixtures or backend/Node imports. Installed, the sandboxed editor renders the bundled frontend from `artisan://app` against the adopted loopback Forge endpoint; development uses the Forge-hosted or HMR browser same-origin. Replaying Effect state refreshes on ready/reconnect, applies only the current hydration's fetched thread snapshot before shell mount, and fences stale subscriptions/actions. Active work cannot be locally settled; its mounted work disclosure remains user-collapsible, while reasoning itself has no disclosure and is absent from settled or non-tail transcript history. Pending model handoffs wait for their source label. Compaction renders as a full-width transcript chapter divider whose single label shimmers only while the durable state is active.                                                                                         |
| System notification delivery                           | implemented | Opt-in completion, failure, approval, and question notifications are derived from durable client events through Effect services; focused active threads suppress redundant toasts and clicks route back to their thread. The renderer preference survives desktop restarts while Forge cookies do not. Windows registers a stable AUMID and toast CLSID before Electron readiness, and installer-owned Start Menu/Desktop shortcuts carry the matching property-store identity; focused tests and a real temporary shortcut roundtrip cover the delivery contract.                                                                                                                                                                                                                                                                                                                                                                                                        |
| Renderer and desktop resource footprint                | partial     | Clean editor models, inactive drafts, transcript DOM, replay IDs, object URLs, WebGL frames, and demand-loaded heavy renderers retain verified resource-lifetime bounds. Work-delivery queues are intentionally unbounded and lossless; sustained backlog growth is treated as an ownership/consumption leak to diagnose and fix, never as permission to drop work. Installed 0.2.60 currently regresses the overall footprint: the dirty live work-session clock reruns a directly yielded `forkScoped` loop and retains 19,594 active timers plus 19,786 Effect fibers. Earlier cold package measurements therefore remain historical baselines rather than current leak acceptance.                                                                                                                                                                                                                                                                                    |
| Monaco and workspace model                             | implemented | File discovery/read/conditional save, workspace-qualified models, markers, view-state restoration, preview/open/pinned/dirty/diff semantics, close consent, recents, changed files, overflow, and Quick Open are covered.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| Chat, orchestration, Marketplace, and session controls | implemented | Live transcript/actions, tool policy, terminal/preview/Git/change/settings surfaces, agent graph controls, Marketplace Routine/Capability lifecycle, identity, and activity state are wired. Active worker rows open participant-filtered read-only conversations from the existing subscription; root and child prose/tool activity remain isolated, while root trace summaries identify one Artisan-named worker or count multiple workers.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| Shadcn-first interaction system                        | implemented | Interactive controls use the repository shadcn-svelte primitives and existing semantic tokens; source gates reject raw interactive HTML and component-local visual-system drift.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| Appearance typography roles                            | implemented | Appearance v1 persistence owns semantic Text and Code roles with bundled Artisan Neo and JetBrains Mono defaults. Legacy device-local sans/serif/mono records preserve their other appearance settings while resolving sans to Text and mono to Code; the obsolete serif choice is discarded. The searchable picker requests a schema-bounded local-font inventory only from an explicit user gesture, single-flight caches successful enumeration, exposes truthful privacy/error/retry states, escapes applied CSS families, and restores defaults. Focused domain, asset, migration, concurrency, accessibility, SER, and production-build checks pass.                                                                                                                                                                                                                                                                                                                |
| Accessibility and responsive behavior                  | implemented | The packaged smoke proves trusted keyboard and native mouse activation, focus restoration, accessible names, wide/narrow pane state, right-pane keyboard reachability, truthful unavailable states, and Electron 200% zoom.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               |

## Testing And Release

| Requirement                                | Status      | Evidence and gate                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| ------------------------------------------ | ----------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Deterministic ordinary validation          | implemented | Focused `validate:frontend`, `validate:backend`, `validate:forge`, `validate:transport`, `validate:desktop`, and `validate:native` gates plus `test:focus -- <file>` cover ordinary milestones. `validate`/`validate:full` remain the integration/release aggregate: formatting, lint, typecheck, production builds, protocol/backend/engine/transport/frontend/deep tests, and no authenticated or paid Engine turn.                                                                                                                                                                                      |
| Bazel build and Forge release graph        | implemented | Bazel 9.2/Bzlmod pins Node 24.18 and pnpm 11.7 from the committed lock graph and rejects lockfile drift. Parity targets cover TypeScript, frontend, Forge, Vitest, and native gates; pnpm actions use a TypeScript runner with explicit inputs, outputs, and allowlisted test/OS capabilities instead of the ambient shell environment. `//:forge_sea` publishes only its stamp and the single executable. Windows-native repository patching avoids Bash/MSYS path rewriting, declared workspace-package closures prevent unrelated invalidation, and Windows watchfs/action caching accelerate rebuilds. |
| Deep integration and generated scenarios   | implemented | Temporary workspace/Git/SQLite/public-protocol scenarios, fixed generated Engine state machines, restart/rebuild, concurrency, cleanup, and architecture/surface traversal are present.                                                                                                                                                                                                                                                                                                                                                                                                                    |
| Provider live policy                       | implemented | Codex usage/authentication is read only through external `codex app-server --stdio` ACP requests; Claude usage is read only through external `claude -p /usage --output-format json`. Installed-CLI probes are explicit, protected, non-billable, and off by default. Deterministic subprocess fixtures cover ordinary tests, and no billable provider turn runs in validation.                                                                                                                                                                                                                            |
| Native addon policy                        | implemented | Native execution remains explicit opt-in outside ordinary local validation for the retained `node-pty` and Koffi dependencies. The dormant bounded-file-store addon and its release gate were removed atomically.                                                                                                                                                                                                                                                                                                                                                                                          |
| Desktop package acceptance                 | implemented | `package:desktop` builds the managed unpacked Editor payload with the bundled renderer frontend and its loopback-Forge CSP variant; `verify:desktop-package` proves the renderer is present, the CSP allows only loopback connects, and the ASAR carries no preload/IPC bridge or embedded Forge lifecycle. The protected production release gate requires and verifies a valid Authenticode signature.                                                                                                                                                                                                    |
| Windows distribution first implementation  | implemented | Signed contracts, deterministic x64 ZIP production, bounded staging, activation, stable launchers, owned integrations, bootstrap handoff, permanent update/repair/uninstall, and rollback are verified by `windows-release-artifact.test.ts` and `windows-install-lifecycle.test.ts`. Release assembly accepts exactly one Forge entry, `forge/Artisan Forge.exe`, and rejects every additional loose file; permanent Rust `ae` launches the SEA directly while retaining legacy-install compatibility.                                                                                                    |
| Native bootstrap and fresh-machine handoff | partial     | Rust `artisan-bootstrap` and permanent `ae` crates implement the cross-platform install/lifecycle boundary; PowerShell and POSIX transports select, verify, execute, and clean platform assets without npm. `sonstabo.com/editor` and both stable script endpoints are deployed. Candidate tooling freezes exact signed GitHub assets for a future hosted release path. Native clean-machine execution, the first product release, Windows arm64 payloads, and macOS/Linux product qualification remain.                                                                                                   |
| macOS and Linux distribution               | planned     | Native artifacts, signing/notarization, integrations, repair, update, uninstall, and platform acceptance remain unimplemented. Overall cross-platform distribution acceptance is not complete.                                                                                                                                                                                                                                                                                                                                                                                                             |
| Hosted release workflow                    | deferred    | Artisan is unpublished, so hosted CI and release workflows are intentionally absent. The local release policy, packaging gates, candidate tooling, and sanitized fixture policy remain ready for a future manually dispatched publication workflow.                                                                                                                                                                                                                                                                                                                                                        |

## Dependency-Ordered Milestones

| Milestone                                                                                        | State    |
| ------------------------------------------------------------------------------------------------ | -------- |
| M0. Protocol/persistence stabilization and test seams                                            | complete |
| M1. Wire protocol, routers, replay, snapshots, and rebuild                                       | complete |
| M2. Terminal, files/diffs, Git, preview, approvals, and tools                                    | complete |
| M3. Codex CLI Engine, fallback, normalization, and lifecycle                                     | complete |
| M4. Fake process, transcript, live opt-in, and conformance harness                               | complete |
| M5. Intake policy, AgentOrchestrator, graph, surfaces, and usage                                 | complete |
| M6. Thread metadata, retention, guidance, Model Behaviour, and affinity                          | complete |
| M7. Routine and MCP Capability Marketplace                                                       | complete |
| M8. Electron editor renderer, standalone Forge daemon, `ae`, WebSocket, and dev browser renderer | complete |
| M9. Deep integration, generated, architecture, transport, and mounted UI gates                   | complete |
| M10. Local validation, release policy, installer, and packaged-process acceptance                | complete |

## Final Completion Audit

- [x] Every V1 envelope and renderer operation is schema-validated and routed through the typed client boundary.
- [x] Accepted domain commands are durable, correlated, semantically idempotent, replayable, and transactionally projected.
- [x] External Codex and Claude executables are the production Engines, controlled only through CLI/ACP protocols and covered by non-billable subprocess fixtures; Forge and Editor embed no provider SDK or executable.
- [x] Threads can switch between the production engines and models through exact-version native continuation or a private, integrity-checked portable checkpoint with bounded canonical fallback.
- [x] Orchestration owns visible identity, intake, lifecycle, topology, steering, joins, approvals, usage, conflicts, and artifacts.
- [x] Terminal/process, files/diffs, Git, preview, tool, retention, guidance, behavior, affinity, Routine, and MCP slices are backend-owned and observable.
- [x] Clients reconnect to independently running Artisan Forge over lossless typed WebSocket control/stream transports; installed, the sandboxed Electron editor renders the bundled frontend and pairs through `ae`'s one-time handoff, while development keeps the paired-browser flow.
- [x] The live frontend uses renderer-safe contracts, shadcn-svelte primitives, semantic tokens, Monaco, and truthful live/unavailable states.
- [x] Deterministic unit, integration, generated, architecture, frontend, native-package, and mounted packaged-product gates pass without an ordinary live-model dependency.

## Current Verification Snapshot

Final local acceptance on 2026-07-22 passed `pnpm run validate` with 155 test files, 1,205 passing tests, and 2 explicit skips. The former NSIS artifact and mounted-renderer gate have since been superseded by the stateless Electron protocol launcher and permanent `ae`-managed distribution. `package:desktop` now emits only the unpacked Editor payload consumed by that distribution; production release CI requires Authenticode signing and verifies the resulting executable signature.

On 2026-07-25 the redesigned thread route was reconnected to the completed backend: live snapshot/patch rendering, send/steer/cancel, approvals/questions, durable Codex model/effort/permission/speed/workflow policy, safe persisted native resume, and live root/sidebar thread navigation are implemented. Direct constituent validation passed formatting, lint, root TypeScript, the production frontend build, 163 test files, and 1,242 tests with 2 explicit skips. The aggregate `pnpm run validate` wrapper could not select pnpm because registry signature verification was unavailable, and the optional installed-CLI smoke could not spawn inside the managed sandbox; neither failure occurred in product code.

On 2026-07-25/26, the standalone-daemon milestone superseded the Electron utility/MessagePort architecture. The production backend now runs as Artisan Forge and serves typed HTTP/WebSocket independently of Electron. The earlier focused standalone proof covered immutable HTTP and authenticated WebSocket access, project/thread creation, Codex app-server stdio execution, a settled assistant conversation, and durable readback after restart; that gate now lives at `.tests/deep/forge/standalone-process.test.ts`. On 2026-07-27 the dormant Electron renderer, preload, IPC, native picker, identity/activity bridge, and desktop MessagePort adapters were removed atomically. The managed per-user `artisan://` registration now targets permanent native `ae` directly, while all client rendering and pairing happens in the Forge-served browser application. Forge retains the dedicated `node.exe`, staged native runtime, and complete CLI lifecycle surface.

On 2026-07-27, Forge thread creation moved from `thread_<snowflake>` to the bare Snowflake decimal while idempotent replay preserves every previously stored identity. Public routes canonicalize both generations to bare URL segments and resolve historical prefixed records through the authoritative thread list. Thread navigation now positions the transcript at its bottom without animation; accepted local user-message projections smoothly align to a 16px top inset exactly once, with a shrinking end spacer that lets streamed assistant output grow below the anchor without forcing later scrolls. Focused protocol/backend/transport/frontend tests, TypeScript, and frontend lint pass; the product build was intentionally not repeated during this UI milestone.

On 2026-07-31, workspace-scoped surface identity was restored: canonical
conversation URLs use `/t/:workspace/:thread`, canonical editor URLs use
`/e/:workspace/:thread`, and optional editor file identity remains in `?file=`.
Historical prefixed thread IDs canonicalize to bare segments, detached threads
use the reserved `_` workspace segment, and these are the sole thread and editor
URL contracts; the root page `/` is the pre-creation draft route (the former
`/threads` draft page was folded into it and removed). The editor
subscribes to authoritative thread metadata, unmounting before reassignment or
detach navigation so revoked workspace authority cannot survive in stale file
reads.

On 2026-07-28, installed rendering moved into the Electron editor and installed
web hosting was retired. `package:desktop` stages the static frontend (with a
loopback-Forge `connect-src` CSP variant) into the payload, the sandboxed
window loads it from `artisan://app`, and `ae open` launches the editor, which
pairs cross-origin through hidden `ae open --handoff` stdout plus the ordinary
`/api/pair` exchange. Forge static hosting became a development opt-in
(`ae setup --serve-frontend`, enabled only by the repo's browser development
home); installed homes serve `/health` and `/api` control/WS surfaces only
while SPA routes 404, enforced by `.tests/forge/static-hosting-gate.test.ts`
and Rust launcher unit gates. `verify:desktop-package` now proves the honest
renderer shape instead of a launcher-only ASAR.

Later on 2026-07-28, the Forge profile concept was removed entirely: each
Artisan home hosts exactly one Forge whose `config.json`, `secrets.json`,
`state.json`, `forge.log`, and `data/` live at the home root beside
`installation.json`. Both CLIs migrate a single legacy `profiles/<name>/`
directory automatically and refuse ambiguous multi-profile homes with an
actionable error. `/health` replaced the `profile` field with
`development: boolean` (true iff the instance serves the frontend), which now
drives the dev badge and `[Dev]` title.

On 2026-07-31, the permission catalog was re-audited against the current native
provider vocabularies. Codex now distinguishes workspace-confined
`Auto approve` from explicit `Full access`: the latter carries host edit scope
and launches app-server and exec with approval policy `never` plus
`danger-full-access`. Assignment policy can narrow filesystem, network, and
approval authority but cannot widen its parent. Claude, Grok, and Cursor retain
their already-correct native catalog mappings. Final mixed-tree
`pnpm run validate` is green: formatting, zero-warning lint, TypeScript,
production frontend and Forge builds, 293 Vitest files/1,971 tests (7 skips),
native format/clippy, and 45 Rust tests.

On 2026-08-01, the frontend SER thermonuclear remediation closed C-01–C-09,
Q-01–Q-11, and every hostile follow-up finding. `svelte-effect-runtime` 4.2.1
fixes transformed callback iteration, canonical controllers own draft/defaults/
usage lifecycles, durable actions reconcile authoritatively, browser and editor
host work use typed yielded boundaries, and a repository-wide lexical source
gate enforces the literal generator dialect. Three independent frozen-tree
reviews closed before the full validation snapshot recorded above.

On 2026-08-03, Claude stream normalization gained run-owned tool-use
correlation, preserving canonical command/search/tool names and producing one
settled file observation with truthful diff counts when provider metadata is
available. Conversation presentation now groups repeated observations for the
same canonical file path, keeps unknown aggregate counts unknown, top-aligns an
accepted local message, exposes a recoverable jump-to-latest control after the
reader leaves follow range, and changes a thinking verb only after its quiet
status line leaves and re-enters the render tree. Provider-started command, tool,
and search activity patches select `Waiting` while that operation is the newest
live detail. Newer assistant or reasoning text restores the stable thinking verb
even when an earlier long-running activity remains open, and a subsequent live
activity selects `Waiting` again. Terminal patches also restore the verb without
polling or a provider-specific UI path when no other live operation still owns
the wait. The full validation snapshot above covers the earlier regressions; the
waiting transition has focused cross-provider lifecycle and renderer coverage.

On 2026-08-09, canonical plan items moved out of transcript rendering into a
conditional right-side Checklist that reuses the Working Threads glass, spacing,
and row language. The thread route publishes the latest full-snapshot plan through
a lease-owned Effect service, so overlapping route finalizers cannot clear a new
thread's tasks and no second conversation subscription is opened. Empty or absent
plans reserve no inspector column. Root TypeScript, production frontend and Forge
builds, native checks, the dev-TUI smoke, and 71 focused tests pass.

On 2026-08-11, settled work history with rendered details initializes collapsed,
including failed and cancelled history. Only currently live work initializes open;
a failure observed live can surface once, and an explicit close still unmounts its
settled trace DOM. The shared inspector drops
its Checklist when every entry is completed or skipped, while active agents still
retain their roster. Composer image paste no longer triggers rustle or duplicate
bump motion; duplicate previews are still revoked and rejected before presentation.
The corrected disclosure policy passes two focused files/nine tests, scoped format
and lint, the production frontend and Electron package builds, packaged-desktop
verification, and direct inspection of the bundled predicate. The earlier
Checklist/paste gate passed nine focused files/93 tests, root TypeScript, full lint,
frontend and Forge builds, native checks/73 tests, SER policy scans, and independent
review. The full frontend suite reaches 680/681 tests; its one failure is an existing
stale source assertion for the protected turn-segment key.

On 2026-08-20, live work disclosure preserved that initially-open contract even when a
session mounts after reply prose has begun. The newest visible durable ordinal identifies
reply versus work phases: later work reopens the trace, later prose folds it, and successful
settlement also folds final prose delivered in the settlement batch; an explicit reader
choice remains authoritative. Six focused frontend files pass 106 tests, frontend format,
lint, and the production build pass, and the aggregate test phase reaches 994/996 with only
the protected composer line budget and concurrent thread-title source assertion failing.

On 2026-08-12, installed Editor freeze diagnosis became durable and bounded. A
home-scoped `profiling-enabled` marker makes every managed launch persist a per-session
JSONL timeline and run Chromium continuous sampling; an unresponsive renderer flushes
the ring buffer to its paired trace. Follow-up instrumentation now uses a 64 MiB
DevTools-grade continuous buffer and an inert main-owned renderer heartbeat to capture
sub-threshold stalls after 1.25 seconds. Five-second CPU/memory metrics, trace-buffer
usage, numbered automatic and Ctrl+Shift+F9 snapshots, cooldown/coalescing, late flush
reporting, and complete session-artifact rotation preserve the surrounding evidence.
Each JSONL is capped at 8 MiB and only eight sessions survive. Installed `0.2.49`
produced the expected session log under `%LOCALAPPDATA%\Artisan\diagnostics\renderer`;
the enhanced source passes the 11-file/39-test desktop gate and production bundling.
The earlier native integration gate passes 76 tests.

Codex app-server start and resume now explicitly request public reasoning summaries.
Streamed public sections and authoritative completed items project without duplication,
including final-only recovery, while private reasoning deltas remain transient and never
enter the durable or public transcript. The renderer now projects exactly one live thinking
line per turn. For models that publish summaries it says the active run's newest non-empty
summary — muted, shimmering, without markdown emphasis markers — at the latest meaningful
visible position across nested and post-steer blocks; later prose, activity, or settlement
retires it without changing durable grouping. The work session's thinking verb is that line's
fallback and never renders beside it: before the model has summarized anything, and for models
whose catalog `reasoning_display` is `trace` because they stream raw chain-of-thought rather
than a written summary. The rail, the stagger stack, and reasoning disclosure are gone.
Post-steer liveness is owned by source-item run identity rather than the acknowledged user turn
id. Focused renderer coverage (4 files / 32 tests), frontend lint, production SSR/client builds,
SER scans, and independent integration review pass.

On 2026-08-09, Codex app-server and Claude CLI child lifecycles and
renderer-safe transcripts gained one provider-neutral adapter contract. Child
completion can no longer settle root metadata or the root reader cursor; global
activity still records its recency, while root-visible activity advances a
separate durable reader cursor. Root transcript rendering keeps only parentless
turns until a worker is explicitly selected. Prose, reasoning, terminal, tool,
file, and search frames replay in order through a schema-decoded durable inbox
beneath the adopted child run. Direct and nested delegation rows use stable Artisan
identity, so the root trace says `Talked to Noodle` for one worker or
`Talked to 2 subagents` for multiple workers. The conditional right inspector
uses Checklist glass and Working Threads row anatomy; selecting an active worker
opens its read-only conversation from the existing subscription, keeps terminal
selection inspectable, and returns by Back or Escape. Claude Agent/Task
correlation excludes ordinary provider background jobs, including child-framed Bash
tasks. Root TypeScript, formatting, lint, production frontend and Forge builds,
134 focused tests with one explicit skip, the thread-erasure regression, the
55-test frontend policy/integration cluster, the complete `pnpm run test`
command, and an independent integration review pass.

On 2026-08-10, provider ownership returned fully to external CLI/ACP processes.
The Claude Agent SDK dependency, adapter, native payload, SEA staging, and runtime
cache copies were removed; Claude runs and usage now invoke the user's installed
`claude` executable, while Codex usage/authentication comes from app-server ACP
instead of reading `auth.json`. The direct Claude stream/control adapter is
schema-decoded and lossless, and its real-process matrix covers approvals, steer,
resume, child transcripts, malformed output, cancellation, and descendant cleanup.
The focused provider/build suite passes 112 tests. The exact isolated milestone
emits a 105,915,904-byte `Artisan Forge.exe` with zero provider assets or exact provider
SDK markers, down from the prior roughly 369 MB SDK-bearing executable.

On 2026-08-07, the renderer performance milestone kept published SER 4.2.3 as
the sole Svelte Effect executor. Later 71 MB and 440 MB heap snapshots disproved
the initial no-leak finding: one SER component scope grew from 1,856 to 25,923
finalizers while its defaults PubSub grew from 1,854 to 25,921 subscribers.
The local SER 4.2.4 candidate fixes that upstream lifecycle defect by giving
each reactive run an isolated scope, closing failed setup with its exact Exit,
and disposing successful scopes on rerun/unmount. It passes 805 runtime tests,
a five-case browser conformance suite with 32 real PubSub reruns, type/format/
lint, build/pack, and a packed-artifact SvelteKit/Playwright smoke. Artisan
temporarily consumed the exact candidate tarball and passed its focused SER,
recovery, and performance tests, production frontend and Electron builds, and
the packaged-desktop verifier; source remains on 4.2.3 pending publication. A
real-thread packaged soak ended after ten minutes at 417.0 MiB working/325.5 MiB
private total and 138.0/72.1 MiB renderer, with used JS heap falling from 40.3
to 14.7 MiB, DOM nodes holding 2,150→2,155, and listeners holding at 92. A second
five-minute launched-visible pass ended at 387.4/244.5 MiB total and 128.1/63.2
MiB renderer. The stable candidate heap retains 60 Effect FiberRuntime objects,
16 latches, and seven PubSub subscription/backing/replay cursor objects, compared
with roughly 73,000 of each leak-family object under 4.2.3. Over a 30-second idle
sample the browser and renderer each used 0.052% of one core while GPU and utility
used no measurable CPU. Conversation
reduction and render projection retain identity indexes and patch one active
render slot; the initial transcript mounts only the newest 24 groups and pages
older groups without scroll jumps. Adjacent activity projection is linear rather
than repeatedly copying a growing array, and hidden diagnostics are not grouped.
Clean inactive editor models compact to text-only state with an 8-document/4 MiB
hot cap; composer image bytes retain their admission limits, while composer gesture
delivery and anchor-layout wake work are unbounded and lossless. Recent patch IDs,
attachment work, blob URLs, settled trace DOM, and the shader loop retain explicit
resource lifetimes or cache limits rather than queue-cardinality caps.

The same day, an installed resume failure showed Forge staying healthy across a
3,679-second sleep while Chromium exhausted its finite pre-ready WebSocket epoch
and parked. Transport retry now latches one recovery authorization even while an
epoch is in progress, and a scoped browser clock-gap monitor releases it after
wake. Ready sessions cannot pre-authorize a later outage, repeated wake signals
coalesce on one Deferred, and runtime disposal interrupts the monitor. The fix
does not reload the renderer, restart Forge, add IPC, lose drafts, or create an
unbounded retry loop. The local 4.2.4 candidate passes 20 focused files/113 tests
covering SER, recovery, retention, trace, and store behavior; frontend production
build and scoped format/lint pass. Root TypeScript is blocked only by two errors
in unrelated in-progress `shell-layout.ts` work.

A 17:29 follow-up heap was captured from the same installed 0.2.15 process
started at 13:04, before the local 4.2.4 candidate existed. Across its three
captures, FiberRuntime instances grew 1,942→26,015→73,474 and PubSub
subscriptions/cursors 1,862→25,929→73,348. This confirms continued published
4.2.3 leakage; it does not test or contradict the local candidate.

On 2026-08-14, an installed 0.2.60 heap exposed a separate dirty-tree
timer/fiber amplifier in the live work-session clock. One directly yielded
reactive site wrote its own `now_ms` state and forked a new one-second loop on
every rerun, while the previous fork survived. The snapshot retains 19,594
`DOMTimer`, `ScheduledAction`, and `V8Function` objects through the window's
`DOMTimerCoordinator`, alongside 19,786 production-minified Effect
`FiberRuntime` objects. Published SER remains 4.2.3, but its earlier PubSub
signature is not dominant in this capture. The 102,706,728-byte live heap does
not by itself explain Chromium's multi-gigabyte working set. In the same
investigation, the main Forge SEA held at 482.3 MiB working / 503.4 MiB private
across a 30-second sample after 99 minutes of uptime; Forge currently exposes
health and logs but no V8 heap telemetry, so its roughly 500 MiB JS/native split
is not yet attributable and no Forge leak is claimed from that single plateau.

On 2026-08-10, the Windows release lifecycle replay installed and activated
0.2.33 end to end. Missing mutable state recovers from bounded authenticated
registry cards, installer retirement addresses each exact superseded PID, setup
leaves launch ownership to Editor, and maintenance updates restore only a Forge
that was already running. A cold `ae open` returned in 697 ms while Electron
showed its loader and completed the hidden handoff independently; closing the
window stopped the exact Editor-owned Forge in 237 ms and the complete Electron
tree in 1.33 seconds, leaving no Artisan process. Windows autostart remains an
explicit current-user, limited `ONLOGON` scheduled task rather than an elevated
machine-wide service.

Later on 2026-08-10, installed 0.2.35 repaired the missed-pairing race exposed
by a genuinely cold Forge start. The initial renderer remains immediate, but
each background handoff now causes a fresh document bootstrap instead of a
fragment-only in-page navigation. `ae open` returned in 781 ms, no `ae` process
lingered, and the installed renderer established its Forge transport 3.41
seconds later. Closing the window again removed both Editor and its exact-owned
Forge. Focused desktop/release verification passed 9 files/58 tests plus both
Windows artifact verifiers; independent review found no blocking issue.

On 2026-08-11, installed 0.2.40 eliminated the desktop loader's impossible
MessagePort retry loop and the CLI's stale-card process-table scan. Startup now
serializes authenticated probe→spawn→ready ownership with a per-home native
lock; four simultaneous cold handoffs produced one Forge. Against 99 stale
registry cards, pre-spawn coordination completed in 273.39 ms, a cold handoff
completed in 5.342 seconds (5.069 seconds was Forge boot), and ten warm handoffs
measured 25.96–39.93 ms with a 30.21 ms median. The packaged renderer established
its loopback Forge transport and clean close left no Artisan process. Focused
pairing gates pass 44 UI tests and 44 CLI tests; the complete native area passes
76 tests plus formatting and warning-denied Clippy. Independent review found no
blocking issue.

Also on 2026-08-10, live-run steering became acknowledgement-gated. Durable
acceptance records the message and outbox work without projecting a canonical
user turn; only a successful engine `Send` appends `thread.message_steering`
immediately before its routed acknowledgement. Until that source-referenced
turn exists, Editor keeps the immutable submission in a lighter `LipCard` above
the stable composer shader and hides the duplicate draft body. Delivery failure
queues the fully decoded and hydrated payload, including rich content, project
mentions, attachment metadata, working directory, and raw origin. Recovery
interrupts an uncertain pre-crash steering run without resuming it, then creates
one idempotent queued replacement. Stable provider messages spanning the
acknowledgement carry bounded, coalesced text cuts, so contiguous fragments stay
below each steer through streaming, completion, multiple steers, and reconnect.
Six focused suites pass 52 tests with scoped formatting/lint, Forge/frontend
production builds, and clean independent review. Root TypeScript currently
stops on one unrelated catalog-manifest narrowing error.

On 2026-08-12, the acknowledgement UI was hardened around the run-authority
refresh gap: a pending steer now keeps its `Steering` shimmer on the owning
pending or active session without reviving settled history. Standalone
post-steer native events re-enter the trace visibility policy instead of
falling through to a raw transcript row, and public diagnostic summary/detail,
provider code, and terminal output remove CSI plus OSC/DCS/SOS/APC/PM control
strings while private raw provenance remains private. Activity disclosures also
restore the historical single-category icon / mixed-group icon rule, aggregate
clauses without category prefixes and remain muted until hover or expansion. Their
full-height child rail is now visual-only and pointer-transparent; the header is the sole
disclosure control, and activity rows no longer open output hover previews. Thread-history
group labels align with their provider icons. Account and image overlays suppress proximity
thread history without banking keyboard hover state; unavailable provider rows keep their
foreground name in the header and put the error in the body. Changed-file cards present
contained paths from the project folder, using `\` for Windows roots and `/` for macOS/Linux
roots while path actions and icon resolution keep the original path. The combined path/trace cluster
passes 42 tests and the account/usage slice passes 34. Frontend formatting, lint, and
production builds pass; aggregate frontend tests retain five protected stale assertions
plus their dependent import suite across six files, while root TypeScript remains blocked
by the protected agent-graph repository error typing.

On 2026-08-11, explicit New thread navigation stopped reviving unrelated composer
state or an app-scoped stale model policy. The target draft slot is discarded
under the draft controller's submit lock before a revision-keyed remount, delayed
seed loads are rejected by an atomic revision guard, and successful creation or
retry clears the sent draft interruption-safely before route teardown. Confirmed
picker changes now persist the exact catalog model identity (including harness),
per-model reasoning effort, context, and speed tier; legacy native model ids still
seed compatibly, while unavailable speeds fall back to a current catalog option.
SQLite adds one nullable `session_model_defaults.service_tier` column. Thirteen
focused suites pass 93 tests, scoped format/lint/SER, root TypeScript, and the
production frontend build. The full validator passed its global pre-test gates,
then the complete Vitest phase became idle and was ended without a final result.
