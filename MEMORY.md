# Artisan Editor Memory

Last updated: 2026-07-14

## Mission

Build the complete Artisan Editor backend/core and deep testing harness described by `artisan-editor-prd.md`. The backend is provider-neutral, Effect service/layer based, SQLite/Drizzle backed, and connected to a UI-only frontend through typed MessagePort RPC/events. Codex CLI and Claude Code CLI are the V1 engines; provider-native data is normalized while raw origin remains attributable.

This project is not complete until the final audit in `backend-completion-matrix.md` is proven requirement by requirement.

## Working State

- [x] Repository: `C:\Users\Sander\Desktop\artisan-editor`
- [x] Branch: `codex/workspace-replace-approval`
- [x] Package manager: pnpm 11.7.0
- [x] Stack: TypeScript 7, Effect 4 beta, Drizzle 1 RC, SQLite
- [x] Latest verified implementation checkpoint: `ad14f53 feat: connect preview targets`
- [x] Local `HEAD`, its upstream, and `origin/codex/workspace-replace-approval` must be equal after every checkpoint push and before handoff; this was last verified on 2026-07-15 at `ad14f53 feat: connect preview targets`.
- [x] `ARTISAN_RUN_NATIVE_ADDON_SMOKE=1 pnpm --filter @artisan/bounded-file-store-native verify:local` is the canonical native gate and passes locally for production reads/replacement, test-hook races, and process-crash recovery.
- [x] Routine development verification is local-first. Do not recreate temporary GitHub Actions, remote runners, or similar testing detours; future CI is a real clean-checkout/release gate and never a substitute for local validation.
- [x] Reconfirmed on 2026-07-14: no `.github` workflow or other remote-testing detour remains, and the canonical native gate passes locally. The earlier BSOD remains classified as a one-off storage-driver incident, not a reason to move routine testing off-machine.
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

- [x] Hosted-project clone persistence is committed and pushed at `70f7431`. The generated SQLite migration, Effect repository/layer, public approval journal, private provider/destination artifacts, destination and hosted-identity claims, exact request/decision replay, external execution gate, owner leases, restart takeover, post-clone continuation, unknown-outcome quarantine, reuse identity binding, corruption checks, and source redaction are covered by 10 real-SQLite tests. The focused neighboring suites pass 35 tests; local lint, TypeScript, frontend production build, and the full 131-file suite pass with 1,152 tests and 4 intentional skips. The aggregate validation command is currently blocked only by the unrelated dirty formatting of `backend-completion-matrix.md`; do not rewrite that user-owned hunk as part of this milestone.
- [x] Hosted-project clone coordination is committed and pushed at `dc94a5b`, `668e3af`, and `1191b81`. The scoped Effect coordinator reuses registered projects, prepares and durably approves new clones, executes the provider once behind the process-safe gate, heartbeats and abandons leases, resumes safe post-clone registration/attachment after restart, quarantines unknown external outcomes, and rejects changed-intent command replay. The production runtime, protocol server, typed `ArtisanClient`, fixture client, thread quiescer, and erasure transaction are wired; source-free query/decision routes survive backend restart and renderer reconnect, pending clone states fence retention, terminal thread-owned clone state and affinity evidence are erased, and the global project catalog survives. The focused 11-file matrix passes 49 tests; lint, TypeScript, and the production frontend build pass; the complete local `pnpm exec vitest run --maxWorkers=1` gate passes 135 files with one intentional file skip, 1,164 tests, and 4 intentional test skips. Unrestricted workers saturated this Windows host's real-Git integration tests, while all six affected suites passed with one worker; no remote runner was used. The aggregate format check remains blocked only by the unrelated user-owned formatting in `backend-completion-matrix.md`. Independent Terra review found no P0-P2 issue.
- [x] Hosted pull-request review and CI projection is committed and pushed at `05ba278` and `9b14a40`. The canonical `GitProvider` read contract and V1 `GitHubProvider` use bounded shell-free `gh api graphql` calls to associate a branch and then read one exact-head pull request, normalizing reviews, review requests, threads, checks, workflows, status contexts, required-check flags, and bounded annotations. SQLite stores one project-global snapshot plus exact thread-owned operation replay; process-safe concurrency, restart, changed-intent rejection, source-free events, and thread erasure are covered. Refresh events remain `unverified`, while live queries compare the visible branch/head and return `current` or `stale_local_git`; authentication and rate-limit failures preserve safe provider meaning through ProtocolServer and the typed `ArtisanClient`. The final local gate passes 141 test files with one intentional file skip, 1,185 tests, and 4 intentional test skips; TypeScript, Oxlint, scoped formatting, production frontend build, and the explicit native gate pass. Independent Terra review and remedy re-review report no P0-P2 issue; no remote runner was used.
- [x] Durable external-wait source closure and exact-head observation are committed and pushed at `72fd855` and `301e365`. Owner-tagged live closure and terminal-run reconciliation cannot cross ordinary/graph run tables; observation and wake leases are process-safe; exact wake replay is idempotent while changed intent fails; startup and periodic cycles use scoped Effect scheduling, exact provider/account/project binding, a lease-safe timeout, and canonical visible suspensions. The focused local matrix passes 56 tests across 10 backend suites, TypeScript, Oxlint, formatting, and diff checks; no remote runner or workflow was used. The remaining reviewed P1 is the planned atomic wake dispatcher: `wake_pending` is durable but does not become a native-resume or linked continuation yet.
- [x] Durable run-continuation intent is committed and pushed at `6aaf32a`. Engine resume tokens now strict-decode with bounded provider state before either ordinary or graph persistence, native thread identity must match, ordinary and graph rows persist start/resume mode, graph continuations have a separate monotonic index without consuming retry attempts, and a populated pre-migration database proves every legacy value and new default survive. The focused local matrix passes 30 tests across 5 suites; TypeScript, Oxlint, formatting, migration generation, and independent P0-P2 review are clean. Atomic wake materialization and dispatch remain the active slice.
- [x] Atomic wake materialization and continuation opening are committed and pushed at `9963a47`. One SQLite transaction now strict-validates the claimed wake and immutable source evidence, chooses native resume only from a valid persisted token plus the caller's exact supported-capability snapshot, creates ordinary or graph continuation state, terminalizes the source, settles the wake, and publishes the visible `woken` projection. Ordinary and graph orchestrators strict-read persisted start/resume intent and pass exact resume tokens into `Engine.Open`; late duplicate delivery survives later coordinator progress while changed immutable evidence fails closed. Twelve neighboring local backend suites pass 79 tests; TypeScript, scoped Oxlint, formatting, and diff checks pass. Independent review found the remaining production P1/P2 at the next planned boundary: no dispatcher yet claims/materializes wakes from the live runtime, and no end-to-end test yet proves wake-to-engine delivery or post-materialization capability drift. No remote runner or workflow was used.
- [x] Production external-wake dispatch is committed and pushed at `e5f3189`. A first-class Effect Service/Layer serializes cycles, claims durable wakes across processes, resolves the claimed owner's exact Engine, releases missing-engine claims, passes the exact resume-capability snapshot into atomic materialization, isolates corrupt rows without starving later wakes, and always nudges ordinary and graph orchestrators so restart repairs a crash between commit and notification. Its one-second Effect Schedule waits a full interval after the explicit startup cycle, owns one scoped fiber, and startup survives typed corrupt-wake failure while direct `RunOnce` remains observable. A real provider observation now reaches exact-token `Engine.Open`; a deterministic two-read capability test proves post-materialization drift fails the durable native continuation with no linked fallback. Fourteen neighboring suites pass 89 tests; the full local gate passes 150 files with one intentional file skip, 1,290 tests, and 4 intentional test skips. Root lint, TypeScript, and the production frontend build pass; scoped formatting and diff checks pass, while the aggregate formatter remains blocked only by the unrelated user-owned `backend-completion-matrix.md` hunk. Independent review and remedy re-review report no P0-P2 finding. No remote runner or workflow was used.
- [x] External-wait application and renderer controls are committed and pushed at `a4f6572`. A first-class Effect Service derives ordinary or graph ownership from the durable source run, exact-replays accepted request intent before provider access, reads fresh hosted state with a before/after local-head fence, and exposes typed request, cancel, manual-resume, and query routes through ProtocolServer and `ArtisanClient`. Real SQLite/Git/MessagePort coverage proves restart replay, graph ownership, cross-thread rejection, manual dispatch failure safety, typed client control, and a live provider-read commit race that creates no wait and leaves the source run running. Twelve neighboring local suites pass 82 tests; TypeScript, Oxlint, and the production frontend build pass. Independent Terra review found no P0-P2 issue. No remote runner or workflow was used.
- [x] External-wake erasure quiescence is committed and pushed at `b70a7ec`. Wake discovery carries thread ownership into a permanent Effect dispatch fence before any durable claim; thread erasure drains admitted claim/materialize/release work, later same-thread discoveries skip before claim, and unrelated threads continue dispatching. `ThreadResourceQuiescer` and the external-wait service share the memoized production dispatcher Layer. Deterministic race tests pass; the focused 12-file matrix passes 85 tests; the complete local gate passes 152 files with one intentional file skip, 1,304 tests, and 4 intentional test skips. TypeScript, Oxlint, scoped formatting, diff checks, and the production frontend build pass. Independent Terra review found no P0-P2 issue. No remote runner or workflow was used.
- [x] Hosted check failure drill-down is committed and pushed at `030a6b3`, `e922270`, `fc62c9f`, `dbf85bc`, `34f694a`, and `b54f873`. Provider-neutral bounded output/log schemas, the optional `GitProvider` read seam, and the GitHub adapter bind every read to the selected account, repository, pull request, branch, exact head, check run, check suite, and Actions job. The application service fences the durable snapshot and visible checkout before and after provider I/O, rejects concurrent refresh, returns detail transiently without writing raw output or logs to SQLite, and exposes a correlated typed client route that refetches after backend restart and retries the exact pending query after a dropped result. The focused eight-file matrix passes 99 tests; post-review provider/transport remedies pass 73 affected tests. The local one-worker repository gate passed 152 files with one intentional file skip, 1,315 tests, and 4 intentional test skips before the test-only review remedies; root lint, TypeScript, scoped formatting, and the production frontend build pass after them. Independent Terra review found no P0-P2 issue, and no remote runner or workflow was used.
- [x] Policy-controlled local Git fetch is committed and pushed at `0f9067b`, `e59ebae`, `5be0fbd`, `5cd266e`, and `12173f2`. The default-off global policy, compact workspace state, exact replay journal, process-safe leases, fixed hidden scheduler, manual dispatch, authentication boundary, thread erasure, ProtocolServer routes, typed `ArtisanClient`, reconnect replay, and renderer fixture are wired through Effect Services and Layers. The focused transport/protocol/fixture matrix passes 73 tests; the complete local one-worker gate passes 159 files with one intentional file skip, 1,353 tests, and 4 intentional test skips. Root lint, TypeScript, the production frontend build, scoped formatting, and the explicit native gate pass; aggregate formatting remains blocked only by the unrelated user-owned `backend-completion-matrix.md` hunk. Independent Terra review found no P0-P2 issue, and no remote runner or workflow was used.
- [x] Typed preview-target protocol, the durable SQLite core, ProtocolServer/runtime composition, and typed `ArtisanClient` integration are committed and pushed at `fe4edf5`, `ac4b5d5`, and `ad14f53`. Exact project/workspace scope, canonical journal replay, process-safe probe leases, expiry takeover, source-safe errors, replacement-generation fencing, a transactional 256-target bound, thread-erasure claim cleanup, dropped-receipt reconnect replay, live event delivery, cursor advancement, durable restart recovery, and exact probe deduplication are covered. The focused seven-file matrix passes 52 tests; complete local one-worker coverage passes 165 files with one intentional file skip, 1,377 tests, and 4 intentional test skips when the Windows real-Git suites use a 60-second diagnostic timeout. TypeScript, root Oxlint, scoped formatting, the production frontend build, and independent P0-P2 review pass. Production local health probing, binary asset routes, and the external-browser inspection lifecycle are the active preview slice; no remote runner or workflow is used.
- [ ] Continue the `GitProvider` milestone with right-pane UI and approval-bearing provider actions. Keep `GitLabProvider` explicitly deferred.
- [x] Durable local Git sessions and approval-bearing checkout are committed and pushed at `22ab8b6` and `c7f1329`. Effect Services and Layers discover the one visible worktree, record bounded status/diff/branch/head projections in SQLite, expose typed query/request/decision/replay client routes, fence controlled file mutations while checkout is unsettled, revalidate immediately before `git switch`, settle stale or interrupted operations after restart, and erase terminal thread-owned state without deleting another thread's current workspace projection. The real temporary-Git/SQLite/MessagePort harness passes alongside checkout concurrency, restart, mutation-fence, migration, and erasure coverage. Full local `pnpm run validate` passes 113 test files with 945 passing tests and 3 intentional skips; independent review found no P0-P2 issue. Remaining Git mutations and hosted-provider behavior are separate milestones.

## Generic Git Mutation Adapter

- [x] Replaced the checkout-only capability with the backend-private `GitMutation` prepare/execute/reconcile contract for branch creation, checkout, reset, exact clean, staged commit, merge/rebase lifecycle actions, fast-forward-only pull, and leased push. Plans, attempts, conflict anchors, source proofs, operation heads, and reconciliation outcomes are schema-validated and cryptographically bound.
- [x] Hardened the Node Git adapter around one pinned absolute Git executable, repository/git/object/index identity checks, replacement process environments, repository and worktree config rejection, disabled global/system config, safe command overrides, literal clean inventories, compare-and-swap ref transactions, bounded aggregate output, and truthful unknown outcomes after races or incomplete settlement.
- [x] Remote operations use exact approved endpoints through fresh Effect-scoped temporary bare transport repositories that share only the pinned object directory. File, HTTPS, and SSH endpoints are accepted; URL rewrites, divergent push URLs, embedded HTTPS credentials, executable config, unsafe protocols, and URL-scoped HTTP overrides fail closed.
- [x] Rebase state is derived from Git's live rebase directories. An inactive leftover `REBASE_HEAD` is inert and never deleted by Artisan; replacement-head and directory-first interleavings preserve foreign state or fail closed without applied attribution.
- [x] Generic mutation approvals, private plans, attempts, reconciliation evidence, and workspace claims are durable in SQLite. Exact command and decision replay, restart lease recovery, continuation-anchor binding, public redaction, terminal settlement, cross-writer fencing, and thread erasure fail closed through the `WorkspaceGitMutationRepository` Effect Service and Layer.
- [x] Applied push evidence is bound to the approved remote name, endpoint, and target branch. A nonzero Git exit can settle only when mutation-phase evidence and an independent remote observation prove the approved ref reached the approved object; precondition and failed settlement phases cannot claim success.
- [x] Full local `pnpm run validate` passes formatting, lint, TypeScript, the production frontend build, and 120 test files with 1,039 passing tests plus 3 intentional skips. The focused durable-mutation suites pass 60 tests, the repository matrix passes 14 tests, the opt-in native verifier passes locally, and the final independent P0-P2 review is clean.
- [x] Focused pushed checkpoints: `e9a317e feat: isolate process environments`, `4f3dc99 feat: generalize approved git mutations`, `dc55fdb test: cover approved git operations`, `315faad test: exercise git mutation races`, `262a2da test: harden git mutation boundaries`, `08a6ed1 feat: define durable git mutation records`, `a22036a fix: fence concurrent git mutations`, and `f300468 feat: persist git mutation approvals`.
- [x] The generic Git mutation coordinator prepares before approval, executes once behind a separate SQLite execution gate, persists attempt and reconciliation evidence, projects the resulting session before settlement, and recovers without re-executing. Owner leases and execution markers fence concurrent runtimes; an interrupted parent process is quarantined as `outcome_unknown` while retaining the workspace claim so a possibly orphaned Git child can never overlap another controlled mutation.
- [x] Pre-marker executing claims migrate conservatively into quarantine, request-journal privacy migration is covered, normal process interruption waits for child close, thread erasure composes mutation quiescing, and production runtime composition owns the coordinator and gate. The six focused suites pass 44 tests; full local `pnpm run validate` passes 121 test files with 1,054 passing tests and 3 intentional skips; independent P0-P2 review is clean.
- [x] Coordinator checkpoints `8a9d39b feat: fence git mutation execution` and `e1bdc80 feat: coordinate durable git mutations` are pushed, and local `HEAD` equals its upstream.
- [x] Typed generic Git mutation request/query/decision/query-result envelopes, backend routing, stable public errors, durable event replay, and renderer-safe `ArtisanClient` methods are published. Standalone and continuation inputs remain mutually exclusive, pending retries retain Schema-decoded detached intent, cross-thread queries fail as unavailable, and public approval projections never expose commit text or private Git evidence.
- [x] Protocol checkpoints `dfb53e0 feat: publish git mutation protocol` and `4d26a28 feat: expose git mutations to renderer` are pushed. Four focused suites pass 40 tests; full local `pnpm run validate` passes 121 test files with 1,057 passing tests and 3 intentional skips; independent P0-P2 review and remedy re-review are clean.
- [x] External parent-process Git mutation crash coverage is committed and pushed at `8ac37bb`. The harness launches a real Git credential-cache daemon through production `NodeProcessRunnerLive`, kills only its owning fixture parent, proves in-memory credential continuity, restarts the real SQLite/Effect coordinator, preserves the exact quarantined execution claim as the sole workspace claim, and forbids `Execute` or `Reconcile`. An Effect `SubscriptionRef`-backed `AwaitIdle` boundary proves the subsequent public `Respond` is what wakes a conflicting mutation dispatch. Cleanup is single-flight, deadline-bounded, path-verified, PID-signal-free, and private-socket-safe on Windows and POSIX. Five sequential process-crash repetitions pass; the six focused mutation suites pass 38 tests; full local `pnpm run validate` passes 122 test files with 1,059 passing tests and 3 intentional skips; fresh independent P0-P2 review is clean; no remote test runner is involved.

- [x] Updated the personal `sanders-skill` with a dedicated risk-based testing reference: every test must protect an observable promise against a plausible consequential regression, use the smallest stable behavioral boundary, and avoid arbitrary content snapshots, compiler duplication, source-string UI claims, and private implementation choreography. A read-only audit covered all 127 files under `.tests`. The strongest coverage is in persistence, concurrency, recovery, process lifecycle, protocol boundaries, and native filesystem behavior. The exact curated-content assertions in `.tests/data/catalogs.test.ts` were removed; remaining cleanup targets include the frontend `*-source.test.ts` suites, type-impossible `as never` input tests, serialized error-tag matching, exact private call/key snapshots, duplicated engine capability fixtures, and a wall-clock transcript timing assertion.

- [x] Added the private `@artisan/data` workspace module with 100 curated one-word Norwegian female agent names, 96 British female agent names, and 70 independently curated Artisan thinking words. Every entry now uses the uniform `{ value, weight }` shape and the small `1/2/4/6/8` rarity palette: `Muhammading` is the rarest thinking word at weight 1 (about 0.24% per draw), while user-favored `Esmebeth` and restored `Mina` use weight 8 (about 1.84% each in their catalogs). The Norwegian data retains `Linnea` and `Elise`, uses `Martha` instead of `Marta`, and keeps a deliberate balance of naturalized everyday and distinctively Norwegian names. The PRD and automatic catalog tests require shipped JSON datasets to stay at 100 entries or fewer and validate only the reusable weighted record contract, uniqueness, NFC, and single-word name policy; individual curated entries remain reviewable data rather than pinned test fixtures. Focused formatting, lint, catalog tests, and the current root validation gate pass. Claude spinner references remain taste/provenance only because the published compilations are unlicensed or non-commercial and the PRD forbids blindly shipping them.

- [x] Durable workspace replacement approval integration is committed and pushed at `13aa893`. Policy-gated requests, private exact diffs, typed query/respond routes, decision receipts, immutable replay, coordinator wake/recovery, denied cleanup, terminal evidence settlement, thread-erasure fences, and canonical operation/authority/event attribution are composed through Effect Services and Layers. Fingerprint, timestamp, command-id, mixed-intent, lifecycle, corruption, restart, and two-runtime races fail closed without exposing source. Focused approval integration passes 63 tests; full local `pnpm run validate` passes 101 files with 878 passing tests plus 3 intentional skips; independent P0-P2 review is clean.

- [x] Immutable workspace diff generation, persistence, recovery, validation, and migration safety are committed and pushed at `07033aa`; the public query route and truthful unavailable/corrupt errors are committed and pushed at `3dadcaa`.

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
- [x] Controlled UTF-8 reads and recoverable attributed replacements are composed into the production backend runtime at `566ceb2`, with real SQLite/repository/authority/payload/snapshot/evidence coverage at `47c28e8` and `a3c9bbd`.
- [x] The real composition harness proves accepted replacement, exact committed retry after a terminal run, applied/finalization recovery after restart without a second replacement, terminal changed replay without a projection/evidence/snapshot, portable empty-registry denial, full fake-native option forwarding, and scoped acquisition/release counts.
- [x] Review and guarded rollback execution are committed at `e68c484`, `72cd1af`, and `f8ea18c`. Rollback reuses the original replace's pinned authority after its run terminates, binds all I/O to event-validated source data, and never issues a store for duplicate or rejected outcomes.
- [x] Real SQLite composition proves review/rollback success, applied restart recovery, committed cleanup recovery, native rejection after payload staging, byte-free tombstones, snapshot preservation on rejection, and exact duplicates without a second native replacement.
- [x] Typed public workspace list/read/replace/review/rollback protocol routes and the renderer-safe `ArtisanClient` surface are committed at `c072a82` and `a333d78`.
- [x] Durable cross-process workspace-evidence idempotency is committed at `3e6302e`. New events use a nullable journal idempotency key with a SQLite unique index, legacy exact duplicates remain immutable and resolve to their earliest event, conflicting legacy intent fails closed, and a prior-schema migration fixture proves upgrade safety.
- [x] Synchronized two-runtime replacement convergence, preflight publication races, native `Changed` reconciliation, committed cleanup, and thread-erasure overlap are committed at `f5ba7e1`.

## Completed Immutable Workspace Diffs

- [x] `WorkspaceChangeDiffService` prepares strict UTF-8 unified patches before any snapshot or native replacement, verifies both source identities, copies caller bytes, and applies V1 source, line, patch-byte, rendered-line, and deterministic edit-distance limits. The synchronous jsdiff path is bounded by `min(16_384, floor(4_000_000 / max(1, before_lines + after_lines)))`; the worst accepted 1,000-by-1,000-line full rewrite has a 750 ms local regression guard.
- [x] Replacement preparation failure durably rejects and settles the operation before filesystem mutation. Applied/finalization recovery reconstructs the exact artifact from retained private payload bytes, while projection, immutable patch, journal command/event, and committed lifecycle transition are inserted atomically.
- [x] Stored patches are SHA-256 and byte-count bound to operation/projection identities, path, thread, workspace, command, format version, and context. jsdiff parse-and-canonical-format validation handles quoted Unicode paths and fails malformed, missing, duplicate, or corrupt state closed without exposing source or patch bytes in errors.
- [x] Prior committed projections upgrade to explicit `legacy_unavailable`. The migration uses inline checked `ADD COLUMN` statements instead of Drizzle's generated parent-table rebuild, preserving existing authority/payload children while migrations execute transactionally; fresh foreign keys, legacy preservation, and invalid-state checks are covered.
- [x] Thread erasure explicitly removes private diff artifacts. Public diff queries return a V1 immutable result, `workspace.diff_unavailable` for missing/legacy state, `workspace.unavailable` for erased state, and `workspace.invariant_failed` for corruption.
- [x] Focused verification passed 8 files with 115 tests. Full local `pnpm run validate` passed formatting, Oxlint, TypeScript, the production Svelte build, and 98 test files with 837 passing tests plus 3 intentional skips. Independent P0-P2 review and re-review are clean after fixing quoted-path validation and reducing/proving the synchronous work ceiling.

## Completed Concurrent Workspace Mutation Convergence

- [x] `WorkspaceChangeRepository.ReconcileChanged` resolves preflight and native changed observations transactionally against claimed, applied, committed, rejected, and exactly staged durable state.
- [x] Replace and rollback callers resume the exact private payload after another runtime publishes, finalize an applied operation, replay a committed event, or settle a terminal rejection without issuing a second native write.
- [x] Evidence publication is process-safe through one generic nullable journal idempotency key; SQLite arbitrates stale-read races while bounded Effect retry re-reads the canonical event.
- [x] The upgrade migration preserves legacy journal rows and stream continuity. Multiple exact legacy evidence rows resolve logically to the earliest event, while any conflicting row remains a hard `WorkspaceEvidenceConflict`.
- [x] Deterministic gates prove both runtimes read stale state before publication, exact replacements converge through preflight and native changed races, committed cleanup emits one evidence event, and erasure cannot bypass unsettled mutation fences.
- [x] Both synchronized replacement scenarios and both evidence/cleanup scenarios passed ten consecutive stress iterations.
- [x] Fresh independent P0-P2 review is clean after replacing the unsafe legacy partial-index migration with the nullable idempotency-key policy and upgrade fixture.
- [x] An isolated staged snapshot passed `pnpm run validate`: formatting, lint, TypeScript, production frontend build, and 95 test files with 799 passing tests plus 3 intentional skips.
- [x] Focused commits: `3e6302e fix: make workspace evidence idempotent` and `f5ba7e1 fix: reconcile concurrent workspace replacements`.

## Completed Controlled Workspace Read And Replace Composition

- [x] `WorkspaceFileServiceLive` is built at the runtime composition root from explicit Effect Services and Layers; dependency Layers remain private to service construction while the existing repositories stay independently available from the backend runtime.
- [x] Production reads use only the registry's restricted reader. Production replacements use atomic run authority, the pinned bounded mutation capability, strict UTF-8 identities, private payload/snapshot recovery, native conditional replacement/finalization, durable change projection, and attributed evidence.
- [x] The deterministic fake native module validates the complete replacement/finalization option set and preserves receipt state across runtime recreation, while real SQLite migrations and production repositories own every durable transition.
- [x] Restart tests terminalize the original base run before exact duplicate, applied, and rejected retries. Duplicate/recovery paths issue no second native replacement, and rejected retry remains source-free and side-effect free.
- [x] Local verification passed TypeScript, scoped Oxfmt/Oxlint, 24 focused service/authority/composition tests, and the full suite with 90 test files, 726 passing tests, and 3 intentional skips.
- [x] Fresh independent P0-P2 re-review is clean after strengthening durable settlement assertions, native interface fidelity, runtime disposal, portable read denial, and acquisition/release checks.
- [x] Focused commits: `566ceb2 feat: compose controlled workspace files`, `47c28e8 test: compose workspace file service`, and `a3c9bbd test: harden workspace file recovery harness`.

## Completed Controlled Review And Rollback Execution

- [x] `WorkspaceFileService` now owns idempotent review and complete rollback recovery behind the same small Effect Service interface as controlled reads and replacements.
- [x] Rollback authority strictly decodes the source operation/projection, exact-replays the original committed replace against its journal event, revalidates graph file scope, fences thread erasure, and returns event-validated source path/identities with no store on duplicate or rejected outcomes.
- [x] Rollback claims authority before reading content and uses only admission-bound source data for snapshot reads, bounded file reads, payload identity, native replacement/finalization, and anonymous frontend evidence.
- [x] The durable order is payload Stage, conditional native Replace, MarkApplied, native Finalize, CommitRolledBack, snapshot Consume, anonymous filesystem evidence, MarkEvidenceRecorded, then payload Consume. Every boundary resumes idempotently after restart.
- [x] Rejected rollback preserves the original snapshot. A native `Changed` after payload staging produces a consumed private row whose byte, count, and hash columns are all null; exact retry receives no store and performs no second replacement.
- [x] A deterministic SQLite trigger proves recovery when CommitRolledBack succeeds but snapshot consumption aborts: restart duplicate cleanup consumes snapshot/payload, records evidence, and never reruns native mutation.
- [x] Local `pnpm run validate` passes formatting, lint, TypeScript, the frontend production build, and 92 test files with 755 passing tests plus 3 intentional skips. The explicit local native addon gate also passes and no remote test workflow is retained.
- [x] Fresh independent P0-P2 review is clean after closing jointly forged source-state and pre-claim projection TOCTOU findings.
- [x] Focused commits: `e68c484 feat: authorize pinned workspace rollbacks`, `72cd1af feat: review and rollback workspace changes`, and `f8ea18c test: prove workspace rollback recovery`.

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

## Hosted Git Provider

- [x] Provider-neutral hosted Git begins with the canonical `GitProvider` Effect Service, bounded Schema ADTs, safe errors, truthful capabilities, and an explicit `GitProviderRegistry`. Static hosts win exactly, authenticated enterprise hosts resolve dynamically, unknown hosts remain unsupported, and ambiguous or malformed registrations fail closed. Checkpoint `9c08989` is pushed.
- [x] `GitHubProvider` is the sole live V1 adapter. It discovers a pinned shell-free `gh` executable, inspects version and multi-host/multi-account authentication without requesting or retaining tokens, and reads bounded account, organization, and search pages through GraphQL. Canonical continuations are bound to provider, host, active account, scope, and page size; the serving account is rechecked after each query.
- [x] Repository projections retain only canonical identity, visibility, archive/default-branch/permission state, safe URLs, and minimal native attribution. Responses exceeding the requested page bound, changing accounts, escaping the selected clone or web host, returning malformed data, or leaking provider output fail with safe canonical errors. Missing, incompatible, signed-out, timeout, permission, rate-limit, network, and server states remain distinct.
- [x] Portable runtimes use an empty registry while desktop composition owns the optional live GitHub Layer. Focused provider tests pass 21 tests with the ordinary live test skipped; the explicit authenticated `ARTISAN_RUN_GITHUB_PROVIDER_LIVE=1` smoke passes locally. Full `pnpm run validate` passes formatting, Oxlint, TypeScript, the production frontend build, and 127 test files with 1,080 passing tests plus 4 intentional skips. Independent review found one cross-host URL P2, the remedy and hostile-response tests are committed at `5c455d2`, and focused re-review is clean.
- [x] Hosted projects are durably registered against one canonical visible checkout with provider identity and thread attachment at `60df1ea` and `2cc9a48`. Thread affinity can attach an unambiguous registered project without creating a hidden checkout, branch, worktree, or commit.
- [x] GitHub clone execution is committed and pushed at `7a0d068`. The desktop Layer pins both `gh` and Git, accepts only one canonical empty child of the configured projects root, obtains the selected account token child-to-child, isolates ambient home, netrc, and Git credentials, clones the exact HTTPS repository, and verifies destination identity, worktree shape, origin URLs, private receipt, and repository/account identity before returning. The destination pin, private home/template, and receipt remain scoped through the remote postcheck and a final local verification; every uncertain mutation outcome is non-retryable `outcome_unknown`.
- [x] Focused provider suites pass 40 tests with one opt-in live test skipped. Full local Oxlint, TypeScript, production frontend build, and Vitest gates pass 128 test files with 1,112 passing tests and 4 intentional skips; targeted Oxfmt passes for the provider slice. Two independent P0-P2 reviews and the credential-harness re-review are clean. The remaining transport-level test gap is a real HTTPS/libcurl endpoint; current coverage proves exact replacement-environment isolation plus real Git credential-helper behavior against seeded ambient `.netrc` and credential-store sources.
- [x] Public hosted-clone request, approval query/respond, query-result, lifecycle event, and replay codecs are committed and pushed at `0f9c768`. Repository selections and approval links are cross-field bound to one canonical provider/host, private execution evidence is rejected at the complete outbound envelope boundary, 20 focused codec tests pass, and independent P0-P2 review is clean.
- [x] Approval-time hosted-clone destinations are bound to one already-visible empty direct child at `ba813ca`. The Effect service persists canonical root and destination device/inode proof, retains scoped handles across execution, and revalidates after execution; `GitHubCli` independently checks the same proof before any Git spawn. Root/destination replacement, proof mismatch, provider composition, and failure semantics pass 48 focused tests with one intentional live skip; TypeScript, targeted Oxfmt/Oxlint, and final independent P0-P2 review are clean.
- [ ] This matrix slice remains partial. Next add the durable approval-bearing clone coordinator and recovery path that reserves one visible destination, persists attempts and unknown outcomes, registers the successful checkout, and attaches the requesting thread without re-executing after restart. Then add safe fetch, stale-head-safe review/CI projections, external waits, public protocol/UI routes, replay, and approval-bearing provider mutations. `GitLabProvider` remains deferred and unsupported.

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
- [x] Add controlled filesystem read/replace execution and real restart/recovery composition through the production Layer graph.
- [x] Add review/rollback execution with pinned source authority, full recovery, anonymous evidence, and private-byte settlement.
- [x] Add public read/replace/list/review/rollback and approval query/respond protocol commands with typed client methods.
- [x] Add one-visible-worktree inventory, durable Git session/change projections, an approval-bearing checkout command, and public query/request/decision/replay protocol commands.
- [ ] Add the remaining explicit approval-bearing local Git mutations: branch creation, reset, clean, commit, merge, rebase, pull, and push.
- [ ] Complete the canonical surface taxonomy: Work, Time, Guidance, Routines, Capabilities, Processes, Changes, Permissions, and native actions.
- [ ] Add explicit intake risk classification, assumptions, usage aggregation, and workspace conflict/review handling.

### Preview And Electron

- [ ] Add the production local preview health probe, binary asset routes, and external-browser inspection lifecycle.
- [ ] Build the real Electron main/utility/renderer bootstrap around the existing shell-neutral MessagePort transport.
- [ ] Add packaged-process restart/equivalence tests and single-instance ownership.
- [ ] Build the SvelteKit/Vite+ frontend and Monaco editor after backend contracts are sufficiently stable.

### Deep Harness And Release

- [ ] Add deterministic projection rebuild from the event ledger and equivalence tests.
- [ ] Add generated property/state-machine scenarios spanning retries, exits, reconnects, rebuilds, and concurrent agents.
- [x] Add one deep public-protocol workspace/Git/SQLite integration scenario with a real temporary repository, restart recovery, reconnect replay, and complete handle cleanup.
- [ ] Extend the deep workspace/Git/SQLite scenario through a fake Engine and deterministic projection rebuild.
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

## Frontend Design Planning

- [x] Added and independently reviewed `frontend-checkpoints.md`, a dependency-ordered frontend build plan that marks slices as ready, fixture-ready, blocked, or moving with the backend. It keeps `ArtisanClient` as the only production renderer gateway and does not claim integration where only protocol envelopes or fixtures exist.
- [x] Added and independently reviewed `barekey-design-language.md`, a source-backed inspection of `usebarekey/barekey` at commit `2811067`. Barekey is treated as visual research only because the public repository has no detected license; Artisan must not copy its code, bundled fonts, logos, or other assets without permission.
- [x] Completed the contract map and initial `modules/frontend` scaffold. Live workspace-change integration remains gated on the backend loop recording the controlled file/change service, typed client, production composition, and verification as complete.

## Frontend Scaffold Milestone

- [x] Added a SvelteKit/Vite+ renderer module with Svelte Effect Runtime 4, Effect 4, Tailwind CSS, shadcn-svelte conventions, mode-watcher, the repository's composer stack, and static production output under `.dist/frontend`.
- [x] Added a source-tested frontend contract registry that classifies every initial interaction as live, fixture, backend-moving, or blocked and keeps command identity, correlation, retries, and subscriptions owned by `ArtisanClient`.
- [x] Added Artisan-owned dark/light semantic tokens, density and reduced-motion tokens, the canonical OFL Artisan Neo variable WOFF2, Cal Sans for the `-0.05em` wordmark, and JetBrains Mono for code.
- [x] Added the fixture-first three-pane editor shell with the `272px minmax(720px, 1fr) 340px` desktop grid, independent pane scrolling, separate editor/chat/orchestrator modes and file tabs, a dense session pane, right-first responsive collapse, a left rail, and keyboard-dismissable edge overlays.
- [x] All new interactive behavior is SER-owned and composed with `Effect.gen`; the frontend source contains no direct Effect runner.
- [x] Verification: `pnpm --filter @artisan/frontend run build` passed; three frontend architecture files passed 15 tests; the full `pnpm run validate` passed 87 files with 711 tests and 3 intentional skips.
- [x] Independent frontend review findings were resolved before the milestone: responsive controls no longer cover mode buttons, duplicated responsive pane instances share shell-owned state and use unique ARIA IDs, file tabs no longer advertise a fake close action, and the button groups use honest keyboard semantics.
- [x] Root `pnpm run validate` now includes the production frontend build, so future SER/Svelte compiler regressions fail the monorepo gate.
- [x] Frontend scaffold checkpoint `9312359` was pushed to `origin/codex/backend-services`; local `HEAD` and the upstream branch both resolved to `9312359a147e97dc011ec8c6b4b704ab0d14683b` immediately after the push.
- [ ] `better-svelte-check` is not available as a portable published package, so its checkpoint remains open; production build, TypeScript, import-boundary, and source-layout gates are green in the meantime.
- [x] Added the test-only typed `ArtisanClient` fixture Layer and complete visual-fixture route. Persisted pane preferences and browser-level layout/accessibility coverage remain the next frontend foundation slices.

## Frontend Fixture Runtime And Visual Lab

- [x] Added an explicitly fixture-only `Layer.succeed(ArtisanClient, ...)` that satisfies the complete current renderer-safe client contract with deterministic protocol-shaped threads, orchestration, terminals, settings, streams, receipts, and not-found failures. Production source tests prove that no composition root silently selects it.
- [x] Fixture values decode through the public Effect Schemas; the guidance fixture's byte count and SHA-256 are verified against its actual content.
- [x] Added `/visual-fixtures` as a compositional, renderer-only lab covering typography, controls, inputs, disclosure actions, tooltips, navigation groups, badges, status rows, empty/loading states, banners, permission prompts, diffs, terminal chrome, and explicit unavailable states.
- [x] The lab provides local dark/light, high-contrast, reduced-motion, whole-interface 200% scale, and long-label stress modes. These are design affordances, not a substitute for the still-open browser zoom and accessibility gate.
- [x] Independent review findings were resolved: every Tabler component is imported under its rendered name, the popup uses honest disclosure semantics, closing restores focus only while open, and all visual radii, shadows, focus rings, and ambient durations use shared semantic tokens.
- [x] All fixture behaviors remain `Effect.gen` programs, streams use Effect's Stream API for the concrete streaming contract, SER owns visual-lab execution, and no direct Effect runner exists in the new source or tests.
- [x] Focused verification: the frontend production build passed, all five frontend test files passed 26 tests, and the fixture/runtime-focused suite passed 11 tests after review fixes.
- [x] Full `pnpm run validate` passed after the backend owner formatted its concurrent work: formatting, lint, root TypeScript, the production SER/Svelte build, and 90 test files passed with 732 tests and 3 intentional skips.
- [x] Frontend fixture/runtime checkpoint `c8011a0` was pushed to `origin/codex/backend-services`; local `HEAD` and upstream both resolved to `c8011a0e366488538e262daea2cd0a64120dcb9e` immediately after the push.

## Frontend Shell Presentation Preferences

- [x] Added a versioned `ShellPresentationPreferences` Effect Service backed by Effect's browser `KeyValueStore` Layer; malformed or unsupported persisted state repairs to safe defaults.
- [x] Browser storage acquisition defects fall back to Effect's memory `KeyValueStore`, and an Effect-owned regression test proves startup remains available without `localStorage`.
- [x] Explicit desktop left/right collapse and expand actions persist, while responsive overlay open, Escape, and backdrop closure remain ephemeral.
- [x] Desktop collapse uses the 56px left rail and removes the right column; responsive breakpoints ignore desktop collapse state without exposing controls that appear to do nothing.
- [x] The frontend production build, focused lint, diff check, and 16 preference/layout tests pass. Independent P0-P2 review and re-review are clean.
- [x] Full `pnpm run validate` passed after the backend loop settled: formatting, lint, root TypeScript, the production SER/Svelte build, and 92 test files passed with 754 tests and 3 intentional skips.
- [x] Shell-presentation checkpoint `14a7d9d` was pushed to `origin/codex/backend-services`; local `HEAD` and upstream both resolved to `14a7d9d9d83e98a82fc712a4419471b07423b659` immediately after the push.

## Frontend Workspace And Tab Model

- [x] Added an immutable, renderer-owned workspace model for editor/chat/orchestrator mode state, user-owned file tabs, recents, changed files, and bounded overflow without introducing a capability Service or mutable singleton.
- [x] The model uses explicit ADTs for preview/open/pinned ownership, file/diff content, clean/dirty state, agent-change badges, tab mutation outcomes, and revision/incarnation-bound dirty-close consent.
- [x] Preview replacement, explicit-open/double-click/edit promotion, monotonic pinning, dirty close/reopen races, injective diff identities, changed-file isolation, badge replacement, mode preservation, recent ordering, active overflow, successor selection, and duplicate opens pass 19 focused Effect tests.
- [x] Independent P0-P2 review and two re-review rounds are clean after closing preview ownership, dirty-consent, stale-diff, pinned-demotion, diff-edit, tab-incarnation, and composite-ID findings.
- [x] Brought the fixture `ArtisanClient` forward to the backend's newly published list/read/replace/review/rollback workspace methods, with protocol-shaped fixture data and Effect-owned contract tests; the combined model/fixture suites pass 24 tests and root TypeScript is green.
- [x] Integrated the model into the main pane with simultaneous open/pinned/dirty/diff/preview/agent-change states, exact dirty-close confirmation, truthful before/after fixture diffs, breadcrumbs, repeatable recent/changed/overflow navigation, and an active-first horizontally scrollable tab strip.
- [x] Added Ctrl/Cmd+P fixture quick open with search, arrow/Enter/Escape/Tab handling, focus containment and prior-focus restoration across close/reopen races, keyboard-selection reveal, transitions.dev modal motion, and reduced-motion fallback.
- [x] Text Editor, Chat, and Orchestrator retain real viewport positions plus active file, draft, and selected node through mode switches. All UI behavior remains SER-owned and no direct Effect runner, timer, Promise, or fake backend command was introduced.
- [x] The focused model/UI/fixture suite passes 32 tests, the production frontend build passes, and final independent P0-P2 re-review is clean after truthful-diff, physical-overflow, state-label, select-reset, focus-return, and keyboard-reveal fixes.
- [ ] Browser-level layout and accessibility verification remains open because no development server was requested; source/build evidence cannot fully prove physical scrolling, focus containment/restoration, native select reset, or narrow diff layout.
- [x] Full `pnpm run validate` passed from an isolated detached snapshot containing current `HEAD` plus exactly this staged frontend checkpoint: formatting, lint, root TypeScript, the production SER/Svelte build, and 95 test files passed with 787 tests and 3 intentional skips. The backend loop's unrelated unfinished files were excluded without modification.
- [x] User-owned workspace/tab checkpoint `08e7eca` was pushed to `origin/codex/backend-services`; local `HEAD` and upstream both resolved to `08e7ecac6e2745bcf40f16977182840dda723400` immediately after the push.

## Frontend Typeface Prototype

- [x] Built `Artisan Neo` v0.1 as an OFL-licensed derivative of the verified Inter 4.1 release. This is the usable application and visual prototype; it is not yet an original proprietary typeface.
- [x] The primary variable font preserves Inter's `wght` 100–900 and `opsz` 14–32 axes, 2,937 glyphs, 2,852 mapped codepoints, OpenType features, and language coverage. The build also emits WOFF2 and eight correctly named static instances.
- [x] The primary build is deterministic. After the packaging audit fixes, two clean builds of `ArtisanNeo-Variable.ttf` produced SHA-256 `028839C365C896CD7202FD100157A293B231B21AAA049E28A6D81EC497C7678D`; every glyph has a variation record, WOFF2 reopens successfully, all variable/static PostScript names are Artisan-owned, and static weight/style metadata was checked.
- [x] The primary builder verifies both the cached official Inter 4.1 archive and extracted variable-font digests before using upstream data, so a changed cached source cannot silently alter the derivative.
- [x] Kept `modules/artisan-font/src/build-font.py` only as a rejected from-scratch construction experiment. Its stencil-like texture did not meet the neo-grotesk visual brief and must not be presented as the product font.
- [ ] The next type-design pass should preserve the stable text texture while redrawing `a`, `e`, `g`, `r`, `s`, `R`, `Q`, `&`, and `@`, then improve `I`/`l`/`1` differentiation. Until that pass, describe the family as Artisan's modified Inter prototype.
- [x] Added five reproducible comparison variants—Edge, Soft, Round, Grotesk, and Wink—as topology-preserving coordinate experiments built directly from the verified Inter source plus the Artisan base treatment. Every variant retains the two axes, glyph order, cmap, variation coverage, features, and OFL metadata; corner/default instances reopen successfully and consecutive amplified TTF/WOFF2 builds are byte-identical.
- [x] The variant pipeline materializes IUP-implied deltas, transforms every point through a fixed affine plan, re-optimizes tuples, constrains expanded outlines inside deliberate sidebearings, recomputes `hmtx`/`hhea`, and distinguishes derivative producer metadata from upstream attribution. An independent corner-coordinate oracle reports at most two units of ordinary rounding drift at `opsz=32`/weight endpoints, with no remaining P0-P2 finding.
- [x] Added the hosted Barekey `/artisan-neo` variant lab with one shared specimen, synchronized `wght`/`opsz`/size/tracking controls, honest same-setting rows, focused UI/display proofs, and individual WOFF2 downloads. The variants were visually amplified after the first comparison proved too subtle; base Neo remains unchanged.
