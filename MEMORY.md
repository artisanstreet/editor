# Artisan Editor Memory

Last updated: 2026-07-12

## Mission

Build the complete Artisan Editor backend/core and deep testing harness described by `artisan-editor-prd.md`. The backend is provider-neutral, Effect service/layer based, SQLite/Drizzle backed, and connected to a UI-only frontend through typed MessagePort RPC/events. Codex CLI and Claude Code CLI are the V1 engines; provider-native data is normalized while raw origin remains attributable.

This project is not complete until the final audit in `backend-completion-matrix.md` is proven requirement by requirement.

## Working State

- [x] Repository: `C:\Users\Sander\Desktop\artisan-editor`
- [x] Branch: `codex/backend-services`
- [x] Package manager: pnpm 11.7.0
- [x] Stack: TypeScript 7, Effect 4 beta, Drizzle 1 RC, SQLite
- [x] Latest code checkpoint: `b81fa19 feat: resume uncommitted workspace snapshots`
- [x] Model Behaviour is committed as focused protocol, persistence, provider, service, composition, transport, and security changes.
- [x] Private remote: `origin` -> `https://github.com/sandersonstabo/artisan-editor.git`; GitHub visibility was verified as `PRIVATE` on 2026-07-11.
- [x] Active work is committed in small, focused, independently understandable checkpoints and pushed to the current feature branch immediately after verification; never push `main` or `master` without explicit approval.
- [x] Local and remote state are checked at session start, after each push, and before handoff. Confirm the current branch tracks `origin` and that local `HEAD` matches its upstream after pushing.

## Active Work

- [x] Durable workspace change command/projection persistence is committed and pushed at `e7048cb`.
- [x] Transactional SQLite rollback snapshots are committed and pushed at `3eba329`.
- [x] Transactional run/agent/workspace authority is committed and pushed at `7cbe507`.
- [x] Recovery-only access to staged rollback bytes is committed and pushed at `b81fa19`.
- [ ] Finish and review the uncommitted conditional regular-file primitive in `filesystem.ts`, `node-filesystem.ts`, and `filesystem-conditional-replace.test.ts`. Its first worker pass has a known post-publication recovery hole: retries after cleanup can misclassify an already-applied exact replacement, and stage/backup cleanup crash windows are not all recoverable yet.
- [ ] Build the controlled read/replace/review/rollback service around the filesystem registry, workspace-change repository, snapshot store, evidence recorder, and public protocol.
- [ ] Prove filesystem mutation crash windows, exact retries, authorization races, and restart recovery through the real production composition.

## Completed Checkpoints

- [x] Versioned protocol envelopes, negotiation, receipts, events, ACKs, replay, heartbeats, and bounded stream transport.
- [x] SQLite WAL persistence, Drizzle migrations, journal command deduplication, stream sequencing, and core projections.
- [x] Codex app-server engine with exec JSONL fallback and raw-origin normalization.
- [x] Claude Code print-mode stream JSON engine with truthful capability limits.
- [x] Shared engine conformance, fake process, transcript replay, lifecycle, cancellation, timeout, and cleanup harnesses.
- [x] Durable thread identity, default seven-day retention, erasure fences, restart recovery, and activity tracking.
- [x] Canonical global guidance with Codex/Claude discovery, first-run selection, backups, drift handling, runtime handoff, and privacy tests.
- [x] First-class orchestration graph with playful names, roles, assignments, fan-out, DAG dependencies, joins, retries, steering, stop, recovery, artifacts, and heartbeats.
- [x] Terminal session service with real/fake PTY coverage, lifecycle persistence, input, resize, clear, restart, kill, and cleanup.
- [x] Engine-neutral filesystem, Git/process, preview target, rich-link, and network-policy service foundations.
- [x] Live thread metadata refinement with bounded context, latest-wins scheduling, locks, source idempotency, and restart replay.
- [x] Project affinity scoring/rehome core with Git-root discovery, recency windows, locks, suggestions, linked projects, and journal coordination.
- [x] Curated Model Behaviour registry with Codex config reconciliation, capability probing, drift handling, private backups, and typed MessagePort control.

## Completed Rollback Snapshot Foundation

Implemented, independently reviewed, committed, and verified:

- [x] SQLite-backed private rollback bytes with strict size, state, content-length, and lowercase SHA-256 constraints.
- [x] Snapshots bind transactionally to the canonical replace claim, thread, and recorded before identity; committed claims are cross-checked against their projection.
- [x] Read and Exists require a committed replace with rollback still available; Consume requires a matching applied or committed rollback operation.
- [x] Stage is allowed before replace commit and cannot recreate a missing or consumed snapshot afterward.
- [x] Consumed snapshots erase bytes and identity metadata, remain non-resurrectable through the service, and are deleted atomically during thread erasure.
- [x] Effect-classified SQLite lock contention uses a bounded exponential retry around the complete write transaction, rechecking thread erasure and authority on every attempt.
- [x] Deterministic transaction-attempt barriers prove real Stage and Consume retries; cross-runtime races and erasure-during-retry passed 20 repeated runs.
- [x] The production backend runtime exposes both `WorkspaceChangeRepository` and `WorkspaceSnapshotStore`; a real composition test claims and stages through that runtime.
- [x] Independent final P0-P2 review is clean.
- [x] Last full validation: 76 test files, 533 passed, 3 intentionally skipped; format, lint, typecheck, migration generation, and Drizzle integrity checks passed.
- [x] Focused commit: `3eba329 feat: store rollback snapshots transactionally`.

## Completed Snapshot Recovery Read Checkpoint

Implemented, independently reviewed, committed, and verified:

- [x] `WorkspaceSnapshotStore.Resume` exposes staged private bytes only to the exact live canonical replace while its lifecycle is `claimed` or `applied` and no committed projection exists.
- [x] The recovery read binds thread, change, canonical before identity, snapshot metadata, content length, and a fresh SHA-256 digest; missing, consumed, corrupt, cross-thread, committed, erased, or malformed state fails closed without leaking content.
- [x] Ordinary `Read` remains committed-only, so recovery access does not broaden the public rollback read contract.
- [x] Focused lifecycle, absent-snapshot, wrong-identity, wrong-thread, consumed, erasure, corruption, and null-input coverage passes 19 tests.
- [x] Independent P0-P2 review is clean. Residual harness work is to add deterministic two-runtime `Resume` races against replace commit and thread-erasure admission, plus direct lifecycle/projection-corruption cases.
- [x] Full worktree validation passed with 78 test files, 553 passing tests, and 3 intentionally skipped tests; format, lint, and typecheck passed. The run also included the still-uncommitted conditional-filesystem worker files, which remain under review rather than accepted by this checkpoint.
- [x] Focused commit: `b81fa19 feat: resume uncommitted workspace snapshots`.

## Completed Workspace Mutation Authority Foundation

Implemented, independently reviewed, committed, and verified:

- [x] One deep `WorkspaceMutationAuthority.ClaimReplace` interface hides base-run, graph-run, registry, repository, and SQLite transaction details from callers.
- [x] Base authority requires the live coordinator, active run, thread, agent, running/waiting state, and a registered canonical workspace root.
- [x] Graph authority derives assignment/group identity from the database and requires a running group, active dispatch, live run/assignment, matching agent/thread/workspace, write policy, and `repo` or `files` scope.
- [x] Effect `FileSystem.realPath` proves canonical workspace roots while returned capabilities omit absolute-path resolution; repo aliases are accepted only when they resolve to the registered root.
- [x] The workspace operation and immutable authority pin commit in one outer SQLite transaction; the existing repository transaction nests as an Effect SQL savepoint.
- [x] Exact retries use the authority pin after a run becomes terminal, reject changed authority/intent and unpinned legacy claims, revalidate pinned scope, and acquire a same-row write fence before liveness admission.
- [x] Bounded SQLite contention retry re-runs the complete proof. Deterministic stale-run, new-claim erasure, exact-retry erasure, and two-runtime convergence scenarios passed 10 consecutive final runs.
- [x] Malformed assignment and pinned-authority state fails closed; denial/conflict/failure errors do not expose workspace roots or requested paths.
- [x] Thread erasure deletes authority pins before their parent operations, and the production backend runtime exposes the registry and authority service.
- [x] The reviewer-discovered exact-retry/erasure stale-snapshot P1 was fixed with the write fence; independent final P0-P2 re-review is clean.
- [x] Last full validation: 77 test files, 542 passed, 3 intentionally skipped; format, lint, typecheck, migration generation, and Drizzle integrity checks passed.
- [x] Focused prerequisite commits: `4ff58ec refactor: share sqlite write retries` and `2615311 feat: authorize registered workspace roots`.
- [x] Focused authority commit: `7cbe507 feat: authorize workspace mutation claims`.

## Completed Model Behaviour Milestone

Implemented, independently reviewed, committed, and verified:

- [x] Curated provider-neutral capability schema for auto-compaction trigger tokens.
- [x] Provider support, scope, activation timing, unavailable/unsupported states, and `drift_ignored` projection.
- [x] Structured Codex TOML parsing/patching with `@decimalturn/toml-patch` owned by `@artisan/backend`.
- [x] Canonical SQLite setting and metadata-only provider reconciliation repository/migration.
- [x] Content-free hashes; full provider config remains ephemeral and is never persisted to SQLite/events.
- [x] Codex behavioral capability probe using isolated `CODEX_HOME`; non-zero exits fail closed.
- [x] Query/update/drift import/overwrite/ignore/retry service with exact operation fingerprints.
- [x] Public protocol routes and typed `ArtisanClient` MessagePort methods.
- [x] Desktop composition, unsupported-provider disabling, exact retry, stale drift, crash-window recovery, and reconnect replay tests.
- [x] External edit races before claim, after claim, and immediately after publication have regression coverage.
- [x] Exact bigint file identities and descriptor-bound POSIX/Windows permission mutation prevent path-substitution races.
- [x] Empty files are created privately and atomically before bytes are written; Windows uses the `FileSecurity` creation constructor.
- [x] Replacements are fully written and synced in a private staging path before no-overwrite publication.
- [x] The dedicated backups directory is current-user-only before backups, permission records, or publication anchors are created.
- [x] Rollback restores or erases through pinned identities and never deletes an external replacement.
- [x] Last full validation: 70 test files, 466 passed, 3 intentionally skipped; format, lint, and typecheck passed.

Closed review and delivery gates:

- [x] External replacement, backup collision, recovery, permission failure, write failure, rollback, and Windows ACL regressions are covered.
- [x] Focused Model Behaviour suites pass.
- [x] `pnpm run validate` passes across the complete repository.
- [x] Independent P0-P2 re-review of staged publication, hard-link restoration, and backup-directory privacy is clean.
- [x] The milestone was split into focused commits from `917ed8e` through `1ecade0`.

## Completed Project Affinity Evidence Milestone

Implemented, independently reviewed, committed, and verified:

- [x] Canonical filesystem mutation, process ownership, Git root/worktree/branch/diff, explicit project mention, and metadata evidence kinds.
- [x] `WorkspaceEvidenceRecorder` validates tool input with Effect Schema, preserves run/agent/raw-origin attribution, rejects changed operation intent, and returns exact duplicates across restart.
- [x] Public project mentions are resolved through `ProjectLocator`; forged project identities are discarded and only the canonical reference is persisted as affinity evidence.
- [x] Automatic rehome requires corroborating high-integrity journal events, so one dirty Git inspection cannot move a thread by itself.
- [x] Metadata evidence is emitted through the real refinement worker/repository path rather than fabricated directly in tests.
- [x] Historical affinity evidence replays after restart without treating its old projection basis as changed intent.
- [x] A real MessagePort subscription and list query observe the multi-source rehome projection.
- [x] Independent P0-P2 review is clean after the replay fix; the two-runtime regression passed 20 repeated runs.
- [x] Last full validation: 71 test files, 483 passed, 3 intentionally skipped; format, lint, and typecheck passed.
- [x] Focused commits: `3da881a feat: complete project affinity evidence` and `7defe4b feat: record attributed workspace evidence`.

## Remaining Backend Work

### Project Affinity Follow-Through

- [ ] Bind future controlled filesystem, Git, and process tool adapters to `WorkspaceEvidenceRecorder`; raw watcher activity remains deliberately non-authoritative. This is tracked with the M2 Files/Git work below.

### Marketplace: Skills And MCP

- [ ] Build the canonical Routine/skill registry with source, version, scope, permissions, compatibility, files, enable/disable/remove, and progressive disclosure.
- [ ] Add skill install preview, approval, rollback, provider sync, drift detection, and runtime-only fallback.
- [ ] Integrate first-class `npx skills` discovery/install without making its format canonical.
- [ ] Build the canonical MCP Capability registry for stdio and HTTP transports.
- [ ] Add MCP auth/secrets, OAuth/token lifecycle, health, tools/resources, scope, approvals, policy, sync/drift, invocation events, crash/restart, and uninstall.
- [ ] Expose Marketplace under the sidebar `New chat` action in the future frontend.

### Files, Git, Surfaces, And Orchestration

- [x] Add attributed controlled-write operation history, changed-file/diff projections, review/rollback state, and private rollback snapshot persistence.
- [x] Add atomic run/agent/workspace authorization and durable mutation-claim pinning.
- [ ] Add controlled filesystem read/replace/review/rollback execution and public protocol commands.
- [ ] Add Git worktree inventory, durable change/session projections, approved mutations, and public protocol commands.
- [ ] Complete the canonical surface taxonomy: Work, Time, Guidance, Routines, Capabilities, Processes, Changes, Permissions, and native actions.
- [ ] Add explicit intake risk classification, assumptions, usage aggregation, and workspace conflict/review handling.

### Preview And Electron

- [ ] Add durable preview projections, production local health probe, binary asset routes, and browser inspection lifecycle.
- [ ] Build the real Electron main/utility/renderer bootstrap around the existing shell-neutral MessagePort transport.
- [ ] Add packaged-process restart/equivalence tests and single-instance ownership.
- [ ] Build the SvelteKit/Vite+ frontend and Monaco editor after backend contracts are sufficiently stable.

### Deep Harness And Release

- [ ] Add deterministic projection rebuild from the event ledger and equivalence tests.
- [ ] Add generated property/state-machine scenarios spanning retries, exits, reconnects, rebuilds, and concurrent agents.
- [ ] Add one deep public-protocol workspace/Git/SQLite/fake-engine integration scenario.
- [ ] Add complete architecture-boundary and surface-normalization suites.
- [ ] Add CI/release workflows; live CLI probes remain explicit opt-in and never run silently in ordinary CI.

## Product And Architecture Decisions

- [x] Electron is the V1 desktop shell; SvelteKit/Vite+ is the frontend; Monaco is the editor.
- [x] Artisan never embeds a browser/WebView. Local web previews open in the user's external browser; Artisan owns only preview lifecycle, URL state, health, and attributable automation.
- [x] Frontend is UI-only. Backend owns provider processes, terminals, filesystem/Git, persistence, policy, and reconciliation.
- [x] Control uses typed MessagePort request/result plus durable events; streams use separate bounded MessagePorts.
- [x] SQLite is the journal/projection store. Full provider configs, credentials, and secrets never enter journal payloads.
- [x] Provider CLIs remain the optimized native harnesses. Artisan adapts their I/O rather than replacing them with a generic API-key harness.
- [x] Canonical Artisan surfaces and registries map to provider-native capabilities with truthful unsupported/runtime-only states.
- [x] Model picker, behavior controls, Git state, and other lower-frequency controls belong in the right pane.
- [x] Threads have live titles/status, default seven-day inactivity deletion, and project affinity/rehome behavior.
- [x] Orchestration is first-class and uses user-supplied playful agent names rather than exposing provider monotony.
- [x] Keep settings curated and opinionated. Do not expose every provider config key as a generic form.

## Required Development Method

- [ ] Read `AGENTS.md` and this file at the start of every session.
- [ ] Always apply `C:\Users\Sander\.codex\skills\sanders-skill\SKILL.md`.
- [ ] Main Sol coordinates architecture, decomposition, integration, communication, and final verification.
- [ ] Never use fast mode or priority service tier for subagents.
- [ ] Use Terra medium for implementation/debugging/integration/review grunt work.
- [ ] Use Luna medium for mechanical/focused grunt work.
- [ ] Use another Sol only for a critical review or to unblock work that Terra cannot complete after a better brief.
- [ ] Keep worker write scopes disjoint and require focused verification reports.
- [ ] Update this memory whenever checked state or remaining work changes.
- [ ] Check the tracked remote at session start and before handoff. Fetch when needed, report divergence, and never overwrite remote work implicitly.
- [ ] Keep commits small, focused, coherent, and independently understandable. Commit each verified checkpoint instead of accumulating an entire milestone or mixing unrelated changes.
- [ ] Push the current feature branch to `origin` immediately after every verified coherent commit and at every natural checkpoint. Do not leave completed commits only on the local machine.
- [ ] After every push, verify that local `HEAD` equals the upstream branch head and record the new checkpoint in this file.
