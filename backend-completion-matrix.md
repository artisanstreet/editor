# Artisan Editor Completion Matrix

Scope: the Codex-only V1 prototype described by `artisan-editor-prd.md`, including the backend, desktop shell, renderer, and release harness. Status is verified implementation status as of 2026-07-22, not design intent. Additional production Engine adapters, embedded browsers/WebViews, and broad Git mutation commands are deliberately outside this prototype rather than incomplete hidden scope.

Verification snapshot: on 2026-07-22, `pnpm run validate` passed formatting, lint, root TypeScript, the static production frontend build, and 155 Vitest files with 1,205 passing tests plus 2 explicit skips. The release-only `verify:desktop-package` gate passes against the real unpacked Electron application and its staged native dependencies. The final installer and exact-package verifier are rerun after the validation record below.

## Status Legend

- `implemented`: the V1 requirement exists in production composition and has direct verification.
- `deferred`: explicitly outside the Codex-only prototype; no production implementation is claimed.

## Protocol, Persistence, And Transport

| Requirement                                                                                                     | Status      | Evidence and gate                                                                                                                                                                                                                 |
| --------------------------------------------------------------------------------------------------------------- | ----------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Shared transport-safe Effect Schemas and exhaustive versioned envelopes                                         | implemented | `modules/protocol/src/`; codec, lifecycle, malformed-input, import-boundary, and router suites.                                                                                                                                   |
| Correlated receipts, stable errors, trace metadata, and semantic command idempotency                            | implemented | `JournalStore`, transactional domain repositories, and transactional acceptance tests cover exact replay, regenerated transport timestamps, conflicting intent, and rollback.                                                     |
| SQLite WAL, generated Drizzle migrations, event ledger, per-stream ordering, and synchronous projection updates | implemented | Production `Database`/runtime Layers plus persistence, restart, contention, erasure, and migration suites.                                                                                                                        |
| Replay, ACK, heartbeat, gap detection, cursor resume, snapshots, patches, and bounded binary/control streams    | implemented | `ProtocolServer` and `modules/transport/` suites cover reconnect, overflow, independent control/stream traffic, and continued operation.                                                                                          |
| Deterministic public-projection rebuild                                                                         | implemented | `ProjectionRebuildService` and deep rebuild tests recreate public ledger-derived projections while preserving private operational bytes, dispatch state, and external-system facts that are intentionally not ledger projections. |
| Electron MessagePort equivalence                                                                                | implemented | Main-brokered control/stream `MessagePortMain` pairs connect the UI-only renderer to one utility-owned backend. The packaged smoke forces utility replacement, reconnect, exact replay, and a later accepted command.             |

## Codex Engine And Orchestration

| Requirement                                                                                                | Status      | Evidence and gate                                                                                                                                                                                                         |
| ---------------------------------------------------------------------------------------------------------- | ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Provider-neutral `Engine` contract, explicit capabilities, registry, and Effect Layers                     | implemented | Engine conformance, registry, fake-process, transcript, and import-boundary suites.                                                                                                                                       |
| Codex CLI subprocess integration                                                                           | implemented | `codex app-server` is spawned and driven over stdin/stdout, with `codex exec --json` fallback, bounded output, cancellation, cleanup, and raw-origin retention. No Agent SDK is used.                                     |
| Claude or other production execution adapters                                                              | deferred    | The concrete Claude adapter, exports, fixture CLI, transcript, and adapter tests are removed. Only provider-neutral seams and explicitly test-only fakes remain.                                                          |
| Engine discovery, auth state, start/resume/stream/steer/approve/cancel, timeout, crash, and orphan cleanup | implemented | Codex and fake lifecycle scenarios plus Windows process/job ownership tests.                                                                                                                                              |
| Normalized canonical surfaces and exact raw-origin attribution                                             | implemented | Work, Time, Guidance, Routine, Capability, Process, Change, Permission, native-action, usage, and lifecycle projections are renderer-safe and covered by orchestration and architecture tests.                            |
| Durable agent graph and intake policy                                                                      | implemented | Identity/roles, scope, policy, strict clarification, assumptions, fan-out, DAG gates, concurrency, retries, steering, stop/pause/resume, joins, usage, conflicts, recovery, and heartbeats are repository/service tested. |

## Backend Capabilities

| Requirement                                                                  | Status      | Evidence and gate                                                                                                                                                                                                                                      |
| ---------------------------------------------------------------------------- | ----------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Terminal/process ownership and lifecycle                                     | implemented | Backend terminal Services own PTY creation, input/output, restart, kill, exit, failure, cleanup, output pinning, and scoped watchers; real/deterministic driver suites pass.                                                                           |
| Controlled files, immutable diffs, review, rollback, and attributed evidence | implemented | Root-confined reads, conditional replacement, native fixed-root store, private rollback bytes, transactional authority, restart recovery, public protocol, and renderer methods are covered.                                                           |
| Git status/worktrees/diffs and approval-bound stage/unstage                  | implemented | Installed Git CLI is spawned with argv arrays and bounded I/O; durable projections, leases, recovery, literal paths, public routes, and real temporary-repository integration pass. Broader destructive Git commands remain intentionally unsupported. |
| Durable preview and rich-link services                                       | implemented | Target/inspection projections, loopback health probes, external launch, pinned-address metadata, content-addressed assets, binary transport, restart, security, and connector lifecycle tests pass. Artisan embeds no browser/WebView.                 |
| Artisan tool control plane                                                   | implemented | Policy-aware tools for questions, assumptions, terminals/processes, files, Git, previews, approvals, and native actions use durable invocation/approval/evidence projections and fail closed when unavailable.                                         |
| Thread metadata, retention, guidance, Model Behaviour, and project affinity  | implemented | Default retention/fences, refinement, canonical guidance/provider sync, Codex settings reconciliation, attributed affinity/rehome, restart, and MessagePort scenarios pass.                                                                            |
| Routine/skill Marketplace                                                    | implemented | Scoped registry, search/detail, compatibility, approval-bound inspection/install, rollback, enable/disable/remove, provider sync/drift, inert `npx skills` discovery, and runtime-only fallback are tested.                                            |
| MCP Capability Marketplace                                                   | implemented | Scoped stdio/HTTP sessions, secret references, OAuth lifecycle seams, health/discovery, policy, two-phase invocation approval, bounded artifacts, crash/restart/disconnect/uninstall, sync/drift, and invocation ledger are tested.                    |

## Electron And Frontend

| Requirement                                            | Status      | Evidence and gate                                                                                                                                                                                                             |
| ------------------------------------------------------ | ----------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Secure desktop process boundary                        | implemented | Single-instance Electron main owns the window and utility supervisor; one utility owns backend/SQLite/PTYS/Codex; context-isolated preload exposes a narrow bridge; a custom application protocol serves the static renderer. |
| Utility lifecycle and native packaging                 | implemented | Bounded restart/shutdown, port handoff, SQLite parent creation, staged `node-pty`, bounded native store, and Koffi bindings are verified in two utility epochs from the packaged artifact.                                    |
| Live UI-only renderer                                  | implemented | Production uses `ArtisanClient` and the desktop bridge, never fixtures or backend/Node imports. Replaying Effect state refreshes on ready/reconnect and fences stale subscriptions/actions.                                   |
| Monaco and workspace model                             | implemented | File discovery/read/conditional save, workspace-qualified models, markers, view-state restoration, preview/open/pinned/dirty/diff semantics, close consent, recents, changed files, overflow, and Quick Open are covered.     |
| Chat, orchestration, Marketplace, and session controls | implemented | Live transcript/actions, tool policy, terminal/preview/Git/change/settings surfaces, agent graph controls, Marketplace Routine/Capability lifecycle, identity, and activity state are wired.                                  |
| Shadcn-first interaction system                        | implemented | Interactive controls use the repository shadcn-svelte primitives and existing semantic tokens; source gates reject raw interactive HTML and component-local visual-system drift.                                              |
| Accessibility and responsive behavior                  | implemented | The packaged smoke proves trusted keyboard and native mouse activation, focus restoration, accessible names, wide/narrow pane state, right-pane keyboard reachability, truthful unavailable states, and Electron 200% zoom.   |
| Windows taskbar activity                               | implemented | Renderer activity state crosses the preload bridge to the main process and is asserted by the packaged product interaction gate.                                                                                              |

## Testing And Release

| Requirement                              | Status      | Evidence and gate                                                                                                                                                                                                                       |
| ---------------------------------------- | ----------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Deterministic ordinary validation        | implemented | `pnpm run validate` runs formatting, lint, typecheck, production frontend build, protocol/backend/engine/transport/frontend/deep tests, and no authenticated or paid Engine turn.                                                       |
| Deep integration and generated scenarios | implemented | Temporary workspace/Git/SQLite/public-protocol scenarios, fixed generated Engine state machines, restart/rebuild, concurrency, cleanup, and architecture/surface traversal are present.                                                 |
| Codex live policy                        | implemented | Live installed-CLI handshake/account-read verification is explicit, protected, non-billable, and off by default. No Claude or Agent SDK job exists.                                                                                     |
| Native addon policy                      | implemented | Native execution remains explicit opt-in outside ordinary CI, with the canonical Windows fixed-root read/replace/crash-recovery gate.                                                                                                   |
| Desktop package acceptance               | implemented | `package:desktop` builds the installer; `verify:desktop-package` inspects ASAR/unpacked layout and exercises the real packaged process, UI, utility restart, MessagePorts, durable replay, forward progress, cleanup, and native loads. |
| Release workflow                         | implemented | CI and manual release workflows use frozen installs, protected optional live/native jobs, the packaged desktop gate, sanitized fixture policy, and no temporary user data artifacts.                                                    |

## Dependency-Ordered Milestones

| Milestone                                                                      | State    |
| ------------------------------------------------------------------------------ | -------- |
| M0. Protocol/persistence stabilization and test seams                          | complete |
| M1. Wire protocol, routers, replay, snapshots, and rebuild                     | complete |
| M2. Terminal, files/diffs, Git, preview, approvals, and tools                  | complete |
| M3. Codex CLI Engine, fallback, normalization, and lifecycle                   | complete |
| M4. Fake process, transcript, live opt-in, and conformance harness             | complete |
| M5. Intake policy, AgentOrchestrator, graph, surfaces, and usage               | complete |
| M6. Thread metadata, retention, guidance, Model Behaviour, and affinity        | complete |
| M7. Routine and MCP Capability Marketplace                                     | complete |
| M8. Electron shell, utility process, MessagePorts, and live renderer           | complete |
| M9. Deep integration, generated, architecture, transport, and mounted UI gates | complete |
| M10. CI, release policy, installer, and packaged-process acceptance            | complete |

## Final Completion Audit

- [x] Every V1 envelope and renderer operation is schema-validated and routed through the typed client boundary.
- [x] Accepted domain commands are durable, correlated, semantically idempotent, replayable, and transactionally projected.
- [x] Codex CLI is the sole production Engine and is controlled as a subprocess over its native protocols; no Agent SDK is used.
- [x] Orchestration owns visible identity, intake, lifecycle, topology, steering, joins, approvals, usage, conflicts, and artifacts.
- [x] Terminal/process, files/diffs, Git, preview, tool, retention, guidance, behavior, affinity, Routine, and MCP slices are backend-owned and observable.
- [x] Electron negotiates and reconnects independent bounded control/stream transports across a forced packaged utility restart.
- [x] The live frontend uses renderer-safe contracts, shadcn-svelte primitives, semantic tokens, Monaco, and truthful live/unavailable states.
- [x] Deterministic unit, integration, generated, architecture, frontend, native-package, and mounted packaged-product gates pass without an ordinary live-model dependency.

## Current Verification Snapshot

Final local acceptance on 2026-07-22 passed `pnpm run validate` with 155 test files, 1,205 passing tests, and 2 explicit skips. `pnpm run package:desktop` then built the signed unpacked application and `Artisan Editor Setup 0.1.0.exe`. The exact-artifact verifier emitted one successful structured record after loading the mounted renderer and both native-runtime epochs. It proved trusted keyboard and native mouse interactions, Marketplace focus restoration, chat/editor/orchestrator switching, composer input, right-pane keyboard navigation, truthful no-file/no-terminal states, Windows activity signaling, wide/narrow layouts, 200% zoom, utility replacement with different PIDs, reconnect, semantic duplicate replay, forward journal progress, and mandatory temporary-data cleanup.
