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
- [x] Latest verified implementation checkpoint: `cc9bedc feat: execute controlled workspace replacements`
- [x] Local `HEAD`, upstream, and `origin/codex/backend-services` were equal at `cc9bedc` before this documentation update.
- [x] `ARTISAN_RUN_NATIVE_ADDON_SMOKE=1 pnpm --filter @artisan/bounded-file-store-native verify:local` is the canonical native gate and passes locally for production reads/replacement, test-hook races, and process-crash recovery.
- [x] Routine development verification is local-first. Do not recreate temporary GitHub Actions, remote runners, or similar testing detours; future CI is a real clean-checkout/release gate and never a substitute for local validation.
- [x] Model Behaviour is committed as focused protocol, persistence, provider, service, composition, transport, and security changes.
- [x] Local GitHub account: `sandersonstabo`; `origin` -> `https://github.com/sandersonstabo/artisan-editor.git`; GitHub visibility was verified as `PRIVATE` on 2026-07-12.
- [x] Commit every coherent, verified checkpoint as a small, focused, independently understandable change. Never bundle unrelated dirty work into the same commit and never push `main` or `master` without explicit approval.
- [x] Push the current feature branch to `origin` after every coherent commit and at natural checkpoints. Local worktrees are temporary working state, not durable project storage.
- [x] Check local and remote state at session start, after each push, and before handoff. Confirm the branch tracks `origin` and that local `HEAD` equals its upstream after every push.
- [x] The PRD now makes one visible shared checkout and selected branch a product guarantee. Artisan must coordinate writers through durable mutation claims and must never create hidden worktrees, per-agent branches, or temporary commits.

## Native Mutation Incident And Local Authorization

- [x] The user explicitly classified the BSOD as a one-off on 2026-07-12 and authorized local native execution. The prior host safety hold is lifted; keep the explicit `ARTISAN_RUN_NATIVE_ADDON_SMOKE=1` gate so ordinary validation never loads the addon silently.
- [x] Windows bugchecked at 08:45:50 with `0xD1 DRIVER_IRQL_NOT_LESS_OR_EQUAL`: `0x000000d1 (0x0, 0x2, 0x1, 0xfffff8052abb750f)`. WinDbg analysis of the preserved copy at `C:\Users\Sander\Desktop\071226-17781-01.dmp` attributes the null write directly to `rcbottom.sys+0x750f`, AMD-RAID Bottom Service version `9.3.3-00218`; the captured stack contains no Node, N-API, NTFS, or Artisan frame.
- [x] The dump's failure bucket is `AV_rcbottom!unknown_function`. `rcbottom.sys` is the boot storage driver used by this machine's actual RAID-backed disks, so it must not be disabled or removed as a workaround. Driver or BIOS changes require an explicit human decision, a current backup, and the motherboard/AMD recovery procedure.
- [x] Correlation with the native harness remains strong: generated addon files were written at 08:45:41, the first concurrent replacement root was created at 08:45:43, and that unfinished root retained a valid 1 MiB stage/backup/published-target receipt after reboot. Preserve `C:\Users\Sander\AppData\Local\Temp\artisan-native-replace-5hTN29`, the copied dump, and `C:\Users\Sander\Desktop\071226-bounded-file-store-native-gnu.node` (`SHA256 2FA0F3DE54816E5DABDD84E4A392E18E3A57BF558F48BBBDD6D43919C480E2E3`) as incident evidence.
- [x] Independent static ABI review found no Rust memory corruption, dangling handle, double close, or malformed EA record. It found four-byte-short `FILE_RENAME_INFORMATION` and `FILE_LINK_INFORMATION` allocations plus an incorrect secondary read of `IO_STATUS_BLOCK.Status` after synchronous `NtFlushBuffersFile`; all three are fixed in the accepted native implementation.
- [x] The corrected native crate passes `cargo fmt`, GNU default/test-hook `cargo check`, and GNU default/test-hook Clippy with warnings denied without loading or executing the addon.
- [x] Fresh independent reviews of the corrected native implementation, authenticated foreign-operation handling, and local build gate report no remaining P0-P2 finding.
- [x] `build:local:gnu` uses isolated GNU release outputs. `verify:local` runs production read/full replacement first, then one test-hook build for deterministic races and every crash window; all addon execution remains explicit opt-in.
- [x] The authenticated native mutation implementation is accepted for fixed local NTFS and process-crash recovery through the local gate. This does not claim power-loss atomicity or universal filesystem support.

## Active Work

- [x] Durable workspace change command/projection persistence is committed and pushed at `e7048cb`.
- [x] Transactional SQLite rollback snapshots are committed and pushed at `3eba329`.
- [x] Transactional run/agent/workspace authority is committed and pushed at `7cbe507`.
- [x] Recovery-only access to staged rollback bytes is committed and pushed at `b81fa19`.
- [x] The narrow `BoundedRegularFileStore` seam and explicitly non-adversarial Node adapter are committed and pushed at `98ef0b3`, `011dff5`, and `720cfaa`. Two-phase receipts retain the exact stage and original backup until SQLite durably records `applied`, then finalization removes them idempotently. Crash, corruption, mode, artifact, and concurrency coverage is green, including 50 synchronized exact-ID and 50 competing-ID stress runs. Independent final P0-P2 review is clean.
- [x] The private `@artisan/bounded-file-store-native` N-API package scaffold is committed and pushed at `dfc08bb`. Rust/NAPI versions are pinned, production output targets Windows x64 MSVC under `.dist`, and the pinned GNU development build proves direct binding load, generated CommonJS loader load, and generated declaration consumption without exposing any filesystem method. Independent P0-P2 review is clean.
- [x] Native Windows pinned-root bounded reads are committed and pushed at `e915763` and `d5ec0bc`. The N-API class accepts only absolute fixed-drive local NTFS roots, resolves children through open directory handles, rejects reparse/private/alias/multiply-linked paths, denies concurrent writers/deletion, and gives in-flight async reads an exact root lease across `close()`.
- [x] Native exact-handle conditional replacement and finalization on Windows local NTFS is complete and accepted with authenticated receipts, process-crash recovery, metadata preservation, deterministic race coverage, and best-effort flush hardening. The contract does not claim power-loss atomicity or universal filesystem support. Koffi remains rejected for this security-critical adapter because maintaining Windows NT ABI structures and syscalls in TypeScript is too fragile.
- [x] The scoped Effect adapter and opaque workspace bounded-store registry are committed and pushed at `78164fd`. The production addon loads only during Layer acquisition, validates the exact Windows x64 MSVC non-test descriptor, adapts native results through the existing `BoundedRegularFileStore` Service, closes pinned roots through `Effect.acquireRelease`, accepts only a caller-owned stable redacted 32-byte key, and canonicalizes every workspace root before native acquisition.
- [x] Terminal optimistic-concurrency rejection is committed and pushed at `247e7b5`. `Changed` transitions remain source-free, replay exactly after restart, reject forged journal/projection state, and never return a filesystem capability through mutation authority.
- [x] Transient expected/replacement byte-pair persistence is committed and pushed at `c143986` with its deep recovery harness at `dade5c0`. The private payload table is hash/length constrained, consumed into non-resurrectable tombstones, deleted during thread erasure, and never copied into commands, events, operations, or public projections.
- [x] Thread erasure now treats every unsettled or still-available workspace mutation as a durable retention fence at `bc37306`. Complete claim and erasure transactions retry bounded SQLite contention, stale post-quiescence claims release without erasing live work, and deterministic two-runtime barriers cover Stage and MarkApplied overlap.
- [x] Rejected mutations now discard exact payload and rollback-snapshot bytes into non-resurrectable tombstones at `54feea9`, after validating canonical operation/projection identity and private-row integrity. Exact retries and concurrent two-runtime cleanup converge without exposing content.
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

## Completed Non-Adversarial Conditional Replacement Checkpoint

Implemented, independently reviewed, committed, and verified:

- [x] `BoundedRegularFileStore` is a narrow Effect Service for bounded reads, recoverable conditional replacement, and receipt finalization; ordinary `Filesystem` and `WorkspaceFilesystemRegistry` expose none of its methods.
- [x] The Node adapter creates and syncs a private same-directory stage, preserves the original mode, moves the exact expected target to a unique backup, and publishes with no-overwrite hard-link semantics.
- [x] Successful publication retains stage and backup receipts until explicit finalization; exact retries recover staged, moved, and published crash windows without overwriting external targets.
- [x] Finalization is idempotent only for valid receipt states, fails closed for missing, external, ambiguous, corrupt, or impossible receipts, and preserves the sole backup when proof is incomplete.
- [x] Owned artifacts are reserved from direct reads and hidden from ordinary list/watch surfaces; unrelated similarly named files remain visible.
- [x] Focused conditional and registry suites pass 31 tests, plus 50 synchronized exact-ID stress runs and 50 synchronized competing-ID stress runs. Full validation passes 78 test files, 567 tests, and 3 intentional skips.
- [x] Independent final P0-P2 review is clean after fixing stage-missing/backup-present finalization.
- [x] Focused commits: `98ef0b3 feat: define bounded regular file store`, `011dff5 feat: add recoverable node file replacement`, and `720cfaa test: prove conditional file replacement recovery`.
- [ ] Production composition now depends on the Effect adapter around the accepted N-API class; the current Node adapter is deliberately named `non_adversarial` and remains referenced only by its focused harness.

## Completed Native Package Scaffold

Implemented, independently reviewed, committed, and verified:

- [x] Private workspace package `@artisan/bounded-file-store-native` uses NAPI-RS v3 and a pinned Rust 1.97.0 toolchain; Rust source, Cargo state, and package metadata remain isolated under `modules/bounded-file-store-native`.
- [x] Production build metadata targets `x86_64-pc-windows-msvc`; generated loader, declarations, native artifacts, and Cargo intermediates are written only beneath `.dist/bounded-file-store-native`.
- [x] A user-scoped GNU verification path uses WinLibs UCRT without treating its binary as production. It smoke-loads the direct binding and generated `index.cjs` package loader, then typechecks a consumer against the generated declarations.
- [x] The scaffold checkpoint exported only `getNativeBuildDescriptor`; filesystem capability was added later as a separately reviewed slice.
- [x] `cargo fmt --check`, GNU Clippy with warnings denied, frozen pnpm install, full repository validation, and independent P0-P2 review pass.
- [x] Local machine state: rustup 1.29.0, Rust 1.97.0 GNU/MSVC toolchains, and portable WinLibs UCRT 16.1 are installed. Visual Studio Build Tools did not install because its elevated bootstrapper could not proceed; production MSVC load remains an explicit CI/platform gate.
- [x] Focused commit: `dfc08bb build: scaffold native file store package`.

## Completed Native Pinned-Root Read Slice

Implemented, independently reviewed, committed, and verified:

- [x] `NativeBoundedRegularFileStore` pins one exact Windows root through `NtCreateFile`, permits only absolute fixed-drive roots on exact local NTFS, and owns its handle through an `Arc` lease.
- [x] Every child component is validated before normalization and opened relative to the preceding directory handle with `OBJ_DONT_REPARSE` and `FILE_OPEN_REPARSE_POINT`; traversal, absolute/device/UNC paths, ADS, invalid segments, trailing dot/space, and private artifact names fail closed.
- [x] Normalized handle names reject 8.3 aliases for `.artisan-trash` and `.artisan-conditional-*`. Multiply-linked leaves are rejected so private artifacts cannot be read through a hard-link alias and later replacement semantics remain unambiguous.
- [x] Leaf reads require a single-link non-directory, non-reparse file; keep every parent handle alive, deny write/delete sharing on the leaf, bound allocation and `ReadFile`, and compare same-handle identity and metadata before and after every read, including empty files.
- [x] N-API validates JavaScript numeric bounds before narrowing, runs disk I/O through `AsyncTask`, generates `Promise<Uint8Array>`, rejects new reads after deterministic `close()`, and lets an already-created read finish through its cloned root lease.
- [x] The opt-in GNU harness covers raw bytes, exact/invalid/oversize bounds, nested and empty files, missing/directory paths, traversal/device/ADS syntax, junction roots and parents, real 8.3 aliases, hard-link aliases, existing writers, opaque errors, repeated close, and an in-flight read surviving close.
- [x] Ten consecutive native build/loader/type/runtime smoke runs pass. Rust format, GNU Clippy with warnings denied, and full repository validation pass with 78 Vitest files, 567 passing tests, and 3 intentional skips.
- [x] Independent review found and closed initial P1 gaps for 8.3/hard-link aliases and explicit local-volume admission; final P0-P2 re-review is clean.
- [x] Focused commits: `e915763 feat: read files through pinned native roots` and `d5ec0bc test: prove native root confinement`.
- [x] Production MSVC compilation/loading passed in the historical one-off runner before that temporary workflow was removed. The maintained runtime gate is local GNU; MSVC and Electron packaging remain future release-platform checks.

## Completed Native Exact-Handle Replacement And Finalization

Implemented, independently reviewed, committed, and verified:

- [x] The N-API constructor binds one pinned root to an exact 32-byte receipt key. Replacement and finalization validate the operation id, normalized relative path, expected/replacement bytes, and byte limit before asynchronous native work begins.
- [x] Root-derived HMAC-SHA256 NTFS EA receipts authenticate the namespace, expected/replacement digests, exact file identities, peer identities, and lifecycle role. Wrong keys, corrupt markers, replayed artifacts, cross-root artifacts, and ambiguous state fail closed.
- [x] Exact-handle no-overwrite rename/link operations preserve the original backup and replacement stage until SQLite can durably record application and request idempotent finalization. Recovery covers creating-stage, staged, backup, published, finalizing, and restoring states.
- [x] Stages inherit ordinary file attributes and semantically equivalent owner/group/DACL security. Raw descriptors remain available for creation/application while canonical SDDL comparison avoids false failures from equivalent Windows descriptor encodings.
- [x] Production replacement coverage passes changed targets, validation, authenticated recovery, corrupt/unmarked collisions, tamper/replay, wrong keys, Windows hidden/read-only attributes, protected DACLs, close leases, and case-insensitive namespaces.
- [x] A deterministic foreign-operation regression proves that a second operation returns `Changed` only after authenticating the first operation's root receipt and exact self identity; wrong-key, corruption, replay, and same-operation changed-intent paths remain failures.
- [x] The test-hook build retains only proof-bearing two-party race rendezvous and crash hooks. Five same-operation and five competing-operation races pass with exact outcomes, and all eleven distinct process-crash windows recover correctly.
- [x] Local command `ARTISAN_RUN_NATIVE_ADDON_SMOKE=1 pnpm --filter @artisan/bounded-file-store-native verify:local` builds dedicated production/test-hook GNU release outputs, validates direct and generated loaders plus declarations, then passes reads, full replacement/recovery, deterministic races, and process-crash recovery.
- [x] Diagnosis-only phase tracing was removed after acceptance. Fresh independent review found no P0-P2 cleanup regression and confirmed all 13 crash call sites plus the replacement rendezvous remain unchanged.
- [x] GNU default/test-hook checks and Clippy pass with warnings denied. The local verifier restores its caller environment, rejects reparse output children, never clobbers MSVC output, and removes temporary loader aliases on success or failure.
- [x] Focused commits: `68fb9ed fix: recognize competing native receipts` and `fc3e818 test: verify native mutation locally`.
- [x] The temporary GitHub Actions workflow was removed at the user's request and must not be recreated for routine testing. The canonical native gate passed locally again on 2026-07-12 without another bugcheck; the original `rcbottom.sys` incident evidence remains preserved as history.

## Completed Scoped Native Effect Adapter

Implemented, independently reviewed, committed, and verified:

- [x] `BuildNativeBoundedRegularFileStore` dynamically loads the addon only when its scoped Layer is acquired; ordinary backend imports and validation never evaluate the native package.
- [x] The adapter rejects GNU, test-hook, wrong-platform, malformed module, malformed instance, malformed read/replace/finalize result, and invalid key states before exposing a bounded store.
- [x] Every opened native root is released exactly once through `Effect.acquireRelease`, including partial multi-root registry construction failures. Temporary JavaScript key copies are zeroed without mutating the caller-owned redacted key.
- [x] `WorkspaceBoundedRegularFileStoreRegistry` uses Effect `FileSystem.realPath` and `stat`, rejects malformed registrations, duplicate IDs, duplicate canonical roots, and non-directory roots before native acquisition, and exposes only opaque workspace IDs plus bounded store capabilities.
- [x] A clean-checkout simulation removed `.dist/bounded-file-store-native` while typecheck and the public adapter suite passed, then restored the output in `finally`. The focused suites pass 14 tests without loading the real addon.
- [x] Full validation passes 80 test files, 581 tests, and 3 intentional skips. Fresh independent P0-P2 review is clean.
- [x] Focused commit: `78164fd feat: adapt native workspace file stores`.

## Completed Durable Mutation Recovery Prerequisites

Implemented, independently reviewed, committed, and verified:

- [x] `WorkspaceChangeRepository` now has a terminal `rejected` lifecycle for native `Changed`. Exact replace/rollback retries return the durable rejection without creating a command, event, or change projection.
- [x] Rejected rows fail closed if forged command, event, replace projection, consumed rollback projection, journal sequence, evidence, or review state is present. `WorkspaceMutationAuthority` maps the terminal retry to typed `operation_rejected` and returns no capability-bearing admission.
- [x] `WorkspaceMutationPayloadStore` privately stages the exact expected/replacement byte pair for replace and rollback recovery, binds both identities to the canonical operation/projection, recomputes SHA-256 on read, and returns fresh byte copies.
- [x] The generated SQLite table constrains available/consumed shape, byte lengths, four-MiB bounds, and lowercase hashes. Committed consumption nulls every sensitive field while retaining a tombstone that prevents resurrection.
- [x] Thread erasure preserves pending, applied, corrupt, or byte-bearing workspace mutations, releases stale claims after quiescence, and erases only fully settled terminal rows whose private bytes are gone.
- [x] Rejected replace and rollback cleanup consumes payloads and replace snapshots only after exact lifecycle, action, thread, identity, and projection checks; absence and tombstones are idempotent without permitting resurrection.
- [x] Focused recovery coverage includes malformed and corrupt rows, restart, erasure claims/tombstones, real thread erasure, source-free public persistence, exact two-runtime staging, deterministic Stage/MarkApplied erasure overlap, and synchronized two-runtime consumption races.
- [x] Full validation passes 82 test files, 674 tests, and 3 intentional skips. Drizzle generation is idempotent, migration integrity passes, and fresh independent P0-P2 reviews are clean.
- [x] Focused commits: `247e7b5 feat: persist rejected workspace changes`, `c143986 feat: store workspace mutation payloads`, `dade5c0 test: prove mutation payload recovery`, `bc37306 fix: preserve pending workspace mutations`, and `54feea9 feat: discard rejected mutation bytes`.

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
- [ ] Add controlled filesystem read/replace/review/rollback execution and public protocol commands. Native Layers, terminal rejection, and exact recovery payload persistence now exist; execution/recovery composition remains.
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
- [x] Production conditional file mutation uses a focused Rust N-API adapter with handle-relative platform operations. Windows x64 on local NTFS is the first target and guarantees process-crash recovery, not power-loss atomicity. The path-based Node implementation remains a non-adversarial development and deterministic-test adapter only.
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
